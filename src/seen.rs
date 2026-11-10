//! Which alerts have been viewed: everything up to `before`, plus single
//! alerts viewed since. Kept in `nvr-data/alerts-seen.json`.

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Default, Serialize, Deserialize)]
struct State {
    before: Option<DateTime<Local>>,
    ids: HashSet<String>,
}

pub struct Seen {
    file: PathBuf,
    st: Mutex<State>,
}

impl Seen {
    pub fn open(data_dir: &std::path::Path) -> Self {
        let file = data_dir.join("alerts-seen.json");
        let st = std::fs::read(&file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_else(|| State {
                // First start: earlier alerts count as viewed.
                before: Some(Local::now()),
                ids: HashSet::new(),
            });
        let s = Self {
            file,
            st: Mutex::new(st),
        };
        s.save();
        s
    }

    fn save(&self) {
        let st = self.st.lock().unwrap();
        if let Ok(b) = serde_json::to_vec(&*st) {
            let tmp = self.file.with_extension("json.tmp");
            if std::fs::write(&tmp, b).is_ok() {
                let _ = std::fs::rename(&tmp, &self.file);
            }
        }
    }

    pub fn is_seen(&self, id: &str, start: DateTime<Local>) -> bool {
        let st = self.st.lock().unwrap();
        st.before.is_some_and(|b| start <= b) || st.ids.contains(id)
    }

    pub fn mark(&self, ids: &[String]) {
        self.st.lock().unwrap().ids.extend(ids.iter().cloned());
        self.save();
    }

    pub fn mark_all(&self) {
        {
            let mut st = self.st.lock().unwrap();
            st.before = Some(Local::now());
            st.ids.clear();
        }
        self.save();
    }

    pub fn unmark(&self, id: &str, start: DateTime<Local>) {
        {
            let mut st = self.st.lock().unwrap();
            st.ids.remove(id);
            // Viewed because of "mark all": keep the others, move the line back.
            if st.before.is_some_and(|b| start <= b) {
                st.before = Some(start - chrono::Duration::milliseconds(1));
            }
        }
        self.save();
    }
}
