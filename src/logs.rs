//! In-app log: every log message is kept in memory (for the Logs page) and
//! appended to `nvr.log` in the data folder (rotated at 10 MB).

use chrono::{DateTime, Local};
use serde::Serialize;
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;

const KEEP: usize = 10_000;
const MAX_FILE: u64 = 10 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub id: u64,
    pub time: DateTime<Local>,
    /// error, warn, info
    pub level: &'static str,
    /// "event" for detections/alerts, "system" otherwise.
    pub kind: &'static str,
    /// Camera name if the message is about one camera.
    pub camera: Option<String>,
    pub message: String,
}

pub struct LogBuffer {
    entries: Mutex<VecDeque<LogEntry>>,
    next_id: AtomicU64,
    file: Mutex<Option<(PathBuf, std::fs::File)>>,
}

static LOGS: OnceLock<Arc<LogBuffer>> = OnceLock::new();

pub fn buffer() -> Arc<LogBuffer> {
    LOGS.get_or_init(|| {
        Arc::new(LogBuffer { entries: Mutex::new(VecDeque::new()), next_id: AtomicU64::new(1), file: Mutex::new(None) })
    })
    .clone()
}

impl LogBuffer {
    /// Start writing to a log file as well.
    pub fn set_file(&self, path: PathBuf) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            *self.file.lock().unwrap() = Some((path, f));
        }
    }

    pub fn file_path(&self) -> Option<PathBuf> {
        self.file.lock().unwrap().as_ref().map(|(p, _)| p.clone())
    }

    fn push(&self, level: &'static str, kind: &'static str, message: String) {
        let camera = message
            .strip_prefix('[')
            .and_then(|r| r.split_once(']'))
            .map(|(name, _)| name.to_string());
        let entry = LogEntry {
            id: self.next_id.fetch_add(1, Ordering::SeqCst),
            time: Local::now(),
            level,
            kind,
            camera,
            message,
        };
        if let Some((path, file)) = self.file.lock().unwrap().as_mut() {
            let _ = writeln!(file, "{} {:5} {}", entry.time.format("%Y-%m-%d %H:%M:%S"), level.to_uppercase(), entry.message);
            if file.metadata().map(|m| m.len() > MAX_FILE).unwrap_or(false) {
                let old = path.with_extension("log.1");
                let _ = std::fs::rename(&*path, old);
                if let Ok(f) = std::fs::OpenOptions::new().create(true).append(true).open(&*path) {
                    *file = f;
                }
            }
        }
        let mut e = self.entries.lock().unwrap();
        e.push_back(entry);
        while e.len() > KEEP {
            e.pop_front();
        }
    }

    /// Newest-last entries matching the filters.
    pub fn query(&self, q: &LogQuery) -> Vec<LogEntry> {
        let text = q.q.as_deref().map(str::to_lowercase).filter(|s| !s.is_empty());
        let min = level_rank(q.level.as_deref().unwrap_or("info"));
        let e = self.entries.lock().unwrap();
        let mut out: Vec<LogEntry> = e
            .iter()
            .rev()
            .filter(|x| x.id > q.after.unwrap_or(0))
            .filter(|x| level_rank(x.level) >= min)
            .filter(|x| q.kind.as_deref().is_none_or(|k| k.is_empty() || k == x.kind))
            .filter(|x| {
                q.camera.as_deref().is_none_or(|c| c.is_empty() || x.camera.as_deref() == Some(c))
            })
            .filter(|x| text.as_ref().is_none_or(|t| x.message.to_lowercase().contains(t)))
            .take(q.limit.unwrap_or(500).min(5000))
            .cloned()
            .collect();
        out.reverse();
        out
    }

    /// Count of warnings/errors since `since`.
    pub fn problems_since(&self, since: DateTime<Local>) -> (usize, usize) {
        let e = self.entries.lock().unwrap();
        let recent = e.iter().rev().take_while(|x| x.time >= since);
        let (mut w, mut er) = (0, 0);
        for x in recent {
            match x.level {
                "warn" => w += 1,
                "error" => er += 1,
                _ => {}
            }
        }
        (w, er)
    }
}

fn level_rank(l: &str) -> u8 {
    match l {
        "error" => 3,
        "warn" => 2,
        _ => 1,
    }
}

#[derive(Debug, Default, serde::Deserialize)]
pub struct LogQuery {
    pub level: Option<String>,
    pub kind: Option<String>,
    pub camera: Option<String>,
    pub q: Option<String>,
    pub after: Option<u64>,
    pub limit: Option<usize>,
}

/// tracing layer feeding the buffer.
pub struct LogLayer;

struct MessageVisitor(String);

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.0, "{value:?}");
        } else {
            let _ = write!(self.0, " {}={value:?}", field.name());
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.0.push_str(value);
        } else {
            let _ = write!(self.0, " {}={value}", field.name());
        }
    }
}

impl<S: Subscriber> Layer<S> for LogLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        // Only our own messages (not library internals).
        if !meta.target().starts_with("nvr") && meta.target() != "event" {
            return;
        }
        let level = match *meta.level() {
            Level::ERROR => "error",
            Level::WARN => "warn",
            Level::INFO => "info",
            _ => return,
        };
        let kind = if meta.target() == "event" { "event" } else { "system" };
        let mut v = MessageVisitor(String::new());
        event.record(&mut v);
        buffer().push(level, kind, v.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters() {
        let b = LogBuffer { entries: Mutex::new(VecDeque::new()), next_id: AtomicU64::new(1), file: Mutex::new(None) };
        b.push("info", "system", "[Office] recorder: recording".into());
        b.push("warn", "system", "[Garage] ffmpeg: Connection refused".into());
        b.push("info", "event", "[Office] alert: person (88%)".into());
        assert_eq!(b.query(&LogQuery::default()).len(), 3);
        let w = b.query(&LogQuery { level: Some("warn".into()), ..Default::default() });
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].camera.as_deref(), Some("Garage"));
        let ev = b.query(&LogQuery { kind: Some("event".into()), camera: Some("Office".into()), ..Default::default() });
        assert_eq!(ev[0].message, "[Office] alert: person (88%)");
        assert_eq!(b.query(&LogQuery { q: Some("REFUSED".into()), ..Default::default() }).len(), 1);
        assert_eq!(b.query(&LogQuery { after: Some(2), ..Default::default() }).len(), 1);
    }
}
