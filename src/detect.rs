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
use crate::events::{Event, EventStore, Label, SeenBox};
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
/// How long a frame waits before analysis, so late damage reports still apply.
const ANALYSIS_DELAY: Duration = Duration::from_millis(700);

/// Spots broken frames. Damaged H.264/H.265 frames keep most of their
/// brightness but get wrong colours (green, pink, smeared colour blocks):
/// across most of the picture the colour changes while the brightness
/// hardly does. A real object changes brightness too and covers only part
/// of the picture. Day/night switches are accepted after a while.
#[derive(Default)]
pub struct TintCheck {
    /// Per cell: (brightness, red-green, blue-green), slowly updated.
    cells: Vec<[f32; 3]>,
    odd_frames: u32,
}

const TINT_COLS: usize = 16;
const TINT_ROWS: usize = 9;

impl TintCheck {
    fn measure(f: &Frame) -> Vec<[f32; 3]> {
        let (w, h) = (f.width as usize, f.height as usize);
        let mut out = vec![[0f32; 3]; TINT_COLS * TINT_ROWS];
        let mut n = vec![0u32; TINT_COLS * TINT_ROWS];
        for y in (0..h).step_by(3) {
            let cy = y * TINT_ROWS / h.max(1);
            for x in (0..w).step_by(3) {
                let i = (y * w + x) * 3;
                let (r, g, b) = (f.rgb[i] as f32, f.rgb[i + 1] as f32, f.rgb[i + 2] as f32);
                let c = cy * TINT_COLS + x * TINT_COLS / w.max(1);
                out[c][0] += 0.3 * r + 0.59 * g + 0.11 * b;
                out[c][1] += r - g;
                out[c][2] += b - g;
                n[c] += 1;
            }
        }
        for (o, k) in out.iter_mut().zip(n) {
            let k = k.max(1) as f32;
            o.iter_mut().for_each(|v| *v /= k);
        }
        out
    }

    /// True if this frame looks broken.
    pub fn check(&mut self, f: &Frame) -> bool {
        if f.width < 16 || f.height < 9 {
            return false;
        }
        let now = Self::measure(f);
        if self.cells.len() != now.len() {
            self.cells = now;
            return false;
        }
        let odd = self
            .cells
            .iter()
            .zip(&now)
            .filter(|(b, c)| {
                let dl = (c[0] - b[0]).abs();
                let dc = ((c[1] - b[1]).powi(2) + (c[2] - b[2]).powi(2)).sqrt();
                dc > 22.0 && dc > dl * 1.2
            })
            .count();
        if odd * 100 >= now.len() * 55 {
            self.odd_frames += 1;
            // Still "odd" after ~10 s: the scene really changed (e.g. the
            // camera switched between night and colour mode).
            if self.odd_frames > 50 {
                self.cells = now;
                self.odd_frames = 0;
            }
            return true;
        }
        self.odd_frames = 0;
        for (b, c) in self.cells.iter_mut().zip(&now) {
            for k in 0..3 {
                b[k] = b[k] * 0.95 + c[k] * 0.05;
            }
        }
        false
    }
}

#[cfg(test)]
mod tint_tests {
    use super::*;

    fn frame(f: impl Fn(usize, usize) -> [u8; 3]) -> Frame {
        let (w, h) = (160usize, 90usize);
        let mut rgb = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                rgb.extend(f(x, y));
            }
        }
        Frame {
            width: w as u32,
            height: h as u32,
            rgb: Arc::new(rgb),
        }
    }

    #[test]
    fn broken_colours_but_not_a_red_van() {
        // A grey driveway with a white wall.
        let scene = |x: usize, y: usize| {
            if y < 30 {
                [230, 230, 230]
            } else {
                [110 + (x % 7) as u8, 110, 112]
            }
        };
        let mut t = TintCheck::default();
        for _ in 0..5 {
            assert!(!t.check(&frame(scene)));
        }
        // A red van covering the left 40%: a real object.
        assert!(!t.check(&frame(|x, y| if x < 64 && y > 20 {
            [200, 40, 35]
        } else {
            scene(x, y)
        })));
        // Same brightness, colours wrong everywhere (pink / green): broken.
        assert!(t.check(&frame(|x, y| {
            let [r, g, b] = scene(x, y);
            [
                r.saturating_add(25),
                g.saturating_sub(30),
                b.saturating_add(25),
            ]
        })));
        // The normal picture again is fine.
        assert!(!t.check(&frame(scene)));
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct DetectStatus {
    pub running: bool,
    /// Motion right now.
    pub motion: bool,
    /// Share of the picture that changed, 0..1.
    pub motion_level: f32,
    /// Moving objects at least the minimum size, right now.
    pub objects: Vec<MotionObject>,
    /// Size of the biggest moving area right now (pixels of 640×360).
    pub biggest_px: u32,
    /// Biggest object in the last few seconds (easier to read than the live value).
    pub peak_px: u32,
    /// Objects seen in the event in progress.
    pub active_labels: Vec<Label>,
    pub event_active: bool,
    pub last_event: Option<DateTime<Local>>,
    pub ai_error: Option<String>,
    pub error: Option<String>,
    pub fps: f32,
    /// Parked vehicles being ignored ("car", …).
    pub parked: Vec<String>,
}

pub struct Detector {
    cam: CameraConfig,
    env: Arc<RuntimeEnv>,
    ai: Arc<AiHandle>,
    store: Arc<EventStore>,
    buffer: Option<Arc<EventBuffer>>,
    status: Mutex<DetectStatus>,
    latest: Mutex<Option<Frame>>,
    /// Analysed frames, also used for the live grid (saves a second decode).
    frames: tokio::sync::watch::Sender<Option<Frame>>,
    /// (width, height) of the sub-stream, if known.
    relay: Arc<crate::relay::Relay>,
    /// Follows objects through the camera's zones.
    zones: Mutex<crate::zones::ZoneWatch>,
    /// The latest picture without any activity (for "item left or taken").
    quiet: Mutex<Option<Frame>>,
    /// Parked vehicles (no alerts until they move).
    parked: Mutex<crate::stationary::Stationary>,
}

struct Active {
    event: Event,
    last_activity: DateTime<Local>,
    qualified: bool,
    /// Score of the AI detection in the thumbnail (0 = motion picture, -1 = none).
    thumb_score: f32,
    thumb_at: Instant,
    /// Largest motion seen, for picking the motion thumbnail.
    motion_best: f32,
    /// Recent recognitions (to settle one label per object).
    seen: Vec<(Detection, Instant)>,
    /// "Item left or taken" areas: (zone index, how the area looked before).
    /// Watched areas before the activity: signature and picture.
    zone_before: Vec<(Watched, Vec<f32>, Frame)>,
}

impl Detector {
    pub fn new(
        cam: CameraConfig,
        env: Arc<RuntimeEnv>,
        ai: Arc<AiHandle>,
        store: Arc<EventStore>,
        buffer: Option<Arc<EventBuffer>>,
        relay: Arc<crate::relay::Relay>,
    ) -> Arc<Self> {
        Arc::new(Self {
            cam,
            env,
            ai,
            store: store.clone(),
            buffer,
            status: Mutex::new(DetectStatus::default()),
            latest: Mutex::new(None),
            frames: tokio::sync::watch::Sender::new(None),
            relay,
            zones: Mutex::new(crate::zones::ZoneWatch::default()),
            quiet: Mutex::new(None),
            parked: Mutex::new(crate::stationary::Stationary::open(Some(
                store.dir().join("parked.json"),
            ))),
        })
    }

    pub fn status(&self) -> DetectStatus {
        self.status.lock().unwrap().clone()
    }

    /// Frames as they are analysed (RGB, about 640 px wide).
    pub fn subscribe_frames(&self) -> tokio::sync::watch::Receiver<Option<Frame>> {
        self.frames.subscribe()
    }

    /// The most recent analysed frame.
    pub fn latest_frame(&self) -> Option<Frame> {
        self.latest.lock().unwrap().clone()
    }

    /// Analysis size: 640 px wide, keeping the sub-stream's shape.
    async fn frame_size(&self) -> (u32, u32) {
        // Holding a subscription makes the relay connect.
        let _connect = self.relay.subscribe();
        let info = self.relay.wait_info(Duration::from_secs(30)).await;
        let (sw, sh) = info
            .and_then(|i| Some((i.width?, i.height?)))
            .map(|(w, h)| self.relay.rotated(w, h))
            .unwrap_or((16, 9));
        let w = ANALYSIS_WIDTH.min(sw.max(2)) & !1;
        let h = (((w as f32 * sh as f32 / sw.max(1) as f32).round() as u32).max(2)) & !1;
        (w, h)
    }

    pub async fn run(self: Arc<Self>, cancel: CancellationToken) {
        // Sub-streams are small: the CPU decodes them more cheaply than the
        // GPU (plus copying frames back), unless the user opts in.
        let mut use_gpu = self.env.hardware.hwaccel != "none" && self.env.hardware.gpu_substreams;
        let mut gpu_failed = false;
        let mut failures = 0;
        let mut active: Option<Active> = None;
        loop {
            let started = Instant::now();
            let res = self
                .clone()
                .run_once(&mut use_gpu, gpu_failed, &cancel, &mut active)
                .await;
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
                    gpu_failed = true;
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
        use_gpu_now: &mut bool,
        gpu_failed: bool,
        cancel: &CancellationToken,
        active: &mut Option<Active>,
    ) -> Result<()> {
        let use_gpu = *use_gpu_now;
        // Automatic CPU/GPU choice (when a GPU is set up but sub-streams are
        // not forced onto it): move when one side is overloaded for a while.
        let auto = crate::load::balance()
            && self.env.hardware.hwaccel != "none"
            && !self.env.hardware.gpu_substreams
            && !gpu_failed;
        let mut strain_since: Option<Instant> = None;
        let d = &self.cam.detect;
        let (w, h) = self.frame_size().await;
        let url = self.cam.live_url().to_string();
        let mut cmd = crate::load::cmd(&self.env.ffmpeg_path);
        // Warnings too: damaged-frame reports are often only warnings.
        cmd.args(["-hide_banner", "-loglevel", "warning"]);
        if use_gpu {
            cmd.args(ffmpeg::decode_args(&self.env.hardware, false));
        }
        let mut child = cmd
            .args(self.relay.input_args())
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
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("starting ffmpeg for detection")?;
        let mut stdout = child.stdout.take().context("no stdout")?;
        let stop_feed = CancellationToken::new();
        let _feed = crate::relay::feed(
            self.relay.subscribe(),
            child.stdin.take().context("no stdin")?,
            stop_feed.clone(),
        );
        let _stop_on_exit = stop_feed.drop_guard();
        // When the decoder reports a damaged frame (lost packets), frames
        // are smeared until the next keyframe: don't use them.
        let damaged_at: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
        if let Some(stderr) = child.stderr.take() {
            let name = self.cam.name.clone();
            let damaged = damaged_at.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                let mut filter = ffmpeg::StderrFilter::default();
                while let Ok(Some(line)) = lines.next_line().await {
                    // Lost or corrupt stream data: the next frames are broken.
                    let l = line.to_ascii_lowercase();
                    if l.contains("[mpegts") || l.contains("corrupt") || l.contains("concealing") {
                        *damaged.lock().unwrap() = Some(Instant::now());
                        continue;
                    }
                    if l.contains("deprecated") {
                        continue;
                    }
                    let before = filter.decode_warnings;
                    if let Some(msg) = filter.filter(&line) {
                        warn!("[{name}] detection ffmpeg: {}", redact(&msg, &url));
                    }
                    if filter.decode_warnings > before {
                        *damaged.lock().unwrap() = Some(Instant::now());
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
        let mut motion = MotionDetector::new(w, h, MotionSettings::from(d));
        let mut peak_at = Instant::now();
        let (ai_tx, mut ai_rx) = mpsc::channel::<(Frame, Result<Vec<Detection>, AiError>)>(4);
        let mut ai_busy = false;
        let mut last_ai = Instant::now() - Duration::from_secs(3600);
        let mut fps_count = 0u32;
        let mut fps_since = Instant::now();
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let mut pending: std::collections::VecDeque<(Frame, Instant)> = Default::default();
        let mut tint = TintCheck::default();

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
                    if auto {
                        let cur = if use_gpu { crate::load::Pick::Gpu } else { crate::load::Pick::Cpu };
                        let want = crate::load::pick_decoder(cur);
                        if want == cur {
                            strain_since = None;
                        } else if strain_since.get_or_insert_with(Instant::now).elapsed() > Duration::from_secs(60) {
                            info!(
                                "[{}] detection: moving video decoding to the {} (CPU {}%, GPU {})",
                                self.cam.name,
                                if want == crate::load::Pick::Gpu { "GPU" } else { "CPU" },
                                crate::load::cpu(),
                                crate::load::gpu().map(|g| format!("{g}%")).unwrap_or_else(|| "unknown".into())
                            );
                            *use_gpu_now = want == crate::load::Pick::Gpu;
                            let _ = child.kill().await;
                            return Ok(());
                        }
                    }
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
            // The live grid gets every frame right away.
            *self.latest.lock().unwrap() = Some(frame.clone());
            self.frames.send_replace(Some(frame.clone()));
            // A sudden colour cast (typically green) is a broken frame even
            // when the decoder said nothing.
            if tint.check(&frame) {
                *damaged_at.lock().unwrap() = Some(Instant::now());
            }
            pending.push_back((frame, Instant::now()));
            fps_count += 1;
            if fps_since.elapsed() >= Duration::from_secs(5) {
                self.status.lock().unwrap().fps =
                    fps_count as f32 / fps_since.elapsed().as_secs_f32();
                fps_count = 0;
                fps_since = Instant::now();
            }

            // Frames are analysed a moment after they arrive: the decoder
            // reports damage on a separate channel, sometimes after the
            // broken frame itself has come out.
            while pending
                .front()
                .is_some_and(|(_, at)| at.elapsed() >= ANALYSIS_DELAY)
            {
                let (frame, at) = pending.pop_front().unwrap();
                let damaged = damaged_at
                    .lock()
                    .unwrap()
                    .is_some_and(|t| t + Duration::from_millis(1500) >= at);
                if damaged {
                    // Keep the event alive but don't learn from or save broken
                    // frames; start the motion background afresh afterwards.
                    motion.restart();
                    continue;
                }
                let m = motion.feed(&frame.rgb);
                let moving = d.motion && m.as_ref().is_some_and(|m| m.moving);
                {
                    let mut s = self.status.lock().unwrap();
                    s.motion = moving;
                    s.motion_level = m.as_ref().map(|m| m.fraction).unwrap_or(0.0);
                    s.objects = m.as_ref().map(|m| m.objects.clone()).unwrap_or_default();
                    s.biggest_px = m.as_ref().map(|m| m.biggest).unwrap_or(0);
                    if peak_at.elapsed() > Duration::from_secs(3) {
                        s.peak_px = 0;
                    }
                    if s.biggest_px >= s.peak_px {
                        s.peak_px = s.biggest_px;
                        peak_at = Instant::now();
                    }
                }
                if !moving && active.is_none() {
                    *self.quiet.lock().unwrap() = Some(frame.clone());
                }
                let now = Local::now();
                if moving && let Some(o) = m.as_ref().and_then(|m| m.objects.first()) {
                    // Thumbnail around the biggest moving object.
                    self.on_motion(active, now, &frame, (o.x, o.y, o.w, o.h), o.size as f32);
                }

                // Object recognition only while motion is happening: no motion,
                // nothing is sent to the AI.
                let ai = self.ai.get();
                if d.ai
                    && ai.enabled()
                    && !ai_busy
                    && moving
                    && last_ai.elapsed().as_secs_f64()
                        >= d.ai_interval
                            * if ai.is_local() {
                                crate::load::ai_slowdown()
                            } else {
                                1.0
                            }
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
        // How the "item left or taken" areas looked before the activity.
        let quiet = self.quiet.lock().unwrap().clone();
        let zone_before: Vec<(Watched, Vec<f32>, Frame)> = quiet
            .map(|f| {
                self.watched_areas()
                    .into_iter()
                    .map(|w| {
                        let sig = crate::zones::area_signature(&f, &w.points);
                        let pic = zone_picture(&f, &w.points);
                        (w, sig, pic)
                    })
                    .collect()
            })
            .unwrap_or_default();
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
                focus: None,
                desc: String::new(),
                boxes: Vec::new(),
                summary: String::new(),
                notes: Vec::new(),
                thumb_at: None,
                faces: Vec::new(),
                plates: Vec::new(),
                identified: false,
                items: Vec::new(),
                notified: 0,
                story: String::new(),
                suspicious: String::new(),
            },
            last_activity: now,
            qualified: false,
            thumb_score: -1.0,
            thumb_at: Instant::now() - Duration::from_secs(10),
            motion_best: 0.0,
            seen: Vec::new(),
            zone_before,
        }
    }

    fn on_motion(
        &self,
        active: &mut Option<Active>,
        now: DateTime<Local>,
        frame: &Frame,
        bbox: (f32, f32, f32, f32),
        fraction: f32,
    ) {
        let a = active.get_or_insert_with(|| self.new_event(now));
        a.event.motion = true;
        // With "objects" as the trigger, motion only keeps a pending event open.
        a.last_activity = now;
        // Until the AI recognizes something, the thumbnail is the frame with
        // the most motion so far.
        if a.thumb_score <= 0.0
            && fraction > a.motion_best * 1.2
            && a.thumb_at.elapsed() > Duration::from_millis(800)
        {
            a.motion_best = fraction;
            let (x, y, w, h) = bbox;
            let boxes = vec![SeenBox {
                label: "motion".into(),
                score: 0.0,
                x,
                y,
                w,
                h,
            }];
            self.save_thumb(a, frame, bbox, 0.0, boxes);
        }
        if self.cam.detect.trigger == "motion" && !a.qualified {
            a.qualified = true;
            info!(target: "event", "[{}] motion detected", self.cam.name);
            self.store.save_detected(a.event.clone());
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
        // The same object recognized as two things: keep the more confident
        // one (before choosing which objects alert, so a cat that also
        // looks a bit like a person counts as a cat).
        let mut hits: Vec<Detection> = crate::ai::best_per_object(dets)
            .into_iter()
            .filter(|x| x.score >= d.min_confidence && d.objects.contains(&x.label))
            .collect();
        // Parked vehicles don't alert until they move.
        if d.ignore_parked && !d.parked_objects.is_empty() {
            let (keep, notes) = self.parked.lock().unwrap().filter(
                hits,
                &d.parked_objects,
                Duration::from_secs(d.parked_seconds.max(10) as u64),
            );
            hits = keep;
            let parked = self.parked.lock().unwrap().parked();
            self.status.lock().unwrap().parked = parked;
            for n in notes {
                info!(target: "event", "[{}] {n}", self.cam.name);
                if let Some(a) = active.as_mut()
                    && !a.event.notes.contains(&n)
                {
                    a.event.notes.push(n);
                }
            }
        }
        // ...and across the pictures of this alert.
        if let Some(a) = active.as_mut() {
            a.seen
                .retain(|(_, t)| t.elapsed() < Duration::from_secs(15));
            hits.retain(|h| {
                !a.seen.iter().any(|(s, _)| {
                    s.label != h.label && s.score > h.score && crate::ai::same_object(s, h)
                })
            });
            for h in &hits {
                // A more confident answer for something already labelled
                // differently: that earlier label was wrong.
                let wrong: Vec<String> = a
                    .seen
                    .iter()
                    .filter(|(s, _)| {
                        s.label != h.label && s.score < h.score && crate::ai::same_object(s, h)
                    })
                    .map(|(s, _)| s.label.clone())
                    .collect();
                for w in wrong {
                    let elsewhere = a
                        .seen
                        .iter()
                        .any(|(s, _)| s.label == w && !crate::ai::same_object(s, h));
                    if !elsewhere {
                        a.event.labels.retain(|l| l.label != w);
                    }
                    a.seen
                        .retain(|(s, _)| !(s.label == w && crate::ai::same_object(s, h)));
                }
            }
        }
        // Zones: required areas, crossings, loitering.
        let mut zone_notes = Vec::new();
        if !self.cam.zones.is_empty() {
            let r = self.zones.lock().unwrap().check(&self.cam.zones, hits);
            hits = r.keep;
            zone_notes = r.notes;
        }
        if hits.is_empty() {
            return;
        }
        let now = Local::now();
        let a = active.get_or_insert_with(|| self.new_event(now));
        for n in zone_notes {
            if !a.event.notes.contains(&n) {
                info!(target: "event", "[{}] {n}", self.cam.name);
                a.event.notes.push(n);
            }
        }
        for h in &hits {
            a.seen.push((h.clone(), Instant::now()));
        }
        a.last_activity = now;
        let mut new_labels = Vec::new();
        let mut changed = !a.qualified;
        for h in &hits {
            // The object's colour: from the language model's description if
            // it gave one, otherwise measured from the pixels in its box.
            let color = crate::imgutil::color_word(&h.desc)
                .map(str::to_string)
                .or_else(|| frame.color_name(h.x, h.y, h.w, h.h).map(str::to_string));
            if !h.desc.is_empty()
                && !a.event.desc.to_lowercase().contains(&h.desc.to_lowercase())
                && a.event.desc.len() < 300
            {
                if !a.event.desc.is_empty() {
                    a.event.desc.push_str("; ");
                }
                a.event.desc.push_str(&h.desc);
                changed = true;
            }
            match a.event.labels.iter_mut().find(|l| l.label == h.label) {
                Some(l) => {
                    if h.score > l.score + 0.05 {
                        changed = true;
                    }
                    if h.score > l.score || l.color.is_none() {
                        l.color = color.or(l.color.take());
                    }
                    l.score = l.score.max(h.score);
                }
                None => {
                    a.event.labels.push(Label {
                        label: h.label.clone(),
                        score: h.score,
                        color,
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
        // AI pictures always win over motion pictures; then the most confident one.
        if best.score > a.thumb_score + 0.05
            && (a.thumb_score <= 0.0 || a.thumb_at.elapsed() > Duration::from_secs(1))
        {
            let boxes = hits
                .iter()
                .map(|h| SeenBox {
                    label: h.label.clone(),
                    score: h.score,
                    x: h.x,
                    y: h.y,
                    w: h.w,
                    h: h.h,
                })
                .collect();
            self.save_thumb(
                a,
                &frame,
                (best.x, best.y, best.w, best.h),
                best.score,
                boxes,
            );
            changed = true;
        }
        if !new_labels.is_empty() {
            info!(target: "event", "[{}] alert: {}", self.cam.name, new_labels.join(", "));
        }
        a.qualified = true;
        // Only write when something changed (the final state is saved when the event ends).
        if changed {
            self.store.save_detected(a.event.clone());
        }
        self.mark_active(a);
    }

    fn save_thumb(
        &self,
        a: &mut Active,
        frame: &Frame,
        (x, y, w, h): (f32, f32, f32, f32),
        score: f32,
        boxes: Vec<SeenBox>,
    ) {
        // The whole picture; for a recognized object the page zooms into
        // `focus` (the card keeps its size).
        let crop = frame.fit(640);
        let name = format!("{}.jpg", a.event.id);
        let dir = self.store.dir();
        let _ = std::fs::create_dir_all(dir);
        // Write then rename, so a browser never reads a half-written picture.
        let tmp = dir.join(format!("{name}.tmp"));
        if let Ok(j) = crop.jpeg(85)
            && std::fs::write(&tmp, j).is_ok()
            && std::fs::rename(&tmp, dir.join(&name)).is_ok()
        {
            a.event.thumb = Some(name);
            a.event.focus = (score > 0.0).then_some([x, y, w, h]);
            // The picture is from a moment ago (analysis delay, AI time).
            a.event.thumb_at = Some(Local::now() - CDuration::milliseconds(1200));
            a.event.boxes = boxes;
            a.thumb_score = score;
            a.thumb_at = Instant::now();
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
        // Item left or taken: the area looks different now that it's quiet.
        if !a.zone_before.is_empty()
            && let Some(now) = self.latest.lock().unwrap().clone()
        {
            let person = a.event.labels.iter().any(|l| l.label == "person");
            for (zi, (z, before, before_pic)) in a.zone_before.iter().enumerate() {
                let change = crate::zones::area_change(
                    before,
                    &crate::zones::area_signature(&now, &z.points),
                );
                if change > 0.12 {
                    // Keep both pictures: smart search tells a delivered
                    // parcel from one picked up (or a moved bin).
                    let dir = self.store.dir();
                    let (bn, an) = (
                        format!("{}-z{zi}-before.jpg", a.event.id),
                        format!("{}-z{zi}-after.jpg", a.event.id),
                    );
                    if let (Ok(b), Ok(af)) =
                        (before_pic.jpeg(85), zone_picture(&now, &z.points).jpeg(85))
                        && std::fs::write(dir.join(&bn), b).is_ok()
                        && std::fs::write(dir.join(&an), af).is_ok()
                    {
                        a.event.items.push(crate::events::ItemChange {
                            zone: z.name.clone(),
                            before: bn,
                            after: an,
                            checked: false,
                        });
                    }
                    let note = if person {
                        format!("Item left or taken in {} (possible delivery)", z.name)
                    } else {
                        format!("Item left or taken in {}", z.name)
                    };
                    info!(target: "event", "[{}] {note} ({:.0}% of the area changed)", self.cam.name, change * 100.0);
                    a.event.notes.push(note);
                    if !a.event.labels.iter().any(|l| l.label == "package") {
                        a.event.labels.push(Label {
                            label: "package".into(),
                            score: 1.0,
                            color: None,
                        });
                    }
                }
            }
        }
        self.store.save_detected(a.event.clone());
        if let Some(b) = &self.buffer {
            b.finish(
                &a.event.id,
                a.event.start - CDuration::seconds(d.pre_seconds as i64),
                a.event.end + CDuration::seconds(d.post_seconds as i64),
            );
        }
    }
}

/// An area watched for items left or taken.
struct Watched {
    name: String,
    points: Vec<[f32; 2]>,
}

impl Detector {
    /// The package spot (camera → AI) and zones set to watch for items.
    fn watched_areas(&self) -> Vec<Watched> {
        let d = &self.cam.detect;
        let mut v: Vec<Watched> = Vec::new();
        if d.packages && d.package_area.len() >= 3 {
            v.push(Watched {
                name: "the package spot".into(),
                points: d.package_area.clone(),
            });
        }
        v.extend(
            self.cam
                .zones
                .iter()
                .filter(|z| z.kind == "area" && z.watch_items && z.points.len() >= 3)
                .map(|z| Watched {
                    name: z.name.clone(),
                    points: z.points.clone(),
                }),
        );
        v
    }
}

/// The part of the picture around a zone (with a margin), for checking
/// what's there.
fn zone_picture(f: &Frame, poly: &[[f32; 2]]) -> Frame {
    let (mut x0, mut y0, mut x1, mut y1) = (1f32, 1f32, 0f32, 0f32);
    for p in poly {
        x0 = x0.min(p[0]);
        y0 = y0.min(p[1]);
        x1 = x1.max(p[0]);
        y1 = y1.max(p[1]);
    }
    let (fw, fh) = (f.width as f32, f.height as f32);
    let (mx, my) = ((x1 - x0) * 0.1, (y1 - y0) * 0.1);
    crate::recog::crop(
        f,
        (x0 - mx) * fw,
        (y0 - my) * fh,
        (x1 - x0 + 2.0 * mx) * fw,
        (y1 - y0 + 2.0 * my) * fh,
    )
    .fit(640)
}

/// A moving object found in a frame.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MotionObject {
    /// Box, normalized 0..1.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Size in pixels of a 640×360 picture (like Blue Iris' object size).
    pub size: u32,
}

pub struct MotionResult {
    /// Motion that passes size, make time and travel: triggers alerts.
    pub moving: bool,
    /// Objects at least the minimum size, biggest first.
    pub objects: Vec<MotionObject>,
    /// Size of the biggest changed area (even below the minimum).
    pub biggest: u32,
    /// Share of the picture that changed, 0..1.
    pub fraction: f32,
}

#[derive(Clone)]
pub struct MotionSettings {
    pub min_contrast: f32,
    pub min_object_px: u32,
    pub make_time: Duration,
    pub min_travel_px: f32,
    /// MASK_W × MASK_H, true = ignore.
    pub mask: Option<Vec<bool>>,
}

impl MotionSettings {
    pub fn from(d: &crate::config::DetectConfig) -> Self {
        let mask = (d.mask.len() == crate::config::MASK_W * crate::config::MASK_H)
            .then(|| d.mask.chars().map(|c| c == '1').collect::<Vec<bool>>())
            .filter(|m| m.iter().any(|x| *x));
        Self {
            min_contrast: d.min_contrast as f32,
            min_object_px: d.min_object_px,
            make_time: Duration::from_millis(d.make_time_ms as u64),
            min_travel_px: d.min_travel_px as f32,
            mask,
        }
    }
}

/// Blue Iris-style motion detection on a 4× downscaled grey image:
/// pixels that differ from a slowly updated background by more than the
/// minimum contrast are grouped into connected objects; objects smaller than
/// the minimum size are ignored; motion must last the make time (and,
/// optionally, the object must travel) before it triggers.
pub struct MotionDetector {
    w: usize,
    h: usize,
    src_w: usize,
    bg: Vec<f32>,
    frames: u32,
    s: MotionSettings,
    /// Pixels of a 640-wide picture per grid cell.
    cell_px: f32,
    ignore: Vec<bool>,
    since: Option<Instant>,
    start: (f32, f32),
    gap: u32,
    triggered: bool,
}

impl MotionDetector {
    pub fn new(src_w: u32, src_h: u32, s: MotionSettings) -> Self {
        let (w, h) = ((src_w / 4).max(1) as usize, (src_h / 4).max(1) as usize);
        let ignore = match &s.mask {
            Some(m) => (0..w * h)
                .map(|i| {
                    let (x, y) = (i % w, i / w);
                    let mx = x * crate::config::MASK_W / w;
                    let my = y * crate::config::MASK_H / h;
                    m[my * crate::config::MASK_W + mx]
                })
                .collect(),
            None => vec![false; w * h],
        };
        let scale = 640.0 / w as f32;
        Self {
            w,
            h,
            src_w: src_w as usize,
            bg: vec![0.0; w * h],
            frames: 0,
            s,
            cell_px: scale * scale,
            ignore,
            since: None,
            start: (0.0, 0.0),
            gap: 0,
            triggered: false,
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

    /// Forget the background (after broken frames).
    pub fn restart(&mut self) {
        self.frames = 0;
        self.reset();
    }

    pub fn feed(&mut self, rgb: &[u8]) -> Option<MotionResult> {
        self.feed_at(rgb, Instant::now())
    }

    pub fn feed_at(&mut self, rgb: &[u8], now: Instant) -> Option<MotionResult> {
        let g = self.grey(rgb);
        self.frames += 1;
        if self.frames == 1 {
            self.bg = g;
            return None;
        }
        let (w, h) = (self.w, self.h);
        let mut changed = vec![false; w * h];
        let mut n = 0usize;
        for i in 0..w * h {
            if !self.ignore[i] && (g[i] - self.bg[i]).abs() > self.s.min_contrast {
                changed[i] = true;
                n += 1;
            }
        }
        let fraction = n as f32 / (w * h) as f32;
        // Whole-picture changes (lights, IR switching, camera moved): relearn.
        if fraction > 0.65 {
            self.bg = g;
            self.reset();
            return Some(MotionResult {
                moving: false,
                objects: vec![],
                biggest: 0,
                fraction,
            });
        }
        let alpha = if self.frames < 10 { 0.5 } else { 0.1 };
        for (b, v) in self.bg.iter_mut().zip(g.iter()) {
            *b += (v - *b) * alpha;
        }
        if self.frames < 10 {
            return None;
        }

        let mut objects = self.objects(&changed);
        objects.sort_by(|a, b| b.size.cmp(&a.size));
        let biggest = objects.first().map(|o| o.size).unwrap_or(0);
        objects.retain(|o| o.size >= self.s.min_object_px.max(1));

        // Make time and travel.
        if let Some(o) = objects.first() {
            let c = (o.x + o.w / 2.0, o.y + o.h / 2.0);
            self.gap = 0;
            if self.since.is_none() {
                self.since = Some(now);
                self.start = c;
            }
            let lasted = now.duration_since(self.since.unwrap()) >= self.s.make_time;
            let (dx, dy) = (
                (c.0 - self.start.0) * 640.0,
                (c.1 - self.start.1) * 640.0 * h as f32 / w as f32,
            );
            let travelled =
                self.s.min_travel_px <= 0.0 || (dx * dx + dy * dy).sqrt() >= self.s.min_travel_px;
            if lasted && travelled {
                self.triggered = true;
            }
        } else {
            // Allow a couple of frames without objects (e.g. passing behind a post).
            self.gap += 1;
            if self.gap > 2 {
                self.reset();
            }
        }
        Some(MotionResult {
            moving: self.triggered && !objects.is_empty(),
            objects,
            biggest,
            fraction,
        })
    }

    fn reset(&mut self) {
        self.since = None;
        self.triggered = false;
        self.gap = 0;
    }

    /// Connected groups of changed cells. Cells are joined across one-cell
    /// gaps so a person split by a fence still counts as one object.
    fn objects(&self, changed: &[bool]) -> Vec<MotionObject> {
        let (w, h) = (self.w, self.h);
        let mut grown = vec![false; w * h];
        for y in 0..h {
            for x in 0..w {
                if changed[y * w + x] {
                    for yy in y.saturating_sub(1)..(y + 2).min(h) {
                        for xx in x.saturating_sub(1)..(x + 2).min(w) {
                            grown[yy * w + xx] = true;
                        }
                    }
                }
            }
        }
        let mut seen = vec![false; w * h];
        let mut out = Vec::new();
        let mut stack = Vec::new();
        for start in 0..w * h {
            if !grown[start] || seen[start] {
                continue;
            }
            let (mut x0, mut y0, mut x1, mut y1, mut count) = (w, h, 0, 0, 0u32);
            seen[start] = true;
            stack.push(start);
            while let Some(i) = stack.pop() {
                let (x, y) = (i % w, i / w);
                if changed[i] {
                    count += 1;
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
                let mut push = |j: usize| {
                    if grown[j] && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                };
                if x > 0 {
                    push(i - 1);
                }
                if x + 1 < w {
                    push(i + 1);
                }
                if y > 0 {
                    push(i - w);
                }
                if y + 1 < h {
                    push(i + w);
                }
            }
            if count > 0 {
                out.push(MotionObject {
                    x: x0 as f32 / w as f32,
                    y: y0 as f32 / h as f32,
                    w: (x1 - x0 + 1) as f32 / w as f32,
                    h: (y1 - y0 + 1) as f32 / h as f32,
                    size: (count as f32 * self.cell_px) as u32,
                });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(w: u32, h: u32, square: Option<(u32, u32, u32)>) -> Vec<u8> {
        let mut v = vec![60u8; (w * h * 3) as usize];
        if let Some((sx, sy, size)) = square {
            for y in sy..sy + size {
                for x in sx..sx + size {
                    let i = ((y * w + x) * 3) as usize;
                    v[i..i + 3].copy_from_slice(&[250, 250, 250]);
                }
            }
        }
        v
    }

    fn settings(min_object_px: u32, make_ms: u64, travel: f32) -> MotionSettings {
        MotionSettings {
            min_contrast: 24.0,
            min_object_px,
            make_time: Duration::from_millis(make_ms),
            min_travel_px: travel,
            mask: None,
        }
    }

    #[test]
    fn measures_object_size_and_ignores_small_ones() {
        // 640×360 picture: a 40×40 square is 1600 px.
        let mut m = MotionDetector::new(640, 360, settings(1000, 0, 0.0));
        let t = Instant::now();
        for i in 0..12 {
            m.feed_at(&frame(640, 360, None), t + Duration::from_millis(i * 200));
        }
        let r = m
            .feed_at(
                &frame(640, 360, Some((100, 100, 40))),
                t + Duration::from_secs(3),
            )
            .unwrap();
        assert!(r.moving);
        assert_eq!(r.objects.len(), 1);
        assert!(
            (1400..=1800).contains(&r.objects[0].size),
            "size {}",
            r.objects[0].size
        );
        assert!((r.objects[0].x - 100.0 / 640.0).abs() < 0.02);
        // An 8×8 speck (64 px) is reported as the biggest change but is not an object.
        let mut m = MotionDetector::new(640, 360, settings(1000, 0, 0.0));
        for i in 0..12 {
            m.feed_at(&frame(640, 360, None), t + Duration::from_millis(i * 200));
        }
        let r = m
            .feed_at(
                &frame(640, 360, Some((300, 200, 8))),
                t + Duration::from_secs(3),
            )
            .unwrap();
        assert!(!r.moving && r.objects.is_empty() && r.biggest > 0);
    }

    #[test]
    fn make_time_and_travel() {
        let mut m = MotionDetector::new(640, 360, settings(500, 1000, 100.0));
        let t = Instant::now();
        for i in 0..12 {
            m.feed_at(&frame(640, 360, None), t + Duration::from_millis(i * 200));
        }
        let t = t + Duration::from_secs(3);
        // Object appears and moves 30 px per 200 ms.
        let mut first_trigger = None;
        for i in 0..12u32 {
            let r = m
                .feed_at(
                    &frame(640, 360, Some((50 + i * 30, 100, 40))),
                    t + Duration::from_millis(i as u64 * 200),
                )
                .unwrap();
            if r.moving && first_trigger.is_none() {
                first_trigger = Some(i);
            }
        }
        // Needs ≥1 s (frame 5) and ≥100 px travel (frame 4): first at frame 5.
        assert_eq!(first_trigger, Some(5));
    }

    #[test]
    fn mask_and_light_changes() {
        let mut mask = vec![false; crate::config::MASK_W * crate::config::MASK_H];
        for y in 0..crate::config::MASK_H {
            for x in 0..crate::config::MASK_W / 2 {
                mask[y * crate::config::MASK_W + x] = true; // ignore the left half
            }
        }
        let mut s = settings(500, 0, 0.0);
        s.mask = Some(mask);
        let mut m = MotionDetector::new(640, 360, s);
        let t = Instant::now();
        for i in 0..12 {
            m.feed_at(&frame(640, 360, None), t + Duration::from_millis(i * 200));
        }
        assert!(
            !m.feed_at(
                &frame(640, 360, Some((100, 100, 60))),
                t + Duration::from_secs(3)
            )
            .unwrap()
            .moving
        );
        assert!(
            m.feed_at(
                &frame(640, 360, Some((450, 100, 60))),
                t + Duration::from_secs(4)
            )
            .unwrap()
            .moving
        );
        // Whole picture brightens (lights on): no motion.
        let mut m = MotionDetector::new(640, 360, settings(500, 0, 0.0));
        for i in 0..12 {
            m.feed_at(&frame(640, 360, None), t + Duration::from_millis(i * 200));
        }
        let bright = vec![200u8; 640 * 360 * 3];
        assert!(
            !m.feed_at(&bright, t + Duration::from_secs(3))
                .unwrap()
                .moving
        );
    }
}
