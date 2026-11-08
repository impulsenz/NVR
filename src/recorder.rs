//! Continuous/scheduled recording for one camera: supervises an ffmpeg
//! process that writes fixed-length MP4 files, restarting it if it dies or
//! stalls, and falling back from GPU to CPU if hardware encoding fails.

use crate::config::{CameraConfig, mask_url, redact};
use crate::ffmpeg;
use crate::manager::RuntimeEnv;
use chrono::{DateTime, Local};
use serde::Serialize;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// No new video for this long means the stream is considered stalled.
const STALL_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RecState {
    /// Not recording because of the camera's mode/schedule or because it is paused.
    Idle,
    Connecting,
    Recording,
    Retrying,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecStatus {
    pub state: RecState,
    /// Recording manually paused from the UI (until resumed or restart).
    pub paused: bool,
    /// Whether the mode/schedule currently calls for recording.
    pub scheduled: bool,
    pub since: Option<DateTime<Local>>,
    pub recorded_seconds: f64,
    pub restarts: u32,
    pub last_error: Option<String>,
    /// Description of the GPU pipeline in use, if any.
    pub gpu: Option<String>,
    /// GPU failed and recording switched to the CPU.
    pub gpu_fallback: bool,
    /// Why the GPU failed.
    pub gpu_error: Option<String>,
}

struct Inner {
    status: RecStatus,
    last_progress: Instant,
}

pub struct Recorder {
    cam: CameraConfig,
    env: Arc<RuntimeEnv>,
    want: watch::Sender<bool>,
    paused: AtomicBool,
    use_gpu: AtomicBool,
    inner: Mutex<Inner>,
    cancel: CancellationToken,
}

impl Recorder {
    pub fn new(cam: CameraConfig, env: Arc<RuntimeEnv>, cancel: CancellationToken) -> Arc<Self> {
        let use_gpu = cam.transcode && env.hardware.uses_gpu();
        Arc::new(Self {
            cam,
            env,
            want: watch::Sender::new(false),
            paused: AtomicBool::new(false),
            use_gpu: AtomicBool::new(use_gpu),
            inner: Mutex::new(Inner {
                status: RecStatus {
                    state: RecState::Idle,
                    paused: false,
                    scheduled: false,
                    since: None,
                    recorded_seconds: 0.0,
                    restarts: 0,
                    last_error: None,
                    gpu: None,
                    gpu_fallback: false,
                    gpu_error: None,
                },
                last_progress: Instant::now(),
            }),
            cancel,
        })
    }

    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
        self.reevaluate();
    }

    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
        self.reevaluate();
    }

    pub fn status(&self) -> RecStatus {
        self.inner.lock().unwrap().status.clone()
    }

    /// Recompute whether we should be recording right now.
    fn reevaluate(&self) {
        let scheduled = self.cam.wants_recording(Local::now().naive_local());
        let paused = self.paused.load(Ordering::SeqCst);
        self.update(|s| {
            s.scheduled = scheduled;
            s.paused = paused;
        });
        self.want.send_if_modified(|w| {
            let new = scheduled && !paused;
            let changed = *w != new;
            *w = new;
            changed
        });
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

    fn spawn_ffmpeg(&self, use_gpu: bool) -> std::io::Result<(Child, Option<String>)> {
        let cam = &self.cam;
        let env = &self.env;
        let dir = env.camera_dir(&cam.id);
        std::fs::create_dir_all(&dir)?;
        let pattern = dir.join(format!("{}_%Y-%m-%d_%H-%M-%S.mp4", cam.id));

        let mut cmd = Command::new(&env.ffmpeg_path);
        // stdin stays open so we can send "q" for a clean stop.
        cmd.args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostats",
            "-progress",
            "pipe:1",
        ]);

        let mut gpu_desc = None;
        let mut out_args = vec!["-c:v".to_string(), "copy".to_string()];
        if cam.transcode {
            let hw = &env.hardware;
            let (encoder, keep) = if use_gpu {
                (hw.resolved_encoder(), ffmpeg::full_gpu_pipeline(hw))
            } else {
                ("libx264", false)
            };
            if use_gpu {
                cmd.args(ffmpeg::decode_args(hw, keep));
                gpu_desc = Some(match hw.hwaccel.as_str() {
                    "none" => format!("{encoder} (CPU decode)"),
                    d => format!("{d} decode → {encoder}"),
                });
            }
            let (pre, out) = ffmpeg::encoder_args(encoder, hw, keep);
            cmd.args(pre);
            out_args = out;
        }
        cmd.args(["-fflags", "+genpts"])
            .args(ffmpeg::input_args(&cam.url, &cam.rtsp_transport))
            .args(["-map", "0:v:0"]);
        if cam.record_audio {
            cmd.args(["-map", "0:a:0?", "-c:a", "aac", "-b:a", "64k"]);
        } else {
            cmd.arg("-an");
        }
        cmd.args(out_args)
            .args([
                "-f",
                "segment",
                "-segment_time",
                &env.segment_seconds.to_string(),
            ])
            .args(["-segment_atclocktime", "1", "-segment_format", "mp4"])
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
        Ok((cmd.spawn()?, gpu_desc))
    }

    /// Supervisor loop; runs until the camera is removed or reconfigured.
    pub async fn run(self: Arc<Self>) {
        // Re-check the schedule periodically.
        let me = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(10));
            loop {
                tokio::select! {
                    _ = tick.tick() => me.reevaluate(),
                    _ = me.cancel.cancelled() => return,
                }
            }
        });
        self.reevaluate();

        let mut want = self.want.subscribe();
        let mut backoff = Duration::from_secs(2);
        let mut quick_failures = 0u32;
        let tag = format!("[{}]", self.cam.name);

        loop {
            // Wait until recording is wanted.
            while !*want.borrow_and_update() {
                self.set_state(RecState::Idle);
                tokio::select! {
                    r = want.changed() => if r.is_err() { return },
                    _ = self.cancel.cancelled() => { self.set_state(RecState::Idle); return }
                }
            }

            self.set_state(RecState::Connecting);
            let use_gpu = self.use_gpu.load(Ordering::SeqCst);
            info!("{tag} recorder: connecting to {}", mask_url(&self.cam.url));
            let (mut child, gpu_desc) = match self.spawn_ffmpeg(use_gpu) {
                Ok(c) => c,
                Err(e) => {
                    let msg = format!("failed to start ffmpeg: {e}");
                    warn!("{tag} recorder: {msg}");
                    self.update(|s| s.last_error = Some(msg));
                    self.set_state(RecState::Retrying);
                    if !self.wait_or_disabled(&mut want, backoff).await {
                        return;
                    }
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    continue;
                }
            };
            self.update(|s| {
                s.gpu = gpu_desc;
                s.recorded_seconds = 0.0;
            });
            // `Child::wait()` closes stdin, so keep it separately for the "q" command.
            let stdin = child.stdin.take();
            let started = Instant::now();
            self.inner.lock().unwrap().last_progress = Instant::now();
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
                    _ = wait_until_false(&mut want) => break Exit::Stopped,
                    _ = self.cancel.cancelled() => break Exit::Cancelled,
                    _ = watchdog.tick() => {
                        if self.inner.lock().unwrap().last_progress.elapsed() > STALL_TIMEOUT {
                            break Exit::Stalled;
                        }
                    }
                }
            };

            let stalled = matches!(outcome, Exit::Stalled);
            let reason = match outcome {
                Exit::Stopped | Exit::Cancelled => {
                    info!("{tag} recorder: stopping");
                    graceful_stop(&mut child, stdin).await;
                    self.set_state(RecState::Idle);
                    if matches!(outcome, Exit::Cancelled) {
                        return;
                    }
                    continue;
                }
                Exit::Died(status) => format!("ffmpeg exited ({status})"),
                Exit::Stalled => {
                    let _ = child.kill().await;
                    "no video received for 30s (stream stalled)".to_string()
                }
            };
            warn!(
                "{tag} recorder: {reason}; retrying in {}s",
                backoff.as_secs()
            );

            // GPU fallback: two failures in a row before any video was
            // recorded while using the GPU → switch this camera to the CPU.
            let reached_recording = self.status().since.is_some();
            if reached_recording {
                quick_failures = 0;
            } else if use_gpu {
                quick_failures += 1;
                if quick_failures >= 2 {
                    self.use_gpu.store(false, Ordering::SeqCst);
                    let err = self.status().last_error.unwrap_or_default();
                    warn!("{tag} recorder: GPU acceleration failed, switching to CPU");
                    self.update(|s| {
                        s.gpu_fallback = true;
                        s.gpu_error = Some(err);
                    });
                    backoff = Duration::from_secs(1);
                }
            }

            self.update(|s| {
                s.restarts += 1;
                if s.last_error.is_none() || stalled {
                    s.last_error = Some(reason);
                }
            });
            self.set_state(RecState::Retrying);
            if started.elapsed() > Duration::from_secs(120) {
                backoff = Duration::from_secs(2);
            }
            if !self.wait_or_disabled(&mut want, backoff).await {
                return;
            }
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    }

    /// Sleep for the backoff unless recording is turned off. Returns false if cancelled.
    async fn wait_or_disabled(&self, want: &mut watch::Receiver<bool>, d: Duration) -> bool {
        tokio::select! {
            _ = tokio::time::sleep(d) => true,
            _ = wait_until_false(want) => true,
            _ = self.cancel.cancelled() => { self.set_state(RecState::Idle); false }
        }
    }

    /// Parse `-progress` output. With stream copy there is no frame count, so
    /// advancing output time is the signal that video is arriving.
    async fn read_progress(self: Arc<Self>, stdout: tokio::process::ChildStdout) {
        let mut lines = BufReader::new(stdout).lines();
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
                if s.state == RecState::Connecting || s.state == RecState::Retrying {
                    s.state = RecState::Recording;
                    s.since = Some(Local::now());
                    s.last_error = None;
                    info!("[{}] recorder: recording", self.cam.name);
                }
            }
        }
    }

    async fn read_errors(self: Arc<Self>, stderr: tokio::process::ChildStderr) {
        let mut lines = BufReader::new(stderr).lines();
        let mut first = true;
        let mut filter = ffmpeg::StderrFilter::default();
        while let Ok(Some(raw)) = lines.next_line().await {
            if let Some(s) = filter.summary(false) {
                info!("[{}] recorder: {s}", self.cam.name);
            }
            let Some(line) = filter.filter(&raw) else {
                continue;
            };
            // ffmpeg echoes the input URL in errors; never leak the password.
            let line = redact(&line, &self.cam.url);
            warn!("[{}] ffmpeg: {line}", self.cam.name);
            // The first line of a failing start names the cause; while recording,
            // the latest message is the relevant one.
            let recording = self.status().state == RecState::Recording;
            if first || recording {
                self.update(|s| s.last_error = Some(line));
            }
            first = false;
        }
        if let Some(s) = filter.summary(true) {
            info!("[{}] recorder: {s}", self.cam.name);
        }
    }
}

enum Exit {
    Died(String),
    Stalled,
    Stopped,
    Cancelled,
}

/// Ask ffmpeg to quit cleanly (so the current file is finalized), then kill it.
async fn graceful_stop(child: &mut Child, stdin: Option<ChildStdin>) {
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

async fn wait_until_false(want: &mut watch::Receiver<bool>) {
    loop {
        if !*want.borrow_and_update() {
            return;
        }
        if want.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}
