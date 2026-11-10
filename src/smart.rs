//! Smart search index: CLIP vectors of each alert picture (and of each
//! recognized object cut out of it), kept next to the alerts in
//! `events/smart.jsonl`, filled in the background at low priority.

use crate::clip::{self, Clip};
use crate::events::{Event, EventStore};
use crate::imgutil::Frame;
use crate::manager::Manager;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::info;

/// A smart search: the words and the "nothing of interest" descriptions as vectors.
pub struct Query {
    pub text: Vec<f32>,
    pub neutral: Vec<Vec<f32>>,
}

#[derive(Default)]
pub struct SmartIndex {
    /// events folder → (event id → vectors)
    dirs: Mutex<HashMap<PathBuf, HashMap<String, Vec<Vec<i8>>>>>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Line {
    id: String,
    v: Vec<String>,
}

impl SmartIndex {
    fn with<R>(&self, dir: &Path, f: impl FnOnce(&mut HashMap<String, Vec<Vec<i8>>>) -> R) -> R {
        let mut dirs = self.dirs.lock().unwrap();
        let m = dirs.entry(dir.to_path_buf()).or_insert_with(|| {
            std::fs::read_to_string(dir.join("smart.jsonl"))
                .map(|t| {
                    t.lines()
                        .filter_map(|l| serde_json::from_str::<Line>(l).ok())
                        .map(|l| (l.id, l.v.iter().filter_map(|s| clip::unpack(s)).collect()))
                        .collect()
                })
                .unwrap_or_default()
        });
        f(m)
    }

    pub fn has(&self, store: &EventStore, id: &str) -> bool {
        self.with(store.dir(), |m| m.contains_key(id))
    }

    pub fn count(&self, store: &EventStore) -> usize {
        self.with(store.dir(), |m| m.len())
    }

    fn add(&self, store: &EventStore, id: &str, vecs: &[Vec<f32>]) {
        let line = Line {
            id: id.to_string(),
            v: vecs.iter().map(|v| clip::pack(v)).collect(),
        };
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(store.dir().join("smart.jsonl"))
            && let Ok(s) = serde_json::to_string(&line)
        {
            let _ = writeln!(f, "{s}");
        }
        let packed: Vec<Vec<i8>> = line.v.iter().filter_map(|s| clip::unpack(s)).collect();
        self.with(store.dir(), |m| m.insert(id.to_string(), packed));
    }

    /// How much better the search fits each alert than "nothing of
    /// interest" does (best over the alert's pictures; > 0 = a match).
    pub fn scores(&self, store: &EventStore, q: &Query) -> HashMap<String, f32> {
        self.with(store.dir(), |m| {
            m.iter()
                .map(|(id, vs)| {
                    let best = vs
                        .iter()
                        .map(|v| {
                            let base = q
                                .neutral
                                .iter()
                                .map(|n| clip::similarity(n, v))
                                .fold(f32::MIN, f32::max);
                            clip::similarity(&q.text, v) - base
                        })
                        .fold(f32::MIN, f32::max);
                    (id.clone(), best)
                })
                .collect()
        })
    }
}

/// The parts of an alert picture worth describing: the whole view and each
/// recognized object (cut out with some surroundings).
fn pictures(f: &Frame, e: &Event) -> Vec<(Frame, Option<(String, Option<String>)>)> {
    let mut out = vec![(f.clone(), None)];
    let color = |l: &str| {
        e.labels
            .iter()
            .find(|x| x.label == l)
            .and_then(|x| x.color.clone())
    };
    let mut boxes: Vec<([f32; 4], Option<(String, Option<String>)>)> = e
        .boxes
        .iter()
        .filter(|b| b.label != "motion")
        .map(|b| {
            (
                [b.x, b.y, b.w, b.h],
                Some((b.label.clone(), color(&b.label))),
            )
        })
        .collect();
    if boxes.is_empty()
        && let Some(fb) = e.focus
    {
        boxes.push((
            fb,
            e.labels.first().map(|l| (l.label.clone(), l.color.clone())),
        ));
    }
    for ([x, y, w, h], what) in boxes.into_iter().take(4) {
        let pad = 0.15;
        let (fw, fh) = (f.width as f32, f.height as f32);
        let x0 = ((x - w * pad) * fw).clamp(0.0, fw - 1.0) as u32;
        let y0 = ((y - h * pad) * fh).clamp(0.0, fh - 1.0) as u32;
        let x1 = ((x + w * (1.0 + pad)) * fw).clamp(0.0, fw) as u32;
        let y1 = ((y + h * (1.0 + pad)) * fh).clamp(0.0, fh) as u32;
        if x1 <= x0 + 16 || y1 <= y0 + 16 {
            continue;
        }
        let (cw, ch) = (x1 - x0, y1 - y0);
        let mut rgb = Vec::with_capacity((cw * ch * 3) as usize);
        for row in y0..y1 {
            let s = ((row * f.width + x0) * 3) as usize;
            rgb.extend_from_slice(&f.rgb[s..s + (cw * 3) as usize]);
        }
        out.push((
            Frame {
                width: cw,
                height: ch,
                rgb: Arc::new(rgb),
            },
            what,
        ));
    }
    out
}

/// Choose the description from `options` that fits the picture best; None
/// when it isn't clearly better than the next, or the last ("nothing
/// special") option wins and `last_is_none` is set.
async fn pick(
    clip: &Clip,
    v: &[f32],
    options: &[&str],
    last_is_none: bool,
    margin: f32,
) -> Option<usize> {
    let mut scores = Vec::with_capacity(options.len());
    for o in options {
        let t = clip.embed_text(o).await.ok()?;
        scores.push(t.iter().zip(v).map(|(a, b)| a * b).sum::<f32>());
    }
    let mut idx: Vec<usize> = (0..scores.len()).collect();
    idx.sort_by(|a, b| scores[*b].total_cmp(&scores[*a]));
    let (best, second) = (idx[0], idx.get(1).copied().unwrap_or(idx[0]));
    if last_is_none && best == options.len() - 1 {
        return None;
    }
    (scores[best] - scores[second] >= margin).then_some(best)
}

const COLORS: &[&str] = &[
    "black", "white", "grey", "red", "blue", "green", "yellow", "orange", "pink", "brown",
];

fn refs(v: &[String]) -> Vec<&str> {
    v.iter().map(|s| s.as_str()).collect()
}

/// A short description of one recognized object from its picture.
async fn describe_object(clip: &Clip, v: &[f32], label: &str, color: Option<&str>) -> String {
    match label {
        "person" => {
            let mut parts = Vec::new();
            let who = match pick(clip, v, &["a child", "an adult"], false, 0.01).await {
                Some(0) => "Child",
                Some(_) => "Adult",
                None => "Person",
            };
            let tops: Vec<String> = COLORS
                .iter()
                .map(|c| format!("a person wearing a {c} top"))
                .collect();
            if let Some(i) = pick(clip, v, &refs(&tops), false, 0.008).await {
                parts.push(format!("in a {} top", COLORS[i]));
            }
            // Each extra is tested against its opposite ("wearing a cap" vs
            // "with no hat"): more reliable than choosing among several.
            const EXTRAS: &[(&str, &str, &str)] = &[
                (
                    "carrying a box",
                    "a person carrying a cardboard box",
                    "a person with empty hands",
                ),
                (
                    "carrying a bag",
                    "a person carrying a bag",
                    "a person with empty hands",
                ),
                (
                    "wearing a backpack",
                    "a person wearing a backpack",
                    "a person without a backpack",
                ),
                (
                    "wearing a hi-vis vest",
                    "a person wearing a high-visibility safety vest",
                    "a person in ordinary clothes",
                ),
                (
                    "wearing a cap",
                    "a person wearing a baseball cap",
                    "a person with no hat",
                ),
                (
                    "with the hood up",
                    "a person with a hood over their head",
                    "a person with their head uncovered",
                ),
                (
                    "wearing a face mask",
                    "a person wearing a face mask",
                    "a person with their face uncovered",
                ),
            ];
            for (text, yes, no) in EXTRAS {
                if pick(clip, v, &[yes, no], false, 0.02).await == Some(0) {
                    parts.push(text.to_string());
                }
            }
            if parts.is_empty() {
                who.to_string()
            } else {
                format!("{who} {}", parts.join(", "))
            }
        }
        "car" | "truck" | "bus" | "motorcycle" => {
            let kinds = [
                "van",
                "ute",
                "SUV",
                "sedan",
                "hatchback",
                "truck",
                "motorbike",
            ];
            let q = [
                "a van",
                "a pickup truck (ute)",
                "an SUV",
                "a sedan car",
                "a hatchback car",
                "a large truck",
                "a motorbike",
            ];
            let kind = pick(clip, v, &q, false, 0.008)
                .await
                .map(|i| kinds[i])
                .unwrap_or(label);
            match color {
                Some(c) => format!("{} {kind}", capitalize(c)),
                None => capitalize(kind),
            }
        }
        other => match color {
            Some(c) => format!("{} {other}", capitalize(c)),
            None => capitalize(other),
        },
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

fn fmt_secs(s: i64) -> String {
    if s < 60 {
        format!("{s} s")
    } else {
        format!("{} min {} s", s / 60, s % 60)
    }
}

/// Background job: describe new alerts (newest first) while smart search is on.
pub async fn run(mgr: Arc<Manager>) {
    let count = |mgr: &Manager| {
        mgr.event_stores()
            .values()
            .map(|s| mgr.smart.count(s))
            .sum::<usize>()
    };
    mgr.clip.status.lock().unwrap().indexed = count(&mgr);
    tokio::time::sleep(Duration::from_secs(30)).await;
    let mut announced = false;
    loop {
        if mgr.config().ai.smart_search {
            let mut done = 0;
            let cutoff = chrono::Local::now() - chrono::Duration::days(60);
            'stores: for store in mgr.event_stores().into_values() {
                let mut todo: Vec<Event> = store
                    .all()
                    .into_iter()
                    .filter(|e| e.start > cutoff && e.thumb.is_some())
                    // Finished alerts only (their picture won't change any more).
                    .filter(|e| (chrono::Local::now() - e.end).num_seconds() > 30)
                    .filter(|e| !mgr.smart.has(&store, &e.id) || e.summary.is_empty())
                    .collect();
                todo.sort_by(|a, b| b.start.cmp(&a.start));
                for e in todo.into_iter().take(40) {
                    if !mgr.config().ai.smart_search {
                        break 'stores;
                    }
                    crate::load::wait_for_cpu(75, Duration::from_secs(300)).await;
                    let Ok(bytes) = std::fs::read(store.dir().join(e.thumb.as_ref().unwrap()))
                    else {
                        continue;
                    };
                    let Ok(f) = crate::imgutil::decode_jpeg(&bytes) else {
                        continue;
                    };
                    let mut vecs = Vec::new();
                    let mut objects = Vec::new();
                    for (p, what) in pictures(&f, &e) {
                        match mgr.clip.embed_image(&p).await {
                            Ok(v) => {
                                if let Some((label, color)) = what {
                                    objects.push(
                                        describe_object(&mgr.clip, &v, &label, color.as_deref())
                                            .await,
                                    );
                                }
                                vecs.push(v);
                            }
                            Err(_) => break 'stores, // model unavailable; status shows why
                        }
                    }
                    // What happened, for the alert's side panel and notifications.
                    let secs = (e.end - e.start).num_seconds().max(1);
                    let what = if !e.desc.is_empty() {
                        capitalize(&e.desc)
                    } else if !objects.is_empty() {
                        // "2 × Adult in a black top" rather than repeating it.
                        let mut uniq: Vec<(String, usize)> = Vec::new();
                        for o in &objects {
                            match uniq.iter_mut().find(|(t, _)| t == o) {
                                Some((_, n)) => *n += 1,
                                None => uniq.push((o.clone(), 1)),
                            }
                        }
                        uniq.iter()
                            .map(|(t, n)| {
                                if *n > 1 {
                                    format!("{n} × {t}")
                                } else {
                                    t.clone()
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("; ")
                    } else {
                        "Motion".to_string()
                    };
                    let cam = mgr
                        .camera(&e.camera)
                        .map(|c| c.cfg.name.clone())
                        .unwrap_or_default();
                    tracing::info!(target: "event", "[{cam}] alert described: {what} ({})", fmt_secs(secs));
                    let mut updated = e.clone();
                    let notes = if e.notes.is_empty() {
                        String::new()
                    } else {
                        format!(". {}", e.notes.join("; "))
                    };
                    updated.summary = format!("{what} — {}{notes}", fmt_secs(secs));
                    store.save(updated);
                    if !announced {
                        info!("smart search: describing alert pictures in the background");
                        announced = true;
                    }
                    mgr.smart.add(&store, &e.id, &vecs);
                    done += 1;
                }
            }
            if done > 0 {
                info!("smart search: {done} alert(s) described");
            }
            mgr.clip.status.lock().unwrap().indexed = count(&mgr);
        }
        mgr.clip.unload_if_idle();
        tokio::time::sleep(Duration::from_secs(20)).await;
    }
}
