//! Streaming a camera live to YouTube, Facebook, Twitch or any RTMP server
//! (camera → General → Stream live). The camera's own video is passed
//! through when the service takes it (H.264), so it costs almost nothing;
//! sound is added (silent if the camera has none: YouTube needs a soundtrack).

use crate::config::CameraConfig;
use crate::manager::Manager;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use tracing::{info, warn};

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Status {
    pub live: bool,
    pub since: Option<chrono::DateTime<chrono::Local>>,
    pub error: Option<String>,
}
static STATUS: LazyLock<Mutex<HashMap<String, Status>>> = LazyLock::new(Default::default);
pub fn status(camera: &str) -> Option<Status> {
    STATUS.lock().unwrap().get(camera).cloned()
}

/// The address with the stream key hidden, for logs.
fn safe(url: &str) -> String {
    match url.rfind('/') {
        Some(i) if i + 1 < url.len() => format!("{}/••••", &url[..i]),
        _ => url.to_string(),
    }
}

async fn stream(mgr: &Arc<Manager>, cam: &CameraConfig) -> anyhow::Result<()> {
    let rt = mgr.camera(&cam.id).ok_or_else(|| anyhow::anyhow!("camera not running"))?;
    let relay = rt.relay_main.clone();
    let _connect = relay.subscribe();
    let info = relay.wait_info(Duration::from_secs(15)).await;
    let hevc = info.as_ref().is_some_and(|i| i.codec.contains("265") || i.codec.contains("hevc"));
    let has_audio = relay.audio().is_some_and(|a| !a.is_empty());
    let mut cmd = crate::load::cmd(&mgr.config().server.ffmpeg_path);
    cmd.args(["-hide_banner", "-loglevel", "error", "-fflags", "+genpts"]).args(relay.input_args());
    if !has_audio {
        cmd.args(["-f", "lavfi", "-i", "anullsrc=channel_layout=stereo:sample_rate=44100"]);
    }
    cmd.args(["-map", "0:v:0", "-map", if has_audio { "0:a:0" } else { "1:a:0" }]);
    if hevc || !cam.privacy.is_empty() {
        // Not every service takes H.265, and privacy areas must be painted in.
        cmd.args(["-c:v", "libx264", "-preset", "veryfast", "-tune", "zerolatency", "-g", "50", "-b:v", "3000k", "-maxrate", "3500k", "-bufsize", "6000k"]);
        if let Some(f) = cam.privacy_filter() {
            cmd.args(["-vf", &f]);
        }
    } else {
        cmd.args(["-c:v", "copy"]);
    }
    cmd.args(["-c:a", "aac", "-b:a", "128k", "-ar", "44100"]);
    if !has_audio {
        cmd.arg("-shortest");
    }
    let mut child = cmd
        .args(["-f", "flv", cam.stream_url.trim()])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stop = tokio_util::sync::CancellationToken::new();
    let _feed = crate::relay::feed(relay.subscribe(), child.stdin.take().unwrap(), stop.clone());
    let _stop = stop.drop_guard();
    STATUS.lock().unwrap().insert(cam.id.clone(), Status { live: true, since: Some(chrono::Local::now()), error: None });
    info!("[{}] streaming live to {}", cam.name, safe(&cam.stream_url));
    let mut err = String::new();
    let mut stderr = child.stderr.take().unwrap();
    let wait = async {
        use tokio::io::AsyncReadExt;
        let mut b = vec![0u8; 4096];
        while let Ok(n) = stderr.read(&mut b).await {
            if n == 0 { break; }
            err.push_str(&String::from_utf8_lossy(&b[..n]));
            if err.len() > 2000 { err = err[err.len() - 1000..].to_string(); }
        }
    };
    // Until it ends, or the settings change.
    let cfg_changed = async {
        loop {
            tokio::time::sleep(Duration::from_secs(3)).await;
            if mgr.config().cameras.iter().find(|c| c.id == cam.id) != Some(cam) {
                return;
            }
        }
    };
    tokio::select! {
        _ = wait => {}
        _ = cfg_changed => { return Ok(()); }
    }
    let last = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("the stream stopped").trim().to_string();
    anyhow::bail!("{}", last.replace(cam.stream_url.trim(), &safe(&cam.stream_url)))
}

pub async fn run(mgr: Arc<Manager>) {
    let mut tasks: HashMap<String, (CameraConfig, tokio::task::JoinHandle<()>)> = HashMap::new();
    loop {
        let want: Vec<CameraConfig> = mgr.config().cameras.into_iter().filter(|c| c.enabled && c.stream_live && !c.stream_url.trim().is_empty()).collect();
        tasks.retain(|id, (c, h)| {
            let keep = want.iter().any(|w| w == c) && !h.is_finished();
            if !keep {
                h.abort();
                if !want.iter().any(|w| &w.id == id) {
                    STATUS.lock().unwrap().remove(id);
                }
            }
            keep
        });
        for c in want {
            if tasks.contains_key(&c.id) {
                continue;
            }
            let (m, cam) = (mgr.clone(), c.clone());
            let h = tokio::spawn(async move {
                let mut wait = 5u64;
                loop {
                    match stream(&m, &cam).await {
                        Ok(()) => return,
                        Err(e) => {
                            warn!("[{}] live stream: {e:#} — trying again in {wait} s", cam.name);
                            STATUS.lock().unwrap().insert(cam.id.clone(), Status { live: false, since: None, error: Some(format!("{e:#}")) });
                        }
                    }
                    tokio::time::sleep(Duration::from_secs(wait)).await;
                    wait = (wait * 2).min(120);
                }
            });
            tasks.insert(c.id.clone(), (c, h));
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
