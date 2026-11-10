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
    /// Light sub-stream copy for the timeline (HD 24/7 cameras).
    pub timeline_recorder: Option<Arc<Recorder>>,
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
        // HD 24/7: also record the sub-stream, for a timeline that opens and
        // scrubs instantly (small files the browser can play directly).
        let timeline_recorder = (cfg.timeline_copy
            && cfg.has_sub()
            && cfg.record_mode.continuous_stream() == Some(false))
        .then(|| {
            let mut c = cfg.clone();
            c.transcode = false;
            c.record_fps = 0;
            let spec = RecSpec {
                label: "timeline copy (sub-stream)",
                relay: relay_sub.clone(),
                dir: crate::storage::timeline_dir(&dir),
                prefix: cfg.id.clone(),
                segment_seconds: segment,
            };
            Recorder::new(c, spec, env.clone(), cancel.clone())
        });
        for r in recorder
            .iter()
            .chain(buffer_recorder.iter())
            .chain(timeline_recorder.iter())
        {
            tasks.push(tokio::spawn(r.clone().run()));
        }

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

        // HD view: decodes the main stream only while someone has a camera open.
        let live_main = Live::new(
            cfg.clone(),
            env.clone(),
            Quality::Main,
            relay_main.clone(),
            None,
            cancel.clone(),
        );
        // Grid view: reuses the motion detector's frames when detection is on.
        let live_sub = Live::new(
            cfg.clone(),
            env.clone(),
            Quality::Sub,
            relay_sub.clone(),
            detector.clone(),
            cancel.clone(),
        );

        Arc::new(Self {
            dir,
            recorder,
            buffer_recorder,
            timeline_recorder,
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
        for r in self
            .recorder
            .iter()
            .chain(self.buffer_recorder.iter())
            .chain(self.timeline_recorder.iter())
        {
            r.pause();
        }
    }

    pub fn resume(&self) {
        for r in self
            .recorder
            .iter()
            .chain(self.buffer_recorder.iter())
            .chain(self.timeline_recorder.iter())
        {
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
    /// When the recordings folder changes, move existing recordings there.
    #[serde(default)]
    pub move_recordings: bool,
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
    /// Smart search (CLIP) model and index.
    pub clip: Arc<crate::clip::Clip>,
    pub smart: Arc<crate::smart::SmartIndex>,
    /// Which alerts have been viewed.
    pub seen: crate::seen::Seen,
    pub monitor: Arc<Monitor>,
    usage: Mutex<UsageReport>,
    /// Temporary stream previews for the camera editor: token → (url, transport).
    pub previews: Mutex<HashMap<String, (String, String, Instant)>>,
    /// ONVIF (PTZ) clients per camera, keeping discovered service addresses.
    pub onvif: Mutex<HashMap<String, Arc<crate::onvif::Onvif>>>,
}

impl Manager {
    pub fn new(cfg_path: PathBuf, data_dir: PathBuf, config: Config) -> Arc<Self> {
        let _ = std::fs::create_dir_all(data_dir.join("tmp"));
        let ai = Arc::new(AiHandle::new(config.ai.clone(), &data_dir));
        let data_dir_for_clip = data_dir.clone();
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
            clip: crate::clip::Clip::new(&data_dir_for_clip),
            smart: Arc::new(crate::smart::SmartIndex::default()),
            seen: crate::seen::Seen::open(&data_dir_for_clip),
            usage: Mutex::new(UsageReport::default()),
            previews: Mutex::new(HashMap::new()),
            onvif: Mutex::new(HashMap::new()),
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

    pub async fn update_camera(
        &self,
        id: &str,
        mut cam: CameraConfig,
        move_recordings: bool,
    ) -> Result<CameraConfig> {
        let _g = self.edit.lock().await;
        let mut cfg = self.config();
        let old_cfg = cfg.clone();
        let Some(slot) = cfg.cameras.iter_mut().find(|c| c.id == id) else {
            bail!("camera not found")
        };
        cam.id = id.to_string();
        cam.name = cam.name.trim().to_string();
        cam.validate()?;
        *slot = cam.clone();
        let old_cam = old_cfg.cameras.iter().find(|c| c.id == id).cloned();
        self.commit(cfg.clone())?;
        info!("updated camera \"{}\"", cam.name);
        // Display, live-view and PTZ options apply without interrupting recording.
        if let (Some(old_cam), Some(rt)) = (&old_cam, self.camera(id))
            && !cam.needs_restart(old_cam)
            && !move_recordings
        {
            rt.live_sub.set_view(cam.view.clone());
            rt.live_main.set_view(cam.view.clone());
            return Ok(cam);
        }
        let moved = if move_recordings {
            self.move_camera_dirs(&old_cfg, &cfg, Some(id)).await
        } else {
            Ok(())
        };
        self.restart_camera(&cam).await;
        moved?;
        Ok(cam)
    }

    /// Reconnect a camera from scratch (e.g. after its stream format changed).
    pub async fn reconnect_camera(&self, id: &str) {
        if let Some(cam) = self.config().cameras.into_iter().find(|c| c.id == id) {
            info!("[{}] reconnecting (camera settings changed)", cam.name);
            self.restart_camera(&cam).await;
        }
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
        cfg.dashboard.remove_camera(id);
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

    pub fn onvif(&self, id: &str) -> Arc<crate::onvif::Onvif> {
        self.onvif
            .lock()
            .unwrap()
            .entry(id.to_string())
            .or_default()
            .clone()
    }

    /// The camera moved to a new IP address: change it in its stream URLs and
    /// PTZ address. Returns true if anything changed.
    pub async fn change_camera_host(&self, id: &str, old: &str, new: &str) -> Result<bool> {
        let Some(mut cam) = self.config().cameras.into_iter().find(|c| c.id == id) else {
            bail!("camera not found")
        };
        let swap = |url: &str| swap_host(url, old, new);
        let before = cam.clone();
        cam.url = swap(&cam.url);
        cam.live_url = swap(&cam.live_url);
        if cam.ptz.host.trim() == old {
            cam.ptz.host = new.to_string();
        }
        if cam == before {
            return Ok(false);
        }
        info!("[{}] camera address changed from {old} to {new}", cam.name);
        self.update_camera(id, cam, false).await?;
        Ok(true)
    }

    /// Change a camera's live-view options without restarting it.
    pub async fn set_camera_view(&self, id: &str, view: crate::config::ViewConfig) -> Result<()> {
        let _g = self.edit.lock().await;
        let mut cfg = self.config();
        let Some(c) = cfg.cameras.iter_mut().find(|c| c.id == id) else {
            bail!("camera not found")
        };
        c.view = view.clone();
        c.validate()?;
        self.commit(cfg)?;
        if let Some(rt) = self.camera(id) {
            rt.live_sub.set_view(view.clone());
            rt.live_main.set_view(view);
        }
        Ok(())
    }

    pub async fn update_dashboard(&self, d: DashboardConfig) -> Result<()> {
        let _g = self.edit.lock().await;
        let mut cfg = self.config();
        cfg.dashboard = d;
        self.commit(cfg)
    }

    pub async fn update_profiles(&self, mut p: Vec<crate::config::Profile>) -> Result<()> {
        let _g = self.edit.lock().await;
        for x in &mut p {
            x.name = x.name.trim().to_string();
            anyhow::ensure!(!x.name.is_empty(), "a profile needs a name");
            // The mask belongs to a camera's view, not to a profile.
            x.detect.mask.clear();
        }
        let mut cfg = self.config();
        cfg.profiles = p;
        self.commit(cfg)
    }

    /// Apply settings. Returns true if a program restart is needed.
    pub async fn update_settings(&self, up: SettingsUpdate) -> Result<bool> {
        let _g = self.edit.lock().await;
        let old = self.config();
        let mut cfg = old.clone();
        let up_move = up.move_recordings;
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
        self.apply(old, cfg, up_move).await
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
        self.apply(old, new, false).await
    }

    /// Move recordings of cameras whose folder changed between `old` and `new`
    /// (only camera `only`, if given). Cameras are stopped first so no file is
    /// being written; the caller restarts them.
    async fn move_camera_dirs(&self, old: &Config, new: &Config, only: Option<&str>) -> Result<()> {
        let mut jobs = Vec::new();
        for cam in &new.cameras {
            if only.is_some_and(|o| o != cam.id) {
                continue;
            }
            let Some(prev) = old.cameras.iter().find(|c| c.id == cam.id) else {
                continue;
            };
            let (from, to) = (old.camera_dir(prev), new.camera_dir(cam));
            if from != to && from.exists() {
                if let Some(rt) = self.camera(&cam.id) {
                    rt.stop().await;
                }
                self.stores.lock().unwrap().remove(&cam.id);
                jobs.push((from, to));
            }
        }
        // Folders of removed cameras follow the default folder.
        if only.is_none() && old.storage.dir != new.storage.dir {
            if let Ok(rd) = std::fs::read_dir(&old.storage.dir) {
                for e in rd.flatten() {
                    let p = e.path();
                    if p.is_dir()
                        && !jobs.iter().any(|j| j.0 == p)
                        && !new.cameras.iter().any(|c| new.camera_dir(c) == p)
                    {
                        jobs.push((p.clone(), new.storage.dir.join(e.file_name())));
                    }
                }
            }
        }
        if jobs.is_empty() {
            return Ok(());
        }
        tokio::task::spawn_blocking(move || -> Result<()> {
            for (from, to) in jobs {
                crate::fsbrowse::move_tree(&from, &to)
                    .with_context(|| format!("moving {} to {}", from.display(), to.display()))?;
            }
            Ok(())
        })
        .await?
    }

    async fn apply(&self, old: Config, cfg: Config, move_recordings: bool) -> Result<bool> {
        self.commit(cfg.clone())?;
        let moved = if move_recordings {
            self.move_camera_dirs(&old, &cfg, None).await
        } else {
            Ok(())
        };
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
        moved?;
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
        tokio::spawn(backfill_colors(self.clone()));
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

/// Alerts saved before colours were recorded: estimate each object's colour
/// from the middle of its thumbnail (once, in the background), so a search
/// like "white car" also finds them.
async fn backfill_colors(mgr: Arc<Manager>) {
    tokio::time::sleep(Duration::from_secs(20)).await;
    let ffmpeg = mgr.config().server.ffmpeg_path.clone();
    let cutoff = chrono::Local::now() - chrono::Duration::days(60);
    let mut done = 0;
    for store in mgr.event_stores().into_values() {
        for mut e in store.all() {
            if e.start < cutoff || e.labels.is_empty() || e.labels.iter().any(|l| l.color.is_some())
            {
                continue;
            }
            let Some(thumb) = &e.thumb else { continue };
            let (w, h) = (160u32, 90u32);
            crate::load::wait_for_cpu(70, Duration::from_secs(600)).await;
            let Ok(out) = crate::load::background(&mut tokio::process::Command::new(&ffmpeg))
                .args(["-hide_banner", "-loglevel", "error", "-i"])
                .arg(store.dir().join(thumb))
                .args([
                    "-vf",
                    &format!("scale={w}:{h}"),
                    "-f",
                    "rawvideo",
                    "-pix_fmt",
                    "rgb24",
                    "pipe:1",
                ])
                .stdin(std::process::Stdio::null())
                .kill_on_drop(true)
                .output()
                .await
            else {
                return; // no ffmpeg
            };
            if out.stdout.len() != (w * h * 3) as usize {
                continue;
            }
            let f = crate::imgutil::Frame {
                width: w,
                height: h,
                rgb: std::sync::Arc::new(out.stdout),
            };
            // Older thumbnails were cropped around the main object.
            let (x, y, bw, bh) = match e.focus {
                Some([x, y, bw, bh]) => (x, y, bw, bh),
                None => (0.3, 0.2, 0.4, 0.6),
            };
            let c = f.color_name(x, y, bw, bh).map(str::to_string);
            if let Some(first) = e.labels.first_mut() {
                first.color = c;
            }
            store.save(e);
            done += 1;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    if done > 0 {
        info!("estimated colours for {done} earlier alert(s) (for search)");
    }
}

/// Replace the host of a URL (`rtsp://user:pass@HOST:554/path`) if it is `old`.
pub fn swap_host(url: &str, old: &str, new: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    // The login may contain '@' or '/' in the password: the host follows the last '@' before the path.
    let path_at = rest.find('/').unwrap_or(rest.len());
    let at = rest[..path_at]
        .rfind('@')
        .or_else(|| rest.rfind('@').filter(|&i| !rest[i..].contains("://")));
    let (auth, hostpart) = match at {
        Some(i) => (&rest[..=i], &rest[i + 1..]),
        None => ("", rest),
    };
    let end = hostpart.find([':', '/', '?']).unwrap_or(hostpart.len());
    if &hostpart[..end] == old {
        format!("{scheme}://{auth}{new}{}", &hostpart[end..])
    } else {
        url.to_string()
    }
}

#[cfg(test)]
mod host_tests {
    use super::swap_host;

    #[test]
    fn swaps_only_the_host() {
        assert_eq!(
            swap_host(
                "rtsp://admin:pw@192.168.1.64:554/Streaming/Channels/101",
                "192.168.1.64",
                "192.168.1.70"
            ),
            "rtsp://admin:pw@192.168.1.70:554/Streaming/Channels/101"
        );
        assert_eq!(
            swap_host("rtsp://192.168.1.64/stream2", "192.168.1.64", "10.0.0.5"),
            "rtsp://10.0.0.5/stream2"
        );
        assert_eq!(
            swap_host("rtsp://u:p@ss@192.168.1.64/s", "192.168.1.64", "1.2.3.4"),
            "rtsp://u:p@ss@1.2.3.4/s"
        );
        // A different camera's address is left alone.
        assert_eq!(
            swap_host("rtsp://192.168.1.640/s", "192.168.1.64", "1.2.3.4"),
            "rtsp://192.168.1.640/s"
        );
        assert_eq!(
            swap_host("person.mp4", "192.168.1.64", "1.2.3.4"),
            "person.mp4"
        );
    }
}
