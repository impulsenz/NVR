//! Configuration stored in a TOML file (default: `nvr.toml`).
//! The web UI edits it, so it is rewritten whenever settings are saved.

use anyhow::{Context, Result, bail};
use chrono::{Datelike, NaiveDateTime, NaiveTime, Timelike};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const HWACCELS: &[&str] = &[
    "none",
    "auto",
    "cuda",
    "qsv",
    "vaapi",
    "videotoolbox",
    "d3d11va",
    "dxva2",
];
pub const ENCODERS: &[&str] = &[
    "auto",
    "libx264",
    "h264_nvenc",
    "h264_qsv",
    "h264_vaapi",
    "h264_videotoolbox",
    "h264_amf",
    "h264_mf",
];

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub storage: StorageConfig,
    pub live: LiveConfig,
    pub hardware: HardwareConfig,
    pub cameras: Vec<CameraConfig>,

    // Version 0.1 single-camera format; migrated on load.
    #[serde(skip_serializing)]
    camera: Option<LegacyCamera>,
    #[serde(skip_serializing)]
    recording: Option<LegacyRecording>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ServerConfig {
    /// Name of this NVR, shown in the header.
    pub name: String,
    /// Address and port the web interface listens on (restart required).
    pub listen: String,
    /// HTTP basic-auth username; login is disabled when empty.
    pub username: String,
    pub password: String,
    pub ffmpeg_path: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            name: "Home NVR".into(),
            listen: "0.0.0.0:8080".into(),
            username: String::new(),
            password: String::new(),
            ffmpeg_path: "ffmpeg".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct StorageConfig {
    /// Root folder; each camera records into its own sub-folder.
    pub dir: PathBuf,
    /// Length of each recording file in seconds.
    pub segment_seconds: u64,
    /// Delete recordings older than this (0 = keep forever).
    pub retention_days: u64,
    /// Delete the oldest recordings above this total size (0 = unlimited).
    pub max_storage_gb: f64,
    /// Delete the oldest recordings when free disk space drops below this (0 = off).
    pub min_free_gb: f64,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            dir: PathBuf::from("recordings"),
            segment_seconds: 300,
            retention_days: 14,
            max_storage_gb: 0.0,
            min_free_gb: 5.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LiveConfig {
    pub fps: u32,
    /// Width of the live view in pixels (0 = native).
    pub width: u32,
    /// JPEG quality 2 (best) .. 31 (worst).
    pub quality: u32,
}

impl Default for LiveConfig {
    fn default() -> Self {
        Self {
            fps: 5,
            width: 1280,
            quality: 6,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct HardwareConfig {
    /// GPU video decoding: none, auto, cuda, qsv, vaapi, videotoolbox, d3d11va, dxva2.
    pub hwaccel: String,
    /// H.264 encoder used when a camera converts its recordings: auto, libx264 (CPU),
    /// h264_nvenc, h264_qsv, h264_vaapi, h264_videotoolbox, h264_amf, h264_mf.
    pub encoder: String,
    /// GPU device (VAAPI only), e.g. /dev/dri/renderD128.
    pub device: String,
}

impl Default for HardwareConfig {
    fn default() -> Self {
        Self {
            hwaccel: "none".into(),
            encoder: "auto".into(),
            device: String::new(),
        }
    }
}

impl HardwareConfig {
    pub fn vaapi_device(&self) -> &str {
        if self.device.trim().is_empty() {
            "/dev/dri/renderD128"
        } else {
            self.device.trim()
        }
    }

    /// The concrete encoder to use, resolving "auto" from the decoder choice.
    pub fn resolved_encoder(&self) -> &str {
        if self.encoder != "auto" {
            return &self.encoder;
        }
        match self.hwaccel.as_str() {
            "cuda" => "h264_nvenc",
            "qsv" => "h264_qsv",
            "vaapi" => "h264_vaapi",
            "videotoolbox" => "h264_videotoolbox",
            _ => "libx264",
        }
    }

    pub fn uses_gpu(&self) -> bool {
        self.hwaccel != "none" || self.resolved_encoder() != "libx264"
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum RecordMode {
    #[default]
    Continuous,
    Scheduled,
    Off,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Schedule {
    /// Days of week, 0 = Monday .. 6 = Sunday.
    pub days: Vec<u8>,
    /// "HH:MM". If `end` <= `start` the window runs past midnight.
    pub start: String,
    pub end: String,
}

impl Default for Schedule {
    fn default() -> Self {
        Self {
            days: (0..7).collect(),
            start: "18:00".into(),
            end: "07:00".into(),
        }
    }
}

impl Schedule {
    fn parse(t: &str) -> Option<NaiveTime> {
        NaiveTime::parse_from_str(t.trim(), "%H:%M").ok()
    }

    pub fn validate(&self) -> Result<()> {
        if Self::parse(&self.start).is_none() || Self::parse(&self.end).is_none() {
            bail!("schedule times must be HH:MM");
        }
        if self.days.iter().any(|d| *d > 6) {
            bail!("schedule days must be 0 (Mon) to 6 (Sun)");
        }
        Ok(())
    }

    pub fn is_active(&self, now: NaiveDateTime) -> bool {
        let (Some(start), Some(end)) = (Self::parse(&self.start), Self::parse(&self.end)) else {
            return false;
        };
        let today = now.weekday().num_days_from_monday() as u8;
        let yesterday = (today + 6) % 7;
        let t = NaiveTime::from_hms_opt(now.hour(), now.minute(), now.second()).unwrap();
        let has = |d: u8| self.days.contains(&d);
        if start < end {
            has(today) && t >= start && t < end
        } else if start == end {
            has(today)
        } else {
            (has(today) && t >= start) || (has(yesterday) && t < end)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CameraConfig {
    /// Stable identifier, also the name of the camera's recordings folder.
    pub id: String,
    pub name: String,
    pub enabled: bool,
    /// Main (high quality) stream, used for recording.
    pub url: String,
    /// Optional sub-stream for the live view.
    pub live_url: String,
    /// "tcp" (reliable, recommended) or "udp".
    pub rtsp_transport: String,
    pub record_mode: RecordMode,
    pub schedule: Schedule,
    pub record_audio: bool,
    /// Re-encode recordings to H.264 (uses the GPU encoder if configured).
    pub transcode: bool,
}

impl Default for CameraConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: "Camera".into(),
            enabled: true,
            url: String::new(),
            live_url: String::new(),
            rtsp_transport: "tcp".into(),
            record_mode: RecordMode::Continuous,
            schedule: Schedule::default(),
            record_audio: false,
            transcode: false,
        }
    }
}

impl CameraConfig {
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            bail!("camera name is required");
        }
        if self.url.trim().is_empty() {
            bail!("main stream URL is required");
        }
        if !["tcp", "udp"].contains(&self.rtsp_transport.as_str()) {
            bail!("RTSP transport must be tcp or udp");
        }
        self.schedule.validate()
    }

    pub fn live_url(&self) -> &str {
        if self.live_url.trim().is_empty() {
            &self.url
        } else {
            &self.live_url
        }
    }

    pub fn wants_recording(&self, now: NaiveDateTime) -> bool {
        self.enabled
            && match self.record_mode {
                RecordMode::Continuous => true,
                RecordMode::Scheduled => self.schedule.is_active(now),
                RecordMode::Off => false,
            }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
#[serde(default)]
struct LegacyCamera {
    name: String,
    url: String,
    live_url: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
#[serde(default)]
struct LegacyRecording {
    dir: Option<PathBuf>,
    record_on_start: Option<bool>,
    segment_seconds: Option<u64>,
    record_audio: Option<bool>,
    retention_days: Option<u64>,
    max_storage_gb: Option<f64>,
}

impl Config {
    /// Load the config, creating a default one if missing. Returns whether
    /// the file was created or migrated (and so should be saved).
    pub fn load_or_create(path: &Path) -> Result<(Self, bool)> {
        if !path.exists() {
            return Ok((Config::default(), true));
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config file {}", path.display()))?;
        let mut cfg: Config =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        let migrated = cfg.migrate();
        cfg.validate()?;
        Ok((cfg, migrated))
    }

    fn migrate(&mut self) -> bool {
        let mut changed = false;
        if let Some(rec) = self.recording.take() {
            if let Some(d) = rec.dir {
                self.storage.dir = d;
            }
            if let Some(s) = rec.segment_seconds {
                self.storage.segment_seconds = s;
            }
            if let Some(r) = rec.retention_days {
                self.storage.retention_days = r;
            }
            if let Some(m) = rec.max_storage_gb {
                self.storage.max_storage_gb = m;
            }
            if let Some(cam) = self.camera.take()
                && !cam.url.is_empty()
            {
                let mut c = CameraConfig {
                    name: if cam.name.is_empty() {
                        "Camera 1".into()
                    } else {
                        cam.name
                    },
                    url: cam.url,
                    live_url: cam.live_url,
                    record_audio: rec.record_audio.unwrap_or(false),
                    record_mode: if rec.record_on_start == Some(false) {
                        RecordMode::Off
                    } else {
                        RecordMode::Continuous
                    },
                    ..Default::default()
                };
                c.id = self.new_camera_id(&c.name);
                self.cameras.push(c);
            }
            changed = true;
        }
        if let Some(cam) = self.camera.take() {
            if !cam.url.is_empty() {
                let mut c = CameraConfig {
                    name: cam.name,
                    url: cam.url,
                    live_url: cam.live_url,
                    ..Default::default()
                };
                c.id = self.new_camera_id(&c.name);
                self.cameras.push(c);
            }
            changed = true;
        }
        changed
    }

    pub fn validate(&self) -> Result<()> {
        let s = &self.storage;
        if s.segment_seconds < 10 || s.segment_seconds > 3600 {
            bail!("file length must be between 10 seconds and 60 minutes");
        }
        if s.max_storage_gb < 0.0 || s.min_free_gb < 0.0 {
            bail!("storage limits cannot be negative");
        }
        if s.dir.as_os_str().is_empty() {
            bail!("recordings folder is required");
        }
        let l = &self.live;
        if l.fps == 0 || l.fps > 30 {
            bail!("live view frame rate must be between 1 and 30");
        }
        if !(2..=31).contains(&l.quality) {
            bail!("live view quality must be between 2 and 31");
        }
        if l.width > 7680 {
            bail!("live view width is too large");
        }
        if !HWACCELS.contains(&self.hardware.hwaccel.as_str()) {
            bail!("unknown hardware decoder \"{}\"", self.hardware.hwaccel);
        }
        if !ENCODERS.contains(&self.hardware.encoder.as_str()) {
            bail!("unknown encoder \"{}\"", self.hardware.encoder);
        }
        if self.server.listen.parse::<std::net::SocketAddr>().is_err() {
            bail!("listen address must look like 0.0.0.0:8080");
        }
        if !self.server.username.is_empty() && self.server.password.is_empty() {
            bail!("a password is required when a username is set");
        }
        for c in &self.cameras {
            c.validate()
                .with_context(|| format!("camera \"{}\"", c.name))?;
        }
        Ok(())
    }

    /// Write the config atomically.
    pub fn save(&self, path: &Path) -> Result<()> {
        let body = toml::to_string_pretty(self).context("serializing config")?;
        let text = format!(
            "# NVR configuration. Normally edited from the web interface (Settings).\n\
             # Restart the NVR after editing this file by hand.\n\n{body}"
        );
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
        Ok(())
    }

    /// A unique, file-name-safe id derived from the camera name.
    pub fn new_camera_id(&self, name: &str) -> String {
        let base = slug(name);
        let mut id = base.clone();
        let mut n = 2;
        while self.cameras.iter().any(|c| c.id == id) {
            id = format!("{base}-{n}");
            n += 1;
        }
        id
    }
}

pub fn slug(name: &str) -> String {
    let mut s = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
        } else if !s.ends_with('-') {
            s.push('-');
        }
    }
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "camera".into() } else { s }
}

/// Hide the password in a URL like `rtsp://user:pass@host/...`.
pub fn mask_url(url: &str) -> String {
    if let Some(scheme_end) = url.find("://") {
        let rest = &url[scheme_end + 3..];
        let auth_end = rest.find('/').unwrap_or(rest.len());
        if let Some(at) = rest[..auth_end].rfind('@') {
            let user = rest[..at].split(':').next().unwrap_or("");
            return format!("{}://{}:****@{}", &url[..scheme_end], user, &rest[at + 1..]);
        }
    }
    url.to_string()
}

/// Replace any occurrence of `url` in `text` with its masked form.
pub fn redact(text: &str, url: &str) -> String {
    if url.is_empty() {
        text.to_string()
    } else {
        text.replace(url, &mask_url(url))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn at(day: u32, h: u32, m: u32) -> NaiveDateTime {
        // 2026-11-02 is a Monday.
        NaiveDate::from_ymd_opt(2026, 11, day)
            .unwrap()
            .and_hms_opt(h, m, 0)
            .unwrap()
    }

    #[test]
    fn masks_password() {
        assert_eq!(
            mask_url("rtsp://admin:secret@10.0.0.5:554/s1"),
            "rtsp://admin:****@10.0.0.5:554/s1"
        );
        assert_eq!(mask_url("rtsp://10.0.0.5/s1"), "rtsp://10.0.0.5/s1");
    }

    #[test]
    fn default_config_roundtrips() {
        let mut cfg = Config::default();
        cfg.cameras.push(CameraConfig {
            id: "a".into(),
            url: "rtsp://x".into(),
            ..Default::default()
        });
        cfg.validate().unwrap();
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back, cfg);
    }

    #[test]
    fn migrates_v1_config() {
        let v1 = r#"
            [server]
            listen = "0.0.0.0:8080"
            [camera]
            name = "Front Door"
            url = "rtsp://a:b@h/s"
            [recording]
            dir = "rec"
            segment_seconds = 60
            record_audio = true
        "#;
        let mut cfg: Config = toml::from_str(v1).unwrap();
        assert!(cfg.migrate());
        assert_eq!(cfg.cameras.len(), 1);
        assert_eq!(cfg.cameras[0].id, "front-door");
        assert!(cfg.cameras[0].record_audio);
        assert_eq!(cfg.storage.segment_seconds, 60);
        assert_eq!(cfg.storage.dir, PathBuf::from("rec"));
    }

    #[test]
    fn unique_ids() {
        let mut cfg = Config::default();
        cfg.cameras.push(CameraConfig {
            id: "front-door".into(),
            ..Default::default()
        });
        assert_eq!(cfg.new_camera_id("Front Door!"), "front-door-2");
        assert_eq!(slug("  %% "), "camera");
    }

    #[test]
    fn schedule_windows() {
        let s = Schedule {
            days: vec![0],
            start: "18:00".into(),
            end: "07:00".into(),
        };
        assert!(s.is_active(at(2, 19, 0))); // Mon evening
        assert!(s.is_active(at(3, 6, 59))); // Tue early morning (from Mon)
        assert!(!s.is_active(at(3, 7, 0)));
        assert!(!s.is_active(at(3, 19, 0))); // Tue evening not scheduled
        let day = Schedule {
            days: vec![5, 6],
            start: "09:00".into(),
            end: "17:00".into(),
        };
        assert!(day.is_active(at(7, 12, 0))); // Sat
        assert!(!day.is_active(at(6, 12, 0))); // Fri
        let all = Schedule {
            days: vec![1],
            start: "00:00".into(),
            end: "00:00".into(),
        };
        assert!(all.is_active(at(3, 23, 59)));
    }

    #[test]
    fn encoder_resolution() {
        let mut hw = HardwareConfig::default();
        assert!(!hw.uses_gpu());
        hw.hwaccel = "cuda".into();
        assert_eq!(hw.resolved_encoder(), "h264_nvenc");
        hw.encoder = "libx264".into();
        assert_eq!(hw.resolved_encoder(), "libx264");
    }
}
