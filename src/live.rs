//! Live view for one camera: a shared ffmpeg process decodes the stream
//! (optionally on the GPU) into JPEG frames. Browsers either hold an MJPEG
//! stream open or poll single frames (used by the multi-camera grid).
//! The process only runs while somebody is watching.

use crate::config::{CameraConfig, redact};
use crate::ffmpeg;
use crate::manager::RuntimeEnv;
use anyhow::{Context, Result, bail};
use bytes::{Bytes, BytesMut};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Keep the stream running this long after the last polled frame.
const POLL_KEEPALIVE: Duration = Duration::from_secs(10);

pub type Frame = Option<(Instant, Bytes)>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    /// Sub-stream (falls back to the main stream), used for the grid.
    Sub,
    /// Main stream in full quality, used when a camera is opened.
    Main,
}

pub struct Live {
    cam: CameraConfig,
    relay: Arc<crate::relay::Relay>,
    /// For the sub-stream: reuse the motion detector's decoded frames
    /// instead of decoding the stream a second time.
    detector: Option<Arc<crate::detect::Detector>>,
    pub quality: Quality,
    fps: Mutex<(u32, Instant, f32)>,
    env: Arc<RuntimeEnv>,
    tx: watch::Sender<Frame>,
    running: Mutex<bool>,
    last_poll: Mutex<Instant>,
    use_gpu: AtomicBool,
    cancel: CancellationToken,
    /// Per-camera live-view options; changing them restarts only the live stream.
    view: Mutex<crate::config::ViewConfig>,
    view_changed: tokio::sync::Notify,
}

impl Live {
    pub fn new(
        cam: CameraConfig,
        env: Arc<RuntimeEnv>,
        quality: Quality,
        relay: Arc<crate::relay::Relay>,
        detector: Option<Arc<crate::detect::Detector>>,
        cancel: CancellationToken,
    ) -> Arc<Self> {
        // The GPU is only worth it for full-quality streams (see HardwareConfig).
        let use_gpu = env.hardware.hwaccel != "none"
            && (quality == Quality::Main || env.hardware.gpu_substreams);
        let cam_view = cam.view.clone();
        Arc::new(Self {
            cam,
            relay,
            detector,
            quality,
            fps: Mutex::new((0, Instant::now(), 0.0)),
            env,
            tx: watch::Sender::new(None),
            running: Mutex::new(false),
            last_poll: Mutex::new(Instant::now() - POLL_KEEPALIVE),
            use_gpu: AtomicBool::new(use_gpu),
            view: Mutex::new(cam_view),
            view_changed: tokio::sync::Notify::new(),
            cancel,
        })
    }

    pub fn viewers(&self) -> usize {
        self.tx.receiver_count()
    }

    /// Frames per second currently produced (0 when idle).
    pub fn fps(&self) -> f32 {
        let f = self.fps.lock().unwrap();
        if f.1.elapsed() > Duration::from_secs(5) {
            0.0
        } else {
            f.2
        }
    }

    /// Frames per second this stream is produced at.
    pub fn target_fps(&self) -> u32 {
        let v = self.view.lock().unwrap();
        match self.quality {
            Quality::Sub if v.grid_fps > 0 => v.grid_fps,
            Quality::Sub => self.env.live.fps,
            Quality::Main if v.hd_fps > 0 => v.hd_fps,
            Quality::Main => self.env.live.hd_fps,
        }
    }

    /// Apply new live-view options (restarts this stream if the rate changes).
    pub fn set_view(&self, v: crate::config::ViewConfig) {
        let before = self.target_fps();
        *self.view.lock().unwrap() = v;
        if self.target_fps() != before {
            self.view_changed.notify_waiters();
        }
    }

    fn q(&self) -> &'static str {
        match self.quality {
            Quality::Sub => "sub-stream",
            Quality::Main => "HD",
        }
    }

    fn url(&self) -> &str {
        match self.quality {
            Quality::Sub => self.cam.live_url(),
            Quality::Main => &self.cam.url,
        }
    }

    fn ensure_running(self: &Arc<Self>) {
        let mut running = self.running.lock().unwrap();
        if !*running && !self.cancel.is_cancelled() {
            *running = true;
            tokio::spawn(self.clone().produce());
        }
    }

    /// Subscribe to live frames (for an MJPEG stream).
    pub fn subscribe(self: &Arc<Self>) -> watch::Receiver<Frame> {
        let rx = self.tx.subscribe();
        self.ensure_running();
        rx
    }

    /// The latest frame, waiting briefly for one if the stream is just starting.
    pub async fn frame(self: &Arc<Self>) -> Result<Bytes> {
        *self.last_poll.lock().unwrap() = Instant::now();
        let mut rx = self.tx.subscribe();
        self.ensure_running();
        let fresh = |f: &Frame| {
            f.as_ref()
                .is_some_and(|(at, _)| at.elapsed() < Duration::from_secs(3))
        };
        let frame = tokio::time::timeout(Duration::from_secs(10), rx.wait_for(fresh))
            .await
            .context("no picture from camera yet")?
            .context("camera removed")?;
        Ok(frame.as_ref().unwrap().1.clone())
    }

    /// A full-resolution JPEG from the main stream.
    pub async fn snapshot(&self) -> Result<Bytes> {
        // Uses the shared camera connection (no extra session on the camera).
        let mut child = Command::new(&self.env.ffmpeg_path)
            .args(["-hide_banner", "-loglevel", "error"])
            .args(self.relay.input_args())
            .args([
                "-frames:v",
                "1",
                "-q:v",
                "2",
                "-f",
                "image2pipe",
                "-c:v",
                "mjpeg",
                "pipe:1",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("running ffmpeg")?;
        let stop = CancellationToken::new();
        let _feed = crate::relay::feed(
            self.relay.subscribe(),
            child.stdin.take().unwrap(),
            stop.clone(),
        );
        let mut out = Vec::new();
        let mut stdout = child.stdout.take().unwrap();
        let r = tokio::time::timeout(Duration::from_secs(25), stdout.read_to_end(&mut out)).await;
        stop.cancel();
        let _ = child.kill().await;
        r.context("timed out waiting for a picture from the camera")??;
        if out.is_empty() {
            let err = self
                .relay
                .status()
                .error
                .unwrap_or_else(|| "no picture".into());
            bail!("no image from camera: {}", redact(&err, &self.cam.url));
        }
        Ok(Bytes::from(out))
    }

    fn wanted(&self) -> bool {
        !self.cancel.is_cancelled()
            && (self.tx.receiver_count() > 0
                || self.last_poll.lock().unwrap().elapsed() < POLL_KEEPALIVE)
    }

    async fn produce(self: Arc<Self>) {
        info!("[{}] live ({}): starting", self.cam.name, self.q());
        let mut failures = 0;
        let mut own_decoder_until = Instant::now();
        loop {
            let use_gpu = self.use_gpu.load(Ordering::SeqCst);
            // Prefer the detector's frames; if it has none (camera starting,
            // detection restarting), decode ourselves for a minute.
            let res = match &self.detector {
                // The detector analyses a few frames per second; faster grid
                // views need their own decoder.
                Some(d)
                    if Instant::now() >= own_decoder_until
                        && self.target_fps() <= self.cam.detect.fps =>
                {
                    let r = self.run_from_detector(d).await;
                    if r.is_err() {
                        own_decoder_until = Instant::now() + Duration::from_secs(60);
                    }
                    r
                }
                _ => self.run_once(use_gpu).await,
            };
            match res {
                Ok(got_frames) if got_frames => failures = 0,
                Ok(_) => failures += 1,
                Err(e) => {
                    warn!("[{}] live ({}): {e:#}", self.cam.name, self.q());
                    failures += 1;
                }
            }
            if use_gpu && failures >= 2 {
                warn!(
                    "[{}] live: GPU decoding failed, switching to CPU",
                    self.cam.name
                );
                self.use_gpu.store(false, Ordering::SeqCst);
            }
            {
                let mut running = self.running.lock().unwrap();
                if !self.wanted() {
                    *running = false;
                    info!(
                        "[{}] live ({}): no viewers, stopping",
                        self.cam.name,
                        self.q()
                    );
                    return;
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                _ = self.cancel.cancelled() => {}
            }
        }
    }

    /// JPEG-encode the motion detector's frames (no extra decoding).
    async fn run_from_detector(&self, det: &Arc<crate::detect::Detector>) -> Result<bool> {
        let mut rx = det.subscribe_frames();
        // ffmpeg's -q:v scale (2 best .. 31 worst) to JPEG quality.
        let quality = (100.0 - self.env.live.quality as f32 * 2.5).clamp(40.0, 95.0) as u8;
        let mut got = false;
        loop {
            tokio::select! {
                r = tokio::time::timeout(Duration::from_secs(10), rx.changed()) => {
                    r.context("no frames from motion detection")?.context("motion detection stopped")?;
                }
                _ = self.cancel.cancelled() => return Ok(got),
                _ = self.view_changed.notified() => return Ok(true),
            }
            let Some(frame) = rx.borrow_and_update().clone() else {
                continue;
            };
            let jpg = tokio::task::spawn_blocking(move || frame.jpeg(quality)).await??;
            got = true;
            self.publish(Bytes::from(jpg));
            if !self.wanted() {
                return Ok(got);
            }
        }
    }

    fn publish(&self, frame: Bytes) {
        self.tx.send_replace(Some((Instant::now(), frame)));
        let mut f = self.fps.lock().unwrap();
        f.0 += 1;
        let el = f.1.elapsed().as_secs_f32();
        if el >= 2.0 {
            f.2 = f.0 as f32 / el;
            f.0 = 0;
            f.1 = Instant::now();
        }
    }

    /// Run ffmpeg until it fails or nobody is watching. Returns whether any frame arrived.
    async fn run_once(&self, use_gpu: bool) -> Result<bool> {
        let live = &self.env.live;
        let fps = self.target_fps();
        let width = match self.quality {
            Quality::Sub => live.width,
            // Full quality, but no wider than 1920 px to keep JPEG encoding affordable.
            Quality::Main => 1920,
        };
        let mut vf = format!("fps={fps}");
        if width > 0 {
            vf.push_str(&format!(",scale='min({width},iw)':-2"));
        }
        let url = self.url().to_string();
        let mut cmd = Command::new(&self.env.ffmpeg_path);
        cmd.args(["-hide_banner", "-loglevel", "error"]);
        if use_gpu {
            cmd.args(ffmpeg::decode_args(&self.env.hardware, false));
        }
        let mut child = cmd
            .args([
                "-fflags",
                "nobuffer+discardcorrupt",
                "-flags",
                "+low_delay-output_corrupt",
            ])
            .args(self.relay.input_args())
            .args(["-an", "-vf", &vf, "-q:v", &live.quality.to_string()])
            .args(["-f", "image2pipe", "-c:v", "mjpeg", "pipe:1"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("starting ffmpeg for live view")?;
        let mut stdout = child.stdout.take().context("no stdout")?;
        let stop_feed = CancellationToken::new();
        let _feed = crate::relay::feed(
            self.relay.subscribe(),
            child.stdin.take().context("no stdin")?,
            stop_feed.clone(),
        );
        let _stop_on_exit = stop_feed.drop_guard();
        if let Some(stderr) = child.stderr.take() {
            let name = self.cam.name.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                let mut filter = ffmpeg::StderrFilter::default();
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Some(msg) = filter.filter(&line) {
                        warn!("[{name}] live ffmpeg: {}", redact(&msg, &url));
                    }
                    if let Some(s) = filter.summary(false) {
                        info!("[{name}] live view: {s}");
                    }
                }
                if let Some(s) = filter.summary(true) {
                    info!("[{name}] live view: {s}");
                }
            });
        }

        let mut buf = BytesMut::with_capacity(512 * 1024);
        let mut chunk = vec![0u8; 64 * 1024];
        let mut scan_from = 0usize;
        let mut got = false;
        loop {
            let n = tokio::select! {
                r = tokio::time::timeout(Duration::from_secs(20), stdout.read(&mut chunk)) => {
                    r.context("live stream stalled")??
                }
                _ = self.cancel.cancelled() => 0,
                _ = self.view_changed.notified() => {
                    let _ = child.kill().await;
                    return Ok(true);
                }
            };
            if n == 0 {
                if self.cancel.is_cancelled() {
                    return Ok(got);
                }
                bail!("live ffmpeg exited");
            }
            buf.extend_from_slice(&chunk[..n]);

            // Split the byte stream into JPEG images (SOI 0xFFD8 .. EOI 0xFFD9).
            while let Some(frame) = next_jpeg(&mut buf, &mut scan_from) {
                got = true;
                self.publish(frame);
            }
            if buf.len() > 8 * 1024 * 1024 {
                buf.clear();
                scan_from = 0;
            }
            if !self.wanted() {
                let _ = child.kill().await;
                return Ok(got);
            }
        }
    }
}

/// Extract the next complete JPEG from `buf`, if any.
pub fn next_jpeg(buf: &mut BytesMut, scan_from: &mut usize) -> Option<Bytes> {
    let start = buf.windows(2).position(|w| w == [0xFF, 0xD8])?;
    if start > 0 {
        let _ = buf.split_to(start);
        *scan_from = 0;
    }
    let from = (*scan_from).max(2);
    if buf.len() < from + 2 {
        return None;
    }
    match buf[from..].windows(2).position(|w| w == [0xFF, 0xD9]) {
        Some(pos) => {
            let end = from + pos + 2;
            *scan_from = 0;
            Some(buf.split_to(end).freeze())
        }
        None => {
            *scan_from = buf.len() - 1;
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_jpegs() {
        let mut buf = BytesMut::new();
        let mut scan = 0;
        buf.extend_from_slice(&[0x00, 0xFF, 0xD8, 1, 2, 0xFF]);
        assert!(next_jpeg(&mut buf, &mut scan).is_none());
        buf.extend_from_slice(&[0xD9, 0xFF, 0xD8, 3, 0xFF, 0xD9]);
        assert_eq!(
            &next_jpeg(&mut buf, &mut scan).unwrap()[..],
            &[0xFF, 0xD8, 1, 2, 0xFF, 0xD9]
        );
        assert_eq!(
            &next_jpeg(&mut buf, &mut scan).unwrap()[..],
            &[0xFF, 0xD8, 3, 0xFF, 0xD9]
        );
        assert!(next_jpeg(&mut buf, &mut scan).is_none());
    }
}
