//! What GAZE is doing in the background (System → Tasks): AI work on
//! alerts, model downloads, previews, cloud copies, network scans.
//!
//! A job is started with [`start`] and reports progress on the returned
//! handle; dropping the handle marks it finished. Finished jobs stay in the
//! list for a while.

use crate::sync::MutexExt;
use chrono::{DateTime, Local};
use serde::Serialize;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Serialize)]
pub struct JobInfo {
    pub id: u64,
    /// "ai", "download", "previews", "cloud", "archive", "scan", "maintenance".
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
    /// Transfer speed in bytes per second (file copies), 0 = not a transfer.
    pub rate: u64,
    /// Bytes copied and bytes to copy in all (file copies).
    pub xfer: (u64, u64),
    /// What was just made, to show it going by (time-lapse previews: the
    /// page of pictures and how they're laid out).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<serde_json::Value>,
    /// Files done so far in this run, newest last (what, which times).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub log: Vec<String>,
}

static JOBS: Mutex<Vec<JobInfo>> = Mutex::new(Vec::new());
/// Kinds of work paused by the user until resumed.
static PAUSED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
/// Kinds of work that can be paused: background work that can wait.
pub const PAUSABLE: &[&str] = &["ai", "previews", "cloud", "archive", "maintenance"];

/// Pause or resume a kind of work. Returns false for a kind that can't be paused.
pub fn set_paused(kind: &str, pause: bool) -> bool {
    let Some(k) = PAUSABLE.iter().find(|k| **k == kind) else {
        return false;
    };
    let mut p = PAUSED.locked();
    p.retain(|x| x != k);
    if pause {
        p.push(k);
    }
    true
}

pub fn paused() -> Vec<&'static str> {
    PAUSED.locked().clone()
}

pub fn is_paused(kind: &str) -> bool {
    PAUSED.locked().contains(&kind)
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
    let title = title.into();
    let mut jobs = JOBS.locked();
    // The same work done again (time-lapse previews every few seconds):
    // the new run takes the old finished one's place in the list.
    jobs.retain(|j| !(j.finished.is_some() && j.kind == kind && j.title == title));
    jobs.push(JobInfo {
        id,
        kind,
        title,
        detail: String::new(),
        done: 0,
        total: 0,
        bytes: false,
        started: Local::now(),
        finished: None,
        state: "running",
        pausable: PAUSABLE.contains(&kind),
        error: String::new(),
        rate: 0,
        xfer: (0, 0),
        preview: None,
        log: Vec::new(),
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
        if let Some(j) = JOBS.locked().iter_mut().find(|j| j.id == self.id) {
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
    /// A file copy's progress: bytes done of all, and the speed.
    pub fn transfer(&self, done: u64, total: u64, rate: u64) {
        self.with(|j| {
            j.xfer = (done, total);
            j.rate = rate;
        });
    }
    /// Show what was just made (see JobInfo::preview).
    pub fn preview(&self, p: serde_json::Value) {
        self.with(|j| j.preview = Some(p));
    }
    /// One more thing finished (kept: the last 8).
    pub fn logged(&self, what: impl Into<String>) {
        let w = what.into();
        self.with(|j| {
            j.log.push(w);
            if j.log.len() > 8 {
                j.log.remove(0);
            }
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
            if let Some(j) = JOBS.locked().iter_mut().find(|j| j.id == id) {
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
    let mut v = JOBS.locked().clone();
    v.sort_by(|a, b| a.finished.is_some().cmp(&b.finished.is_some()).then_with(|| b.finished.unwrap_or(b.started).cmp(&a.finished.unwrap_or(a.started))));
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
    reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(30)).read_timeout(std::time::Duration::from_secs(60)).build()
}

/// SHA-256 of a file, read a piece at a time.
pub fn sha256_raw(path: &std::path::Path) -> anyhow::Result<[u8; 32]> {
    use sha2::Digest;
    use std::io::Read;
    let mut f = std::io::BufReader::with_capacity(1 << 20, std::fs::File::open(path)?);
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().into())
}

/// SHA-256 of a file, as lower-case hex.
pub fn sha256_file(path: &std::path::Path) -> anyhow::Result<String> {
    Ok(sha256_raw(path)?.iter().map(|b| format!("{b:02x}")).collect())
}

/// SHA-256 of bytes already in memory, as lower-case hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Check a downloaded file against the checksum pinned in the program. A file
/// that doesn't match is deleted (so it is never unpacked, loaded or run) and
/// the error says so. Programs and libraries GAZE downloads and then runs must
/// go through this (see docs/SECURITY-CHECKLIST.md).
pub fn verify_sha256(path: &std::path::Path, what: &str, want: &str) -> anyhow::Result<()> {
    let got = sha256_file(path)?;
    if got != want {
        let _ = std::fs::remove_file(path);
        anyhow::bail!("the downloaded {what} is not the expected file (checksum {got}); it was deleted and will not be used");
    }
    Ok(())
}

/// Download a file, showing it as a task with progress.
pub async fn download(what: &str, url: &str, path: &std::path::Path, min_bytes: u64) -> anyhow::Result<()> {
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
    use super::*;

    #[test]
    fn a_pinned_download_is_checked_and_a_wrong_one_deleted() {
        let dir = std::env::temp_dir().join(format!("nvr-sha-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("x.bin");
        std::fs::write(&f, b"abc").unwrap();
        // SHA-256("abc"), the standard test vector.
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(sha256_file(&f).unwrap(), abc);
        assert_eq!(sha256_hex(b"abc"), abc);
        verify_sha256(&f, "test file", abc).unwrap();
        assert!(f.exists());
        let e = verify_sha256(&f, "test file", &"0".repeat(64)).unwrap_err().to_string();
        assert!(e.contains("not the expected file") && e.contains(abc), "{e}");
        assert!(!f.exists(), "a file that fails its checksum is deleted");
        let _ = std::fs::remove_dir_all(&dir);
    }

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
        let st = || super::list().into_iter().find(|x| x.id == id).unwrap().state;
        assert_eq!(st(), "paused");
        assert!(!t.is_finished());
        super::set_paused("cloud", false);
        let j = t.await.unwrap();
        assert_eq!(st(), "running");
        drop(j);
    }
}
