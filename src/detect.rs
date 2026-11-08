//! Motion detection and object recognition for one camera.
//!
//! An ffmpeg process decodes the sub-stream at a few frames per second into
//! raw RGB. Motion is found by comparing each frame with a slowly updated
//! background. On motion, frames are sent to the AI (at most every
//! `ai_interval` seconds). Activity is grouped into events, saved with a
//! thumbnail, and (in event-recording modes) turned into full-quality clips.

use crate::ai::{AiError, AiHandle, Detection};
use crate::buffer::EventBuffer;
use crate::config::{CameraConfig, redact};
use crate::events::{Event, EventStore, Label};
use crate::ffmpeg;
use crate::imgutil::Frame;
use crate::manager::RuntimeEnv;
use anyhow::{Context, Result};
use chrono::{DateTime, Duration as CDuration, Local};
use serde::Serialize;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Events longer than this are split.
const MAX_EVENT_SECONDS: i64 = 600;
const ANALYSIS_WIDTH: u32 = 640;

#[derive(Debug, Clone, Serialize, Default)]
pub struct DetectStatus {
    pub running: bool,
    /// Motion right now.
    pub motion: bool,
    /// Share of the picture that changed, 0..1.
    pub motion_level: f32,
    /// Objects seen in the event in progress.
    pub active_labels: Vec<Label>,
    pub event_active: bool,
    pub last_event: Option<DateTime<Local>>,
    pub ai_error: Option<String>,
    pub error: Option<String>,
    pub fps: f32,
}

pub struct Detector {
    cam: CameraConfig,
    env: Arc<RuntimeEnv>,
    ai: Arc<AiHandle>,
    store: Arc<EventStore>,
    buffer: Option<Arc<EventBuffer>>,
    status: Mutex<DetectStatus>,
    latest: Mutex<Option<Frame>>,
    /// (width, height) of the sub-stream, if known.
    source_size: Option<(u32, u32)>,
}

struct Active {
    event: Event,
    last_activity: DateTime<Local>,
    qualified: bool,
    thumb_score: f32,
}

impl Detector {
    pub fn new(
        cam: CameraConfig,
        env: Arc<RuntimeEnv>,
        ai: Arc<AiHandle>,
        store: Arc<EventStore>,
        buffer: Option<Arc<EventBuffer>>,
        source_size: Option<(u32, u32)>,
    ) -> Arc<Self> {
        Arc::new(Self {
            cam,
            env,
            ai,
            store,
            buffer,
            status: Mutex::new(DetectStatus::default()),
            latest: Mutex::new(None),
            source_size,
        })
    }

    pub fn status(&self) -> DetectStatus {
        self.status.lock().unwrap().clone()
    }

    /// The most recent analysed frame.
    pub fn latest_frame(&self) -> Option<Frame> {
        self.latest.lock().unwrap().clone()
    }

    fn frame_size(&self) -> (u32, u32) {
        let (sw, sh) = self.source_size.unwrap_or((16, 9));
        let w = ANALYSIS_WIDTH.min(sw.max(2)) & !1;
        let h = (((w as f32 * sh as f32 / sw.max(1) as f32).round() as u32).max(2)) & !1;
        (w, h)
    }

    pub async fn run(self: Arc<Self>, cancel: CancellationToken) {
        let mut use_gpu = self.env.hardware.hwaccel != "none";
        let mut failures = 0;
        let mut active: Option<Active> = None;
        loop {
            let started = Instant::now();
            let res = self.clone().run_once(use_gpu, &cancel, &mut active).await;
            if cancel.is_cancelled() {
                if let Some(a) = active.take() {
                    self.finish(a);
                }
                self.status.lock().unwrap().running = false;
                return;
            }
            if let Err(e) = res {
                let msg = redact(&format!("{e:#}"), self.cam.live_url());
                warn!("[{}] detection: {msg}", self.cam.name);
                self.status.lock().unwrap().error = Some(msg);
            }
            self.status.lock().unwrap().running = false;
            if started.elapsed() < Duration::from_secs(15) {
                failures += 1;
                if use_gpu && failures >= 2 {
                    warn!(
                        "[{}] detection: GPU decoding failed, using CPU",
                        self.cam.name
                    );
                    use_gpu = false;
                }
            } else {
                failures = 0;
            }
            let wait = Duration::from_secs((2u64 << failures.min(5)).min(60));
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = cancel.cancelled() => {}
            }
        }
    }

    async fn run_once(
        self: Arc<Self>,
        use_gpu: bool,
        cancel: &CancellationToken,
        active: &mut Option<Active>,
    ) -> Result<()> {
        let d = &self.cam.detect;
        let (w, h) = self.frame_size();
        let url = self.cam.live_url().to_string();
        let mut cmd = tokio::process::Command::new(&self.env.ffmpeg_path);
        cmd.args(["-hide_banner", "-nostdin", "-loglevel", "error"]);
        if use_gpu {
            cmd.args(ffmpeg::decode_args(&self.env.hardware, false));
        }
        let mut child = cmd
            .args(ffmpeg::input_args(&url, &self.cam.rtsp_transport))
            .args([
                "-an",
                "-vf",
                &format!("fps={},scale={w}:{h}", d.fps),
                "-pix_fmt",
                "rgb24",
                "-f",
                "rawvideo",
                "pipe:1",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("starting ffmpeg for detection")?;
        let mut stdout = child.stdout.take().context("no stdout")?;
        if let Some(stderr) = child.stderr.take() {
            let name = self.cam.name.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                let mut filter = ffmpeg::StderrFilter::default();
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Some(msg) = filter.filter(&line) {
                        warn!("[{name}] detection ffmpeg: {}", redact(&msg, &url));
                    }
                }
            });
        }
        {
            let mut s = self.status.lock().unwrap();
            s.running = true;
            s.error = None;
        }
        info!(
            "[{}] detection: started ({}×{} at {} fps)",
            self.cam.name, w, h, d.fps
        );

        let frame_len = (w * h * 3) as usize;
        let mut motion = MotionDetector::new(w, h, d.sensitivity);
        let (ai_tx, mut ai_rx) = mpsc::channel::<(Frame, Result<Vec<Detection>, AiError>)>(4);
        let mut ai_busy = false;
        let mut last_ai = Instant::now() - Duration::from_secs(3600);
        let mut fps_count = 0u32;
        let mut fps_since = Instant::now();
        let mut tick = tokio::time::interval(Duration::from_secs(1));

        loop {
            let mut buf = vec![0u8; frame_len];
            tokio::select! {
                r = tokio::time::timeout(Duration::from_secs(20), stdout.read_exact(&mut buf)) => {
                    r.context("no video for 20s")?.context("detection stream ended")?;
                }
                Some((frame, res)) = ai_rx.recv() => {
                    ai_busy = false;
                    self.handle_ai(frame, res, active);
                    continue;
                }
                _ = tick.tick() => {
                    self.check_end(active);
                    continue;
                }
                _ = cancel.cancelled() => {
                    let _ = child.kill().await;
                    return Ok(());
                }
            }
            let frame = Frame {
                width: w,
                height: h,
                rgb: Arc::new(buf),
            };
            *self.latest.lock().unwrap() = Some(frame.clone());
            fps_count += 1;
            if fps_since.elapsed() >= Duration::from_secs(5) {
                self.status.lock().unwrap().fps =
                    fps_count as f32 / fps_since.elapsed().as_secs_f32();
                fps_count = 0;
                fps_since = Instant::now();
            }

            let m = motion.feed(&frame.rgb);
            let moving = d.motion && m.as_ref().is_some_and(|m| m.moving);
            {
                let mut s = self.status.lock().unwrap();
                s.motion = moving;
                s.motion_level = m.as_ref().map(|m| m.fraction).unwrap_or(0.0);
            }
            let now = Local::now();
            if moving {
                let bbox = m.as_ref().map(|m| m.bbox).unwrap_or((0.0, 0.0, 1.0, 1.0));
                self.on_motion(active, now, &frame, bbox);
            }

            // Object recognition: on motion, or continuously if motion detection is off.
            let ai = self.ai.get();
            if d.ai
                && ai.enabled()
                && !ai_busy
                && (moving || !d.motion || active.is_some())
                && last_ai.elapsed().as_secs_f64() >= d.ai_interval
            {
                ai_busy = true;
                last_ai = Instant::now();
                let tx = ai_tx.clone();
                let f = frame.clone();
                tokio::spawn(async move {
                    let res = ai.detect(&f).await;
                    let _ = tx.send((f, res)).await;
                });
            }
            self.check_end(active);
        }
    }

    fn new_event(&self, now: DateTime<Local>) -> Active {
        let id = format!(
            "ev-{}-{:03}",
            now.format("%Y%m%d-%H%M%S"),
            now.timestamp_subsec_millis()
        );
        if let Some(b) = &self.buffer {
            b.hold(&id, now);
        }
        Active {
            event: Event {
                id,
                camera: self.cam.id.clone(),
                start: now,
                end: now,
                labels: vec![],
                motion: false,
                thumb: None,
                clip: None,
                clip_start: None,
            },
            last_activity: now,
            qualified: false,
            thumb_score: -1.0,
        }
    }

    fn on_motion(
        &self,
        active: &mut Option<Active>,
        now: DateTime<Local>,
        frame: &Frame,
        bbox: (f32, f32, f32, f32),
    ) {
        let a = active.get_or_insert_with(|| self.new_event(now));
        a.event.motion = true;
        // With "objects" as the trigger, motion only keeps a pending event open.
        a.last_activity = now;
        if a.thumb_score < 0.0 {
            // Motion thumbnail until an object is recognized.
            self.save_thumb(a, frame, bbox, 0.0);
        }
        if self.cam.detect.trigger == "motion" && !a.qualified {
            a.qualified = true;
            info!(target: "event", "[{}] motion detected", self.cam.name);
            self.store.save(a.event.clone());
            self.mark_active(a);
        }
    }

    fn handle_ai(
        &self,
        frame: Frame,
        res: Result<Vec<Detection>, AiError>,
        active: &mut Option<Active>,
    ) {
        let d = &self.cam.detect;
        let dets = match res {
            Ok(d) => {
                self.status.lock().unwrap().ai_error = None;
                d
            }
            Err(AiError::RateLimited) => return,
            Err(e) => {
                let msg = e.to_string();
                let mut s = self.status.lock().unwrap();
                if s.ai_error.as_deref() != Some(&msg) {
                    warn!("[{}] AI: {msg}", self.cam.name);
                }
                s.ai_error = Some(msg);
                return;
            }
        };
        let hits: Vec<Detection> = dets
            .into_iter()
            .filter(|x| x.score >= d.min_confidence && d.objects.contains(&x.label))
            .collect();
        if hits.is_empty() {
            return;
        }
        let now = Local::now();
        let a = active.get_or_insert_with(|| self.new_event(now));
        a.last_activity = now;
        let mut new_labels = Vec::new();
        let mut changed = !a.qualified;
        for h in &hits {
            match a.event.labels.iter_mut().find(|l| l.label == h.label) {
                Some(l) => {
                    if h.score > l.score + 0.05 {
                        changed = true;
                    }
                    l.score = l.score.max(h.score);
                }
                None => {
                    a.event.labels.push(Label {
                        label: h.label.clone(),
                        score: h.score,
                    });
                    new_labels.push(format!("{} {:.0}%", h.label, h.score * 100.0));
                    changed = true;
                }
            }
        }
        a.event.labels.sort_by(|x, y| y.score.total_cmp(&x.score));
        let best = hits
            .iter()
            .max_by(|x, y| x.score.total_cmp(&y.score))
            .unwrap();
        if best.score > a.thumb_score + 0.05 {
            self.save_thumb(a, &frame, (best.x, best.y, best.w, best.h), best.score);
            changed = true;
        }
        if !new_labels.is_empty() {
            info!(target: "event", "[{}] alert: {}", self.cam.name, new_labels.join(", "));
        }
        a.qualified = true;
        // Only write when something changed (the final state is saved when the event ends).
        if changed {
            self.store.save(a.event.clone());
        }
        self.mark_active(a);
    }

    fn save_thumb(
        &self,
        a: &mut Active,
        frame: &Frame,
        (x, y, w, h): (f32, f32, f32, f32),
        score: f32,
    ) {
        let crop = frame.crop(x, y, w, h, 0.25, 200).fit(480);
        let name = format!("{}.jpg", a.event.id);
        let _ = std::fs::create_dir_all(self.store.dir());
        if let Ok(j) = crop.jpeg(85)
            && std::fs::write(self.store.dir().join(&name), j).is_ok()
        {
            a.event.thumb = Some(name);
            a.thumb_score = score;
        }
    }

    fn mark_active(&self, a: &Active) {
        let mut s = self.status.lock().unwrap();
        s.event_active = true;
        s.active_labels = a.event.labels.clone();
        s.last_event = Some(a.event.start);
    }

    /// End the event when activity has stopped for the post-event time.
    fn check_end(&self, active: &mut Option<Active>) {
        let Some(a) = active.as_ref() else { return };
        let now = Local::now();
        let idle = (now - a.last_activity).num_seconds();
        let post = self.cam.detect.post_seconds.max(3) as i64;
        let too_long = (now - a.event.start).num_seconds() > MAX_EVENT_SECONDS;
        if idle >= post || too_long {
            let a = active.take().unwrap();
            self.finish(a);
        }
    }

    fn finish(&self, mut a: Active) {
        {
            let mut s = self.status.lock().unwrap();
            s.event_active = false;
            s.active_labels.clear();
        }
        let d = &self.cam.detect;
        if !a.qualified {
            if let Some(b) = &self.buffer {
                b.release(&a.event.id);
            }
            if let Some(t) = &a.event.thumb {
                let _ = std::fs::remove_file(self.store.dir().join(t));
            }
            return;
        }
        a.event.end = a.last_activity.max(a.event.start + CDuration::seconds(1));
        self.store.save(a.event.clone());
        if let Some(b) = &self.buffer {
            b.finish(
                &a.event.id,
                a.event.start - CDuration::seconds(d.pre_seconds as i64),
                a.event.end + CDuration::seconds(d.post_seconds as i64),
            );
        }
    }
}

pub struct MotionResult {
    pub moving: bool,
    pub fraction: f32,
    /// Normalized (x, y, w, h) of the changed area.
    pub bbox: (f32, f32, f32, f32),
}

/// Background-subtraction motion detector on a 4× downscaled grey image.
pub struct MotionDetector {
    w: usize,
    h: usize,
    src_w: usize,
    bg: Vec<f32>,
    frames: u32,
    streak: u32,
    threshold: f32,
    min_fraction: f32,
}

impl MotionDetector {
    pub fn new(src_w: u32, src_h: u32, sensitivity: u32) -> Self {
        let (w, h) = ((src_w / 4).max(1) as usize, (src_h / 4).max(1) as usize);
        let s = sensitivity.clamp(1, 100) as f32;
        Self {
            w,
            h,
            src_w: src_w as usize,
            bg: vec![0.0; w * h],
            frames: 0,
            streak: 0,
            // Sensitivity 1 → large changes only; 100 → small changes.
            threshold: 45.0 - s * 0.33,
            min_fraction: 0.03 - s * 0.000_28,
        }
    }

    fn grey(&self, rgb: &[u8]) -> Vec<f32> {
        let mut g = vec![0.0f32; self.w * self.h];
        for y in 0..self.h {
            for x in 0..self.w {
                let mut sum = 0.0;
                for dy in 0..4 {
                    for dx in 0..4 {
                        let i = ((y * 4 + dy) * self.src_w + x * 4 + dx) * 3;
                        sum += 0.299 * rgb[i] as f32
                            + 0.587 * rgb[i + 1] as f32
                            + 0.114 * rgb[i + 2] as f32;
                    }
                }
                g[y * self.w + x] = sum / 16.0;
            }
        }
        g
    }

    pub fn feed(&mut self, rgb: &[u8]) -> Option<MotionResult> {
        let g = self.grey(rgb);
        self.frames += 1;
        if self.frames == 1 {
            self.bg = g;
            return None;
        }
        let (mut n, mut x0, mut y0, mut x1, mut y1) = (0usize, self.w, self.h, 0, 0);
        for y in 0..self.h {
            for x in 0..self.w {
                let i = y * self.w + x;
                if (g[i] - self.bg[i]).abs() > self.threshold {
                    n += 1;
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        let fraction = n as f32 / (self.w * self.h) as f32;
        // Whole-picture changes (lights, IR switching, camera moved): reset.
        if fraction > 0.65 {
            self.bg = g;
            self.streak = 0;
            return Some(MotionResult {
                moving: false,
                fraction,
                bbox: (0.0, 0.0, 1.0, 1.0),
            });
        }
        // Slowly absorb changes into the background.
        let alpha = if self.frames < 10 { 0.5 } else { 0.1 };
        for (b, v) in self.bg.iter_mut().zip(g.iter()) {
            *b += (v - *b) * alpha;
        }
        if self.frames < 10 {
            return None;
        }
        self.streak = if fraction > self.min_fraction {
            self.streak + 1
        } else {
            0
        };
        let bbox = if n > 0 {
            (
                x0 as f32 / self.w as f32,
                y0 as f32 / self.h as f32,
                (x1 - x0 + 1) as f32 / self.w as f32,
                (y1 - y0 + 1) as f32 / self.h as f32,
            )
        } else {
            (0.0, 0.0, 1.0, 1.0)
        };
        // Require two frames in a row to ignore single-frame noise.
        Some(MotionResult {
            moving: self.streak >= 2,
            fraction,
            bbox,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(w: u32, h: u32, square: Option<(u32, u32)>) -> Vec<u8> {
        let mut v = vec![60u8; (w * h * 3) as usize];
        if let Some((sx, sy)) = square {
            for y in sy..sy + 40 {
                for x in sx..sx + 40 {
                    let i = ((y * w + x) * 3) as usize;
                    v[i..i + 3].copy_from_slice(&[250, 250, 250]);
                }
            }
        }
        v
    }

    #[test]
    fn detects_moving_square_not_static_scene() {
        let mut m = MotionDetector::new(320, 180, 50);
        for _ in 0..15 {
            let r = m.feed(&frame(320, 180, None));
            assert!(r.is_none_or(|r| !r.moving));
        }
        let mut moved = false;
        for i in 0..5 {
            let r = m.feed(&frame(320, 180, Some((40 + i * 30, 60)))).unwrap();
            moved |= r.moving;
            if r.moving {
                assert!(r.bbox.0 > 0.1 && r.bbox.0 < 0.9);
            }
        }
        assert!(moved);
    }

    #[test]
    fn ignores_global_light_change() {
        let mut m = MotionDetector::new(320, 180, 50);
        for _ in 0..12 {
            m.feed(&frame(320, 180, None));
        }
        let bright = vec![200u8; 320 * 180 * 3];
        for _ in 0..3 {
            assert!(!m.feed(&bright).unwrap().moving);
        }
    }
}
