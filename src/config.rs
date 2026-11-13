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
pub const AI_PROVIDERS: &[&str] = &["none", "builtin"];
pub const BUILTIN_MODELS: &[&str] = &["yolox_nano", "yolox_tiny", "yolox_s", "yolox_m", "yolox_l"];
/// Motion mask grid size: one cell per pixel motion detection looks at
/// (a 640×360 picture shrunk 4×).
pub const MASK_W: usize = 160;
pub const MASK_H: usize = 90;
/// The first, coarser mask grid (stored as 576 '0'/'1' characters).
const OLD_MASK_W: usize = 32;
const OLD_MASK_H: usize = 18;

/// Read a motion mask (true = ignore): "160x90:" followed by run lengths
/// (unmasked, masked, unmasked, …), or the old 32×18 '0'/'1' form, which is
/// scaled up. None when empty or invalid.
pub fn decode_mask(s: &str) -> Option<Vec<bool>> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if s.len() == OLD_MASK_W * OLD_MASK_H && s.chars().all(|c| c == '0' || c == '1') {
        let old: Vec<bool> = s.chars().map(|c| c == '1').collect();
        return Some(
            (0..MASK_W * MASK_H)
                .map(|i| {
                    let (x, y) = (i % MASK_W, i / MASK_W);
                    old[(y * OLD_MASK_H / MASK_H) * OLD_MASK_W + x * OLD_MASK_W / MASK_W]
                })
                .collect(),
        );
    }
    let runs = s.strip_prefix(&format!("{MASK_W}x{MASK_H}:"))?;
    let mut out = Vec::with_capacity(MASK_W * MASK_H);
    let mut on = false;
    for r in runs.split(',').filter(|r| !r.is_empty()) {
        let n: usize = r.trim().parse().ok()?;
        if out.len() + n > MASK_W * MASK_H {
            return None;
        }
        out.extend(std::iter::repeat_n(on, n));
        on = !on;
    }
    (out.len() == MASK_W * MASK_H).then_some(out)
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct RemoteServer {
    pub name: String,
    /// Its address, e.g. https://cabin.tail1234.ts.net:8443
    pub url: String,
    /// An API token made on that server.
    pub token: String,
    /// It uses its own certificate (not a trusted one): accept it anyway.
    pub trust_own_cert: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Webhook {
    pub name: String,
    pub enabled: bool,
    pub url: String,
    /// Only these cameras (empty = all).
    pub cameras: Vec<String>,
    /// Only alerts with these (person, car, …, "motion"; empty = all).
    pub objects: Vec<String>,
    /// Include the alert picture (base64 JPEG).
    pub picture: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MqttConfig {
    pub enabled: bool,
    /// The MQTT broker (e.g. Home Assistant's Mosquitto add-on).
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    /// Start of every topic GAZE uses.
    pub prefix: String,
    /// Announce everything to Home Assistant, so it appears by itself.
    pub discovery: bool,
}

impl Default for MqttConfig {
    fn default() -> Self {
        Self { enabled: false, host: String::new(), port: 1883, username: String::new(), password: String::new(), prefix: "gaze".into(), discovery: true }
    }
}

/// Home, Away and Night (like an alarm system): each says which cameras
/// send phone notifications. Recording and alerts carry on in every mode.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ModesConfig {
    /// "home", "away" or "night".
    pub current: String,
    /// While Home, Night comes on by itself in the night hours
    /// (Settings → Notifications → At night).
    pub auto_night: bool,
    pub home: ModeConfig,
    pub away: ModeConfig,
    pub night: ModeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ModeConfig {
    /// Phone notifications in this mode.
    pub notify: bool,
    /// The cameras that notify (empty = every camera).
    pub cameras: Vec<String>,
}

impl Default for ModeConfig {
    fn default() -> Self {
        Self { notify: true, cameras: Vec::new() }
    }
}

impl Default for ModesConfig {
    fn default() -> Self {
        Self {
            // Away = everything notifies, as before modes existed.
            current: "away".into(),
            auto_night: false,
            home: ModeConfig { notify: false, cameras: Vec::new() },
            away: ModeConfig::default(),
            night: ModeConfig::default(),
        }
    }
}

pub const MODES: &[&str] = &["home", "away", "night"];

impl ModesConfig {
    /// The mode in force now (Home turns into Night at night, if set).
    pub fn effective(&self, night: bool) -> &str {
        if self.current == "home" && self.auto_night && night { "night" } else { self.current.as_str() }
    }
    pub fn mode(&self, name: &str) -> &ModeConfig {
        match name {
            "home" => &self.home,
            "night" => &self.night,
            _ => &self.away,
        }
    }
    /// Does this camera send notifications in this mode?
    pub fn notifies(&self, mode: &str, camera: &str) -> bool {
        let m = self.mode(mode);
        m.notify && (m.cameras.is_empty() || m.cameras.iter().any(|c| c == camera))
    }
}

/// Listening for sounds (camera → Audio): glass breaking, alarms, screams …
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SoundConfig {
    pub enabled: bool,
    /// What to listen for (ids from `sound::KINDS`).
    pub kinds: Vec<String>,
    /// 1 (only clear, loud sounds) … 10 (the faintest).
    pub sensitivity: u8,
}

impl Default for SoundConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            kinds: ["glass", "alarm", "scream", "gunshot"].map(String::from).to_vec(),
            sensitivity: 6,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DewarpConfig {
    pub enabled: bool,
    /// The lens's field of view in degrees (180 for most fisheye cameras).
    pub lens: f32,
    /// Where to look: left/right (yaw) and up/down (pitch), in degrees.
    pub yaw: f32,
    pub pitch: f32,
    /// How wide the flat picture is, in degrees (smaller = zoomed in).
    pub width: f32,
}

impl Default for DewarpConfig {
    fn default() -> Self {
        Self { enabled: false, lens: 180.0, yaw: 0.0, pitch: 0.0, width: 100.0 }
    }
}

impl DewarpConfig {
    /// The ffmpeg filter that makes the flat picture.
    pub fn filter(&self) -> Option<String> {
        self.enabled.then(|| {
            let w = self.width.clamp(30.0, 160.0);
            format!(
                "v360=input=fisheye:output=flat:ih_fov={l}:iv_fov={l}:h_fov={w:.1}:v_fov={v:.1}:yaw={y:.1}:pitch={p:.1}:w=1920:h=1080",
                l = self.lens.clamp(120.0, 360.0),
                v = w * 9.0 / 16.0,
                y = self.yaw.clamp(-180.0, 180.0),
                p = self.pitch.clamp(-90.0, 90.0)
            )
        })
    }
}

static SAFE_MODE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Write a mask in the "160x90:" run-length form `decode_mask` reads.
pub fn encode_mask(cells: &[bool]) -> String {
    let mut runs = Vec::new();
    let (mut on, mut n) = (false, 0usize);
    for &c in cells {
        if c == on {
            n += 1;
        } else {
            runs.push(n.to_string());
            on = c;
            n = 1;
        }
    }
    runs.push(n.to_string());
    format!("{MASK_W}x{MASK_H}:{}", runs.join(","))
}

/// Places on the picture: top/middle/bottom × left/centre/right.
pub const CORNERS: &[&str] = &["tl", "tc", "tr", "ml", "mc", "mr", "bl", "bc", "br"];

/// An on-screen display position: one of the nine places, or anywhere on
/// the picture as "p:X,Y" (per cent from the left and top).
pub fn valid_position(c: &str) -> bool {
    if CORNERS.contains(&c) {
        return true;
    }
    c.strip_prefix("p:")
        .and_then(|xy| xy.split_once(','))
        .and_then(|(x, y)| Some((x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?)))
        .is_some_and(|(x, y)| (0.0..=100.0).contains(&x) && (0.0..=100.0).contains(&y))
}
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
    pub notify: NotifyConfig,
    pub update: UpdateConfig,
    pub dashboard: DashboardConfig,
    /// Saved recording / motion / AI settings to apply to cameras.
    pub profiles: Vec<Profile>,
    /// Home / Away / Night: which cameras send phone notifications.
    pub modes: ModesConfig,
    /// MQTT (Home Assistant and other home automation).
    pub mqtt: MqttConfig,
    /// Addresses each new alert is sent to (JSON).
    pub webhooks: Vec<Webhook>,
    /// Other GAZE servers whose cameras show on the Live page.
    pub remotes: Vec<RemoteServer>,
    pub cameras: Vec<CameraConfig>,

}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ServerConfig {
    /// Name of this NVR, shown in the header.
    pub name: String,
    /// Address and port the web interface listens on (restart required).
    pub listen: String,
    /// The single login of versions before 0.4: read once to create the
    /// admin account, never written back (accounts live in nvr-data).
    #[serde(skip_serializing)]
    pub username: String,
    #[serde(skip_serializing)]
    pub password: String,
    pub ffmpeg_path: String,
    /// Secure (https) web interface on this port, next to the plain one
    /// (0 = off). Phones' share menu, the installable app and access from
    /// outside need it.
    pub https_port: u16,
    /// Certificate and key files (PEM). Empty: GAZE makes its own
    /// (browsers warn about it once).
    pub https_cert: String,
    pub https_key: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            name: "GAZE".into(),
            listen: "0.0.0.0:8080".into(),
            username: String::new(),
            password: String::new(),
            ffmpeg_path: "ffmpeg".into(),
            https_port: 8443,
            https_cert: String::new(),
            https_key: String::new(),
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
    /// Keep alerts (their clips, pictures and the recordings they play
    /// from) this long (0 = as long as the recordings).
    pub alert_retention_days: u64,
    /// Delete the oldest recordings above this total size (0 = unlimited).
    pub max_storage_gb: f64,
    /// Delete the oldest recordings when free disk space drops below this (0 = off).
    pub min_free_gb: f64,
    /// Time-lapse previews (the pictures shown while dragging along a
    /// timeline): on or off.
    pub timelapse: bool,
    /// Time-lapse previews: seconds between pictures (1–120).
    pub timelapse_seconds: u32,
    /// Time-lapse pictures' width in pixels (256, 384 or 512; 16:9).
    pub timelapse_width: u32,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            dir: PathBuf::from("recordings"),
            segment_seconds: 300,
            retention_days: 14,
            alert_retention_days: 0,
            max_storage_gb: 0.0,
            min_free_gb: 5.0,
            timelapse: true,
            timelapse_seconds: 10,
            timelapse_width: 256,
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
    /// camera is instant. Off by default: it costs a lot of GPU/CPU.
    /// (Named differently from the 0.3 setting, which defaulted to on.)
    pub preload_hd_streams: bool,
}

impl Default for LiveConfig {
    fn default() -> Self {
        Self {
            fps: 5,
            width: 960,
            quality: 6,
            hd_fps: 15,
            preload_hd_streams: false,
        }
    }
}

/// What is drawn on top of live video.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DisplayConfig {
    /// Corners with a fullscreen button: any of tl, tr, bl, br.
    pub fullscreen_corners: Vec<String>,
    /// Tokens: YYYY YY MMMM MMM MM M DD D dddd ddd HH H hh h mm ss A a
    pub clock_format: String,
    /// The on-screen display of every camera without its own.
    pub osd: OsdConfig,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            fullscreen_corners: vec!["tr".into(), "br".into()],
            clock_format: "DD/MM/YYYY HH:mm:ss".into(),
            osd: OsdConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct HardwareConfig {
    /// Choose the GPU decoder and encoder automatically at start-up (the
    /// first that works on this computer). Off: use the choices below.
    pub auto: bool,
    /// GPU video decoding: none, auto, cuda, qsv, vaapi, videotoolbox, d3d11va, dxva2.
    pub hwaccel: String,
    /// H.264 encoder used when recordings are converted.
    pub encoder: String,
    /// GPU device (VAAPI only), e.g. /dev/dri/renderD128.
    pub device: String,
    /// Also decode the low-resolution sub-streams on the GPU. Usually not
    /// worth it: copying frames back from the GPU costs more than decoding
    /// small streams on the CPU.
    pub gpu_substreams: bool,
    /// Smart load balancing: when the GPU is busy, new video work goes to
    /// the CPU (and motion-detection decoding moves back and forth).
    pub balance: bool,
}

impl Default for HardwareConfig {
    fn default() -> Self {
        Self {
            auto: true,
            hwaccel: "none".into(),
            encoder: "auto".into(),
            device: String::new(),
            gpu_substreams: false,
            balance: true,
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
    /// Object detection: "none" (off) or "builtin" (on this computer).
    pub provider: String,
    pub builtin_model: String,
    /// Your own model (builtin_model "custom"): an ONNX file, its picture
    /// size and its class names, one per line (empty = the 80 COCO classes).
    pub custom_model: String,
    pub custom_size: u32,
    pub custom_labels: Vec<String>,
    /// A bigger model that double-checks what the main
    /// model found before an alert is raised ("" = off).
    pub verify_model: String,
    /// Brighten dark and infrared pictures before recognition.
    pub night_boost: bool,
    /// Where the AI runs: "gpu" (the graphics card, via DirectML) or "cpu".
    pub device: String,
    /// Look for faces in alerts and name known people.
    pub faces: bool,
    /// Read number plates of vehicles in alerts.
    pub plates: bool,
    /// Search alerts by description (CLIP: downloads 420 MB the first time).
    pub search: bool,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            provider: "none".into(),
            builtin_model: "yolox_tiny".into(),
            custom_model: String::new(),
            custom_size: 640,
            custom_labels: Vec::new(),
            verify_model: String::new(),
            night_boost: true,
            device: "gpu".into(),
            faces: true,
            plates: true,
            search: false,
        }
    }
}

/// Updates from GitHub (Settings → About).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
#[derive(Default)]
pub struct UpdateConfig {
    /// Install new versions as soon as they're found (off: ask first).
    pub auto: bool,
    /// GitHub repository to update from ("owner/name"); empty = the one
    /// this program was built from.
    pub repo: String,
}

/// Phone notifications through ntfy (https://ntfy.sh, or your own server).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct NotifyConfig {
    pub enabled: bool,
    /// ntfy server, e.g. https://ntfy.sh
    pub server: String,
    /// Topic to subscribe to in the ntfy app (long and random: on the public
    /// server anyone who knows it can read it).
    pub topic: String,
    /// Access token for a protected topic / own server (optional).
    pub token: String,
    /// Objects that notify.
    pub objects: Vec<String>,
    /// Cameras that notify (empty = all).
    pub cameras: Vec<String>,
    /// Minutes between notifications from one camera.
    pub gap_minutes: u32,
    /// At night: shorter gap (more watchful).
    pub night_from: String,
    pub night_to: String,
    pub night_gap_minutes: u32,
    /// Send a summary after the alert (besides the picture right away).
    pub summary: bool,
    /// Address that opens GAZE from the phone (e.g. your Tailscale name).
    pub link: String,
    /// Also tell when a camera goes offline or stops recording, and when
    /// the recordings drive is nearly full.
    pub health: bool,
}

impl Default for NotifyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            server: "https://ntfy.sh".into(),
            topic: String::new(),
            token: String::new(),
            objects: vec![
                "person".into(),
                "car".into(),
                "truck".into(),
                "bus".into(),
                "motorcycle".into(),
                "bicycle".into(),
                "package".into(),
            ],
            cameras: Vec::new(),
            gap_minutes: 5,
            night_from: "22:00".into(),
            night_to: "06:00".into(),
            night_gap_minutes: 1,
            summary: true,
            link: String::new(),
            health: true,
        }
    }
}

/// Optional copy of recordings to cloud storage using rclone.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CloudConfig {
    pub enabled: bool,
    pub rclone_path: String,
    /// Where to: a folder or network share (`D:\Backup`, `\\NAS\cameras`),
    /// a NAS over FTP (`ftp://nas.local/Cameras`) or an rclone destination
    /// (`gdrive:NVR`).
    pub remote: String,
    /// Login for the NAS (FTP, or a network share that needs one).
    pub user: String,
    pub password: String,
    /// "ai" (only alerts confirmed by object recognition), "events" (all alert
    /// clips and pictures, including motion-only) or "all" (every recording).
    pub upload: String,
}

impl Default for CloudConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            rclone_path: "rclone".into(),
            remote: String::new(),
            user: String::new(),
            password: String::new(),
            upload: "events".into(),
        }
    }
}

/// Live dashboard layout (grid units; 12 columns).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct DashboardConfig {
    /// Layout of "All cameras".
    pub tiles: Vec<Tile>,
    /// Layout of "All cameras" in full-screen (video wall) mode.
    pub fullscreen_tiles: Vec<Tile>,
    pub groups: Vec<CameraGroup>,
}

/// A named set of cameras with its own layouts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct CameraGroup {
    pub id: String,
    pub name: String,
    pub cameras: Vec<String>,
    pub tiles: Vec<Tile>,
    pub fullscreen_tiles: Vec<Tile>,
}

impl DashboardConfig {
    pub fn validate(&self) -> Result<()> {
        let mut ids = std::collections::HashSet::new();
        for g in &self.groups {
            if g.name.trim().is_empty() {
                bail!("every camera group needs a name");
            }
            if g.id.is_empty() || !ids.insert(&g.id) {
                bail!("camera group \"{}\" has an invalid id", g.name);
            }
        }
        let tiles = self.tiles.iter().chain(&self.fullscreen_tiles).chain(
            self.groups
                .iter()
                .flat_map(|g| g.tiles.iter().chain(&g.fullscreen_tiles)),
        );
        for t in tiles {
            if t.w == 0 || t.h == 0 || t.x + t.w > 12 || t.h > 48 || t.y > 200 {
                bail!("invalid layout tile for camera {}", t.camera);
            }
        }
        Ok(())
    }

    /// Forget a deleted camera everywhere.
    pub fn remove_camera(&mut self, id: &str) {
        self.tiles.retain(|t| t.camera != id);
        self.fullscreen_tiles.retain(|t| t.camera != id);
        for g in &mut self.groups {
            g.cameras.retain(|c| c != id);
            g.tiles.retain(|t| t.camera != id);
            g.fullscreen_tiles.retain(|t| t.camera != id);
        }
    }
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
    /// Full-quality stream 24/7, plus full-quality clips around alerts kept
    /// as their own files: they stay (Storage → keep alerts for) after the
    /// 24/7 footage around them is deleted to make room.
    ContinuousPlusEvents,
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
            RecordMode::Continuous | RecordMode::ContinuousPlusEvents | RecordMode::Scheduled => Some(false),
            RecordMode::ContinuousSub | RecordMode::SubPlusEvents => Some(true),
            RecordMode::Events | RecordMode::Off => None,
        }
    }

    /// Whether full-quality event clips with a pre-event buffer are recorded.
    pub fn event_clips(self) -> bool {
        matches!(self, RecordMode::SubPlusEvents | RecordMode::ContinuousPlusEvents | RecordMode::Events)
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
    /// Motion settings (sizes are in pixels of a 640×360 picture):
    /// how different a pixel must be from the background to count as changed (1–255).
    pub min_contrast: u32,
    /// Smallest moving object that counts, in pixels.
    pub min_object_px: u32,
    /// Motion must last this long before it triggers.
    pub make_time_ms: u32,
    /// The object must move at least this far (0 = off). Ignores trees and
    /// flags swaying in place.
    pub min_travel_px: u32,
    /// Areas to ignore: 32×18 grid, row by row, '1' = ignore. Empty = none.
    pub mask: String,
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
    /// More activity within this many seconds of the last joins the same
    /// alert (one longer video instead of many short ones). The timer
    /// starts again with each new trigger. 0 = the "record after" time.
    pub join_seconds: u64,
    /// Frames per second analysed.
    pub fps: u32,
    /// Minimum seconds between AI checks for this camera.
    pub ai_interval: f64,
    /// Vehicles that stay in one place stop alerting until they move.
    pub ignore_parked: bool,
    /// Motion set off by an animal that isn't in `objects` (a cat, a
    /// dog, a bird …) doesn't make an alert. Needs object detection on.
    pub ignore_animals: bool,
    /// Objects the AI finds mostly inside the areas drawn to ignore are left
    /// out too (not only motion there), whatever started the look.
    pub ai_ignores_mask: bool,
    /// What the AI finds in the sub-stream is checked in a full-quality
    /// (main stream) picture: "off", "mark" (not confirmed: marked as such)
    /// or "drop" (not confirmed: left out of the alert).
    pub hd_check: String,
    /// Seconds without moving before something counts as parked.
    pub parked_seconds: u32,
    /// Kinds of objects that stop alerting while they stand still.
    pub parked_objects: Vec<String>,
    /// Package delivery: watch `package_area` for parcels left or taken.
    pub packages: bool,
    /// Where parcels are left, painted on the picture: the same grid as
    /// `mask` ('1' = part of the spot). Empty: `package_area` (older box).
    pub package_mask: String,
    /// Where parcels are left (polygon, 0..1 of the picture), from older
    /// versions (a box); replaced by `package_mask` once painted.
    pub package_area: Vec<[f32; 2]>,
}

impl Default for DetectConfig {
    fn default() -> Self {
        Self {
            motion: true,
            min_contrast: 24,
            min_object_px: 300,
            make_time_ms: 500,
            min_travel_px: 0,
            mask: String::new(),
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
            pre_seconds: 3,
            post_seconds: 15,
            join_seconds: 30,
            fps: 5,
            ai_interval: 1.0,
            ignore_parked: true,
            ignore_animals: true,
            ai_ignores_mask: true,
            hd_check: "mark".into(),
            parked_seconds: 60,
            parked_objects: crate::stationary::PARKING
                .iter()
                .map(|s| s.to_string())
                .collect(),
            packages: false,
            package_mask: String::new(),
            package_area: Vec::new(),
        }
    }
}

/// On-screen display for one camera (overrides the global display settings).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct OsdConfig {
    /// false = use Settings → Live view & display.
    pub custom: bool,
    pub show_name: bool,
    pub name_corner: String,
    pub show_clock: bool,
    pub clock_corner: String,
    /// Empty = the global format.
    pub clock_format: String,
    /// Recording / armed / offline badge.
    pub show_status: bool,
    pub status_corner: String,
    /// What is moving right now: "motion", "person", "car" ...
    pub show_detections: bool,
    pub detections_corner: String,
    pub show_fps: bool,
    pub fps_corner: String,
    /// Resolution and video format of the stream shown.
    pub show_res: bool,
    pub res_corner: String,
    /// Extra text, e.g. "Driveway – north".
    pub text: String,
    pub text_corner: String,
}

impl Default for OsdConfig {
    fn default() -> Self {
        Self {
            custom: false,
            show_name: true,
            name_corner: "bl".into(),
            show_clock: true,
            clock_corner: "tl".into(),
            clock_format: String::new(),
            show_status: true,
            status_corner: "tr".into(),
            show_detections: true,
            detections_corner: "bl".into(),
            show_fps: false,
            fps_corner: "tl".into(),
            show_res: false,
            res_corner: "tl".into(),
            text: String::new(),
            text_corner: "br".into(),
        }
    }
}

/// How this camera is shown live (quick options on the Live page).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ViewConfig {
    /// Grid picture: "auto" (sub-stream) or "main" (full quality, more GPU/CPU).
    pub grid_quality: String,
    /// Grid frame rate (0 = default from Live view settings).
    pub grid_fps: u32,
    /// Full-screen/opened frame rate (0 = default).
    pub hd_fps: u32,
}

impl Default for ViewConfig {
    fn default() -> Self {
        Self {
            grid_quality: "auto".into(),
            grid_fps: 0,
            hd_fps: 0,
        }
    }
}

/// Pan / tilt / zoom over ONVIF.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PtzConfig {
    pub enabled: bool,
    /// Camera address (empty = from the stream URL).
    pub host: String,
    /// ONVIF port (80 for most cameras; Reolink 8000, Tapo 2020).
    pub port: u16,
    /// Login (empty = from the stream URL).
    pub username: String,
    pub password: String,
    /// Movement speed 0.1–1.
    pub speed: f32,
    pub invert_pan: bool,
    pub invert_tilt: bool,
    /// Patrol: preset tokens visited in turn (empty = off).
    pub patrol: Vec<String>,
    /// Seconds at each patrol preset.
    pub patrol_seconds: u32,
    /// Follow (auto-tracking): turn to keep what the AI finds in view.
    pub follow: bool,
    /// What to follow (empty = person).
    pub follow_objects: Vec<String>,
}

impl Default for PtzConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            host: String::new(),
            port: 80,
            username: String::new(),
            password: String::new(),
            speed: 0.5,
            invert_pan: false,
            invert_tilt: false,
            patrol: Vec::new(),
            patrol_seconds: 30,
            follow: false,
            follow_objects: Vec::new(),
        }
    }
}

/// A zone on the camera picture (see `zones.rs`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ZoneConfig {
    pub name: String,
    /// "area" (polygon) or "line" (two points).
    pub kind: String,
    /// Corners (area) or ends (line), 0..1 of the picture.
    pub points: Vec<[f32; 2]>,
    /// Objects that count here (empty = any).
    pub objects: Vec<String>,
    /// Area: alerts only for objects inside required areas.
    pub required: bool,
    /// Area: report an object staying this many seconds (0 = off).
    pub loiter_seconds: u32,
    /// Line: "any", "ab" or "ba".
    pub direction: String,
    /// Area: report when something was left or taken (parcel, theft).
    /// Real length of the area's long side in metres (0 = off): GAZE then
    /// notes how fast things go through it.
    pub distance_m: f32,
    pub watch_items: bool,
}

impl Default for ZoneConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            kind: "area".into(),
            points: vec![],
            objects: vec![],
            required: false,
            loiter_seconds: 0,
            direction: "any".into(),
            distance_m: 0.0,
            watch_items: false,
        }
    }
}

/// A named set of recording, motion and AI settings that can be applied to
/// any camera (the camera's own detection mask is kept).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Profile {
    pub name: String,
    pub record_mode: RecordMode,
    pub schedule_enabled: bool,
    pub schedule: Schedule,
    pub record_audio: bool,
    pub transcode: bool,
    pub record_fps: u32,
    pub segment_seconds: u64,
    pub retention_days: u64,
    pub max_storage_gb: f64,
    pub detect: DetectConfig,
}

impl Profile {
    /// Put this profile's recording, motion and AI settings on a camera
    /// (its motion mask and everything else stay).
    pub fn apply(&self, cam: &mut CameraConfig) {
        cam.record_mode = self.record_mode;
        cam.schedule_enabled = self.schedule_enabled;
        cam.schedule = self.schedule.clone();
        cam.record_audio = self.record_audio;
        cam.transcode = self.transcode;
        cam.record_fps = self.record_fps;
        cam.segment_seconds = self.segment_seconds;
        cam.retention_days = self.retention_days;
        cam.max_storage_gb = self.max_storage_gb;
        // Areas drawn on this camera's own picture stay the camera's.
        let mask = std::mem::take(&mut cam.detect.mask);
        let (packages, area, spot) = (
            cam.detect.packages,
            std::mem::take(&mut cam.detect.package_area),
            std::mem::take(&mut cam.detect.package_mask),
        );
        cam.detect = self.detect.clone();
        cam.detect.mask = mask;
        cam.detect.packages = packages;
        cam.detect.package_area = area;
        cam.detect.package_mask = spot;
        cam.profile = self.name.clone();
    }
}

impl Default for Profile {
    fn default() -> Self {
        let c = CameraConfig::default();
        Self {
            name: String::new(),
            record_mode: c.record_mode,
            schedule_enabled: c.schedule_enabled,
            schedule: c.schedule,
            record_audio: c.record_audio,
            transcode: c.transcode,
            record_fps: c.record_fps,
            segment_seconds: c.segment_seconds,
            retention_days: c.retention_days,
            max_storage_gb: c.max_storage_gb,
            detect: c.detect,
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
    /// Live sound: a speaker button on the camera's live picture.
    pub live_sound: bool,
    /// Two-way audio: talk through the camera's speaker from the live view.
    pub talk: bool,
    /// RTSP address with the camera's audio back-channel (empty = main stream).
    pub talk_url: String,
    /// A doorbell chime plays through the camera's speaker as soon as you
    /// start talking (to get the attention of whoever is there).
    pub talk_chime: bool,
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
    /// When recording the main stream 24/7: also keep a light sub-stream copy
    /// for fast timeline browsing and previews.
    pub timeline_copy: bool,
    /// Use the camera's own detection (ONVIF events): its person / vehicle
    /// detection, line crossing, doorbell, tampering become alerts.
    /// Privacy areas (x, y, w, h; 0..1 of the picture): blacked out in the
    /// live view, recordings, snapshots and alerts, and never detected in.
    /// Stream live to YouTube / Facebook / Twitch (RTMP address with the key).
    /// A fisheye (360°) camera: the live view is turned into a normal,
    /// flat picture (recordings keep the whole circle).
    pub dewarp: DewarpConfig,
    /// Listen for sounds and alert on them.
    pub sound: SoundConfig,
    pub stream_live: bool,
    pub stream_url: String,
    pub privacy: Vec<[f32; 4]>,
    pub onvif_events: bool,
    /// …and also the camera's own (plain) motion detection.
    pub onvif_motion: bool,
    /// Turn the picture, degrees clockwise: 0, 90, 180 or 270.
    pub rotate: u16,
    /// Name of the recording profile this camera follows ("" = none). When
    /// the profile is changed, the camera's settings follow.
    pub profile: String,
    /// Zones and lines on the picture.
    pub zones: Vec<ZoneConfig>,
    pub detect: DetectConfig,
    pub osd: OsdConfig,
    pub view: ViewConfig,
    pub ptz: PtzConfig,
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
            live_sound: true,
            talk: false,
            talk_chime: true,
            talk_url: String::new(),
            transcode: false,
            record_fps: 0,
            segment_seconds: 0,
            storage_dir: String::new(),
            retention_days: 0,
            max_storage_gb: 0.0,
            timeline_copy: true,
            dewarp: DewarpConfig::default(),
            sound: SoundConfig::default(),
            stream_live: false,
            stream_url: String::new(),
            privacy: Vec::new(),
            onvif_events: false,
            onvif_motion: false,
            rotate: 0,
            profile: String::new(),
            zones: Vec::new(),
            detect: DetectConfig::default(),
            osd: OsdConfig::default(),
            view: ViewConfig::default(),
            ptz: PtzConfig::default(),
        }
    }
}

impl CameraConfig {
    pub fn validate(&self) -> Result<()> {
        if ![0, 90, 180, 270].contains(&self.rotate) {
            bail!("rotation must be 0, 90, 180 or 270 degrees");
        }
        for z in &self.zones {
            if z.name.trim().is_empty() {
                bail!("every zone needs a name");
            }
            let need = if z.kind == "line" { 2 } else { 3 };
            if z.points.len() < need || (z.kind == "line" && z.points.len() != 2) {
                bail!(
                    "zone \"{}\" needs {} points",
                    z.name,
                    if z.kind == "line" { "2" } else { "at least 3" }
                );
            }
        }
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
        if !(1..=255).contains(&d.min_contrast) {
            bail!("motion contrast must be 1 to 255");
        }
        if d.min_object_px > 230_400 || d.make_time_ms > 30_000 || d.min_travel_px > 640 {
            bail!("motion object size, make time or travel distance is out of range");
        }
        if !d.mask.trim().is_empty() && decode_mask(&d.mask).is_none() {
            bail!("invalid motion mask");
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
        if d.join_seconds > 600 {
            bail!("joining alerts is limited to 600 s");
        }
        if d.fps == 0 || d.fps > 15 {
            bail!("detection frame rate must be 1 to 15");
        }
        if self.record_mode.event_clips() && !d.motion && !d.ai {
            bail!("this recording mode needs motion detection or object recognition turned on");
        }
        if d.ai && !d.motion {
            bail!("object recognition only runs when motion is seen: turn on motion detection");
        }
        if d.trigger == "objects" && !d.ai {
            bail!("alerts on objects need object recognition turned on");
        }
        let o = &self.osd;
        for c in [
            &o.name_corner,
            &o.clock_corner,
            &o.status_corner,
            &o.detections_corner,
            &o.fps_corner,
            &o.res_corner,
            &o.text_corner,
        ] {
            if !valid_position(c) {
                bail!("invalid on-screen display position {c:?}");
            }
        }
        if o.text.len() > 200 {
            bail!("on-screen text is too long");
        }
        if !["auto", "main"].contains(&self.view.grid_quality.as_str())
            || self.view.grid_fps > 30
            || self.view.hd_fps > 30
        {
            bail!("live view quality must be auto or main, frame rates 0–30");
        }
        if !(0.05..=1.0).contains(&self.ptz.speed) {
            bail!("PTZ speed must be between 0.05 and 1");
        }
        self.schedule.validate()
    }

    /// Whether going from `old` to `self` needs the camera restarted.
    /// Display, live-view and PTZ settings apply without a restart.
    pub fn needs_restart(&self, old: &CameraConfig) -> bool {
        let strip = |c: &CameraConfig| CameraConfig {
            osd: OsdConfig::default(),
            view: ViewConfig::default(),
            ptz: PtzConfig::default(),
            ..c.clone()
        };
        strip(self) != strip(old)
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

    /// Recordings must be re-encoded (for H.264 conversion, a lower frame
    /// rate, or privacy areas painted in).
    pub fn reencode(&self) -> bool {
        self.transcode || self.record_fps > 0 || !self.privacy.is_empty()
    }

    /// The filters for the live view: privacy areas, then dewarping.
    pub fn view_filter(&self) -> Option<String> {
        let f: Vec<String> = [self.privacy_filter(), self.dewarp.filter()].into_iter().flatten().collect();
        (!f.is_empty()).then(|| f.join(","))
    }

    /// The ffmpeg filter that blacks out the privacy areas (None = none).
    pub fn privacy_filter(&self) -> Option<String> {
        let boxes: Vec<String> = self
            .privacy
            .iter()
            .filter(|b| b[2] > 0.0 && b[3] > 0.0)
            .map(|b| {
                format!(
                    "drawbox=x=iw*{:.4}:y=ih*{:.4}:w=iw*{:.4}:h=ih*{:.4}:color=black:t=fill",
                    b[0].clamp(0.0, 1.0), b[1].clamp(0.0, 1.0), b[2].clamp(0.0, 1.0), b[3].clamp(0.0, 1.0)
                )
            })
            .collect();
        (!boxes.is_empty()).then(|| boxes.join(","))
    }

    /// Black out the privacy areas in an RGB picture.
    pub fn paint_privacy(&self, rgb: &mut [u8], w: u32, h: u32) {
        for b in &self.privacy {
            let x0 = ((b[0].clamp(0.0, 1.0) * w as f32) as u32).min(w);
            let y0 = ((b[1].clamp(0.0, 1.0) * h as f32) as u32).min(h);
            let x1 = (((b[0] + b[2]).clamp(0.0, 1.0) * w as f32).ceil() as u32).min(w);
            let y1 = (((b[1] + b[3]).clamp(0.0, 1.0) * h as f32).ceil() as u32).min(h);
            for y in y0..y1 {
                let row = (y * w * 3) as usize;
                rgb[row + (x0 * 3) as usize..row + (x1 * 3) as usize].fill(0);
            }
        }
    }
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

    /// The settings file couldn't be read: GAZE started in safe mode with
    /// this problem, and saves nothing until it's fixed.
    pub fn safe_mode() -> Option<String> {
        SAFE_MODE.lock().unwrap().clone()
    }
    pub fn set_safe_mode(problem: String) {
        *SAFE_MODE.lock().unwrap() = Some(problem);
    }

    /// For safe mode: the settings that can still be read from a broken
    /// file (where the web interface listens), the rest left at defaults.
    pub fn rescue(text: &str) -> Self {
        let mut cfg = Config::default();
        // Line by line: one broken line mustn't hide the rest.
        let mut section = String::new();
        for line in text.lines() {
            let l = line.trim();
            if l.starts_with('[') {
                section = l.trim_matches(|c| c == '[' || c == ']').trim().to_string();
                continue;
            }
            if section != "server" {
                continue;
            }
            let Some((k, v)) = l.split_once('=') else { continue };
            let v = v.trim().trim_matches('"').to_string();
            match k.trim() {
                "listen" if !v.is_empty() => cfg.server.listen = v,
                "ffmpeg_path" if !v.is_empty() => cfg.server.ffmpeg_path = v.replace("\\\\", "\\"),
                "https_port" => {
                    if let Ok(p) = v.parse() {
                        cfg.server.https_port = p;
                    }
                }
                _ => {}
            }
        }
        cfg
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
        // 0.5.9: object detection is off or built in (the AI servers and
        // cloud services are gone).
        if !AI_PROVIDERS.contains(&self.ai.provider.as_str()) {
            self.ai.provider = "builtin".into();
            changed = true;
        }
        for c in &mut self.cameras {
            // Object recognition only runs on motion now.
            if c.detect.ai && !c.detect.motion {
                c.detect.motion = true;
                changed = true;
            }
            // 0.6.50: one place to watch for parcels — the painted package
            // spot. Zones marked for packages and the older drawn box are
            // painted into it.
            let areas: Vec<Vec<[f32; 2]>> = c
                .zones
                .iter()
                .filter(|z| z.watch_items && z.kind == "area" && z.points.len() >= 3)
                .map(|z| z.points.clone())
                .chain((c.detect.package_area.len() >= 3).then(|| c.detect.package_area.clone()))
                .collect();
            if !areas.is_empty() || c.zones.iter().any(|z| z.watch_items) {
                let mut cells = decode_mask(&c.detect.package_mask).unwrap_or_else(|| vec![false; MASK_W * MASK_H]);
                for (i, cell) in cells.iter_mut().enumerate() {
                    let p = [((i % MASK_W) as f32 + 0.5) / MASK_W as f32, ((i / MASK_W) as f32 + 0.5) / MASK_H as f32];
                    if areas.iter().any(|a| crate::zones::inside(a, p)) {
                        *cell = true;
                    }
                }
                if cells.iter().any(|x| *x) {
                    c.detect.package_mask = encode_mask(&cells);
                    c.detect.packages = true;
                }
                c.detect.package_area.clear();
                for z in &mut c.zones {
                    z.watch_items = false;
                }
                changed = true;
            }
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
        if ![256, 384, 512].contains(&s.timelapse_width) {
            bail!("time-lapse pictures: 256, 384 or 512 pixels wide");
        }
        if !(1..=120).contains(&s.timelapse_seconds) {
            bail!("time-lapse: one picture every 1 to 120 seconds");
        }
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
            .any(|c| !CORNERS.contains(&c.as_str()))
        {
            bail!("positions must be one of {}", CORNERS.join(", "));
        }
        let o = &d.osd;
        for c in [
            &o.name_corner,
            &o.clock_corner,
            &o.status_corner,
            &o.detections_corner,
            &o.fps_corner,
            &o.res_corner,
            &o.text_corner,
        ] {
            if !valid_position(c) {
                bail!("invalid on-screen display position {c:?}");
            }
        }
        if o.text.len() > 200 {
            bail!("on-screen text is too long");
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
        if !ai.verify_model.is_empty() && !BUILTIN_MODELS.contains(&ai.verify_model.as_str()) {
            bail!(
                "unknown AI model \"{}\" for double-checking",
                ai.verify_model
            );
        }
        if ai.builtin_model == "custom" {
            if ai.custom_model.trim().is_empty() {
                bail!("choose your model file (.onnx)");
            }
            if ![320, 416, 480, 512, 608, 640, 736, 800, 960, 1024, 1280].contains(&ai.custom_size) {
                bail!("your model's picture size must be one of 320, 416, 480, 512, 608, 640, 736, 800, 960, 1024 or 1280");
            }
        } else if !BUILTIN_MODELS.contains(&ai.builtin_model.as_str()) {
            bail!("unknown built-in AI model \"{}\"", ai.builtin_model);
        }
        if self.cloud.enabled && self.cloud.remote.trim().is_empty() {
            bail!("enter an rclone destination such as gdrive:NVR");
        }
        if !["ai", "events", "all"].contains(&self.cloud.upload.as_str()) {
            bail!("cloud upload must be ai, events or all");
        }
        self.dashboard.validate()?;
        if self.server.listen.parse::<std::net::SocketAddr>().is_err() {
            bail!("listen address must look like 0.0.0.0:8080");
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
        // Safe mode: the settings file couldn't be read. It's never written
        // over (that would lose everything in it) until it's fixed.
        if Self::safe_mode().is_some() {
            return Ok(());
        }
        // Camera passwords and API keys are in here: only the owner reads it.
        let text = self.to_toml()?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
        private_file(&tmp);
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
/// Only the owner may read this file (Unix; on Windows the user's own
/// folders already are).
pub fn private_file(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// A backup without passwords: what stands in for a camera's password.
const PW_HOLE: &str = "PASSWORD";
/// … and for a live-streaming key (the end of the RTMP address).
const KEY_HOLE: &str = "STREAM-KEY";

/// The password in `scheme://user:password@host/…`, if any.
fn url_password(url: &str) -> Option<&str> {
    let rest = &url[url.find("://")? + 3..];
    let auth = &rest[..rest.find('/').unwrap_or(rest.len())];
    let at = auth.rfind('@')?;
    auth[..at].split_once(':').map(|(_, p)| p)
}

/// `url` with its password replaced by `pw`.
fn with_password(url: &str, pw: &str) -> String {
    match (url.find("://"), url_password(url)) {
        (Some(s), Some(old)) => {
            let rest = &url[s + 3..];
            let at = rest[..rest.find('/').unwrap_or(rest.len())].rfind('@').unwrap_or(0);
            let user = &rest[..at - old.len() - 1];
            format!("{}://{user}:{pw}@{}", &url[..s], &rest[at + 1..])
        }
        _ => url.to_string(),
    }
}

impl Config {
    /// For a backup without passwords: every stored secret is taken out
    /// (camera passwords become "PASSWORD", a streaming key "STREAM-KEY").
    pub fn remove_secrets(&mut self) {
        self.server.password.clear();
        self.server.username.clear();
        self.notify.token.clear();
        self.cloud.password.clear();
        self.mqtt.password.clear();
        for r in &mut self.remotes {
            r.token.clear();
        }
        for c in &mut self.cameras {
            for u in [&mut c.url, &mut c.live_url, &mut c.talk_url] {
                if url_password(u).is_some_and(|p| !p.is_empty()) {
                    *u = with_password(u, PW_HOLE);
                }
            }
            c.ptz.password.clear();
            if let Some(i) = c.stream_url.trim_end_matches('/').rfind('/')
                && !c.stream_url.trim().is_empty()
            {
                c.stream_url = format!("{}/{KEY_HOLE}", &c.stream_url[..i]);
            }
        }
    }

    /// Importing a backup made without passwords: the secrets it left out
    /// are kept from the settings in use (same camera, same server), so
    /// cameras don't lose their passwords.
    pub fn keep_secrets_from(&mut self, old: &Config) {
        let keep = |new: &mut String, old: &str| {
            if new.is_empty() {
                *new = old.to_string();
            }
        };
        keep(&mut self.notify.token, &old.notify.token);
        keep(&mut self.cloud.password, &old.cloud.password);
        keep(&mut self.mqtt.password, &old.mqtt.password);
        for r in &mut self.remotes {
            if let Some(o) = old.remotes.iter().find(|o| o.url.trim() == r.url.trim()) {
                keep(&mut r.token, &o.token);
            }
        }
        for c in &mut self.cameras {
            let Some(o) = old.cameras.iter().find(|o| o.id == c.id) else { continue };
            for (u, ou) in [(&mut c.url, &o.url), (&mut c.live_url, &o.live_url), (&mut c.talk_url, &o.talk_url)] {
                if url_password(u) == Some(PW_HOLE)
                    && let Some(pw) = url_password(ou)
                {
                    *u = with_password(u, pw);
                }
            }
            keep(&mut c.ptz.password, &o.ptz.password);
            if c.stream_url.ends_with(&format!("/{KEY_HOLE}")) {
                c.stream_url = o.stream_url.clone();
            }
        }
    }
}

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
    fn motion_masks() {
        assert!(decode_mask("").is_none());
        let old: String = (0..576)
            .map(|i| if i % 32 < 16 { '1' } else { '0' })
            .collect();
        let m = decode_mask(&old).unwrap();
        assert_eq!(m.len(), MASK_W * MASK_H);
        assert!(m[0] && m[79] && !m[80] && m[MASK_W * 89]);
        let m = decode_mask("160x90:10,5,14385").unwrap();
        assert!(!m[9] && m[10] && m[14] && !m[15]);
        assert!(decode_mask("160x90:10,5").is_none());
        assert!(decode_mask("160x90:99999").is_none());
        assert!(decode_mask("hello").is_none());
    }

    #[test]
    fn backup_secrets_out_and_back() {
        let mut c = Config::default();
        let mut cam = CameraConfig { id: "front".into(), ..Default::default() };
        cam.url = "rtsp://admin:s3cr:et@10.0.0.5:554/Streaming/101".into();
        cam.live_url = "rtsp://10.0.0.5/sub".into();
        cam.stream_url = "rtmp://a.rtmp.youtube.com/live2/abcd-efgh".into();
        cam.ptz.password = "p".into();
        c.cameras.push(cam);
        c.mqtt.password = "m".into();
        let old = c.clone();
        let mut b = c.clone();
        b.remove_secrets();
        let text = b.to_toml().unwrap();
        for s in ["s3cr", "abcd-efgh", "\"m\"", "\"p\""] {
            assert!(!text.contains(s), "{s} left in the backup");
        }
        assert_eq!(b.cameras[0].url, "rtsp://admin:PASSWORD@10.0.0.5:554/Streaming/101");
        assert_eq!(b.cameras[0].live_url, "rtsp://10.0.0.5/sub");
        b.keep_secrets_from(&old);
        assert_eq!(b.cameras[0].url, old.cameras[0].url);
        assert_eq!(b.cameras[0].stream_url, old.cameras[0].stream_url);
        assert_eq!(b.cameras[0].ptz.password, "p");
        assert_eq!(b.mqtt.password, "m");
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
    fn other_ai_providers_become_builtin() {
        let (c, changed) =
            super::Config::from_toml("[ai]\nprovider = \"gemini\"\napi_key = \"x\"\n").unwrap();
        assert!(changed);
        assert_eq!(c.ai.provider, "builtin");
    }

    #[test]
    fn safe_mode_keeps_the_broken_file() {
        let text = "[server]\nname = \"Home\"\nlisten = \"127.0.0.1:8096\"\nthis is broken = = 3\nhttps_port = 9443\n";
        assert!(Config::from_toml(text).is_err());
        let r = Config::rescue(text);
        assert_eq!(r.server.listen, "127.0.0.1:8096");
        assert_eq!(r.server.https_port, 9443);
        let dir = std::env::temp_dir().join(format!("nvr-safe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("nvr.toml");
        std::fs::write(&path, text).unwrap();
        Config::set_safe_mode("broken".into());
        r.save(&path).unwrap();
        *SAFE_MODE.lock().unwrap() = None;
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn package_zones_become_the_painted_spot() {
        let mut cfg = Config::default();
        let mut c = CameraConfig { id: "door".into(), name: "Door".into(), url: "rtsp://x".into(), ..Default::default() };
        c.zones.push(ZoneConfig {
            name: "Step".into(),
            kind: "area".into(),
            points: vec![[0.0, 0.5], [0.5, 0.5], [0.5, 1.0], [0.0, 1.0]],
            watch_items: true,
            ..Default::default()
        });
        cfg.cameras.push(c);
        assert!(cfg.migrate());
        let d = &cfg.cameras[0].detect;
        assert!(d.packages && !cfg.cameras[0].zones[0].watch_items);
        let cells = decode_mask(&d.package_mask).unwrap();
        assert!(cells[(MASK_H * 3 / 4) * MASK_W + MASK_W / 4]); // inside the old zone
        assert!(!cells[(MASK_H / 4) * MASK_W + MASK_W * 3 / 4]); // outside it
        assert!(!cfg.migrate()); // only once
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
