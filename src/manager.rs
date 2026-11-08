//! Owns the configuration and the running cameras. Changes made from the
//! web UI go through here: they are validated, saved to disk, and applied
//! by restarting only the cameras that are affected.

use crate::config::{CameraConfig, Config, HardwareConfig, LiveConfig, StorageConfig};
use crate::live::Live;
use crate::recorder::Recorder;
use crate::storage;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::info;

/// Settings shared by all cameras. Changing any of them restarts the cameras.
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeEnv {
    pub ffmpeg_path: String,
    pub storage_dir: PathBuf,
    pub segment_seconds: u64,
    pub live: LiveConfig,
    pub hardware: HardwareConfig,
}

impl RuntimeEnv {
    fn from(cfg: &Config) -> Self {
        Self {
            ffmpeg_path: cfg.server.ffmpeg_path.clone(),
            storage_dir: cfg.storage.dir.clone(),
            segment_seconds: cfg.storage.segment_seconds,
            live: cfg.live.clone(),
            hardware: cfg.hardware.clone(),
        }
    }

    pub fn camera_dir(&self, id: &str) -> PathBuf {
        self.storage_dir.join(id)
    }
}

pub struct CameraRuntime {
    pub cfg: CameraConfig,
    pub env: Arc<RuntimeEnv>,
    pub recorder: Arc<Recorder>,
    pub live: Arc<Live>,
    cancel: CancellationToken,
    task: std::sync::Mutex<Option<JoinHandle<()>>>,
}

impl CameraRuntime {
    fn start(cfg: CameraConfig, env: Arc<RuntimeEnv>) -> Arc<Self> {
        let cancel = CancellationToken::new();
        let recorder = Recorder::new(cfg.clone(), env.clone(), cancel.clone());
        let live = Live::new(cfg.clone(), env.clone(), cancel.clone());
        let task = tokio::spawn(recorder.clone().run());
        Arc::new(Self {
            cfg,
            env,
            recorder,
            live,
            cancel,
            task: std::sync::Mutex::new(Some(task)),
        })
    }

    /// Stop recording (finishing the current file) and the live view.
    async fn stop(&self) {
        self.cancel.cancel();
        let task = self.task.lock().unwrap().take();
        if let Some(t) = task {
            let _ = tokio::time::timeout(Duration::from_secs(10), t).await;
        }
    }

    pub fn dir(&self) -> PathBuf {
        self.env.camera_dir(&self.cfg.id)
    }
}

/// Fields of the general settings form; `None` sections are left unchanged.
#[derive(Debug, Deserialize)]
pub struct SettingsUpdate {
    pub server: Option<ServerUpdate>,
    pub storage: Option<StorageConfig>,
    pub live: Option<LiveConfig>,
    pub hardware: Option<HardwareConfig>,
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

pub struct Manager {
    pub cfg_path: PathBuf,
    config: RwLock<Config>,
    cameras: RwLock<Vec<Arc<CameraRuntime>>>,
    edit: tokio::sync::Mutex<()>,
    pub started: Instant,
    /// The address actually bound at startup (listen changes need a restart).
    pub bound_listen: String,
}

impl Manager {
    pub fn new(cfg_path: PathBuf, config: Config) -> Arc<Self> {
        let env = Arc::new(RuntimeEnv::from(&config));
        let cameras = config
            .cameras
            .iter()
            .map(|c| CameraRuntime::start(c.clone(), env.clone()))
            .collect();
        Arc::new(Self {
            cfg_path,
            bound_listen: config.server.listen.clone(),
            config: RwLock::new(config),
            cameras: RwLock::new(cameras),
            edit: tokio::sync::Mutex::new(()),
            started: Instant::now(),
        })
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

    fn env(&self) -> Arc<RuntimeEnv> {
        Arc::new(RuntimeEnv::from(&self.config.read().unwrap()))
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
        let rt = CameraRuntime::start(cam.clone(), self.env());
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
        let rt = CameraRuntime::start(cam.clone(), self.env());
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
        self.commit(cfg)?;
        let rt = self.camera(id);
        self.cameras.write().unwrap().retain(|c| c.cfg.id != id);
        if let Some(rt) = rt {
            rt.stop().await;
            if delete_recordings {
                let dir = rt.dir();
                tokio::task::spawn_blocking(move || std::fs::remove_dir_all(dir))
                    .await?
                    .context("deleting recordings")?;
            }
            info!("deleted camera \"{}\"", rt.cfg.name);
        }
        Ok(())
    }

    /// Apply general settings. Returns true if a program restart is needed.
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
        if let Some(h) = up.hardware {
            cfg.hardware = h;
        }
        if cfg.storage.dir != old.storage.dir {
            std::fs::create_dir_all(&cfg.storage.dir)
                .with_context(|| format!("cannot create {}", cfg.storage.dir.display()))?;
        }
        self.commit(cfg.clone())?;

        if RuntimeEnv::from(&cfg) != RuntimeEnv::from(&old) {
            info!("settings changed; restarting cameras");
            for cam in &cfg.cameras {
                self.restart_camera(cam).await;
            }
        }
        if cfg.storage != old.storage {
            let s = cfg.storage.clone();
            tokio::task::spawn_blocking(move || storage::cleanup(&s));
        }
        Ok(cfg.server.listen != self.bound_listen)
    }

    pub async fn shutdown(&self) {
        let cams = self.cameras();
        futures_util::future::join_all(cams.iter().map(|c| c.stop())).await;
    }

    /// Periodically apply retention rules.
    pub async fn retention_task(self: Arc<Self>) {
        let mut tick = tokio::time::interval(Duration::from_secs(300));
        loop {
            tick.tick().await;
            let s = self.config().storage;
            let _ = tokio::task::spawn_blocking(move || storage::cleanup(&s)).await;
        }
    }
}
