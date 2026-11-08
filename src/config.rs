//! Configuration loaded from a TOML file (default: `nvr.toml`).

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub camera: CameraConfig,
    pub recording: RecordingConfig,
    pub live: LiveConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Address the web interface listens on.
    pub listen: String,
    /// Optional HTTP basic-auth username. Auth is disabled when empty.
    pub username: String,
    /// Optional HTTP basic-auth password.
    pub password: String,
    /// Path to the ffmpeg binary.
    pub ffmpeg_path: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen: "0.0.0.0:8080".into(),
            username: String::new(),
            password: String::new(),
            ffmpeg_path: "ffmpeg".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CameraConfig {
    /// Friendly camera name shown in the UI and used in file names.
    pub name: String,
    /// Main stream URL used for recording, e.g. `rtsp://user:pass@192.168.1.50:554/stream1`.
    /// A local video file path also works (it is looped in real time) which is handy for testing.
    pub url: String,
    /// Optional lower-resolution sub-stream used for the live view. Falls back to `url`.
    pub live_url: String,
}

impl Default for CameraConfig {
    fn default() -> Self {
        Self {
            name: "Camera 1".into(),
            url: String::new(),
            live_url: String::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RecordingConfig {
    /// Directory recordings are written to.
    pub dir: PathBuf,
    /// Start recording automatically when the NVR starts.
    pub record_on_start: bool,
    /// Length of each recording file in seconds.
    pub segment_seconds: u64,
    /// Include the camera's audio track (re-encoded to AAC).
    pub record_audio: bool,
    /// Delete recordings older than this many days (0 = keep forever).
    pub retention_days: u64,
    /// Delete the oldest recordings once the folder exceeds this size (0 = unlimited).
    pub max_storage_gb: f64,
}

impl Default for RecordingConfig {
    fn default() -> Self {
        Self {
            dir: PathBuf::from("recordings"),
            record_on_start: true,
            segment_seconds: 300,
            record_audio: false,
            retention_days: 14,
            max_storage_gb: 0.0,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LiveConfig {
    /// Frames per second for the live view.
    pub fps: u32,
    /// Width in pixels of the live view (height keeps the aspect ratio). 0 = native.
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

pub const EXAMPLE_CONFIG: &str = include_str!("../nvr.example.toml");

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config file {}", path.display()))?;
        let cfg: Config =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<()> {
        if self.camera.url.trim().is_empty() {
            bail!("camera.url must be set to your camera's RTSP URL");
        }
        if self.recording.segment_seconds < 10 {
            bail!("recording.segment_seconds must be at least 10");
        }
        if self.live.fps == 0 || self.live.fps > 30 {
            bail!("live.fps must be between 1 and 30");
        }
        if !(2..=31).contains(&self.live.quality) {
            bail!("live.quality must be between 2 and 31");
        }
        Ok(())
    }

    pub fn live_url(&self) -> &str {
        if self.camera.live_url.trim().is_empty() {
            &self.camera.url
        } else {
            &self.camera.live_url
        }
    }

    /// Camera name reduced to characters that are safe in file names.
    pub fn camera_slug(&self) -> String {
        let s: String = self
            .camera
            .name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect();
        let s = s.trim_matches('-').to_string();
        if s.is_empty() { "camera".into() } else { s }
    }
}

/// Input arguments for ffmpeg for a given source URL.
pub fn ffmpeg_input_args(url: &str) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    if url.starts_with("rtsp://") || url.starts_with("rtsps://") {
        // TCP is far more reliable than UDP over Wi-Fi; timeout is in microseconds.
        args.extend(["-rtsp_transport", "tcp", "-timeout", "10000000"].map(String::from));
    } else if !url.contains("://") {
        // Local file: play it in real time on a loop (test mode).
        args.extend(["-re", "-stream_loop", "-1"].map(String::from));
    }
    args.push("-i".into());
    args.push(url.into());
    args
}

/// Hide the password in a URL like `rtsp://user:pass@host/...`.
pub fn mask_url(url: &str) -> String {
    if let Some(scheme_end) = url.find("://") {
        let rest = &url[scheme_end + 3..];
        let auth_end = rest.find('/').unwrap_or(rest.len());
        if let Some(at) = rest[..auth_end].rfind('@') {
            let userinfo = &rest[..at];
            let user = userinfo.split(':').next().unwrap_or("");
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

    #[test]
    fn masks_password() {
        assert_eq!(
            mask_url("rtsp://admin:secret@10.0.0.5:554/s1"),
            "rtsp://admin:****@10.0.0.5:554/s1"
        );
        assert_eq!(mask_url("rtsp://10.0.0.5/s1"), "rtsp://10.0.0.5/s1");
    }

    #[test]
    fn example_config_parses() {
        let cfg: Config = toml::from_str(EXAMPLE_CONFIG).unwrap();
        cfg.validate().unwrap();
    }

    #[test]
    fn slug() {
        let mut cfg = Config::default();
        cfg.camera.name = "Front Door %1".into();
        assert_eq!(cfg.camera_slug(), "front-door--1");
    }
}
