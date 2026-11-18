//! Learned states: teach GAZE to tell what a camera sees apart, from a few
//! examples you show it. "Garage door: open / closed", "Bin: out / in",
//! "Gate", "Porch light", "Car in the driveway".
//!
//! You choose a camera and (optionally) the piece of the picture that
//! matters, then press *Add example* while the scene is in each state. GAZE
//! turns each example into CLIP numbers (`clip.rs`, the model description
//! search already uses: no training, nothing leaves this computer) and, every
//! few seconds, compares the camera's current picture with the examples:
//! the closest label wins. A change must be seen twice in a row before it
//! counts, so a passing shadow doesn't flip it.
//!
//! The result is shown on the page, sent to MQTT (so Home Assistant gets a
//! sensor) and, if asked, notified when it changes. It never touches
//! recording, detection or alerts. Kept in `nvr-data/states.json`, example
//! pictures in `nvr-data/states/<id>/`.

use crate::sync::{MutexExt, RwLockExt};
use anyhow::{Result, bail};
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

/// Examples kept per label (more adds little and costs time).
pub const MAX_EXAMPLES: usize = 20;
/// A label needs at least this much resemblance to count at all.
const MIN_SCORE: f32 = 0.5;
/// How much closer the best label must be than the runner-up. CLIP pictures
/// of the same scene are all very similar, so this is small.
const MIN_MARGIN: f32 = 0.01;
/// Readings in a row that must agree before the state changes.
const CONFIRM: u8 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Example {
    /// CLIP numbers, f32 little-endian, base64.
    pub v: String,
    /// Picture file name in the state's folder.
    pub thumb: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LabelSet {
    pub name: String,
    #[serde(default)]
    pub examples: Vec<Example>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    pub id: String,
    pub camera: String,
    pub name: String,
    /// The piece of the picture that matters: x, y, width, height as 0..1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<[f32; 4]>,
    /// Seconds between looks.
    #[serde(default = "default_interval")]
    pub interval_secs: u32,
    /// Notify when it changes from one known state to another.
    #[serde(default)]
    pub notify: bool,
    #[serde(default)]
    pub labels: Vec<LabelSet>,
}

fn default_interval() -> u32 {
    10
}

impl State {
    /// Needs two labels with examples to tell anything apart.
    pub fn ready(&self) -> bool {
        self.labels.iter().filter(|l| !l.examples.is_empty()).count() >= 2
    }
}

/// What the latest looks say.
#[derive(Debug, Clone, Serialize, Default)]
pub struct Current {
    /// The confirmed state (None: not known yet).
    pub label: Option<String>,
    /// How close the best label was (0..1) at the last look.
    pub score: f32,
    pub since: Option<DateTime<Local>>,
    pub checked: Option<DateTime<Local>>,
    /// Why there is no answer, when there isn't.
    pub note: String,
}

pub struct StateStore {
    file: PathBuf,
    pub dir: PathBuf,
    items: RwLock<Vec<State>>,
    current: Mutex<HashMap<String, Current>>,
}

pub fn pack(v: &[f32]) -> String {
    use base64::Engine;
    let b: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
    base64::engine::general_purpose::STANDARD.encode(b)
}

pub fn unpack(s: &str) -> Vec<f32> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map(|b| b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
        .unwrap_or_default()
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut d, mut na, mut nb) = (0f32, 0f32, 0f32);
    for (x, y) in a.iter().zip(b) {
        d += x * y;
        na += x * x;
        nb += y * y;
    }
    d / (na.sqrt() * nb.sqrt()).max(1e-6)
}

// ---------- the decision ----------

#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub label: Option<String>,
    pub score: f32,
    pub margin: f32,
}

/// Compare one picture's numbers with every label's examples. A label's
/// score is the mean of its closest (up to three) examples, so one odd
/// example can't decide on its own and a label with many examples isn't
/// favoured over one with few.
pub fn classify(labels: &[(String, Vec<Vec<f32>>)], v: &[f32]) -> Verdict {
    let mut scored: Vec<(&str, f32)> = labels
        .iter()
        .filter(|(_, ex)| !ex.is_empty())
        .map(|(name, ex)| {
            let mut s: Vec<f32> = ex.iter().map(|e| cosine(e, v)).collect();
            s.sort_by(|a, b| b.total_cmp(a));
            let top = &s[..s.len().min(3)];
            (name.as_str(), top.iter().sum::<f32>() / top.len() as f32)
        })
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    match scored.as_slice() {
        [] | [_] => Verdict { label: None, score: 0.0, margin: 0.0 },
        [best, second, ..] => {
            let margin = best.1 - second.1;
            let ok = best.1 >= MIN_SCORE && margin >= MIN_MARGIN;
            Verdict { label: ok.then(|| best.0.to_string()), score: best.1, margin }
        }
    }
}

/// A change counts once it has been seen `CONFIRM` times in a row.
#[derive(Debug, Default)]
pub struct Debounce {
    pending: Option<(String, u8)>,
}

impl Debounce {
    /// Feed one look. Returns the label to switch to, when it's time.
    pub fn feed(&mut self, current: Option<&str>, seen: Option<&str>) -> Option<String> {
        match seen {
            // Undecided looks change nothing and cancel a half-way change.
            None => {
                self.pending = None;
                None
            }
            Some(l) if Some(l) == current => {
                self.pending = None;
                None
            }
            Some(l) => {
                let n = match &self.pending {
                    Some((p, n)) if p == l => n + 1,
                    _ => 1,
                };
                if n >= CONFIRM {
                    self.pending = None;
                    Some(l.to_string())
                } else {
                    self.pending = Some((l.to_string(), n));
                    None
                }
            }
        }
    }
}

// ---------- the store ----------

/// What the page gets (no numbers).
#[derive(Debug, Clone, Serialize)]
pub struct StateInfo {
    pub id: String,
    pub camera: String,
    pub name: String,
    pub region: Option<[f32; 4]>,
    pub interval_secs: u32,
    pub notify: bool,
    pub ready: bool,
    pub labels: Vec<LabelInfo>,
    pub current: Current,
}

#[derive(Debug, Clone, Serialize)]
pub struct LabelInfo {
    pub name: String,
    /// Example picture file names.
    pub examples: Vec<String>,
}

fn clean(s: &str, what: &str) -> Result<String> {
    let s = s.trim();
    if s.is_empty() || s.chars().count() > 40 || s.chars().any(|c| c.is_control()) {
        bail!("{what}: 1 to 40 characters");
    }
    Ok(s.to_string())
}

fn region_ok(r: Option<[f32; 4]>) -> Result<Option<[f32; 4]>> {
    let Some([x, y, w, h]) = r else { return Ok(None) };
    if [x, y, w, h].iter().any(|v| !v.is_finite()) || x < 0.0 || y < 0.0 || w < 0.02 || h < 0.02 || x + w > 1.001 || y + h > 1.001 {
        bail!("the area must lie inside the picture");
    }
    Ok(Some([x, y, w, h]))
}

impl StateStore {
    pub fn open(data_dir: &Path) -> Self {
        let file = data_dir.join("states.json");
        let items = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Self { file, dir: data_dir.join("states"), items: RwLock::new(items), current: Mutex::new(HashMap::new()) }
    }

    fn save(&self) -> Result<()> {
        let b = serde_json::to_vec_pretty(&*self.items.reader())?;
        let tmp = self.file.with_extension("json.tmp");
        std::fs::write(&tmp, b)?;
        std::fs::rename(&tmp, &self.file)?;
        Ok(())
    }

    pub fn all(&self) -> Vec<State> {
        self.items.reader().clone()
    }

    pub fn get(&self, id: &str) -> Option<State> {
        self.items.reader().iter().find(|s| s.id == id).cloned()
    }

    pub fn info(&self, s: &State) -> StateInfo {
        StateInfo {
            id: s.id.clone(),
            camera: s.camera.clone(),
            name: s.name.clone(),
            region: s.region,
            interval_secs: s.interval_secs,
            notify: s.notify,
            ready: s.ready(),
            labels: s.labels.iter().map(|l| LabelInfo { name: l.name.clone(), examples: l.examples.iter().map(|e| e.thumb.clone()).collect() }).collect(),
            current: self.current.locked().get(&s.id).cloned().unwrap_or_default(),
        }
    }

    pub fn current(&self, id: &str) -> Current {
        self.current.locked().get(id).cloned().unwrap_or_default()
    }

    pub fn create(&self, camera: &str, name: &str, region: Option<[f32; 4]>, interval_secs: u32, notify: bool, labels: &[String]) -> Result<String> {
        let name = clean(name, "name")?;
        let mut ls: Vec<LabelSet> = Vec::new();
        for l in labels {
            let l = clean(l, "state name")?;
            if ls.iter().any(|x| x.name.eq_ignore_ascii_case(&l)) {
                bail!("two states are called \"{l}\"");
            }
            ls.push(LabelSet { name: l, examples: Vec::new() });
        }
        if self.items.reader().len() >= 50 {
            bail!("at most 50 learned states");
        }
        use rand_core::RngCore;
        let mut b = [0u8; 6];
        rand_core::OsRng.fill_bytes(&mut b);
        let id: String = b.iter().map(|x| format!("{x:02x}")).collect();
        let s = State { id: id.clone(), camera: camera.to_string(), name, region: region_ok(region)?, interval_secs: interval_secs.clamp(2, 3600), notify, labels: ls };
        self.items.writer().push(s);
        self.save()?;
        Ok(id)
    }

    /// Change a state's settings; a changed area makes the examples stale
    /// (they were of another piece of the picture), so they're dropped.
    pub fn update(&self, id: &str, name: &str, region: Option<[f32; 4]>, interval_secs: u32, notify: bool) -> Result<()> {
        let name = clean(name, "name")?;
        let region = region_ok(region)?;
        {
            let mut it = self.items.writer();
            let s = it.iter_mut().find(|s| s.id == id).ok_or_else(|| anyhow::anyhow!("no such state"))?;
            let moved = s.region != region;
            s.name = name;
            s.interval_secs = interval_secs.clamp(2, 3600);
            s.notify = notify;
            if moved {
                s.region = region;
                for l in &mut s.labels {
                    l.examples.clear();
                }
                let _ = std::fs::remove_dir_all(self.dir.join(id));
            }
        }
        self.current.locked().remove(id);
        self.save()
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        self.items.writer().retain(|s| s.id != id);
        self.current.locked().remove(id);
        let _ = std::fs::remove_dir_all(self.dir.join(id));
        self.save()
    }

    /// Add an example to a label (created if new).
    pub fn add_example(&self, id: &str, label: &str, v: &[f32], jpeg: &[u8]) -> Result<()> {
        let label = clean(label, "state name")?;
        let file = {
            let mut it = self.items.writer();
            let s = it.iter_mut().find(|s| s.id == id).ok_or_else(|| anyhow::anyhow!("no such state"))?;
            let pos = match s.labels.iter().position(|l| l.name.eq_ignore_ascii_case(&label)) {
                Some(p) => p,
                None => {
                    if s.labels.len() >= 8 {
                        bail!("at most 8 states per item");
                    }
                    s.labels.push(LabelSet { name: label.clone(), examples: Vec::new() });
                    s.labels.len() - 1
                }
            };
            let l = &mut s.labels[pos];
            if l.examples.len() >= MAX_EXAMPLES {
                bail!("{} examples already: remove one first", MAX_EXAMPLES);
            }
            let file = format!("{}.jpg", Local::now().timestamp_millis());
            l.examples.push(Example { v: pack(v), thumb: file.clone() });
            file
        };
        let d = self.dir.join(id);
        std::fs::create_dir_all(&d)?;
        std::fs::write(d.join(&file), jpeg)?;
        self.current.locked().remove(id);
        self.save()
    }

    pub fn remove_example(&self, id: &str, thumb: &str) -> Result<()> {
        {
            let mut it = self.items.writer();
            let s = it.iter_mut().find(|s| s.id == id).ok_or_else(|| anyhow::anyhow!("no such state"))?;
            for l in &mut s.labels {
                l.examples.retain(|e| e.thumb != thumb);
            }
            s.labels.retain(|l| !l.examples.is_empty());
        }
        // (The name is a file name we made: never a path.)
        if !thumb.contains(['/', '\\']) && !thumb.contains("..") {
            let _ = std::fs::remove_file(self.dir.join(id).join(thumb));
        }
        self.current.locked().remove(id);
        self.save()
    }

    /// An example picture's path (only a file the state lists).
    pub fn example_path(&self, id: &str, thumb: &str) -> Option<PathBuf> {
        let s = self.get(id)?;
        s.labels.iter().any(|l| l.examples.iter().any(|e| e.thumb == thumb)).then(|| self.dir.join(id).join(thumb))
    }

    fn set_current(&self, id: &str, c: Current) {
        self.current.locked().insert(id.to_string(), c);
    }
}

// ---------- the background look ----------

/// The piece of a frame a state looks at.
pub fn piece(s: &State, f: &crate::imgutil::Frame) -> crate::imgutil::Frame {
    match s.region {
        Some([x, y, w, h]) => f.crop(x, y, w, h),
        None => f.clone(),
    }
}

/// A camera's picture right now: the one detection is looking at, or
/// (detection off) a fresh grab from the shared camera connection.
pub async fn current_frame(mgr: &crate::manager::Manager, rt: &crate::manager::CameraRuntime) -> Result<crate::imgutil::Frame> {
    if let Some(f) = rt.detector().and_then(|d| d.latest_frame()) {
        return Ok(f);
    }
    let ffmpeg = mgr.config().server.ffmpeg_path;
    let (w, h) = (640u32, 360u32);
    let rgb = crate::relay::grab_rgb(&rt.relay_sub, &ffmpeg, w, h).await?;
    Ok(crate::imgutil::Frame { width: w, height: h, rgb: Arc::new(rgb) })
}

/// Runs for as long as GAZE does; idle (and the 420 MB model untouched)
/// until a state has two labels with examples.
pub async fn run(mgr: Arc<crate::manager::Manager>) {
    tokio::time::sleep(Duration::from_secs(30)).await;
    let mut last: HashMap<String, Instant> = HashMap::new();
    let mut debounce: HashMap<String, Debounce> = HashMap::new();
    loop {
        let todo: Vec<State> = mgr.states.all().into_iter().filter(|s| s.ready()).collect();
        if todo.is_empty() {
            tokio::time::sleep(Duration::from_secs(10)).await;
            continue;
        }
        let models = mgr.data_dir.join("models");
        let clip = match crate::clip::model(&models).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("learned states: {e:#}");
                for s in &todo {
                    mgr.states.set_current(&s.id, Current { note: format!("the picture model isn't ready: {e:#}"), ..Default::default() });
                }
                tokio::time::sleep(Duration::from_secs(120)).await;
                continue;
            }
        };
        for s in todo {
            if last.get(&s.id).is_some_and(|t| t.elapsed() < Duration::from_secs(s.interval_secs as u64)) {
                continue;
            }
            last.insert(s.id.clone(), Instant::now());
            let Some(rt) = mgr.camera(&s.camera) else {
                mgr.states.set_current(&s.id, Current { note: "this camera was removed".into(), ..Default::default() });
                continue;
            };
            crate::load::wait_for_cpu(85, Duration::from_secs(60)).await;
            let frame = match current_frame(&mgr, &rt).await {
                Ok(f) => f,
                Err(e) => {
                    let prev = mgr.states.current(&s.id);
                    mgr.states.set_current(&s.id, Current { note: format!("no picture from the camera: {e:#}"), ..prev });
                    continue;
                }
            };
            let labels: Vec<(String, Vec<Vec<f32>>)> = s.labels.iter().map(|l| (l.name.clone(), l.examples.iter().map(|e| unpack(&e.v)).collect())).collect();
            let (c2, s2) = (clip.clone(), s.clone());
            let verdict = tokio::task::spawn_blocking(move || -> Result<Verdict> {
                let v = c2.region(&piece(&s2, &frame))?;
                Ok(classify(&labels, &v))
            })
            .await;
            let verdict = match verdict {
                Ok(Ok(v)) => v,
                Ok(Err(e)) => {
                    tracing::debug!("learned states: {}: {e:#}", s.name);
                    continue;
                }
                Err(_) => continue,
            };
            let prev = mgr.states.current(&s.id);
            let now = Local::now();
            let switch = debounce.entry(s.id.clone()).or_default().feed(prev.label.as_deref(), verdict.label.as_deref());
            let mut cur = Current { score: verdict.score, checked: Some(now), note: String::new(), ..prev.clone() };
            if verdict.label.is_none() {
                cur.note = "not sure which it is".into();
            }
            if let Some(new) = switch {
                cur.label = Some(new.clone());
                cur.since = Some(now);
                if let Some(old) = prev.label.clone()
                    && s.notify
                {
                    tracing::info!("learned states: {} / {}: {old} → {new}", rt.cfg.name, s.name);
                    notify_change(&mgr, &rt.cfg.name, &s, &old, &new).await;
                }
            }
            mgr.states.set_current(&s.id, cur);
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn notify_change(mgr: &crate::manager::Manager, camera: &str, s: &State, old: &str, new: &str) {
    let cfg = mgr.config().notify;
    if !cfg.enabled || !cfg.delivers() {
        return;
    }
    let msg = crate::notify::Message {
        title: format!("{camera}: {} is now {new}", s.name),
        body: format!("{} changed from {old} to {new}.", s.name),
        tags: "eyes",
        priority: 3,
        click: (!cfg.link.trim().is_empty()).then(|| cfg.link.trim().to_string()),
        picture: None,
        actions: None,
    };
    if let Err(e) = crate::notify::send(&cfg, msg).await {
        tracing::warn!("learned states: notification: {e:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit vector at an angle in a plane: similar angles = similar pictures.
    fn at(deg: f32) -> Vec<f32> {
        let r = deg.to_radians();
        vec![r.cos(), r.sin(), 0.0, 0.0]
    }

    fn labels() -> Vec<(String, Vec<Vec<f32>>)> {
        vec![("open".into(), vec![at(0.0), at(4.0), at(-3.0)]), ("closed".into(), vec![at(30.0), at(34.0)])]
    }

    #[test]
    fn the_closest_label_wins_and_unsure_is_none() {
        assert_eq!(classify(&labels(), &at(2.0)).label.as_deref(), Some("open"));
        assert_eq!(classify(&labels(), &at(31.0)).label.as_deref(), Some("closed"));
        // Right between the two: not sure.
        let v = classify(&labels(), &at(16.5));
        assert!(v.label.is_none(), "{v:?}");
        // One label alone can't tell anything apart.
        assert!(classify(&labels()[..1], &at(0.0)).label.is_none());
        // A picture resembling nothing.
        let far = vec![0.0, 0.0, 1.0, 0.0];
        assert!(classify(&labels(), &far).label.is_none());
    }

    #[test]
    fn one_odd_example_cannot_decide_alone() {
        let mut l = labels();
        // A wrong "closed" example that looks like "open".
        l[1].1.push(at(1.0));
        assert_eq!(classify(&l, &at(1.0)).label.as_deref(), Some("open"));
    }

    #[test]
    fn a_change_must_be_seen_twice_in_a_row() {
        let mut d = Debounce::default();
        assert_eq!(d.feed(Some("open"), Some("open")), None);
        assert_eq!(d.feed(Some("open"), Some("closed")), None);
        // A flicker back cancels it.
        assert_eq!(d.feed(Some("open"), Some("open")), None);
        assert_eq!(d.feed(Some("open"), Some("closed")), None);
        assert_eq!(d.feed(Some("open"), Some("closed")), Some("closed".into()));
        // Unsure looks change nothing and reset the count.
        assert_eq!(d.feed(Some("closed"), Some("open")), None);
        assert_eq!(d.feed(Some("closed"), None), None);
        assert_eq!(d.feed(Some("closed"), Some("open")), None);
        // The very first answer also needs two looks.
        let mut d = Debounce::default();
        assert_eq!(d.feed(None, Some("open")), None);
        assert_eq!(d.feed(None, Some("open")), Some("open".into()));
    }

    #[test]
    fn numbers_survive_packing_exactly() {
        let v: Vec<f32> = (0..512).map(|i| (i as f32 * 0.37).sin() / 23.0).collect();
        assert_eq!(unpack(&pack(&v)), v);
    }

    #[test]
    fn store_keeps_examples_and_forgets_them_when_the_area_moves() {
        let dir = std::env::temp_dir().join(format!("nvr-states-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let s = StateStore::open(&dir);
        let id = s.create("cam", "Garage door", None, 10, false, &["open".into(), "closed".into()]).unwrap();
        assert!(s.create("cam", " ", None, 10, false, &[]).is_err());
        assert!(s.create("cam", "x", None, 10, false, &["a".into(), "A".into()]).is_err());
        assert!(s.create("cam", "x", Some([0.9, 0.9, 0.5, 0.5]), 10, false, &[]).is_err());
        s.add_example(&id, "open", &at(0.0), b"jpg").unwrap();
        assert!(!s.get(&id).unwrap().ready());
        s.add_example(&id, "closed", &at(30.0), b"jpg").unwrap();
        assert!(s.get(&id).unwrap().ready());
        // Survives a restart.
        let again = StateStore::open(&dir);
        assert_eq!(again.get(&id).unwrap().labels.len(), 2);
        let thumb = again.get(&id).unwrap().labels[0].examples[0].thumb.clone();
        assert!(again.example_path(&id, &thumb).unwrap().exists());
        assert!(again.example_path(&id, "../../etc/passwd").is_none());
        // Moving the area drops the examples (they were of another piece).
        again.update(&id, "Garage door", Some([0.1, 0.1, 0.5, 0.5]), 10, false).unwrap();
        assert!(again.get(&id).unwrap().labels.iter().all(|l| l.examples.is_empty()));
        again.delete(&id).unwrap();
        assert!(again.all().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A synthetic garage: a wall with a door that is shut (light slatted
    /// panel) or open (dark inside), under other light and a little noise.
    fn scene(open: bool, light: f32, seed: u32) -> crate::imgutil::Frame {
        let (w, h) = (640u32, 360u32);
        let mut rgb = vec![0u8; (w * h * 3) as usize];
        let mut n = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
        for y in 0..h {
            for x in 0..w {
                let in_door = (192..448).contains(&x) && (100..340).contains(&y);
                let base: f32 = if !in_door {
                    110.0 + (x as f32 / w as f32) * 30.0
                } else if open {
                    25.0
                } else if (y / 20) % 2 == 0 {
                    200.0
                } else {
                    170.0
                };
                n = n.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = ((n >> 24) as f32 - 128.0) / 128.0 * 6.0;
                let v = ((base * light + noise).clamp(0.0, 255.0)) as u8;
                let i = ((y * w + x) * 3) as usize;
                rgb[i] = v;
                rgb[i + 1] = v;
                rgb[i + 2] = (v as f32 * 0.95) as u8;
            }
        }
        crate::imgutil::Frame { width: w, height: h, rgb: Arc::new(rgb) }
    }

    /// With the real picture model (NVR_TEST_MODELS = a folder that has, or
    /// may download, the description-search models): examples taken in
    /// ordinary light still tell open from shut in dimmer and brighter light.
    #[tokio::test]
    async fn real_model_tells_a_door_open_from_shut() {
        let Ok(dir) = std::env::var("NVR_TEST_MODELS") else { return };
        let _ = rustls::crypto::ring::default_provider().install_default();
        let clip = crate::clip::model(Path::new(&dir)).await.unwrap();
        let embed = |f: &crate::imgutil::Frame| clip.region(f).unwrap();
        let mut labels = vec![("open".to_string(), vec![]), ("closed".to_string(), vec![])];
        for (i, light) in [0.9f32, 1.0, 1.1].into_iter().enumerate() {
            labels[0].1.push(embed(&scene(true, light, i as u32)));
            labels[1].1.push(embed(&scene(false, light, 10 + i as u32)));
        }
        let (mut worst_margin, mut wrong) = (f32::MAX, 0);
        for (i, light) in [0.7f32, 0.8, 0.95, 1.05, 1.2, 1.35].into_iter().enumerate() {
            for open in [true, false] {
                let v = classify(&labels, &embed(&scene(open, light, 100 + i as u32)));
                let want = if open { "open" } else { "closed" };
                eprintln!("light {light}: {want} -> {:?} (score {:.3}, margin {:.3})", v.label, v.score, v.margin);
                if v.label.as_deref() != Some(want) {
                    wrong += 1;
                }
                worst_margin = worst_margin.min(v.margin);
            }
        }
        eprintln!("worst margin {worst_margin:.3}");
        assert_eq!(wrong, 0, "wrong or unsure answers");
    }
}
