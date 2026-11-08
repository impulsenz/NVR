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
pub const FILTER_LABELS: &[&str] = &[
    "person",
    "car",
    "truck",
    "bus",
    "motorcycle",
    "bicycle",
    "dog",
    "cat",
    "bird",
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
}

pub struct Query {
    pub cameras: Vec<String>,
    pub from: DateTime<Local>,
    pub to: DateTime<Local>,
    /// Time-of-day window (may wrap past midnight).
    pub tod: Option<(NaiveTime, NaiveTime)>,
    /// "recording", "motion", "other" and object labels. Empty = everything.
    pub types: HashSet<String>,
    pub limit: usize,
}

fn enc(s: &str) -> String {
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

pub fn search(mgr: &Manager, q: &Query) -> Vec<Item> {
    let mut items = Vec::new();
    for rt in mgr.cameras() {
        if !q.cameras.is_empty() && !q.cameras.contains(&rt.cfg.id) {
            continue;
        }
        let id = &rt.cfg.id;
        let recs = recordings(&rt);
        if q.types.is_empty() || q.types.contains("recording") {
            for r in recs
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
                });
            }
        }
        for e in rt.events.between(q.from, q.to) {
            if !in_tod(e.start, q.tod) || !event_matches(&e, &q.types) {
                continue;
            }
            let play = play_at(&rt, &recs, Some(&e), e.start);
            let pending = play.is_none()
                && e.clip.is_none()
                && rt.cfg.record_mode.event_clips()
                && (Local::now() - e.end).num_seconds() < 180;
            let size = e
                .clip
                .as_ref()
                .and_then(|c| std::fs::metadata(rt.events.dir().join(c)).ok())
                .map(|m| m.len());
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
                    // alert is in progress: version the URL by its time.
                    let v = std::fs::metadata(rt.events.dir().join(t))
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    format!("{}?v={v}", event_file_url(id, t))
                }),
                download: e
                    .clip
                    .as_ref()
                    .map(|c| format!("{}?download=1", event_file_url(id, c))),
                play,
                pending,
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
    /// "recording" or "clip".
    pub kind: &'static str,
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
        let mut segments: Vec<Segment> = recordings(&rt)
            .into_iter()
            .filter(|r| r.end >= from && r.start <= to)
            .map(|r| Segment {
                start: r.start,
                end: r.end,
                url: rec_url(id, &r.name),
                name: r.name,
                kind: "recording",
            })
            .collect();
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
    if parts.is_empty() {
        // Event-only cameras: use full-quality clips. Back-to-back clips
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
    parts
}
