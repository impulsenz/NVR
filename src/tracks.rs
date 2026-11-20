//! Following people (Plan page): every time the AI looks at a camera's
//! picture, the people it finds are joined, look after look, into a **walk** on
//! that camera; walks on different cameras that fit together in time,
//! distance and (weakly) clothing colour are joined into a **journey**, which
//! the plan draws as a path.
//!
//! What is kept is a list of positions and times for each walk (and a handful
//! of colour numbers), never a picture. It is off until switched on in the
//! plan's settings, kept for the number of days set there, and can be deleted
//! at any time. Recording, detection and alerts never wait for it: the
//! detector only hands over what it found, and a problem here is dropped.
//!
//! Pieces, each testable on its own: `Tracker` (one camera: looks → walks),
//! `signature` (clothing colour), `link` (walks → journeys), `Store` (files).
#![allow(clippy::needless_range_loop)]  // small fixed arrays (colour bands, signatures): indexed loops read like the maths

use crate::plan::{Plan, Tuning};
use crate::sync::MutexExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};

/// A person's place at one look: when (ms since 1970), where their feet are
/// (fractions of the picture) and how big the box was.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Point(pub i64, pub f32, pub f32, pub f32, pub f32);

impl Point {
    pub fn t(&self) -> i64 {
        self.0
    }
}

/// Numbers for clothing colour: for the top, middle and bottom of a person,
/// the red and green share and how bright it is compared with the rest.
pub type Sig = [f32; 9];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Track {
    pub id: String,
    pub cam: String,
    pub label: String,
    pub pts: Vec<Point>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sig: Option<Sig>,
}

impl Track {
    pub fn start(&self) -> i64 {
        self.pts.first().map(Point::t).unwrap_or(0)
    }
    pub fn end(&self) -> i64 {
        self.pts.last().map(Point::t).unwrap_or(0)
    }
    pub fn seconds(&self) -> f32 {
        (self.end() - self.start()) as f32 / 1000.0
    }
}

/// What the AI found at one look.
#[derive(Debug, Clone)]
pub struct Obs {
    pub label: String,
    pub score: f32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub sig: Option<Sig>,
}

impl Obs {
    fn foot(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, (self.y + self.h).min(1.0))
    }
}

// ---------- clothing colour ----------

/// Colour numbers for a person's box in a picture (None when the box is too
/// small to say anything).
pub fn signature(f: &crate::imgutil::Frame, x: f32, y: f32, w: f32, h: f32) -> Option<Sig> {
    let (fw, fh) = (f.width as f32, f.height as f32);
    if w * fw < 12.0 || h * fh < 36.0 {
        return None;
    }
    // The middle of the box: the edges are mostly background.
    let (x0, x1) = (((x + w * 0.2) * fw) as usize, ((x + w * 0.8) * fw) as usize);
    let mut bands = [[0f64; 3]; 3];
    let mut count = [0f64; 3];
    for (b, (a, z)) in [(0.05f32, 0.33f32), (0.33, 0.66), (0.66, 0.95)].into_iter().enumerate() {
        let (y0, y1) = (((y + h * a) * fh) as usize, ((y + h * z) * fh) as usize);
        for yy in y0..y1.min(f.height as usize) {
            for xx in x0..x1.min(f.width as usize) {
                let p = (yy * f.width as usize + xx) * 3;
                for c in 0..3 {
                    bands[b][c] += f.rgb[p + c] as f64;
                }
                count[b] += 1.0;
            }
        }
    }
    if count.iter().any(|c| *c < 4.0) {
        return None;
    }
    let mut luma = [0f64; 3];
    let mut out = [0f32; 9];
    for b in 0..3 {
        for c in 0..3 {
            bands[b][c] /= count[b] * 255.0;
        }
        luma[b] = (0.3 * bands[b][0] + 0.59 * bands[b][1] + 0.11 * bands[b][2]).max(1e-3);
    }
    let mean = (luma[0] + luma[1] + luma[2]) / 3.0;
    for b in 0..3 {
        let sum = (bands[b][0] + bands[b][1] + bands[b][2]).max(1e-3);
        out[3 * b] = (bands[b][0] / sum) as f32;
        out[3 * b + 1] = (bands[b][1] / sum) as f32;
        out[3 * b + 2] = (luma[b] / mean / 2.0).min(1.0) as f32;
    }
    Some(out)
}

/// How alike two colour signatures are (0 = nothing like, 1 = the same).
pub fn similarity(a: &Sig, b: &Sig) -> f32 {
    let d: f32 = a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum::<f32>().sqrt();
    // Two unrelated outfits are about 0.5 apart; the same one under another
    // light about 0.1.
    (1.0 - d / 0.6).clamp(0.0, 1.0)
}

// ---------- one camera: looks → walks ----------

struct Open {
    id: String,
    label: String,
    pts: Vec<Point>,
    sig_sum: [f32; 9],
    sig_n: u32,
}

impl Open {
    fn last(&self) -> Point {
        *self.pts.last().unwrap()
    }

    fn finish(self, cam: &str, t: &Tuning) -> Option<Track> {
        let seconds = (self.pts.last()?.t() - self.pts.first()?.t()) as f32 / 1000.0;
        if self.pts.len() < 2 || seconds < t.min_track_seconds {
            return None;
        }
        let sig = (self.sig_n > 0).then(|| self.sig_sum.map(|v| v / self.sig_n as f32));
        Some(Track { id: self.id, cam: cam.to_string(), label: self.label, pts: self.pts, sig })
    }
}

/// The walks being followed on one camera.
#[derive(Default)]
pub struct Tracker {
    next: u64,
    open: Vec<Open>,
}

/// The most points kept for one walk (a long one is thinned, not cut).
const MAX_POINTS: usize = 400;

impl Tracker {
    /// One look at the picture at `t_ms`. Returns walks that ended.
    pub fn update(&mut self, cam: &str, t_ms: i64, obs: &[Obs], tuning: &Tuning) -> Vec<Track> {
        let mut done = self.flush(cam, t_ms, tuning);
        // Every plausible pairing, nearest first, each walk and each person used once.
        let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
        let wanted = |o: &Obs| o.score >= tuning.min_score && tuning.labels.contains(&o.label);
        for (i, o) in obs.iter().enumerate().filter(|(_, o)| wanted(o)) {
            let (fx, fy) = o.foot();
            for (j, tr) in self.open.iter().enumerate().filter(|(_, tr)| tr.label == o.label) {
                let last = tr.last();
                let gap = ((t_ms - last.t()) as f32 / 1000.0).max(0.0);
                // The longer since the last look, the further they may have gone.
                let reach = tuning.match_radius * gap.max(1.0).sqrt();
                let d = ((fx - last.1).powi(2) + (fy - last.2).powi(2)).sqrt();
                if d <= reach {
                    pairs.push((d, i, j));
                }
            }
        }
        pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let (mut seen_o, mut seen_t) = (vec![false; obs.len()], vec![false; self.open.len()]);
        for (_, i, j) in pairs {
            if seen_o[i] || seen_t[j] {
                continue;
            }
            seen_o[i] = true;
            seen_t[j] = true;
            let o = &obs[i];
            let (fx, fy) = o.foot();
            let tr = &mut self.open[j];
            tr.pts.push(Point(t_ms, fx, fy, o.w, o.h));
            if tr.pts.len() > MAX_POINTS {
                // Thin to every second point.
                let kept: Vec<Point> = tr.pts.iter().copied().step_by(2).collect();
                tr.pts = kept;
            }
            if let Some(s) = &o.sig {
                for k in 0..9 {
                    tr.sig_sum[k] += s[k];
                }
                tr.sig_n += 1;
            }
        }
        for (i, o) in obs.iter().enumerate().filter(|(_, o)| wanted(o)) {
            if seen_o[i] {
                continue;
            }
            self.next += 1;
            let (fx, fy) = o.foot();
            let mut sig_sum = [0f32; 9];
            if let Some(s) = &o.sig {
                sig_sum = *s;
            }
            self.open.push(Open {
                id: format!("{}-{}-{}", cam, t_ms, self.next),
                label: o.label.clone(),
                pts: vec![Point(t_ms, fx, fy, o.w, o.h)],
                sig_sum,
                sig_n: o.sig.is_some() as u32,
            });
        }
        done.shrink_to_fit();
        done
    }

    /// Close the walks not seen for the set time as of `now_ms`.
    pub fn flush(&mut self, cam: &str, now_ms: i64, tuning: &Tuning) -> Vec<Track> {
        let gap = (tuning.track_gap_seconds * 1000.0) as i64;
        let (stale, keep): (Vec<Open>, Vec<Open>) = std::mem::take(&mut self.open).into_iter().partition(|o| now_ms - o.last().t() > gap);
        self.open = keep;
        stale.into_iter().filter_map(|o| o.finish(cam, tuning)).collect()
    }

    /// The latest place of every walk still going (the live dots).
    pub fn live(&self, now_ms: i64, within_ms: i64) -> Vec<(String, String, Point)> {
        self.open.iter().filter(|o| now_ms - o.last().t() <= within_ms).map(|o| (o.id.clone(), o.label.clone(), o.last())).collect()
    }
}

// ---------- journeys: walks across cameras ----------

/// A walk's path on the plan (fractions of the plan), smoothed.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Segment {
    pub track: String,
    pub cam: String,
    pub start: i64,
    pub end: i64,
    /// Times (ms) of each point, same length as `path` (empty without calibration).
    pub times: Vec<i64>,
    /// Where on the plan (empty when the camera isn't calibrated).
    pub path: Vec<[f64; 2]>,
    pub metres: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Journey {
    pub id: String,
    pub start: i64,
    pub end: i64,
    pub seconds: f64,
    pub cameras: Vec<String>,
    pub segments: Vec<Segment>,
    /// How sure the joins are (the lowest join; 1 for a walk on one camera).
    pub confidence: f32,
    /// Walked distance, where the plan has a scale and calibrated cameras.
    pub metres: f64,
    /// Average speed while followed (metres per second), when it can be known.
    pub speed_mps: Option<f64>,
    pub label: String,
}

fn smooth(path: &mut [[f64; 2]], amount: f32) {
    let a = (amount as f64 * 0.8).clamp(0.0, 0.95);
    for i in 1..path.len() {
        for k in 0..2 {
            path[i][k] = a * path[i - 1][k] + (1.0 - a) * path[i][k];
        }
    }
}

struct Placed<'a> {
    t: &'a Track,
    seg: Segment,
}

fn place<'a>(t: &'a Track, plan: &Plan) -> Placed<'a> {
    let h = plan.homography_for(&t.cam);
    let mut path: Vec<[f64; 2]> = Vec::new();
    let mut times: Vec<i64> = Vec::new();
    if let Some(h) = &h {
        for p in &t.pts {
            if let Some((x, y)) = crate::geo::apply(h, (p.1 as f64, p.2 as f64)) {
                path.push([x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)]);
                times.push(p.t());
            }
        }
        smooth(&mut path, plan.tuning.smoothing);
    }
    let metres: f64 = path.windows(2).filter_map(|w| plan.metres((w[0][0], w[0][1]), (w[1][0], w[1][1]))).sum();
    Placed { t, seg: Segment { track: t.id.clone(), cam: t.cam.clone(), start: t.start(), end: t.end(), times, path, metres } }
}

/// How well a walk that ended (`a`) continues as one that began (`b`) on
/// another camera. None when it can't be (too late, too far, not neighbours).
fn join_score(a: &Placed, b: &Placed, plan: &Plan) -> Option<f32> {
    let t = &plan.tuning;
    if a.t.cam == b.t.cam || a.t.label != b.t.label || b.t.start() <= a.t.start() {
        return None;
    }
    let gap = (b.t.start() - a.t.end()) as f32 / 1000.0;
    if gap < -t.overlap_seconds || gap > t.max_gap_seconds {
        return None;
    }
    let link = plan.linked(&a.t.cam, &b.t.cam);
    if !plan.links.is_empty() && !t.link_all_pairs && link.is_none() {
        return None;
    }
    let gap_pos = gap.max(0.0);
    let mut parts: Vec<(f32, f32)> = Vec::new(); // (weight, score)
    // Timing: close to the usual walking time (if given), else sooner is better.
    let time_s = match link.and_then(|l| l.travel_seconds) {
        Some(travel) => {
            let sigma = (travel * 0.5).max(2.0);
            (-0.5 * ((gap_pos - travel) / sigma).powi(2)).exp()
        }
        None => (-gap_pos / (t.max_gap_seconds / 2.0).max(0.5)).exp(),
    };
    parts.push((t.time_weight, time_s));
    // Distance: from where they were last seen to where they appear.
    if let (Some(p), Some(q)) = (a.seg.path.last(), b.seg.path.first())
        && let Some(d) = plan.metres((p[0], p[1]), (q[0], q[1]))
    {
        let allowed = (t.max_speed_mps * gap_pos + t.slack_m) as f64;
        if d > allowed {
            return None;
        }
        parts.push((t.distance_weight, (1.0 - d / allowed.max(0.1)) as f32));
    }
    if let (Some(x), Some(y)) = (&a.t.sig, &b.t.sig) {
        parts.push((t.colour_weight, similarity(x, y)));
    }
    let weight: f32 = parts.iter().map(|p| p.0).sum();
    if weight <= 0.0 {
        return None;
    }
    let score = parts.iter().map(|p| p.0 * p.1).sum::<f32>() / weight;
    (score >= t.min_link_score).then_some(score)
}

/// Join walks into journeys. Each walk continues into at most one other, the
/// best joins first.
pub fn link(tracks: &[Track], plan: &Plan) -> Vec<Journey> {
    let placed: Vec<Placed> = tracks.iter().map(|t| place(t, plan)).collect();
    let mut edges: Vec<(f32, usize, usize)> = Vec::new();
    for (i, a) in placed.iter().enumerate() {
        for (j, b) in placed.iter().enumerate() {
            if i != j
                && let Some(s) = join_score(a, b, plan)
            {
                edges.push((s, i, j));
            }
        }
    }
    edges.sort_by(|x, y| y.0.total_cmp(&x.0));
    type Links = (Vec<Option<(usize, f32)>>, Vec<Option<usize>>);
    let (mut next, mut prev): Links = (vec![None; placed.len()], vec![None; placed.len()]);
    for (s, i, j) in edges {
        if next[i].is_none() && prev[j].is_none() {
            next[i] = Some((j, s));
            prev[j] = Some(i);
        }
    }
    let mut out: Vec<Journey> = Vec::new();
    for first in (0..placed.len()).filter(|i| prev[*i].is_none()) {
        let (mut chain, mut conf, mut at) = (vec![first], 1.0f32, first);
        while let Some((n, s)) = next[at] {
            chain.push(n);
            conf = conf.min(s);
            at = n;
        }
        let segs: Vec<Segment> = chain.iter().map(|i| placed[*i].seg.clone()).collect();
        let (start, end) = (segs.first().unwrap().start, segs.last().unwrap().end);
        let mut metres: f64 = segs.iter().map(|s| s.metres).sum();
        // The hop between two cameras counts too, when both ends are known.
        for w in segs.windows(2) {
            if let (Some(p), Some(q)) = (w[0].path.last(), w[1].path.first()) {
                metres += plan.metres((p[0], p[1]), (q[0], q[1])).unwrap_or(0.0);
            }
        }
        let seconds = (end - start) as f64 / 1000.0;
        let known = segs.iter().all(|s| !s.path.is_empty()) && plan.px_per_m > 0.0;
        let mut cameras: Vec<String> = Vec::new();
        for s in &segs {
            if cameras.last() != Some(&s.cam) {
                cameras.push(s.cam.clone());
            }
        }
        out.push(Journey {
            id: placed[first].t.id.clone(),
            start,
            end,
            seconds,
            cameras,
            confidence: conf,
            metres,
            speed_mps: (known && seconds > 0.5).then(|| metres / seconds),
            label: placed[first].t.label.clone(),
            segments: segs,
        });
    }
    out.sort_by_key(|j| j.start);
    out
}

// ---------- numbers ----------

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CamStat {
    pub cam: String,
    pub walks: usize,
    pub avg_seconds: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Stats {
    /// Journeys: about as many as the people who walked (the same person
    /// seen twice with a long gap counts twice).
    pub people: usize,
    pub avg_seconds: f64,
    /// Average walking speed over the journeys where it can be known.
    pub avg_speed_mps: Option<f64>,
    pub cameras: Vec<CamStat>,
    /// Journeys started in each hour of the day (local time).
    pub by_hour: [u32; 24],
}

pub fn stats(journeys: &[Journey], tracks: &[Track]) -> Stats {
    use chrono::Timelike;
    let mut by_hour = [0u32; 24];
    for j in journeys {
        if let Some(d) = chrono::DateTime::from_timestamp_millis(j.start) {
            by_hour[d.with_timezone(&chrono::Local).hour() as usize] += 1;
        }
    }
    let speeds: Vec<f64> = journeys.iter().filter_map(|j| j.speed_mps).collect();
    let mut cams: std::collections::BTreeMap<&str, (usize, f64)> = std::collections::BTreeMap::new();
    for t in tracks {
        let e = cams.entry(t.cam.as_str()).or_default();
        e.0 += 1;
        e.1 += t.seconds() as f64;
    }
    Stats {
        people: journeys.len(),
        avg_seconds: if journeys.is_empty() { 0.0 } else { journeys.iter().map(|j| j.seconds).sum::<f64>() / journeys.len() as f64 },
        avg_speed_mps: (!speeds.is_empty()).then(|| speeds.iter().sum::<f64>() / speeds.len() as f64),
        cameras: cams.into_iter().map(|(c, (n, s))| CamStat { cam: c.to_string(), walks: n, avg_seconds: s / n as f64 }).collect(),
        by_hour,
    }
}

/// Where a point seen by a camera is on the plan (None when it isn't calibrated).
pub fn on_plan(plan: &Plan, cam: &str, p: &Point) -> Option<[f64; 2]> {
    let h = plan.homography_for(cam)?;
    crate::geo::apply(&h, (p.1 as f64, p.2 as f64)).map(|(x, y)| [x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)])
}

// ---------- files ----------

/// Walks on disk: one file per day, one line per walk.
pub struct Store {
    dir: PathBuf,
}

fn day_of(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms).map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string()).unwrap_or_else(|| "unknown".into())
}

impl Store {
    pub fn open(data_dir: &Path) -> Self {
        Self { dir: data_dir.join("tracks") }
    }

    pub fn append(&self, tracks: &[Track]) -> std::io::Result<()> {
        use std::io::Write;
        std::fs::create_dir_all(&self.dir)?;
        let mut by_day: HashMap<String, Vec<&Track>> = HashMap::new();
        for t in tracks {
            by_day.entry(day_of(t.start())).or_default().push(t);
        }
        for (day, list) in by_day {
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(self.dir.join(format!("{day}.jsonl")))?;
            for t in list {
                if let Ok(line) = serde_json::to_string(t) {
                    writeln!(f, "{line}")?;
                }
            }
        }
        Ok(())
    }

    /// Walks that started in `from_ms..to_ms`.
    pub fn read(&self, from_ms: i64, to_ms: i64) -> Vec<Track> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return out };
        let (d0, d1) = (day_of(from_ms - 86_400_000), day_of(to_ms + 86_400_000));
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Some(day) = name.strip_suffix(".jsonl") else { continue };
            if day < d0.as_str() || day > d1.as_str() {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
            out.extend(text.lines().filter_map(|l| serde_json::from_str::<Track>(l).ok()).filter(|t| t.start() >= from_ms && t.start() < to_ms));
        }
        out.sort_by_key(|t| t.start());
        out
    }

    /// Delete days older than `keep_days`. Returns how many files went.
    pub fn prune(&self, keep_days: u32, now_ms: i64) -> usize {
        let limit = day_of(now_ms - keep_days as i64 * 86_400_000);
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return 0 };
        let mut n = 0;
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if let Some(day) = name.strip_suffix(".jsonl")
                && day < limit.as_str()
                && std::fs::remove_file(e.path()).is_ok()
            {
                n += 1;
            }
        }
        n
    }

    /// Delete everything.
    pub fn delete_all(&self) -> usize {
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return 0 };
        rd.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "jsonl") && std::fs::remove_file(e.path()).is_ok()).count()
    }
}

// ---------- the hand-over from the detector ----------

static TUNING: RwLock<Option<Tuning>> = RwLock::new(None);
static TRACKERS: Mutex<Option<HashMap<String, Tracker>>> = Mutex::new(None);
static DONE: Mutex<Vec<Track>> = Mutex::new(Vec::new());
/// Walks waiting to be written never pile up past this (a stuck disk drops them).
const MAX_WAITING: usize = 5000;

pub fn set_tuning(t: Tuning) {
    *TUNING.write().unwrap_or_else(|e| e.into_inner()) = Some(t);
}

fn tuning() -> Option<Tuning> {
    TUNING.read().unwrap_or_else(|e| e.into_inner()).clone().filter(|t| t.enabled)
}

pub fn enabled() -> bool {
    tuning().is_some()
}

/// Called by the detector after each AI look. Cheap, never blocks on disk,
/// and a no-op while following people is off.
pub fn observe(cam: &str, now_ms: i64, frame: &crate::imgutil::Frame, found: &[crate::ai::Detection]) {
    let Some(t) = tuning() else { return };
    let obs: Vec<Obs> = found
        .iter()
        .filter(|d| d.score >= t.min_score && t.labels.contains(&d.label))
        .map(|d| Obs { label: d.label.clone(), score: d.score, x: d.x, y: d.y, w: d.w, h: d.h, sig: signature(frame, d.x, d.y, d.w, d.h) })
        .collect();
    let mut all = TRACKERS.locked();
    let done = all.get_or_insert_with(HashMap::new).entry(cam.to_string()).or_default().update(cam, now_ms, &obs, &t);
    drop(all);
    if !done.is_empty() {
        let mut q = DONE.locked();
        if q.len() < MAX_WAITING {
            q.extend(done);
        }
    }
}

/// Close walks that have gone quiet (also when nothing more is detected) and
/// take the finished ones.
pub fn collect(now_ms: i64) -> Vec<Track> {
    if let Some(t) = TUNING.read().unwrap_or_else(|e| e.into_inner()).clone() {
        let mut all = TRACKERS.locked();
        if let Some(map) = all.as_mut() {
            let mut fresh = Vec::new();
            for (cam, tr) in map.iter_mut() {
                fresh.extend(tr.flush(cam, now_ms, &t));
            }
            drop(all);
            DONE.locked().extend(fresh);
        }
    }
    std::mem::take(&mut *DONE.locked())
}

/// Where people are right now: (camera, walk, label, point).
pub fn live(now_ms: i64) -> Vec<(String, String, String, Point)> {
    let all = TRACKERS.locked();
    let Some(map) = all.as_ref() else { return Vec::new() };
    map.iter().flat_map(|(cam, tr)| tr.live(now_ms, 6000).into_iter().map(move |(id, label, p)| (cam.clone(), id, label, p))).collect()
}

/// Runs for as long as GAZE does: closes quiet walks, writes finished ones to
/// disk, and removes walks older than the days kept.
pub async fn run(mgr: std::sync::Arc<crate::manager::Manager>) {
    use std::time::Duration;
    let mut last_prune = std::time::Instant::now() - Duration::from_secs(7200);
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let now = chrono::Local::now().timestamp_millis();
        let done = collect(now);
        if !done.is_empty() {
            let m = mgr.clone();
            let _ = tokio::task::spawn_blocking(move || {
                if let Err(e) = m.tracks.append(&done) {
                    tracing::warn!("floor plan: couldn't save walks: {e}");
                }
            })
            .await;
        }
        if last_prune.elapsed() > Duration::from_secs(3600) {
            last_prune = std::time::Instant::now();
            let keep = mgr.plan.tuning().keep_days;
            let m = mgr.clone();
            let _ = tokio::task::spawn_blocking(move || m.tracks.prune(keep, now)).await;
        }
    }
}

/// Forget walks in progress (following was switched off, or the plan changed).
pub fn reset() {
    *TRACKERS.locked() = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Calib, Link, PlanCamera};

    fn obs(x: f32, y: f32) -> Obs {
        Obs { label: "person".into(), score: 0.9, x: x - 0.03, y: y - 0.2, w: 0.06, h: 0.2, sig: None }
    }

    fn tuning() -> Tuning {
        Tuning { enabled: true, ..Default::default() }
    }

    // ----- the tracker -----

    #[test]
    fn one_person_walking_across_is_one_walk() {
        let (mut tr, t) = (Tracker::default(), tuning());
        let mut done = Vec::new();
        for i in 0..8 {
            done.extend(tr.update("front", 1000 * i, &[obs(0.1 + 0.1 * i as f32, 0.7)], &t));
        }
        assert!(done.is_empty(), "still walking");
        done.extend(tr.flush("front", 8000 + 10_000, &t));
        assert_eq!(done.len(), 1);
        assert_eq!((done[0].pts.len(), done[0].seconds()), (8, 7.0));
        assert!((done[0].pts[0].1 - 0.1).abs() < 1e-6, "foot = bottom centre");
        assert!((done[0].pts[0].2 - 0.7).abs() < 1e-6);
    }

    #[test]
    fn two_people_passing_keep_their_own_walks() {
        let (mut tr, t) = (Tracker::default(), tuning());
        // One walks right along y = 0.6, the other left along y = 0.8.
        for i in 0..8 {
            let f = i as f32 * 0.1;
            tr.update("c", 1000 * i, &[obs(0.1 + f, 0.6), obs(0.9 - f, 0.8)], &t);
        }
        let mut done = tr.flush("c", 99_000, &t);
        done.sort_by(|a, b| a.pts[0].1.total_cmp(&b.pts[0].1));
        assert_eq!(done.len(), 2);
        assert!(done[0].pts.iter().all(|p| (p.2 - 0.6).abs() < 1e-6), "the first stays on their line");
        assert!(done[1].pts.iter().all(|p| (p.2 - 0.8).abs() < 1e-6));
        assert!(done[0].pts.windows(2).all(|w| w[1].1 > w[0].1) && done[1].pts.windows(2).all(|w| w[1].1 < w[0].1));
    }

    #[test]
    fn flickers_weak_detections_and_other_kinds_are_ignored() {
        let (mut tr, t) = (Tracker::default(), tuning());
        tr.update("c", 0, &[obs(0.5, 0.5)], &t); // seen once, never again
        let weak = Obs { score: 0.3, ..obs(0.2, 0.5) };
        let dog = Obs { label: "dog".into(), ..obs(0.3, 0.5) };
        for i in 1..6 {
            tr.update("c", 1000 * i, &[weak.clone(), dog.clone()], &t);
        }
        assert!(tr.flush("c", 99_000, &t).is_empty());
        // Following dogs as well changes that.
        let t2 = Tuning { labels: vec!["person".into(), "dog".into()], ..tuning() };
        let mut tr = Tracker::default();
        for i in 0..4 {
            tr.update("c", 1000 * i, std::slice::from_ref(&dog), &t2);
        }
        assert_eq!(tr.flush("c", 99_000, &t2).len(), 1);
    }

    #[test]
    fn a_jump_beyond_the_radius_or_a_long_gap_starts_a_new_walk() {
        let (mut tr, t) = (Tracker::default(), tuning());
        for i in 0..4 {
            tr.update("c", 1000 * i, &[obs(0.1, 0.5)], &t);
        }
        // Far from anywhere they were, one second later: someone else.
        for i in 4..8 {
            tr.update("c", 1000 * i, &[obs(0.8, 0.5)], &t);
        }
        let done = tr.flush("c", 99_000, &t);
        assert_eq!(done.len(), 2, "two walks");
        // The same spot after more than the gap (4 s): a new walk too.
        let mut tr = Tracker::default();
        let mut all = Vec::new();
        for ms in [0, 1000, 2000, 12_000, 13_000, 14_000] {
            all.extend(tr.update("c", ms, &[obs(0.5, 0.5)], &t));
        }
        all.extend(tr.flush("c", 99_000, &t));
        assert_eq!(all.len(), 2);
        // A longer gap setting keeps them together.
        let t3 = Tuning { track_gap_seconds: 20.0, match_radius: 0.2, ..tuning() };
        let mut tr = Tracker::default();
        let mut all = Vec::new();
        for ms in [0, 1000, 2000, 12_000, 13_000, 14_000] {
            all.extend(tr.update("c", ms, &[obs(0.5, 0.5)], &t3));
        }
        all.extend(tr.flush("c", 99_000, &t3));
        assert_eq!(all.len(), 1);
    }

    // ----- clothing colour -----

    fn person_picture(top: [u8; 3], bottom: [u8; 3]) -> crate::imgutil::Frame {
        let (w, h) = (320usize, 180usize);
        let mut rgb = vec![90u8; w * h * 3];
        // A person at x 0.4..0.6, y 0.2..0.9: top half one colour, bottom half another.
        for y in (0.2 * h as f32) as usize..(0.9 * h as f32) as usize {
            for x in (0.4 * w as f32) as usize..(0.6 * w as f32) as usize {
                let c = if (y as f32) < 0.55 * h as f32 { top } else { bottom };
                rgb[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&c);
            }
        }
        crate::imgutil::Frame { width: w as u32, height: h as u32, rgb: std::sync::Arc::new(rgb) }
    }

    #[test]
    fn the_same_outfit_matches_better_than_a_different_one_even_in_other_light() {
        let a = signature(&person_picture([200, 30, 30], [30, 30, 150]), 0.4, 0.2, 0.2, 0.7).unwrap();
        // The same clothes, darker (another camera, another light).
        let a_dim = signature(&person_picture([150, 22, 22], [22, 22, 110]), 0.4, 0.2, 0.2, 0.7).unwrap();
        let other = signature(&person_picture([30, 150, 30], [200, 200, 40]), 0.4, 0.2, 0.2, 0.7).unwrap();
        let same = similarity(&a, &a_dim);
        let diff = similarity(&a, &other);
        assert!(same > 0.8, "same outfit, other light: {same}");
        assert!(diff < 0.4, "different outfit: {diff}");
        assert!(same > diff + 0.4);
        // Too small a box says nothing.
        assert!(signature(&person_picture([0; 3], [0; 3]), 0.5, 0.5, 0.01, 0.02).is_none());
    }

    // ----- joining walks across cameras -----

    /// Two cameras side by side on a 40 m × 20 m plan: A looks at the left half, B at the right.
    fn plan() -> Plan {
        let calib = |x0: f64, x1: f64| Calib { picture: [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]], plan: [[x0, 1.0], [x1, 1.0], [x1, 0.0], [x0, 0.0]] };
        let cam = |id: &str, x0: f64, x1: f64| PlanCamera { id: id.into(), x: (x0 + x1) / 2.0, y: 0.5, rotation_deg: 0.0, fov_deg: 90.0, range_m: 15.0, calib: Some(calib(x0, x1)), size: 1.0 };
        Plan { has_image: true, image_w: 800, image_h: 400, px_per_m: 20.0, cameras: vec![cam("a", 0.0, 0.5), cam("b", 0.5, 1.0)], tuning: tuning(), ..Default::default() }
    }

    fn walk(id: &str, cam: &str, from_ms: i64, secs: i64, x0: f32, x1: f32, sig: Option<Sig>) -> Track {
        let n = secs + 1;
        let pts = (0..n).map(|i| Point(from_ms + 1000 * i, x0 + (x1 - x0) * i as f32 / secs as f32, 0.5, 0.05, 0.2)).collect();
        Track { id: id.into(), cam: cam.into(), label: "person".into(), pts, sig }
    }

    #[test]
    fn a_person_leaving_one_camera_and_appearing_on_the_next_is_one_journey_with_a_speed() {
        // A: walks right across all of the camera's width (20 m) in 14 s, ends at its right edge.
        // B: appears 2 s later at its left edge and walks on 14 s.
        let a = walk("a1", "a", 0, 14, 0.0, 1.0, None);
        let b = walk("b1", "b", 16_000, 14, 0.0, 1.0, None);
        let j = link(&[a, b], &plan());
        assert_eq!(j.len(), 1, "{j:?}");
        let j = &j[0];
        assert_eq!(j.cameras, vec!["a", "b"]);
        assert_eq!((j.start, j.end), (0, 30_000));
        // 20 m + 20 m on the plan (smoothing makes the path a touch shorter), 30 s.
        assert!(j.metres > 30.0 && j.metres < 41.0, "{}", j.metres);
        let v = j.speed_mps.unwrap();
        assert!(v > 1.0 && v < 1.4, "a walking speed: {v}");
        assert!(j.confidence > 0.5);
    }

    #[test]
    fn too_late_or_too_far_or_the_wrong_way_is_not_the_same_person() {
        let a = walk("a1", "a", 0, 10, 0.0, 0.2, None); // ends at x=0.1 of the plan (4 m from the left wall)
        // Appears on B 60 s later: beyond the 30 s limit.
        assert_eq!(link(&[a.clone(), walk("b1", "b", 70_000, 5, 0.0, 0.5, None)], &plan()).len(), 2);
        // Appears on B one second later, but B's left edge is 16 m from where A lost them: nobody walks that in 1 s.
        assert_eq!(link(&[a.clone(), walk("b1", "b", 11_000, 5, 0.0, 0.5, None)], &plan()).len(), 2);
        // The same pair a walk apart is fine when the speed allows it (16 m in 12 s = 1.3 m/s).
        assert_eq!(link(&[a.clone(), walk("b1", "b", 22_000, 5, 0.0, 0.5, None)], &plan()).len(), 1);
        // A faster allowed speed makes the near-instant hop possible; a stricter one forbids the slow one.
        let mut fast = plan();
        fast.tuning.max_speed_mps = 15.0;
        fast.tuning.slack_m = 20.0;
        assert_eq!(link(&[a.clone(), walk("b1", "b", 11_000, 5, 0.0, 0.5, None)], &fast).len(), 1);
        let mut strict = plan();
        strict.tuning.max_speed_mps = 0.5;
        strict.tuning.slack_m = 0.0;
        assert_eq!(link(&[a, walk("b1", "b", 22_000, 5, 0.0, 0.5, None)], &strict).len(), 2);
    }

    #[test]
    fn two_people_at_once_are_told_apart_by_colour_and_each_joined_once() {
        let red: Sig = [0.6, 0.2, 0.6, 0.6, 0.2, 0.4, 0.55, 0.25, 0.5];
        let blue: Sig = [0.2, 0.3, 0.6, 0.2, 0.3, 0.5, 0.2, 0.3, 0.5];
        // Both leave A right at the same moment and both appear on B within a second of each other.
        let a_red = walk("ar", "a", 0, 10, 0.5, 0.9, Some(red));
        let a_blue = walk("ab", "a", 0, 10, 0.5, 0.9, Some(blue));
        // Listed in the "wrong" order on B: the colours decide.
        let b_blue = walk("bb", "b", 12_000, 8, 0.0, 0.3, Some(blue));
        let b_red = walk("br", "b", 12_500, 8, 0.0, 0.3, Some(red));
        let js = link(&[a_red, a_blue, b_blue, b_red], &plan());
        assert_eq!(js.len(), 2, "two people, two journeys");
        for j in &js {
            let first = &j.segments[0].track;
            let second = &j.segments[1].track;
            assert_eq!((first.as_str(), second.as_str()), if first == "ar" { ("ar", "br") } else { ("ab", "bb") }, "colours kept the right people together");
        }
        // With colour switched off, the pairing is a coin flip, but still one-to-one.
        let mut p = plan();
        p.tuning.colour_weight = 0.0;
        let a1 = walk("a1", "a", 0, 10, 0.5, 0.9, Some(red));
        let a2 = walk("a2", "a", 0, 10, 0.5, 0.9, Some(blue));
        let b1 = walk("b1", "b", 12_000, 8, 0.0, 0.3, None);
        let b2 = walk("b2", "b", 12_500, 8, 0.0, 0.3, None);
        let js = link(&[a1, a2, b1, b2], &p);
        assert!(js.iter().all(|j| j.segments.len() == 2) && js.len() == 2);
    }

    #[test]
    fn neighbours_drawn_on_the_plan_limit_the_joins_unless_all_pairs_are_allowed() {
        let mut p = plan();
        p.cameras.push(PlanCamera { id: "c".into(), x: 0.9, y: 0.1, rotation_deg: 0.0, fov_deg: 90.0, range_m: 15.0, calib: None, size: 1.0 });
        p.links = vec![Link { a: "a".into(), b: "b".into(), travel_seconds: Some(3.0) }];
        // C is not a neighbour of A, so A → C is never joined (no calibration on C: only timing counts).
        let a = walk("a1", "a", 0, 5, 0.0, 1.0, None);
        let c = Track { cam: "c".into(), ..walk("c1", "c", 6_000, 5, 0.1, 0.2, None) };
        assert_eq!(link(&[a.clone(), c.clone()], &p).len(), 2);
        p.tuning.link_all_pairs = true;
        assert_eq!(link(&[a.clone(), c], &p).len(), 1);
        // With a known walking time, a join near that time beats one far from it.
        let mut p2 = plan();
        p2.links = vec![Link { a: "a".into(), b: "b".into(), travel_seconds: Some(10.0) }];
        p2.tuning.min_link_score = 0.0;
        let near = |gap: i64| link(&[walk("a1", "a", 0, 5, 0.0, 1.0, None), walk("b1", "b", 5_000 + gap, 5, 0.0, 0.5, None)], &p2)[0].confidence;
        assert!(near(10_000) > near(24_000), "{} vs {}", near(10_000), near(24_000));
    }

    #[test]
    fn without_calibration_there_is_still_a_journey_but_no_path_or_speed() {
        let mut p = plan();
        p.cameras.iter_mut().for_each(|c| c.calib = None);
        let js = link(&[walk("a1", "a", 0, 10, 0.0, 1.0, None), walk("b1", "b", 12_000, 10, 0.0, 1.0, None)], &p);
        assert_eq!(js.len(), 1);
        assert!(js[0].segments.iter().all(|s| s.path.is_empty()));
        assert_eq!((js[0].metres, js[0].speed_mps), (0.0, None));
        assert_eq!(js[0].cameras, vec!["a", "b"]);
    }

    // ----- files -----

    #[test]
    fn walks_are_stored_by_day_read_back_pruned_and_deleted() {
        let dir = std::env::temp_dir().join(format!("nvr-tracks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let s = Store::open(&dir);
        let day = 86_400_000i64;
        let now = 1_800_000_000_000i64;
        let old = walk("old", "a", now - 20 * day, 5, 0.1, 0.5, Some([0.3; 9]));
        let recent = walk("new", "a", now - 1000, 5, 0.1, 0.5, None);
        s.append(&[old.clone(), recent.clone()]).unwrap();
        s.append(&[walk("new2", "b", now - 500, 3, 0.2, 0.4, None)]).unwrap();
        let got = s.read(now - 2 * day, now + day);
        assert_eq!(got.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), vec!["new", "new2"]);
        assert_eq!(s.read(now - 30 * day, now + day).len(), 3);
        assert_eq!(s.read(now - 30 * day, now + day)[0].sig, old.sig, "colour numbers survive");
        // A damaged line is skipped, not fatal.
        std::fs::write(dir.join("tracks").join("2001-01-01.jsonl"), "not json\n").unwrap();
        assert_eq!(s.read(now - 30 * day, now + day).len(), 3);
        // 14 days kept: the 20-day-old file goes (and the 2001 one).
        assert_eq!(s.prune(14, now), 2);
        assert_eq!(s.read(now - 30 * day, now + day).len(), 2);
        assert_eq!(s.delete_all(), 1);
        assert!(s.read(0, i64::MAX / 2).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_numbers_add_up() {
        let p = plan();
        let tracks = [walk("a1", "a", 1_800_000_000_000, 14, 0.0, 1.0, None), walk("b1", "b", 1_800_000_016_000, 14, 0.0, 1.0, None), walk("a2", "a", 1_800_000_500_000, 10, 0.2, 0.4, None)];
        let js = link(&tracks, &p);
        let st = stats(&js, &tracks);
        assert_eq!(st.people, 2);
        assert_eq!(st.by_hour.iter().sum::<u32>(), 2);
        assert_eq!(st.cameras.iter().map(|c| (c.cam.as_str(), c.walks)).collect::<Vec<_>>(), vec![("a", 2), ("b", 1)]);
        assert!(st.avg_speed_mps.unwrap() > 0.5);
        assert!((st.cameras[0].avg_seconds - 12.0).abs() < 1e-9);
        let none = stats(&[], &[]);
        assert_eq!((none.people, none.avg_speed_mps, none.avg_seconds), (0, None, 0.0));
        // On the plan: the left edge of A's picture is the left edge of the plan.
        let at = on_plan(&p, "a", &Point(0, 0.0, 0.5, 0.0, 0.0)).unwrap();
        assert!(at[0].abs() < 1e-9 && (at[1] - 0.5).abs() < 1e-9);
        assert!(on_plan(&p, "zzz", &Point(0, 0.0, 0.5, 0.0, 0.0)).is_none());
    }
}

