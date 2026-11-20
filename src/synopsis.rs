//! Daily summary (video synopsis): one camera's day (or the last few hours)
//! squeezed into a short video of the objects the **AI recognised**. The empty
//! scene stays still and every person, vehicle or animal of the kinds you
//! chose is laid over it, shifted in time so those from different hours play
//! together, each with its name and the clock time it was really there. Click
//! one in the page and the real footage opens at that moment.
//!
//! Which objects is independent of what is alerted on or recorded: the AI
//! looks at the footage again for the summary, so a cat or a delivery van
//! shows even if no alert or clip was ever made for it. Moving things the AI
//! does not name (shadows, a flapping flag, rain) are left out.
//!
//! How (made on request or at a set time, never touching recording):
//!   1. *Empty scene*: a few keyframes per hour give a per-pixel median, the
//!      scene without anything passing through it (one per hour, so day and
//!      night both work).
//!   2. *Movement*: the footage is decoded at 6 pictures a second at 640×360
//!      and compared with a slowly adapting copy of the scene (things that
//!      stop become scene after a minute); the changed areas are cut out and
//!      joined from picture to picture into **tubes**.
//!   3. *Recognition*: every second or so, while a tube is still unnamed, the
//!      AI looks at that picture; a tube takes the name of the wanted kind the
//!      AI found on it. A tube the AI never names is dropped (and its memory freed).
//!   4. *Arrangement*: tubes go on the new timeline in the order they
//!      happened, as early as they fit without covering each other; if that is
//!      too long they may overlap a little, then play faster.
//!   5. *Video*: scene plus cut-outs with a name and clock label, encoded by
//!      ffmpeg to `nvr-data/synopsis/<camera>/<id>.mp4` with a poster picture
//!      and a JSON file (objects, real times, boxes for the click-through).
//!
//! Footage comes from the light timeline copy where there is one (cheap to
//! decode), else the recordings, local or in deep storage. Nothing here
//! deletes or changes footage. Summaries are made one at a time from a queue,
//! at low priority, waiting while the computer is busy.

use crate::manager::{CameraRuntime, Manager};
use crate::storage::{self, Recording};
use crate::sync::MutexExt;
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration as CDuration, Local, NaiveDate, TimeZone};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

/// Picture size used throughout (other shapes are letter-boxed into it).
pub const W: usize = 640;
pub const H: usize = 360;
/// Movement is looked for on every second pixel.
const DW: usize = W / 2;
const DH: usize = H / 2;
/// Pictures looked at per second of footage, and in the result.
const ANALYSIS_FPS: f64 = 6.0;
const OUT_FPS: f64 = 24.0;
/// A cell of the movement grid is 8×8 detection pixels = 16×16 picture pixels.
const CELL: usize = 8;
const CW: usize = DW / CELL;
const CH: usize = DH.div_ceil(CELL);
/// Footprint grid of the arrangement step: 16 × 9 cells of 40 px.
const GC: usize = 16;
const GR: usize = 9;
type Foot = [u64; 3];
/// Longest piece of one tube (frames) and most tubes kept.
const MAX_TUBE_FRAMES: usize = 540;
const MAX_TUBES: usize = 400;
/// Fewest pictures an object must be seen in, and the least it must move.
const MIN_TUBE_FRAMES: usize = 8;
/// Memory for cut-outs; past this the day is analysed no further.
const MAX_CROP_BYTES: usize = 320 << 20;

/// How movement is found and how long the video is (the length comes from Settings → Daily summary).
#[derive(Debug, Clone, Copy)]
pub struct Params {
    /// How different a pixel must be to count as changed (1–255; lower = more sensitive).
    pub threshold: i32,
    /// Fewest grid cells of movement for something to count.
    pub min_cells: usize,
    /// Wanted length of the video, in seconds.
    pub target_secs: u32,
    /// Objects play at least this much faster than real time (1 = as it happened).
    pub speed: f64,
}

impl Default for Params {
    fn default() -> Self {
        Params { threshold: 28, min_cells: 3, target_secs: 60, speed: 2.0 }
    }
}

// ---------- footage ----------

/// A piece of footage to read: file, when the file starts, and which part.
#[derive(Debug, Clone)]
pub struct Source {
    pub path: PathBuf,
    pub start: DateTime<Local>,
    pub skip: f64,
    pub dur: f64,
}

fn overlap_secs(a: &Recording, b: &Recording) -> f64 {
    let s = a.start.max(b.start);
    let e = a.end.min(b.end);
    ((e - s).num_milliseconds() as f64 / 1000.0).max(0.0)
}

/// The footage of `[from, to)`, in time order: the light timeline copy where
/// there is one, recordings (local, then deep storage) elsewhere.
pub fn pick_sources(rt: &CameraRuntime, from: DateTime<Local>, to: DateTime<Local>) -> Vec<Source> {
    let tdir = storage::timeline_dir(&rt.dir);
    let tl = storage::list(&tdir);
    let mut recs: Vec<(Recording, PathBuf)> = tl.iter().cloned().map(|r| (r, tdir.clone())).collect();
    let mut main: Vec<(Recording, PathBuf)> = storage::list(&rt.dir).into_iter().map(|r| (r, rt.dir.clone())).collect();
    if let Some(a) = storage::archive_of(&rt.dir) {
        main.extend(storage::list(&a).into_iter().map(|r| (r, a.clone())));
    }
    for (r, d) in main {
        let len = (r.end - r.start).num_milliseconds().max(1) as f64 / 1000.0;
        let covered: f64 = tl.iter().map(|t| overlap_secs(&r, t)).sum();
        if covered < len * 0.8 {
            recs.push((r, d));
        }
    }
    recs.retain(|(r, _)| r.end > from && r.start < to);
    recs.sort_by_key(|(r, _)| r.start);
    recs.into_iter()
        .filter_map(|(r, d)| {
            let skip = ((from - r.start).num_milliseconds() as f64 / 1000.0).max(0.0);
            let dur = (r.end.min(to) - r.start.max(from)).num_milliseconds() as f64 / 1000.0;
            (dur >= 1.0).then(|| Source { path: d.join(&r.name), start: r.start, skip, dur })
        })
        .collect()
}

/// ffmpeg, quietly and at low priority (never slows recording or live view).
fn low_cmd(ffmpeg: &str) -> std::process::Command {
    #[cfg(unix)]
    {
        if std::process::Command::new("nice").arg("true").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success()) {
            let mut c = std::process::Command::new("nice");
            c.args(["-n", "10", ffmpeg]);
            return c;
        }
    }
    #[allow(unused_mut)]
    let mut c = crate::load::std_cmd(ffmpeg);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000 | 0x0000_4000); // no window, below normal priority
    }
    c
}

/// Settings → Hardware, as they were when the summary being made started: the
/// graphics card decodes the footage and encodes the video when one is set up
/// (the processor otherwise, or if the card can't do it).
static HW: Mutex<Option<crate::config::HardwareConfig>> = Mutex::new(None);

fn hw() -> Option<crate::config::HardwareConfig> {
    HW.locked().clone().filter(|h| h.uses_gpu())
}

/// Does this ffmpeg encode with this GPU encoder here? (tried once on a tiny picture)
fn encoder_works(ffmpeg: &str, hw: &crate::config::HardwareConfig, encoder: &str) -> bool {
    static TRIED: LazyLock<Mutex<HashMap<String, bool>>> = LazyLock::new(Default::default);
    if let Some(r) = TRIED.locked().get(encoder) {
        return *r;
    }
    let (pre, out) = crate::ffmpeg::encoder_args(encoder, hw, false);
    let mut c = low_cmd(ffmpeg);
    c.args(["-v", "error", "-nostdin"]).args(&pre).args(["-f", "lavfi", "-i", "color=c=black:s=256x144:r=5:d=0.6"]).args(&out).args(["-f", "null", "-"]);
    c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let ok = c.status().is_ok_and(|s| s.success());
    TRIED.locked().insert(encoder.to_string(), ok);
    ok
}

struct Decoder {
    child: Child,
    out: std::process::ChildStdout,
    /// The first picture, already read to see that the graphics card could decode this.
    first: Option<Vec<u8>>,
}

impl Decoder {
    /// On the graphics card when one is set up, and it can read this file; the processor otherwise.
    fn open(ffmpeg: &str, s: &Source, fps: &str, keyframes: bool) -> Result<Decoder> {
        if let Some(h) = hw() {
            let args = crate::ffmpeg::decode_args(&h, false);
            if !args.is_empty()
                && let Ok(mut d) = Self::spawn(ffmpeg, s, fps, keyframes, &args)
            {
                let mut first = vec![0u8; W * H * 3];
                if d.out.read_exact(&mut first).is_ok() {
                    d.first = Some(first);
                    return Ok(d);
                }
            }
        }
        Self::spawn(ffmpeg, s, fps, keyframes, &[])
    }

    fn spawn(ffmpeg: &str, s: &Source, fps: &str, keyframes: bool, hw_args: &[String]) -> Result<Decoder> {
        let vf = format!("fps={fps},scale={W}:{H}:force_original_aspect_ratio=decrease,pad={W}:{H}:(ow-iw)/2:(oh-ih)/2:black");
        let mut c = low_cmd(ffmpeg);
        c.args(["-v", "error", "-nostdin"]);
        c.args(hw_args);
        if keyframes {
            c.args(["-skip_frame", "nokey"]);
        }
        c.args(["-ss", &format!("{:.3}", s.skip), "-t", &format!("{:.3}", s.dur), "-i"]).arg(&s.path);
        c.args(["-an", "-vf", &vf, "-pix_fmt", "rgb24", "-f", "rawvideo", "-"]);
        c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
        let mut child = c.spawn().context("could not start ffmpeg")?;
        let out = child.stdout.take().context("no ffmpeg output")?;
        Ok(Decoder { child, out, first: None })
    }

    /// The next picture into `buf`; false at the end.
    fn next(&mut self, buf: &mut [u8]) -> bool {
        if let Some(f) = self.first.take() {
            buf.copy_from_slice(&f);
            return true;
        }
        self.out.read_exact(buf).is_ok()
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ---------- the empty scene ----------

/// Per-pixel median of pictures (a few of them: the scene without what passed through).
pub fn median_frame(frames: &[Vec<u8>]) -> Vec<u8> {
    let n = frames.len();
    let mut out = vec![0u8; frames[0].len()];
    let mut col = vec![0u8; n];
    for (i, o) in out.iter_mut().enumerate() {
        for (k, f) in frames.iter().enumerate() {
            col[k] = f[i];
        }
        col.select_nth_unstable(n / 2);
        *o = col[n / 2];
    }
    out
}

fn luma(f: &[u8]) -> f64 {
    f.iter().step_by(97).map(|&v| v as f64).sum::<f64>() / f.iter().step_by(97).count().max(1) as f64
}

const BLOCK_SECS: i64 = 3600;
const BLOCK_CAP: usize = 15;
const SAMPLE_EVERY: &str = "1/90:round=up";

/// The empty scene for each hour of `[from, ..)` (None where no footage), and
/// the one shown in the video: the hour with the middle brightness.
type Scenes = (Vec<Option<Vec<u8>>>, Vec<u8>);

fn backgrounds(sources: &[Source], from: DateTime<Local>, ffmpeg: &str, pr: &Progress) -> Result<Scenes> {
    let last = sources.last().map(|s| s.start + CDuration::milliseconds((s.skip * 1000.0) as i64 + (s.dur * 1000.0) as i64)).unwrap_or(from);
    let nblocks = (((last - from).num_seconds() / BLOCK_SECS) + 1).max(1) as usize;
    let mut samples: Vec<Vec<Vec<u8>>> = vec![Vec::new(); nblocks];
    let mut strides = vec![1usize; nblocks];
    let mut seen = vec![0usize; nblocks];
    let mut buf = vec![0u8; W * H * 3];
    for (i, s) in sources.iter().enumerate() {
        if pr.cancelled() {
            bail!("cancelled");
        }
        pr.set("Finding the empty scene", (i * 12 / sources.len().max(1)) as u32);
        let Ok(mut d) = Decoder::open(ffmpeg, s, SAMPLE_EVERY, true) else { continue };
        let mut k = 0.0;
        while d.next(&mut buf) {
            let at = s.start + CDuration::milliseconds(((s.skip + k) * 1000.0) as i64);
            k += 90.0;
            let b = (((at - from).num_seconds() / BLOCK_SECS).max(0) as usize).min(nblocks - 1);
            seen[b] += 1;
            if !seen[b].is_multiple_of(strides[b]) {
                continue;
            }
            samples[b].push(buf.clone());
            if samples[b].len() > BLOCK_CAP {
                // Keep every other one and take fewer from now on.
                let kept: Vec<Vec<u8>> = samples[b].drain(..).step_by(2).collect();
                samples[b] = kept;
                strides[b] *= 2;
            }
        }
    }
    tracing::debug!("synopsis: {} sources, {} blocks, samples per block {:?}", sources.len(), nblocks, samples.iter().map(|s| s.len()).collect::<Vec<_>>());
    let bgs: Vec<Option<Vec<u8>>> = samples.iter().map(|s| (!s.is_empty()).then(|| median_frame(s))).collect();
    let mut lit: Vec<(f64, usize)> = bgs.iter().enumerate().filter_map(|(i, b)| b.as_ref().map(|b| (luma(b), i))).collect();
    if lit.is_empty() {
        bail!("no readable footage found for that day");
    }
    lit.sort_by(|a, b| a.0.total_cmp(&b.0));
    let show = bgs[lit[lit.len() / 2].1].clone().unwrap();
    Ok((bgs, show))
}

fn nearest_bg(bgs: &[Option<Vec<u8>>], b: usize) -> &[u8] {
    let n = bgs.len();
    for d in 0..n {
        for i in [b.checked_sub(d), Some(b + d)].into_iter().flatten() {
            if let Some(Some(bg)) = bgs.get(i) {
                return bg;
            }
        }
    }
    unreachable!("backgrounds() guarantees one scene")
}

// ---------- movement ----------

/// A moving area: its box in picture pixels.
#[derive(Debug, Clone, Copy)]
pub struct Blob {
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
}

/// Compares each picture with a slowly adapting copy of the scene.
pub struct Detector {
    bg: Vec<f32>,
    ready: bool,
    n: u64,
    thr: f32,
    min_cells: usize,
    /// Changed pixels of the last picture (detection size, 0 or 1).
    pub fg: Vec<u8>,
}

fn shrink(full: &[u8], out: &mut [u8]) {
    for y in 0..DH {
        let row = y * 2 * W * 3;
        for x in 0..DW {
            let (s, d) = (row + x * 6, (y * DW + x) * 3);
            out[d..d + 3].copy_from_slice(&full[s..s + 3]);
        }
    }
}

impl Detector {
    pub fn new(p: &Params) -> Detector {
        Detector { bg: vec![0.0; DW * DH * 3], ready: false, n: 0, thr: p.threshold as f32, min_cells: p.min_cells, fg: vec![0; DW * DH] }
    }

    /// Start from this picture as the scene (a full-size picture).
    pub fn seed(&mut self, full: &[u8]) {
        let mut small = vec![0u8; DW * DH * 3];
        shrink(full, &mut small);
        for (b, s) in self.bg.iter_mut().zip(&small) {
            *b = *s as f32;
        }
        self.ready = true;
    }

    /// The moving areas in this picture; `None` when the whole picture changed
    /// (a light switched, a camera knocked): the scene is then taken anew.
    pub fn step(&mut self, full: &[u8]) -> Option<Vec<Blob>> {
        let mut small = vec![0u8; DW * DH * 3];
        shrink(full, &mut small);
        if !self.ready {
            self.seed(full);
        }
        self.n += 1;
        // Overall brightness drift (the sun, auto-exposure) is not movement.
        let mut sum = 0.0f32;
        for p in 0..DW * DH {
            sum += small[p * 3 + 1] as f32 - self.bg[p * 3 + 1];
        }
        let delta = sum / (DW * DH) as f32;
        let mut on = 0usize;
        for p in 0..DW * DH {
            let mut d = 0.0f32;
            for c in 0..3 {
                d = d.max((small[p * 3 + c] as f32 - self.bg[p * 3 + c] - delta).abs());
            }
            let f = (d > self.thr) as u8;
            self.fg[p] = f;
            on += f as usize;
        }
        if on * 2 > DW * DH {
            self.seed(full);
            self.fg.fill(0);
            return None;
        }
        // The scene follows slowly (about once a second): what stops moving becomes scene.
        if self.n.is_multiple_of(ANALYSIS_FPS as u64) {
            for (b, s) in self.bg.iter_mut().zip(&small) {
                *b += (*s as f32 - *b) * 0.02;
            }
        }
        Some(self.blobs())
    }

    fn blobs(&self) -> Vec<Blob> {
        let mut cell = vec![0u16; CW * CH];
        for y in 0..DH {
            for x in 0..DW {
                if self.fg[y * DW + x] != 0 {
                    cell[(y / CELL) * CW + x / CELL] += 1;
                }
            }
        }
        let on: Vec<bool> = cell.iter().map(|&c| c >= 8).collect();
        // Joined if neighbouring or one cell apart (a body is not always one piece).
        let near = |i: usize| -> Vec<usize> {
            let (cx, cy) = ((i % CW) as i32, (i / CW) as i32);
            let mut v = Vec::new();
            for dy in -2..=2 {
                for dx in -2..=2 {
                    let (nx, ny) = (cx + dx, cy + dy);
                    if nx >= 0 && ny >= 0 && (nx as usize) < CW && (ny as usize) < CH {
                        v.push(ny as usize * CW + nx as usize);
                    }
                }
            }
            v
        };
        let mut seen = vec![false; CW * CH];
        let mut out = Vec::new();
        for start in 0..CW * CH {
            if !on[start] || seen[start] {
                continue;
            }
            let (mut n, mut x0, mut y0, mut x1, mut y1) = (0usize, CW, CH, 0usize, 0usize);
            let mut stack = vec![start];
            seen[start] = true;
            while let Some(i) = stack.pop() {
                n += 1;
                let (cx, cy) = (i % CW, i / CW);
                x0 = x0.min(cx);
                x1 = x1.max(cx);
                y0 = y0.min(cy);
                y1 = y1.max(cy);
                for j in near(i) {
                    if on[j] && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
            if n >= self.min_cells {
                let px = CELL * 2;
                out.push(Blob { x0: (x0 * px).saturating_sub(8), y0: (y0 * px).saturating_sub(8), x1: ((x1 + 1) * px + 8).min(W), y1: ((y1 + 1) * px + 8).min(H) });
            }
        }
        out
    }
}

fn dilate(a: &[u8], w: usize, h: usize, r: usize) -> Vec<u8> {
    let mut t = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let lo = x.saturating_sub(r);
            let hi = (x + r).min(w - 1);
            t[y * w + x] = a[y * w + lo..=y * w + hi].iter().copied().max().unwrap_or(0);
        }
    }
    let mut o = vec![0u8; w * h];
    for y in 0..h {
        let lo = y.saturating_sub(r);
        let hi = (y + r).min(h - 1);
        for x in 0..w {
            o[y * w + x] = (lo..=hi).map(|yy| t[yy * w + x]).max().unwrap_or(0);
        }
    }
    o
}

/// Soften a 0–255 mask with a box blur (twice ≈ a smooth bell), radius `r`.
fn feather(a: &[u8], w: usize, h: usize, r: usize) -> Vec<u8> {
    let mut cur: Vec<u8> = a.to_vec();
    for _ in 0..2 {
        let mut t = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                let (lo, hi) = (x.saturating_sub(r), (x + r).min(w - 1));
                let sum: u32 = cur[y * w + lo..=y * w + hi].iter().map(|&v| v as u32).sum();
                t[y * w + x] = (sum / (hi - lo + 1) as u32) as u8;
            }
        }
        for y in 0..h {
            let (lo, hi) = (y.saturating_sub(r), (y + r).min(h - 1));
            for x in 0..w {
                let sum: u32 = (lo..=hi).map(|yy| t[yy * w + x] as u32).sum();
                cur[y * w + x] = (sum / (hi - lo + 1) as u32) as u8;
            }
        }
    }
    cur
}

/// How bright the scene `bg` is in the area of `b` (sampled).
fn area_luma(bg: &[u8], b: &Blob) -> f32 {
    let (mut sum, mut n) = (0.0f32, 0.0f32);
    for y in (b.y0..b.y1).step_by(4) {
        for x in (b.x0..b.x1).step_by(4) {
            let i = (y * W + x) * 3;
            sum += bg[i] as f32 * 0.3 + bg[i + 1] as f32 * 0.59 + bg[i + 2] as f32 * 0.11;
            n += 1.0;
        }
    }
    if n > 0.0 { sum / n } else { 128.0 }
}

/// How much to brighten (or darken) an object seen in the light of `src` so it
/// sits naturally on the scene shown, `shown`, in the area of `b`.
pub fn relight(src: &[u8], shown: &[u8], b: &Blob) -> f32 {
    (area_luma(shown, b).max(8.0) / area_luma(src, b).max(8.0)).clamp(0.6, 1.6)
}

/// The cut-out of a moving area: its pixels with a soft, feathered edge (RGBA),
/// lightened by `gain` to match the scene it will be shown on.
pub fn cutout(full: &[u8], fg: &[u8], b: &Blob, gain: f32) -> Vec<u8> {
    let (w, h) = (b.x1 - b.x0, b.y1 - b.y0);
    let mut a = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (((b.x0 + x) / 2).min(DW - 1), ((b.y0 + y) / 2).min(DH - 1));
            a[y * w + x] = fg[fy * DW + fx] * 255;
        }
    }
    // Fill small holes, then fade the edge over a few pixels.
    let a = feather(&dilate(&a, w, h, 3), w, h, 2);
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let src = ((b.y0 + y) * W + b.x0 + x) * 3;
            let d = (y * w + x) * 4;
            for c in 0..3 {
                rgba[d + c] = (full[src + c] as f32 * gain).min(255.0) as u8;
            }
            rgba[d + 3] = a[y * w + x];
        }
    }
    rgba
}

// ---------- tubes ----------

pub struct TFrame {
    pub rel_ms: u32,
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    pub rgba: Vec<u8>,
}

/// One object the AI found in a picture (pixels of the 640×360 picture).
#[derive(Debug, Clone)]
pub struct Det {
    pub label: String,
    pub score: f32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// What the AI is asked: a full picture in, what it found out.
pub type Classifier<'a> = &'a mut dyn FnMut(&[u8]) -> Result<Vec<Det>>;

/// Looks the AI may take at one tube, and when: the first a little after it
/// appears, then every 1.5 s; after this many looks with no wanted object it is given up.
const MAX_LOOKS: u8 = 6;
const GIVE_UP_LOOKS: u8 = 3;
/// Surest the AI must be that a wanted object is there.
const MIN_SCORE: f32 = 0.35;

pub struct Tube {
    /// Unix milliseconds of the first picture.
    pub start_ms: i64,
    pub frames: Vec<TFrame>,
    /// What the AI named it: (kind, looks that said so, best score).
    pub votes: Vec<(String, u32, f32)>,
    looks: u8,
    last_look_ms: i64,
    dead: bool,
}

impl Tube {
    fn new(start_ms: i64, first: TFrame) -> Tube {
        Tube { start_ms, frames: vec![first], votes: Vec::new(), looks: 0, last_look_ms: start_ms, dead: false }
    }
    fn last(&self) -> &TFrame {
        self.frames.last().unwrap()
    }
    fn centre(f: &TFrame) -> (f32, f32) {
        (f.x as f32 + f.w as f32 / 2.0, f.y as f32 + f.h as f32 / 2.0)
    }
    /// How far from where it started it got (picture pixels).
    fn travel(&self) -> f32 {
        let (cx, cy) = Tube::centre(&self.frames[0]);
        self.frames.iter().map(|f| { let (x, y) = Tube::centre(f); ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() }).fold(0.0, f32::max)
    }
    fn bytes(&self) -> usize {
        self.frames.iter().map(|f| f.rgba.len()).sum()
    }
    /// The kind most looks agreed on (the surest on a tie).
    pub fn label(&self) -> Option<(&str, f32)> {
        self.votes.iter().max_by(|a, b| a.1.cmp(&b.1).then(a.2.total_cmp(&b.2))).map(|v| (v.0.as_str(), v.2))
    }
    fn needs_look(&self, ms: i64) -> bool {
        let due = if self.looks == 0 { 600 } else { 1500 };
        !self.dead && self.frames.len() >= 2 && self.looks < MAX_LOOKS && ms - self.last_look_ms >= due
    }
    /// Give up on it: it is not something the AI names, so its pictures are not kept.
    fn kill(&mut self) -> usize {
        let freed = self.bytes();
        let last = self.frames.pop();
        self.frames.clear();
        self.frames.shrink_to_fit();
        // (A placeholder, so `last()` still works while the blob goes on being followed.)
        if let Some(mut l) = last {
            l.rgba = Vec::new();
            self.frames.push(l);
        }
        self.dead = true;
        freed
    }
}

/// Joins moving areas from picture to picture into tubes, and keeps the
/// ones the AI names as one of the wanted kinds.
pub struct Tracker {
    active: Vec<Tube>,
    pub done: Vec<Tube>,
    pub bytes: usize,
    wanted: Vec<String>,
}

impl Tracker {
    /// `wanted`: kinds as GAZE names them ("vehicle" for any vehicle).
    pub fn new(wanted: &[String]) -> Tracker {
        Tracker { active: Vec::new(), done: Vec::new(), bytes: 0, wanted: wanted.iter().map(|w| crate::ai::canon(w).to_string()).collect() }
    }

    fn finish(&mut self, t: Tube) {
        if t.dead || t.frames.len() < MIN_TUBE_FRAMES || t.votes.is_empty() {
            self.bytes = self.bytes.saturating_sub(t.bytes());
            return;
        }
        self.done.push(t);
    }

    /// Is some tube due another look by the AI?
    pub fn wants_look(&self, ms: i64) -> bool {
        self.active.iter().any(|t| t.needs_look(ms))
    }

    /// What the AI found in the picture at `ms`: each tube that was due a look
    /// takes the wanted kind found where it is.
    pub fn apply(&mut self, ms: i64, dets: &[Det]) {
        let mut freed = 0;
        for t in self.active.iter_mut() {
            if !t.needs_look(ms) {
                continue;
            }
            t.looks += 1;
            t.last_look_ms = ms;
            let f = t.last();
            let (fx, fy, fw, fh) = (f.x as f32, f.y as f32, f.w as f32, f.h as f32);
            let mut best: Option<&Det> = None;
            for d in dets {
                let kind = crate::ai::canon(&d.label);
                if d.score < MIN_SCORE || !self.wanted.iter().any(|w| w == kind) {
                    continue;
                }
                let inter = ((fx + fw).min(d.x + d.w) - fx.max(d.x)).max(0.0) * ((fy + fh).min(d.y + d.h) - fy.max(d.y)).max(0.0);
                // It is where the moving area is: most of the smaller of the two is shared.
                if inter / (fw * fh).min((d.w * d.h).max(1.0)).max(1.0) >= 0.3 && best.is_none_or(|b| d.score > b.score) {
                    best = Some(d);
                }
            }
            match best {
                Some(d) => {
                    let kind = crate::ai::canon(&d.label).to_string();
                    match t.votes.iter_mut().find(|v| v.0 == kind) {
                        Some(v) => {
                            v.1 += 1;
                            v.2 = v.2.max(d.score);
                        }
                        None => t.votes.push((kind, 1, d.score)),
                    }
                }
                None if t.looks >= GIVE_UP_LOOKS && t.votes.is_empty() => freed += t.kill(),
                None => {}
            }
        }
        self.bytes = self.bytes.saturating_sub(freed);
    }

    /// One picture's moving areas (with their cut-outs) at `ms`.
    pub fn feed(&mut self, ms: i64, blobs: &[Blob], cut: &mut dyn FnMut(&Blob) -> Vec<u8>) {
        // A tube not seen for a second is over; a very long one is cut here.
        let mut keep = Vec::new();
        for t in std::mem::take(&mut self.active) {
            let gone = ms - (t.start_ms + t.last().rel_ms as i64) > 1250;
            if gone || t.frames.len() >= MAX_TUBE_FRAMES {
                self.finish(t);
            } else {
                keep.push(t);
            }
        }
        self.active = keep;
        let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
        for (ti, t) in self.active.iter().enumerate() {
            let (tx, ty) = Tube::centre(t.last());
            for (bi, b) in blobs.iter().enumerate() {
                let (bx, by) = ((b.x0 + b.x1) as f32 / 2.0, (b.y0 + b.y1) as f32 / 2.0);
                let d = ((tx - bx).powi(2) + (ty - by).powi(2)).sqrt();
                let size = ((b.x1 - b.x0).max(b.y1 - b.y0) as f32).max(t.last().w.max(t.last().h) as f32);
                if d < 0.8 * size + 30.0 {
                    pairs.push((d, ti, bi));
                }
            }
        }
        pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let (mut used_t, mut used_b) = (vec![false; self.active.len()], vec![false; blobs.len()]);
        let mut add: Vec<(usize, usize)> = Vec::new();
        for (_, ti, bi) in pairs {
            if !used_t[ti] && !used_b[bi] {
                used_t[ti] = true;
                used_b[bi] = true;
                add.push((ti, bi));
            }
        }
        for (ti, bi) in add {
            let b = &blobs[bi];
            let t = &mut self.active[ti];
            let rel = (ms - t.start_ms) as u32;
            if t.dead {
                // Followed so it does not start a new tube; its pictures are not kept.
                t.frames[0] = TFrame { rel_ms: rel, x: b.x0 as u16, y: b.y0 as u16, w: (b.x1 - b.x0) as u16, h: (b.y1 - b.y0) as u16, rgba: Vec::new() };
                continue;
            }
            let rgba = cut(b);
            self.bytes += rgba.len();
            t.frames.push(TFrame { rel_ms: rel, x: b.x0 as u16, y: b.y0 as u16, w: (b.x1 - b.x0) as u16, h: (b.y1 - b.y0) as u16, rgba });
            // Still after 10 s and never named by the AI: scenery (a flag, a shrub).
            if t.frames.len() == 60 && t.votes.is_empty() && t.travel() < 12.0 {
                let freed = t.kill();
                self.bytes = self.bytes.saturating_sub(freed);
            }
        }
        for (bi, b) in blobs.iter().enumerate() {
            if used_b[bi] {
                continue;
            }
            let rgba = cut(b);
            self.bytes += rgba.len();
            self.active.push(Tube::new(ms, TFrame { rel_ms: 0, x: b.x0 as u16, y: b.y0 as u16, w: (b.x1 - b.x0) as u16, h: (b.y1 - b.y0) as u16, rgba }));
        }
    }

    pub fn flush(&mut self) {
        for t in std::mem::take(&mut self.active) {
            self.finish(t);
        }
    }
}

// ---------- arrangement ----------

fn frame_at(t: &Tube, rel_ms: f64) -> usize {
    t.frames.partition_point(|f| f.rel_ms as f64 <= rel_ms).saturating_sub(1)
}

/// Output pictures a tube takes at this speed.
fn out_len(t: &Tube, speed: f64) -> usize {
    (t.last().rel_ms as f64 / (1000.0 / OUT_FPS * speed)).floor() as usize + 1
}

fn footprint(t: &Tube, speed: f64) -> Vec<Foot> {
    let step = 1000.0 / OUT_FPS * speed;
    (0..out_len(t, speed))
        .map(|f| {
            let fr = &t.frames[frame_at(t, f as f64 * step)];
            let mut m: Foot = [0; 3];
            let (c0, c1) = (fr.x as usize * GC / W, ((fr.x as usize + fr.w as usize - 1) * GC / W).min(GC - 1));
            let (r0, r1) = (fr.y as usize * GR / H, ((fr.y as usize + fr.h as usize - 1) * GR / H).min(GR - 1));
            for r in r0..=r1 {
                for c in c0..=c1 {
                    let bit = r * GC + c;
                    m[bit / 64] |= 1 << (bit % 64);
                }
            }
            m
        })
        .collect()
}

fn pop(a: &Foot, b: &Foot) -> u32 {
    (a[0] & b[0]).count_ones() + (a[1] & b[1]).count_ones() + (a[2] & b[2]).count_ones()
}

/// Start picture of each tube and the total length. Each tube goes as early as
/// it fits, **anywhere on the timeline**, so objects in different parts of the
/// picture play at the same time; `tol` is how many cells of overlap per
/// picture, on average, are allowed (0 = they never cover each other).
pub fn schedule(foots: &[Vec<Foot>], tol: f64) -> (Vec<usize>, usize) {
    let mut occ: Vec<Foot> = Vec::new();
    let mut starts = Vec::with_capacity(foots.len());
    for foot in foots {
        let allow = (tol * foot.len() as f64) as u32;
        let mut o = 0usize;
        loop {
            let mut cost = 0u32;
            for (f, cell) in foot.iter().enumerate() {
                match occ.get(o + f) {
                    Some(x) => cost += pop(x, cell),
                    None => break,
                }
                if cost > allow {
                    break;
                }
            }
            if cost <= allow || o >= occ.len() {
                break;
            }
            o += 1;
        }
        if occ.len() < o + foot.len() {
            occ.resize(o + foot.len(), [0; 3]);
        }
        for (f, cell) in foot.iter().enumerate() {
            for k in 0..3 {
                occ[o + f][k] |= cell[k];
            }
        }
        starts.push(o);
    }
    (starts, occ.len())
}

pub struct Plan {
    pub speed: f64,
    pub starts: Vec<usize>,
    pub len: usize,
}

/// Fit the tubes into `target` pictures, as fast as asked for: at `base` times
/// real time first, letting objects overlap a little more and then play faster
/// only while it is still too long.
pub fn plan(tubes: &[Tube], target: usize, base: f64) -> Plan {
    let base = base.clamp(1.0, 6.0);
    let mut speeds: Vec<f64> = vec![base, base * 1.5, base * 2.0, base * 3.0, base * 4.0].into_iter().filter(|s| *s <= 8.0).collect();
    speeds.dedup();
    let mut best: Option<Plan> = None;
    for speed in speeds {
        let foots: Vec<Vec<Foot>> = tubes.iter().map(|t| footprint(t, speed)).collect();
        for &tol in &[0.0, 0.2, 0.6, 1.5, 3.0] {
            let (starts, len) = schedule(&foots, tol);
            if len <= target {
                return Plan { speed, starts, len };
            }
            best = Some(Plan { speed, starts, len });
        }
    }
    best.unwrap_or(Plan { speed: base, starts: Vec::new(), len: 0 })
}

// ---------- the video ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectMeta {
    pub id: usize,
    /// What the AI named it ("person", "vehicle", "cat" …) and how sure it was.
    pub label: String,
    pub score: f32,
    /// When it was really there (RFC 3339, local time).
    pub at: String,
    pub end: String,
    pub out_start: usize,
    pub out_end: usize,
    /// `[output picture, x, y, w, h]` (picture pixels) twice a second, for finding it by click.
    pub boxes: Vec<[u32; 5]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meta {
    /// Which summary: a day ("2026-11-18") or a window ("2026-11-19_0630_24h").
    pub id: String,
    pub camera: String,
    pub camera_name: String,
    pub from: String,
    pub to: String,
    pub built_at: String,
    /// Made by the schedule rather than asked for.
    pub scheduled: bool,
    pub w: usize,
    pub h: usize,
    pub fps: f64,
    pub frames: usize,
    pub seconds: f64,
    pub footage_seconds: f64,
    pub speed: f64,
    /// The kinds asked for, and how many objects of each are in it.
    pub wanted: Vec<String>,
    pub counts: Vec<(String, usize)>,
    pub objects: Vec<ObjectMeta>,
    pub notes: Vec<String>,
}

/// Lay one moment of an object on the picture. Between two seen moments the
/// object is drawn as both, each moved towards where it is now and faded by how
/// near it is in time, so its movement is smooth instead of stepping (the
/// footage is looked at 6 times a second, the video shows 24). Returns the box it
/// ended up in (x, y, w, h).
fn blit_between(dst: &mut [u8], a: &TFrame, b: Option<&TFrame>, t: f32) -> (i32, i32, i32, i32) {
    let b = b.filter(|_| t > 0.0);
    // How far the object moved between the two moments.
    let (dx, dy) = match b {
        Some(b) => {
            let (ac, bc) = (Tube::centre(a), Tube::centre(b));
            (bc.0 - ac.0, bc.1 - ac.1)
        }
        None => (0.0, 0.0),
    };
    // (moment, where its top-left goes, how much of it)
    let mut layers: Vec<(&TFrame, i32, i32, f32)> = vec![(a, (a.x as f32 + dx * t).round() as i32, (a.y as f32 + dy * t).round() as i32, 1.0 - t)];
    if let Some(b) = b {
        layers.push((b, (b.x as f32 - dx * (1.0 - t)).round() as i32, (b.y as f32 - dy * (1.0 - t)).round() as i32, t));
    }
    let x0 = layers.iter().map(|l| l.1).min().unwrap().max(0);
    let y0 = layers.iter().map(|l| l.2).min().unwrap().max(0);
    let x1 = layers.iter().map(|l| l.1 + l.0.w as i32).max().unwrap().min(W as i32);
    let y1 = layers.iter().map(|l| l.2 + l.0.h as i32).max().unwrap().min(H as i32);
    if x1 <= x0 || y1 <= y0 {
        return (x0, y0, 0, 0);
    }
    let (uw, uh) = ((x1 - x0) as usize, (y1 - y0) as usize);
    // Colour and coverage of both added up (so two solid layers stay solid).
    let mut acc = vec![0f32; uw * uh * 4];
    for (f, lx, ly, wgt) in &layers {
        if *wgt <= 0.0 {
            continue;
        }
        let (fw, fh) = (f.w as i32, f.h as i32);
        for y in 0..fh {
            let uy = ly + y - y0;
            if uy < 0 || uy >= uh as i32 {
                continue;
            }
            for x in 0..fw {
                let ux = lx + x - x0;
                if ux < 0 || ux >= uw as i32 {
                    continue;
                }
                let s = ((y * fw + x) * 4) as usize;
                let al = f.rgba[s + 3] as f32 / 255.0 * wgt;
                if al <= 0.0 {
                    continue;
                }
                let d = (uy as usize * uw + ux as usize) * 4;
                for c in 0..3 {
                    acc[d + c] += f.rgba[s + c] as f32 * al;
                }
                acc[d + 3] += al;
            }
        }
    }
    for uy in 0..uh {
        for ux in 0..uw {
            let d = (uy * uw + ux) * 4;
            let al = acc[d + 3];
            if al <= 0.001 {
                continue;
            }
            let cover = al.min(1.0);
            let o = ((y0 as usize + uy) * W + x0 as usize + ux) * 3;
            for c in 0..3 {
                // (average colour of what covers it) × coverage + what was there × the rest
                dst[o + c] = (acc[d + c] / al * cover + dst[o + c] as f32 * (1.0 - cover)).min(255.0) as u8;
            }
        }
    }
    let (bx, by) = (layers.iter().map(|l| l.1 as f32 * l.3).sum::<f32>().round() as i32, layers.iter().map(|l| l.2 as f32 * l.3).sum::<f32>().round() as i32);
    (bx, by, a.w as i32, a.h as i32)
}

fn outline(dst: &mut [u8], bx: (i32, i32, i32, i32), colour: [u8; 3]) {
    let (x0, y0) = (bx.0.max(0) as usize, bx.1.max(0) as usize);
    let (x1, y1) = (((bx.0 + bx.2).min(W as i32) - 1).max(0) as usize, ((bx.1 + bx.3).min(H as i32) - 1).max(0) as usize);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    // Drawn half-strength, so it frames the object without a hard line.
    let mut put = |x: usize, y: usize| {
        let i = (y * W + x) * 3;
        for c in 0..3 {
            dst[i + c] = ((dst[i + c] as u32 + colour[c] as u32 * 2) / 3) as u8;
        }
    };
    for x in x0..=x1 {
        put(x, y0);
        put(x, y1);
    }
    for y in y0..=y1 {
        put(x0, y);
        put(x1, y);
    }
}

/// A colour for each kind: people green, vehicles blue, animals amber, the rest purple.
fn colour_of(label: &str) -> [u8; 3] {
    if label == "person" {
        [34, 197, 94]
    } else if label == "vehicle" || label == "bicycle" {
        [59, 130, 246]
    } else if crate::ai::ANIMALS.contains(&label) {
        [234, 179, 8]
    } else {
        [168, 85, 247]
    }
}

fn clock(ms: i64) -> String {
    Local.timestamp_millis_opt(ms).single().map(|t| t.format("%H:%M:%S").to_string()).unwrap_or_default()
}

/// What `render` gives back.
pub struct Rendered {
    pub frames: usize,
    pub objects: Vec<ObjectMeta>,
    /// The busiest moment, as a picture (RGB), for the card.
    pub poster: Vec<u8>,
}

/// The video frames: the scene with each tube laid over it; written to `sink`.
pub fn render(bg: &[u8], tubes: &[Tube], plan: &Plan, sink: &mut dyn FnMut(&[u8]) -> Result<()>, pr: &Progress) -> Result<Rendered> {
    let step = 1000.0 / OUT_FPS * plan.speed;
    // At least two seconds, so an empty day still shows its scene.
    let total = (plan.len + OUT_FPS as usize).max(OUT_FPS as usize * 2);
    let mut metas: Vec<ObjectMeta> = tubes
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let (label, score) = t.label().map(|(l, s)| (l.to_string(), s)).unwrap_or_default();
            ObjectMeta {
                id: i,
                label,
                score,
                at: Local.timestamp_millis_opt(t.start_ms).single().map(|d| d.to_rfc3339()).unwrap_or_default(),
                end: Local.timestamp_millis_opt(t.start_ms + t.last().rel_ms as i64).single().map(|d| d.to_rfc3339()).unwrap_or_default(),
                out_start: plan.starts[i],
                out_end: plan.starts[i] + out_len(t, plan.speed) - 1,
                boxes: Vec::new(),
            }
        })
        .collect();
    let mut frame = vec![0u8; bg.len()];
    let (mut poster, mut poster_n) = (bg.to_vec(), 0usize);
    for f in 0..total {
        if pr.cancelled() {
            bail!("cancelled");
        }
        if f % 24 == 0 {
            pr.set("Making the video", 88 + (f * 12 / total) as u32);
        }
        frame.copy_from_slice(bg);
        let mut shown = 0;
        for (i, t) in tubes.iter().enumerate() {
            let (s, e) = (plan.starts[i], plan.starts[i] + out_len(t, plan.speed));
            if f < s || f >= e {
                continue;
            }
            shown += 1;
            // Which two seen moments it is between, and how far.
            let rel = (f - s) as f64 * step;
            let ia = frame_at(t, rel);
            let fa = &t.frames[ia];
            let next = t.frames.get(ia + 1).filter(|n| n.rel_ms.saturating_sub(fa.rel_ms) <= 600);
            let frac = next.map(|n| ((rel - fa.rel_ms as f64) / (n.rel_ms - fa.rel_ms).max(1) as f64).clamp(0.0, 1.0) as f32).unwrap_or(0.0);
            let bx = blit_between(&mut frame, fa, next, frac);
            outline(&mut frame, bx, colour_of(&metas[i].label));
            let label = format!("{} {}", metas[i].label, clock(t.start_ms + fa.rel_ms as i64));
            let ly = if bx.1 >= 12 { bx.1 - 12 } else { bx.1 + bx.3 + 1 };
            crate::draw::caption(&mut frame, W as u32, H as u32, bx.0, ly, &label, 1);
            if (f - s) % (OUT_FPS as usize / 2).max(1) == 0 {
                metas[i].boxes.push([f as u32, bx.0.max(0) as u32, bx.1.max(0) as u32, bx.2 as u32, bx.3 as u32]);
            }
        }
        if shown > poster_n {
            poster_n = shown;
            poster.copy_from_slice(&frame);
        }
        sink(&frame)?;
    }
    Ok(Rendered { frames: total, objects: metas, poster })
}

fn encode_to(ffmpeg: &str, out: &Path, run: impl FnOnce(&mut dyn FnMut(&[u8]) -> Result<()>) -> Result<()>) -> Result<()> {
    let tmp = out.with_extension("part.mp4");
    let mut c = low_cmd(ffmpeg);
    // The graphics card's H.264 encoder when one is set up and works here; libx264 otherwise.
    let gpu = hw().and_then(|h| {
        let enc = h.resolved_encoder().to_string();
        (enc != "libx264" && encoder_works(ffmpeg, &h, &enc)).then(|| (crate::ffmpeg::encoder_args(&enc, &h, false), enc))
    });
    if let Some(((pre, _), _)) = &gpu {
        c.args(["-y", "-v", "error"]).args(pre);
    } else {
        c.args(["-y", "-v", "error"]);
    }
    c.args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-s", &format!("{W}x{H}"), "-r", &OUT_FPS.to_string(), "-i", "-"]);
    match &gpu {
        Some(((_, out_args), enc)) => {
            tracing::debug!("daily summary: encoding with {enc}");
            c.args(out_args).args(["-movflags", "+faststart"]).arg(&tmp);
        }
        None => {
            c.args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "25", "-pix_fmt", "yuv420p", "-movflags", "+faststart"]).arg(&tmp);
        }
    }
    c.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = c.spawn().context("could not start ffmpeg")?;
    let mut stdin = child.stdin.take().context("no ffmpeg input")?;
    let mut stderr = child.stderr.take().context("no ffmpeg error output")?;
    let errs = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.by_ref().take(4000).read_to_string(&mut s);
        s
    });
    let r = run(&mut |f| stdin.write_all(f).context("ffmpeg stopped while the video was being made"));
    drop(stdin);
    let status = child.wait()?;
    let msg = errs.join().unwrap_or_default();
    if let Err(e) = r {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if !status.success() {
        let _ = std::fs::remove_file(&tmp);
        bail!("ffmpeg could not make the video: {}", msg.trim());
    }
    std::fs::rename(&tmp, out)?;
    Ok(())
}

// ---------- one summary ----------

pub struct Progress {
    pct: AtomicU32,
    phase: Mutex<String>,
    cancel: AtomicBool,
}

impl Progress {
    pub fn new() -> Progress {
        Progress { pct: AtomicU32::new(0), phase: Mutex::new(String::new()), cancel: AtomicBool::new(false) }
    }
    fn set(&self, phase: &str, pct: u32) {
        self.pct.store(pct.min(100), Ordering::Relaxed);
        let mut p = self.phase.locked();
        if *p != phase {
            *p = phase.to_string();
        }
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// What to make.
#[derive(Debug, Clone)]
pub struct Request {
    pub camera: String,
    pub camera_name: String,
    /// "2026-11-18" or "2026-11-19_0630_24h" (see `parse_id`).
    pub id: String,
    pub from: DateTime<Local>,
    pub to: DateTime<Local>,
    pub scheduled: bool,
    pub params: Params,
    /// The kinds to show (as the AI names them; "vehicle" for any vehicle).
    pub wanted: Vec<String>,
}

/// Make the summary of `req` from these files into `out` (+ `.json`, `.jpg`).
pub fn make(req: &Request, sources: &[Source], ffmpeg: &str, out: &Path, classify: Classifier, pr: &Progress) -> Result<Meta> {
    if sources.is_empty() {
        bail!("there is no footage for that time");
    }
    let p = &req.params;
    let total_secs: f64 = sources.iter().map(|s| s.dur).sum();
    let (bgs, show) = backgrounds(sources, req.from, ffmpeg, pr)?;
    // Movement, and what the AI says it is.
    let mut det = Detector::new(p);
    let mut tr = Tracker::new(&req.wanted);
    let mut notes = Vec::new();
    let mut buf = vec![0u8; W * H * 3];
    let mut done_secs = 0.0;
    let (mut block, mut looks, mut ai_errors) = (usize::MAX, 0usize, 0usize);
    'sources: for s in sources {
        if pr.cancelled() {
            bail!("cancelled");
        }
        // Wait politely while the computer is busy (but not for ever).
        for _ in 0..15 {
            if crate::load::cpu() <= 85 || pr.cancelled() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
        pr.set("Looking for movement", 12 + (done_secs / total_secs * 70.0) as u32);
        let Ok(mut d) = Decoder::open(ffmpeg, s, &ANALYSIS_FPS.to_string(), false) else { continue };
        let t0 = s.start.timestamp_millis() + (s.skip * 1000.0) as i64;
        let mut k = 0u64;
        let mut last_ms = i64::MIN;
        while d.next(&mut buf) {
            let ms = t0 + (k as f64 * 1000.0 / ANALYSIS_FPS) as i64;
            k += 1;
            if last_ms != i64::MIN && ms - last_ms > 3000 {
                tr.flush();
            }
            last_ms = ms;
            let b = (((ms - req.from.timestamp_millis()) / (BLOCK_SECS * 1000)).max(0) as usize).min(bgs.len() - 1);
            if b != block || k == 1 {
                det.seed(nearest_bg(&bgs, b));
                block = b;
            }
            if let Some(blobs) = det.step(&buf) {
                let fg = det.fg.clone();
                let src_bg = nearest_bg(&bgs, block);
                tr.feed(ms, &blobs, &mut |bl| cutout(&buf, &fg, bl, relight(src_bg, &show, bl)));
                if tr.wants_look(ms) && looks < MAX_LOOKS_TOTAL {
                    looks += 1;
                    match classify(&buf) {
                        Ok(dets) => {
                            ai_errors = 0;
                            tr.apply(ms, &dets);
                        }
                        Err(e) => {
                            ai_errors += 1;
                            if ai_errors >= 3 {
                                bail!("the object recognition (AI) is not answering: {e:#}");
                            }
                        }
                    }
                }
            }
            if tr.bytes > MAX_CROP_BYTES {
                notes.push(format!("Stopped looking at {}: too many objects to hold in memory.", clock(ms)));
                tr.flush();
                break 'sources;
            }
            if looks >= MAX_LOOKS_TOTAL && !notes.iter().any(|n| n.starts_with("Looked")) {
                notes.push(format!("Looked at {MAX_LOOKS_TOTAL} pictures with the AI, the most for one summary: later objects may be missing."));
            }
            if k.is_multiple_of(40) && pr.cancelled() {
                bail!("cancelled");
            }
        }
        done_secs += s.dur;
    }
    tr.flush();
    let mut tubes = std::mem::take(&mut tr.done);
    if tubes.len() > MAX_TUBES {
        tubes.sort_by(|a, b| (b.travel() * b.frames.len() as f32).total_cmp(&(a.travel() * a.frames.len() as f32)));
        tubes.truncate(MAX_TUBES);
        notes.push(format!("{MAX_TUBES} of the busiest objects are shown; the rest were left out."));
    }
    tubes.sort_by_key(|t| t.start_ms);
    // Arrangement.
    pr.set("Arranging", 84);
    let plan = plan(&tubes, (p.target_secs as f64 * OUT_FPS) as usize, p.speed);
    if plan.speed > p.speed.clamp(1.0, 6.0) {
        notes.push(format!("Played {}× faster than real time to fit {} s.", plan.speed, p.target_secs));
    }
    // Video.
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut result = None;
    encode_to(ffmpeg, out, |sink| {
        result = Some(render(&show, &tubes, &plan, sink, pr)?);
        Ok(())
    })?;
    let r = result.context("nothing was drawn")?;
    let _ = std::fs::write(out.with_extension("jpg"), crate::imgutil::encode_jpeg(&r.poster, W as u32, H as u32, 80)?);
    let mut counts: Vec<(String, usize)> = Vec::new();
    for o in &r.objects {
        match counts.iter_mut().find(|c| c.0 == o.label) {
            Some(c) => c.1 += 1,
            None => counts.push((o.label.clone(), 1)),
        }
    }
    counts.sort_by_key(|x| std::cmp::Reverse(x.1));
    let meta = Meta {
        id: req.id.clone(),
        camera: req.camera.clone(),
        camera_name: req.camera_name.clone(),
        from: req.from.to_rfc3339(),
        to: req.to.to_rfc3339(),
        built_at: Local::now().to_rfc3339(),
        scheduled: req.scheduled,
        w: W,
        h: H,
        fps: OUT_FPS,
        frames: r.frames,
        seconds: r.frames as f64 / OUT_FPS,
        footage_seconds: total_secs,
        speed: plan.speed,
        wanted: req.wanted.clone(),
        counts,
        objects: r.objects,
        notes,
    };
    std::fs::write(out.with_extension("json"), serde_json::to_vec(&meta)?)?;
    pr.set("Done", 100);
    Ok(meta)
}

// ---------- which summary: ids and windows ----------

/// An id names what a summary covers: a calendar day ("2026-11-18") or the
/// `N` hours up to a time ("2026-11-19_0630_24h"). Returns (from, to, hours;
/// 0 for a day). Nothing else is accepted, so an id is always a safe file name.
pub fn parse_id(id: &str, now: DateTime<Local>) -> Option<(DateTime<Local>, DateTime<Local>, u32)> {
    if let Ok(d) = NaiveDate::parse_from_str(id, "%Y-%m-%d") {
        if id.len() != 10 {
            return None;
        }
        let from = Local.from_local_datetime(&d.and_hms_opt(0, 0, 0)?).earliest()?;
        let next = Local.from_local_datetime(&d.succ_opt()?.and_hms_opt(0, 0, 0)?).earliest()?;
        return (from < now).then(|| (from, next.min(now), 0));
    }
    let parts: Vec<&str> = id.split('_').collect();
    if let [date, hm, h] = parts[..] {
        let hours: u32 = h.strip_suffix('h')?.parse().ok()?;
        if !(1..=72).contains(&hours) || hm.len() != 4 || date.len() != 10 {
            return None;
        }
        let d = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
        let t = chrono::NaiveTime::from_hms_opt(hm[..2].parse().ok()?, hm[2..].parse().ok()?, 0)?;
        let to = Local.from_local_datetime(&d.and_time(t)).earliest()?;
        return (to <= now + CDuration::minutes(1)).then(|| (to - CDuration::hours(hours as i64), to.min(now), hours));
    }
    None
}

/// The id of the `hours` up to `end` (to the minute).
pub fn window_id(end: DateTime<Local>, hours: u32) -> String {
    format!("{}_{}h", end.format("%Y-%m-%d_%H%M"), hours)
}

// ---------- the queue ----------

enum State {
    Queued,
    Running,
    Done,
    Failed(String),
}

struct Job {
    pr: Arc<Progress>,
    state: State,
    req: Request,
    mgr: Arc<Manager>,
    handle: tokio::runtime::Handle,
    queued_at: std::time::Instant,
}

static JOBS: LazyLock<Mutex<HashMap<String, Job>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static QUEUE: LazyLock<Mutex<std::collections::VecDeque<String>>> = LazyLock::new(|| Mutex::new(std::collections::VecDeque::new()));
static WORKER: AtomicBool = AtomicBool::new(false);
/// Most pictures the AI is shown for one summary.
const MAX_LOOKS_TOTAL: usize = 20_000;

fn key(camera: &str, id: &str) -> String {
    format!("{camera}|{id}")
}

pub fn dir_for(mgr: &Manager, camera: &str) -> Option<PathBuf> {
    storage::valid_folder(camera).then(|| mgr.data_dir.join("synopsis").join(camera))
}

pub fn file_for(mgr: &Manager, camera: &str, id: &str, ext: &str) -> Option<PathBuf> {
    parse_id(id, Local::now())?;
    let p = dir_for(mgr, camera)?.join(format!("{id}.{ext}"));
    p.is_file().then_some(p)
}

fn tidy(dir: &Path, keep_days: u32) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|m| m.elapsed().ok()).is_some_and(|a| a.as_secs() > keep_days as u64 * 86400);
        if old && e.path().extension().is_some_and(|x| x == "mp4" || x == "json" || x == "jpg") {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// One summary per camera: once `id` is made, the camera's other summaries
/// (their video, objects and card picture) are deleted. Never footage, and
/// never a video still being written (`*.part.mp4`).
fn keep_only(dir: &Path, id: &str) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let path = e.path();
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        if stem != id && !stem.ends_with(".part") && path.extension().is_some_and(|x| x == "mp4" || x == "json" || x == "jpg") {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// How `enqueue` went.
#[derive(Debug, PartialEq)]
pub enum Enqueued {
    Queued,
    /// Made already (and a past day does not change).
    Exists,
}

/// Ask for a camera's summary of `id` (a day or window). It joins the queue;
/// summaries are made one at a time.
pub fn enqueue(mgr: &Arc<Manager>, handle: &tokio::runtime::Handle, camera: &str, id: &str, force: bool, scheduled: bool) -> Result<Enqueued> {
    let rt = mgr.camera(camera).context("camera not found")?;
    let dir = dir_for(mgr, camera).context("this camera's name can't be used as a folder")?;
    let now = Local::now();
    let (from, to, hours) = parse_id(id, now).context("that day or time can't be used")?;
    if to <= from {
        bail!("that time has not started yet");
    }
    let finished = dir.join(format!("{id}.json")).is_file();
    // A past day or window doesn't change; today (still going) can be made again.
    let settled = hours > 0 || to < now - CDuration::minutes(2) && from.date_naive() < now.date_naive();
    if finished && !force && settled {
        return Ok(Enqueued::Exists);
    }
    let cfg = mgr.config();
    let sc = &cfg.summary;
    let mut wanted = sc.objects.clone();
    crate::ai::canon_list(&mut wanted);
    let d = Params::default();
    let req = Request {
        camera: camera.to_string(),
        camera_name: rt.cfg.name.clone(),
        id: id.to_string(),
        from,
        to,
        scheduled,
        params: Params { target_secs: sc.seconds.clamp(10, 600), speed: sc.speed.clamp(1.0, 6.0) as f64, ..d },
        wanted,
    };
    let k = key(camera, id);
    {
        let mut jobs = JOBS.locked();
        if jobs.get(&k).is_some_and(|j| matches!(j.state, State::Queued | State::Running)) {
            return Ok(Enqueued::Queued);
        }
        jobs.insert(k.clone(), Job { pr: Arc::new(Progress::new()), state: State::Queued, req, mgr: mgr.clone(), handle: handle.clone(), queued_at: std::time::Instant::now() });
    }
    QUEUE.locked().push_back(k);
    start_worker();
    Ok(Enqueued::Queued)
}

fn start_worker() {
    if WORKER.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        loop {
            let Some(k) = QUEUE.locked().pop_front() else {
                WORKER.store(false, Ordering::SeqCst);
                // (Something may have been queued between the check and the flag.)
                if QUEUE.locked().is_empty() || WORKER.swap(true, Ordering::SeqCst) {
                    return;
                }
                continue;
            };
            let (req, mgr, handle, pr) = {
                let mut jobs = JOBS.locked();
                let Some(j) = jobs.get_mut(&k) else { continue };
                if !matches!(j.state, State::Queued) {
                    continue;
                }
                j.state = State::Running;
                (j.req.clone(), j.mgr.clone(), j.handle.clone(), j.pr.clone())
            };
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_one(&req, &mgr, &handle, &pr))).unwrap_or_else(|_| Err(anyhow::anyhow!("the summary stopped unexpectedly")));
            if let Some(j) = JOBS.locked().get_mut(&k) {
                j.state = match r {
                    Ok(()) => State::Done,
                    Err(e) => {
                        if !format!("{e:#}").contains("no footage") {
                            tracing::warn!("daily summary {k}: {e:#}");
                        }
                        State::Failed(format!("{e:#}"))
                    }
                };
            }
        }
    });
}

fn run_one(req: &Request, mgr: &Arc<Manager>, handle: &tokio::runtime::Handle, pr: &Progress) -> Result<()> {
    let ai = mgr.ai.get();
    if !ai.enabled() {
        bail!("object recognition (AI) is switched off, and the summary only shows what the AI recognises: switch it on in Settings → Object recognition");
    }
    let rt = mgr.camera(&req.camera).context("camera not found")?;
    let dir = dir_for(mgr, &req.camera).context("camera folder")?;
    let ffmpeg = mgr.config().server.ffmpeg_path.clone();
    *HW.locked() = Some(mgr.config().hardware.clone());
    tidy(&dir, mgr.config().summary.keep_days);
    let sources = pick_sources(&rt, req.from, req.to);
    let mut classify = |rgb: &[u8]| -> Result<Vec<Det>> {
        let frame = crate::imgutil::Frame { width: W as u32, height: H as u32, rgb: Arc::new(rgb.to_vec()) };
        let dets = handle.block_on(ai.detect(&frame)).map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(dets
            .into_iter()
            .map(|d| Det { label: d.label, score: d.score, x: d.x * W as f32, y: d.y * H as f32, w: d.w * W as f32, h: d.h * H as f32 })
            .collect())
    };
    make(req, &sources, &ffmpeg, &dir.join(format!("{}.mp4", req.id)), &mut classify, pr)?;
    // The new one replaces the camera's earlier summary.
    keep_only(&dir, &req.id);
    JOBS.locked().retain(|_, j| j.req.camera != req.camera || j.req.id == req.id || !matches!(j.state, State::Done));
    Ok(())
}

/// Stop one (or take it off the queue).
pub fn cancel(camera: &str, id: &str) -> bool {
    let k = key(camera, id);
    let mut jobs = JOBS.locked();
    match jobs.get_mut(&k) {
        Some(j) if matches!(j.state, State::Queued) => {
            j.state = State::Failed("cancelled".into());
            true
        }
        Some(j) if matches!(j.state, State::Running) => {
            j.pr.cancel.store(true, Ordering::Relaxed);
            true
        }
        _ => false,
    }
}

/// Delete a finished summary's files (never footage).
pub fn remove(mgr: &Manager, camera: &str, id: &str) -> bool {
    let Some(dir) = dir_for(mgr, camera) else { return false };
    if parse_id(id, Local::now()).is_none() {
        return false;
    }
    let mut any = false;
    for ext in ["mp4", "json", "jpg"] {
        any |= std::fs::remove_file(dir.join(format!("{id}.{ext}"))).is_ok();
    }
    JOBS.locked().remove(&key(camera, id));
    any
}

fn job_json(k: &str, j: &Job) -> Option<serde_json::Value> {
    use serde_json::json;
    let base = json!({ "camera": j.req.camera, "camera_name": j.req.camera_name, "id": j.req.id });
    let mut v = match &j.state {
        State::Queued => {
            let ahead = QUEUE.locked().iter().position(|q| q == k).unwrap_or(0);
            json!({ "state": "queued", "ahead": ahead })
        }
        State::Running => json!({ "state": "running", "phase": j.pr.phase.locked().clone(), "pct": j.pr.pct.load(Ordering::Relaxed) }),
        State::Failed(e) => json!({ "state": "failed", "error": e }),
        State::Done => return None,
    };
    v.as_object_mut()?.extend(base.as_object()?.clone());
    Some(v)
}

/// What the page shows for one summary: queued / running / failed, or the finished one.
pub fn status(mgr: &Manager, camera: &str, id: &str) -> serde_json::Value {
    use serde_json::json;
    if let Some(j) = JOBS.locked().get(&key(camera, id))
        && let Some(v) = job_json(&key(camera, id), j)
    {
        return v;
    }
    let meta = dir_for(mgr, camera).and_then(|d| std::fs::read(d.join(format!("{id}.json"))).ok()).and_then(|b| serde_json::from_slice::<Meta>(&b).ok());
    match meta {
        Some(m) => json!({ "state": "done", "meta": m }),
        None => json!({ "state": "none" }),
    }
}

/// A finished summary as the card on the Alerts page shows it.
#[derive(Serialize)]
pub struct Card {
    pub camera: String,
    pub camera_name: String,
    pub id: String,
    pub from: String,
    pub to: String,
    pub built_at: String,
    pub scheduled: bool,
    pub seconds: f64,
    pub objects: usize,
    pub counts: Vec<(String, usize)>,
}

/// The latest finished summary of each camera `visible` allows (all when None),
/// newest first, at most `limit`, and the ones queued or being made. (Folders
/// from before 1.7.1 may still hold older ones until the camera's next summary.)
pub fn list(mgr: &Manager, visible: Option<&[String]>, limit: usize) -> (Vec<Card>, Vec<serde_json::Value>) {
    let mut cards: Vec<Card> = Vec::new();
    for rt in mgr.cameras() {
        let id = &rt.cfg.id;
        if visible.is_some_and(|v| !v.contains(id)) {
            continue;
        }
        let Some(dir) = dir_for(mgr, id) else { continue };
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let path = e.path();
            if path.extension().is_none_or(|x| x != "json") {
                continue;
            }
            let Some(m) = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Meta>(&b).ok()) else { continue };
            if let Some(c) = cards.iter_mut().find(|c| &c.camera == id) {
                if m.built_at > c.built_at {
                    *c = Card { camera: id.clone(), camera_name: rt.cfg.name.clone(), id: m.id, from: m.from, to: m.to, built_at: m.built_at, scheduled: m.scheduled, seconds: m.seconds, objects: m.objects.len(), counts: m.counts };
                }
                continue;
            }
            cards.push(Card { camera: id.clone(), camera_name: rt.cfg.name.clone(), id: m.id, from: m.from, to: m.to, built_at: m.built_at, scheduled: m.scheduled, seconds: m.seconds, objects: m.objects.len(), counts: m.counts });
        }
    }
    cards.sort_by(|a, b| b.built_at.cmp(&a.built_at));
    cards.truncate(limit);
    let active: Vec<serde_json::Value> = {
        let jobs = JOBS.locked();
        let mut v: Vec<(std::time::Instant, serde_json::Value)> = jobs
            .iter()
            .filter(|(_, j)| visible.is_none_or(|vis| vis.contains(&j.req.camera)))
            .filter_map(|(k, j)| job_json(k, j).map(|x| (j.queued_at, x)))
            .collect();
        v.sort_by_key(|x| x.0);
        v.into_iter().map(|x| x.1).collect()
    };
    (cards, active)
}

// ---------- the schedule ----------

/// A time is "today" for the schedule for this long after it passes (GAZE
/// may have been off), as the daily digest does.
const CATCH_UP: std::time::Duration = std::time::Duration::from_secs(6 * 3600);

fn parse_time(s: &str) -> chrono::NaiveTime {
    chrono::NaiveTime::parse_from_str(s.trim(), "%H:%M").unwrap_or_else(|_| chrono::NaiveTime::from_hms_opt(6, 30, 0).unwrap())
}

/// The scheduled time of `now`'s day, if it is time to make today's summaries.
pub fn due(now: DateTime<Local>, time: &str, last_done: Option<&str>) -> Option<DateTime<Local>> {
    let at = now.date_naive().and_time(parse_time(time)).and_local_timezone(Local).earliest()?;
    if now < at || (now - at).to_std().is_ok_and(|d| d > CATCH_UP) {
        return None;
    }
    (last_done.map(str::trim) != Some(&now.format("%Y-%m-%d").to_string())).then_some(at)
}

/// Make summaries by themselves at the set time (Settings → Daily summary).
pub async fn run(mgr: Arc<Manager>) {
    let marker = mgr.data_dir.join("summary-last");
    tokio::time::sleep(std::time::Duration::from_secs(50)).await;
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        let cfg = mgr.config();
        let sc = &cfg.summary;
        if !sc.enabled {
            continue;
        }
        let now = Local::now();
        let last = std::fs::read_to_string(&marker).ok();
        let Some(at) = due(now, &sc.time, last.as_deref()) else { continue };
        // Marked first: a failing camera must not be retried every minute.
        let _ = std::fs::write(&marker, now.format("%Y-%m-%d").to_string());
        let id = window_id(at, sc.hours.clamp(1, 72));
        let handle = tokio::runtime::Handle::current();
        for rt in mgr.cameras() {
            if !sc.cameras.is_empty() && !sc.cameras.contains(&rt.cfg.id) {
                continue;
            }
            if let Err(e) = enqueue(&mgr, &handle, &rt.cfg.id, &id, false, true) {
                tracing::warn!("daily summary for {}: {e:#}", rt.cfg.name);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphics_card_settings_that_do_not_work_fall_back_to_the_processor() {
        let ff = "ffmpeg";
        let dir = std::env::temp_dir().join(format!("nvr-syn-hw-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("in.mp4");
        let made = std::process::Command::new(ff).args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=320x180:rate=5:d=3", "-c:v", "libx264", "-pix_fmt", "yuv420p"]).arg(&src).status();
        if !made.is_ok_and(|s| s.success()) {
            return; // no ffmpeg with libx264 here
        }
        // A decoder and an encoder this computer doesn't have: the picture and the video still come out.
        *HW.locked() = Some(crate::config::HardwareConfig { hwaccel: "nosuchaccel".into(), encoder: "h264_nosuch".into(), ..Default::default() });
        let source = Source { path: src, start: Local::now(), skip: 0.0, dur: 2.0 };
        let mut d = Decoder::open(ff, &source, "5", false).unwrap();
        let mut buf = vec![0u8; W * H * 3];
        assert!(d.next(&mut buf), "decoded on the processor");
        assert!(buf.iter().any(|b| *b != 0));
        let out = dir.join("out.mp4");
        encode_to(ff, &out, |sink| {
            for _ in 0..10 {
                sink(&buf)?;
            }
            Ok(())
        })
        .unwrap();
        assert!(std::fs::metadata(&out).unwrap().len() > 100, "encoded with libx264");
        *HW.locked() = None;
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn solid(r: u8, g: u8, b: u8) -> Vec<u8> {
        let mut v = vec![0u8; W * H * 3];
        for p in v.chunks_mut(3) {
            p.copy_from_slice(&[r, g, b]);
        }
        v
    }

    fn with_box(base: &[u8], x: usize, y: usize, w: usize, h: usize, c: [u8; 3]) -> Vec<u8> {
        let mut v = base.to_vec();
        for yy in y..y + h {
            for xx in x..x + w {
                v[(yy * W + xx) * 3..(yy * W + xx) * 3 + 3].copy_from_slice(&c);
            }
        }
        v
    }

    #[test]
    fn the_median_scene_ignores_what_passes_through() {
        let bg = solid(90, 120, 60);
        let frames = vec![bg.clone(), with_box(&bg, 100, 100, 60, 90, [250, 0, 0]), bg.clone(), with_box(&bg, 300, 50, 60, 90, [0, 0, 250]), bg.clone()];
        assert_eq!(median_frame(&frames), bg);
    }

    #[test]
    fn movement_is_found_where_it_is_and_brightness_drift_is_not_movement() {
        let bg = solid(90, 120, 60);
        let p = Params::default();
        let mut d = Detector::new(&p);
        d.seed(&bg);
        // Nothing moved.
        assert!(d.step(&bg).unwrap().is_empty());
        // A person-sized thing.
        let b = d.step(&with_box(&bg, 200, 120, 40, 100, [230, 40, 40])).unwrap();
        assert_eq!(b.len(), 1, "{b:?}");
        let b = b[0];
        assert!(b.x0 <= 200 && b.x1 >= 240 && b.y0 <= 120 && b.y1 >= 220, "{b:?}");
        assert!(b.x1 - b.x0 < 120 && b.y1 - b.y0 < 160, "{b:?}");
        // The whole picture a little brighter (cloud, auto-exposure): no movement.
        let mut d = Detector::new(&p);
        d.seed(&bg);
        assert!(d.step(&solid(110, 140, 80)).unwrap().is_empty());
        // The whole picture changed (a light switched): scene taken anew, no blob.
        let mut d = Detector::new(&p);
        d.seed(&bg);
        assert!(d.step(&solid(250, 20, 20)).is_none());
        assert!(d.step(&solid(250, 20, 20)).unwrap().is_empty());
    }

    #[test]
    fn something_that_stops_becomes_scene() {
        let bg = solid(90, 120, 60);
        let mut d = Detector::new(&Params::default());
        d.seed(&bg);
        let parked = with_box(&bg, 200, 120, 80, 60, [230, 230, 230]);
        // Seen at once, gone from the picture of movement after about two minutes.
        assert!(!d.step(&parked).unwrap().is_empty());
        let mut gone_at = None;
        for i in 0..(ANALYSIS_FPS as usize * 400) {
            if d.step(&parked).unwrap().is_empty() {
                gone_at = Some(i);
                break;
            }
        }
        let i = gone_at.expect("a parked thing must fade into the scene");
        assert!(i > ANALYSIS_FPS as usize * 10, "too quick: {i}");
    }

    /// A tube of an object walking left to right over `secs` seconds from `ms0`, named by the AI.
    fn walker(ms0: i64, secs: f64, y: usize, x_start: usize) -> Tube {
        let n = (secs * ANALYSIS_FPS) as usize;
        let mk = |i: usize| TFrame { rel_ms: (i as f64 * 1000.0 / ANALYSIS_FPS) as u32, x: (x_start + i * 10) as u16, y: y as u16, w: 40, h: 80, rgba: vec![255; 40 * 80 * 4] };
        let mut t = Tube::new(ms0, mk(0));
        t.frames.extend((1..n).map(mk));
        t.votes.push(("person".into(), 3, 0.9));
        t
    }

    fn person_at(b: &Blob) -> Det {
        Det { label: "person".into(), score: 0.9, x: b.x0 as f32, y: b.y0 as f32, w: (b.x1 - b.x0) as f32, h: (b.y1 - b.y0) as f32 }
    }

    /// Feed a scene frame by frame; `ai` says what the AI finds when asked.
    fn track(wanted: &[&str], frames: usize, scene: impl Fn(usize) -> Vec<u8>, ai: impl Fn(&[Blob]) -> Vec<Det>) -> Tracker {
        let bg = solid(90, 120, 60);
        let mut d = Detector::new(&Params::default());
        d.seed(&bg);
        let wanted: Vec<String> = wanted.iter().map(|w| w.to_string()).collect();
        let mut tr = Tracker::new(&wanted);
        for i in 0..frames {
            let f = scene(i);
            let blobs = d.step(&f).unwrap();
            let fg = d.fg.clone();
            let ms = (i as f64 * 1000.0 / ANALYSIS_FPS) as i64;
            tr.feed(ms, &blobs, &mut |b| cutout(&f, &fg, b, 1.0));
            if tr.wants_look(ms) {
                tr.apply(ms, &ai(&blobs));
            }
        }
        tr.flush();
        tr
    }

    #[test]
    fn only_what_the_ai_names_becomes_an_object() {
        let bg = solid(90, 120, 60);
        // 12 s: someone walking right while a flag flaps in place in a corner.
        let scene = |i: usize| with_box(&with_box(&bg, 30 + i * 10, 200, 30, 70, [220, 40, 40]), 560, 20 + (i % 2) * 4, 30, 40, [250, 250, 0]);
        // The AI finds a person where the walker is, and nothing at the flag.
        let ai = |blobs: &[Blob]| blobs.iter().filter(|b| b.y0 > 100).map(person_at).collect::<Vec<_>>();
        let tr = track(&["person"], 48, scene, ai);
        assert_eq!(tr.done.len(), 1, "only the walker: {}", tr.done.len());
        let t = &tr.done[0];
        assert!(t.frames.len() >= 40 && t.travel() > 300.0);
        assert_eq!(t.label().map(|l| l.0), Some("person"));
        // A flag nobody names leaves nothing behind in memory either.
        assert!(tr.bytes <= t.bytes(), "pictures of the unnamed flag kept: {} vs {}", tr.bytes, t.bytes());
    }

    #[test]
    fn a_kind_you_did_not_pick_is_left_out_and_any_vehicle_counts_as_vehicle() {
        let bg = solid(90, 120, 60);
        let scene = |i: usize| with_box(&bg, 30 + i * 10, 200, 40, 40, [220, 220, 40]);
        let said = |label: &'static str| move |blobs: &[Blob]| blobs.iter().map(|b| Det { label: label.into(), ..person_at(b) }).collect::<Vec<_>>();
        // A cat, when only people are wanted: not shown.
        assert_eq!(track(&["person"], 40, scene, said("cat")).done.len(), 0);
        // A cat, when cats are wanted: shown, named cat.
        let tr = track(&["person", "cat"], 40, scene, said("cat"));
        assert_eq!((tr.done.len(), tr.done[0].label().map(|l| l.0)), (1, Some("cat")));
        // A truck, when "vehicle" is wanted: a vehicle.
        let tr = track(&["vehicle"], 40, scene, said("truck"));
        assert_eq!(tr.done[0].label().map(|l| l.0), Some("vehicle"));
        // An unsure AI (below the minimum) names nothing.
        let weak = |blobs: &[Blob]| blobs.iter().map(|b| Det { score: 0.2, ..person_at(b) }).collect::<Vec<_>>();
        assert_eq!(track(&["person"], 40, scene, weak).done.len(), 0);
    }

    #[test]
    fn someone_standing_still_is_kept_when_the_ai_names_them() {
        let bg = solid(90, 120, 60);
        // Arrives and then stands in place for 15 s.
        let scene = |i: usize| with_box(&bg, 200 + i.min(6) * 6, 150, 40, 100, [40, 40, 220]);
        let ai = |blobs: &[Blob]| blobs.iter().map(person_at).collect::<Vec<_>>();
        let tr = track(&["person"], 60, scene, ai);
        assert!(!tr.done.is_empty(), "a person standing still was dropped");
    }

    #[test]
    fn objects_from_different_times_share_the_timeline_without_covering_each_other() {
        // Three people crossing at the same height at 8:00, 12:00 and 17:00 (10 s each).
        let tubes = [walker(0, 10.0, 100, 0), walker(4 * 3_600_000, 10.0, 100, 0), walker(9 * 3_600_000, 10.0, 100, 0)];
        let foots: Vec<Vec<Foot>> = tubes.iter().map(|t| footprint(t, 1.0)).collect();
        let (starts, len) = schedule(&foots, 0.0);
        // Same place: they can't all play at once, but each starts only after the
        // one before has moved on (so it is shorter than three walks end to end).
        assert_eq!(starts[0], 0);
        assert!(starts[1] > 0 && starts[2] >= starts[1], "{starts:?}");
        assert!(len < 3 * foots[0].len(), "no saving: {len}");
        // Somewhere else on the picture they can play together.
        let tubes = [walker(0, 10.0, 20, 0), walker(1000, 10.0, 280, 0)];
        let foots: Vec<Vec<Foot>> = tubes.iter().map(|t| footprint(t, 1.0)).collect();
        let (starts, len) = schedule(&foots, 0.0);
        assert_eq!(starts, vec![0, 0]);
        assert_eq!(len, foots[0].len());
    }

    #[test]
    fn objects_in_different_parts_of_the_picture_play_together_whenever_they_happened() {
        // Three people at 3 different heights, hours apart, each 10 s long: they share the screen from the start.
        let tubes: Vec<Tube> = (0..3).map(|i| walker(i as i64 * 3_600_000, 10.0, 20 + i * 100, 0)).collect();
        let foots: Vec<Vec<Foot>> = tubes.iter().map(|t| footprint(t, 1.0)).collect();
        let (starts, len) = schedule(&foots, 0.0);
        assert_eq!(starts, vec![0, 0, 0], "nothing is in the way, so they all start at once");
        assert_eq!(len, foots[0].len(), "one person's time, not three");
        // Even a later tube slots into a gap left earlier (it is not queued after the others).
        let tubes = [walker(0, 10.0, 100, 0), walker(3_600_000, 10.0, 100, 0), walker(7_200_000, 10.0, 250, 0)];
        let foots: Vec<Vec<Foot>> = tubes.iter().map(|t| footprint(t, 1.0)).collect();
        let (starts, _) = schedule(&foots, 0.0);
        assert_eq!(starts[2], 0, "the third is elsewhere on the picture: it starts with the first: {starts:?}");
    }

    #[test]
    fn a_long_busy_day_is_played_at_the_asked_speed_and_faster_only_when_it_must() {
        // 60 walks across the same row.
        let tubes: Vec<Tube> = (0..60).map(|i| walker(i * 600_000, 10.0, 100, 0)).collect();
        // Plenty of time: played at the asked speed (here 2×), not faster.
        let roomy = plan(&tubes, 600 * OUT_FPS as usize, 2.0);
        assert_eq!(roomy.speed, 2.0);
        assert!(roomy.len <= 600 * OUT_FPS as usize);
        // A short wish: faster.
        let tight = plan(&tubes, 10 * OUT_FPS as usize, 1.0);
        assert!(tight.speed > 1.0, "speed {}", tight.speed);
        // Faster means shorter, not longer.
        let slow = plan(&tubes, 10_000 * OUT_FPS as usize, 1.0);
        assert!(roomy.len <= slow.len, "{} vs {}", roomy.len, slow.len);
    }

    #[test]
    fn an_object_moves_smoothly_between_the_moments_it_was_seen() {
        let bg = solid(30, 30, 30);
        // Seen twice, a sixth of a second apart, 60 px apart.
        let mk = |ms: u32, x: u16| TFrame { rel_ms: ms, x, y: 100, w: 40, h: 80, rgba: vec![255; 40 * 80 * 4] };
        let mut t = Tube::new(0, mk(0, 100));
        t.frames.push(mk(167, 160));
        t.votes.push(("person".into(), 2, 0.9));
        let pl = Plan { speed: 1.0, starts: vec![0], len: 6 };
        // The left edge of the white object in each video picture.
        let mut lefts: Vec<i32> = Vec::new();
        render(&bg, &[t], &pl, &mut |f| {
            let row = 140 * W * 3;
            let left = (0..W).find(|x| f[row + x * 3 + 1] > 200).map(|x| x as i32);
            lefts.extend(left);
            Ok(())
        }, &Progress::new()).unwrap();
        // The first pictures cover the 167 ms: steady steps to the right, no jump back and no big leap.
        let steps: Vec<i32> = lefts.windows(2).take(5).map(|w| w[1] - w[0]).collect();
        assert!(steps.iter().all(|&d| (3..=30).contains(&d)), "movement not smooth: lefts {:?}", &lefts[..8.min(lefts.len())]);
        assert!(lefts[0] >= 95 && lefts[0] <= 105, "{lefts:?}");
    }

    #[test]
    fn an_object_is_brightened_to_suit_the_scene_it_is_shown_on() {
        let (day, night) = (solid(160, 160, 160), solid(80, 80, 80));
        let b = Blob { x0: 100, y0: 100, x1: 160, y1: 200 };
        // Seen at night (dark scene), shown on a day scene: lighter. The other way round: darker.
        assert!((relight(&night, &day, &b) - 1.6).abs() < 0.01);
        assert!(relight(&day, &night, &b) < 0.7);
        assert!((relight(&day, &day, &b) - 1.0).abs() < 0.01);
        // The cut-out has the gain in its colours.
        let fg = vec![1u8; DW * DH];
        let c = cutout(&solid(100, 100, 100), &fg, &b, 1.5);
        let mid = ((50 * 60) + 30) * 4;
        assert_eq!(&c[mid..mid + 3], &[150, 150, 150]);
        // And its edge fades over several pixels instead of stopping hard.
        let mut half = vec![0u8; DW * DH];
        for y in 0..DH {
            for x in 65..DW {
                half[y * DW + x] = 1;
            }
        }
        let c = cutout(&solid(100, 100, 100), &half, &b, 1.0);
        let row: Vec<u8> = (0..60).map(|x| c[(50 * 60 + x) * 4 + 3]).collect();
        let soft = row.iter().filter(|&&a| a > 10 && a < 245).count();
        assert!(soft >= 6, "edge too hard ({soft} soft pixels): {row:?}");
        assert!(row[0] < 30 && row[59] > 225, "{row:?}");
    }

    #[test]
    fn the_video_has_the_scene_with_named_objects_and_their_clock_times() {
        let bg = solid(30, 30, 30);
        let tubes = vec![walker(Local::now().timestamp_millis(), 5.0, 100, 0)];
        let pl = plan(&tubes, 10 * OUT_FPS as usize, 2.0);
        let mut frames = 0;
        let mut painted = false;
        let pr = Progress::new();
        let r = render(&bg, &tubes, &pl, &mut |f| {
            frames += 1;
            painted |= f != &bg[..];
            Ok(())
        }, &pr).unwrap();
        assert_eq!(r.frames, frames);
        assert!(painted, "the object was not drawn");
        assert_eq!(r.objects.len(), 1);
        assert_eq!(r.objects[0].label, "person");
        assert!(!r.objects[0].boxes.is_empty() && r.objects[0].out_end >= r.objects[0].out_start);
        // The poster is the busiest moment: it has the object in it.
        assert_ne!(r.poster, bg);
    }

    #[test]
    fn a_new_summary_replaces_the_cameras_earlier_ones() {
        let dir = std::env::temp_dir().join(format!("gaze-syn-keep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let files = ["2026-11-18.mp4", "2026-11-18.json", "2026-11-18.jpg", "2026-11-19_0630_24h.mp4", "2026-11-19_0630_24h.json", "2026-11-19_0630_24h.jpg", "2026-11-19.part.mp4", "notes.txt"];
        for f in files {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        keep_only(&dir, "2026-11-19_0630_24h");
        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["2026-11-19.part.mp4", "2026-11-19_0630_24h.jpg", "2026-11-19_0630_24h.json", "2026-11-19_0630_24h.mp4", "notes.txt"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ids_say_what_a_summary_covers_and_nothing_else_is_accepted() {
        let now = Local.with_ymd_and_hms(2026, 11, 19, 8, 0, 0).earliest().unwrap();
        // A day.
        let (f, t, h) = parse_id("2026-11-18", now).unwrap();
        assert_eq!((f.format("%H:%M").to_string(), t.format("%d %H:%M").to_string(), h), ("00:00".into(), "19 00:00".into(), 0));
        // Today so far ends now.
        let (_, t, _) = parse_id("2026-11-19", now).unwrap();
        assert_eq!(t, now);
        // A window of 24 hours ending 06:30.
        let (f, t, h) = parse_id("2026-11-19_0630_24h", now).unwrap();
        assert_eq!((f.format("%d %H:%M").to_string(), t.format("%d %H:%M").to_string(), h), ("18 06:30".into(), "19 06:30".into(), 24));
        assert_eq!(window_id(t, 24), "2026-11-19_0630_24h");
        // Not accepted: other shapes, path tricks, absurd hours, the future.
        for bad in ["", "..", "../x", "2026-11-18/../x", "2026-1-5", "2026-11-19_0630_0h", "2026-11-19_0630_99h", "2026-11-19_0630_24", "2026-11-19_2530_24h", "2026-11-20", "2026-11-19_0630_24h.mp4", "x_y_z"] {
            assert!(parse_id(bad, now).is_none(), "{bad:?} should not be a summary id");
        }
    }

    #[test]
    fn the_schedule_fires_once_a_day_at_its_time_and_catches_up_for_six_hours() {
        let at = |h, m| Local.with_ymd_and_hms(2026, 11, 19, h, m, 0).earliest().unwrap();
        // Before the time: no.
        assert!(due(at(6, 29), "06:30", None).is_none());
        // At the time: yes, for that time.
        assert_eq!(due(at(6, 31), "06:30", None), Some(at(6, 30)));
        // Already done today: no.
        assert!(due(at(6, 31), "06:30", Some("2026-11-19")).is_none());
        // Done yesterday: yes.
        assert!(due(at(6, 31), "06:30", Some("2026-11-18\n")).is_some());
        // GAZE was off and starts at noon: still caught up; at 13:00 (over 6 h late): not today.
        assert!(due(at(12, 0), "06:30", None).is_some());
        assert!(due(at(13, 0), "06:30", None).is_none());
    }

    #[test]
    fn a_whole_day_from_real_files() {
        // Needs ffmpeg with libx264; skipped without it.
        let ff = "ffmpeg";
        if std::process::Command::new(ff).arg("-version").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("gaze-syn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let clip = dir.join("clip.mp4");
        // 30 s of grey with a red box crossing in 10 s (from 2 s, and again from 17 s) (and a blue one flickering in place).
        let st = std::process::Command::new(ff)
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "color=c=0x606060:s=640x360:r=10:d=30", "-f", "lavfi", "-i", "color=c=red:s=50x90:r=10:d=30", "-f", "lavfi", "-i", "color=c=blue:s=40x40:r=10:d=30",
                "-filter_complex", "[0][1]overlay=x='if(between(mod(t,15),2,12),(mod(t,15)-2)*55,-100)':y=140[a];[a][2]overlay=x=560:y='20+4*mod(floor(t*4),2)'", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-g", "10"])
            .arg(&clip)
            .status()
            .unwrap();
        if !st.success() {
            return; // no libx264 here
        }
        let from = Local::now() - CDuration::minutes(5);
        let src = Source { path: clip, start: from, skip: 0.0, dur: 30.0 };
        let out = dir.join("out").join("day.mp4");
        let req = Request { camera: "cam".into(), camera_name: "Cam".into(), id: "2026-11-18".into(), from, to: from + CDuration::seconds(30), scheduled: false, params: Params::default(), wanted: vec!["person".into()] };
        // A stand-in AI: a person wherever there is something red, nothing else.
        let mut asked = 0;
        let mut ai = |rgb: &[u8]| -> Result<Vec<Det>> {
            asked += 1;
            let (mut x0, mut y0, mut x1, mut y1) = (W, H, 0, 0);
            for y in 0..H {
                for x in 0..W {
                    let i = (y * W + x) * 3;
                    if rgb[i] > 200 && rgb[i + 1] < 80 && rgb[i + 2] < 80 {
                        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
                    }
                }
            }
            Ok(if x1 > x0 { vec![Det { label: "person".into(), score: 0.9, x: x0 as f32, y: y0 as f32, w: (x1 - x0) as f32, h: (y1 - y0) as f32 }] } else { vec![] })
        };
        let m = make(&req, &[src], ff, &out, &mut ai, &Progress::new()).unwrap();
        assert!(out.is_file() && out.with_extension("json").is_file() && out.with_extension("jpg").is_file());
        assert_eq!(m.objects.len(), 2, "two crossings by the red box, none by the flickering blue one: {:?}", m.objects.iter().map(|o| (&o.label, &o.at, o.out_start, o.out_end)).collect::<Vec<_>>());
        assert!(m.objects.iter().all(|o| o.label == "person"));
        assert_eq!(m.counts, vec![("person".to_string(), 2)]);
        assert!(m.seconds >= 2.0);
        assert!(asked > 0 && asked < 200, "the AI was asked {asked} times");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
