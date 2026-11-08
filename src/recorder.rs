//! Continuous recording: supervises an ffmpeg process that copies the camera
//! stream into fixed-length MP4 segments, restarting it if it dies or stalls.

use crate::config::{Config, ffmpeg_input_args};
use chrono::{DateTime, Local};
use serde::Serialize;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::watch;
use tracing::{info, warn};

/// No new frames for this long means the stream is considered stalled.
const STALL_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RecState {
    Stopped,
    Connecting,
    Recording,
    Retrying,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecStatus {
    pub state: RecState,
    /// Whether the user wants recording on.
    pub enabled: bool,
    /// When the current ffmpeg session started producing frames.
    pub since: Option<DateTime<Local>>,
    /// Seconds of video recorded in the current session.
    pub recorded_seconds: f64,
    pub restarts: u32,
    pub last_error: Option<String>,
}

struct Inner {
    status: RecStatus,
    last_progress: Instant,
}

pub struct Recorder {
    cfg: Arc<Config>,
    want: watch::Sender<bool>,
    inner: Mutex<Inner>,
}

impl Recorder {
    pub fn new(cfg: Arc<Config>) -> Arc<Self> {
        let enabled = cfg.recording.record_on_start;
        Arc::new(Self {
            cfg,
            want: watch::Sender::new(enabled),
            inner: Mutex::new(Inner {
                status: RecStatus {
                    state: RecState::Stopped,
                    enabled,
                    since: None,
                    recorded_seconds: 0.0,
                    restarts: 0,
                    last_error: None,
                },
                last_progress: Instant::now(),
            }),
        })
    }

    pub fn start(&self) {
        self.want.send_replace(true);
        self.update(|s| s.enabled = true);
    }

    pub fn stop(&self) {
        self.want.send_replace(false);
        self.update(|s| s.enabled = false);
    }

    pub fn status(&self) -> RecStatus {
        self.inner.lock().unwrap().status.clone()
    }

    /// Stop recording and wait (briefly) for the current file to be finalized.
    pub async fn shutdown(&self) {
        self.stop();
        let deadline = Instant::now() + Duration::from_secs(8);
        while self.status().state != RecState::Stopped && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    fn update(&self, f: impl FnOnce(&mut RecStatus)) {
        f(&mut self.inner.lock().unwrap().status);
    }

    fn set_state(&self, state: RecState) {
        self.update(|s| {
            s.state = state;
            if state != RecState::Recording {
                s.since = None;
            }
        });
    }

    fn spawn_ffmpeg(&self) -> std::io::Result<Child> {
        let cfg = &self.cfg;
        std::fs::create_dir_all(&cfg.recording.dir)?;
        let pattern = cfg
            .recording
            .dir
            .join(format!("{}_%Y-%m-%d_%H-%M-%S.mp4", cfg.camera_slug()));

        let mut cmd = Command::new(&cfg.server.ffmpeg_path);
        // stdin stays open so we can send "q" for a clean stop.
        cmd.args(["-hide_banner", "-loglevel", "error", "-nostats"])
            .args(["-progress", "pipe:1"])
            .args(["-fflags", "+genpts"])
            .args(ffmpeg_input_args(&cfg.camera.url))
            .args(["-map", "0:v:0"]);
        if cfg.recording.record_audio {
            cmd.args(["-map", "0:a:0?", "-c:a", "aac", "-b:a", "64k"]);
        } else {
            cmd.arg("-an");
        }
        cmd.args(["-c:v", "copy"])
            .args(["-f", "segment"])
            .args(["-segment_time", &cfg.recording.segment_seconds.to_string()])
            .args(["-segment_atclocktime", "1"])
            .args(["-segment_format", "mp4"])
            // Fragmented MP4: files stay playable even if power is lost mid-recording.
            .args([
                "-segment_format_options",
                "movflags=+frag_keyframe+empty_moov+default_base_moof",
            ])
            .args(["-reset_timestamps", "1", "-strftime", "1"])
            .arg(pattern)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        cmd.spawn()
    }

    /// Supervisor loop; runs for the life of the process.
    pub async fn run(self: Arc<Self>) {
        let mut want = self.want.subscribe();
        let mut backoff = Duration::from_secs(2);

        loop {
            // Wait until recording is wanted.
            while !*want.borrow_and_update() {
                self.set_state(RecState::Stopped);
                if want.changed().await.is_err() {
                    return;
                }
            }

            self.set_state(RecState::Connecting);
            info!(
                "recorder: connecting to {}",
                crate::config::mask_url(&self.cfg.camera.url)
            );
            let mut child = match self.spawn_ffmpeg() {
                Ok(c) => c,
                Err(e) => {
                    let msg = format!("failed to start ffmpeg: {e}");
                    warn!("recorder: {msg}");
                    self.update(|s| s.last_error = Some(msg));
                    self.set_state(RecState::Retrying);
                    wait_or_disabled(&mut want, backoff).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    continue;
                }
            };
            // Take stdin now: `Child::wait()` would otherwise close it, and we
            // need it to send "q" for a clean stop.
            let mut stdin = child.stdin.take();
            let started = Instant::now();
            self.inner.lock().unwrap().last_progress = Instant::now();
            self.update(|s| s.recorded_seconds = 0.0);

            if let Some(stdout) = child.stdout.take() {
                tokio::spawn(self.clone().read_progress(stdout));
            }
            if let Some(stderr) = child.stderr.take() {
                tokio::spawn(self.clone().read_errors(stderr));
            }

            let mut watchdog = tokio::time::interval(Duration::from_secs(5));
            let outcome = loop {
                tokio::select! {
                    res = child.wait() => break Exit::Died(res.map(|s| s.to_string()).unwrap_or_else(|e| e.to_string())),
                    _ = wait_until_disabled(&mut want) => break Exit::Stopped,
                    _ = watchdog.tick() => {
                        let since = self.inner.lock().unwrap().last_progress.elapsed();
                        if since > STALL_TIMEOUT {
                            break Exit::Stalled;
                        }
                    }
                }
            };

            match outcome {
                Exit::Stopped => {
                    info!("recorder: stopping");
                    graceful_stop(&mut child, stdin.take()).await;
                    self.set_state(RecState::Stopped);
                }
                Exit::Died(_) | Exit::Stalled => {
                    let reason = match &outcome {
                        Exit::Stalled => {
                            let _ = child.kill().await;
                            "no video received for 30s (stream stalled)".to_string()
                        }
                        Exit::Died(status) => format!("ffmpeg exited ({status})"),
                        Exit::Stopped => unreachable!(),
                    };
                    warn!("recorder: {reason}; retrying in {}s", backoff.as_secs());
                    self.update(|s| {
                        s.restarts += 1;
                        if s.last_error.is_none() || matches!(outcome, Exit::Stalled) {
                            s.last_error = Some(reason);
                        }
                    });
                    self.set_state(RecState::Retrying);
                    if started.elapsed() > Duration::from_secs(120) {
                        backoff = Duration::from_secs(2);
                    }
                    wait_or_disabled(&mut want, backoff).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                }
            }
        }
    }

    /// Parse `-progress` output (key=value lines) to track health.
    async fn read_progress(self: Arc<Self>, stdout: tokio::process::ChildStdout) {
        let mut lines = BufReader::new(stdout).lines();
        // With stream copy ffmpeg reports no frame count, so advancing
        // output time is the signal that video is arriving.
        let mut last_us = 0i64;
        while let Ok(Some(line)) = lines.next_line().await {
            let Some(value) = line.strip_prefix("out_time_us=") else {
                continue;
            };
            let us: i64 = value.trim().parse().unwrap_or(0);
            if us > last_us {
                last_us = us;
                let mut inner = self.inner.lock().unwrap();
                inner.last_progress = Instant::now();
                let s = &mut inner.status;
                s.recorded_seconds = us as f64 / 1e6;
                if s.state != RecState::Recording && s.enabled {
                    s.state = RecState::Recording;
                    s.since = Some(Local::now());
                    s.last_error = None;
                    info!("recorder: recording");
                }
            }
        }
    }

    async fn read_errors(self: Arc<Self>, stderr: tokio::process::ChildStderr) {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            // ffmpeg echoes the input URL in errors; never leak the password.
            let line = crate::config::redact(line.trim(), &self.cfg.camera.url);
            if line.is_empty() {
                continue;
            }
            warn!("ffmpeg: {line}");
            self.update(|s| s.last_error = Some(line));
        }
    }
}

enum Exit {
    Died(String),
    Stalled,
    Stopped,
}

/// Ask ffmpeg to quit cleanly (so the current file is finalized), then kill it.
async fn graceful_stop(child: &mut Child, stdin: Option<tokio::process::ChildStdin>) {
    if let Some(mut stdin) = stdin {
        let _ = stdin.write_all(b"q").await;
        let _ = stdin.flush().await;
    }
    if tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .is_err()
    {
        let _ = child.kill().await;
    }
}

async fn wait_until_disabled(want: &mut watch::Receiver<bool>) {
    loop {
        if !*want.borrow_and_update() {
            return;
        }
        if want.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

async fn wait_or_disabled(want: &mut watch::Receiver<bool>, d: Duration) {
    tokio::select! {
        _ = tokio::time::sleep(d) => {}
        _ = wait_until_disabled(want) => {}
    }
}
