//! What GAZE is doing in the background (System → Tasks): AI work on
//! alerts, model downloads, previews, cloud copies, network scans.
//!
//! A job is started with [`start`] and reports progress on the returned
//! handle; dropping the handle marks it finished. Finished jobs stay in the
//! list for a while.

use chrono::{DateTime, Local};
use serde::Serialize;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Serialize)]
pub struct JobInfo {
    pub id: u64,
    /// "ai", "download", "previews", "cloud", "scan", "maintenance".
    pub kind: &'static str,
    pub title: String,
    /// What it's on right now ("Front Door, 14:02").
    pub detail: String,
    pub done: u64,
    pub total: u64,
    /// Progress is in bytes (downloads) rather than items.
    pub bytes: bool,
    pub started: DateTime<Local>,
    pub finished: Option<DateTime<Local>>,
    /// "running", "waiting" (for spare CPU), "paused", "done", "failed".
    pub state: &'static str,
    /// Can be paused (Settings → Tasks).
    pub pausable: bool,
    pub error: String,
}

static JOBS: Mutex<Vec<JobInfo>> = Mutex::new(Vec::new());
/// Kinds of work paused by the user until resumed.
static PAUSED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
/// Kinds of work that can be paused: background work that can wait.
pub const PAUSABLE: &[&str] = &["ai", "previews", "cloud", "maintenance"];

/// Pause or resume a kind of work. Returns false for a kind that can't be paused.
pub fn set_paused(kind: &str, pause: bool) -> bool {
    let Some(k) = PAUSABLE.iter().find(|k| **k == kind) else {
        return false;
    };
    let mut p = PAUSED.lock().unwrap();
    p.retain(|x| x != k);
    if pause {
        p.push(k);
    }
    true
}

pub fn paused() -> Vec<&'static str> {
    PAUSED.lock().unwrap().clone()
}

pub fn is_paused(kind: &str) -> bool {
    PAUSED.lock().unwrap().contains(&kind)
}
static NEXT: AtomicU64 = AtomicU64::new(1);
/// How many finished jobs to keep.
const KEEP: usize = 60;

pub struct Job {
    id: u64,
    kind: &'static str,
}

pub fn start(kind: &'static str, title: impl Into<String>) -> Job {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let mut jobs = JOBS.lock().unwrap();
    jobs.push(JobInfo {
        id,
        kind,
        title: title.into(),
        detail: String::new(),
        done: 0,
        total: 0,
        bytes: false,
        started: Local::now(),
        finished: None,
        state: "running",
        pausable: PAUSABLE.contains(&kind),
        error: String::new(),
    });
    // Forget the oldest finished ones.
    let finished = jobs.iter().filter(|j| j.finished.is_some()).count();
    if finished > KEEP {
        let mut drop = finished - KEEP;
        jobs.retain(|j| {
            if drop > 0 && j.finished.is_some() {
                drop -= 1;
                false
            } else {
                true
            }
        });
    }
    Job { id, kind }
}

impl Job {
    fn with(&self, f: impl FnOnce(&mut JobInfo)) {
        if let Some(j) = JOBS.lock().unwrap().iter_mut().find(|j| j.id == self.id) {
            f(j);
        }
    }
    pub fn total(&self, total: u64) -> &Self {
        self.with(|j| j.total = total);
        self
    }
    pub fn bytes(&self, done: u64, total: u64) {
        self.with(|j| {
            j.bytes = true;
            j.done = done;
            j.total = total;
        });
    }
    pub fn step(&self, detail: impl Into<String>) {
        let d = detail.into();
        self.with(|j| {
            j.detail = d;
            j.state = "running";
        });
    }
    /// Something that sets how many are done, usable from another thread.
    pub fn setter(&self) -> impl FnMut(u64) + Send + 'static {
        let id = self.id;
        move |n| {
            if let Some(j) = JOBS.lock().unwrap().iter_mut().find(|j| j.id == id) {
                j.done = n;
            }
        }
    }
        pub fn done_one(&self) {
        self.with(|j| j.done += 1);
    }
    /// Waiting for the computer to have spare capacity.
    pub fn waiting(&self) {
        self.with(|j| j.state = "waiting");
    }
    /// Wait here while the user has paused this kind of work.
    pub async fn pause_point(&self) {
        if !is_paused(self.kind) {
            return;
        }
        self.with(|j| j.state = "paused");
        while is_paused(self.kind) {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        self.with(|j| j.state = "running");
    }
    /// Something went wrong with one item; the job carries on.
    pub fn problem(&self, e: impl std::fmt::Display) {
        let e = e.to_string();
        self.with(|j| j.error = e);
    }
    pub fn fail(&self, e: impl std::fmt::Display) {
        let e = e.to_string();
        self.with(|j| {
            j.state = "failed";
            j.error = e;
            j.finished = Some(Local::now());
        });
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.with(|j| {
            if j.finished.is_none() {
                j.finished = Some(Local::now());
                j.state = "done";
            }
        });
    }
}

/// Running jobs first, then the most recently finished.
pub fn list() -> Vec<JobInfo> {
    let mut v = JOBS.lock().unwrap().clone();
    v.sort_by(|a, b| {
        a.finished
            .is_some()
            .cmp(&b.finished.is_some())
            .then_with(|| {
                b.finished
                    .unwrap_or(b.started)
                    .cmp(&a.finished.unwrap_or(a.started))
            })
    });
    v
}

/// A download must be as long as the server said; a connection that
/// dropped part-way must never leave a cut-off model file behind (loading
/// one can crash the program).
pub fn check_complete(tmp: &std::path::Path, got: u64, total: u64) -> anyhow::Result<()> {
    if total > 0 && got != total {
        let _ = std::fs::remove_file(tmp);
        anyhow::bail!("the download was cut off ({got} of {total} bytes); it will be tried again");
    }
    Ok(())
}

/// For downloads: give up when nothing arrives for a minute, however long
/// the whole file takes on a slow connection.
pub fn download_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .read_timeout(std::time::Duration::from_secs(60))
        .build()
}

/// Download a file, showing it as a task with progress.
pub async fn download(
    what: &str,
    url: &str,
    path: &std::path::Path,
    min_bytes: u64,
) -> anyhow::Result<()> {
    let job = start("download", format!("Downloading {what}"));
    job.step(url.rsplit('/').next().unwrap_or(url).to_string());
    let res = async {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let client = download_client()?;
        let mut resp = client.get(url).send().await?.error_for_status()?;
        let total = resp.content_length().unwrap_or(0);
        let tmp = path.with_extension("part");
        let mut file = tokio::fs::File::create(&tmp).await?;
        let mut got = 0u64;
        while let Some(chunk) = resp.chunk().await? {
            tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
            got += chunk.len() as u64;
            job.bytes(got, total);
        }
        tokio::io::AsyncWriteExt::flush(&mut file).await?;
        drop(file);
        check_complete(&tmp, got, total)?;
        if got < min_bytes {
            let _ = std::fs::remove_file(&tmp);
            anyhow::bail!("the download looks wrong ({got} bytes)");
        }
        tokio::fs::rename(&tmp, path).await?;
        anyhow::Ok(())
    }
    .await;
    if let Err(e) = &res {
        job.fail(format!("{e:#}"));
    }
    res
}

#[cfg(test)]
mod tests {
    #[test]
    fn jobs_finish_when_dropped() {
        let j = super::start("ai", "Testing");
        j.total(3).done_one();
        let id = j.id;
        let find = || super::list().into_iter().find(|x| x.id == id).unwrap();
        assert_eq!((find().done, find().total, find().state), (1, 3, "running"));
        drop(j);
        assert_eq!(find().state, "done");
        let k = super::start("ai", "Broken");
        k.fail("no model");
        let kid = k.id;
        drop(k);
        let f = super::list().into_iter().find(|x| x.id == kid).unwrap();
        assert_eq!((f.state, f.error.as_str()), ("failed", "no model"));
    }

    #[tokio::test]
    async fn paused_work_waits_until_resumed() {
        assert!(!super::set_paused("download", true));
        let j = super::start("cloud", "Testing pause");
        assert!(super::set_paused("cloud", true));
        let id = j.id;
        let t = tokio::spawn(async move {
            j.pause_point().await;
            j
        });
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        let st = || {
            super::list()
                .into_iter()
                .find(|x| x.id == id)
                .unwrap()
                .state
        };
        assert_eq!(st(), "paused");
        assert!(!t.is_finished());
        super::set_paused("cloud", false);
        let j = t.await.unwrap();
        assert_eq!(st(), "running");
        drop(j);
    }
}
