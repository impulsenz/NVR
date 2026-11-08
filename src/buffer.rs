//! Pre-event buffer: full-quality video is recorded continuously into short
//! files in `<camera>/.buffer`. Files are deleted once they are older than the
//! pre-event time, unless an event needs them; then they are joined into one
//! clip (`<camera>/events/<event id>.mp4`) covering the event plus the
//! pre-/post-event time.

use crate::events::EventStore;
use chrono::{DateTime, Duration as CDuration, Local, NaiveDateTime, TimeZone};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Length of each buffer file.
pub const BUFFER_SEGMENT_SECONDS: u64 = 10;
pub const PREFIX: &str = "buf";

struct Job {
    event_id: String,
    from: DateTime<Local>,
    to: DateTime<Local>,
}

pub struct EventBuffer {
    pub dir: PathBuf,
    name: String,
    ffmpeg: String,
    pre: i64,
    store: Arc<EventStore>,
    /// Events in progress: their pre-event start must be kept.
    holds: Mutex<HashMap<String, DateTime<Local>>>,
    jobs: Mutex<Vec<Job>>,
}

#[derive(Debug, Clone)]
struct Seg {
    path: PathBuf,
    start: DateTime<Local>,
    end: DateTime<Local>,
}

impl EventBuffer {
    pub fn new(camera_dir: &Path, name: &str, ffmpeg: &str, pre_seconds: u64, store: Arc<EventStore>) -> Arc<Self> {
        Arc::new(Self {
            dir: camera_dir.join(".buffer"),
            name: name.to_string(),
            ffmpeg: ffmpeg.to_string(),
            pre: pre_seconds as i64,
            store,
            holds: Mutex::new(HashMap::new()),
            jobs: Mutex::new(Vec::new()),
        })
    }

    /// An event started: keep footage from `start - pre` on.
    pub fn hold(&self, id: &str, start: DateTime<Local>) {
        self.holds.lock().unwrap().insert(id.to_string(), start);
    }

    /// An event was discarded (e.g. motion without a recognized object).
    pub fn release(&self, id: &str) {
        self.holds.lock().unwrap().remove(id);
    }

    /// An event finished: build a clip from `start - pre` to `end + post`.
    pub fn finish(&self, id: &str, from: DateTime<Local>, to: DateTime<Local>) {
        self.holds.lock().unwrap().remove(id);
        self.jobs.lock().unwrap().push(Job { event_id: id.to_string(), from, to });
    }

    fn segments(&self) -> Vec<Seg> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return vec![] };
        let mut v: Vec<(PathBuf, DateTime<Local>)> = rd
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                let name = p.file_name()?.to_str()?.to_string();
                let ts = name.strip_prefix(&format!("{PREFIX}_"))?.strip_suffix(".mp4")?;
                let n = NaiveDateTime::parse_from_str(ts, "%Y-%m-%d_%H-%M-%S").ok()?;
                Some((p, Local.from_local_datetime(&n).earliest()?))
            })
            .collect();
        v.sort_by_key(|x| x.1);
        // The newest file is still being written; its end is "now".
        let n = v.len();
        (0..n)
            .map(|i| Seg {
                path: v[i].0.clone(),
                start: v[i].1,
                end: if i + 1 < n { v[i + 1].1 } else { Local::now() },
            })
            .collect()
    }

    pub async fn run(self: Arc<Self>, cancel: CancellationToken) {
        let mut tick = tokio::time::interval(Duration::from_secs(3));
        loop {
            tokio::select! {
                _ = tick.tick() => self.tick().await,
                _ = cancel.cancelled() => {
                    // Finish clips for events that already ended.
                    self.tick().await;
                    return;
                }
            }
        }
    }

    async fn tick(&self) {
        let segs = self.segments();
        if segs.is_empty() {
            return;
        }
        let writing_start = segs.last().unwrap().start;

        // Build clips whose time range is fully written.
        let ready: Vec<Job> = {
            let mut jobs = self.jobs.lock().unwrap();
            let (ready, wait): (Vec<Job>, Vec<Job>) = jobs.drain(..).partition(|j| j.to <= writing_start);
            *jobs = wait;
            ready
        };
        for job in ready {
            let parts: Vec<Seg> = segs[..segs.len() - 1]
                .iter()
                .filter(|s| s.end > job.from && s.start < job.to)
                .cloned()
                .collect();
            if parts.is_empty() {
                warn!("[{}] no buffered video for event {}", self.name, job.event_id);
                continue;
            }
            match self.build_clip(&job.event_id, &parts).await {
                Ok(name) => {
                    if let Some(mut e) = self.store.get(&job.event_id) {
                        e.clip = Some(name);
                        e.clip_start = Some(parts[0].start);
                        self.store.save(e);
                    }
                }
                Err(e) => warn!("[{}] could not save event clip: {e:#}", self.name),
            }
        }

        // Delete buffer files nobody needs any more.
        let now = Local::now();
        let keep_from = {
            let holds = self.holds.lock().unwrap();
            let jobs = self.jobs.lock().unwrap();
            let mut t = now - CDuration::seconds(self.pre + 5);
            for h in holds.values() {
                t = t.min(*h - CDuration::seconds(self.pre + 5));
            }
            for j in jobs.iter() {
                t = t.min(j.from);
            }
            t
        };
        for s in &segs[..segs.len() - 1] {
            if s.end < keep_from {
                let _ = std::fs::remove_file(&s.path);
            }
        }
    }

    async fn build_clip(&self, event_id: &str, parts: &[Seg]) -> anyhow::Result<String> {
        let out_dir = self.store.dir().to_path_buf();
        std::fs::create_dir_all(&out_dir)?;
        let name = format!("{event_id}.mp4");
        let list = self.dir.join(format!("{event_id}.txt"));
        let text: String = parts
            .iter()
            .map(|p| format!("file '{}'\n", p.path.display().to_string().replace('\\', "/").replace('\'', "'\\''")))
            .collect();
        std::fs::write(&list, text)?;
        let out = tokio::time::timeout(
            Duration::from_secs(300),
            tokio::process::Command::new(&self.ffmpeg)
                .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "concat", "-safe", "0", "-i"])
                .arg(&list)
                .args(["-c", "copy", "-movflags", "+faststart"])
                .arg(out_dir.join(&name))
                .stdin(std::process::Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await??;
        let _ = std::fs::remove_file(&list);
        if !out.status.success() {
            anyhow::bail!("{}", String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("ffmpeg failed"));
        }
        let secs = (parts.last().unwrap().end - parts[0].start).num_seconds();
        info!(target: "event", "[{}] saved {}s full-quality clip for event {}", self.name, secs, event_id);
        Ok(name)
    }
}
