//! Finding recordings and events for the Recordings page: filtered search,
//! multi-camera timelines, and the file pieces needed to export a time range.

use crate::events::Event;
use crate::manager::{CameraRuntime, Manager};
use crate::storage::{self, Recording};
use chrono::{DateTime, Duration as CDuration, Local, NaiveTime};
use serde::Serialize;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

/// Object labels with their own filter tick box; anything else is "other".
/// Objects with their own button in the Alerts filter; "Other objects"
/// is everything else.
pub const FILTER_LABELS: &[&str] = &[
    "person",
    "car",
    "truck",
    "motorcycle",
    "bicycle",
    "dog",
    "cat",
    "package",
];

#[derive(Debug, Clone, Serialize)]
pub struct Play {
    pub url: String,
    /// Seconds into the file to start at.
    pub offset: f64,
    /// Wall-clock time of the file's first frame.
    pub file_start: DateTime<Local>,
    pub kind: &'static str,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Item {
    /// "recording" or "event".
    pub kind: &'static str,
    pub camera: String,
    pub camera_name: String,
    pub start: DateTime<Local>,
    pub end: DateTime<Local>,
    pub size: Option<u64>,
    pub name: Option<String>,
    pub event: Option<Event>,
    pub thumb: Option<String>,
    pub play: Option<Play>,
    pub download: Option<String>,
    /// An alert whose full-quality clip is still being recorded.
    pub pending: bool,
    /// Alert already viewed.
    pub seen: bool,
}

pub struct Query {
    pub cameras: Vec<String>,
    pub from: DateTime<Local>,
    pub to: DateTime<Local>,
    /// Time-of-day window (may wrap past midnight).
    pub tod: Option<(NaiveTime, NaiveTime)>,
    /// "recording", "motion", "other" and object labels. Empty = everything.
    pub types: HashSet<String>,
    /// Only alerts with this known person or vehicle (by name, or one of
    /// its plates).
    pub who: Option<Who>,
    /// Only viewed (Some(true)) or only new (Some(false)) alerts.
    pub seen: Option<bool>,
    pub limit: usize,
}

/// A known person or vehicle to look for.
#[derive(Debug, Clone)]
pub struct Who {
    pub name: String,
    pub plates: Vec<String>,
}

/// Was this known person or vehicle in the alert?
fn has_who(e: &Event, w: &Who) -> bool {
    e.faces
        .iter()
        .any(|f| f.name.as_deref() == Some(w.name.as_str()))
        || e.plates
            .iter()
            .any(|p| w.plates.iter().any(|k| crate::recog::plate_matches(k, p)))
        || e.notes.iter().any(|n| {
            n == &format!("Recognized {}", w.name) || n.ends_with(&format!(" — {}", w.name))
        })
}

pub fn enc(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c.to_string()
            } else {
                format!("%{:02X}", c as u32)
            }
        })
        .collect()
}

pub fn rec_url(cam: &str, name: &str) -> String {
    format!("media/{}/rec/{}", enc(cam), enc(name))
}

pub fn event_file_url(cam: &str, file: &str) -> String {
    format!("media/{}/events/{}", enc(cam), enc(file))
}

fn in_tod(t: DateTime<Local>, tod: Option<(NaiveTime, NaiveTime)>) -> bool {
    let Some((a, b)) = tod else { return true };
    let x = t.time();
    if a <= b {
        x >= a && x <= b
    } else {
        x >= a || x <= b
    }
}

fn event_matches(e: &Event, types: &HashSet<String>) -> bool {
    if types.is_empty() {
        return true;
    }
    // Alerts where a face was seen or a number plate read.
    if (types.contains("face") && !e.faces.is_empty())
        || (types.contains("plate") && !e.plates.is_empty())
    {
        return true;
    }
    if e.labels.is_empty() {
        return types.contains("motion");
    }
    e.labels.iter().any(|l| {
        types.contains(&l.label)
            || (types.contains("other") && !FILTER_LABELS.contains(&l.label.as_str()))
    })
}

/// Where to play footage at time `t` for a camera.
pub fn play_at(
    rt: &CameraRuntime,
    recs: &[Recording],
    e: Option<&Event>,
    t: DateTime<Local>,
) -> Option<Play> {
    if let Some(e) = e
        && let (Some(clip), Some(cs)) = (&e.clip, e.clip_start)
    {
        return Some(Play {
            url: event_file_url(&rt.cfg.id, clip),
            offset: 0.0,
            file_start: cs,
            kind: "clip",
            name: clip.clone(),
        });
    }
    let r = recs
        .iter()
        .find(|r| r.start <= t && t <= r.end + CDuration::seconds(2))?;
    let lead = if e.is_some() { 3.0 } else { 0.0 };
    Some(Play {
        url: rec_url(&rt.cfg.id, &r.name),
        offset: ((t - r.start).num_milliseconds() as f64 / 1000.0 - lead).max(0.0),
        file_start: r.start,
        kind: "recording",
        name: r.name.clone(),
    })
}

/// A camera's continuous recordings, newest first. The newest file of a camera
/// that is recording right now is treated as running up to this moment (its
/// modification time lags behind while it is being written).
pub fn recordings(rt: &CameraRuntime) -> Vec<Recording> {
    let mut recs = storage::list(&rt.dir);
    let live = rt
        .recorder
        .as_ref()
        .is_some_and(|r| r.status().state == crate::recorder::RecState::Recording);
    if live && let Some(r) = recs.first_mut() {
        r.end = Local::now();
    }
    recs
}

/// The light sub-stream copy for the timeline (newest still growing).
pub fn timeline_recordings(rt: &CameraRuntime) -> Vec<Recording> {
    let mut recs = storage::list(&storage::timeline_dir(&rt.dir));
    let live = rt
        .timeline_recorder
        .as_ref()
        .is_some_and(|r| r.status().state == crate::recorder::RecState::Recording);
    if live && let Some(r) = recs.first_mut() {
        r.end = Local::now();
    }
    recs
}

pub fn search(mgr: &Manager, q: &Query) -> Vec<Item> {
    let mut items = Vec::new();
    for rt in mgr.cameras() {
        if !q.cameras.is_empty() && !q.cameras.contains(&rt.cfg.id) {
            continue;
        }
        let id = &rt.cfg.id;
        // The camera's recordings: read only if needed (listing recordings,
        // or an alert without its own clip to play).
        let recs_cell: std::cell::OnceCell<Vec<Recording>> = std::cell::OnceCell::new();
        let recs = || recs_cell.get_or_init(|| recordings(&rt));
        // Looking for someone only finds alerts.
        if q.who.is_none() && (q.types.is_empty() || q.types.contains("recording")) {
            for r in recs()
                .iter()
                .filter(|r| r.end >= q.from && r.start <= q.to && in_tod(r.start, q.tod))
            {
                items.push(Item {
                    kind: "recording",
                    camera: id.clone(),
                    camera_name: rt.cfg.name.clone(),
                    start: r.start,
                    end: r.end,
                    size: Some(r.size),
                    name: Some(r.name.clone()),
                    event: None,
                    thumb: Some(format!("media/{}/thumb/{}", enc(id), enc(&r.name))),
                    play: Some(Play {
                        url: rec_url(id, &r.name),
                        offset: 0.0,
                        file_start: r.start,
                        kind: "recording",
                        name: r.name.clone(),
                    }),
                    download: Some(format!("{}?download=1", rec_url(id, &r.name))),
                    pending: false,
                    seen: true,
                });
            }
        }
        for e in rt.events.between(q.from, q.to) {
            if !in_tod(e.start, q.tod) || !event_matches(&e, &q.types) {
                continue;
            }
            if let Some(want) = q.seen
                && mgr.seen.is_seen(&e.id, e.start) != want
            {
                continue;
            }
            if let Some(w) = &q.who
                && !has_who(&e, w)
            {
                continue;
            }
            let play = if e.clip.is_some() && e.clip_start.is_some() {
                play_at(&rt, &[], Some(&e), e.start)
            } else {
                play_at(&rt, recs(), Some(&e), e.start)
            };
            let pending = play.is_none()
                && e.clip.is_none()
                && rt.cfg.record_mode.event_clips()
                && (Local::now() - e.end).num_seconds() < 180;
            // (Not read from the disk per alert: that made the list slow.)
            let size = None;
            items.push(Item {
                kind: "event",
                camera: id.clone(),
                camera_name: rt.cfg.name.clone(),
                start: e.start,
                end: e.end,
                size,
                name: None,
                thumb: e.thumb.as_ref().map(|t| {
                    // The picture can be replaced by a better one while the
                    // alert is in progress: versioned by when it was taken
                    // (no disk read per alert).
                    let v = e.thumb_at.map(|x| x.timestamp_millis()).unwrap_or(0);
                    format!("{}?v={v}", event_file_url(id, t))
                }),
                download: e
                    .clip
                    .as_ref()
                    .map(|c| format!("{}?download=1", event_file_url(id, c))),
                play,
                pending,
                seen: mgr.seen.is_seen(&e.id, e.start),
                event: Some(e),
            });
        }
    }
    items.sort_by(|a, b| b.start.cmp(&a.start));
    items.truncate(q.limit);
    items
}

#[derive(Debug, Clone, Serialize)]
pub struct Segment {
    pub start: DateTime<Local>,
    pub end: DateTime<Local>,
    pub url: String,
    pub name: String,
    /// "recording" (as recorded), "sub" (light copy for the timeline) or "clip".
    pub kind: &'static str,
    /// Which camera stream it is: "main" (full quality) or "sub".
    pub stream: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct CameraTimeline {
    pub camera: String,
    pub name: String,
    pub segments: Vec<Segment>,
    pub events: Vec<Event>,
}

pub fn timeline(
    mgr: &Manager,
    cameras: &[String],
    from: DateTime<Local>,
    to: DateTime<Local>,
) -> Vec<CameraTimeline> {
    let mut out = Vec::new();
    for rt in mgr.cameras() {
        if !cameras.is_empty() && !cameras.contains(&rt.cfg.id) {
            continue;
        }
        let id = &rt.cfg.id;
        let stream_of = |name: &str| recording_stream(&rt, name);
        let mut segments: Vec<Segment> = recordings(&rt)
            .into_iter()
            .filter(|r| r.end >= from && r.start <= to)
            .map(|r| Segment {
                start: r.start,
                end: r.end,
                url: rec_url(id, &r.name),
                stream: stream_of(&r.name),
                name: r.name,
                kind: "recording",
            })
            .collect();
        segments.extend(
            timeline_recordings(&rt)
                .into_iter()
                .filter(|r| r.end >= from && r.start <= to)
                .map(|r| Segment {
                    start: r.start,
                    end: r.end,
                    url: format!("media/{}/tl/{}", enc(id), enc(&r.name)),
                    name: r.name,
                    kind: "sub",
                    stream: "sub",
                }),
        );
        let events = rt.events.between(from, to);
        for e in &events {
            if let (Some(clip), Some(cs)) = (&e.clip, e.clip_start) {
                let len = std::fs::metadata(rt.events.dir().join(clip))
                    .ok()
                    .map(|_| ());
                if len.is_some() {
                    let end = e.end + CDuration::seconds(rt.cfg.detect.post_seconds as i64);
                    segments.push(Segment {
                        start: cs,
                        end,
                        url: event_file_url(id, clip),
                        name: clip.clone(),
                        kind: "clip",
                        stream: "main",
                    });
                }
            }
        }
        segments.sort_by_key(|s| s.start);
        out.push(CameraTimeline {
            camera: id.clone(),
            name: rt.cfg.name.clone(),
            segments,
            events,
        });
    }
    out
}

/// Pieces of files covering `[from, to)`: (path, seconds in, seconds out).
pub fn export_parts(
    rt: &Arc<CameraRuntime>,
    from: DateTime<Local>,
    to: DateTime<Local>,
) -> Vec<(PathBuf, f64, f64)> {
    let secs = |a: DateTime<Local>, b: DateTime<Local>| (b - a).num_milliseconds() as f64 / 1000.0;
    let mut recs: Vec<Recording> = storage::list(&rt.dir)
        .into_iter()
        .filter(|r| r.end > from && r.start < to)
        .collect();
    recs.sort_by_key(|r| r.start);
    let mut parts: Vec<(PathBuf, f64, f64)> = recs
        .iter()
        .map(|r| {
            (
                rt.dir.join(&r.name),
                secs(r.start, from).max(0.0),
                secs(r.start, to.min(r.end)),
            )
        })
        .collect();
    // Always the higher quality: when the continuous recording is the
    // sub-stream, use the full-quality alert clips where there are any.
    let main_recordings = rt.cfg.record_mode.continuous_stream() == Some(false);
    let rec_parts = std::mem::take(&mut parts);
    {
        // Full-quality clips. Back-to-back clips
        // overlap (each includes its pre-event buffer), so each clip only
        // adds the time not already covered.
        let post = chrono::Duration::seconds(rt.cfg.detect.post_seconds as i64 + 2);
        let mut clips: Vec<_> = rt
            .events
            .between(from, to)
            .into_iter()
            .filter_map(|e| {
                let (clip, cs) = (e.clip.clone()?, e.clip_start?);
                let p = rt.events.dir().join(clip);
                p.exists().then_some((p, cs, e.end + post))
            })
            .collect();
        clips.sort_by_key(|c| c.1);
        let mut covered = from;
        for (p, cs, ce) in clips {
            let start = covered.max(cs);
            let end = to.min(ce);
            if end > start {
                parts.push((p, secs(cs, start), secs(cs, end)));
                covered = end;
            }
        }
    }
    parts.retain(|p| p.2 > p.1);
    let clip_parts = parts;
    if main_recordings && !rec_parts.is_empty() || clip_parts.is_empty() {
        rec_parts.into_iter().filter(|p| p.2 > p.1).collect()
    } else {
        clip_parts
    }
}

/// New (not yet viewed) alerts in the last week.
pub fn unseen_count(mgr: &Manager) -> usize {
    let from = Local::now() - CDuration::days(7);
    mgr.cameras()
        .iter()
        .flat_map(|rt| rt.events.between(from, Local::now()))
        .filter(|e| !mgr.seen.is_seen(&e.id, e.start))
        .count()
}

/// Which stream a recording is ("main" or "sub"): told by its picture size
/// (the recording mode may have changed since); a file still being written
/// (no header yet) is of the stream recorded now.
pub fn recording_stream(rt: &CameraRuntime, name: &str) -> &'static str {
    let now_stream = if rt.cfg.record_mode.continuous_stream() == Some(true) { "sub" } else { "main" };
    let h = |r: &crate::relay::Relay| r.info().and_then(|i| i.height);
    let (main_h, sub_h) = (h(&rt.relay_main), h(&rt.relay_sub));
    match mp4_height_cached(&rt.dir.join(name)) {
        Some(fh) => match (main_h, sub_h) {
            (Some(m), Some(s)) if s < m => {
                if fh * 2 <= s + m { "sub" } else { "main" }
            }
            _ => {
                if fh <= 576 { "sub" } else { "main" }
            }
        },
        None => now_stream,
    }
}

/// Picture height of an MP4 file, read from its header (no ffmpeg), kept
/// once known (a file still being written has no header yet).
fn mp4_height_cached(path: &std::path::Path) -> Option<u32> {
    use std::collections::HashMap;
    use std::sync::Mutex;
    static CACHE: Mutex<Option<HashMap<PathBuf, u32>>> = Mutex::new(None);
    if let Some(h) = CACHE
        .lock()
        .ok()?
        .get_or_insert_with(HashMap::new)
        .get(path)
    {
        return Some(*h);
    }
    let h = mp4_height(path)?;
    let mut c = CACHE.lock().ok()?;
    let map = c.get_or_insert_with(HashMap::new);
    if map.len() > 100_000 {
        map.clear();
    }
    map.insert(path.to_path_buf(), h);
    Some(h)
}

/// Picture height of an MP4 file from its `moov` box (wherever it is).
pub fn mp4_height(path: &std::path::Path) -> Option<u32> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let mut pos = 0u64;
    while pos + 8 <= len {
        f.seek(SeekFrom::Start(pos)).ok()?;
        let mut h = [0u8; 16];
        f.read_exact(&mut h[..8]).ok()?;
        let mut size = u32::from_be_bytes(h[0..4].try_into().ok()?) as u64;
        let mut hdr = 8u64;
        if size == 1 {
            f.read_exact(&mut h[8..16]).ok()?;
            size = u64::from_be_bytes(h[8..16].try_into().ok()?);
            hdr = 16;
        } else if size == 0 {
            size = len - pos;
        }
        if size < hdr {
            return None;
        }
        if &h[4..8] == b"moov" {
            let n = (size - hdr).min(64 << 20) as usize;
            let mut buf = vec![0; n];
            f.read_exact(&mut buf).ok()?;
            return tkhd_height(&buf);
        }
        pos += size;
    }
    None
}

/// The first track with a picture: its height (`tkhd` ends with width and
/// height as 16.16 numbers).
fn tkhd_height(b: &[u8]) -> Option<u32> {
    let mut i = 0;
    while i + 8 <= b.len() {
        let size = u32::from_be_bytes(b[i..i + 4].try_into().ok()?) as usize;
        if size < 8 || i + size > b.len() {
            return None;
        }
        let body = &b[i + 8..i + size];
        match &b[i + 4..i + 8] {
            b"trak" => {
                if let Some(h) = tkhd_height(body) {
                    return Some(h);
                }
            }
            b"tkhd" if body.len() >= 8 => {
                let h = u32::from_be_bytes(body[body.len() - 4..].try_into().ok()?) >> 16;
                if h > 0 {
                    return Some(h);
                }
            }
            _ => {}
        }
        i += size;
    }
    None
}

#[cfg(test)]
mod mp4_tests {
    #[test]
    fn reads_picture_height_from_the_header() {
        // moov > trak > tkhd, with width 1280 and height 720 at the end.
        let mut tkhd = vec![0u8; 84];
        tkhd[76..80].copy_from_slice(&(1280u32 << 16).to_be_bytes());
        tkhd[80..84].copy_from_slice(&(720u32 << 16).to_be_bytes());
        let bx = |t: &[u8], body: &[u8]| {
            [&((body.len() + 8) as u32).to_be_bytes()[..], t, body].concat()
        };
        let file = [
            bx(b"ftyp", &[0; 8]),
            bx(b"mdat", &[1; 32]),
            bx(b"moov", &bx(b"trak", &bx(b"tkhd", &tkhd))),
        ]
        .concat();
        let p = std::env::temp_dir().join(format!("gaze-mp4-{}.mp4", std::process::id()));
        std::fs::write(&p, file).unwrap();
        assert_eq!(super::mp4_height(&p), Some(720));
        let _ = std::fs::remove_file(&p);
    }
}

/// Space a camera's footage takes, by kind (bytes).
#[derive(Debug, Default, Clone, Serialize)]
pub struct SpaceUse {
    pub camera: String,
    pub name: String,
    /// 24/7 (and other) recordings of the main stream.
    pub main: u64,
    /// Recordings of the sub-stream, and the light timeline copies.
    pub sub: u64,
    /// Alert clips and pictures.
    pub alerts: u64,
    /// Time-lapse previews (and other thumbnails).
    pub timelapse: u64,
    /// Anything else (push-to-talk sound, notes).
    pub other: u64,
}

fn dir_size(dir: &std::path::Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    rd.flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

/// What a camera's folder holds, by kind.
pub fn space_use(rt: &CameraRuntime) -> SpaceUse {
    let mut u = SpaceUse { camera: rt.cfg.id.clone(), name: rt.cfg.name.clone(), ..Default::default() };
    let events = rt.events.dir().to_path_buf();
    let timeline = storage::timeline_dir(&rt.dir);
    for (dir, is_rec) in [(rt.dir.clone(), true), (timeline.clone(), false)] {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() {
                if p == events || p == timeline {
                    continue;
                }
                let n = dir_size(&p);
                if e.file_name() == ".thumbs" { u.timelapse += n } else { u.other += n }
                continue;
            }
            let name = e.file_name().to_string_lossy().to_string();
            if !name.ends_with(".mp4") {
                u.other += m.len();
            } else if !is_rec || recording_stream(rt, &name) == "sub" {
                u.sub += m.len();
            } else {
                u.main += m.len();
            }
        }
    }
    if let Ok(rd) = std::fs::read_dir(&events) {
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() {
                let n = dir_size(&e.path());
                if e.file_name() == ".thumbs" { u.timelapse += n } else { u.alerts += n }
            } else {
                u.alerts += m.len();
            }
        }
    }
    u
}
