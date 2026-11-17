//! go2rtc as the camera connection layer (Settings → System → Streaming
//! engine), an option next to the built-in FFmpeg pipeline.
//!
//! With "go2rtc" chosen, GAZE runs go2rtc (github.com/AlexxIT/go2rtc, MIT)
//! as a supervised child process. go2rtc holds the one connection to each
//! camera stream and re-serves it on this computer only
//! (`rtsp://127.0.0.1:18554/<camera>_main` / `_sub`); GAZE's own relays read
//! from there, so recording, detection and live view work exactly as before.
//!
//! Recording never depends on it: each camera connection asks [`source`]
//! where to connect *at every (re)connect*, and gets the camera itself
//! whenever go2rtc isn't running and answering. Switching back to FFmpeg
//! stops go2rtc and the cameras reconnect directly.
//!
//! The FFmpeg pipeline is untouched and stays the default.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{info, warn};

/// go2rtc's RTSP server and API: on this computer only.
pub const RTSP_PORT: u16 = 18554;
pub const API_PORT: u16 = 11984;
/// The go2rtc version fetched when none is installed.
const VERSION: &str = "1.9.9";

/// go2rtc is running and answering (checked every few seconds).
static HEALTHY: AtomicBool = AtomicBool::new(false);
/// What the Settings page shows.
static STATUS: Mutex<String> = Mutex::new(String::new());

pub fn healthy() -> bool {
    HEALTHY.load(Ordering::Relaxed)
}

pub fn status() -> String {
    STATUS.lock().unwrap().clone()
}

fn set_status(s: impl Into<String>) {
    *STATUS.lock().unwrap() = s.into();
}

/// The name of one camera stream in go2rtc.
pub fn stream_name(camera: &str, sub: bool) -> String {
    let safe: String = camera.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    format!("{safe}_{}", if sub { "sub" } else { "main" })
}

/// Where a camera stream is read from right now: through go2rtc when it's
/// chosen and running, otherwise the camera itself.
pub fn source(camera_url: &str, camera: &str, sub: bool) -> String {
    if healthy() {
        format!("rtsp://127.0.0.1:{RTSP_PORT}/{}", stream_name(camera, sub))
    } else {
        camera_url.to_string()
    }
}

/// go2rtc's settings for these cameras: every main and sub-stream, the RTSP
/// server and API on this computer only, nothing else listening.
pub fn yaml(cameras: &[crate::config::CameraConfig]) -> String {
    // Values quoted for YAML (camera addresses contain ':' '@' '#' …).
    let q = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    let mut out = String::from("# Written by GAZE: changes here are replaced.\n");
    out += &format!("api:\n  listen: {}\n", q(&format!("127.0.0.1:{API_PORT}")));
    out += &format!("rtsp:\n  listen: {}\n", q(&format!("127.0.0.1:{RTSP_PORT}")));
    // No WebRTC / RTMP / SRTP servers on the network (not used yet).
    out += "webrtc:\n  listen: \"\"\nrtmp:\n  listen: \"\"\nsrtp:\n  listen: \"\"\n";
    out += "log:\n  level: warn\n";
    out += "streams:\n";
    for c in cameras.iter().filter(|c| c.enabled) {
        out += &format!("  {}: {}\n", stream_name(&c.id, false), q(&source_for(&c.url)));
        if c.has_sub() {
            out += &format!("  {}: {}\n", stream_name(&c.id, true), q(&source_for(&c.live_url)));
        }
    }
    out
}

/// A camera address as a go2rtc source. RTSP: without two-way audio
/// (`#backchannel=0`). go2rtc otherwise asks the camera for its talk-back
/// channel, which some cameras (TP-Link Tapo among them) answer badly — no
/// picture at all — and GAZE talks through cameras itself anyway.
fn source_for(url: &str) -> String {
    let u = url.trim();
    // Only streaming addresses (go2rtc also reads files and runs programs).
    if !crate::config::stream_url_ok(u) {
        return String::new();
    }
    if (u.starts_with("rtsp://") || u.starts_with("rtsps://")) && !u.contains("#backchannel") {
        format!("{u}#backchannel=0")
    } else {
        u.to_string()
    }
}

/// What go2rtc says about one stream (its connection to the camera and any
/// error), for the log when a camera gets no picture through it.
pub async fn stream_report(camera: &str, sub: bool, secrets: &[&str]) -> Option<String> {
    let name = stream_name(camera, sub);
    let c = reqwest::Client::builder().timeout(Duration::from_secs(3)).build().ok()?;
    let v: serde_json::Value = c.get(format!("http://127.0.0.1:{API_PORT}/api/streams?src={name}")).send().await.ok()?.json().await.ok()?;
    let mut text = v.to_string();
    for s in secrets {
        text = crate::config::redact(&text, s);
    }
    Some(text.chars().take(1500).collect())
}

/// The go2rtc program: the one set in Settings, GAZE's own, or none.
fn program(set: &str, data_dir: &Path) -> Option<PathBuf> {
    if !set.trim().is_empty() {
        return Some(PathBuf::from(set.trim()));
    }
    let name = if cfg!(windows) { "go2rtc.exe" } else { "go2rtc" };
    let p = crate::getffmpeg::own_dir(data_dir).with_file_name("go2rtc").join(name);
    p.exists().then_some(p)
}

/// The release file for this computer.
fn asset() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("go2rtc_win64.zip"),
        ("linux", "x86_64") => Some("go2rtc_linux_amd64"),
        ("linux", "aarch64") => Some("go2rtc_linux_arm64"),
        _ => None,
    }
}

/// Download go2rtc into GAZE's own folder.
async fn fetch(data_dir: &Path) -> Result<PathBuf> {
    let Some(asset) = asset() else {
        bail!("no ready-made go2rtc for this computer: download it from github.com/AlexxIT/go2rtc and set its path");
    };
    let dir = crate::getffmpeg::own_dir(data_dir).with_file_name("go2rtc");
    std::fs::create_dir_all(&dir)?;
    let url = format!("https://github.com/AlexxIT/go2rtc/releases/download/v{VERSION}/{asset}");
    set_status(format!("downloading go2rtc {VERSION}…"));
    info!("go2rtc: downloading {url}");
    let bytes = crate::jobs::download_client()?.get(&url).send().await?.error_for_status()?.bytes().await?;
    anyhow::ensure!(bytes.len() > 1_000_000, "the go2rtc download is incomplete");
    let name = if cfg!(windows) { "go2rtc.exe" } else { "go2rtc" };
    let out = dir.join(name);
    if asset.ends_with(".zip") {
        use std::io::Read;
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec()))?;
        let mut f = z.by_name(name).context("go2rtc.exe isn't in the download")?;
        let mut b = Vec::new();
        f.read_to_end(&mut b)?;
        std::fs::write(&out, b)?;
    } else {
        std::fs::write(&out, &bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o755))?;
        }
    }
    Ok(out)
}

/// Is go2rtc answering on its API?
async fn answering() -> bool {
    let Ok(c) = reqwest::Client::builder().timeout(Duration::from_secs(3)).build() else { return false };
    c.get(format!("http://127.0.0.1:{API_PORT}/api")).send().await.is_ok_and(|r| r.status().is_success())
}

/// Supervise go2rtc for the life of GAZE: started when chosen, restarted
/// (with growing pauses) if it stops, given new settings when the cameras
/// change, stopped when FFmpeg is chosen again.
pub async fn run(mgr: Arc<crate::manager::Manager>) {
    let mut backoff = Duration::from_secs(2);
    loop {
        let cfg = mgr.config();
        if cfg.streaming.engine != "go2rtc" {
            HEALTHY.store(false, Ordering::Relaxed);
            set_status("");
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }
        let started = std::time::Instant::now();
        let res = serve(&mgr).await;
        // It had been running fine for a while: start again promptly.
        if started.elapsed() > Duration::from_secs(120) {
            backoff = Duration::from_secs(2);
        }
        match res {
            Ok(()) => backoff = Duration::from_secs(2),
            Err(e) => {
                warn!("go2rtc: {e:#} — cameras connect directly meanwhile; trying again in {}s", backoff.as_secs());
                set_status(format!("not running: {e:#} (cameras connect directly)"));
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(300));
            }
        }
        HEALTHY.store(false, Ordering::Relaxed);
    }
}

/// One run of go2rtc, until it stops, its settings change or FFmpeg is
/// chosen again.
async fn serve(mgr: &Arc<crate::manager::Manager>) -> Result<()> {
    let cfg = mgr.config();
    let exe = match program(&cfg.streaming.go2rtc_path, &mgr.data_dir) {
        Some(p) => p,
        None => fetch(&mgr.data_dir).await?,
    };
    let conf = mgr.data_dir.join("go2rtc.yaml");
    let text = yaml(&cfg.cameras);
    std::fs::write(&conf, &text)?;
    // It holds the camera passwords: only for this account.
    crate::config::private_file(&conf);
    let mut child = crate::load::cmd(&exe)
        .arg("-config")
        .arg(&conf)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("can't start {}", exe.display()))?;
    // Its warnings go to GAZE's log (camera passwords hidden).
    if let Some(err) = child.stderr.take() {
        let cams = cfg.cameras.clone();
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let mut lines = tokio::io::BufReader::new(err).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                let l = cams.iter().fold(l, |l, c| crate::config::redact(&crate::config::redact(&l, &c.url), &c.live_url));
                warn!("go2rtc: {l}");
            }
        });
    }
    // Ready when its API answers.
    let mut up = false;
    for _ in 0..30 {
        if answering().await {
            up = true;
            break;
        }
        if let Ok(Some(s)) = child.try_wait() {
            bail!("go2rtc stopped straight away ({s})");
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    if !up {
        bail!("go2rtc didn't answer within 15 s");
    }
    info!("go2rtc: running ({} stream(s)); cameras now connect through it", text.matches("rtsp://").count() + text.matches("rtsps://").count());
    HEALTHY.store(true, Ordering::Relaxed);
    set_status(format!("running — {}", exe.display()));
    let mut misses = 0;
    loop {
        tokio::select! {
            s = child.wait() => {
                HEALTHY.store(false, Ordering::Relaxed);
                bail!("go2rtc stopped ({})", s.map(|s| s.to_string()).unwrap_or_else(|e| e.to_string()));
            }
            _ = tokio::time::sleep(Duration::from_secs(5)) => {}
        }
        let now = mgr.config();
        if now.streaming.engine != "go2rtc" {
            HEALTHY.store(false, Ordering::Relaxed);
            info!("go2rtc: switched back to FFmpeg — stopped; cameras connect directly");
            let _ = child.kill().await;
            return Ok(());
        }
        if yaml(&now.cameras) != text {
            info!("go2rtc: cameras changed — restarting it with the new list");
            HEALTHY.store(false, Ordering::Relaxed);
            let _ = child.kill().await;
            return Ok(());
        }
        // Not answering three times running: restarted.
        if answering().await {
            misses = 0;
        } else {
            misses += 1;
            if misses >= 3 {
                HEALTHY.store(false, Ordering::Relaxed);
                let _ = child.kill().await;
                bail!("go2rtc stopped answering");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CameraConfig;

    #[test]
    fn settings_hold_every_stream_on_this_computer_only() {
        let mut a = CameraConfig { id: "front door".into(), name: "Front".into(), url: "rtsp://admin:p@ss#1@10.0.0.5/main".into(), ..Default::default() };
        a.live_url = "rtsp://admin:p@ss#1@10.0.0.5/sub".into();
        let mut b = CameraConfig { id: "yard".into(), name: "Yard".into(), url: "rtsp://10.0.0.6/s0".into(), ..Default::default() };
        b.enabled = true;
        let off = CameraConfig { id: "off".into(), enabled: false, url: "rtsp://10.0.0.7/x".into(), ..Default::default() };
        let y = yaml(&[a, b, off]);
        assert!(y.contains("  front_door_main: \"rtsp://admin:p@ss#1@10.0.0.5/main#backchannel=0\"\n"));
        assert!(y.contains("  front_door_sub: \"rtsp://admin:p@ss#1@10.0.0.5/sub#backchannel=0\"\n"));
        assert!(y.contains("  yard_main:"));
        assert!(!y.contains("yard_sub") && !y.contains("off_main"));
        assert!(y.contains("listen: \"127.0.0.1:18554\"") && y.contains("listen: \"127.0.0.1:11984\""));
        assert!(y.contains("webrtc:\n  listen: \"\""));
    }

    #[test]
    fn cameras_connect_directly_unless_go2rtc_is_running() {
        HEALTHY.store(false, Ordering::Relaxed);
        assert_eq!(source("rtsp://cam/main", "front", false), "rtsp://cam/main");
        HEALTHY.store(true, Ordering::Relaxed);
        assert_eq!(source("rtsp://cam/main", "front", true), "rtsp://127.0.0.1:18554/front_sub");
        HEALTHY.store(false, Ordering::Relaxed);
    }
}
