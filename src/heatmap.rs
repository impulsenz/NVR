//! The Heat map page: where motion and recognised objects happen on each
//! camera, to review like the alerts: which kinds, over the last day, week or
//! month. It is also how to see where motion is *wasted* (a swaying branch, a
//! flag, a road): motion that no recognised object explains.
//!
//! What is logged, always, with nothing to switch on:
//!   * every look of the motion detector: the boxes of moving areas, **before**
//!     the ignore areas are applied (so areas already ignored still show what
//!     is there), and
//!   * every look of the AI: the boxes of every object it recognised (people,
//!     vehicles, animals, anything), whether or not they raise an alert or are
//!     wanted by any setting.
//!
//! They are added up per hour into a coarse 64×36 grid per kind and kept as
//! small files (`nvr-data/heat/<camera>/<day>.json`, only the cells that
//! counted) for 30 days. The page smooths the grid when it draws it. Before
//! this log existed (or for hours GAZE was off) the boxes stored with alerts
//! stand in, so there is something to look at at once.
//!
//! Counts are "looks": about a second of something being there per look, so a
//! person who stands still for a minute weighs more than one who walks past.
//!
//! The areas painted as "ignore" in a camera's Motion settings are **not**
//! applied when logging (the data is always kept). The page decides when it
//! reads: by default the cells those areas cover are left out (what is
//! ignored for alerts is ignored here), and the user can tick them back in.
//! It is applied with the camera's *current* areas, so changing them changes
//! old data's view too, and nothing is ever lost by ignoring.
//!
//! Recording, detection and alerts never read any of it; a failure here
//! drops data, never delays them.

use crate::detect::MotionObject;
use crate::events::Event;
use crate::manager::Manager;
use crate::sync::MutexExt;
use chrono::{DateTime, Local, NaiveDate, TimeZone, Timelike};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

pub const COLS: usize = 64;
pub const ROWS: usize = 36;
/// Cells, then one more that counts the boxes added.
const GRID: usize = COLS * ROWS + 1;
/// Days of log kept.
pub const KEEP_DAYS: i64 = 30;
/// The only objects the page shows (everything the AI sees, and the list of what it saw):
/// the rest is still logged, and still explains movement for "unexplained motion".
pub const SHOWN: [&str; 4] = ["person", "vehicle", "cat", "dog"];
/// How sure the AI must be for an object to count.
const MIN_SCORE: f32 = 0.4;

type Grid = Vec<u32>;
/// One hour of one camera: label ("motion" or an object's name) → grid.
type Hour = HashMap<String, Grid>;

/// What has been seen since the last write to disk: (camera, hour) → kinds.
static ACC: LazyLock<Mutex<HashMap<(String, i64), Hour>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// The cells a box covers, as (first column, last column, first row, last row).
fn cells_of(b: [f32; 4]) -> Option<(usize, usize, usize, usize)> {
    let [x, y, w, h] = b;
    if ![x, y, w, h].iter().all(|v| v.is_finite()) || w <= 0.0 || h <= 0.0 {
        return None;
    }
    let (x0, y0) = (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0));
    let (x1, y1) = ((x + w).clamp(0.0, 1.0), (y + h).clamp(0.0, 1.0));
    if x1 <= x0 || y1 <= y0 {
        return None; // wholly outside the picture
    }
    let c0 = ((x0 * COLS as f32) as usize).min(COLS - 1);
    let c1 = (((x1 * COLS as f32).ceil() as usize).max(1) - 1).min(COLS - 1);
    let r0 = ((y0 * ROWS as f32) as usize).min(ROWS - 1);
    let r1 = (((y1 * ROWS as f32).ceil() as usize).max(1) - 1).min(ROWS - 1);
    Some((c0, c1, r0, r1))
}

fn add_box(g: &mut Grid, b: [f32; 4]) -> bool {
    let Some((c0, c1, r0, r1)) = cells_of(b) else { return false };
    for r in r0..=r1 {
        for c in c0..=c1 {
            g[r * COLS + c] = g[r * COLS + c].saturating_add(1);
        }
    }
    g[COLS * ROWS] = g[COLS * ROWS].saturating_add(1);
    true
}

fn record(cam: &str, now_ms: i64, label: &str, b: [f32; 4]) {
    let hour = now_ms.div_euclid(3_600_000);
    let mut acc = ACC.locked();
    let g = acc.entry((cam.to_string(), hour)).or_default().entry(label.to_string()).or_insert_with(|| vec![0; GRID]);
    add_box(g, b);
}

/// One look of the motion detector: its moving areas (before ignore areas).
pub fn motion(cam: &str, now_ms: i64, objects: &[MotionObject]) {
    for o in objects {
        record(cam, now_ms, "motion", [o.x, o.y, o.w, o.h]);
    }
}

/// One look of the AI: everything it recognised, whatever it is.
pub fn objects(cam: &str, now_ms: i64, dets: &[crate::ai::Detection]) {
    for d in dets.iter().filter(|d| d.score >= MIN_SCORE) {
        record(cam, now_ms, crate::ai::canon(&d.label), [d.x, d.y, d.w, d.h]);
    }
}

// ---------- on disk ----------

/// hour → label → the cells that counted, as `[cell, count]` (cell `COLS*ROWS` = boxes added).
type DayFile = BTreeMap<String, BTreeMap<String, Vec<[u32; 2]>>>;

fn dir(data: &Path, cam: &str) -> Option<PathBuf> {
    crate::storage::valid_folder(cam).then(|| data.join("heat").join(cam))
}

fn day_of(hour: i64) -> NaiveDate {
    Local.timestamp_opt(hour * 3600, 0).single().map(|t| t.date_naive()).unwrap_or_default()
}

fn sparse(g: &Grid) -> Vec<[u32; 2]> {
    g.iter().enumerate().filter(|(_, v)| **v > 0).map(|(i, v)| [i as u32, *v]).collect()
}

fn read_day(path: &Path) -> DayFile {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Write what has been seen since last time into the day files.
pub fn flush(data: &Path) {
    let taken = std::mem::take(&mut *ACC.locked());
    let mut by_file: HashMap<(String, NaiveDate), Vec<(i64, Hour)>> = HashMap::new();
    for ((cam, hour), h) in taken {
        by_file.entry((cam, day_of(hour))).or_default().push((hour, h));
    }
    for ((cam, day), hours) in by_file {
        let Some(d) = dir(data, &cam) else { continue };
        if std::fs::create_dir_all(&d).is_err() {
            continue;
        }
        let path = d.join(format!("{day}.json"));
        let mut file = read_day(&path);
        for (hour, h) in hours {
            let slot = file.entry(hour.to_string()).or_default();
            for (label, g) in h {
                // Add to what is there.
                let mut cur: Grid = vec![0; GRID];
                for [i, v] in slot.get(&label).cloned().unwrap_or_default() {
                    if let Some(c) = cur.get_mut(i as usize) {
                        *c = v;
                    }
                }
                for (c, v) in cur.iter_mut().zip(g.iter()) {
                    *c = c.saturating_add(*v);
                }
                slot.insert(label, sparse(&cur));
            }
        }
        let tmp = path.with_extension("json.tmp");
        if let Ok(b) = serde_json::to_vec(&file)
            && std::fs::write(&tmp, b).is_ok()
        {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

/// Remove day files older than `KEEP_DAYS`.
pub fn tidy(data: &Path) {
    let Ok(cams) = std::fs::read_dir(data.join("heat")) else { return };
    let cutoff = Local::now().date_naive() - chrono::Duration::days(KEEP_DAYS);
    for c in cams.flatten() {
        let Ok(files) = std::fs::read_dir(c.path()) else { continue };
        for f in files.flatten() {
            let p = f.path();
            let day = p.file_stem().and_then(|s| s.to_str()).and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());
            if day.is_some_and(|d| d < cutoff) {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
}

/// Write the log every minute and tidy it every hour.
pub async fn run(mgr: Arc<Manager>) {
    let mut n = 0u32;
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;
        let data = mgr.data_dir.clone();
        n += 1;
        let _ = tokio::task::spawn_blocking(move || {
            flush(&data);
            if n % 60 == 1 {
                tidy(&data);
            }
        })
        .await;
    }
}

// ---------- reading it back ----------

/// Everything logged for one camera in a period: label → grid.
#[derive(Debug, Default)]
pub struct Raw {
    pub grids: HashMap<String, Grid>,
    /// Hours that had anything logged.
    pub hours: usize,
    /// Alerts whose boxes stood in for hours before the log started.
    pub alerts_used: usize,
    /// Label → boxes by hour of the day (local time), after the ignore areas.
    pub hourly: HashMap<String, [u32; 24]>,
    /// Label → (heat inside the ignore areas, all heat), before they were left out.
    pub ignored: HashMap<String, [u64; 2]>,
}

/// Which heat cells the ignore areas of a camera's Motion settings cover (a
/// cell counts when at least half of it is painted), or None when there are
/// none. `mask` is the setting as stored.
pub fn ignore_cells(mask: &str) -> Option<Vec<bool>> {
    use crate::config::{MASK_H, MASK_W};
    let m = crate::config::decode_mask(mask)?;
    let mut out = vec![false; COLS * ROWS];
    for r in 0..ROWS {
        for c in 0..COLS {
            let (x0, x1) = (c * MASK_W / COLS, ((c + 1) * MASK_W).div_ceil(COLS));
            let (y0, y1) = (r * MASK_H / ROWS, ((r + 1) * MASK_H).div_ceil(ROWS));
            let (mut on, mut all) = (0usize, 0usize);
            for y in y0..y1.min(MASK_H) {
                for x in x0..x1.min(MASK_W) {
                    all += 1;
                    on += m[y * MASK_W + x] as usize;
                }
            }
            out[r * COLS + c] = all > 0 && on * 2 >= all;
        }
    }
    out.iter().any(|v| *v).then_some(out)
}

fn local_hour(hour: i64) -> usize {
    Local.timestamp_opt(hour * 3600, 0).single().map(|t| t.hour() as usize).unwrap_or(0)
}

/// Add one label's cells for one hour, leaving out the cells under `ignore`.
/// The count of boxes is scaled to what is left (a box can't be split by
/// cell, so it is an estimate whenever something was left out).
fn take(raw: &mut Raw, label: &str, hour: i64, cells: &[(usize, u32)], ignore: Option<&[bool]>) {
    let (mut counter, mut kept, mut ign) = (0u32, 0u64, 0u64);
    let mut keep = Vec::with_capacity(cells.len() + 1);
    for &(i, v) in cells {
        if i == COLS * ROWS {
            counter = v;
        } else if ignore.is_some_and(|m| m.get(i) == Some(&true)) {
            ign += v as u64;
        } else {
            kept += v as u64;
            keep.push((i, v));
        }
    }
    let total = kept + ign;
    if ign > 0 && total > 0 {
        counter = ((counter as u64 * kept + total / 2) / total) as u32;
    }
    keep.push((COLS * ROWS, counter));
    add_into(raw, label, keep.into_iter());
    let h = raw.hourly.entry(label.to_string()).or_insert([0; 24]);
    h[local_hour(hour)] = h[local_hour(hour)].saturating_add(counter);
    let e = raw.ignored.entry(label.to_string()).or_default();
    e[0] += ign;
    e[1] += total;
}

fn add_into(raw: &mut Raw, label: &str, cells: impl Iterator<Item = (usize, u32)>) {
    let g = raw.grids.entry(label.to_string()).or_insert_with(|| vec![0; GRID]);
    for (i, v) in cells {
        if let Some(c) = g.get_mut(i) {
            *c = c.saturating_add(v);
        }
    }
}

/// What was logged for `cam` in `from..=to`, plus the boxes stored with
/// `events` for the time before the log began.
pub fn read(data: &Path, cam: &str, from: DateTime<Local>, to: DateTime<Local>, events: &[Event], ignore: Option<&[bool]>) -> Raw {
    let mut raw = Raw::default();
    let (from_h, to_h) = (from.timestamp().div_euclid(3600), to.timestamp().div_euclid(3600));
    let mut hours_seen = std::collections::HashSet::new();
    let mut first_logged: Option<i64> = None;
    if let Some(d) = dir(data, cam) {
        // The earliest hour ever logged for this camera (the first day file).
        let mut names: Vec<String> =
            std::fs::read_dir(&d).into_iter().flatten().flatten().filter_map(|e| e.file_name().to_str().map(String::from)).filter(|n| n.ends_with(".json")).collect();
        names.sort();
        if let Some(first) = names.first() {
            first_logged = read_day(&d.join(first)).keys().filter_map(|h| h.parse::<i64>().ok()).min();
        }
        let mut day = from.date_naive();
        while day <= to.date_naive() {
            for (h, labels) in read_day(&d.join(format!("{day}.json"))) {
                let Ok(h) = h.parse::<i64>() else { continue };
                if h < from_h || h > to_h {
                    continue;
                }
                hours_seen.insert(h);
                for (label, cells) in labels {
                    let cells: Vec<(usize, u32)> = cells.into_iter().map(|[i, v]| (i as usize, v)).collect();
                    take(&mut raw, &label, h, &cells, ignore);
                }
            }
            day = day.succ_opt().unwrap_or(to.date_naive() + chrono::Duration::days(1));
        }
    }
    // Not written yet.
    for ((c, h), labels) in ACC.locked().iter() {
        if c != cam || *h < from_h || *h > to_h {
            continue;
        }
        hours_seen.insert(*h);
        first_logged = Some(first_logged.map_or(*h, |f| f.min(*h)));
        for (label, g) in labels {
            let cells: Vec<(usize, u32)> = g.iter().enumerate().filter(|(_, v)| **v > 0).map(|(i, v)| (i, *v)).collect();
            take(&mut raw, label, *h, &cells, ignore);
        }
    }
    raw.hours = hours_seen.len();
    // Before the log: the alerts' own boxes.
    let log_start = first_logged.map(|h| h * 3600);
    for e in events.iter().filter(|e| e.start >= from && e.start <= to && log_start.is_none_or(|s| e.start.timestamp() < s)) {
        let mut any = false;
        for b in &e.boxes {
            let label = crate::ai::canon(&b.label);
            let mut g = vec![0; GRID];
            if add_box(&mut g, [b.x, b.y, b.w, b.h]) {
                let cells: Vec<(usize, u32)> = g.iter().enumerate().filter(|(_, v)| **v > 0).map(|(i, v)| (i, *v)).collect();
                take(&mut raw, label, e.start.timestamp().div_euclid(3600), &cells, ignore);
                any = true;
            }
        }
        if any {
            raw.alerts_used += 1;
        }
    }
    raw
}

// ---------- kinds ----------

/// What to show.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// Anything that moved (before ignore areas).
    Motion,
    /// Everything the AI recognised.
    Objects,
    People,
    Vehicles,
    /// Cats and dogs together.
    Pets,
    Animals,
    /// Recognised things that are not people, vehicles or animals.
    Other,
    /// One kind by its name.
    Label(String),
    /// Motion that no recognised object explains: the places to ignore.
    Unexplained,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "" | "motion" => Kind::Motion,
            "objects" | "all" => Kind::Objects,
            "people" | "person" => Kind::People,
            "pets" | "cats and dogs" => Kind::Pets,
            "vehicles" | "vehicle" => Kind::Vehicles,
            "animals" | "animal" => Kind::Animals,
            "other" => Kind::Other,
            "unexplained" => Kind::Unexplained,
            _ => {
                let l = s.strip_prefix("label:")?.trim().to_lowercase();
                if l.is_empty() || l.len() > 40 || l == "motion" || !l.chars().all(|c| c.is_alphanumeric() || " -_".contains(c)) {
                    return None;
                }
                Kind::Label(l)
            }
        })
    }

    fn takes(&self, label: &str) -> bool {
        if label == "motion" {
            return *self == Kind::Motion;
        }
        let animal = crate::ai::ANIMALS.contains(&label);
        let vehicle = label == "vehicle" || label == "bicycle";
        match self {
            Kind::Motion | Kind::Unexplained => false,
            Kind::Objects => SHOWN.contains(&label),
            Kind::People => label == "person",
            Kind::Vehicles => vehicle,
            Kind::Pets => label == "cat" || label == "dog",
            Kind::Animals => animal,
            Kind::Other => label != "person" && !vehicle && !animal,
            Kind::Label(l) => label == l,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Heatmap {
    pub cols: usize,
    pub rows: usize,
    /// Row by row, left to right: how many looks had something there.
    /// (For "unexplained": 0 to 1000, how much motion nothing explains.)
    pub cells: Vec<u32>,
    pub max: u32,
    /// The level the busiest 5% of the places reach: a good "full heat",
    /// so one very hot spot doesn't make everything else look cold.
    pub p95: u32,
    /// Boxes counted.
    pub detections: u32,
    /// Hours that had anything logged, and alerts that stood in for older time.
    pub hours: usize,
    pub alerts_used: usize,
    /// Every kind of recognised object seen in the period, the most first.
    pub labels: Vec<String>,
    /// How many of each (same order as `labels`).
    pub counts: Vec<u32>,
    /// Boxes by hour of the day (local time) for this kind.
    pub hourly: Vec<u32>,
    /// How much of this kind's heat lay inside the camera's ignore areas
    /// (0 to 1), whether or not they were left out.
    pub ignored_share: f32,
}

fn p95(cells: &[u32]) -> u32 {
    let mut v: Vec<u32> = cells.iter().copied().filter(|c| *c > 0).collect();
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    v[((v.len() - 1) as f32 * 0.95) as usize]
}

/// Motion that nothing recognised accounts for, per cell, 0 to 1000: motion
/// and recognised objects are each scaled to their own busy level, objects
/// are spread a little (their boxes are bigger than the movement inside
/// them), and what motion has beyond that is what is left.
pub fn unexplained(motion: &[u32], objects: &[u32]) -> Vec<u32> {
    let (pm, po) = (p95(motion).max(1) as f32, p95(objects).max(1) as f32);
    let mut spread = vec![0f32; COLS * ROWS];
    for r in 0..ROWS {
        for c in 0..COLS {
            let mut best = 0f32;
            for rr in r.saturating_sub(1)..=(r + 1).min(ROWS - 1) {
                for cc in c.saturating_sub(1)..=(c + 1).min(COLS - 1) {
                    best = best.max(objects[rr * COLS + cc] as f32 / po);
                }
            }
            spread[r * COLS + c] = best;
        }
    }
    (0..COLS * ROWS).map(|i| (((motion[i] as f32 / pm).min(1.0) - spread[i].min(1.0) * 0.9).max(0.0) * 1000.0) as u32).collect()
}

/// The grid for one kind of what was logged.
pub fn select(raw: &Raw, kind: &Kind) -> Heatmap {
    let mut cells = vec![0u32; COLS * ROWS];
    let mut detections = 0u32;
    let (mut hourly, mut ign) = (vec![0u32; 24], [0u64; 2]);
    let mut count_hours = |l: &str| {
        if let Some(h) = raw.hourly.get(l) {
            for (a, b) in hourly.iter_mut().zip(h.iter()) {
                *a = a.saturating_add(*b);
            }
        }
        if let Some(e) = raw.ignored.get(l) {
            ign[0] += e[0];
            ign[1] += e[1];
        }
    };
    if *kind == Kind::Unexplained {
        count_hours("motion");
        let zero = vec![0; GRID];
        let m = raw.grids.get("motion").unwrap_or(&zero);
        let mut obj = vec![0u32; COLS * ROWS];
        for (l, g) in &raw.grids {
            if l != "motion" {
                for (o, v) in obj.iter_mut().zip(g.iter()) {
                    *o = o.saturating_add(*v);
                }
            }
        }
        cells = unexplained(&m[..COLS * ROWS], &obj);
        detections = m[COLS * ROWS];
    } else {
        for (l, g) in &raw.grids {
            if kind.takes(l) {
                count_hours(l);
                for (c, v) in cells.iter_mut().zip(g.iter()) {
                    *c = c.saturating_add(*v);
                }
                detections = detections.saturating_add(g[COLS * ROWS]);
            }
        }
    }
    // The list of what was seen: cats and dogs are one entry ("pets").
    let mut counted: BTreeMap<String, u64> = BTreeMap::new();
    for (l, g) in raw.grids.iter().filter(|(l, _)| SHOWN.contains(&l.as_str())) {
        let name = if l == "cat" || l == "dog" { "pets" } else { l.as_str() };
        *counted.entry(name.to_string()).or_default() += g[COLS * ROWS] as u64;
    }
    let mut labels: Vec<(String, u64)> = counted.into_iter().filter(|(_, n)| *n > 0).collect();
    labels.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let counts: Vec<u32> = labels.iter().map(|(_, n)| (*n).min(u32::MAX as u64) as u32).collect();
    Heatmap {
        ignored_share: if ign[1] > 0 { ign[0] as f32 / ign[1] as f32 } else { 0.0 },
        hourly,
        counts,
        cols: COLS,
        rows: ROWS,
        max: cells.iter().copied().max().unwrap_or(0),
        p95: p95(&cells),
        cells,
        detections,
        hours: raw.hours,
        alerts_used: raw.alerts_used,
        labels: labels.into_iter().map(|(l, _)| l).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::SeenBox;

    fn event(start: DateTime<Local>, boxes: Vec<(&str, f32, f32, f32, f32)>) -> Event {
        let mut e: Event = serde_json::from_value(serde_json::json!({"id": "e", "camera": "c", "start": start, "end": start})).unwrap();
        e.boxes = boxes.into_iter().map(|(l, x, y, w, h)| SeenBox { label: l.into(), score: 0.9, x, y, w, h }).collect();
        e
    }

    /// The log's accumulator is shared by the whole program: tests that write to it take turns.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nvr-heat-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn mo(x: f32, y: f32, w: f32, h: f32) -> MotionObject {
        MotionObject { x, y, w, h, size: 100 }
    }

    fn det(label: &str, score: f32, x: f32, y: f32, w: f32, h: f32) -> crate::ai::Detection {
        crate::ai::Detection { label: label.into(), score, x, y, w, h }
    }

    #[test]
    fn a_box_heats_exactly_the_cells_it_covers() {
        let mut g = vec![0; GRID];
        assert!(add_box(&mut g, [0.0, 0.0, 0.5, 0.5]));
        assert_eq!(g[COLS * ROWS], 1);
        assert_eq!(g[0], 1);
        assert_eq!(g[(ROWS / 2 - 1) * COLS + COLS / 2 - 1], 1);
        assert_eq!(g[(ROWS / 2) * COLS], 0, "below the box");
        assert_eq!(g[COLS / 2], 0, "right of the box");
        assert_eq!(g[..COLS * ROWS].iter().filter(|v| **v > 0).count(), (ROWS / 2) * (COLS / 2));
        // Nonsense and boxes off the picture add nothing.
        assert!(!add_box(&mut g, [f32::NAN, 0.0, 0.1, 0.1]));
        assert!(!add_box(&mut g, [1.2, 0.0, 0.1, 0.1]));
        assert!(!add_box(&mut g, [0.1, 0.1, 0.0, 0.1]));
    }

    #[test]
    fn what_is_seen_is_written_to_disk_added_up_and_read_back_by_period_and_kind() {
        let _turn = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let data = scratch("roundtrip");
        let cam = "front";
        let now = Local::now();
        let ms = now.timestamp_millis();
        // Motion in the top left, a person and a car lower down, a cat, something unusual.
        motion(cam, ms, &[mo(0.0, 0.0, 0.25, 0.25), mo(0.0, 0.0, 0.25, 0.25)]);
        objects(
            cam,
            ms,
            &[
                det("person", 0.9, 0.5, 0.5, 0.1, 0.3),
                det("truck", 0.8, 0.6, 0.7, 0.3, 0.2),
                det("cat", 0.7, 0.9, 0.9, 0.1, 0.1),
                det("chair", 0.8, 0.1, 0.5, 0.1, 0.1),
                det("person", 0.2, 0.0, 0.5, 0.1, 0.1),
            ],
        );
        // Some of it already on disk, some still waiting: both count, and the second write adds to the first.
        flush(&data);
        motion(cam, ms, &[mo(0.0, 0.0, 0.25, 0.25)]);
        let raw = read(&data, cam, now - chrono::Duration::hours(2), now + chrono::Duration::hours(1), &[], None);
        let all = select(&raw, &Kind::Motion);
        assert_eq!(all.detections, 3, "two on disk, one waiting");
        assert_eq!(all.cells[0], 3);
        assert_eq!(all.cells[(ROWS - 1) * COLS + COLS - 1], 0);
        // Kinds.
        assert_eq!(select(&raw, &Kind::People).detections, 1, "the 20%-sure person is not counted");
        assert_eq!(select(&raw, &Kind::Vehicles).detections, 1, "a truck is a vehicle");
        assert_eq!(select(&raw, &Kind::Animals).detections, 1);
        assert_eq!(select(&raw, &Kind::Other).detections, 1, "the chair");
        assert_eq!(select(&raw, &Kind::Objects).detections, 3, "a person, a truck and a cat: the chair is not shown, nor is motion");
        assert_eq!(select(&raw, &Kind::Label("cat".into())).detections, 1);
        assert_eq!(select(&raw, &Kind::Pets).detections, 1, "the cat; a dog would count with it");
        assert_eq!(Kind::parse("pets"), Some(Kind::Pets));
        assert_eq!(select(&raw, &Kind::Label("dog".into())).detections, 0);
        // Which kinds were seen, for the page's choices.
        let labels = select(&raw, &Kind::Objects).labels;
        for l in ["person", "vehicle", "pets"] {
            assert!(labels.contains(&l.to_string()), "{labels:?}");
        }
        assert!(!labels.contains(&"chair".to_string()), "only person, vehicle, cat and dog are listed: {labels:?}");
        assert!(!labels.contains(&"motion".to_string()));
        // Another period or another camera has none of it.
        assert_eq!(select(&read(&data, cam, now - chrono::Duration::days(10), now - chrono::Duration::days(5), &[], None), &Kind::Motion).detections, 0);
        assert_eq!(select(&read(&data, "back", now - chrono::Duration::hours(2), now + chrono::Duration::hours(1), &[], None), &Kind::Motion).detections, 0);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn alerts_stand_in_for_the_time_before_the_log_began_and_not_after() {
        let _turn = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let data = scratch("fallback");
        let cam = "side";
        let now = Local::now();
        // No log at all: the alerts' boxes are used.
        let evs = [event(now - chrono::Duration::hours(30), vec![("person", 0.4, 0.4, 0.2, 0.2), ("motion", 0.0, 0.0, 0.1, 0.1)])];
        let raw = read(&data, cam, now - chrono::Duration::days(3), now, &evs, None);
        assert_eq!((raw.alerts_used, select(&raw, &Kind::People).detections, select(&raw, &Kind::Motion).detections), (1, 1, 1));
        // The log starts now: an older alert still counts, one from inside the logged time does not (it was logged already).
        motion(cam, now.timestamp_millis(), &[mo(0.5, 0.5, 0.1, 0.1)]);
        flush(&data);
        let evs = [event(now - chrono::Duration::hours(30), vec![("person", 0.4, 0.4, 0.2, 0.2)]), event(now + chrono::Duration::minutes(5), vec![("person", 0.4, 0.4, 0.2, 0.2)])];
        let raw = read(&data, cam, now - chrono::Duration::days(3), now + chrono::Duration::hours(1), &evs, None);
        assert_eq!(select(&raw, &Kind::People).detections, 1, "only the old one");
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn motion_that_nothing_recognised_stands_out_and_motion_with_a_person_in_it_does_not() {
        let mut motion = vec![0u32; COLS * ROWS];
        let mut objects = vec![0u32; COLS * ROWS];
        // A path where people walk (lots of motion, lots of people) and a tree (lots of motion, no one).
        for r in 20..24 {
            for c in 10..30 {
                motion[r * COLS + c] = 400;
                objects[r * COLS + c] = 40;
            }
        }
        for r in 3..8 {
            for c in 45..55 {
                motion[r * COLS + c] = 380;
            }
        }
        let u = unexplained(&motion, &objects);
        let tree = u[5 * COLS + 50];
        let path = u[21 * COLS + 20];
        assert!(tree > 800, "the tree should stand out: {tree}");
        assert!(path < 200, "the path is explained by the people on it: {path}");
        assert_eq!(u[30 * COLS + 5], 0, "nothing where nothing moved");
    }

    #[test]
    fn the_areas_ignored_for_motion_are_left_out_by_default_but_always_logged() {
        let _turn = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let data = scratch("ignore");
        let cam = "yard";
        let now = Local::now();
        let ms = now.timestamp_millis();
        // The top half of the picture is painted as ignored (a swaying branch).
        let mask = "160x90:0,7200,7200";
        let cells = ignore_cells(mask).expect("some cells are ignored");
        assert!(cells[0] && cells[(ROWS / 2 - 1) * COLS], "top is ignored");
        assert!(!cells[(ROWS / 2 + 1) * COLS] && !cells[COLS * ROWS - 1], "bottom is not");
        assert!(ignore_cells("").is_none(), "no mask, nothing ignored");
        motion(cam, ms, &[mo(0.0, 0.0, 0.25, 0.25), mo(0.0, 0.0, 0.25, 0.25), mo(0.6, 0.7, 0.2, 0.2)]);
        flush(&data);
        let (from, to) = (now - chrono::Duration::hours(2), now + chrono::Duration::hours(1));
        // Left out: only the bottom box is there.
        let left_out = select(&read(&data, cam, from, to, &[], Some(&cells)), &Kind::Motion);
        assert_eq!(left_out.detections, 1);
        assert_eq!(left_out.cells[0], 0);
        assert!(left_out.cells.iter().any(|c| *c > 0));
        assert_eq!(left_out.hourly.iter().sum::<u32>(), 1);
        // Two boxes of 16×9 cells inside the ignored area, one of 13×8 outside: 288 of 392 cells.
        assert!((left_out.ignored_share - 288.0 / 392.0).abs() < 0.03, "{}", left_out.ignored_share);
        // Ticked back in: the logged data was never lost.
        let all = select(&read(&data, cam, from, to, &[], None), &Kind::Motion);
        assert_eq!(all.detections, 3);
        assert_eq!(all.cells[0], 2);
        assert_eq!(all.hourly.iter().sum::<u32>(), 3);
        assert_eq!(all.ignored_share, 0.0, "nothing was marked when nothing is left out");
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn kinds_are_parsed_strictly() {
        assert_eq!(Kind::parse(""), Some(Kind::Motion));
        assert_eq!(Kind::parse("people"), Some(Kind::People));
        assert_eq!(Kind::parse("label:Cat"), Some(Kind::Label("cat".into())));
        assert_eq!(Kind::parse("label:sports ball"), Some(Kind::Label("sports ball".into())));
        for bad in ["nope", "label:", "label:motion", "label:../x", "label:a/b", &format!("label:{}", "x".repeat(60))] {
            assert_eq!(Kind::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn old_day_files_are_removed_and_recent_ones_kept() {
        let data = scratch("tidy");
        let d = data.join("heat").join("cam");
        std::fs::create_dir_all(&d).unwrap();
        let today = Local::now().date_naive();
        for age in [0, 5, KEEP_DAYS + 3] {
            std::fs::write(d.join(format!("{}.json", today - chrono::Duration::days(age))), "{}").unwrap();
        }
        tidy(&data);
        let left = std::fs::read_dir(&d).unwrap().count();
        assert_eq!(left, 2);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn scaling_uses_a_high_level_not_the_single_hottest_cell() {
        let mut cells = vec![0u32; COLS * ROWS];
        for (i, c) in cells.iter_mut().enumerate().take(200) {
            *c = 10 + (i % 5) as u32;
        }
        cells[500] = 100_000; // one extreme cell
        let p = p95(&cells);
        assert!(p < 20, "one extreme cell must not set the scale: {p}");
    }
}
