//! Motion/AI events for one camera, stored as JSON lines in
//! `<camera folder>/events/events.jsonl` next to their thumbnails and clips.

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Label {
    pub label: String,
    pub score: f32,
    /// Main colour of the object ("red", "white"...), for search.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub id: String,
    pub camera: String,
    pub start: DateTime<Local>,
    pub end: DateTime<Local>,
    /// Recognized objects (best score per label).
    #[serde(default)]
    pub labels: Vec<Label>,
    /// Motion was detected.
    #[serde(default)]
    pub motion: bool,
    /// Thumbnail file name in the events folder.
    #[serde(default)]
    pub thumb: Option<String>,
    /// Full-quality clip file name in the events folder (event-recording modes).
    #[serde(default)]
    pub clip: Option<String>,
    /// Wall-clock time of the clip's first frame.
    #[serde(default)]
    pub clip_start: Option<DateTime<Local>>,
    /// Area of the thumbnail to zoom into (x, y, w, h as 0..1 of the picture):
    /// the recognized object. The thumbnail itself is the whole picture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<[f32; 4]>,
    /// What was detected in the thumbnail, with boxes (0..1 of the
    /// picture), so the alert can be reviewed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub boxes: Vec<SeenBox>,
    /// Zone happenings ("Person in Porch", "Car crossed Gate (A→B)").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// When the alert picture was taken (to find the same moment in HD).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumb_at: Option<DateTime<Local>>,
    /// Faces found in the alert (in full quality).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub faces: Vec<FaceSeen>,
    /// Number plates read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plates: Vec<String>,
    /// Faces and plates have been looked for.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub identified: bool,
    /// Areas where something was left or taken (checked for packages).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<ItemChange>,
    /// Phone notifications sent: 1 = picture, 2 = picture and summary.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub notified: u8,
    /// Why this looks suspicious (empty = not suspicious).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub suspicious: String,
}

fn is_zero(n: &u8) -> bool {
    *n == 0
}

/// Something left or taken in a watched area during an alert.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ItemChange {
    pub zone: String,
    /// Pictures of the area before and after (in the events folder).
    pub before: String,
    pub after: String,
    /// Looked at by the smart search model.
    #[serde(default)]
    pub checked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FaceSeen {
    /// Picture of the face (in the events folder).
    pub file: String,
    /// Faceprint, packed.
    pub print: String,
    /// Who it is, if known.
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SeenBox {
    pub label: String,
    pub score: f32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Event {
    #[cfg(test)]
    pub fn has_label(&self, l: &str) -> bool {
        self.labels.iter().any(|x| x.label == l)
    }
}

pub struct EventStore {
    dir: PathBuf,
    events: RwLock<Vec<Event>>,
}

impl EventStore {
    pub fn open(camera_dir: &Path) -> Self {
        let dir = camera_dir.join("events");
        let mut events: Vec<Event> = std::fs::read_to_string(dir.join("events.jsonl"))
            .map(|t| {
                t.lines()
                    .filter_map(|l| serde_json::from_str(l).ok())
                    .collect()
            })
            .unwrap_or_default();
        // Later lines replace earlier ones with the same id (updates).
        let mut seen = std::collections::HashMap::new();
        for (i, e) in events.iter().enumerate() {
            seen.insert(e.id.clone(), i);
        }
        let mut idx: Vec<usize> = seen.into_values().collect();
        idx.sort();
        events = idx.into_iter().map(|i| events[i].clone()).collect();
        events.sort_by_key(|e| e.start);
        // Alerts whose picture and clip are both gone (deleted, while the
        // list of alerts couldn't be rewritten): left out, and the list
        // tidied.
        let before = events.len();
        events.retain(|e| {
            let has = |f: &Option<String>| f.as_ref().is_some_and(|f| dir.join(f).is_file());
            e.thumb.is_none() || has(&e.thumb) || has(&e.clip)
        });
        let store = Self {
            dir,
            events: RwLock::new(events),
        };
        if store.events.read().unwrap().len() != before {
            store.rewrite(&store.events.read().unwrap());
        }
        store
    }

    /// Write the whole list of alerts again (after removing some). Written
    /// to a new file that replaces the old one; if that can't be done (on
    /// Windows the old file may be held open for a moment by another
    /// program), written over the old file directly, so a deletion is never
    /// undone at the next start.
    fn rewrite(&self, ev: &[Event]) {
        let text: String = ev
            .iter()
            .filter_map(|e| serde_json::to_string(e).ok())
            .map(|l| l + "\n")
            .collect();
        let path = self.dir.join("events.jsonl");
        let tmp = self.dir.join("events.jsonl.tmp");
        let swapped = std::fs::write(&tmp, &text).is_ok()
            && std::fs::File::open(&tmp).and_then(|f| f.sync_all()).is_ok()
            && std::fs::rename(&tmp, &path).is_ok();
        if !swapped {
            let _ = std::fs::remove_file(&tmp);
            for _ in 0..5 {
                if std::fs::write(&path, &text).is_ok() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            tracing::warn!("couldn't save the list of alerts in {}", self.dir.display());
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn append_line(&self, e: &Event) {
        let _ = std::fs::create_dir_all(&self.dir);
        let path = self.dir.join("events.jsonl");
        // If the last write was cut off by a crash or power cut, start on a
        // new line so this event is not glued to the broken one (broken
        // lines are skipped when loading).
        let torn = std::fs::File::open(&path)
            .and_then(|mut f| {
                use std::io::{Read, Seek, SeekFrom};
                if f.seek(SeekFrom::End(0))? == 0 {
                    return Ok(false);
                }
                f.seek(SeekFrom::End(-1))?;
                let mut b = [0u8; 1];
                f.read_exact(&mut b)?;
                Ok(b[0] != b'\n')
            })
            .unwrap_or(false);
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            && let Ok(line) = serde_json::to_string(e)
        {
            let nl = if torn { "\n" } else { "" };
            let _ = writeln!(f, "{nl}{line}");
            let _ = f.sync_data();
        }
    }

    /// Add or update an event.
    /// Save from the detector, which keeps its own copy of the alert in
    /// progress: what background jobs added meanwhile (notifications sent,
    /// descriptions, faces) is kept.
    pub fn save_detected(&self, mut e: Event) {
        if let Some(old) = self.get(&e.id) {
            e.notified = e.notified.max(old.notified);
            if e.suspicious.is_empty() {
                e.suspicious = old.suspicious;
            }
            if e.faces.is_empty() {
                e.faces = old.faces;
            }
            if e.plates.is_empty() {
                e.plates = old.plates;
            }
            e.identified |= old.identified;
            for n in old.notes {
                if !e.notes.contains(&n) {
                    e.notes.push(n);
                }
            }
            // A picture already made from the saved recording stays, unless
            // detection has since chosen a better moment (that one is then
            // made from the recording again).
            if old.thumb.as_deref().is_some_and(|t| t.ends_with("-hd.jpg"))
                && e.thumb != old.thumb
                && e.thumb_at == old.thumb_at
            {
                e.thumb = old.thumb;
            }
        }
        self.insert_or_update(e);
    }

    /// Add a new event, or update it.
    fn insert_or_update(&self, e: Event) {
        let mut ev = self.events.write().unwrap();
        self.append_line(&e);
        if let Some(slot) = ev.iter_mut().find(|x| x.id == e.id) {
            *slot = e;
        } else {
            ev.push(e);
            ev.sort_by_key(|e| e.start);
        }
    }

    /// Update an event that's still there. Background jobs (faces, clips,
    /// notifications, pictures) work on a copy for a while: if the event
    /// was deleted meanwhile (one alert, or "Delete footage"), their save
    /// mustn't bring it back. New events come from the detector
    /// (`save_detected`).
    pub fn save(&self, e: Event) {
        let mut ev = self.events.write().unwrap();
        if let Some(slot) = ev.iter_mut().find(|x| x.id == e.id) {
            self.append_line(&e);
            *slot = e;
        }
    }

    pub fn get(&self, id: &str) -> Option<Event> {
        self.events
            .read()
            .unwrap()
            .iter()
            .find(|e| e.id == id)
            .cloned()
    }

    pub fn between(&self, from: DateTime<Local>, to: DateTime<Local>) -> Vec<Event> {
        self.events
            .read()
            .unwrap()
            .iter()
            .filter(|e| e.end >= from && e.start <= to)
            .cloned()
            .collect()
    }

    pub fn all(&self) -> Vec<Event> {
        self.events.read().unwrap().clone()
    }

    /// Delete events (and their files) that match `remove`, then compact the file.
    pub fn remove_where(&self, remove: impl Fn(&Event) -> bool) -> usize {
        let mut ev = self.events.write().unwrap();
        let before = ev.len();
        ev.retain(|e| {
            if remove(e) {
                for f in [&e.thumb, &e.clip].into_iter().flatten() {
                    let _ = std::fs::remove_file(self.dir.join(f));
                }
                false
            } else {
                true
            }
        });
        let removed = before - ev.len();
        if removed > 0 {
            self.rewrite(&ev);
        }
        removed
    }

    /// Delete a single event and its files.
    pub fn delete(&self, id: &str) -> bool {
        self.remove_where(|e| e.id == id) > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_update_reload_remove() {
        let dir = std::env::temp_dir().join(format!("nvr-ev-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let s = EventStore::open(&dir);
        let now = Local::now();
        let mut e = Event {
            id: "e1".into(),
            camera: "cam".into(),
            start: now,
            end: now,
            labels: vec![],
            motion: true,
            thumb: None,
            clip: None,
            clip_start: None,
            focus: None,
            boxes: Vec::new(),
            notes: Vec::new(),
            thumb_at: None,
            faces: Vec::new(),
            plates: Vec::new(),
            identified: false,
            items: Vec::new(),
            notified: 0,
            suspicious: String::new(),
        };
        s.save_detected(e.clone());
        e.labels.push(Label {
            label: "person".into(),
            score: 0.9,
            color: None,
        });
        s.save(e.clone());
        let s2 = EventStore::open(&dir);
        assert_eq!(s2.all().len(), 1);
        assert!(s2.get("e1").unwrap().has_label("person"));
        assert_eq!(s2.remove_where(|e| e.has_label("person")), 1);
        // An alert whose picture was deleted is dropped when loading; one
        // with its picture is kept.
        let mut a = e.clone();
        a.id = "kept".into();
        a.thumb = Some("kept.jpg".into());
        let mut b = e.clone();
        b.id = "gone".into();
        b.thumb = Some("gone.jpg".into());
        std::fs::write(s2.dir().join("kept.jpg"), b"jpg").unwrap();
        s2.save_detected(a);
        s2.save_detected(b);
        let s3 = EventStore::open(&dir);
        assert!(s3.get("kept").is_some() && s3.get("gone").is_none());
        assert_eq!(s3.remove_where(|x| x.id == "kept"), 1);
        // A background job saving its copy afterwards doesn't bring it back.
        s2.save(e.clone());
        assert!(s2.get("e1").is_none());
        assert_eq!(EventStore::open(&dir).all().len(), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
