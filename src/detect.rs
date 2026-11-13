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
    /// What the AI found in its latest look (for PTZ follow), and when.
    #[serde(skip)]
    pub seen_now: Vec<SeenBox>,
    #[serde(skip)]
    pub seen_at: Option<Instant>,
    pub ai_error: Option<String>,
    pub error: Option<String>,
    pub fps: f32,
    /// Parked vehicles being ignored ("car", …).
    pub parked: Vec<String>,
    /// Decoding the camera's pictures on the GPU right now.
    pub gpu_decoding: bool,
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
    quiet: Mutex<Option<(Frame, Instant)>>,
    /// Parked vehicles (no alerts until they move).
    parked: Mutex<crate::stationary::Stationary>,
    /// The full-quality stream, for checking what the AI found (none when
    /// the camera has no separate sub-stream: it's already the same).
    relay_main: Option<Arc<crate::relay::Relay>>,
    /// Things the full-quality check didn't confirm: (label, box, when).
    /// The same thing at the same spot isn't alerted on again for a while.
    rejected: Mutex<Vec<(String, [f32; 4], Instant)>>,
    /// What the camera itself reported (ONVIF events): (label, note), to
    /// start or extend an alert on the next analysed frame.
    external: Mutex<Vec<(String, String)>>,
}

/// What the full-quality picture showed for an object the AI found.
#[derive(Debug, Clone, PartialEq)]
enum HdVerdict {
    /// The same thing (its score in full quality).
    Confirmed(f32),
    /// Something else that's watched for.
    Relabel(String, f32),
    /// Nothing there.
    NotSeen,
    /// Couldn't be checked (no picture from the camera).
    Failed,
}

/// The results of one full-quality check: event, then (label, box, verdict).
struct HdResult {
    event: String,
    checks: Vec<(String, [f32; 4], HdVerdict)>,
}

/// Is `d`'s middle within box `b` (grown by half its size each way: the
/// full-quality picture is from a moment later)?
fn same_spot(d: &Detection, b: [f32; 4]) -> bool {
    let (cx, cy) = (d.x + d.w / 2.0, d.y + d.h / 2.0);
    cx >= b[0] - b[2] * 0.5 && cx <= b[0] + b[2] * 1.5 && cy >= b[1] - b[3] * 0.5 && cy <= b[1] + b[3] * 1.5
}

/// One object checked in a full-quality picture: a close-up around where
/// the sub-stream saw it (the AI looks at a small picture: only a close-up
/// gains from the detail).
async fn hd_verify(ai: &crate::ai::Ai, f: &Frame, label: &str, b: [f32; 4], min_conf: f32, watched: &[String]) -> HdVerdict {
    let (fw, fh) = (f.width as f32, f.height as f32);
    let (cx, cy) = ((b[0] + b[2] / 2.0) * fw, (b[1] + b[3] / 2.0) * fh);
    let cw = (b[2] * fw * 2.2).max(320.0).min(fw);
    let ch = (b[3] * fh * 2.2).max(320.0).min(fh);
    let (x0, y0) = ((cx - cw / 2.0).clamp(0.0, fw - cw), (cy - ch / 2.0).clamp(0.0, fh - ch));
    let crop = crate::recog::crop(f, x0, y0, cw, ch);
    let Ok(dets) = ai.detect(&crop).await else { return HdVerdict::Failed };
    let (sw, sh) = (crop.width as f32, crop.height as f32);
    let found: Vec<Detection> = dets
        .into_iter()
        .map(|d| Detection { x: (x0 + d.x * sw) / fw, y: (y0 + d.y * sh) / fh, w: d.w * sw / fw, h: d.h * sh / fh, ..d })
        .filter(|d| d.score >= min_conf * 0.85 && same_spot(d, b))
        .collect();
    const VEHICLES: [&str; 5] = ["vehicle", "car", "truck", "bus", "motorcycle"];
    let alike = |l: &str| l == label || (VEHICLES.contains(&l) && VEHICLES.contains(&label));
    let best = |pick: &dyn Fn(&Detection) -> bool| found.iter().filter(|d| pick(d)).max_by(|a, b| a.score.total_cmp(&b.score)).cloned();
    if let Some(d) = best(&|d| alike(&d.label)) {
        return HdVerdict::Confirmed(d.score);
    }
    match best(&|d| watched.contains(&d.label)) {
        Some(d) => HdVerdict::Relabel(d.label, d.score),
        None => HdVerdict::NotSeen,
    }
}

/// For the test in Settings: a full-quality picture now, and each find
/// checked in it as during an alert ("confirmed", "seen as …", "not seen").
pub async fn hd_test(
    ai: &crate::ai::Ai,
    cam: &crate::config::CameraConfig,
    relay: &crate::relay::Relay,
    ffmpeg: &str,
    finds: &[Detection],
) -> anyhow::Result<(Frame, Vec<(Detection, String)>)> {
    let mut f = hd_frame(relay, ffmpeg).await?;
    if !cam.privacy.is_empty() {
        let mut px = (*f.rgb).clone();
        cam.paint_privacy(&mut px, f.width, f.height);
        f.rgb = Arc::new(px);
    }
    let d = &cam.detect;
    let mut out = Vec::new();
    for x in finds.iter().filter(|x| x.score >= d.min_confidence) {
        let v = match hd_verify(ai, &f, &x.label, [x.x, x.y, x.w, x.h], d.min_confidence, &d.objects).await {
            HdVerdict::Confirmed(s) => format!("✓ {} confirmed {:.0}%", x.label, s * 100.0),
            HdVerdict::Relabel(l, s) => format!("↻ really a {l} {:.0}%", s * 100.0),
            HdVerdict::NotSeen => format!("✗ {} not seen in HD", x.label),
            HdVerdict::Failed => "couldn't check".to_string(),
        };
        out.push((x.clone(), v));
    }
    Ok((f, out))
}

/// What set off an alert that has nothing to see (a sound), to write on
/// its picture: "Heard: smoke or fire alarm".
pub fn caption_for(e: &Event) -> Option<String> {
    if !e.labels.iter().any(|l| l.label == "sound") {
        return None;
    }
    let heard: Vec<&str> = e.notes.iter().filter_map(|n| n.strip_prefix("Heard: ")).collect();
    (!heard.is_empty()).then(|| format!("Heard: {}", heard.join(" - ")))
}

/// `f` with `text` written along the bottom, big enough to read on a
/// thumbnail.
pub fn captioned(f: &Frame, text: &str) -> Frame {
    let mut px = (*f.rgb).clone();
    let scale = ((f.width / 200) as i32).clamp(2, 6);
    let th = 11 * scale;
    crate::draw::caption(&mut px, f.width, f.height, 4 * scale, f.height as i32 - th - 4 * scale, text, scale);
    Frame { width: f.width, height: f.height, rgb: Arc::new(px) }
}

/// A full-quality picture of this moment (at most 1920 px wide).
async fn hd_frame(relay: &crate::relay::Relay, ffmpeg: &str) -> anyhow::Result<Frame> {
    let _connect = relay.subscribe();
    let info = relay.wait_info(Duration::from_secs(10)).await;
    let (sw, sh) = info
        .and_then(|i| Some((i.width?, i.height?)))
        .map(|(w, h)| relay.rotated(w, h))
        .ok_or_else(|| anyhow::anyhow!("main stream size not known"))?;
    let w = sw.min(1920).max(2) & !1;
    let h = ((w as f32 * sh as f32 / sw.max(1) as f32).round() as u32).max(2) & !1;
    let rgb = crate::relay::grab_rgb_live(relay, ffmpeg, w, h).await?;
    Ok(Frame { width: w, height: h, rgb: Arc::new(rgb) })
}

struct Active {
    event: Event,
    last_activity: DateTime<Local>,
    qualified: bool,
    /// When the motion of this event began, and how many times the AI has
    /// looked since (for "ignore animals": motion only alerts once the AI
    /// has had a look).
    began: Instant,
    ai_looks: u32,
    /// The AI saw an animal that isn't watched for (a cat, a dog …) at some
    /// point during this event.
    animal_seen: bool,
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
    /// For each of those areas: did the activity (something moving, or an
    /// object the AI found) reach it? Only those are checked afterwards.
    touched: Vec<bool>,
    /// An area looked different once things went quiet: the alert is kept
    /// open a minute (since when, and how each area looked then) to see the
    /// change stays — a parcel left, not a shadow or something set down
    /// for a moment.
    item_hold: Option<(Instant, Vec<Option<Vec<f32>>>)>,
}

/// How long a change in a watched area must stay before it counts as an
/// item left or taken.
const ITEM_HOLD: Duration = Duration::from_secs(60);

impl Active {
    /// Something was seen at this box (normalized): mark the watched areas
    /// it reaches (with a small margin, for an arm putting a parcel down).
    fn touch(&mut self, x: f32, y: f32, w: f32, h: f32) {
        for (i, (z, ..)) in self.zone_before.iter().enumerate() {
            if !self.touched[i] && z.reaches(x, y, w, h) {
                self.touched[i] = true;
            }
        }
    }
}

impl Detector {
    pub fn new(
        cam: CameraConfig,
        env: Arc<RuntimeEnv>,
        ai: Arc<AiHandle>,
        store: Arc<EventStore>,
        buffer: Option<Arc<EventBuffer>>,
        relay: Arc<crate::relay::Relay>,
        relay_main: Option<Arc<crate::relay::Relay>>,
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
            relay_main,
            rejected: Mutex::new(Vec::new()),
            external: Mutex::new(Vec::new()),
        })
    }

    /// The camera itself detected something (its own motion, person or
    /// vehicle detection, a doorbell press): an alert, as if GAZE had seen it.
    pub fn external(&self, label: &str, note: &str) {
        let mut q = self.external.lock().unwrap();
        if q.len() < 20 {
            q.push((label.to_string(), note.to_string()));
        }
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
        self.status.lock().unwrap().gpu_decoding = use_gpu;
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
        let (hd_tx, mut hd_rx) = mpsc::channel::<HdResult>(8);
        let mut ai_busy = false;
        let mut last_ai = Instant::now() - Duration::from_secs(3600);
        let mut fps_count = 0u32;
        let mut fps_since = Instant::now();
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let mut pending: std::collections::VecDeque<(Frame, Instant)> = Default::default();
        let mut tint = TintCheck::default();
        let (mut last_pic, mut last_prune): (Option<i64>, Instant) = (None, Instant::now() - Duration::from_secs(3600));

        loop {
            let mut buf = vec![0u8; frame_len];
            tokio::select! {
                r = tokio::time::timeout(Duration::from_secs(20), stdout.read_exact(&mut buf)) => {
                    r.context("no video for 20s")?.context("detection stream ended")?;
                }
                Some((frame, res)) = ai_rx.recv() => {
                    ai_busy = false;
                    self.handle_ai(frame, res, active, &hd_tx);
                    continue;
                }
                Some(r) = hd_rx.recv() => {
                    self.apply_hd(r, active);
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
            // Privacy areas: blacked out before anything sees the frame (the
            // grid, motion, the AI, alert pictures, the time-lapse).
            if !self.cam.privacy.is_empty() {
                self.cam.paint_privacy(&mut buf, w, h);
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
                // The time-lapse: one picture per interval, as it's recorded
                // (see picturelog).
                let shot = Local::now() - CDuration::from_std(at.elapsed()).unwrap_or_default();
                let step = crate::picturelog::seconds() as i64 * 1000;
                let slot = shot.timestamp_millis() / step;
                if last_pic != Some(slot) {
                    last_pic = Some(slot);
                    if let Some(cam_dir) = self.store.dir().parent().map(|p| p.to_path_buf()) {
                        let f = frame.clone();
                        let prune = last_prune.elapsed() > Duration::from_secs(600);
                        if prune {
                            last_prune = Instant::now();
                        }
                        tokio::task::spawn_blocking(move || {
                            let _ = crate::picturelog::save(&cam_dir, shot, &f);
                            if prune {
                                crate::picturelog::prune(&cam_dir);
                            }
                        });
                    }
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
                    *self.quiet.lock().unwrap() = Some((frame.clone(), Instant::now()));
                }
                let now = Local::now();
                if moving && let Some(o) = m.as_ref().and_then(|m| m.objects.first()) {
                    // Thumbnail around the biggest moving object.
                    self.on_motion(active, now, &frame, (o.x, o.y, o.w, o.h), o.size as f32);
                    // Which package / item areas the movement reached.
                    if let (Some(a), Some(m)) = (active.as_mut(), m.as_ref()) {
                        for o in &m.objects {
                            a.touch(o.x, o.y, o.w, o.h);
                        }
                    }
                }

                // What the camera itself reported.
                let ext: Vec<(String, String)> = std::mem::take(&mut *self.external.lock().unwrap());
                for (label, note) in ext {
                    // (Only the camera's own motion detection counts as
                    // motion; a sound or a doorbell press doesn't.)
                    if label == "motion" && (active.is_none() || active.as_ref().is_some_and(|a| a.thumb_score < 0.0)) {
                        self.on_motion(active, now, &frame, (0.0, 0.0, 1.0, 1.0), 0.0);
                    }
                    let a = active.get_or_insert_with(|| self.new_event(now));
                    a.last_activity = now;
                    if label != "motion" && !a.event.labels.iter().any(|l| l.label == label) {
                        a.event.labels.push(Label { label: label.clone(), score: 1.0, color: None, hd: None });
                    }
                    if !note.is_empty() && !a.event.notes.contains(&note) {
                        a.event.notes.push(note.clone());
                    }
                    // A picture of the moment, saying what set it off: for a
                    // sound (replacing a plain motion picture; something the
                    // AI recognized stays the picture), or anything else
                    // the camera reported when there's no picture yet.
                    if a.thumb_score < 0.0 || (label == "sound" && a.thumb_score < 0.01) {
                        let text = caption_for(&a.event).unwrap_or_else(|| if note.is_empty() { label.clone() } else { note.clone() });
                        let pic = captioned(&frame.fit(1280), &text);
                        self.save_thumb(a, &pic, (0.0, 0.0, 1.0, 1.0), 0.0, vec![]);
                        a.thumb_score = 0.01;
                    }
                    if !a.qualified {
                        a.qualified = true;
                        info!(target: "event", "[{}] {}", self.cam.name, if note.is_empty() { "the camera detected motion" } else { &note });
                    }
                    self.store.save_detected(a.event.clone());
                    self.mark_active(a);
                }

                // Object recognition only while motion is happening: no motion,
                // nothing is sent to the AI.
                let ai = self.ai.get();
                if d.ai
                    && ai.enabled()
                    && !ai_busy
                    && moving
                    && last_ai.elapsed().as_secs_f64() >= d.ai_interval * crate::load::ai_slowdown()
                {
                    ai_busy = true;
                    last_ai = Instant::now();
                    let tx = ai_tx.clone();
                    let f = frame.clone();
                    // Where the motion is, for a closer look if the whole
                    // picture shows nothing (small or far-away objects).
                    let near = m
                        .as_ref()
                        .and_then(|m| m.objects.first())
                        .map(|o| (o.x, o.y, o.w, o.h));
                    let (want, min_conf) = (d.objects.clone(), d.min_confidence);
                    tokio::spawn(async move {
                        let mut res = ai.detect(&f).await;
                        if let Ok(dets) = &mut res
                            && !dets
                                .iter()
                                .any(|x| x.score >= min_conf && want.contains(&x.label))
                            && let Some((crop, [ox, oy, sw, sh])) =
                                near.and_then(|n| zoom_on(&f, n))
                            && let Ok(more) = ai.detect(&crop).await
                        {
                            dets.extend(more.into_iter().map(|x| Detection {
                                x: ox + x.x * sw,
                                y: oy + x.y * sh,
                                w: x.w * sw,
                                h: x.h * sh,
                                ..x
                            }));
                        }
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
        // Only a recent quiet picture is a fair "before" (one from long ago
        // differs anyway: light, shadows, day and night).
        let quiet = self
            .quiet
            .lock()
            .unwrap()
            .clone()
            .filter(|(_, at)| at.elapsed() < Duration::from_secs(120))
            .map(|(f, _)| f);
        let zone_before: Vec<(Watched, Vec<f32>, Frame)> = quiet
            .map(|f| {
                self.watched_areas()
                    .into_iter()
                    .map(|w| {
                        let sig = w.signature(&f);
                        let pic = w.picture(&f);
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
                boxes: Vec::new(),
                notes: Vec::new(),
                thumb_at: None,
                faces: Vec::new(),
                plates: Vec::new(),
                identified: false,
                items: Vec::new(),
                notified: 0,
                suspicious: String::new(),
                hd_checking: false,
                meaning: None,
            },
            last_activity: now,
            qualified: false,
            began: Instant::now(),
            ai_looks: 0,
            animal_seen: false,
            thumb_score: -1.0,
            thumb_at: Instant::now() - Duration::from_secs(10),
            motion_best: 0.0,
            seen: Vec::new(),
            touched: vec![false; zone_before.len()],
            zone_before,
            item_hold: None,
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
        // "Check motion with AI first": the AI looks at the frames of the
        // motion (at least twice, or up to 4 s in case it's busy) before it
        // becomes an alert; motion with an animal that isn't watched for
        // never does (and is dropped when it ends, see finish()).
        let d = &self.cam.detect;
        let wait_for_ai = d.ignore_animals
            && d.ai
            && self.ai.get().enabled()
            && (a.animal_seen || (a.ai_looks < 2 && a.began.elapsed() < Duration::from_secs(4)));
        if d.trigger == "motion" && !a.qualified && !wait_for_ai {
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
        hd_tx: &mpsc::Sender<HdResult>,
    ) {
        let d = &self.cam.detect;
        let dets = match res {
            Ok(d) => {
                self.status.lock().unwrap().ai_error = None;
                d
            }
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
        let mut best = crate::ai::best_per_object(dets);
        // The areas drawn to ignore count for the AI too (not only for
        // motion): whatever started this look, something mostly inside
        // them is left out.
        if d.ai_ignores_mask
            && let Some(mask) = ignore_mask(d)
        {
            best.retain(|x| {
                let keep = masked_share(&mask, x) <= 0.5;
                if !keep {
                    tracing::debug!("[{}] {} in an ignored area: left out", self.cam.name, x.label);
                }
                keep
            });
        }
        let mut hits: Vec<Detection> = best
            .iter()
            .filter(|x| x.score >= d.min_confidence && d.objects.contains(&x.label))
            .cloned()
            .collect();
        {
            let mut s = self.status.lock().unwrap();
            s.seen_now = hits.iter().map(|x| SeenBox { label: x.label.clone(), score: x.score, x: x.x, y: x.y, w: x.w, h: x.h }).collect();
            s.seen_at = Some(Instant::now());
        }
        // For "ignore animals": what this look found.
        if let Some(a) = active.as_mut() {
            for x in best.iter().filter(|x| x.score >= 0.4) {
                a.touch(x.x, x.y, x.w, x.h);
            }
            a.ai_looks += 1;
            let animal = best.iter().any(|x| {
                x.score >= d.min_confidence.min(0.4)
                    && crate::ai::ANIMALS.contains(&x.label.as_str())
                    && !d.objects.contains(&x.label)
            });
            if animal && !a.animal_seen {
                a.animal_seen = true;
                tracing::debug!(
                    "[{}] an animal that isn't watched for is moving: no motion alert",
                    self.cam.name
                );
            }
        }
        // Parked vehicles don't alert until they move.
        if d.ignore_parked && !d.parked_objects.is_empty() {
            // Tracked from every sighting (from 30%), not only those strong
            // enough to alert: a parked car that comes and goes around the
            // minimum confidence would otherwise be forgotten and alert again.
            let kinds = &d.parked_objects;
            let mut others: Vec<Detection> = hits
                .into_iter()
                .filter(|h| !kinds.contains(&h.label))
                .collect();
            let candidates: Vec<Detection> = best
                .iter()
                .filter(|x| x.score >= 0.3 && kinds.contains(&x.label))
                .cloned()
                .collect();
            let (keep, notes) = self.parked.lock().unwrap().filter(
                candidates,
                kinds,
                Duration::from_secs(d.parked_seconds.max(10) as u64),
            );
            others.extend(
                keep.into_iter()
                    .filter(|x| x.score >= d.min_confidence && d.objects.contains(&x.label)),
            );
            hits = others;
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
        // Not confirmed in full quality a moment ago, at the same spot: still
        // nothing (no new alert for it).
        {
            let mut rj = self.rejected.lock().unwrap();
            rj.retain(|(_, _, t)| t.elapsed() < Duration::from_secs(120));
            if !rj.is_empty() {
                hits.retain(|h| !rj.iter().any(|(l, b, _)| *l == h.label && same_spot(h, *b)));
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
        let mut hd_checks: Vec<(String, [f32; 4])> = Vec::new();
        let mut changed = !a.qualified;
        for h in &hits {
            // The object's colour, measured from the pixels in its box.
            let color = frame.color_name(h.x, h.y, h.w, h.h).map(str::to_string);
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
                        hd: None,
                    });
                    new_labels.push(format!("{} {:.0}%", h.label, h.score * 100.0));
                    hd_checks.push((h.label.clone(), [h.x, h.y, h.w, h.h]));
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
        // Something new: checked in a full-quality picture (see apply_hd).
        if !hd_checks.is_empty()
            && d.hd_check != "off"
            && let Some(relay) = self.relay_main.clone()
        {
            a.event.hd_checking = true;
            changed = true;
            let (ai, tx, id, ffmpeg) = (self.ai.get(), hd_tx.clone(), a.event.id.clone(), self.env.ffmpeg_path.clone());
            let (min_conf, watched) = (d.min_confidence, d.objects.clone());
            let cam = self.cam.clone();
            tokio::spawn(async move {
                let mut checks = Vec::new();
                match hd_frame(&relay, &ffmpeg).await {
                    Ok(mut f) => {
                        // Privacy areas are black here too.
                        if !cam.privacy.is_empty() {
                            let mut px = (*f.rgb).clone();
                            cam.paint_privacy(&mut px, f.width, f.height);
                            f.rgb = Arc::new(px);
                        }
                        for (label, b) in hd_checks {
                            let v = hd_verify(&ai, &f, &label, b, min_conf, &watched).await;
                            checks.push((label, b, v));
                        }
                    }
                    Err(e) => {
                        tracing::debug!("full-quality check: {e:#}");
                        checks = hd_checks.into_iter().map(|(l, b)| (l, b, HdVerdict::Failed)).collect();
                    }
                }
                let _ = tx.send(HdResult { event: id, checks }).await;
            });
        }
        a.qualified = true;
        // Only write when something changed (the final state is saved when the event ends).
        if changed {
            self.store.save_detected(a.event.clone());
        }
        self.mark_active(a);
    }

    /// The full-quality check's answer: confirmed, something else, or not
    /// seen (marked, or left out of the alert — the setting).
    fn apply_hd(&self, r: HdResult, active: &mut Option<Active>) {
        let drop = self.cam.detect.hd_check == "drop";
        let is_active = active.as_ref().is_some_and(|a| a.event.id == r.event);
        let Some(mut e) = (if is_active { active.as_ref().map(|a| a.event.clone()) } else { self.store.get(&r.event) }) else {
            return;
        };
        let had_labels = !e.labels.is_empty();
        for (label, b, v) in r.checks {
            match v {
                HdVerdict::Confirmed(s) => {
                    if let Some(l) = e.labels.iter_mut().find(|l| l.label == label) {
                        l.hd = Some(true);
                        l.score = l.score.max(s);
                    }
                    info!(target: "event", "[{}] {label}: confirmed in full quality ({:.0}%)", self.cam.name, s * 100.0);
                }
                HdVerdict::Relabel(new, s) => {
                    info!(target: "event", "[{}] {label}: a {new} in full quality ({:.0}%)", self.cam.name, s * 100.0);
                    if e.labels.iter().any(|l| l.label == new) {
                        e.labels.retain(|l| l.label != label);
                        if let Some(l) = e.labels.iter_mut().find(|l| l.label == new) {
                            l.hd = Some(true);
                            l.score = l.score.max(s);
                        }
                    } else if let Some(l) = e.labels.iter_mut().find(|l| l.label == label) {
                        l.label = new.clone();
                        l.score = s;
                        l.hd = Some(true);
                    }
                    for bx in e.boxes.iter_mut().filter(|bx| bx.label == label) {
                        bx.label = new.clone();
                    }
                    e.notes.push(format!("Full-quality check: a {new}, not a {label}"));
                }
                HdVerdict::NotSeen => {
                    self.rejected.lock().unwrap().push((label.clone(), b, Instant::now()));
                    if drop {
                        info!(target: "event", "[{}] {label}: not seen in full quality — left out", self.cam.name);
                        e.labels.retain(|l| l.label != label);
                        e.boxes.retain(|bx| bx.label != label);
                    } else {
                        info!(target: "event", "[{}] {label}: not seen in full quality — marked", self.cam.name);
                        if let Some(l) = e.labels.iter_mut().find(|l| l.label == label) {
                            l.hd = Some(false);
                        }
                    }
                }
                HdVerdict::Failed => {}
            }
        }
        e.labels.sort_by(|x, y| y.score.total_cmp(&x.score));
        e.hd_checking = false;
        // Nothing left that was found: no alert.
        let gone = drop && had_labels && e.labels.is_empty() && e.items.is_empty();
        if gone {
            info!(target: "event", "[{}] alert dropped: nothing confirmed in full quality", self.cam.name);
            self.store.delete(&e.id);
        }
        match active.as_mut().filter(|a| a.event.id == r.event) {
            Some(a) => {
                a.event = e;
                if gone {
                    // Kept going (unsaved) in case something real turns up.
                    a.qualified = false;
                    a.thumb_score = -1.0;
                } else {
                    self.store.save_detected(a.event.clone());
                }
            }
            None if !gone => self.store.save(e),
            None => {}
        }
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
        // Kept open while more activity comes within the join time: one
        // alert and one video, however many times it triggers.
        let d = &self.cam.detect;
        let post = d.post_seconds.max(d.join_seconds).max(3) as i64;
        let too_long = (now - a.event.start).num_seconds() > MAX_EVENT_SECONDS;
        if idle >= post || too_long {
            // A watched area changed: wait a minute before deciding (the
            // alert stays open; more activity carries on as usual).
            if !too_long {
                let a = active.as_mut().unwrap();
                match &a.item_hold {
                    None => {
                        if let Some(sigs) = self.changed_areas(a) {
                            info!(target: "event", "[{}] watched area looks different: checking again in a minute", self.cam.name);
                            a.item_hold = Some((Instant::now(), sigs));
                            return;
                        }
                    }
                    Some((t, _)) if t.elapsed() < ITEM_HOLD => return,
                    Some(_) => {}
                }
            }
            let a = active.take().unwrap();
            self.finish(a);
        }
    }

    /// The watched areas the activity reached that look different now: how
    /// each looks (None: unchanged / not reached). None if none changed.
    fn changed_areas(&self, a: &Active) -> Option<Vec<Option<Vec<f32>>>> {
        if a.zone_before.is_empty() {
            return None;
        }
        let now = self.latest.lock().unwrap().clone()?;
        let sigs: Vec<Option<Vec<f32>>> = a
            .zone_before
            .iter()
            .enumerate()
            .map(|(zi, (z, before, _))| {
                let sig = z.signature(&now);
                (a.touched.get(zi).copied().unwrap_or(false) && crate::zones::area_change(before, &sig) > 0.12).then_some(sig)
            })
            .collect();
        sigs.iter().any(Option::is_some).then_some(sigs)
    }

    fn finish(&self, mut a: Active) {
        {
            let mut s = self.status.lock().unwrap();
            s.event_active = false;
            s.active_labels.clear();
        }
        let d = &self.cam.detect;
        // Motion that turned out to be a cat (or another animal that isn't
        // watched for), with nothing watched for seen: not kept as an alert.
        if d.ignore_animals
            && a.animal_seen
            && a.event.labels.is_empty()
            && a.event.items.is_empty()
        {
            if a.qualified {
                info!(target: "event", "[{}] motion was an animal that isn't watched for: alert dropped", self.cam.name);
                self.store.delete(&a.event.id);
                a.qualified = false;
            }
        }
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
                // Activity elsewhere in the picture: this area isn't checked
                // (shadows and light changing over it are no parcel).
                if !a.touched.get(zi).copied().unwrap_or(false) {
                    continue;
                }
                let change = crate::zones::area_change(
                    before,
                    &z.signature(&now),
                );
                // Still different, and the same as a minute ago (it stayed:
                // not something that came and went or moved on).
                let stayed = match &a.item_hold {
                    Some((_, sigs)) => sigs
                        .get(zi)
                        .and_then(|x| x.as_ref())
                        .is_some_and(|then| crate::zones::area_change(then, &z.signature(&now)) < 0.08),
                    None => true,
                };
                if change > 0.12 && !stayed {
                    info!(target: "event", "[{}] {}: the change didn't stay a minute — not an item", self.cam.name, z.name);
                }
                if change > 0.12 && stayed {
                    // Keep both pictures: smart search tells a delivered
                    // parcel from one picked up (or a moved bin).
                    let dir = self.store.dir();
                    let (bn, an) = (
                        format!("{}-z{zi}-before.jpg", a.event.id),
                        format!("{}-z{zi}-after.jpg", a.event.id),
                    );
                    if let (Ok(b), Ok(af)) =
                        (before_pic.jpeg(85), z.picture(&now).jpeg(85))
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
                            hd: None,
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

/// The package spot (camera → AI): cells painted on the picture, the same
/// grid as the ignore areas.
struct Watched {
    name: String,
    cells: Vec<bool>,
}

impl Watched {
    fn cell(p: [f32; 2]) -> usize {
        use crate::config::{MASK_H, MASK_W};
        let x = ((p[0].clamp(0.0, 1.0) * MASK_W as f32) as usize).min(MASK_W - 1);
        let y = ((p[1].clamp(0.0, 1.0) * MASK_H as f32) as usize).min(MASK_H - 1);
        y * MASK_W + x
    }
    fn contains(&self, p: [f32; 2]) -> bool {
        self.cells[Self::cell(p)]
    }
    /// Its box (x0, y0, x1, y1), 0..1 of the picture.
    fn bounds(&self) -> (f32, f32, f32, f32) {
        use crate::config::{MASK_H, MASK_W};
        let (mut x0, mut y0, mut x1, mut y1) = (1f32, 1f32, 0f32, 0f32);
        for (i, _) in self.cells.iter().enumerate().filter(|(_, v)| **v) {
            let (cx, cy) = ((i % MASK_W) as f32, (i / MASK_W) as f32);
            x0 = x0.min(cx / MASK_W as f32);
            y0 = y0.min(cy / MASK_H as f32);
            x1 = x1.max((cx + 1.0) / MASK_W as f32);
            y1 = y1.max((cy + 1.0) / MASK_H as f32);
        }
        (x0, y0, x1.max(x0), y1.max(y0))
    }
    /// Does an object's box (with a little margin) reach the area?
    fn reaches(&self, x: f32, y: f32, w: f32, h: f32) -> bool {
        let (mx, my) = (w * 0.1, h * 0.1);
        let (x0, y0, x1, y1) = (x - mx, y - my, x + w + mx, y + h + my);
        (0..=12).any(|i| {
            (0..=12).any(|j| self.contains([x0 + (x1 - x0) * i as f32 / 12.0, y0 + (y1 - y0) * j as f32 / 12.0]))
        })
    }
    /// What the area looks like, for comparing before and after.
    fn signature(&self, f: &Frame) -> Vec<f32> {
        crate::zones::area_signature_by(f, self.bounds(), |p| self.contains(p))
    }
    /// The picture of the area for the alert: just the painted spot (the
    /// rest blacked out).
    fn picture(&self, f: &Frame) -> Frame {
        let (x0, y0, x1, y1) = self.bounds();
        let (fw, fh) = (f.width as f32, f.height as f32);
        let (px, py) = ((x0 * fw).floor(), (y0 * fh).floor());
        let crop = crate::recog::crop(f, px, py, (x1.min(1.0) * fw).ceil() - px, (y1.min(1.0) * fh).ceil() - py);
        let mut rgb = (*crop.rgb).clone();
        for yy in 0..crop.height {
            for xx in 0..crop.width {
                let p = [(px + xx as f32 + 0.5) / fw, (py + yy as f32 + 0.5) / fh];
                if !self.contains(p) {
                    let i = ((yy * crop.width + xx) * 3) as usize;
                    rgb[i..i + 3].copy_from_slice(&[0, 0, 0]);
                }
            }
        }
        let crop = Frame { width: crop.width, height: crop.height, rgb: std::sync::Arc::new(rgb) };
        crop.fit(640)
    }
}

impl Detector {
    /// The package spot (camera → AI), if painted.
    fn watched_areas(&self) -> Vec<Watched> {
        let d = &self.cam.detect;
        let mut v: Vec<Watched> = Vec::new();
        if d.packages
            && let Some(cells) = crate::config::decode_mask(&d.package_mask).filter(|m| m.iter().any(|x| *x))
        {
            v.push(Watched {
                name: "the package spot".into(),
                cells,
            });
        }
        v
    }
}

/// The areas drawn to ignore, if any.
pub fn ignore_mask(d: &crate::config::DetectConfig) -> Option<Vec<bool>> {
    if !d.ai_ignores_mask {
        return None;
    }
    crate::config::decode_mask(&d.mask).filter(|m| m.iter().any(|x| *x))
}

/// How much of a detection's box lies in the areas drawn to ignore (0..1).
pub fn masked_share(mask: &[bool], x: &Detection) -> f32 {
    use crate::config::{MASK_H, MASK_W};
    let cells = |a: f32, b: f32, n: usize| {
        let a = ((a.clamp(0.0, 1.0) * n as f32).floor() as usize).min(n - 1);
        let b = ((b.clamp(0.0, 1.0) * n as f32).ceil() as usize).clamp(a + 1, n);
        a..b
    };
    let (cx, cy) = (cells(x.x, x.x + x.w, MASK_W), cells(x.y, x.y + x.h, MASK_H));
    let (mut all, mut ignored) = (0u32, 0u32);
    for gy in cy {
        for gx in cx.clone() {
            all += 1;
            ignored += mask[gy * MASK_W + gx] as u32;
        }
    }
    if all == 0 { 0.0 } else { ignored as f32 / all as f32 }
}

/// A closer view around a small moving area (normalized box), as a
/// square about three times its size, and where that view sits in the
/// picture (normalized x, y, width, height). None when the moving area is
/// already big enough for the AI to see well.
fn zoom_on(f: &Frame, (x, y, w, h): (f32, f32, f32, f32)) -> Option<(Frame, [f32; 4])> {
    let (fw, fh) = (f.width as f32, f.height as f32);
    let (bw, bh) = (w * fw, h * fh);
    let short = fw.min(fh);
    if bw.max(bh) > short * 0.45 {
        return None;
    }
    let side = (bw.max(bh) * 3.0).clamp(short * 0.35, short);
    let cx = (x + w / 2.0) * fw;
    let cy = (y + h / 2.0) * fh;
    let x0 = (cx - side / 2.0).clamp(0.0, fw - side);
    let y0 = (cy - side / 2.0).clamp(0.0, fh - side);
    let crop = crate::recog::crop(f, x0, y0, side, side);
    Some((
        crop.clone(),
        [
            x0 / fw,
            y0 / fh,
            crop.width as f32 / fw,
            crop.height as f32 / fh,
        ],
    ))
}


/// A moving object found in a frame.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MotionObject {
    /// Box, normalized 0..1.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Size in pixels of a 640×360 picture.
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
        let mask = crate::config::decode_mask(&d.mask).filter(|m| m.iter().any(|x| *x));
        Self {
            min_contrast: d.min_contrast as f32,
            min_object_px: d.min_object_px,
            make_time: Duration::from_millis(d.make_time_ms as u64),
            min_travel_px: d.min_travel_px as f32,
            mask,
        }
    }
}

/// Motion detection on a 4× downscaled grey image:
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
    fn only_activity_at_the_package_spot_counts() {
        // The doorstep: bottom-left corner of the picture.
        use crate::config::{MASK_H, MASK_W};
        let cells = (0..MASK_W * MASK_H)
            .map(|i| {
                let (x, y) = ((i % MASK_W) as f32 / MASK_W as f32, (i / MASK_W) as f32 / MASK_H as f32);
                (0.05..0.3).contains(&x) && (0.7..0.95).contains(&y)
            })
            .collect();
        let spot = super::Watched { name: "spot".into(), cells };
        assert!(spot.reaches(0.1, 0.4, 0.15, 0.45)); // someone at the door
        assert!(spot.reaches(0.0, 0.0, 1.0, 1.0)); // covers it all
        assert!(!spot.reaches(0.6, 0.1, 0.2, 0.3)); // a tree, top right
        assert!(!spot.reaches(0.32, 0.2, 0.1, 0.3)); // close, but above and beside
    }

    #[test]
    fn zooms_on_small_motion_only() {
        let f = Frame {
            width: 640,
            height: 360,
            rgb: std::sync::Arc::new(vec![0; 640 * 360 * 3]),
        };
        let (c, [x, y, w, h]) = super::zoom_on(&f, (0.9, 0.9, 0.05, 0.08)).unwrap();
        assert!(c.width >= 126 && c.width == c.height);
        assert!(x + w <= 1.0001 && y + h <= 1.0001 && x > 0.5 && y > 0.3);
        assert!(super::zoom_on(&f, (0.1, 0.1, 0.6, 0.6)).is_none());
    }

    #[test]
    fn painted_package_spot_compares_only_itself() {
        use crate::config::{MASK_H, MASK_W};
        // A spot painted at the bottom left (x 10–30 %, y 70–90 %).
        let mut cells = vec![false; MASK_W * MASK_H];
        for y in (MASK_H * 7 / 10)..(MASK_H * 9 / 10) {
            for x in (MASK_W / 10)..(MASK_W * 3 / 10) {
                cells[y * MASK_W + x] = true;
            }
        }
        let spot = super::Watched { name: "spot".into(), cells };
        let frame = |parcel: bool, beside: bool| {
            let (w, h) = (320u32, 180u32);
            let mut rgb = vec![90u8; (w * h * 3) as usize];
            for y in 0..h {
                for x in 0..w {
                    let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
                    let on = (parcel && (0.15..0.25).contains(&fx) && (0.75..0.85).contains(&fy))
                        || (beside && (0.4..0.6).contains(&fx) && (0.75..0.85).contains(&fy));
                    if on {
                        let i = ((y * w + x) * 3) as usize;
                        rgb[i..i + 3].copy_from_slice(&[200, 150, 90]);
                    }
                }
            }
            Frame { width: w, height: h, rgb: std::sync::Arc::new(rgb) }
        };
        let before = spot.signature(&frame(false, false));
        assert!(crate::zones::area_change(&before, &spot.signature(&frame(true, false))) > 0.15); // parcel left
        assert!(crate::zones::area_change(&before, &spot.signature(&frame(false, true))) < 0.01); // next to it: not compared
        assert!(spot.reaches(0.12, 0.5, 0.1, 0.3)); // someone at the spot
        assert!(!spot.reaches(0.6, 0.1, 0.2, 0.3)); // elsewhere
        // The alert's picture: exactly the spot, nothing around it.
        let pic = spot.picture(&frame(false, true));
        assert!((pic.width as f32 / pic.height as f32 - (0.2 * 320.0) / (0.2 * 180.0)).abs() < 0.2);
    }

    #[test]
    fn ai_leaves_out_what_is_in_ignored_areas() {
        use crate::config::{MASK_H, MASK_W};
        let mut mask = vec![false; MASK_W * MASK_H];
        for y in 0..MASK_H / 2 {
            for x in 0..MASK_W {
                mask[y * MASK_W + x] = true; // ignore the top half
            }
        }
        let det = |y: f32| Detection { label: "car".into(), score: 0.9, x: 0.4, y, w: 0.2, h: 0.2 };
        assert!(super::masked_share(&mask, &det(0.1)) > 0.99); // in the trees
        assert!(super::masked_share(&mask, &det(0.7)) < 0.01); // on the drive
        assert!(super::masked_share(&mask, &det(0.4)) <= 0.5); // half in: kept
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
