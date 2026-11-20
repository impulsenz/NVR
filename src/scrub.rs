//! Time-lapse previews for scrubbing the timeline (what shows while you drag
//! along it, and the blue line under each camera).
//!
//! The pictures are kept **per camera and per clock hour on a fixed grid**:
//! hour `h` (a multiple of 3 600 000 ms) holds slot `n` = the picture nearest
//! `h + n × interval`. Nothing depends on which recording file a moment is in,
//! so the page needs one lookup per moment (`Meta::nearest`) and the server has
//! one job: fill the grid. Pictures come from two places:
//!
//! 1. **As it records**: motion detection keeps a small picture every
//!    interval (`picturelog`); every 15 s they are put into the current hour.
//! 2. **Older footage** (before this existed, or while GAZE was off): one
//!    recording at a time, newest first, only while the computer is quiet,
//!    read from the file by ffmpeg and put into the same grid.
//!
//! Files: `<camera>/.scrub/<h>.json` (a [`Meta`]: which slots hold a picture,
//! a version for each page) and `<h>-<n>.jpg`: pages of 10 × 10 pictures. A
//! page is written under a temporary name and moved into place; the meta last,
//! so a page is only ever announced once it is complete. Recording never
//! depends on any of this.

use crate::manager::{CameraRuntime, Manager};
use crate::sync::MutexExt;
use anyhow::{Context, Result};
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const HOUR_MS: i64 = 3_600_000;
/// Pictures per row, and per page, of a page.
pub const COLS: u32 = 10;
pub const PER_PAGE: u32 = 100;

/// What an hour holds (also what the page is told).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Meta {
    /// Start of the hour, ms since 1970.
    pub h: i64,
    /// Seconds between pictures.
    pub interval: u32,
    pub tw: u32,
    pub th: u32,
    pub slots: u32,
    pub pages: u32,
    /// The recording's own picture shape (width ÷ height).
    pub aspect: f64,
    /// Slots that hold a picture, as inclusive ranges.
    pub have: Vec<[u32; 2]>,
    /// Version of each page (changes when the page is written again).
    pub pv: Vec<u64>,
}

impl Meta {
    fn new(h: i64, interval: u32, tile: (u32, u32), aspect: f64) -> Meta {
        let slots = (3600 / interval.max(1)).max(1);
        Meta { h, interval, tw: tile.0, th: tile.1, slots, pages: slots.div_ceil(PER_PAGE), aspect, have: Vec::new(), pv: vec![0; slots.div_ceil(PER_PAGE) as usize] }
    }

    /// The slot a moment falls in.
    pub fn slot_of(&self, ms: i64) -> u32 {
        (((ms - self.h).max(0) / (self.interval as i64 * 1000)) as u32).min(self.slots - 1)
    }

    pub fn has(&self, slot: u32) -> bool {
        let i = self.have.partition_point(|r| r[1] < slot);
        self.have.get(i).is_some_and(|r| r[0] <= slot)
    }

    /// The slot holding a picture nearest to `slot` (None: the hour is empty).
    #[cfg(test)]
    pub fn nearest(&self, slot: u32) -> Option<u32> {
        let i = self.have.partition_point(|r| r[1] < slot);
        let after = self.have.get(i).map(|r| r[0].max(slot));
        let before = i.checked_sub(1).and_then(|j| self.have.get(j)).map(|r| r[1]);
        match (before, after) {
            (Some(b), Some(a)) => Some(if slot - b <= a.saturating_sub(slot) { b } else { a }),
            (b, a) => b.or(a),
        }
    }

    fn set_have(&mut self, tiles: &[Option<Arc<Vec<u8>>>]) {
        self.have.clear();
        for (i, t) in tiles.iter().enumerate() {
            if t.is_none() {
                continue;
            }
            let i = i as u32;
            match self.have.last_mut() {
                Some(r) if r[1] + 1 == i => r[1] = i,
                _ => self.have.push([i, i]),
            }
        }
    }

    #[cfg(test)]
    pub fn count(&self) -> u32 {
        self.have.iter().map(|r| r[1] - r[0] + 1).sum()
    }
}

/// Forget what was read from disk (after the pictures were removed).
pub fn forget() {
    METAS.locked().take();
    OPENS.locked().take();
}

pub fn hour_of(ms: i64) -> i64 {
    ms.div_euclid(HOUR_MS) * HOUR_MS
}

pub fn dir(camera_dir: &Path) -> PathBuf {
    camera_dir.join(".scrub")
}
fn meta_path(camera_dir: &Path, h: i64) -> PathBuf {
    dir(camera_dir).join(format!("{h}.json"))
}
pub fn page_path(camera_dir: &Path, h: i64, n: u32) -> PathBuf {
    dir(camera_dir).join(format!("{h}-{n}.jpg"))
}

// ---------- what the page asks: the hours that exist ----------

/// Hours already read (a camera's id and the hour), `None` for one found empty a moment ago.
type MetaCache = HashMap<(String, i64), (Instant, Option<Arc<Meta>>)>;
static METAS: Mutex<Option<MetaCache>> = Mutex::new(None);

fn cache_put(camera: &str, m: &Meta) {
    METAS.locked().get_or_insert_with(HashMap::new).insert((camera.to_string(), m.h), (Instant::now(), Some(Arc::new(m.clone()))));
}

/// One hour's meta (remembered: the page asks every few seconds).
pub fn meta(camera: &str, camera_dir: &Path, h: i64) -> Option<Arc<Meta>> {
    let key = (camera.to_string(), h);
    if let Some((at, m)) = METAS.locked().as_ref().and_then(|c| c.get(&key)).cloned()
        && (m.is_some() || at.elapsed() < Duration::from_secs(20))
    {
        return m;
    }
    let m = std::fs::read(meta_path(camera_dir, h)).ok().and_then(|b| serde_json::from_slice::<Meta>(&b).ok()).filter(|m| m.interval > 0 && m.slots > 0).map(Arc::new);
    let mut c = METAS.locked();
    let c = c.get_or_insert_with(HashMap::new);
    if c.len() > 20_000 {
        c.retain(|_, (t, _)| t.elapsed() < Duration::from_secs(60));
    }
    c.insert(key, (Instant::now(), m.clone()));
    m
}

/// The hours from `from` to `to` (ms) that hold pictures, oldest first.
pub fn list(camera: &str, camera_dir: &Path, from: i64, to: i64) -> Vec<Arc<Meta>> {
    let (a, b) = (hour_of(from), hour_of(to.min(from + 62 * 24 * HOUR_MS)));
    (0..=((b - a) / HOUR_MS)).filter_map(|i| meta(camera, camera_dir, a + i * HOUR_MS)).filter(|m| !m.have.is_empty()).collect()
}

// ---------- an hour being filled ----------

/// An hour's pictures, held as small JPEGs while it is being filled.
struct Open {
    meta: Meta,
    tiles: Vec<Option<Arc<Vec<u8>>>>,
    dirty: BTreeSet<u32>,
}

impl Open {
    fn new(h: i64, interval: u32, tile: (u32, u32), aspect: f64) -> Open {
        let meta = Meta::new(h, interval, tile, aspect);
        Open { tiles: vec![None; meta.slots as usize], meta, dirty: BTreeSet::new() }
    }

    /// An hour as it is on disk (so more pictures can be added to it).
    fn load(camera_dir: &Path, meta: &Meta) -> Result<Open> {
        let mut o = Open { tiles: vec![None; meta.slots as usize], meta: meta.clone(), dirty: BTreeSet::new() };
        let (tw, th) = (meta.tw, meta.th);
        for n in 0..meta.pages {
            if !meta.have.iter().any(|r| r[1] / PER_PAGE >= n && r[0] / PER_PAGE <= n) {
                continue;
            }
            let f = crate::imgutil::decode_jpeg(&std::fs::read(page_path(camera_dir, meta.h, n)).context("a page is missing")?)?;
            for k in 0..PER_PAGE {
                let slot = n * PER_PAGE + k;
                if slot >= meta.slots || !meta.has(slot) {
                    continue;
                }
                let (cx, cy) = ((k % COLS) * tw, (k / COLS) * th);
                if cx + tw > f.width || cy + th > f.height {
                    continue;
                }
                let mut rgb = Vec::with_capacity((tw * th * 3) as usize);
                for y in 0..th {
                    let s = (((cy + y) * f.width + cx) * 3) as usize;
                    rgb.extend_from_slice(&f.rgb[s..s + (tw * 3) as usize]);
                }
                o.tiles[slot as usize] = Some(Arc::new(crate::imgutil::encode_jpeg(&rgb, tw, th, 85)?));
            }
        }
        Ok(o)
    }

    /// Put a picture (a JPEG of any size) into the slot nearest `ms`. False if that slot already has one.
    fn put(&mut self, ms: i64, jpeg: &[u8]) -> bool {
        let slot = self.meta.slot_of(ms);
        if self.tiles[slot as usize].is_some() {
            return false;
        }
        let Ok(f) = crate::imgutil::decode_jpeg(jpeg) else { return false };
        let (tw, th) = (self.meta.tw, self.meta.th);
        let jpg = if (f.width, f.height) == (tw, th) {
            jpeg.to_vec()
        } else if let Ok(j) = f.resize(tw, th).jpeg(80) {
            j
        } else {
            return false;
        };
        self.tiles[slot as usize] = Some(Arc::new(jpg));
        self.dirty.insert(slot / PER_PAGE);
        true
    }

    /// Write the pages that changed, then the meta.
    fn flush(&mut self, camera: &str, camera_dir: &Path) -> Result<()> {
        if self.dirty.is_empty() {
            return Ok(());
        }
        std::fs::create_dir_all(dir(camera_dir))?;
        let (tw, th) = (self.meta.tw, self.meta.th);
        for n in std::mem::take(&mut self.dirty) {
            let first = n * PER_PAGE;
            let in_page = PER_PAGE.min(self.meta.slots - first);
            let (w, h) = (COLS * tw, in_page.div_ceil(COLS) * th);
            let mut img = vec![0u8; (w * h * 3) as usize];
            for k in 0..in_page {
                let Some(t) = &self.tiles[(first + k) as usize] else { continue };
                let Ok(f) = crate::imgutil::decode_jpeg(t) else { continue };
                if (f.width, f.height) != (tw, th) {
                    continue;
                }
                let (cx, cy) = ((k % COLS) * tw, (k / COLS) * th);
                for y in 0..th {
                    let d = (((cy + y) * w + cx) * 3) as usize;
                    let s = (y * tw * 3) as usize;
                    img[d..d + (tw * 3) as usize].copy_from_slice(&f.rgb[s..s + (tw * 3) as usize]);
                }
            }
            let jpg = crate::imgutil::encode_jpeg(&img, w, h, 70)?;
            let p = page_path(camera_dir, self.meta.h, n);
            let tmp = p.with_extension("jpg.tmp");
            std::fs::write(&tmp, jpg)?;
            std::fs::rename(&tmp, &p)?;
            let v = &mut self.meta.pv[n as usize];
            *v = (*v + 1).max(Local::now().timestamp_millis() as u64);
        }
        self.meta.set_have(&self.tiles);
        let mp = meta_path(camera_dir, self.meta.h);
        let tmp = mp.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(&self.meta)?)?;
        std::fs::rename(&tmp, &mp)?;
        cache_put(camera, &self.meta);
        Ok(())
    }
}

/// Hours being filled (camera id, hour). Only the builder touches these.
static OPENS: Mutex<Option<HashMap<(String, i64), Open>>> = Mutex::new(None);

/// Run `f` on an hour being filled: the one in memory, or loaded from disk, or a new one.
fn with_hour<R>(camera: &str, camera_dir: &Path, h: i64, interval: u32, aspect: f64, f: impl FnOnce(&mut Open) -> R) -> R {
    let mut g = OPENS.locked();
    let opens = g.get_or_insert_with(HashMap::new);
    let o = opens.entry((camera.to_string(), h)).or_insert_with(|| match meta(camera, camera_dir, h) {
        Some(m) => Open::load(camera_dir, &m).unwrap_or_else(|_| Open::new(h, interval, crate::picturelog::tile(), aspect)),
        None => Open::new(h, interval, crate::picturelog::tile(), aspect),
    });
    f(o)
}

/// Hours long past are written out and let go of (they are loaded again if more is added).
fn release_old(now_ms: i64) {
    if let Some(o) = OPENS.locked().as_mut() {
        o.retain(|(_, h), op| *h >= hour_of(now_ms) - HOUR_MS || !op.dirty.is_empty());
    }
}

// ---------- 1. as it records ----------

/// Put the pictures motion detection kept into their hours. Returns how many were new.
fn ingest_live(camera: &str, camera_dir: &Path, interval: u32, aspect: f64, now_ms: i64) -> usize {
    let mut n = 0;
    let from = now_ms - 6 * HOUR_MS;
    for p in crate::picturelog::between(camera_dir, from, now_ms + 60_000) {
        let Some(t) = p.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<i64>().ok()) else { continue };
        let h = hour_of(t);
        // Cheap check first: most pictures were put in earlier.
        if meta(camera, camera_dir, h).is_some_and(|m| m.has(m.slot_of(t))) && !OPENS.locked().as_ref().is_some_and(|o| o.contains_key(&(camera.to_string(), h))) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&p) else { continue };
        if with_hour(camera, camera_dir, h, interval, aspect, |o| o.put(t, &bytes)) {
            n += 1;
        }
    }
    n
}

fn flush_all(camera: &str, camera_dir: &Path) {
    let mut g = OPENS.locked();
    for ((c, _), o) in g.get_or_insert_with(HashMap::new).iter_mut() {
        if c == camera
            && let Err(e) = o.flush(camera, camera_dir)
        {
            tracing::warn!("time-lapse previews: could not save an hour ({e:#})");
        }
    }
}

// ---------- 2. older footage ----------

/// Is the footage from `start` to `end` already pictured (all but a few moments)?
fn covered(camera: &str, camera_dir: &Path, start: i64, end: i64) -> bool {
    let (mut want, mut got) = (0u32, 0u32);
    for h in (hour_of(start)..=hour_of(end)).step_by(HOUR_MS as usize) {
        let m = meta(camera, camera_dir, h);
        let step = m.as_ref().map_or(crate::picturelog::seconds(), |m| m.interval) as i64 * 1000;
        let (a, b) = (start.max(h), end.min(h + HOUR_MS));
        let mut t = a;
        while t < b {
            want += 1;
            if m.as_ref().is_some_and(|m| m.has(m.slot_of(t))) {
                got += 1;
            }
            t += step;
        }
    }
    want == 0 || got as f64 >= want as f64 * 0.9
}

/// The pictures of a sprite page file as separate tiles (RGB).
fn page_tiles(page: &Path, tw: u32, th: u32, count: usize) -> Result<Vec<Vec<u8>>> {
    let f = crate::imgutil::decode_jpeg(&std::fs::read(page)?)?;
    let mut out = Vec::new();
    for k in 0..count as u32 {
        let (cx, cy) = ((k % COLS) * tw, (k / COLS) * th);
        if cx + tw > f.width || cy + th > f.height {
            break;
        }
        let mut rgb = Vec::with_capacity((tw * th * 3) as usize);
        for y in 0..th {
            let s = (((cy + y) * f.width + cx) * 3) as usize;
            rgb.extend_from_slice(&f.rgb[s..s + (tw * 3) as usize]);
        }
        out.push(rgb);
    }
    Ok(out)
}

/// One recording file read by ffmpeg and put into its hours.
async fn fill_from_file(m: &Manager, c: &CameraRuntime, file: &Path, start_ms: i64) -> Result<usize> {
    let cfg = m.config();
    let staging = dir(&c.dir).join(format!("building-{}", c.cfg.id));
    std::fs::create_dir_all(dir(&c.dir))?;
    let hw = crate::ffmpeg::decode_args(&cfg.hardware, false);
    let info = crate::ffmpeg::sprite(&cfg.server.ffmpeg_path, file, &staging, crate::picturelog::seconds() as f64, &hw).await?;
    let (cam_id, cam_dir, interval) = (c.cfg.id.clone(), c.dir.clone(), crate::picturelog::seconds());
    let added = tokio::task::spawn_blocking(move || -> Result<usize> {
        let mut added = 0;
        let mut i = 0u32;
        for n in 0..info.pages {
            let page = PathBuf::from(format!("{}-{n:03}.jpg", staging.display()));
            let left = (info.count - n * info.per_page).min(info.per_page) as usize;
            let tiles = page_tiles(&page, info.tile_width, info.tile_height, left);
            let _ = std::fs::remove_file(&page);
            for rgb in tiles? {
                let t = start_ms + (i as f64 * info.interval * 1000.0) as i64;
                i += 1;
                let jpg = crate::imgutil::encode_jpeg(&rgb, info.tile_width, info.tile_height, 80)?;
                if with_hour(&cam_id, &cam_dir, hour_of(t), interval, info.aspect, |o| o.put(t, &jpg)) {
                    added += 1;
                }
            }
        }
        flush_all(&cam_id, &cam_dir);
        Ok(added)
    })
    .await??;
    Ok(added)
}

/// Footage worth picturing: the light timeline copies, and the recordings of any time without one.
fn candidates(c: &CameraRuntime) -> Vec<(&'static str, crate::storage::Recording)> {
    let copies = crate::library::timeline_recordings(c);
    let mut v: Vec<(&'static str, crate::storage::Recording)> = copies.iter().cloned().map(|r| ("tl", r)).collect();
    for r in crate::library::recordings(c) {
        let spanned = copies.iter().any(|t| t.start <= r.start + chrono::Duration::seconds(5) && t.end >= r.end - chrono::Duration::seconds(5));
        if !spanned {
            v.push(("rec", r));
        }
    }
    v.sort_by_key(|(_, r)| std::cmp::Reverse(r.start));
    v
}

fn file_of(c: &CameraRuntime, kind: &str, name: &str) -> Result<PathBuf> {
    if kind == "tl" { crate::storage::resolve(&crate::storage::timeline_dir(&c.dir), name) } else { crate::storage::resolve(&c.dir, name) }
}

// ---------- the builder ----------

/// "Rebuild previews" (System → Tasks): look for older footage now, even if it is busy.
pub static NOW: tokio::sync::Notify = tokio::sync::Notify::const_new();
static FORCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn rebuild_now() {
    FORCE.store(true, std::sync::atomic::Ordering::SeqCst);
    NOW.notify_one();
}

/// Runs for as long as GAZE does.
pub async fn run(m: Arc<Manager>) {
    tokio::time::sleep(Duration::from_secs(10)).await;
    // File → (tries, when, size it had): a file that gave nothing isn't tried again at once.
    let mut failed: HashMap<String, (u32, Instant, u64)> = HashMap::new();
    let mut aspects: HashMap<String, (Instant, f64)> = HashMap::new();
    let mut last_back = Instant::now() - Duration::from_secs(3600);
    let mut last_prune = Instant::now() - Duration::from_secs(3600);
    loop {
        if !crate::picturelog::enabled() {
            tokio::select! { _ = tokio::time::sleep(Duration::from_secs(60)) => {}, _ = NOW.notified() => {} }
            continue;
        }
        let force = FORCE.swap(false, std::sync::atomic::Ordering::SeqCst);
        let interval = crate::picturelog::seconds();
        let cams = m.cameras();
        // 1. As it records.
        for c in &cams {
            let asp = match aspects.get(&c.cfg.id) {
                Some((t, a)) if t.elapsed() < Duration::from_secs(1800) => *a,
                _ => {
                    let a = match candidates(c).first() {
                        Some((k, r)) => match file_of(c, k, &r.name) {
                            Ok(p) => crate::ffmpeg::display_aspect(&m.config().server.ffmpeg_path, &p).await.unwrap_or(16.0 / 9.0),
                            Err(_) => 16.0 / 9.0,
                        },
                        None => 16.0 / 9.0,
                    };
                    aspects.insert(c.cfg.id.clone(), (Instant::now(), a));
                    a
                }
            };
            let (id, dir) = (c.cfg.id.clone(), c.dir.clone());
            let _ = tokio::task::spawn_blocking(move || {
                let now = Local::now().timestamp_millis();
                ingest_live(&id, &dir, interval, asp, now);
                flush_all(&id, &dir);
                release_old(now);
            })
            .await;
        }
        // 2. Older footage: a short batch when it is quiet (or when asked), newest first.
        if force || last_back.elapsed() >= Duration::from_secs(60) {
            last_back = Instant::now();
            let mut todo = Vec::new();
            for c in &cams {
                let cd = c.dir.clone();
                let id = c.cfg.id.clone();
                let list = candidates(c);
                let now = Local::now();
                let found = tokio::task::spawn_blocking(move || {
                    list.into_iter()
                        .filter(|(_, r)| (now - r.end).num_seconds() >= 30 && !covered(&id, &cd, r.start.timestamp_millis(), r.end.timestamp_millis()))
                        .collect::<Vec<_>>()
                })
                .await
                .unwrap_or_default();
                todo.extend(found.into_iter().map(|(k, r)| (c.clone(), k, r)));
            }
            todo.sort_by_key(|(_, _, r)| std::cmp::Reverse(r.start));
            todo.retain(|(c, _, r)| {
                failed
                    .get(&format!("{}/{}", c.cfg.id, r.name))
                    .is_none_or(|(n, at, size)| force || *size != r.size || (*n < 3 && at.elapsed() >= Duration::from_secs(300u64 << (n - 1).min(5))))
            });
            if !todo.is_empty() {
                let total = todo.len();
                if !force {
                    todo.truncate(40);
                }
                let j = crate::jobs::start("previews", format!("Time-lapse previews: {total} recording(s) to picture"));
                j.total(todo.len() as u64);
                for (c, kind, r) in todo {
                    j.waiting();
                    if !force {
                        crate::load::wait_for_cpu(82, Duration::from_secs(120)).await;
                        if crate::load::cpu() > 94 {
                            break;
                        }
                    }
                    j.pause_point().await;
                    if crate::vstore::watching() {
                        tokio::time::sleep(Duration::from_secs(3)).await;
                    }
                    j.step(format!("{} — {}", c.cfg.name, r.start.format("%d/%m %H:%M")));
                    let key = format!("{}/{}", c.cfg.id, r.name);
                    let res = match file_of(&c, kind, &r.name) {
                        Ok(p) => fill_from_file(&m, &c, &p, r.start.timestamp_millis()).await,
                        Err(e) => Err(e),
                    };
                    match res {
                        Ok(n) if n > 0 => {
                            failed.remove(&key);
                            j.logged(format!("{} — {}: {n} pictures", c.cfg.name, r.start.format("%d/%m %H:%M")));
                            let h = hour_of(r.start.timestamp_millis());
                            if let Some(mt) = meta(&c.cfg.id, &c.dir, h) {
                                j.preview(serde_json::json!({
                                    "url": format!("media/{}/scrub/{h}/0?v={}", c.cfg.id, mt.pv.first().copied().unwrap_or(0)),
                                    "columns": COLS, "count": mt.have.first().map_or(1, |r| (r[1] - r[0] + 1).min(PER_PAGE)),
                                    "tile": [mt.tw, mt.th], "camera": c.cfg.name, "from": r.start, "every": mt.interval,
                                }));
                            }
                        }
                        other => {
                            let tries = failed.get(&key).map_or(0, |x| x.0) + 1;
                            if let Err(e) = other {
                                tracing::debug!("time-lapse previews: {key}: {e:#}");
                            }
                            failed.insert(key, (tries, Instant::now(), r.size));
                        }
                    }
                    j.done_one();
                }
            }
            if failed.len() > 20_000 {
                failed.clear();
            }
        }
        // 3. Pictures of hours no footage remains for.
        if last_prune.elapsed() >= Duration::from_secs(3600) {
            last_prune = Instant::now();
            for c in &cams {
                let oldest = candidates(c).last().map(|(_, r)| r.start.timestamp_millis());
                // Footage in deep storage keeps its pictures too.
                let oldest = crate::storage::list_archived(&c.dir).iter().map(|r| r.start.timestamp_millis()).chain(oldest).min();
                if let Some(o) = oldest {
                    let cd = c.dir.clone();
                    let _ = tokio::task::spawn_blocking(move || prune(&cd, hour_of(o) - HOUR_MS)).await;
                }
            }
        }
        tokio::select! { _ = tokio::time::sleep(Duration::from_secs(15)) => {}, _ = NOW.notified() => {} }
    }
}

/// Remove hours before `keep_from` (the oldest footage there still is).
fn prune(camera_dir: &Path, keep_from: i64) {
    let Ok(rd) = std::fs::read_dir(dir(camera_dir)) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(h) = name.split(['.', '-']).next().and_then(|s| s.parse::<i64>().ok()) else { continue };
        if h < keep_from {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// What the Recordings page is told about a camera.
pub fn describe(camera: &str, camera_dir: &Path, from: i64, to: i64) -> serde_json::Value {
    serde_json::json!({ "cols": COLS, "per_page": PER_PAGE, "hours": list(camera, camera_dir, from, to).iter().map(|m| (**m).clone()).collect::<Vec<Meta>>() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile_jpeg(v: u8, w: u32, h: u32) -> Vec<u8> {
        // (A picture with a gradient: not flat, so it survives like a real one.)
        let rgb: Vec<u8> = (0..w * h * 3).map(|i| v.wrapping_add((i % 50) as u8)).collect();
        crate::imgutil::encode_jpeg(&rgb, w, h, 80).unwrap()
    }

    #[test]
    fn slots_and_the_nearest_picture() {
        let mut m = Meta::new(0, 10, (64, 36), 16.0 / 9.0);
        assert_eq!((m.slots, m.pages), (360, 4));
        assert_eq!(m.slot_of(0), 0);
        assert_eq!(m.slot_of(9_999), 0);
        assert_eq!(m.slot_of(10_000), 1);
        assert_eq!(m.slot_of(HOUR_MS + 5), 359, "never past the hour");
        assert_eq!(m.nearest(5), None);
        m.have = vec![[10, 20], [100, 100]];
        assert!(m.has(10) && m.has(20) && m.has(100) && !m.has(21) && !m.has(9));
        assert_eq!(m.nearest(15), Some(15));
        assert_eq!(m.nearest(0), Some(10), "before the first");
        assert_eq!(m.nearest(300), Some(100), "after the last");
        assert_eq!(m.nearest(25), Some(20));
        assert_eq!(m.nearest(95), Some(100));
        assert_eq!(m.nearest(60), Some(20), "a tie goes to the earlier one");
        assert_eq!(m.count(), 12);
    }

    #[test]
    fn an_hour_is_written_read_back_and_added_to() {
        let cam = std::env::temp_dir().join(format!("nvr-scrub-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cam);
        let h = hour_of(1_700_000_000_000);
        let mut o = Open::new(h, 10, (64, 36), 16.0 / 9.0);
        // Slots 0, 1, 2 and 150 (a page further on).
        for (i, s) in [0i64, 1, 2, 150].into_iter().enumerate() {
            assert!(o.put(h + s * 10_000 + 3_000, &tile_jpeg(i as u8 * 40, 64, 36)));
        }
        assert!(!o.put(h + 3_000, &tile_jpeg(9, 64, 36)), "a slot holds the first picture");
        // A picture of another size is fitted to the grid.
        assert!(o.put(h + 200 * 10_000, &tile_jpeg(5, 100, 50)));
        o.flush("cam", &cam).unwrap();
        let m = meta("cam", &cam, h).unwrap();
        assert_eq!(m.have, vec![[0, 2], [150, 150], [200, 200]]);
        assert!(m.pv[0] > 0 && m.pv[1] > 0 && m.pv[2] > 0 && m.pv[3] == 0, "only pages with pictures are written: {:?}", m.pv);
        assert!(page_path(&cam, h, 0).is_file() && page_path(&cam, h, 1).is_file() && !page_path(&cam, h, 3).exists());
        let f = crate::imgutil::decode_jpeg(&std::fs::read(page_path(&cam, h, 0)).unwrap()).unwrap();
        assert_eq!((f.width, f.height), (640, 360), "100 pictures: 10 × 10");
        // Loaded again (after a restart) and added to: the old pictures stay.
        let from_disk = serde_json::from_slice::<Meta>(&std::fs::read(meta_path(&cam, h)).unwrap()).unwrap();
        let mut again = Open::load(&cam, &from_disk).unwrap();
        assert_eq!(again.tiles.iter().filter(|t| t.is_some()).count(), 5);
        assert!(again.put(h + 3 * 10_000, &tile_jpeg(90, 64, 36)));
        let before = m.pv[0];
        again.flush("cam", &cam).unwrap();
        let m2 = meta("cam", &cam, h).unwrap();
        assert_eq!(m2.have, vec![[0, 3], [150, 150], [200, 200]]);
        assert!(m2.pv[0] > before, "a page written again has a new version");
        assert_eq!(m2.pv[1], m.pv[1], "an untouched page keeps its version");
        // The page asks for hours in a range.
        assert_eq!(list("cam", &cam, h + 5, h + 100).len(), 1);
        assert!(list("cam", &cam, h + HOUR_MS * 3, h + HOUR_MS * 4).is_empty());
        // Hours long gone are removed.
        prune(&cam, h + HOUR_MS);
        assert!(!meta_path(&cam, h).exists() && !page_path(&cam, h, 0).exists());
        let _ = std::fs::remove_dir_all(&cam);
    }

    #[test]
    fn footage_counts_as_pictured_when_nearly_all_of_it_is() {
        let cam = std::env::temp_dir().join(format!("nvr-scrub-cov-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cam);
        let h = hour_of(1_700_100_000_000);
        let mut o = Open::new(h, 10, (64, 36), 1.78);
        // The first 10 minutes pictured.
        for s in 0..60 {
            o.put(h + s * 10_000, &tile_jpeg(s as u8, 64, 36));
        }
        o.flush("covcam", &cam).unwrap();
        assert!(covered("covcam", &cam, h, h + 600_000 - 1));
        assert!(!covered("covcam", &cam, h, h + 1_200_000), "the next 10 minutes are not");
        assert!(!covered("covcam", &cam, h + HOUR_MS, h + HOUR_MS + 60_000), "no hour at all");
        let _ = std::fs::remove_dir_all(&cam);
    }
}
