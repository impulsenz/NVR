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
pub const AI_PROVIDERS: &[&str] = &["none", "builtin", "deepstack", "ollama", "gemini", "openai"];
pub const BUILTIN_MODELS: &[&str] = &["yolox_nano", "yolox_tiny", "yolox_s"];
pub const CORNERS: &[&str] = &["tl", "tr", "bl", "br"];
/// Longest allowed recording file ("one long file").
pub const MAX_SEGMENT_SECONDS: u64 = 86_400;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub storage: StorageConfig,
    pub live: LiveConfig,
    pub display: DisplayConfig,
    pub hardware: HardwareConfig,
    pub ai: AiConfig,
    pub cloud: CloudConfig,
    pub dashboard: DashboardConfig,
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
    /// Default root folder; each camera records into its own sub-folder
    /// unless the camera has its own storage folder.
    pub dir: PathBuf,
    /// Default length of each recording file in seconds.
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
    /// Frame rate of the grid (sub-stream) view.
    pub fps: u32,
    /// Width of grid images in pixels (0 = native).
    pub width: u32,
    /// JPEG quality 2 (best) .. 31 (worst).
    pub quality: u32,
    /// Frame rate of the full-quality view.
    pub hd_fps: u32,
    /// Keep full-quality streams decoding in the background so opening a
    /// camera is instant (uses more CPU/GPU).
    pub preload_hd: bool,
}

impl Default for LiveConfig {
    fn default() -> Self {
        Self {
            fps: 5,
            width: 960,
            quality: 6,
            hd_fps: 15,
            preload_hd: true,
        }
    }
}

/// What is drawn on top of live video.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DisplayConfig {
    /// Corners with a fullscreen button: any of tl, tr, bl, br.
    pub fullscreen_corners: Vec<String>,
    pub show_clock: bool,
    pub clock_corner: String,
    /// Tokens: YYYY YY MMMM MMM MM M DD D dddd ddd HH H hh h mm ss A a
    pub clock_format: String,
    pub show_name: bool,
    pub show_fps: bool,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            fullscreen_corners: vec!["tr".into(), "br".into()],
            show_clock: true,
            clock_corner: "tl".into(),
            clock_format: "DD/MM/YYYY HH:mm:ss".into(),
            show_name: true,
            show_fps: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct HardwareConfig {
    /// GPU video decoding: none, auto, cuda, qsv, vaapi, videotoolbox, d3d11va, dxva2.
    pub hwaccel: String,
    /// H.264 encoder used when recordings are converted.
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

/// Object recognition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AiConfig {
    /// none, builtin (on this computer), deepstack (CodeProject.AI / DeepStack
    /// server), ollama (vision model on a local computer), gemini, openai
    /// (any OpenAI-compatible API).
    pub provider: String,
    pub builtin_model: String,
    /// Server address for deepstack/ollama/openai.
    pub url: String,
    pub api_key: String,
    /// Model name for ollama/gemini/openai.
    pub model: String,
    /// Upper limit on requests per minute across all cameras (cloud free tiers).
    pub max_per_minute: u32,
    pub timeout_seconds: u64,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            provider: "none".into(),
            builtin_model: "yolox_tiny".into(),
            url: String::new(),
            api_key: String::new(),
            model: String::new(),
            max_per_minute: 0,
            timeout_seconds: 20,
        }
    }
}

/// Optional copy of recordings to cloud storage using rclone.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CloudConfig {
    pub enabled: bool,
    pub rclone_path: String,
    /// rclone destination, e.g. `gdrive:NVR` or `dropbox:Cameras`.
    pub remote: String,
    /// "events" (alert clips and snapshots) or "all" (every recording).
    pub upload: String,
}

impl Default for CloudConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            rclone_path: "rclone".into(),
            remote: String::new(),
            upload: "events".into(),
        }
    }
}

/// Live dashboard layout (grid units; 12 columns).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct DashboardConfig {
    pub tiles: Vec<Tile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Tile {
    pub camera: String,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RecordMode {
    /// Full-quality stream 24/7.
    #[default]
    Continuous,
    /// Sub-stream 24/7 (saves space).
    ContinuousSub,
    /// Sub-stream 24/7, plus full-quality clips around motion/alerts.
    SubPlusEvents,
    /// Full-quality clips around motion/alerts only.
    Events,
    /// No recording (live view and detection only).
    Off,
    /// 0.2 format; converted to Continuous + schedule on load.
    #[serde(skip_serializing)]
    Scheduled,
}

impl RecordMode {
    /// Stream recorded around the clock, if any: (use sub-stream?).
    pub fn continuous_stream(self) -> Option<bool> {
        match self {
            RecordMode::Continuous | RecordMode::Scheduled => Some(false),
            RecordMode::ContinuousSub | RecordMode::SubPlusEvents => Some(true),
            RecordMode::Events | RecordMode::Off => None,
        }
    }

    /// Whether full-quality event clips with a pre-event buffer are recorded.
    pub fn event_clips(self) -> bool {
        matches!(self, RecordMode::SubPlusEvents | RecordMode::Events)
    }
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

/// Motion detection and object recognition for one camera.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DetectConfig {
    pub motion: bool,
    /// 1 (only big movements) .. 100 (very sensitive).
    pub sensitivity: u32,
    /// Run object recognition on motion.
    pub ai: bool,
    /// Objects that count as an alert, e.g. person, car, dog.
    pub objects: Vec<String>,
    /// Minimum AI confidence 0..1.
    pub min_confidence: f32,
    /// What starts an event: "motion" (any motion) or "objects" (only recognized objects).
    pub trigger: String,
    /// Seconds of video kept from before an event (pre-event buffer).
    pub pre_seconds: u64,
    /// Seconds kept after the last activity.
    pub post_seconds: u64,
    /// Frames per second analysed.
    pub fps: u32,
    /// Minimum seconds between AI checks for this camera.
    pub ai_interval: f64,
}

impl Default for DetectConfig {
    fn default() -> Self {
        Self {
            motion: true,
            sensitivity: 50,
            ai: false,
            objects: vec![
                "person".into(),
                "car".into(),
                "truck".into(),
                "dog".into(),
                "cat".into(),
            ],
            min_confidence: 0.5,
            trigger: "motion".into(),
            pre_seconds: 30,
            post_seconds: 15,
            fps: 5,
            ai_interval: 1.0,
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
    /// Main (high quality) stream.
    pub url: String,
    /// Optional sub-stream (lower resolution).
    pub live_url: String,
    /// "tcp" (reliable, recommended) or "udp".
    pub rtsp_transport: String,
    pub record_mode: RecordMode,
    /// Only record inside the schedule.
    pub schedule_enabled: bool,
    pub schedule: Schedule,
    pub record_audio: bool,
    /// Re-encode recordings to H.264 (uses the GPU encoder if configured).
    pub transcode: bool,
    /// Recording frame rate (0 = camera's own; other values re-encode).
    pub record_fps: u32,
    /// Recording file length in seconds (0 = default from storage settings).
    pub segment_seconds: u64,
    /// Storage folder for this camera (empty = default folder).
    pub storage_dir: String,
    /// Days to keep this camera's recordings (0 = default).
    pub retention_days: u64,
    /// Space this camera may use in GB (0 = no per-camera limit).
    pub max_storage_gb: f64,
    pub detect: DetectConfig,
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
            schedule_enabled: false,
            schedule: Schedule::default(),
            record_audio: true,
            transcode: false,
            record_fps: 0,
            segment_seconds: 0,
            storage_dir: String::new(),
            retention_days: 0,
            max_storage_gb: 0.0,
            detect: DetectConfig::default(),
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
        if self.record_mode == RecordMode::Scheduled {
            bail!("unsupported recording mode");
        }
        if self.record_fps > 60 {
            bail!("recording frame rate must be 0 (camera default) to 60");
        }
        if self.segment_seconds != 0 && !(10..=MAX_SEGMENT_SECONDS).contains(&self.segment_seconds)
        {
            bail!("file length must be between 10 seconds and 24 hours");
        }
        if self.max_storage_gb < 0.0 {
            bail!("storage limit cannot be negative");
        }
        let d = &self.detect;
        if !(1..=100).contains(&d.sensitivity) {
            bail!("motion sensitivity must be 1 to 100");
        }
        if !(0.05..=1.0).contains(&d.min_confidence) {
            bail!("minimum confidence must be between 5% and 100%");
        }
        if !["motion", "objects"].contains(&d.trigger.as_str()) {
            bail!("alert trigger must be motion or objects");
        }
        if d.pre_seconds > 120 || d.post_seconds > 600 {
            bail!("pre-event buffer is limited to 120 s and post-event to 600 s");
        }
        if d.fps == 0 || d.fps > 15 {
            bail!("detection frame rate must be 1 to 15");
        }
        if self.record_mode.event_clips() && !d.motion && !d.ai {
            bail!("this recording mode needs motion detection or object recognition turned on");
        }
        if d.trigger == "objects" && !d.ai {
            bail!("alerts on objects need object recognition turned on");
        }
        self.schedule.validate()
    }

    pub fn has_sub(&self) -> bool {
        !self.live_url.trim().is_empty()
    }

    /// Sub-stream URL, falling back to the main stream.
    pub fn live_url(&self) -> &str {
        if self.has_sub() {
            &self.live_url
        } else {
            &self.url
        }
    }

    /// Whether the mode and schedule call for recording right now.
    pub fn wants_recording(&self, now: NaiveDateTime) -> bool {
        self.enabled
            && self.record_mode != RecordMode::Off
            && (!self.schedule_enabled || self.schedule.is_active(now))
    }

    pub fn detection_enabled(&self) -> bool {
        self.enabled && (self.detect.motion || self.detect.ai)
    }

    pub fn segment_seconds(&self, default: u64) -> u64 {
        if self.segment_seconds == 0 {
            default
        } else {
            self.segment_seconds
        }
    }

    /// Recordings must be re-encoded (for H.264 conversion or a lower frame rate).
    pub fn reencode(&self) -> bool {
        self.transcode || self.record_fps > 0
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
        Self::from_toml(&text).with_context(|| format!("in {}", path.display()))
    }

    /// Parse (and migrate) config text, e.g. from an imported backup.
    pub fn from_toml(text: &str) -> Result<(Self, bool)> {
        let mut cfg: Config = toml::from_str(text).context("not a valid NVR configuration")?;
        let migrated = cfg.migrate();
        cfg.validate()?;
        Ok((cfg, migrated))
    }

    fn migrate(&mut self) -> bool {
        let mut changed = false;
        let rec = self.recording.take();
        if let Some(rec) = &rec {
            if let Some(d) = &rec.dir {
                self.storage.dir = d.clone();
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
            changed = true;
        }
        if let Some(cam) = self.camera.take() {
            if !cam.url.is_empty() {
                let rec = rec.unwrap_or_default();
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
        for c in &mut self.cameras {
            if c.record_mode == RecordMode::Scheduled {
                c.record_mode = RecordMode::Continuous;
                c.schedule_enabled = true;
                changed = true;
            }
        }
        changed
    }

    pub fn validate(&self) -> Result<()> {
        let s = &self.storage;
        if s.segment_seconds < 10 || s.segment_seconds > MAX_SEGMENT_SECONDS {
            bail!("file length must be between 10 seconds and 24 hours");
        }
        if s.max_storage_gb < 0.0 || s.min_free_gb < 0.0 {
            bail!("storage limits cannot be negative");
        }
        if s.dir.as_os_str().is_empty() {
            bail!("recordings folder is required");
        }
        let l = &self.live;
        if l.fps == 0 || l.fps > 30 || l.hd_fps == 0 || l.hd_fps > 30 {
            bail!("live view frame rate must be between 1 and 30");
        }
        if !(2..=31).contains(&l.quality) {
            bail!("live view quality must be between 2 and 31");
        }
        if l.width > 7680 {
            bail!("live view width is too large");
        }
        let d = &self.display;
        if d.fullscreen_corners
            .iter()
            .chain([&d.clock_corner])
            .any(|c| !CORNERS.contains(&c.as_str()))
        {
            bail!("corners must be tl, tr, bl or br");
        }
        if d.clock_format.trim().is_empty() {
            bail!("clock format cannot be empty");
        }
        if !HWACCELS.contains(&self.hardware.hwaccel.as_str()) {
            bail!("unknown hardware decoder \"{}\"", self.hardware.hwaccel);
        }
        if !ENCODERS.contains(&self.hardware.encoder.as_str()) {
            bail!("unknown encoder \"{}\"", self.hardware.encoder);
        }
        let ai = &self.ai;
        if !AI_PROVIDERS.contains(&ai.provider.as_str()) {
            bail!("unknown AI provider \"{}\"", ai.provider);
        }
        if !BUILTIN_MODELS.contains(&ai.builtin_model.as_str()) {
            bail!("unknown built-in AI model \"{}\"", ai.builtin_model);
        }
        if ["deepstack", "ollama", "openai"].contains(&ai.provider.as_str())
            && ai.url.trim().is_empty()
        {
            bail!("enter the address of the AI server");
        }
        if ["gemini"].contains(&ai.provider.as_str()) && ai.api_key.trim().is_empty() {
            bail!("an API key is required for Gemini");
        }
        if ai.timeout_seconds == 0 || ai.timeout_seconds > 300 {
            bail!("AI timeout must be 1 to 300 seconds");
        }
        if self.cloud.enabled && self.cloud.remote.trim().is_empty() {
            bail!("enter an rclone destination such as gdrive:NVR");
        }
        if !["events", "all"].contains(&self.cloud.upload.as_str()) {
            bail!("cloud upload must be events or all");
        }
        if self.server.listen.parse::<std::net::SocketAddr>().is_err() {
            bail!("listen address must look like 0.0.0.0:8080");
        }
        if !self.server.username.is_empty() && self.server.password.is_empty() {
            bail!("a password is required when a username is set");
        }
        let mut ids = std::collections::HashSet::new();
        for c in &self.cameras {
            c.validate()
                .with_context(|| format!("camera \"{}\"", c.name))?;
            if !ids.insert(&c.id) || slug(&c.id) != c.id {
                bail!("camera \"{}\" has an invalid or duplicate id", c.name);
            }
        }
        Ok(())
    }

    /// Write the config atomically.
    pub fn save(&self, path: &Path) -> Result<()> {
        let text = self.to_toml()?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
        Ok(())
    }

    pub fn to_toml(&self) -> Result<String> {
        let body = toml::to_string_pretty(self).context("serializing config")?;
        Ok(format!(
            "# NVR configuration. Normally edited from the web interface (Settings).\n\
             # Restart the NVR after editing this file by hand.\n\n{body}"
        ))
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

    /// Folder a camera records into.
    pub fn camera_dir(&self, cam: &CameraConfig) -> PathBuf {
        if cam.storage_dir.trim().is_empty() {
            self.storage.dir.join(&cam.id)
        } else {
            PathBuf::from(cam.storage_dir.trim()).join(&cam.id)
        }
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
        cfg.dashboard.tiles.push(Tile {
            camera: "a".into(),
            x: 0,
            y: 0,
            w: 6,
            h: 4,
        });
        cfg.validate().unwrap();
        let (back, migrated) = Config::from_toml(&cfg.to_toml().unwrap()).unwrap();
        assert!(!migrated);
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
        let (cfg, migrated) = Config::from_toml(v1).unwrap();
        assert!(migrated);
        assert_eq!(cfg.cameras.len(), 1);
        assert_eq!(cfg.cameras[0].id, "front-door");
        assert!(cfg.cameras[0].record_audio);
        assert_eq!(cfg.storage.segment_seconds, 60);
        assert_eq!(cfg.storage.dir, PathBuf::from("rec"));
    }

    #[test]
    fn migrates_v2_scheduled_mode() {
        let v2 = r#"
            [[cameras]]
            id = "garage"
            name = "Garage"
            url = "rtsp://x"
            record_mode = "scheduled"
            record_audio = false
        "#;
        let (cfg, migrated) = Config::from_toml(v2).unwrap();
        assert!(migrated);
        let c = &cfg.cameras[0];
        assert_eq!(c.record_mode, RecordMode::Continuous);
        assert!(c.schedule_enabled);
        assert!(!c.record_audio); // explicit value kept
        assert!(
            cfg.to_toml()
                .unwrap()
                .contains("record_mode = \"continuous\"")
        );
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
        assert!(s.is_active(at(2, 19, 0)));
        assert!(s.is_active(at(3, 6, 59)));
        assert!(!s.is_active(at(3, 7, 0)));
        assert!(!s.is_active(at(3, 19, 0)));
        let day = Schedule {
            days: vec![5, 6],
            start: "09:00".into(),
            end: "17:00".into(),
        };
        assert!(day.is_active(at(7, 12, 0)));
        assert!(!day.is_active(at(6, 12, 0)));
        let all = Schedule {
            days: vec![1],
            start: "00:00".into(),
            end: "00:00".into(),
        };
        assert!(all.is_active(at(3, 23, 59)));
    }

    #[test]
    fn camera_validation() {
        let mut c = CameraConfig {
            id: "a".into(),
            url: "rtsp://x".into(),
            ..Default::default()
        };
        c.validate().unwrap();
        c.record_mode = RecordMode::Events;
        c.detect.motion = false;
        assert!(c.validate().is_err());
        c.detect.motion = true;
        c.segment_seconds = 5;
        assert!(c.validate().is_err());
        c.segment_seconds = MAX_SEGMENT_SECONDS;
        c.validate().unwrap();
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
