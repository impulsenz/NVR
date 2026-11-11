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

pub struct SmartIndex {
    /// events folder → (event id → vectors)
    dirs: Mutex<HashMap<PathBuf, HashMap<String, Vec<Vec<i8>>>>>,
    /// Index file name for the current model.
    file: Mutex<String>,
}

impl Default for SmartIndex {
    fn default() -> Self {
        Self {
            dirs: Mutex::default(),
            file: Mutex::new("smart.jsonl".into()),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Line {
    id: String,
    v: Vec<String>,
}

impl SmartIndex {
    /// Use another model's index (different models' vectors don't mix).
    pub fn set_file(&self, f: &str) {
        let mut cur = self.file.lock().unwrap();
        if *cur != f {
            *cur = f.to_string();
            self.dirs.lock().unwrap().clear();
        }
    }

    fn with<R>(&self, dir: &Path, f: impl FnOnce(&mut HashMap<String, Vec<Vec<i8>>>) -> R) -> R {
        let mut dirs = self.dirs.lock().unwrap();
        let m = dirs.entry(dir.to_path_buf()).or_insert_with(|| {
            std::fs::read_to_string(dir.join(&*self.file.lock().unwrap()))
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
            .open(store.dir().join(&*self.file.lock().unwrap()))
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
/// How much a picture looks like a parcel on the ground (vs. an empty spot).
async fn parcel_score(clip: &Clip, f: &Frame) -> Option<f32> {
    const YES: &[&str] = &[
        "a photo of a parcel on the ground",
        "a cardboard box on a doorstep",
        "a delivered package by the front door",
        "a plastic delivery bag on the ground",
    ];
    const NO: &[&str] = &[
        "a photo of an empty doorstep",
        "an empty porch floor",
        "an empty path",
        "a doormat on the ground",
    ];
    let v = clip.embed_image(f).await.ok()?;
    let best = |opts: &'static [&'static str]| {
        let v = v.clone();
        async move {
            let mut m = f32::MIN;
            for o in opts {
                let t = clip.embed_text(o).await.ok()?;
                m = m.max(t.iter().zip(&v).map(|(a, b)| a * b).sum::<f32>());
            }
            Some(m)
        }
    };
    let yes = best(YES).await?;
    let no = best(NO).await?;
    Some(yes - no)
}

/// What was seen across the pictures of one alert, as (description,
/// count). Of each kind of object there are as many as were seen together
/// in one picture; the most detailed descriptions are used first ("Adult in
/// a black top" rather than "Person"), and "Adult" seen in one picture and
/// "Adult in a black top" in another count as one.
fn merge_objects(per_picture: &[Vec<(String, String)>]) -> Vec<(String, usize)> {
    let mut labels: Vec<&str> = Vec::new();
    for (l, _) in per_picture.iter().flatten() {
        if !labels.contains(&l.as_str()) {
            labels.push(l);
        }
    }
    let mut out: Vec<(String, usize)> = Vec::new();
    for label in labels {
        let n = per_picture
            .iter()
            .map(|p| p.iter().filter(|(l, _)| l == label).count())
            .max()
            .unwrap_or(0);
        let generic = capitalize(label);
        let mut descs: Vec<&str> = Vec::new();
        for (l, d) in per_picture.iter().flatten() {
            if l == label && *d != generic && !descs.contains(&d.as_str()) {
                descs.push(d);
            }
        }
        let all = descs.clone();
        descs.retain(|t| {
            !all.iter()
                .any(|o| o.len() > t.len() && o.starts_with(&format!("{t} ")))
        });
        descs.sort_by_key(|d| std::cmp::Reverse(d.len()));
        // Several pictures of one person can give different details: no
        // more descriptions than people.
        descs.truncate(n);
        let mut group: Vec<(String, usize)> = Vec::new();
        for d in &descs {
            match group.iter_mut().find(|(t, _)| t == d) {
                Some((_, c)) => *c += 1,
                None => group.push((d.to_string(), 1)),
            }
        }
        // Within one picture the same description twice is two of them.
        for (t, c) in group.iter_mut() {
            let together = per_picture
                .iter()
                .map(|p| p.iter().filter(|(l, d)| l == label && d == t).count())
                .max()
                .unwrap_or(1);
            *c = (*c).max(together).min(n);
        }
        let used: usize = group.iter().map(|(_, c)| c).sum();
        if used < n {
            group.push((generic, n - used));
        }
        out.extend(group);
    }
    out
}

/// A quick description of an alert's picture ("Adult in a black top;
/// White car"), for a notification sent while the alert is going on.
pub async fn describe_now(mgr: &Manager, e: &Event) -> Option<String> {
    if !mgr.config().ai.smart_search {
        return None;
    }
    let rt = mgr.camera(&e.camera)?;
    let bytes = std::fs::read(rt.events.dir().join(e.thumb.as_ref()?)).ok()?;
    let f = crate::imgutil::decode_jpeg(&bytes).ok()?;
    let mut objs = Vec::new();
    for (p, what) in pictures(&f, e) {
        if let Some((label, color)) = what {
            let v = mgr.clip.embed_image(&p).await.ok()?;
            let d = describe_object(&mgr.clip, &v, &label, color.as_deref()).await;
            objs.push((label, d));
        }
    }
    let m = merge_objects(&[objs]);
    (!m.is_empty()).then(|| {
        m.iter()
            .map(|(t, n)| {
                if *n > 1 {
                    format!("{n} × {t}")
                } else {
                    t.clone()
                }
            })
            .collect::<Vec<_>>()
            .join("; ")
    })
}

/// A parcel appeared between `before` and `after` (parcel scores).
fn is_parcel_change(before: f32, after: f32) -> bool {
    after > 0.01 && after - before > 0.015
}

/// Item left or taken in a watched area: was it a parcel delivered or
/// picked up? Rewrites the alert's note and its "package" label.
async fn check_items(clip: &Clip, store: &EventStore, e: &mut Event) {
    let mut any_package = false;
    for i in 0..e.items.len() {
        if e.items[i].checked {
            any_package |= e
                .notes
                .iter()
                .any(|n| n.starts_with("Package") && n.contains(&e.items[i].zone));
            continue;
        }
        let it = e.items[i].clone();
        let load = |n: &str| {
            std::fs::read(store.dir().join(n))
                .ok()
                .and_then(|b| crate::imgutil::decode_jpeg(&b).ok())
        };
        let (Some(b), Some(a)) = (load(&it.before), load(&it.after)) else {
            e.items[i].checked = true;
            continue;
        };
        let (Some(sb), Some(sa)) = (parcel_score(clip, &b).await, parcel_score(clip, &a).await)
        else {
            return; // model unavailable: try again later
        };
        let verdict = if is_parcel_change(sb, sa) {
            Some(format!("Package delivered at {}", it.zone))
        } else if is_parcel_change(sa, sb) {
            Some(format!("Package picked up from {}", it.zone))
        } else {
            None
        };
        if let Some(v) = &verdict {
            let prefix = format!("Item left or taken in {}", it.zone);
            e.notes.retain(|n| !n.starts_with(&prefix));
            if !e.notes.contains(v) {
                e.notes.push(v.clone());
            }
            tracing::info!(target: "event", "[{}] {v}", e.camera);
            any_package = true;
        }
        e.items[i].checked = true;
    }
    if !e.items.is_empty() && !any_package {
        // Something changed, but it doesn't look like a parcel.
        e.labels.retain(|l| l.label != "package" || l.score < 1.0);
    }
}

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
        // The chosen model, and its own index.
        mgr.clip.set_model(&mgr.config().ai.smart_model);
        mgr.smart.set_file(&mgr.clip.index_file());
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
                    .filter(|e| !mgr.camera(&e.camera).is_some_and(|c| c.in_progress(e)))
                    .filter(|e| !mgr.smart.has(&store, &e.id) || e.summary.is_empty())
                    // Faces / plates first, so names go into the description.
                    .filter(|e| {
                        let ai = mgr.config().ai;
                        !(ai.faces || ai.plates)
                            || e.identified
                            || !e.boxes.iter().any(|b| {
                                ["person", "car", "truck", "bus", "motorcycle"]
                                    .contains(&b.label.as_str())
                            })
                            || (chrono::Local::now() - e.end).num_minutes() > 10
                    })
                    .collect();
                todo.sort_by(|a, b| b.start.cmp(&a.start));
                todo.truncate(40);
                if todo.is_empty() {
                    continue;
                }
                let job = crate::jobs::start("ai", "Smart search — describing alerts");
                job.total(todo.len() as u64);
                for mut e in todo {
                    if !mgr.config().ai.smart_search {
                        break 'stores;
                    }
                    job.waiting();
                    crate::load::wait_for_cpu(75, Duration::from_secs(300)).await;
                    job.pause_point().await;
                    job.step(format!(
                        "{} — {}",
                        mgr.camera(&e.camera)
                            .map(|c| c.cfg.name.clone())
                            .unwrap_or_default(),
                        e.start.format("%d/%m %H:%M:%S")
                    ));
                    job.done_one();
                    let Ok(bytes) = std::fs::read(store.dir().join(e.thumb.as_ref().unwrap()))
                    else {
                        continue;
                    };
                    let Ok(f) = crate::imgutil::decode_jpeg(&bytes) else {
                        continue;
                    };
                    check_items(&mgr.clip, &store, &mut e).await;
                    let mut vecs = Vec::new();
                    // Descriptions per picture: the best moment first, then
                    // more moments from the full-quality video.
                    let mut per_picture: Vec<Vec<(String, String)>> = Vec::new();
                    let extra = match mgr.camera(&e.camera) {
                        Some(rt) if mgr.config().ai.summary_frames > 1 => {
                            crate::identify::timed_moments(
                                &mgr,
                                &rt,
                                &e,
                                mgr.config().ai.summary_frames as usize - 1,
                            )
                            .await
                        }
                        _ => Vec::new(),
                    };
                    // Every picture in time order, for a vision model.
                    let mut timeline: Vec<(
                        chrono::DateTime<chrono::Local>,
                        crate::imgutil::Frame,
                    )> = extra.iter().map(|(t, f, _)| (*t, f.clone())).collect();
                    timeline.push((e.thumb_at.unwrap_or(e.start), f.clone()));
                    timeline.sort_by_key(|(t, _)| *t);
                    let mut frames = vec![(f, e.clone())];
                    for (_, xf, boxes) in extra {
                        // Without boxes there's nothing new to describe.
                        if boxes.is_empty() {
                            continue;
                        }
                        let mut x = e.clone();
                        x.boxes = boxes;
                        x.focus = None;
                        frames.push((xf, x));
                    }
                    for (fr, ev) in &frames {
                        let mut objects = Vec::new();
                        for (p, what) in pictures(fr, ev) {
                            match mgr.clip.embed_image(&p).await {
                                Ok(v) => {
                                    if let Some((label, color)) = what {
                                        let d = describe_object(
                                            &mgr.clip,
                                            &v,
                                            &label,
                                            color.as_deref(),
                                        )
                                        .await;
                                        objects.push((label, d));
                                    }
                                    vecs.push(v);
                                }
                                Err(err) => {
                                    // Model unavailable; the AI settings show why.
                                    job.fail(format!("{err:#}"));
                                    break 'stores;
                                }
                            }
                        }
                        per_picture.push(objects);
                    }
                    let objects = merge_objects(&per_picture);
                    // What happened, for the alert's side panel and notifications.
                    let secs = (e.end - e.start).num_seconds().max(1);
                    let what = if !e.desc.is_empty() {
                        capitalize(&e.desc)
                    } else if !objects.is_empty() {
                        objects
                            .iter()
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
                    // A vision model writes a proper description when one is
                    // set up; the built-in one stays if it can't.
                    let ai = mgr.config().ai;
                    let mut judged = None;
                    let what = if crate::describe::enabled(&ai) {
                        let info = crate::describe::AlertInfo::of(&e, &cam, &mgr.config());
                        let pics: Vec<_> = timeline.into_iter().map(|(_, f)| f).collect();
                        match crate::describe::describe(&ai, &pics, &info).await {
                            Ok(d) => {
                                judged = Some(d.suspicious.unwrap_or_default());
                                d.text.trim_end_matches('.').to_string()
                            }
                            Err(err) => {
                                job.problem(format!("vision model: {err:#}"));
                                what
                            }
                        }
                    } else {
                        what
                    };
                    tracing::info!(target: "event", "[{cam}] alert described: {what} ({})", fmt_secs(secs));
                    let mut updated = e.clone();
                    let notes = if e.notes.is_empty() {
                        String::new()
                    } else {
                        format!(". {}", e.notes.join("; "))
                    };
                    updated.summary = format!("{what} — {}{notes}", fmt_secs(secs));
                    // The vision model's judgement, for notifications.
                    if let Some(sus) = judged {
                        updated.suspicious = sus;
                    }
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

#[cfg(test)]
mod parcel_tests {
    /// Needs the CLIP model: GAZE_TEST_MODELS=<dir with clip-vit-b32-*> GAZE_TEST_PARCEL=<dir with before.jpg, after.jpg>
    #[test]
    fn merging_pictures() {
        let p = |v: &[(&str, &str)]| {
            v.iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect::<Vec<_>>()
        };
        let m = super::merge_objects(&[
            p(&[("person", "Adult in a black top"), ("person", "Person")]),
            p(&[("person", "Person"), ("person", "Person")]),
            p(&[("person", "Adult"), ("car", "White car")]),
        ]);
        assert_eq!(
            m,
            [
                ("Adult in a black top".to_string(), 1),
                ("Person".into(), 1),
                ("White car".into(), 1)
            ]
        );
        let m = super::merge_objects(&[
            p(&[("dog", "Dog"), ("dog", "Dog")]),
            p(&[("dog", "Brown dog")]),
        ]);
        assert_eq!(m, [("Brown dog".to_string(), 1), ("Dog".into(), 1)]);
        let m = super::merge_objects(&[p(&[("person", "Child"), ("person", "Child")])]);
        assert_eq!(m, [("Child".to_string(), 2)]);
    }

    #[tokio::test]
    #[ignore]
    async fn parcel_seen() {
        let (Ok(models), Ok(dir)) = (
            std::env::var("GAZE_TEST_MODELS"),
            std::env::var("GAZE_TEST_PARCEL"),
        ) else {
            return;
        };
        let clip = super::Clip::new(std::path::Path::new(&models));
        let load = |n: &str| {
            crate::imgutil::decode_jpeg(&std::fs::read(std::path::Path::new(&dir).join(n)).unwrap())
                .unwrap()
        };
        let b = super::parcel_score(&clip, &load("before.jpg"))
            .await
            .unwrap();
        let a = super::parcel_score(&clip, &load("after.jpg"))
            .await
            .unwrap();
        println!("parcel score before {b:.3}, after {a:.3}");
        assert!(super::is_parcel_change(b, a) && !super::is_parcel_change(a, b));

        // The whole check on an alert: the note and label are rewritten.
        let tmp = std::env::temp_dir().join(format!("nvr-parcel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let store = crate::events::EventStore::open(&tmp);
        std::fs::create_dir_all(store.dir()).unwrap();
        for (from, to) in [
            ("before.jpg", "e-z0-before.jpg"),
            ("after.jpg", "e-z0-after.jpg"),
        ] {
            std::fs::copy(std::path::Path::new(&dir).join(from), store.dir().join(to)).unwrap();
        }
        let now = chrono::Local::now();
        let mut e = crate::events::Event {
            id: "e".into(),
            camera: "cam".into(),
            start: now,
            end: now,
            labels: vec![crate::events::Label {
                label: "package".into(),
                score: 1.0,
                color: None,
            }],
            motion: true,
            thumb: None,
            clip: None,
            clip_start: None,
            focus: None,
            desc: String::new(),
            boxes: Vec::new(),
            summary: String::new(),
            notes: vec!["Item left or taken in Porch (possible delivery)".into()],
            thumb_at: None,
            faces: Vec::new(),
            plates: Vec::new(),
            identified: false,
            items: vec![crate::events::ItemChange {
                zone: "Porch".into(),
                before: "e-z0-before.jpg".into(),
                after: "e-z0-after.jpg".into(),
                checked: false,
            }],
            notified: 0,
            suspicious: String::new(),
        };
        super::check_items(&clip, &store, &mut e).await;
        assert_eq!(e.notes, ["Package delivered at Porch"]);
        assert!(e.items[0].checked && e.labels.iter().any(|l| l.label == "package"));
        // Swapped pictures: picked up.
        e.items[0] = crate::events::ItemChange {
            zone: "Porch".into(),
            before: "e-z0-after.jpg".into(),
            after: "e-z0-before.jpg".into(),
            checked: false,
        };
        e.notes = vec!["Item left or taken in Porch".into()];
        super::check_items(&clip, &store, &mut e).await;
        assert_eq!(e.notes, ["Package picked up from Porch"]);
        std::fs::remove_dir_all(&tmp).ok();
    }
}
