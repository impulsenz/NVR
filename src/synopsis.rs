//! Day summary (video synopsis): one camera's day squeezed into a short
//! video. The empty scene stays still and everything that moved that day is
//! laid over it, shifted in time so that people and cars who were there at
//! different hours appear together, each with the clock time they were really
//! there. Click one in the page and the real footage opens at that moment.
//!
//! How (all on request, never in the background, never touching recording):
//!   1. *Empty scene*: a few keyframes per hour are taken from the day's
//!      footage and the per-pixel median is the scene without anything
//!      passing through it (one per hour, so day and night both work).
//!   2. *Movement*: the footage is decoded at 4 pictures a second at
//!      640×360, compared with a slowly adapting copy of the scene (things
//!      that stop become scene after a minute, so a parked car is not
//!      "moving" for an hour), and the changed areas are cut out and joined
//!      from picture to picture into **tubes** (one object's cut-outs over time).
//!   3. *Arrangement*: tubes are placed on the new timeline in the order they
//!      happened, as early as they fit without covering each other (a coarse
//!      grid per output picture); if the result is longer than asked, objects
//!      are allowed to overlap a little and then to move faster.
//!   4. *Video*: the scene plus the cut-outs, a clock label on each, encoded
//!      by ffmpeg to `nvr-data/synopsis/<camera>/<day>.mp4`, with a JSON file
//!      saying where and when each object was (for the click-through).
//!
//! Footage comes from the light timeline copy where there is one (cheap to
//! decode), else the recordings, local or in deep storage. Nothing here
//! deletes or changes footage. Only one summary is made at a time, at low
//! priority, and it waits while the computer is busy.

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
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

/// Picture size used throughout (other shapes are letter-boxed into it).
pub const W: usize = 640;
pub const H: usize = 360;
/// Movement is looked for on every second pixel.
const DW: usize = W / 2;
const DH: usize = H / 2;
/// Pictures looked at per second of footage, and in the result.
const ANALYSIS_FPS: f64 = 4.0;
const OUT_FPS: f64 = 12.0;
/// A cell of the movement grid is 8×8 detection pixels = 16×16 picture pixels.
const CELL: usize = 8;
const CW: usize = DW / CELL;
const CH: usize = DH.div_ceil(CELL);
/// Footprint grid of the arrangement step: 16 × 9 cells of 40 px.
const GC: usize = 16;
const GR: usize = 9;
type Foot = [u64; 3];
/// Longest piece of one tube (frames) and most tubes kept.
const MAX_TUBE_FRAMES: usize = 360;
const MAX_TUBES: usize = 400;
/// Fewest pictures an object must be seen in, and the least it must move.
const MIN_TUBE_FRAMES: usize = 6;
/// Memory for cut-outs; past this the day is analysed no further.
const MAX_CROP_BYTES: usize = 320 << 20;
/// Summaries older than this many days are deleted when a new one is made.
const KEEP_DAYS: u64 = 30;

/// What the user can tune (per request; nothing is saved as a setting).
#[derive(Debug, Clone, Copy)]
pub struct Params {
    /// How different a pixel must be to count as changed (1–255; lower = more sensitive).
    pub threshold: i32,
    /// Fewest grid cells of movement for something to count.
    pub min_cells: usize,
    /// Wanted length of the video, in seconds.
    pub target_secs: u32,
}

impl Default for Params {
    fn default() -> Self {
        Params { threshold: 28, min_cells: 3, target_secs: 120 }
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

struct Decoder {
    child: Child,
    out: std::process::ChildStdout,
}

impl Decoder {
    fn open(ffmpeg: &str, s: &Source, fps: &str, keyframes: bool) -> Result<Decoder> {
        let vf = format!("fps={fps},scale={W}:{H}:force_original_aspect_ratio=decrease,pad={W}:{H}:(ow-iw)/2:(oh-ih)/2:black");
        let mut c = low_cmd(ffmpeg);
        c.args(["-v", "error", "-nostdin"]);
        if keyframes {
            c.args(["-skip_frame", "nokey"]);
        }
        c.args(["-ss", &format!("{:.3}", s.skip), "-t", &format!("{:.3}", s.dur), "-i"]).arg(&s.path);
        c.args(["-an", "-vf", &vf, "-pix_fmt", "rgb24", "-f", "rawvideo", "-"]);
        c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
        let mut child = c.spawn().context("could not start ffmpeg")?;
        let out = child.stdout.take().context("no ffmpeg output")?;
        Ok(Decoder { child, out })
    }

    /// The next picture into `buf`; false at the end.
    fn next(&mut self, buf: &mut [u8]) -> bool {
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
const SAMPLE_EVERY: &str = "1/240:round=up";

/// The empty scene for each hour of `[from, ..)` (None where no footage), and
/// the one shown in the video: the hour with the middle brightness.
fn backgrounds(sources: &[Source], from: DateTime<Local>, ffmpeg: &str, pr: &Progress) -> Result<(Vec<Option<Vec<u8>>>, Vec<u8>)> {
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
            k += 240.0;
            let b = (((at - from).num_seconds() / BLOCK_SECS).max(0) as usize).min(nblocks - 1);
            seen[b] += 1;
            if seen[b] % strides[b] != 0 {
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
        if self.n % ANALYSIS_FPS as u64 == 0 {
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

/// The cut-out of a moving area: its pixels with a soft edge (RGBA).
pub fn cutout(full: &[u8], fg: &[u8], b: &Blob) -> Vec<u8> {
    let (w, h) = (b.x1 - b.x0, b.y1 - b.y0);
    let mut a = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (((b.x0 + x) / 2).min(DW - 1), ((b.y0 + y) / 2).min(DH - 1));
            a[y * w + x] = fg[fy * DW + fx] * 255;
        }
    }
    let a = dilate(&a, w, h, 3);
    // Soften the edge (3×3 average).
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0u32;
            let mut c = 0u32;
            for yy in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                for xx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    s += a[yy * w + xx] as u32;
                    c += 1;
                }
            }
            let src = ((b.y0 + y) * W + b.x0 + x) * 3;
            let d = (y * w + x) * 4;
            rgba[d..d + 3].copy_from_slice(&full[src..src + 3]);
            rgba[d + 3] = (s / c) as u8;
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

pub struct Tube {
    /// Unix milliseconds of the first picture.
    pub start_ms: i64,
    pub frames: Vec<TFrame>,
    dead: bool,
}

impl Tube {
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
    fn mean_w(&self) -> f32 {
        self.frames.iter().map(|f| f.w as f32).sum::<f32>() / self.frames.len().max(1) as f32
    }
    fn bytes(&self) -> usize {
        self.frames.iter().map(|f| f.rgba.len()).sum()
    }
}

/// Joins moving areas from picture to picture into tubes.
pub struct Tracker {
    active: Vec<Tube>,
    pub done: Vec<Tube>,
    pub bytes: usize,
}

impl Tracker {
    pub fn new() -> Tracker {
        Tracker { active: Vec::new(), done: Vec::new(), bytes: 0 }
    }

    fn finish(&mut self, t: Tube) {
        if t.dead || t.frames.len() < MIN_TUBE_FRAMES || t.travel() < (0.4 * t.mean_w()).max(24.0) {
            self.bytes = self.bytes.saturating_sub(t.bytes());
            return;
        }
        self.done.push(t);
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
            if t.dead {
                continue;
            }
            let rgba = cut(b);
            self.bytes += rgba.len();
            let rel = (ms - t.start_ms) as u32;
            t.frames.push(TFrame { rel_ms: rel, x: b.x0 as u16, y: b.y0 as u16, w: (b.x1 - b.x0) as u16, h: (b.y1 - b.y0) as u16, rgba });
            // Still after 10 s: scenery (a flag, a shrub), not someone passing.
            if t.frames.len() == 40 && t.travel() < 12.0 {
                self.bytes = self.bytes.saturating_sub(t.bytes());
                t.frames.clear();
                t.frames.shrink_to_fit();
                t.dead = true;
                // (kept as a placeholder frame so `last()` works)
                t.frames.push(TFrame { rel_ms: rel, x: b.x0 as u16, y: b.y0 as u16, w: (b.x1 - b.x0) as u16, h: (b.y1 - b.y0) as u16, rgba: Vec::new() });
            }
        }
        for (bi, b) in blobs.iter().enumerate() {
            if used_b[bi] {
                continue;
            }
            let rgba = cut(b);
            self.bytes += rgba.len();
            self.active.push(Tube {
                start_ms: ms,
                frames: vec![TFrame { rel_ms: 0, x: b.x0 as u16, y: b.y0 as u16, w: (b.x1 - b.x0) as u16, h: (b.y1 - b.y0) as u16, rgba }],
                dead: false,
            });
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

/// Start picture of each tube (in the given order) and the total length: each
/// as early as it fits (not before the one before it), allowing `tol` cells
/// of overlap per picture on average.
pub fn schedule(foots: &[Vec<Foot>], tol: f64) -> (Vec<usize>, usize) {
    let mut occ: Vec<Foot> = Vec::new();
    let mut starts = Vec::with_capacity(foots.len());
    let mut lo = 0usize;
    for foot in foots {
        let allow = (tol * foot.len() as f64) as u32;
        let mut o = lo;
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
        lo = o;
    }
    (starts, occ.len())
}

pub struct Plan {
    pub speed: f64,
    pub starts: Vec<usize>,
    pub len: usize,
}

/// Fit the tubes into `target` pictures: first without overlap, then with a
/// little, then faster.
pub fn plan(tubes: &[Tube], target: usize) -> Plan {
    let mut best: Option<Plan> = None;
    for &speed in &[1.0, 1.5, 2.0, 3.0, 4.0] {
        let foots: Vec<Vec<Foot>> = tubes.iter().map(|t| footprint(t, speed)).collect();
        for &tol in &[0.0, 0.15, 0.4, 1.0] {
            let (starts, len) = schedule(&foots, tol);
            if len <= target {
                return Plan { speed, starts, len };
            }
            best = Some(Plan { speed, starts, len });
        }
    }
    best.unwrap_or(Plan { speed: 1.0, starts: Vec::new(), len: 0 })
}

// ---------- the video ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectMeta {
    pub id: usize,
    /// When it was really there (RFC 3339, local time).
    pub at: String,
    pub end: String,
    pub out_start: usize,
    pub out_end: usize,
    /// `[output picture, x, y, w, h]` (picture pixels), for finding it by click.
    pub boxes: Vec<[u32; 5]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meta {
    pub camera: String,
    pub day: String,
    pub from: String,
    pub to: String,
    pub built_at: String,
    pub w: usize,
    pub h: usize,
    pub fps: f64,
    pub frames: usize,
    pub seconds: f64,
    pub footage_seconds: f64,
    pub speed: f64,
    pub objects: Vec<ObjectMeta>,
    pub notes: Vec<String>,
}

fn blit(dst: &mut [u8], f: &TFrame) {
    let (w, h) = (f.w as usize, f.h as usize);
    for y in 0..h {
        let dy = f.y as usize + y;
        if dy >= H {
            break;
        }
        for x in 0..w {
            let dx = f.x as usize + x;
            if dx >= W {
                break;
            }
            let s = (y * w + x) * 4;
            let a = f.rgba[s + 3] as u32;
            if a == 0 {
                continue;
            }
            let d = (dy * W + dx) * 3;
            for c in 0..3 {
                dst[d + c] = ((f.rgba[s + c] as u32 * a + dst[d + c] as u32 * (255 - a)) / 255) as u8;
            }
        }
    }
}

fn outline(dst: &mut [u8], f: &TFrame, colour: [u8; 3]) {
    let (x0, y0) = (f.x as usize, f.y as usize);
    let (x1, y1) = ((x0 + f.w as usize).min(W) - 1, (y0 + f.h as usize).min(H) - 1);
    let mut put = |x: usize, y: usize| dst[(y * W + x) * 3..(y * W + x) * 3 + 3].copy_from_slice(&colour);
    for x in x0..=x1 {
        put(x, y0);
        put(x, y1);
    }
    for y in y0..=y1 {
        put(x0, y);
        put(x1, y);
    }
}

const COLOURS: [[u8; 3]; 6] = [[239, 68, 68], [34, 197, 94], [59, 130, 246], [234, 179, 8], [168, 85, 247], [20, 184, 166]];

fn clock(ms: i64) -> String {
    Local.timestamp_millis_opt(ms).single().map(|t| t.format("%H:%M:%S").to_string()).unwrap_or_default()
}

/// The video frames: the scene with each tube laid over it; written to `sink`.
pub fn render(bg: &[u8], tubes: &[Tube], plan: &Plan, sink: &mut dyn FnMut(&[u8]) -> Result<()>, pr: &Progress) -> Result<(usize, Vec<ObjectMeta>)> {
    let step = 1000.0 / OUT_FPS * plan.speed;
    // At least two seconds, so an empty day still shows its scene.
    let total = (plan.len + OUT_FPS as usize).max(OUT_FPS as usize * 2);
    let mut metas: Vec<ObjectMeta> = tubes.iter().enumerate().map(|(i, t)| ObjectMeta {
        id: i,
        at: Local.timestamp_millis_opt(t.start_ms).single().map(|d| d.to_rfc3339()).unwrap_or_default(),
        end: Local.timestamp_millis_opt(t.start_ms + t.last().rel_ms as i64).single().map(|d| d.to_rfc3339()).unwrap_or_default(),
        out_start: plan.starts[i],
        out_end: plan.starts[i] + out_len(t, plan.speed) - 1,
        boxes: Vec::new(),
    }).collect();
    let mut frame = vec![0u8; bg.len()];
    for f in 0..total {
        if pr.cancelled() {
            bail!("cancelled");
        }
        if f % 24 == 0 {
            pr.set("Making the video", 88 + (f * 12 / total) as u32);
        }
        frame.copy_from_slice(bg);
        for (i, t) in tubes.iter().enumerate() {
            let (s, e) = (plan.starts[i], plan.starts[i] + out_len(t, plan.speed));
            if f < s || f >= e {
                continue;
            }
            let fr = &t.frames[frame_at(t, (f - s) as f64 * step)];
            blit(&mut frame, fr);
            outline(&mut frame, fr, COLOURS[i % COLOURS.len()]);
            let label = clock(t.start_ms + fr.rel_ms as i64);
            let ly = if fr.y >= 12 { fr.y as i32 - 12 } else { (fr.y + fr.h) as i32 + 1 };
            crate::draw::caption(&mut frame, W as u32, H as u32, fr.x as i32, ly, &label, 1);
            if (f - s) % (OUT_FPS as usize / 4).max(1) == 0 {
                metas[i].boxes.push([f as u32, fr.x as u32, fr.y as u32, fr.w as u32, fr.h as u32]);
            }
        }
        sink(&frame)?;
    }
    Ok((total, metas))
}

fn encode_to(ffmpeg: &str, out: &Path, run: impl FnOnce(&mut dyn FnMut(&[u8]) -> Result<()>) -> Result<()>) -> Result<()> {
    let tmp = out.with_extension("part.mp4");
    let mut c = low_cmd(ffmpeg);
    c.args(["-y", "-v", "error", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", &format!("{W}x{H}"), "-r", &OUT_FPS.to_string(), "-i", "-"]);
    c.args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "25", "-pix_fmt", "yuv420p", "-movflags", "+faststart"]).arg(&tmp);
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

// ---------- the whole job ----------

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

/// Make the summary of `[from, to)` from these files into `out` (+ `out.json`).
pub fn make(camera: &str, sources: &[Source], from: DateTime<Local>, to: DateTime<Local>, ffmpeg: &str, out: &Path, p: &Params, pr: &Progress) -> Result<Meta> {
    if sources.is_empty() {
        bail!("there is no footage for that day");
    }
    let total_secs: f64 = sources.iter().map(|s| s.dur).sum();
    let (bgs, show) = backgrounds(sources, from, ffmpeg, pr)?;
    // Movement.
    let mut det = Detector::new(p);
    let mut tr = Tracker::new();
    let mut notes = Vec::new();
    let mut buf = vec![0u8; W * H * 3];
    let mut done_secs = 0.0;
    let mut block = usize::MAX;
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
            let b = (((ms - from.timestamp_millis()) / (BLOCK_SECS * 1000)).max(0) as usize).min(bgs.len() - 1);
            if b != block || k == 1 {
                det.seed(nearest_bg(&bgs, b));
                block = b;
            }
            if let Some(blobs) = det.step(&buf) {
                let fg = det.fg.clone();
                tr.feed(ms, &blobs, &mut |bl| cutout(&buf, &fg, bl));
            }
            if tr.bytes > MAX_CROP_BYTES {
                notes.push(format!("Stopped looking at {}: too many moving objects to hold in memory. Raise the sensitivity threshold to see fewer.", clock(ms)));
                tr.flush();
                break 'sources;
            }
            if k % 40 == 0 && pr.cancelled() {
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
    let plan = plan(&tubes, (p.target_secs as f64 * OUT_FPS) as usize);
    if plan.speed > 1.0 {
        notes.push(format!("Played {}× faster to fit {} s.", plan.speed, p.target_secs));
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
    let (frames, objects) = result.context("nothing was drawn")?;
    let meta = Meta {
        camera: camera.to_string(),
        day: from.format("%Y-%m-%d").to_string(),
        from: from.to_rfc3339(),
        to: to.to_rfc3339(),
        built_at: Local::now().to_rfc3339(),
        w: W,
        h: H,
        fps: OUT_FPS,
        frames,
        seconds: frames as f64 / OUT_FPS,
        footage_seconds: total_secs,
        speed: plan.speed,
        objects,
        notes,
    };
    std::fs::write(out.with_extension("json"), serde_json::to_vec(&meta)?)?;
    pr.set("Done", 100);
    Ok(meta)
}

// ---------- on request (web) ----------

enum State {
    Running,
    Done,
    Failed(String),
}

struct Job {
    pr: Arc<Progress>,
    state: State,
}

static JOBS: LazyLock<Mutex<HashMap<String, Job>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static BUSY: AtomicUsize = AtomicUsize::new(0);

fn key(camera: &str, day: &str) -> String {
    format!("{camera}|{day}")
}

pub fn dir_for(mgr: &Manager, camera: &str) -> Option<PathBuf> {
    storage::valid_folder(camera).then(|| mgr.data_dir.join("synopsis").join(camera))
}

pub fn video_path(mgr: &Manager, camera: &str, day: NaiveDate) -> Option<PathBuf> {
    let p = dir_for(mgr, camera)?.join(format!("{day}.mp4"));
    p.is_file().then_some(p)
}

fn day_range(day: NaiveDate) -> Option<(DateTime<Local>, DateTime<Local>)> {
    let from = Local.from_local_datetime(&day.and_hms_opt(0, 0, 0)?).earliest()?;
    let next = Local.from_local_datetime(&day.succ_opt()?.and_hms_opt(0, 0, 0)?).earliest()?;
    Some((from, next.min(Local::now())))
}

fn tidy(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|m| m.elapsed().ok()).is_some_and(|a| a.as_secs() > KEEP_DAYS * 86400);
        if old && e.path().extension().is_some_and(|x| x == "mp4" || x == "json") {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Start making a camera's summary of a day (in a thread); one at a time.
pub fn start(mgr: &Arc<Manager>, camera: &str, day: NaiveDate, params: Params, force: bool) -> Result<()> {
    let rt = mgr.camera(camera).context("camera not found")?;
    let dir = dir_for(mgr, camera).context("this camera's name can't be used as a folder")?;
    let k = key(camera, &day.to_string());
    if !force && day < Local::now().date_naive() && dir.join(format!("{day}.json")).is_file() {
        return Ok(());
    }
    let (from, to) = day_range(day).context("that day can't be used")?;
    if to <= from {
        bail!("that day has not started yet");
    }
    {
        let mut jobs = JOBS.locked();
        if BUSY.load(Ordering::SeqCst) > 0 {
            bail!("another day summary is being made; try again when it is done");
        }
        BUSY.fetch_add(1, Ordering::SeqCst);
        jobs.insert(k.clone(), Job { pr: Arc::new(Progress::new()), state: State::Running });
    }
    let pr = JOBS.locked().get(&k).map(|j| j.pr.clone()).unwrap();
    let ffmpeg = mgr.config().server.ffmpeg_path.clone();
    let id = camera.to_string();
    std::thread::spawn(move || {
        // However this ends (even a panic), the next summary may start.
        struct Free;
        impl Drop for Free {
            fn drop(&mut self) {
                BUSY.fetch_sub(1, Ordering::SeqCst);
            }
        }
        let _free = Free;
        tidy(&dir);
        let sources = pick_sources(&rt, from, to);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| make(&id, &sources, from, to, &ffmpeg, &dir.join(format!("{day}.mp4")), &params, &pr)))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("the summary stopped unexpectedly")));
        let mut jobs = JOBS.locked();
        if let Some(j) = jobs.get_mut(&k) {
            j.state = match r {
                Ok(_) => State::Done,
                Err(e) => {
                    tracing::warn!("day summary {k}: {e:#}");
                    State::Failed(format!("{e:#}"))
                }
            };
        }
    });
    Ok(())
}

pub fn cancel(camera: &str, day: NaiveDate) {
    if let Some(j) = JOBS.locked().get(&key(camera, &day.to_string())) {
        j.pr.cancel.store(true, Ordering::Relaxed);
    }
}

/// What the page shows: progress, or the finished summary's details.
pub fn status(mgr: &Manager, camera: &str, day: NaiveDate) -> serde_json::Value {
    use serde_json::json;
    let k = key(camera, &day.to_string());
    if let Some(j) = JOBS.locked().get(&k) {
        match &j.state {
            State::Running => return json!({ "state": "running", "phase": j.pr.phase.locked().clone(), "pct": j.pr.pct.load(Ordering::Relaxed) }),
            State::Failed(e) => return json!({ "state": "failed", "error": e }),
            State::Done => {}
        }
    }
    let meta = dir_for(mgr, camera).and_then(|d| std::fs::read(d.join(format!("{day}.json"))).ok()).and_then(|b| serde_json::from_slice::<Meta>(&b).ok());
    match meta {
        Some(m) => json!({ "state": "done", "meta": m }),
        None => json!({ "state": "none" }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// A tube of an object walking left to right over `secs` seconds from `ms0`.
    fn walker(ms0: i64, secs: f64, y: usize, x_start: usize) -> Tube {
        let n = (secs * ANALYSIS_FPS) as usize;
        let frames = (0..n)
            .map(|i| TFrame { rel_ms: (i as f64 * 1000.0 / ANALYSIS_FPS) as u32, x: (x_start + i * 10) as u16, y: y as u16, w: 40, h: 80, rgba: vec![255; 40 * 80 * 4] })
            .collect();
        Tube { start_ms: ms0, frames, dead: false }
    }

    #[test]
    fn a_walk_becomes_one_tube_and_scenery_that_stays_put_does_not() {
        let bg = solid(90, 120, 60);
        let mut d = Detector::new(&Params::default());
        d.seed(&bg);
        let mut tr = Tracker::new();
        // 12 s of someone walking right (10 px per picture) while a flag flaps in place.
        for i in 0..48usize {
            let f = with_box(&with_box(&bg, 30 + i * 10, 200, 30, 70, [220, 40, 40]), 560, 20 + (i % 2) * 4, 30, 40, [250, 250, 0]);
            let blobs = d.step(&f).unwrap();
            let fg = d.fg.clone();
            tr.feed(i as i64 * 250, &blobs, &mut |b| cutout(&f, &fg, b));
        }
        tr.flush();
        assert_eq!(tr.done.len(), 1, "one walk expected, got {}", tr.done.len());
        let t = &tr.done[0];
        assert!(t.frames.len() >= 40 && t.travel() > 300.0);
    }

    #[test]
    fn objects_from_different_times_share_the_timeline_without_covering_each_other() {
        // Three people crossing at the same height at 8:00, 12:00 and 17:00 (10 s each).
        let tubes = vec![walker(0, 10.0, 100, 0), walker(4 * 3_600_000, 10.0, 100, 0), walker(9 * 3_600_000, 10.0, 100, 0)];
        let foots: Vec<Vec<Foot>> = tubes.iter().map(|t| footprint(t, 1.0)).collect();
        let (starts, len) = schedule(&foots, 0.0);
        // Same place: they can't all play at once, but each starts only after the
        // one before has moved on (so it is shorter than three walks end to end).
        assert_eq!(starts[0], 0);
        assert!(starts[1] > 0 && starts[2] >= starts[1], "{starts:?}");
        assert!(len < 3 * foots[0].len(), "no saving: {len}");
        // Somewhere else on the picture they can play together.
        let tubes = vec![walker(0, 10.0, 20, 0), walker(1000, 10.0, 280, 0)];
        let foots: Vec<Vec<Foot>> = tubes.iter().map(|t| footprint(t, 1.0)).collect();
        let (starts, len) = schedule(&foots, 0.0);
        assert_eq!(starts, vec![0, 0]);
        assert_eq!(len, foots[0].len());
    }

    #[test]
    fn a_long_busy_day_is_sped_up_to_the_wanted_length() {
        // 60 walks across the same row: 60 × 10 s can't fit in 60 s without help.
        let tubes: Vec<Tube> = (0..60).map(|i| walker(i * 600_000, 10.0, 100, 0)).collect();
        let p = plan(&tubes, 60 * OUT_FPS as usize);
        assert!(p.len <= 60 * OUT_FPS as usize || p.speed >= 4.0, "len {} speed {}", p.len, p.speed);
        assert!(p.speed > 1.0);
    }

    #[test]
    fn the_video_has_the_scene_with_objects_and_their_clock_times() {
        let bg = solid(30, 30, 30);
        let tubes = vec![walker(Local::now().timestamp_millis(), 5.0, 100, 0)];
        let pl = plan(&tubes, 10 * OUT_FPS as usize);
        let mut frames = 0;
        let mut painted = false;
        let pr = Progress::new();
        let (n, metas) = render(&bg, &tubes, &pl, &mut |f| {
            frames += 1;
            painted |= f != &bg[..];
            Ok(())
        }, &pr).unwrap();
        assert_eq!(n, frames);
        assert!(painted, "the object was not drawn");
        assert_eq!(metas.len(), 1);
        assert!(!metas[0].boxes.is_empty() && metas[0].out_end >= metas[0].out_start);
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
        // 30 s of grey with a red box crossing in 10 s, twice.
        let st = std::process::Command::new(ff)
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "color=c=0x606060:s=640x360:r=10:d=30", "-f", "lavfi", "-i", "color=c=red:s=50x90:r=10:d=30", "-filter_complex", "[0][1]overlay=x='if(lt(mod(t,15),10),mod(t,15)*55,-100)':y=140", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-g", "10"])
            .arg(&clip)
            .status()
            .unwrap();
        if !st.success() {
            return; // no libx264 here
        }
        let from = Local::now() - CDuration::minutes(5);
        let src = Source { path: clip, start: from, skip: 0.0, dur: 30.0 };
        let out = dir.join("out").join("day.mp4");
        let m = make("cam", &[src], from, from + CDuration::seconds(30), ff, &out, &Params::default(), &Progress::new()).unwrap();
        assert!(out.is_file() && out.with_extension("json").is_file());
        assert_eq!(m.objects.len(), 2, "two crossings expected: {:?}", m.objects.iter().map(|o| (&o.at, o.out_start, o.out_end)).collect::<Vec<_>>());
        assert!(m.seconds >= 2.0);
        // The second crossing started 15 s after the first in real time, and
        // plays together with / right after it in the summary.
        assert!(m.objects[1].out_start <= m.objects[0].out_end + 1 + 12 * 15);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
