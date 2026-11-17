//! Evidence Locker: cases that keep important footage safe.
//!
//! A **case** ("Break-in 12 March", "Parcel theft") holds clips and pictures:
//! an alert (its picture and video), a stretch of recording, or a single
//! picture from a recording. Each one is **copied into the case**
//! (`nvr-data/cases/<case>/`), so it outlives the recordings it came from.
//!
//! An item is **protected** when added. While it is, the footage it came
//! from (the alert, and the recording files covering its time) can't be
//! deleted anywhere else in GAZE — not from Recordings or Alerts, not by
//! "Delete footage", and not by the automatic clean-up (retention, storage
//! limits). Deleting it there says which case holds it; it is unprotected
//! in the case first. A protected item can't be removed from its case, and
//! a case with protected items can't be deleted.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Case {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub notes: String,
    pub created: DateTime<Local>,
    #[serde(default)]
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// An alert: its picture, and its video when there is one.
    Alert,
    /// A stretch of recording.
    Clip,
    /// One picture.
    Image,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Item {
    pub id: String,
    pub kind: Kind,
    pub title: String,
    pub camera: String,
    #[serde(default)]
    pub camera_name: String,
    /// The alert it came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    /// The time it covers.
    pub from: DateTime<Local>,
    pub to: DateTime<Local>,
    /// Files in the case's folder: the video and/or the picture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picture: Option<String>,
    #[serde(default)]
    pub notes: String,
    pub added: DateTime<Local>,
    #[serde(default)]
    pub added_by: String,
    /// Locks the footage it came from against deletion.
    pub protected: bool,
}

struct Locker {
    dir: PathBuf,
    cases: Vec<Case>,
}

static LOCKER: LazyLock<Mutex<Option<Locker>>> = LazyLock::new(|| Mutex::new(None));

/// Load the cases (at start-up). `data_dir` is GAZE's data folder.
pub fn init(data_dir: &Path) {
    let dir = data_dir.join("cases");
    let file = dir.join("cases.json");
    let cases = match std::fs::read_to_string(&file) {
        Err(_) => Vec::new(),
        Ok(t) => match serde_json::from_str::<Vec<Case>>(&t) {
            Ok(c) => c,
            Err(e) => {
                // Kept aside (never overwritten), so nothing is lost.
                let aside = dir.join(format!("cases.json.unreadable-{}", Local::now().format("%Y%m%d-%H%M%S")));
                let _ = std::fs::rename(&file, &aside);
                tracing::error!("Evidence Locker: cases.json can't be read ({e}); it was kept as {}", aside.display());
                Vec::new()
            }
        },
    };
    if !cases.is_empty() {
        let n: usize = cases.iter().map(|c| c.items.iter().filter(|i| i.protected).count()).sum();
        tracing::info!("Evidence Locker: {} case(s), {n} protected item(s)", cases.len());
    }
    *LOCKER.lock().unwrap() = Some(Locker { dir, cases });
}

fn with<R>(f: impl FnOnce(&mut Locker) -> Result<R>) -> Result<R> {
    let mut g = LOCKER.lock().unwrap();
    let l = g.as_mut().context("the Evidence Locker isn't ready")?;
    f(l)
}

/// Write the list safely (a new file, then renamed over the old one).
fn save(l: &Locker) -> Result<()> {
    std::fs::create_dir_all(&l.dir)?;
    let tmp = l.dir.join("cases.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&l.cases)?)?;
    std::fs::rename(&tmp, l.dir.join("cases.json"))?;
    Ok(())
}

pub fn list() -> Vec<Case> {
    LOCKER.lock().unwrap().as_ref().map(|l| l.cases.clone()).unwrap_or_default()
}

pub fn get(id: &str) -> Option<Case> {
    list().into_iter().find(|c| c.id == id)
}

/// The folder a case's files are kept in.
pub fn case_dir(id: &str) -> Option<PathBuf> {
    let g = LOCKER.lock().unwrap();
    let l = g.as_ref()?;
    l.cases.iter().any(|c| c.id == id).then(|| l.dir.join(id))
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}{}", Local::now().format("%Y%m%d%H%M%S%3f"))
}

pub fn create(name: &str, notes: &str) -> Result<Case> {
    let name = name.trim();
    if name.is_empty() {
        bail!("give the case a name");
    }
    with(|l| {
        let c = Case { id: new_id("case-"), name: name.to_string(), notes: notes.trim().to_string(), created: Local::now(), items: Vec::new() };
        std::fs::create_dir_all(l.dir.join(&c.id))?;
        l.cases.push(c.clone());
        save(l)?;
        Ok(c)
    })
}

pub fn rename(id: &str, name: &str, notes: &str) -> Result<Case> {
    let name = name.trim();
    if name.is_empty() {
        bail!("give the case a name");
    }
    with(|l| {
        let c = l.cases.iter_mut().find(|c| c.id == id).context("no such case")?;
        c.name = name.to_string();
        c.notes = notes.trim().to_string();
        let out = c.clone();
        save(l)?;
        Ok(out)
    })
}

/// Delete a case and its files: only once nothing in it is protected.
pub fn delete(id: &str) -> Result<()> {
    with(|l| {
        let i = l.cases.iter().position(|c| c.id == id).context("no such case")?;
        let n = l.cases[i].items.iter().filter(|x| x.protected).count();
        if n > 0 {
            bail!("“{}” has {n} protected item(s): unprotect them first", l.cases[i].name);
        }
        let dir = l.dir.join(id);
        l.cases.remove(i);
        save(l)?;
        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    })
}

/// Add an item whose files are already in the case's folder.
pub fn add(case: &str, mut item: Item) -> Result<Item> {
    with(|l| {
        let c = l.cases.iter_mut().find(|c| c.id == case).context("no such case")?;
        item.id = new_id("item-");
        item.added = Local::now();
        c.items.push(item.clone());
        save(l)?;
        Ok(item)
    })
}

/// Protect or unprotect an item; change its title or notes.
pub fn update_item(case: &str, item: &str, protected: Option<bool>, title: Option<&str>, notes: Option<&str>) -> Result<Item> {
    with(|l| {
        let c = l.cases.iter_mut().find(|c| c.id == case).context("no such case")?;
        let it = c.items.iter_mut().find(|i| i.id == item).context("no such item in this case")?;
        if let Some(p) = protected {
            it.protected = p;
        }
        if let Some(t) = title.map(str::trim).filter(|t| !t.is_empty()) {
            it.title = t.to_string();
        }
        if let Some(n) = notes {
            it.notes = n.trim().to_string();
        }
        let out = it.clone();
        save(l)?;
        Ok(out)
    })
}

/// Remove an item (and its copied files) from a case: only when unprotected.
pub fn remove_item(case: &str, item: &str) -> Result<()> {
    with(|l| {
        let dir = l.dir.join(case);
        let c = l.cases.iter_mut().find(|c| c.id == case).context("no such case")?;
        let i = c.items.iter().position(|x| x.id == item).context("no such item in this case")?;
        if c.items[i].protected {
            bail!("“{}” is protected: unprotect it first", c.items[i].title);
        }
        let it = c.items.remove(i);
        save(l)?;
        for f in [it.video, it.picture].into_iter().flatten() {
            let _ = std::fs::remove_file(dir.join(f));
        }
        Ok(())
    })
}

/// The case (by name) whose protected items lock this alert.
pub fn locks_event(camera: &str, event: &str) -> Option<String> {
    let g = LOCKER.lock().unwrap();
    let l = g.as_ref()?;
    l.cases.iter().find(|c| c.items.iter().any(|i| i.protected && i.camera == camera && i.event.as_deref() == Some(event))).map(|c| c.name.clone())
}

/// The case whose protected items lock footage of this camera between these
/// times (a recording file, an alert clip).
pub fn locks_time(camera: &str, from: DateTime<Local>, to: DateTime<Local>) -> Option<String> {
    let g = LOCKER.lock().unwrap();
    let l = g.as_ref()?;
    l.cases
        .iter()
        .find(|c| c.items.iter().any(|i| i.protected && i.camera == camera && i.from <= to && i.to >= from))
        .map(|c| c.name.clone())
}

/// What a blocked delete says.
pub fn locked_message(case: &str) -> String {
    format!("This is locked in the Evidence Locker case “{case}”. To delete it, open that case (Evidence) and unprotect it first.")
}

/// A safe file name from a title.
pub fn file_name(title: &str, ext: &str) -> String {
    let base: String = title.chars().map(|c| if c.is_alphanumeric() || " -_".contains(c) { c } else { '_' }).collect();
    let base = base.trim();
    format!("{}-{}.{ext}", if base.is_empty() { "item" } else { base }, Local::now().format("%H%M%S%3f"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    // One locker for the whole program: these steps run as one test.
    #[test]
    fn protection_blocks_deletes_until_unprotected() {
        let dir = std::env::temp_dir().join(format!("gaze-cases-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        init(&dir);
        let c = create("Break-in", "back door").unwrap();
        assert!(create("  ", "").is_err());
        let t = Local::now();
        let it = add(&c.id, Item {
            id: String::new(),
            kind: Kind::Alert,
            title: "Person at the back door".into(),
            camera: "back".into(),
            camera_name: "Back".into(),
            event: Some("e1".into()),
            from: t - Duration::seconds(10),
            to: t,
            video: None,
            picture: None,
            notes: String::new(),
            added: t,
            added_by: "admin".into(),
            protected: true,
        })
        .unwrap();
        // The alert and the footage of its time are locked; other footage isn't.
        assert_eq!(locks_event("back", "e1").as_deref(), Some("Break-in"));
        assert!(locks_event("front", "e1").is_none());
        assert!(locks_time("back", t - Duration::seconds(5), t + Duration::seconds(60)).is_some());
        assert!(locks_time("back", t + Duration::seconds(1), t + Duration::seconds(60)).is_none());
        // Protected: can't be removed, nor the case deleted.
        assert!(remove_item(&c.id, &it.id).is_err());
        assert!(delete(&c.id).is_err());
        // Kept across a restart.
        init(&dir);
        assert_eq!(get(&c.id).unwrap().items.len(), 1);
        // Unprotected: the footage is free again, and the item can go.
        update_item(&c.id, &it.id, Some(false), None, None).unwrap();
        assert!(locks_event("back", "e1").is_none());
        remove_item(&c.id, &it.id).unwrap();
        delete(&c.id).unwrap();
        assert!(list().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
