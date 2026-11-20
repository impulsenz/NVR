//! A log for each camera: what the camera itself reported (its own person /
//! vehicle detection, line crossing, doorbell press, tampering … over
//! ONVIF, see `onvif_events.rs`) and how listening to it is going. Shown on
//! the camera's own **Log** tab together with that camera's lines of the
//! system log (`logs.rs`: connection, recording, alerts), so one camera's
//! history can be looked up on its own.
//!
//! Kept in `nvr-data/camlog/<camera id>.jsonl`, the last `KEEP` entries of
//! each camera, so it survives a restart. Nothing reads it but that tab:
//! failing to write it loses a line, never anything else.

use crate::sync::MutexExt;
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

const KEEP: usize = 1000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Entry {
    pub time: DateTime<Local>,
    /// "camera" (what the camera reported) or "connection" (listening to it).
    pub kind: String,
    /// The camera's own name for it (an ONVIF topic), when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    pub message: String,
}

static DIR: OnceLock<PathBuf> = OnceLock::new();
static CAMS: Mutex<Option<HashMap<String, VecDeque<Entry>>>> = Mutex::new(None);

pub fn init(data_dir: &std::path::Path) {
    let _ = DIR.set(data_dir.join("camlog"));
}

fn file(cam: &str) -> Option<PathBuf> {
    crate::storage::valid_folder(cam).then(|| DIR.get().map(|d| d.join(format!("{cam}.jsonl"))))?
}

fn load(cam: &str) -> VecDeque<Entry> {
    let text = file(cam).and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    let mut v: VecDeque<Entry> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    while v.len() > KEEP {
        v.pop_front();
    }
    v
}

/// Add a line to a camera's log.
pub fn add(cam: &str, kind: &str, topic: Option<&str>, message: &str) {
    let e = Entry { time: Local::now(), kind: kind.into(), topic: topic.map(String::from), message: message.into() };
    let mut g = CAMS.locked();
    let list = g.get_or_insert_with(HashMap::new).entry(cam.to_string()).or_insert_with(|| load(cam));
    if let Some(p) = file(cam) {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p)
            && let Ok(line) = serde_json::to_string(&e)
        {
            let _ = writeln!(f, "{line}");
        }
    }
    list.push_back(e);
    // Over the limit: the oldest go (the file is rewritten now and then).
    if list.len() > KEEP + KEEP / 4 {
        while list.len() > KEEP {
            list.pop_front();
        }
        if let Some(p) = file(cam) {
            let text: String = list.iter().filter_map(|e| serde_json::to_string(e).ok()).map(|l| l + "\n").collect();
            let tmp = p.with_extension("jsonl.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, &p);
            }
        }
    }
}

/// A camera's entries, newest last: `kind` ("camera", "connection" or all), a text to find, at most `limit`.
pub fn list(cam: &str, kind: Option<&str>, q: Option<&str>, limit: usize) -> Vec<Entry> {
    let mut g = CAMS.locked();
    let list = g.get_or_insert_with(HashMap::new).entry(cam.to_string()).or_insert_with(|| load(cam));
    let q = q.map(str::to_lowercase).filter(|s| !s.is_empty());
    let mut out: Vec<Entry> = list
        .iter()
        .rev()
        .filter(|e| kind.is_none_or(|k| k.is_empty() || e.kind == k))
        .filter(|e| q.as_ref().is_none_or(|q| e.message.to_lowercase().contains(q) || e.topic.as_deref().is_some_and(|t| t.to_lowercase().contains(q))))
        .take(limit.min(KEEP))
        .cloned()
        .collect();
    out.reverse();
    out
}

/// The camera was deleted: its log goes too.
pub fn forget(cam: &str) {
    if let Some(m) = CAMS.locked().as_mut() {
        m.remove(cam);
    }
    if let Some(p) = file(cam) {
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cameras_entries_are_kept_filtered_and_survive_a_restart() {
        let dir = std::env::temp_dir().join(format!("nvr-camlog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        init(&dir);
        add("front", "connection", None, "Listening to the camera");
        add("front", "camera", Some("RuleEngine/MyRuleDetector/PeopleDetect"), "The camera detected a person");
        add("front", "camera", Some("RuleEngine/LineDetector/Crossed"), "The camera saw a line being crossed");
        add("back", "camera", None, "Doorbell pressed");
        assert_eq!(list("front", None, None, 100).len(), 3);
        assert_eq!(list("front", Some("camera"), None, 100).len(), 2);
        assert_eq!(list("front", None, Some("LINE"), 100).len(), 1, "found in the message or the topic, whatever the case");
        assert_eq!(list("back", None, None, 100).len(), 1, "one camera's log has only its own lines");
        assert_eq!(list("front", None, None, 2).len(), 2);
        assert_eq!(list("front", None, None, 2)[1].message, "The camera saw a line being crossed", "the newest last");
        // Read back from the file as after a restart.
        *CAMS.locked() = None;
        assert_eq!(list("front", None, None, 100).len(), 3);
        // A camera id that isn't a folder name is refused.
        add("../evil", "camera", None, "x");
        assert!(!dir.join("camlog").join("..").join("evil.jsonl").exists());
        forget("front");
        assert!(list("front", None, None, 100).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
