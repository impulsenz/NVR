//! Recording files on disk: listing, usage statistics and retention clean-up.
//!
//! Layout per camera folder (`<storage dir>/<camera id>` or the camera's own folder):
//!   `<id>_YYYY-MM-DD_HH-MM-SS.mp4`   continuous recordings
//!   `events/`                         event thumbnails, clips and events.jsonl
//!   `.buffer/`                        pre-event buffer (short-lived)
//!   `.thumbs/`                        cached previews

use crate::config::Config;
use crate::events::EventStore;
use crate::sync::MutexExt;
use anyhow::{Result, bail};
use chrono::{DateTime, Duration as CDuration, Local, NaiveDateTime, TimeZone};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize)]
pub struct Recording {
    pub name: String,
    pub size: u64,
    /// Start time parsed from the file name (falls back to modification time).
    pub start: DateTime<Local>,
    /// Last write (≈ end of the recording).
    pub end: DateTime<Local>,
}

/// A camera's folder name: any name that's one safe folder on every system
/// (no path separators or reserved characters, nothing hidden or "..").
pub fn valid_folder(s: &str) -> bool {
    !s.is_empty() && s.len() <= 120 && !s.starts_with('.') && !s.ends_with([' ', '.']) && !s.chars().any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
}

pub fn valid_component(s: &str) -> bool {
    !s.is_empty() && !s.starts_with('.') && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

/// All continuous recordings in one camera folder, newest first.
/// The recordings in a folder, newest first. Kept for a few seconds:
/// reading the details of thousands of files takes a while (on Windows
/// especially), and pages ask for them often (alerts, timelines).
type ListCache = HashMap<PathBuf, (std::time::Instant, std::sync::Arc<Vec<Recording>>)>;
static LIST_CACHE: std::sync::Mutex<Option<ListCache>> = std::sync::Mutex::new(None);

pub fn list(dir: &Path) -> Vec<Recording> {
    {
        let mut c = LIST_CACHE.locked();
        let map = c.get_or_insert_with(HashMap::new);
        if let Some((t, v)) = map.get(dir)
            && t.elapsed() < std::time::Duration::from_secs(5)
        {
            return (**v).clone();
        }
    }
    let v = list_now(dir);
    let mut c = LIST_CACHE.locked();
    let map = c.get_or_insert_with(HashMap::new);
    map.retain(|_, (t, _)| t.elapsed() < std::time::Duration::from_secs(60));
    map.insert(dir.to_path_buf(), (std::time::Instant::now(), std::sync::Arc::new(v.clone())));
    v
}

/// The names of the files in a folder (one listing, kept a few seconds):
/// cheaper than checking files one by one on a network drive.
type NamesCache = HashMap<PathBuf, (std::time::Instant, Arc<std::collections::HashSet<String>>)>;
static NAMES_CACHE: std::sync::LazyLock<std::sync::Mutex<NamesCache>> = std::sync::LazyLock::new(Default::default);

pub fn names_in(dir: &Path) -> std::sync::Arc<std::collections::HashSet<String>> {
    use std::collections::HashSet;
    if let Some((t, v)) = NAMES_CACHE.locked().get(dir)
        && t.elapsed() < std::time::Duration::from_secs(5)
    {
        return v.clone();
    }
    let v: Arc<HashSet<String>> = Arc::new(std::fs::read_dir(dir).map(|rd| rd.flatten().filter_map(|e| e.file_name().into_string().ok()).collect()).unwrap_or_default());
    let mut c = NAMES_CACHE.locked();
    c.retain(|_, (t, _)| t.elapsed() < std::time::Duration::from_secs(60));
    c.insert(dir.to_path_buf(), (std::time::Instant::now(), v.clone()));
    v
}

/// A recording was deleted: it is taken out of what is remembered of its folder (the other folders, and the rest of
/// this one, stay as they are: reading thousands of files again for every deleted one made everything slow).
fn list_removed(dir: &Path, name: &str) {
    if let Some((_, v)) = LIST_CACHE.locked().get_or_insert_with(HashMap::new).get_mut(dir) {
        *v = Arc::new(v.iter().filter(|r| r.name != name).cloned().collect());
    }
    if let Some((_, v)) = NAMES_CACHE.locked().get_mut(dir) {
        let mut n = (**v).clone();
        n.remove(name);
        *v = Arc::new(n);
    }
}

/// The recordings in a folder, read now.
fn list_now(dir: &Path) -> Vec<Recording> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Recording> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            if !name.ends_with(".mp4") || name.starts_with('.') {
                return None;
            }
            let meta = e.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            let mt = meta.modified().ok()?;
            let modified: DateTime<Local> = mt.into();
            let start = parse_start(&name).unwrap_or(modified);
            Some(Recording { name, size: meta.len(), start, end: modified.max(start) })
        })
        .collect();
    out.sort_by(|a, b| b.start.cmp(&a.start).then(b.name.cmp(&a.name)));
    // The listing tells each finished file's size and time: the video
    // reader needn't ask the disk again (see vstore). Not the newest, nor
    // anything changed lately: a file being recorded can be listed with an
    // old size on Windows.
    let recent = Local::now() - chrono::Duration::minutes(3);
    for r in out.iter().skip(1).filter(|r| r.end < recent) {
        crate::vstore::seed(&dir.join(&r.name), r.size, std::time::SystemTime::from(r.end));
    }
    out
}

/// File names end in `_YYYY-MM-DD_HH-MM-SS.mp4`.
pub fn parse_start(name: &str) -> Option<DateTime<Local>> {
    let stem = name.strip_suffix(".mp4")?;
    let ts = stem.get(stem.len().checked_sub(19)?..)?;
    let naive = NaiveDateTime::parse_from_str(ts, "%Y-%m-%d_%H-%M-%S").ok()?;
    Local.from_local_datetime(&naive).earliest()
}

/// Where each camera's older footage went (deep storage): its folder on the
/// local disk → its folder in deep storage. Kept up to date by the timeline
/// and the mover (see archive.rs).
static ARCHIVES: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<PathBuf, PathBuf>>> = std::sync::LazyLock::new(Default::default);

pub fn set_archive(local: &Path, deep: Option<PathBuf>) {
    let mut m = ARCHIVES.locked();
    match deep {
        Some(d) => {
            m.insert(local.to_path_buf(), d);
        }
        None => {
            m.remove(local);
        }
    }
}

/// The deep storage folder of a camera's local folder, if it has one.
pub fn archive_of(local: &Path) -> Option<PathBuf> {
    ARCHIVES.locked().get(local).cloned()
}

/// The recordings that moved to deep storage (newest first).
pub fn list_archived(local: &Path) -> Vec<Recording> {
    archive_of(local).map(|d| list(&d)).unwrap_or_default()
}

/// Resolve a user-supplied file name to a path inside `dir` — or, if it's
/// not there (it moved to deep storage), inside the camera's deep storage.
pub fn resolve(dir: &Path, name: &str) -> Result<PathBuf> {
    if !valid_component(name) {
        bail!("invalid file name");
    }
    let path = dir.join(name);
    if path.is_file() {
        return Ok(path);
    }
    if let Some(a) = archive_of(dir) {
        let p = a.join(name);
        if p.is_file() {
            return Ok(p);
        }
    }
    bail!("file not found")
}

/// Space use and recording rate of one camera.
#[derive(Debug, Clone, Serialize, Default)]
pub struct CameraUsage {
    pub camera: String,
    pub dir: PathBuf,
    pub files: usize,
    /// Continuous recordings + event clips/thumbnails.
    pub used_bytes: u64,
    /// Bytes written in the last 24 hours.
    pub bytes_24h: u64,
    /// Average bytes per hour over the measured period (up to 24 h).
    pub bytes_per_hour: f64,
    pub oldest: Option<DateTime<Local>>,
}

pub fn camera_usage(id: &str, dir: &Path) -> CameraUsage {
    let recs = list(dir);
    let events_dir = dir.join("events");
    let now = Local::now();
    let day_ago = now - CDuration::hours(24);
    let mut u = CameraUsage { camera: id.into(), dir: dir.to_path_buf(), files: recs.len(), ..Default::default() };
    let mut items: Vec<(DateTime<Local>, u64)> = recs.iter().map(|r| (r.start, r.size)).collect();
    if let Ok(rd) = std::fs::read_dir(&events_dir) {
        for e in rd.flatten() {
            if let Ok(m) = e.metadata()
                && m.is_file()
            {
                let t: DateTime<Local> = m.modified().map(Into::into).unwrap_or(now);
                items.push((t, m.len()));
            }
        }
    }
    for (t, size) in &items {
        u.used_bytes += size;
        if *t >= day_ago {
            u.bytes_24h += size;
        }
        u.oldest = Some(u.oldest.map_or(*t, |o: DateTime<Local>| o.min(*t)));
    }
    // Rate over the time actually covered (a camera added an hour ago has 1 h of data).
    let since = items.iter().map(|x| x.0).filter(|t| *t >= day_ago).min();
    if let Some(since) = since {
        let hours = ((now - since).num_seconds() as f64 / 3600.0).clamp(0.25, 24.0);
        u.bytes_per_hour = u.bytes_24h as f64 / hours;
    }
    u
}

/// Move recordings from the 0.1 layout (files directly in the root) into
/// per-camera folders.
pub fn migrate_flat_files(root: &Path, camera_ids: &[String]) {
    for r in list(root) {
        if let Some(id) = camera_ids.iter().find(|id| r.name.starts_with(&format!("{id}_"))) {
            let dir = root.join(id);
            if std::fs::create_dir_all(&dir).is_ok() {
                let _ = std::fs::rename(root.join(&r.name), dir.join(&r.name));
            }
        }
    }
}

/// Recordings deleted lately whose cached previews are still to be removed, per folder: removed together (one look at
/// the previews folder for many files, not one for each: a clean-up or a move to deep storage deletes hundreds).
type PendingThumbs = (std::time::Instant, HashMap<PathBuf, std::collections::HashSet<String>>);
static PENDING_THUMBS: std::sync::LazyLock<std::sync::Mutex<PendingThumbs>> = std::sync::LazyLock::new(|| std::sync::Mutex::new((std::time::Instant::now(), HashMap::new())));

/// Remove a recording; its cached previews follow in a batch. Only this file is let go of and forgotten: players of
/// other recordings are not disturbed (every deletion used to drop every open file and every remembered folder).
pub fn delete_recording(dir: &Path, name: &str) -> std::io::Result<()> {
    let path = dir.join(name);
    crate::vstore::forget(&path);
    std::fs::remove_file(&path)?;
    list_removed(dir, name);
    let due = {
        let mut p = PENDING_THUMBS.lock().unwrap_or_else(|e| e.into_inner());
        p.1.entry(dir.to_path_buf()).or_default().insert(name.to_string());
        p.1.values().map(|v| v.len()).sum::<usize>() >= 40 || p.0.elapsed() > std::time::Duration::from_secs(20)
    };
    if due {
        flush_pending_thumbs();
    }
    Ok(())
}

/// Remove the cached previews of the recordings deleted lately ("<name>.jpg", "<name>.sprite-000.jpg" …).
pub fn flush_pending_thumbs() {
    let pending = {
        let mut p = PENDING_THUMBS.lock().unwrap_or_else(|e| e.into_inner());
        p.0 = std::time::Instant::now();
        std::mem::take(&mut p.1)
    };
    for (dir, names) in pending {
        let Ok(rd) = std::fs::read_dir(dir.join(".thumbs")) else { continue };
        for e in rd.flatten() {
            let f = e.file_name().to_string_lossy().to_string();
            // The recording's name is whatever comes before one of the dots.
            if f.match_indices('.').any(|(i, _)| names.contains(&f[..i])) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// Something retention may delete.
enum Item {
    File { dir: PathBuf, name: String },
    Clip { store: Arc<EventStore>, id: String },
}

struct Candidate {
    start: DateTime<Local>,
    size: u64,
    item: Item,
    /// Part of an alert still within the alerts' keep time: kept past the
    /// recordings' own, and deleted last when space runs out.
    alert: bool,
    /// A recording of a camera whose older footage goes to deep storage: it is only deleted for room once it has been
    /// moved there (a move takes it off this disk; it is never deleted before).
    needs_copy: bool,
}

impl Candidate {
    fn delete(&self, why: &str) -> bool {
        match &self.item {
            Item::File { dir, name } => match delete_recording(dir, name) {
                Ok(()) => {
                    info!("retention: deleted {} ({why})", dir.join(name).display());
                    true
                }
                Err(e) => {
                    warn!("retention: could not delete {}: {e}", dir.join(name).display());
                    false
                }
            },
            Item::Clip { store, id } => {
                let ok = store.delete(id);
                if ok {
                    info!("retention: deleted event {id} ({why})");
                }
                ok
            }
        }
    }
}

/// Apply retention rules once across all cameras. The newest recording of
/// each camera (possibly still being written) is never deleted.
/// Where a camera's light sub-stream copy for the timeline is kept.
pub fn timeline_dir(camera_dir: &Path) -> PathBuf {
    camera_dir.join("timeline")
}

pub fn cleanup(cfg: &Config, stores: &HashMap<String, Arc<EventStore>>) -> usize {
    let removed = cleanup_inner(cfg, stores);
    flush_pending_thumbs();
    removed
}

fn cleanup_inner(cfg: &Config, stores: &HashMap<String, Arc<EventStore>>) -> usize {
    let now = Local::now();
    let mut removed = 0;
    // Footage that was kept because it has not been moved to deep storage yet, while room is needed.
    let mut held_back = false;
    let mut all: Vec<Candidate> = Vec::new();
    let mut dirs: Vec<PathBuf> = Vec::new();

    // Alerts may be kept longer (or shorter) than ordinary recordings.
    let alert_days_for = |days: u64| if cfg.storage.alert_retention_days > 0 { cfg.storage.alert_retention_days } else { days };
    type CameraPlan = (String, PathBuf, u64, f64, Option<Arc<EventStore>>);
    let mut cams: Vec<CameraPlan> = cfg
        .cameras
        .iter()
        .map(|c| {
            let days = if c.retention_days > 0 { c.retention_days } else { cfg.storage.retention_days };
            (c.id.clone(), cfg.camera_dir(c), days, c.max_storage_gb, stores.get(&c.id).cloned())
        })
        .collect();
    // Folders of removed cameras under the default root still age out.
    if let Ok(rd) = std::fs::read_dir(&cfg.storage.dir) {
        for e in rd.flatten() {
            let p = e.path();
            let same = |a: &Path, b: &Path| a == b || matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(x), Ok(y)) if x == y);
            if p.is_dir() && !cams.iter().any(|c| same(&c.1, &p)) {
                let id = e.file_name().to_string_lossy().to_string();
                cams.push((id, p, cfg.storage.retention_days, 0.0, None));
            }
        }
    }

    // Cameras whose footage moves to deep storage: the move (and deep
    // storage's keep time) replaces the age limit on the local disk.
    let archived: std::collections::HashSet<String> = cfg.cameras.iter().filter(|c| cfg.archive_for(c).is_some()).map(|c| c.id.clone()).collect();
    for (id, dir, days, max_gb, store) in cams {
        dirs.push(dir.clone());
        let alert_days = alert_days_for(days);
        let alert_cutoff = (alert_days > 0).then(|| now - CDuration::days(alert_days as i64));
        // The alerts still kept (their times).
        let alerts: Vec<(DateTime<Local>, DateTime<Local>)> =
            store.as_ref().map(|s| s.all().iter().filter(|e| alert_cutoff.is_none_or(|c| e.end >= c)).map(|e| (e.start, e.end)).collect()).unwrap_or_default();
        let in_alert = |a: DateTime<Local>, b: DateTime<Local>| alerts.iter().any(|(s, e)| *s <= b && *e >= a);
        let mut recs = list(&dir);
        // A file ends at the latest where the next one starts (its last
        // write can be much later: files are tidied up afterwards).
        let ends: Vec<DateTime<Local>> = (0..recs.len()).map(|i| if i == 0 { recs[0].end } else { recs[i].end.min(recs[i - 1].start) }).collect();
        if !recs.is_empty() {
            recs.remove(0); // keep newest
        }
        // Footage locked in an Evidence Locker case is never deleted here.
        let locked = |a: DateTime<Local>, b: DateTime<Local>| crate::cases::locks_time(&id, a, b).is_some();
        let mut cands: Vec<Candidate> = recs
            .into_iter()
            .zip(ends.into_iter().skip(1))
            .filter(|(r, end)| !locked(r.start, *end))
            .map(|(r, end)| Candidate {
                start: r.start,
                size: r.size,
                alert: in_alert(r.start, end),
                needs_copy: archived.contains(&id),
                item: Item::File { dir: dir.clone(), name: r.name },
            })
            .collect();
        // The timeline copy ages out with the recordings.
        let tdir = timeline_dir(&dir);
        let mut trecs = list(&tdir);
        if !trecs.is_empty() {
            trecs.remove(0);
        }
        cands.extend(trecs.into_iter().filter(|r| !locked(r.start, r.end)).map(|r| Candidate {
            start: r.start,
            size: r.size,
            alert: false,
            needs_copy: false,
            item: Item::File { dir: tdir.clone(), name: r.name },
        }));
        if let Some(store) = &store {
            for e in store.all() {
                if crate::cases::locks_event(&id, &e.id).is_some() || locked(e.start, e.end) {
                    continue;
                }
                if let Some(clip) = &e.clip {
                    let size = std::fs::metadata(store.dir().join(clip)).map(|m| m.len()).unwrap_or(0);
                    cands.push(Candidate {
                        start: e.start,
                        size,
                        alert: alert_cutoff.is_none_or(|c| e.end >= c),
                        needs_copy: false,
                        item: Item::Clip { store: store.clone(), id: e.id.clone() },
                    });
                }
            }
            // Alerts (pictures, details) past the alerts' keep time.
            if let Some(cutoff) = alert_cutoff {
                removed += store.remove_where(|e| e.end < cutoff && crate::cases::locks_event(&id, &e.id).is_none() && !locked(e.start, e.end));
            }
        }
        cands.sort_by_key(|c| c.start);

        let mut kept = Vec::new();
        let cutoff = now - CDuration::days(days as i64);
        for c in cands {
            // An alert's footage goes with the alert; the rest with the recordings.
            let old = if c.alert {
                false
            } else if matches!(c.item, Item::Clip { .. }) {
                alert_cutoff.is_some_and(|a| c.start < a)
            } else {
                days > 0 && !archived.contains(&id) && c.start < cutoff
            };
            if old && c.delete("older than retention period") {
                removed += 1;
                continue;
            }
            kept.push(c);
        }
        // Space limits take plain footage first, alerts last.
        kept.sort_by_key(|c| (c.alert, c.start));
        if max_gb > 0.0 {
            let limit = gb(max_gb);
            let mut used = camera_usage(&id, &dir).used_bytes;
            let mut rest = Vec::new();
            for c in kept {
                if used > limit && c.needs_copy {
                    held_back = true;
                    rest.push(c);
                } else if used > limit && c.delete("camera storage limit reached") {
                    used = used.saturating_sub(c.size);
                    removed += 1;
                } else {
                    rest.push(c);
                }
            }
            kept = rest;
        }
        all.extend(kept);
    }

    all.sort_by_key(|c| (c.alert, c.start));
    let mut rest: Vec<Candidate> = Vec::new();
    {
        let mut all = all.into_iter();
        if cfg.storage.max_storage_gb > 0.0 {
            let limit = gb(cfg.storage.max_storage_gb);
            let mut used: u64 = dirs.iter().map(|d| camera_usage("", d).used_bytes).sum();
            while used > limit {
                let Some(c) = all.next() else { break };
                if c.needs_copy {
                    held_back = true;
                    rest.push(c);
                } else if c.delete("total storage limit reached") {
                    used = used.saturating_sub(c.size);
                    removed += 1;
                } else {
                    rest.push(c);
                }
            }
        }
        rest.extend(all);
        rest.sort_by_key(|c| (c.alert, c.start));
    }
    {
        // Check each disk the cameras use.
        let mut by_disk: HashMap<u64, Vec<&Candidate>> = HashMap::new();
        for c in &rest {
            let dir = match &c.item {
                Item::File { dir, .. } => dir.clone(),
                Item::Clip { store, .. } => store.dir().to_path_buf(),
            };
            let key = crate::sysmon::disk_space(&dir).map(|(_, t)| t).unwrap_or(0);
            by_disk.entry(key).or_default().push(c);
        }
        for (total, list) in &by_disk {
            // Always some room kept free (at least 2 GB or 1% of the disk,
            // even when the setting is lower or 0): a full disk stops
            // recording. Once clearing starts, a little more goes, so it
            // isn't done file by file right at the edge.
            let reserve = gb(cfg.storage.min_free_gb).max(gb(2.0)).max(total / 100);
            let target = reserve + gb(1.0).max(total / 200);
            let mut clearing = false;
            for c in list {
                let dir = match &c.item {
                    Item::File { dir, .. } => dir.clone(),
                    Item::Clip { store, .. } => store.dir().to_path_buf(),
                };
                let free = crate::sysmon::disk_space(&dir).map(|(f, _)| f);
                let need = free.is_some_and(|f| f < if clearing { target } else { reserve });
                if !need {
                    break;
                }
                clearing = true;
                if c.needs_copy {
                    // Not moved to deep storage yet: never deleted for room. The mover is asked to move the oldest
                    // files now (see `archive::run`); only when the disk is about to be completely full (recording
                    // would stop) is the oldest footage given up, loudly.
                    held_back = true;
                    let emergency = gb(0.5).max(total / 400);
                    if free.is_some_and(|f| f < emergency) {
                        warn!(
                            "DISK ALMOST FULL: deleting {} that was not moved to deep storage, to keep recording",
                            dir.join(match &c.item {
                                Item::File { name, .. } => name.as_str(),
                                _ => "",
                            })
                            .display()
                        );
                        if c.delete("disk almost full — deep storage could not take it in time") {
                            removed += 1;
                            pruned(&dir);
                        }
                    }
                    continue;
                }
                if c.delete("keeping room free on the disk") {
                    removed += 1;
                    pruned(&dir);
                }
            }
        }
    }
    if held_back {
        // Footage kept for deep storage while the disk needs room: move it now, oldest first.
        URGENT_MOVE.store(true, std::sync::atomic::Ordering::Relaxed);
        crate::archive::RUN_NOW.notify_one();
    }
    removed
}

/// Set when the clean-up kept footage that deep storage still has to take in, because the disk needs room.
static URGENT_MOVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Does the deep-storage mover have to hurry (and was told so since last asked)?
pub fn take_urgent_move() -> bool {
    URGENT_MOVE.swap(false, std::sync::atomic::Ordering::Relaxed)
}

/// (free bytes, bytes wanted free, bytes to reach once clearing starts) of the disk holding `dir`.
pub fn room_needed(cfg: &Config, dir: &Path) -> Option<(u64, u64, u64)> {
    let (free, total) = crate::sysmon::disk_space(dir)?;
    let reserve = gb(cfg.storage.min_free_gb).max(gb(2.0)).max(total / 100);
    Some((free, reserve, reserve + gb(1.0).max(total / 200)))
}

/// Cameras whose oldest footage was just deleted to make room (by their
/// folder): the timeline shows it.
static PRUNED: std::sync::LazyLock<std::sync::Mutex<HashMap<PathBuf, std::time::Instant>>> = std::sync::LazyLock::new(Default::default);

fn pruned(dir: &Path) {
    let cam = match dir.file_name().and_then(|n| n.to_str()) {
        Some("timeline" | "events") => dir.parent().unwrap_or(dir),
        _ => dir,
    };
    PRUNED.locked().insert(cam.to_path_buf(), std::time::Instant::now());
}

/// Is this camera's oldest footage being deleted to make room (in the last
/// few minutes)?
pub fn pruning(camera_dir: &Path) -> bool {
    PRUNED.locked().get(camera_dir).is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(300))
}

fn gb(x: f64) -> u64 {
    (x * 1024.0 * 1024.0 * 1024.0) as u64
}

/// Recordings folders that can't be reached right now (a NAS that's off,
/// or a mapped drive letter that a Windows service can't see), checked
/// every 30 s in the background (a look at a NAS that's away can take a
/// while, so never while answering the page).
static UNREACHABLE: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

pub fn unreachable() -> Vec<String> {
    UNREACHABLE.locked().clone()
}

pub async fn watch_roots(m: std::sync::Arc<crate::manager::Manager>) {
    loop {
        let cfg = m.config();
        let mut roots: Vec<PathBuf> = vec![cfg.storage.dir.clone()];
        for c in &cfg.cameras {
            if c.remote.is_some() {
                continue;
            }
            let d = PathBuf::from(c.storage_dir.trim());
            if !c.storage_dir.trim().is_empty() && !roots.contains(&d) {
                roots.push(d);
            }
        }
        let bad = tokio::task::spawn_blocking(move || roots.into_iter().filter(|r| std::fs::read_dir(r).is_err()).map(|r| r.display().to_string()).collect::<Vec<_>>())
            .await
            .unwrap_or_default();
        let was = std::mem::replace(&mut *UNREACHABLE.locked(), bad.clone());
        for b in bad.iter().filter(|b| !was.contains(b)) {
            tracing::warn!("the recordings folder {b} can't be reached");
        }
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
    }
}

/// Is this folder on another computer (a NAS share) rather than a drive in
/// this one? Only then is opening a recording slow enough to say "loading
/// from deep storage". A network address, a mapped network drive (Windows) or
/// an NFS / SMB / sshfs mount (Linux and macOS-like) count.
pub fn is_network_path(p: &Path) -> bool {
    let s = p.to_string_lossy();
    if (s.starts_with("\\\\") || s.starts_with("//")) && !s.starts_with("\\\\?\\") && !s.starts_with("\\\\.\\") {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let abs = std::path::absolute(p).unwrap_or(p.to_path_buf());
        let mut root = abs.components().next().map(|c| c.as_os_str().to_os_string()).unwrap_or_default();
        root.push("\\");
        let w: Vec<u16> = root.encode_wide().chain(std::iter::once(0)).collect();
        // DRIVE_REMOTE = 4
        return unsafe { windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(w.as_ptr()) } == 4;
    }
    #[cfg(not(windows))]
    {
        let abs = std::fs::canonicalize(p).unwrap_or(p.to_path_buf());
        let Ok(mounts) = std::fs::read_to_string("/proc/mounts") else { return false };
        mounts
            .lines()
            .filter_map(|l| {
                let mut f = l.split_whitespace();
                let (_, point, kind) = (f.next()?, f.next()?, f.next()?);
                abs.starts_with(point).then_some((point.len(), kind))
            })
            .max_by_key(|(n, _)| *n)
            .is_some_and(|(_, k)| ["nfs", "nfs4", "cifs", "smb3", "smbfs", "sshfs", "fuse.sshfs", "9p", "afs", "ceph", "glusterfs", "fuse.rclone"].contains(&k))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How long listing a camera's folder takes with a month of 5-minute
    /// files (8,640): the measurement behind keeping folder listings rather
    /// than adding a database index. `cargo test --release -- --ignored list_speed --nocapture`
    #[test]
    #[ignore]
    fn list_speed() {
        let d = std::env::temp_dir().join(format!("gaze-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let t0 = Local::now() - CDuration::days(30);
        for i in 0..8640 {
            let t = t0 + CDuration::minutes(5 * i);
            std::fs::write(d.join(format!("rec_{}.mp4", t.format("%Y-%m-%d_%H-%M-%S"))), b"x").unwrap();
        }
        let mut times = Vec::new();
        for _ in 0..5 {
            let s = std::time::Instant::now();
            let l = list_now(&d);
            times.push(s.elapsed());
            assert_eq!(l.len(), 8640);
        }
        println!("list of 8,640 recordings: {:?} (each run)", times);
        let _ = std::fs::remove_dir_all(&d);
    }
    use crate::config::CameraConfig;

    #[test]
    fn parses_names() {
        let t = parse_start("front-door_2026-11-08_14-05-00.mp4").unwrap();
        assert_eq!(t.format("%Y-%m-%d %H:%M:%S").to_string(), "2026-11-08 14:05:00");
        assert!(parse_start("garbage.mp4").is_none());
    }

    #[test]
    fn rejects_traversal() {
        let dir = std::env::temp_dir();
        assert!(resolve(&dir, "../etc/passwd.mp4").is_err());
        assert!(resolve(&dir, "a/b.mp4").is_err());
        assert!(resolve(&dir, ".hidden").is_err());
    }

    #[test]
    fn alerts_outlive_ordinary_recordings() {
        let root = std::env::temp_dir().join(format!("nvr-al-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut cfg = Config::default();
        cfg.storage.dir = root.clone();
        cfg.storage.min_free_gb = 0.0;
        cfg.storage.retention_days = 1;
        cfg.storage.alert_retention_days = 30;
        cfg.cameras.push(CameraConfig { id: "a".into(), url: "x".into(), ..Default::default() });
        let d = cfg.camera_dir(&cfg.cameras[0]);
        std::fs::create_dir_all(&d).unwrap();
        // Three 5-minute files five days ago, and one now.
        let day = Local::now() - CDuration::days(5);
        let base = day.date_naive().and_hms_opt(10, 0, 0).unwrap().and_local_timezone(Local).unwrap();
        for m in [0, 5, 10] {
            let t = base + CDuration::minutes(m);
            std::fs::write(d.join(format!("a_{}.mp4", t.format("%Y-%m-%d_%H-%M-%S"))), b"x").unwrap();
        }
        std::fs::write(d.join(format!("a_{}.mp4", Local::now().format("%Y-%m-%d_%H-%M-%S"))), b"x").unwrap();
        // An alert in the middle file.
        let store = Arc::new(EventStore::open(&d));
        let e: crate::events::Event = serde_json::from_value(serde_json::json!({"id": "ev1", "camera": "a",
            "start": base + CDuration::minutes(6), "end": base + CDuration::minutes(7)}))
        .unwrap();
        store.save_detected(e);
        let stores = HashMap::from([("a".to_string(), store.clone())]);
        cleanup(&cfg, &stores);
        let left: Vec<String> = list(&d).into_iter().map(|r| r.name).collect();
        assert_eq!(left.len(), 2, "{left:?}"); // today's, and the alert's
        assert!(left.iter().any(|n| n.contains("10-05-00")));
        assert_eq!(store.all().len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn per_camera_limits_and_custom_folders() {
        let root = std::env::temp_dir().join(format!("nvr-st-{}", std::process::id()));
        let other = std::env::temp_dir().join(format!("nvr-st2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&other);
        let mut cfg = Config::default();
        cfg.storage.dir = root.clone();
        cfg.storage.min_free_gb = 0.0;
        cfg.storage.retention_days = 0;
        cfg.cameras.push(CameraConfig { id: "a".into(), url: "x".into(), max_storage_gb: 1.5 / 1024.0, ..Default::default() });
        cfg.cameras.push(CameraConfig { id: "b".into(), url: "x".into(), storage_dir: other.display().to_string(), ..Default::default() });
        for (cam, t) in [("a", "10-00-00"), ("a", "10-05-00"), ("a", "10-10-00"), ("b", "10-00-00"), ("b", "10-05-00")] {
            let d = cfg.camera_dir(cfg.cameras.iter().find(|c| c.id == cam).unwrap());
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(format!("{cam}_2026-01-01_{t}.mp4")), vec![0u8; 1 << 20]).unwrap();
        }
        // Camera a: 3 MB with a 1.5 MB limit → the two oldest go. Camera b untouched.
        assert_eq!(cleanup(&cfg, &HashMap::new()), 2);
        assert_eq!(list(&root.join("a")).len(), 1);
        assert_eq!(list(&other.join("b")).len(), 2);
        let u = camera_usage("b", &other.join("b"));
        assert_eq!(u.used_bytes, 2 << 20);
        std::fs::remove_dir_all(&root).unwrap();
        std::fs::remove_dir_all(&other).unwrap();
    }

    #[test]
    fn deleting_a_recording_leaves_the_other_folders_lists_and_clears_its_previews() {
        let base = std::env::temp_dir().join(format!("gaze-del-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (a, b) = (base.join("a"), base.join("b"));
        for d in [&a, &b] {
            std::fs::create_dir_all(d.join(".thumbs")).unwrap();
            for n in 0..3 {
                std::fs::write(d.join(format!("cam_2026-11-19_10-0{n}-00.mp4")), b"x").unwrap();
            }
        }
        let gone = "cam_2026-11-19_10-01-00.mp4";
        for f in [format!("{gone}.jpg"), format!("{gone}.sprite-000.jpg"), format!("{gone}.sprite.json"), "cam_2026-11-19_10-02-00.mp4.jpg".to_string()] {
            std::fs::write(a.join(".thumbs").join(f), b"x").unwrap();
        }
        assert_eq!((list(&a).len(), list(&b).len()), (3, 3));
        // A file of b appears on disk after it was listed: a's deletion must not make b be read again.
        std::fs::write(b.join("cam_2026-11-19_10-09-00.mp4"), b"x").unwrap();
        delete_recording(&a, gone).unwrap();
        assert_eq!(list(&a).len(), 2, "the deleted one is gone from a's list");
        assert_eq!(list(&b).len(), 3, "b's remembered list was left alone");
        flush_pending_thumbs();
        let left: Vec<String> = std::fs::read_dir(a.join(".thumbs")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        assert_eq!(left, vec!["cam_2026-11-19_10-02-00.mp4.jpg".to_string()], "only the other recording's preview is left");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn footage_is_not_deleted_for_room_before_it_was_moved_to_deep_storage() {
        let root = std::env::temp_dir().join(format!("nvr-nodel-{}", std::process::id()));
        let deep = std::env::temp_dir().join(format!("nvr-nodel-deep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&deep);
        std::fs::create_dir_all(&deep).unwrap();
        let mut cfg = Config::default();
        cfg.storage.dir = root.clone();
        // More free room wanted than any disk has: the disk always "needs room".
        cfg.storage.min_free_gb = 1.0e9;
        cfg.storage.retention_days = 0;
        cfg.storage.archive.enabled = true;
        cfg.storage.archive.dir = deep.clone();
        cfg.cameras.push(CameraConfig { id: "with".into(), url: "x".into(), ..Default::default() });
        cfg.cameras.push(CameraConfig { id: "without".into(), url: "x".into(), archive_mode: "off".into(), ..Default::default() });
        let base = Local::now() - CDuration::hours(5);
        for c in &cfg.cameras {
            let d = cfg.camera_dir(c);
            std::fs::create_dir_all(&d).unwrap();
            for m in 0..4 {
                let t = base + CDuration::minutes(m * 5);
                std::fs::write(d.join(format!("{}_{}.mp4", c.id, t.format("%Y-%m-%d_%H-%M-%S"))), b"x").unwrap();
            }
        }
        cleanup(&cfg, &HashMap::new());
        let with = list(&cfg.camera_dir(&cfg.cameras[0])).len();
        let without = list(&cfg.camera_dir(&cfg.cameras[1])).len();
        assert_eq!(with, 4, "nothing is deleted before deep storage has it");
        assert_eq!(without, 1, "a camera without deep storage keeps only its newest file");
        assert!(take_urgent_move(), "the mover is asked to hurry");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&deep);
    }
}
