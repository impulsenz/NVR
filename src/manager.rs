//! Owns the configuration and the running cameras. Changes made from the
//! web UI go through here: they are validated, saved to disk, and applied by
//! restarting only what is affected.

use crate::ai::AiHandle;
use crate::buffer::{self, EventBuffer};
use crate::config::{
    AiConfig, CameraConfig, CloudConfig, Config, DashboardConfig, DisplayConfig, HardwareConfig,
    LiveConfig, StorageConfig,
};
use crate::detect::Detector;
use crate::events::EventStore;
use crate::live::{Live, Quality};
use crate::recorder::{RecSpec, Recorder};
use crate::relay::Relay;
use crate::storage::{self, CameraUsage};
use crate::sysmon::Monitor;
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::info;

/// Settings shared by all cameras. Changing any of them restarts the cameras.
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeEnv {
    pub ffmpeg_path: String,
    pub segment_seconds: u64,
    pub live: LiveConfig,
    pub hardware: HardwareConfig,
    pub storage_dir: PathBuf,
}

impl RuntimeEnv {
    fn from(cfg: &Config) -> Self {
        Self {
            ffmpeg_path: cfg.server.ffmpeg_path.clone(),
            segment_seconds: cfg.storage.segment_seconds,
            live: cfg.live.clone(),
            hardware: cfg.hardware.clone(),
            storage_dir: cfg.storage.dir.clone(),
        }
    }
}

pub struct CameraRuntime {
    pub cfg: CameraConfig,
    pub dir: PathBuf,
    /// 24/7 recording (main or sub-stream), if the mode has one.
    pub recorder: Option<Arc<Recorder>>,
    /// Full-quality pre-event buffer recorder, in event-clip modes.
    pub buffer_recorder: Option<Arc<Recorder>>,
    pub detector: Mutex<Option<Arc<Detector>>>,
    pub live_sub: Arc<Live>,
    pub live_main: Arc<Live>,
    pub events: Arc<EventStore>,
    /// Shared camera connections (the same one twice when there is no sub-stream).
    pub relay_main: Arc<Relay>,
    pub relay_sub: Arc<Relay>,
    cancel: CancellationToken,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl CameraRuntime {
    fn start(
        cfg: CameraConfig,
        dir: PathBuf,
        env: Arc<RuntimeEnv>,
        ai: Arc<AiHandle>,
        events: Arc<EventStore>,
    ) -> Arc<Self> {
        let cancel = CancellationToken::new();
        let mut tasks = Vec::new();
        let segment = cfg.segment_seconds(env.segment_seconds);

        // One connection per camera stream, shared by everything below.
        let relay_main = Relay::new(
            &cfg.name,
            "main stream",
            &cfg.url,
            &cfg.rtsp_transport,
            &env.ffmpeg_path,
            cancel.clone(),
        );
        let relay_sub = if cfg.has_sub() {
            Relay::new(
                &cfg.name,
                "sub-stream",
                &cfg.live_url,
                &cfg.rtsp_transport,
                &env.ffmpeg_path,
                cancel.clone(),
            )
        } else {
            relay_main.clone()
        };

        let recorder = cfg.record_mode.continuous_stream().map(|use_sub| {
            let spec = RecSpec {
                label: if use_sub && cfg.has_sub() {
                    "recording (sub-stream)"
                } else {
                    "recording"
                },
                relay: if use_sub {
                    relay_sub.clone()
                } else {
                    relay_main.clone()
                },
                dir: dir.clone(),
                prefix: cfg.id.clone(),
                segment_seconds: segment,
            };
            Recorder::new(cfg.clone(), spec, env.clone(), cancel.clone())
        });
        let (buffer_recorder, event_buffer) = if cfg.record_mode.event_clips() {
            let buf = EventBuffer::new(
                &dir,
                &cfg.name,
                &env.ffmpeg_path,
                cfg.detect.pre_seconds,
                events.clone(),
            );
            let spec = RecSpec {
                label: "event buffer",
                relay: relay_main.clone(),
                dir: buf.dir.clone(),
                prefix: buffer::PREFIX.into(),
                segment_seconds: buffer::BUFFER_SEGMENT_SECONDS,
            };
            tasks.push(tokio::spawn(buf.clone().run(cancel.clone())));
            (
                Some(Recorder::new(
                    cfg.clone(),
                    spec,
                    env.clone(),
                    cancel.clone(),
                )),
                Some(buf),
            )
        } else {
            (None, None)
        };
        for r in recorder.iter().chain(buffer_recorder.iter()) {
            tasks.push(tokio::spawn(r.clone().run()));
        }

        let live_main = Live::new(
            cfg.clone(),
            env.clone(),
            Quality::Main,
            relay_main.clone(),
            cancel.clone(),
        );
        let live_sub = if cfg.has_sub() {
            Live::new(
                cfg.clone(),
                env.clone(),
                Quality::Sub,
                relay_sub.clone(),
                cancel.clone(),
            )
        } else {
            live_main.clone()
        };

        let detector = (cfg.detection_enabled()).then(|| {
            Detector::new(
                cfg.clone(),
                env.clone(),
                ai,
                events.clone(),
                event_buffer,
                relay_sub.clone(),
            )
        });
        if let Some(d) = &detector {
            tasks.push(tokio::spawn(d.clone().run(cancel.clone())));
        }

        Arc::new(Self {
            dir,
            recorder,
            buffer_recorder,
            detector: Mutex::new(detector),
            live_sub,
            live_main,
            events,
            relay_main,
            relay_sub,
            cancel,
            tasks: Mutex::new(tasks),
            cfg,
        })
    }

    /// Stop recording (finishing the current files), detection and live view.
    async fn stop(&self) {
        self.cancel.cancel();
        let tasks: Vec<_> = self.tasks.lock().unwrap().drain(..).collect();
        let _ = tokio::time::timeout(
            Duration::from_secs(15),
            futures_util::future::join_all(tasks),
        )
        .await;
    }

    pub fn detector(&self) -> Option<Arc<Detector>> {
        self.detector.lock().unwrap().clone()
    }

    pub fn live(&self, q: Quality) -> Arc<Live> {
        match q {
            Quality::Sub => self.live_sub.clone(),
            Quality::Main => self.live_main.clone(),
        }
    }

    pub fn pause(&self) {
        for r in self.recorder.iter().chain(self.buffer_recorder.iter()) {
            r.pause();
        }
    }

    pub fn resume(&self) {
        for r in self.recorder.iter().chain(self.buffer_recorder.iter()) {
            r.resume();
        }
    }
}

/// Fields of the settings forms; `None` sections are left unchanged.
#[derive(Debug, Deserialize, Default)]
pub struct SettingsUpdate {
    pub server: Option<ServerUpdate>,
    pub storage: Option<StorageConfig>,
    pub live: Option<LiveConfig>,
    pub display: Option<DisplayConfig>,
    pub hardware: Option<HardwareConfig>,
    pub ai: Option<AiConfig>,
    pub cloud: Option<CloudConfig>,
}

#[derive(Debug, Deserialize)]
pub struct ServerUpdate {
    pub name: String,
    pub listen: String,
    pub username: String,
    /// Empty = keep the current password.
    #[serde(default)]
    pub password: String,
    pub ffmpeg_path: String,
}

/// Disk space and how long recordings will fit.
#[derive(Debug, Clone, Serialize, Default)]
pub struct UsageReport {
    pub updated: Option<DateTime<Local>>,
    pub cameras: Vec<CameraUsageView>,
    pub locations: Vec<Location>,
    pub bytes_per_hour: f64,
    pub bytes_per_day: f64,
    pub used_bytes: u64,
    /// Space the recordings may use in total (limit, or disk space).
    pub capacity_bytes: Option<u64>,
    /// How many days of recordings fit at the current rate.
    pub days_capacity: Option<f64>,
    /// Days until the space is full, from now.
    pub days_until_full: Option<f64>,
    /// "max storage" or "disk space".
    pub limited_by: String,
    pub retention_days: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CameraUsageView {
    pub name: String,
    #[serde(flatten)]
    pub usage: CameraUsage,
    pub bytes_per_day: f64,
    pub retention_days: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Location {
    pub path: PathBuf,
    pub cameras: Vec<String>,
    pub free_bytes: u64,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub bytes_per_day: f64,
    pub days_until_full: Option<f64>,
}

pub struct Manager {
    pub cfg_path: PathBuf,
    pub data_dir: PathBuf,
    config: RwLock<Config>,
    cameras: RwLock<Vec<Arc<CameraRuntime>>>,
    stores: Mutex<HashMap<String, Arc<EventStore>>>,
    edit: tokio::sync::Mutex<()>,
    pub started: Instant,
    /// The address actually bound at startup (listen changes need a restart).
    pub bound_listen: String,
    pub ai: Arc<AiHandle>,
    pub monitor: Arc<Monitor>,
    usage: Mutex<UsageReport>,
    /// Temporary stream previews for the camera editor: token → (url, transport).
    pub previews: Mutex<HashMap<String, (String, String, Instant)>>,
}

impl Manager {
    pub fn new(cfg_path: PathBuf, data_dir: PathBuf, config: Config) -> Arc<Self> {
        let _ = std::fs::create_dir_all(data_dir.join("tmp"));
        let ai = Arc::new(AiHandle::new(config.ai.clone(), &data_dir));
        let m = Arc::new(Self {
            cfg_path,
            bound_listen: config.server.listen.clone(),
            monitor: Arc::new(Monitor::new(&data_dir)),
            data_dir,
            config: RwLock::new(config.clone()),
            cameras: RwLock::new(Vec::new()),
            stores: Mutex::new(HashMap::new()),
            edit: tokio::sync::Mutex::new(()),
            started: Instant::now(),
            ai,
            usage: Mutex::new(UsageReport::default()),
            previews: Mutex::new(HashMap::new()),
        });
        let cams: Vec<_> = config.cameras.iter().map(|c| m.start_camera(c)).collect();
        *m.cameras.write().unwrap() = cams;
        m
    }

    fn store(&self, cam: &CameraConfig, dir: &std::path::Path) -> Arc<EventStore> {
        let mut stores = self.stores.lock().unwrap();
        match stores.get(&cam.id) {
            Some(s) if s.dir() == dir.join("events") => s.clone(),
            _ => {
                let s = Arc::new(EventStore::open(dir));
                stores.insert(cam.id.clone(), s.clone());
                s
            }
        }
    }

    fn start_camera(&self, cam: &CameraConfig) -> Arc<CameraRuntime> {
        let cfg = self.config();
        let dir = cfg.camera_dir(cam);
        let _ = std::fs::create_dir_all(&dir);
        let store = self.store(cam, &dir);
        CameraRuntime::start(
            cam.clone(),
            dir,
            Arc::new(RuntimeEnv::from(&cfg)),
            self.ai.clone(),
            store,
        )
    }

    pub fn config(&self) -> Config {
        self.config.read().unwrap().clone()
    }

    pub fn cameras(&self) -> Vec<Arc<CameraRuntime>> {
        self.cameras.read().unwrap().clone()
    }

    pub fn camera(&self, id: &str) -> Option<Arc<CameraRuntime>> {
        self.cameras
            .read()
            .unwrap()
            .iter()
            .find(|c| c.cfg.id == id)
            .cloned()
    }

    pub fn event_stores(&self) -> HashMap<String, Arc<EventStore>> {
        self.stores.lock().unwrap().clone()
    }

    /// Validate and persist a new config, then make it current.
    fn commit(&self, new: Config) -> Result<()> {
        new.validate()?;
        new.save(&self.cfg_path)?;
        *self.config.write().unwrap() = new;
        Ok(())
    }

    pub async fn add_camera(&self, mut cam: CameraConfig) -> Result<CameraConfig> {
        let _g = self.edit.lock().await;
        let mut cfg = self.config();
        cam.name = cam.name.trim().to_string();
        cam.id = cfg.new_camera_id(&cam.name);
        cam.validate()?;
        cfg.cameras.push(cam.clone());
        self.commit(cfg)?;
        info!("added camera \"{}\"", cam.name);
        let rt = self.start_camera(&cam);
        self.cameras.write().unwrap().push(rt);
        Ok(cam)
    }

    pub async fn update_camera(&self, id: &str, mut cam: CameraConfig) -> Result<CameraConfig> {
        let _g = self.edit.lock().await;
        let mut cfg = self.config();
        let Some(slot) = cfg.cameras.iter_mut().find(|c| c.id == id) else {
            bail!("camera not found")
        };
        cam.id = id.to_string();
        cam.name = cam.name.trim().to_string();
        cam.validate()?;
        *slot = cam.clone();
        self.commit(cfg)?;
        info!("updated camera \"{}\"", cam.name);
        self.restart_camera(&cam).await;
        Ok(cam)
    }

    async fn restart_camera(&self, cam: &CameraConfig) {
        if let Some(old) = self.camera(&cam.id) {
            old.stop().await;
        }
        let rt = self.start_camera(cam);
        let mut cams = self.cameras.write().unwrap();
        if let Some(slot) = cams.iter_mut().find(|c| c.cfg.id == cam.id) {
            *slot = rt;
        } else {
            cams.push(rt);
        }
    }

    pub async fn delete_camera(&self, id: &str, delete_recordings: bool) -> Result<()> {
        let _g = self.edit.lock().await;
        let mut cfg = self.config();
        let before = cfg.cameras.len();
        cfg.cameras.retain(|c| c.id != id);
        if cfg.cameras.len() == before {
            bail!("camera not found");
        }
        cfg.dashboard.tiles.retain(|t| t.camera != id);
        self.commit(cfg)?;
        let rt = self.camera(id);
        self.cameras.write().unwrap().retain(|c| c.cfg.id != id);
        self.stores.lock().unwrap().remove(id);
        if let Some(rt) = rt {
            rt.stop().await;
            if delete_recordings {
                let dir = rt.dir.clone();
                tokio::task::spawn_blocking(move || std::fs::remove_dir_all(dir))
                    .await?
                    .context("deleting recordings")?;
            }
            info!("deleted camera \"{}\"", rt.cfg.name);
        }
        Ok(())
    }

    pub async fn update_dashboard(&self, d: DashboardConfig) -> Result<()> {
        let _g = self.edit.lock().await;
        let mut cfg = self.config();
        cfg.dashboard = d;
        self.commit(cfg)
    }

    /// Apply settings. Returns true if a program restart is needed.
    pub async fn update_settings(&self, up: SettingsUpdate) -> Result<bool> {
        let _g = self.edit.lock().await;
        let old = self.config();
        let mut cfg = old.clone();
        if let Some(s) = up.server {
            cfg.server.name = s.name.trim().to_string();
            cfg.server.listen = s.listen.trim().to_string();
            cfg.server.username = s.username.trim().to_string();
            cfg.server.ffmpeg_path = s.ffmpeg_path.trim().to_string();
            if cfg.server.username.is_empty() {
                cfg.server.password.clear();
            } else if !s.password.is_empty() {
                cfg.server.password = s.password;
            }
            if cfg.server.ffmpeg_path.is_empty() {
                cfg.server.ffmpeg_path = "ffmpeg".into();
            }
        }
        if let Some(s) = up.storage {
            cfg.storage = s;
        }
        if let Some(l) = up.live {
            cfg.live = l;
        }
        if let Some(d) = up.display {
            cfg.display = d;
        }
        if let Some(h) = up.hardware {
            cfg.hardware = h;
        }
        if let Some(mut a) = up.ai {
            // The form never receives the stored key; empty means "keep".
            if a.api_key.is_empty() {
                a.api_key = old.ai.api_key.clone();
            }
            cfg.ai = a;
        }
        if let Some(c) = up.cloud {
            cfg.cloud = c;
        }
        if cfg.storage.dir != old.storage.dir {
            std::fs::create_dir_all(&cfg.storage.dir)
                .with_context(|| format!("cannot create {}", cfg.storage.dir.display()))?;
        }
        self.apply(old, cfg).await
    }

    /// Replace the whole configuration (backup import).
    pub async fn import(&self, mut new: Config, keep_login: bool) -> Result<bool> {
        let _g = self.edit.lock().await;
        let old = self.config();
        if keep_login {
            new.server.username = old.server.username.clone();
            new.server.password = old.server.password.clone();
            new.server.listen = old.server.listen.clone();
        }
        // Stop cameras that no longer exist.
        for c in &old.cameras {
            if !new.cameras.iter().any(|n| n.id == c.id) {
                if let Some(rt) = self.camera(&c.id) {
                    rt.stop().await;
                }
                self.cameras.write().unwrap().retain(|r| r.cfg.id != c.id);
            }
        }
        self.apply(old, new).await
    }

    async fn apply(&self, old: Config, cfg: Config) -> Result<bool> {
        self.commit(cfg.clone())?;
        if cfg.ai != old.ai {
            self.ai.replace(cfg.ai.clone(), &self.data_dir);
        }
        let env_changed = RuntimeEnv::from(&cfg) != RuntimeEnv::from(&old);
        for cam in &cfg.cameras {
            let prev = old.cameras.iter().find(|c| c.id == cam.id);
            if env_changed || prev != Some(cam) {
                self.restart_camera(cam).await;
            }
        }
        if env_changed {
            info!("settings changed; cameras restarted");
        }
        if cfg.storage != old.storage {
            let stores = self.event_stores();
            tokio::task::spawn_blocking(move || storage::cleanup(&cfg, &stores));
        }
        Ok(self.config().server.listen != self.bound_listen)
    }

    pub async fn shutdown(&self) {
        let cams = self.cameras();
        futures_util::future::join_all(cams.iter().map(|c| c.stop())).await;
    }

    pub fn usage(&self) -> UsageReport {
        self.usage.lock().unwrap().clone()
    }

    fn compute_usage(cfg: &Config) -> UsageReport {
        let mut cams = Vec::new();
        for c in &cfg.cameras {
            let u = storage::camera_usage(&c.id, &cfg.camera_dir(c));
            cams.push(CameraUsageView {
                name: c.name.clone(),
                bytes_per_day: u.bytes_per_hour * 24.0,
                retention_days: if c.retention_days > 0 {
                    c.retention_days
                } else {
                    cfg.storage.retention_days
                },
                usage: u,
            });
        }
        // Group camera folders by the disk they are on.
        let mut locations: Vec<Location> = Vec::new();
        let default_root = cfg.storage.dir.clone();
        let mut roots: Vec<(PathBuf, Vec<&CameraUsageView>)> = vec![(default_root, vec![])];
        for c in &cams {
            let parent = c
                .usage
                .dir
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_default();
            match roots.iter_mut().find(|r| r.0 == parent) {
                Some(r) => r.1.push(c),
                None => roots.push((parent, vec![c])),
            }
        }
        for (path, list) in roots {
            let Some((free, total)) = crate::sysmon::disk_space(&path) else {
                continue;
            };
            let per_day: f64 = list.iter().map(|c| c.bytes_per_day).sum();
            let used: u64 = list.iter().map(|c| c.usage.used_bytes).sum();
            let room = free.saturating_sub((cfg.storage.min_free_gb * 1073741824.0) as u64);
            // Same disk as another location: merge.
            if let Some(l) = locations
                .iter_mut()
                .find(|l| l.total_bytes == total && l.free_bytes.abs_diff(free) < total / 100)
            {
                l.cameras.extend(list.iter().map(|c| c.name.clone()));
                l.used_bytes += used;
                l.bytes_per_day += per_day;
                l.days_until_full = (l.bytes_per_day > 0.0).then(|| room as f64 / l.bytes_per_day);
                continue;
            }
            locations.push(Location {
                path,
                cameras: list.iter().map(|c| c.name.clone()).collect(),
                free_bytes: free,
                total_bytes: total,
                used_bytes: used,
                bytes_per_day: per_day,
                days_until_full: (per_day > 0.0).then(|| room as f64 / per_day),
            });
        }
        let per_hour: f64 = cams.iter().map(|c| c.usage.bytes_per_hour).sum();
        let used: u64 = cams.iter().map(|c| c.usage.used_bytes).sum();
        let disk_room: u64 = locations
            .iter()
            .map(|l| {
                l.free_bytes
                    .saturating_sub((cfg.storage.min_free_gb * 1073741824.0) as u64)
            })
            .sum();
        let (capacity, limited_by) = if cfg.storage.max_storage_gb > 0.0 {
            let limit = (cfg.storage.max_storage_gb * 1073741824.0) as u64;
            (
                limit.min(used + disk_room),
                if limit <= used + disk_room {
                    "max storage"
                } else {
                    "disk space"
                },
            )
        } else {
            (used + disk_room, "disk space")
        };
        let per_day = per_hour * 24.0;
        UsageReport {
            updated: Some(Local::now()),
            bytes_per_hour: per_hour,
            bytes_per_day: per_day,
            used_bytes: used,
            capacity_bytes: Some(capacity),
            days_capacity: (per_day > 0.0).then(|| capacity as f64 / per_day),
            days_until_full: (per_day > 0.0)
                .then(|| capacity.saturating_sub(used) as f64 / per_day),
            limited_by: limited_by.into(),
            retention_days: cfg.storage.retention_days,
            cameras: cams,
            locations,
        }
    }

    /// Periodic housekeeping: retention, usage statistics, temp files.
    pub async fn housekeeping(self: Arc<Self>) {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        let mut n = 0u64;
        loop {
            tick.tick().await;
            let cfg = self.config();
            let stores = self.event_stores();
            let me = self.clone();
            let _ = tokio::task::spawn_blocking(move || {
                if n.is_multiple_of(5) {
                    storage::cleanup(&cfg, &stores);
                    // Exports older than an hour.
                    if let Ok(rd) = std::fs::read_dir(me.data_dir.join("tmp")) {
                        for e in rd.flatten() {
                            let old = e
                                .metadata()
                                .and_then(|m| m.modified())
                                .map(|t| {
                                    t.elapsed().unwrap_or_default() > Duration::from_secs(3600)
                                })
                                .unwrap_or(false);
                            if old {
                                let _ = std::fs::remove_file(e.path());
                            }
                        }
                    }
                }
                *me.usage.lock().unwrap() = Self::compute_usage(&cfg);
            })
            .await;
            self.previews
                .lock()
                .unwrap()
                .retain(|_, v| v.2.elapsed() < Duration::from_secs(1800));
            n += 1;
        }
    }
}
