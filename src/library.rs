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
    /// Smart search: how well the alert matches the words (0..1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
}

pub struct Query {
    pub cameras: Vec<String>,
    pub from: DateTime<Local>,
    pub to: DateTime<Local>,
    /// Time-of-day window (may wrap past midnight).
    pub tod: Option<(NaiveTime, NaiveTime)>,
    /// "recording", "motion", "other" and object labels. Empty = everything.
    pub types: HashSet<String>,
    /// Words searched for (see `parse_text`). Empty = no text search.
    pub text: Vec<Term>,
    /// Only viewed (Some(true)) or only new (Some(false)) alerts.
    pub seen: Option<bool>,
    /// Smart search: the index and the search words as a CLIP vector.
    pub smart: Option<(Arc<crate::smart::SmartIndex>, crate::smart::Query)>,
    pub limit: usize,
}

/// One part of a text search.
#[derive(Debug, Clone, PartialEq)]
pub enum Term {
    /// An object, optionally with a colour: "red van" → colours [red],
    /// labels [car, truck], word "van".
    Object {
        colors: Vec<&'static str>,
        labels: Vec<&'static str>,
        word: String,
    },
    /// A colour on its own ("red"): any object of that colour.
    Color(Vec<&'static str>),
    /// Anything else: matched against descriptions, labels, camera name.
    Word(String),
}

/// Colours a searched colour also accepts (pictures of a white car often
/// measure as silver, etc.).
fn color_matches(c: &str) -> Option<Vec<&'static str>> {
    Some(match c {
        "red" | "maroon" => vec!["red", "pink"],
        "pink" => vec!["pink", "red"],
        "orange" => vec!["orange", "brown", "red"],
        "yellow" | "gold" => vec!["yellow", "orange"],
        "green" => vec!["green"],
        "blue" | "navy" => vec!["blue"],
        "purple" | "violet" => vec!["purple", "pink", "blue"],
        "brown" | "tan" | "beige" => vec!["brown", "beige", "orange"],
        "black" | "dark" => vec!["black", "grey"],
        "white" => vec!["white", "silver"],
        "grey" | "gray" => vec!["grey", "silver"],
        "silver" => vec!["silver", "grey", "white"],
        _ => return None,
    })
}

/// Object words people search for, and the detector labels they mean.
fn object_labels(w: &str) -> Option<Vec<&'static str>> {
    let w = w
        .strip_suffix("es")
        .filter(|x| x.ends_with("bus"))
        .unwrap_or(w);
    let w = if w.len() > 3 && w.ends_with('s') && !w.ends_with("ss") && w != "bus" {
        &w[..w.len() - 1]
    } else {
        w
    };
    Some(match w {
        "person" | "people" | "man" | "men" | "woman" | "women" | "kid" | "child" | "children"
        | "human" | "someone" | "guy" | "lady" | "courier" | "postman" | "visitor"
        | "pedestrian" => vec!["person"],
        "car" | "sedan" | "hatchback" | "suv" | "wagon" | "taxi" | "auto" | "automobile" => {
            vec!["car"]
        }
        "van" | "ute" | "pickup" | "4x4" => vec!["car", "truck"],
        "truck" | "lorry" => vec!["truck"],
        "bus" | "coach" => vec!["bus"],
        "vehicle" => vec!["car", "truck", "bus", "motorcycle"],
        "motorcycle" | "motorbike" | "scooter" => vec!["motorcycle"],
        "bike" => vec!["bicycle", "motorcycle"],
        "bicycle" | "cyclist" => vec!["bicycle"],
        "dog" | "puppy" | "pup" => vec!["dog"],
        "cat" | "kitten" | "kitty" => vec!["cat"],
        "bird" => vec!["bird"],
        "animal" | "pet" => vec!["dog", "cat", "bird", "horse", "sheep", "cow", "bear"],
        "horse" => vec!["horse"],
        "cow" => vec!["cow"],
        "sheep" => vec!["sheep"],
        "bag" => vec!["backpack", "handbag", "suitcase"],
        "backpack" => vec!["backpack"],
        "suitcase" => vec!["suitcase"],
        "umbrella" => vec!["umbrella"],
        _ => return None,
    })
}

/// Understand a search like "red van", "white car person", "black cat".
pub fn parse_text(q: &str) -> Vec<Term> {
    const SKIP: &[&str] = &[
        "a", "an", "the", "with", "of", "in", "on", "and", "or", "at", "coloured", "colored",
    ];
    let mut out = Vec::new();
    let mut colors: Vec<&'static str> = Vec::new();
    for w in q
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty() && !SKIP.contains(w))
    {
        if let Some(c) = color_matches(w) {
            colors.extend(c);
        } else if let Some(labels) = object_labels(w) {
            out.push(Term::Object {
                colors: std::mem::take(&mut colors),
                labels,
                word: w.to_string(),
            });
        } else {
            out.push(Term::Word(w.to_string()));
        }
    }
    if !colors.is_empty() {
        out.push(Term::Color(colors));
    }
    out
}

/// Does an event match every part of a text search?
pub fn text_matches(e: &Event, camera_name: &str, terms: &[Term]) -> bool {
    // The AI description, the smart-search summary and zone notes.
    let desc = format!("{}; {}; {}", e.desc, e.summary, e.notes.join("; ")).to_lowercase();
    // Description parts ("white delivery van; man in red jacket").
    let parts: Vec<&str> = desc.split(';').map(str::trim).collect();
    let has_word = |text: &str, w: &str| {
        text.split(|c: char| !c.is_alphanumeric())
            .any(|x| x == w || x.strip_suffix('s') == Some(w))
    };
    terms.iter().all(|t| match t {
        Term::Object {
            colors,
            labels,
            word,
        } => {
            let by_label = e.labels.iter().any(|l| {
                labels.contains(&l.label.as_str())
                    && (colors.is_empty()
                        || l.color.as_deref().is_some_and(|c| colors.contains(&c)))
            });
            let by_desc = parts.iter().any(|p| {
                (has_word(p, word) || labels.iter().any(|l| has_word(p, l)))
                    && (colors.is_empty() || colors.iter().any(|c| has_word(p, c)))
            });
            by_label || by_desc
        }
        Term::Color(colors) => {
            e.labels
                .iter()
                .any(|l| l.color.as_deref().is_some_and(|c| colors.contains(&c)))
                || colors.iter().any(|c| has_word(&desc, c))
        }
        Term::Word(w) => {
            has_word(&desc, w)
                || e.labels.iter().any(|l| l.label.contains(w.as_str()))
                || camera_name.to_lowercase().contains(w.as_str())
        }
    })
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
        let recs = recordings(&rt);
        // A text search only finds alerts.
        if q.text.is_empty() && (q.types.is_empty() || q.types.contains("recording")) {
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
                    seen: true,
                    score: None,
                });
            }
        }
        let scores = q
            .smart
            .as_ref()
            .map(|(ix, v)| ix.scores(&rt.events, v))
            .unwrap_or_default();
        for e in rt.events.between(q.from, q.to) {
            if !in_tod(e.start, q.tod) || !event_matches(&e, &q.types) {
                continue;
            }
            if let Some(want) = q.seen
                && mgr.seen.is_seen(&e.id, e.start) != want
            {
                continue;
            }
            let keyword = !q.text.is_empty() && text_matches(&e, &rt.cfg.name, &q.text);
            let score = scores.get(&e.id).copied();
            // Smart search: keyword matches, plus pictures that look like the
            // description (thresholded below, once all scores are known).
            if !q.text.is_empty() && !keyword && score.is_none() {
                continue;
            }
            let score = q.smart.as_ref().map(|_| match (score, keyword) {
                (Some(s), true) => s.max(0.0) + KEYWORD_BONUS,
                (Some(s), false) => s,
                (None, true) => KEYWORD_BONUS,
                (None, false) => f32::MIN,
            });
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
                seen: mgr.seen.is_seen(&e.id, e.start),
                score,
                event: Some(e),
            });
        }
    }
    if q.smart.is_some() {
        // Keep the alerts close to the best match, best first.
        let best = items
            .iter()
            .filter_map(|i| i.score)
            .fold(f32::MIN, f32::max);
        let cut = (best - SMART_SPREAD).max(SMART_MIN);
        items.retain(|i| i.score.is_some_and(|s| s >= cut));
        items.sort_by(|a, b| b.score.unwrap_or(0.0).total_cmp(&a.score.unwrap_or(0.0)));
    } else {
        items.sort_by(|a, b| b.start.cmp(&a.start));
    }
    items.truncate(q.limit);
    items
}

/// Smart search: a keyword match (e.g. the AI's own "dog" label) counts
/// this much extra.
const KEYWORD_BONUS: f32 = 0.05;
/// Smart search: results within this of the best match are shown...
const SMART_SPREAD: f32 = 0.06;
/// ...and only if the words fit better than "nothing of interest" does
/// (true matches are typically +0.03 to +0.08).
const SMART_MIN: f32 = 0.015;

#[derive(Debug, Clone, Serialize)]
pub struct Segment {
    pub start: DateTime<Local>,
    pub end: DateTime<Local>,
    pub url: String,
    pub name: String,
    /// "recording" (as recorded), "sub" (light copy for the timeline) or "clip".
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

#[cfg(test)]
mod text_tests {
    use super::*;
    use crate::events::Label;

    fn ev(labels: &[(&str, Option<&str>)], desc: &str) -> Event {
        Event {
            id: "e".into(),
            camera: "c".into(),
            start: Local::now(),
            end: Local::now(),
            labels: labels
                .iter()
                .map(|(l, c)| Label {
                    label: l.to_string(),
                    score: 0.9,
                    color: c.map(str::to_string),
                })
                .collect(),
            motion: true,
            thumb: None,
            clip: None,
            clip_start: None,
            focus: None,
            desc: desc.into(),
            boxes: Vec::new(),
            summary: String::new(),
            notes: Vec::new(),
        }
    }

    #[test]
    fn searches() {
        let m = |e: &Event, q: &str| text_matches(e, "Front Door", &parse_text(q));
        let white_car = ev(&[("car", Some("white"))], "");
        assert!(m(&white_car, "white van"));
        assert!(m(&white_car, "silver car"));
        assert!(!m(&white_car, "red van"));
        assert!(!m(&white_car, "black cat"));
        let cat = ev(&[("cat", Some("black"))], "");
        assert!(m(&cat, "black cat"));
        assert!(m(&cat, "Black Cats"));
        assert!(m(&cat, "kitten"));
        assert!(!m(&cat, "white cat"));
        // From a language model's description.
        let van = ev(
            &[("car", Some("grey"))],
            "red delivery van; man in blue jacket",
        );
        assert!(m(&van, "red van"));
        assert!(m(&van, "delivery"));
        assert!(m(&van, "front door van"));
        assert!(!m(&van, "ladder"));
        assert!(m(&ev(&[("person", None)], ""), "people"));
        assert!(m(&ev(&[("bus", None)], ""), "buses"));
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
