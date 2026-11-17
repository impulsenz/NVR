//! Optional backup of recordings, three ways:
//! - **a folder or network share**: a folder synced by a cloud app (Google
//!   Drive, OneDrive or Dropbox for desktop), another drive, or a NAS's
//!   shared folder (`\\NAS\cameras`, with its login if it needs one);
//! - **a NAS over FTP** (`ftp://nas.local/Cameras`), built in;
//! - **rclone** (Google Drive, OneDrive, Dropbox, S3, Backblaze B2, … — anything
//!   rclone supports), no sync app needed.
//!
//! Finished files are copied to `<remote>/<camera id>/...`. Uploaded files are
//! remembered in `<camera folder>/.cloud-uploaded`.

use crate::config::CloudConfig;
use crate::manager::Manager;
use anyhow::{Result, bail};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

async fn rclone(cfg: &CloudConfig, args: &[&str], secs: u64) -> Result<String> {
    let out = tokio::time::timeout(
        Duration::from_secs(secs),
        crate::load::cmd(&cfg.rclone_path)
            .args(args)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("rclone timed out"))?
    .map_err(|e| anyhow::anyhow!("cannot run rclone ({e}); install it from rclone.org"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!(
            "{}",
            err.lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("rclone failed")
                .trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A folder on this computer (synced by a cloud app) rather than an rclone remote.
pub fn is_local(remote: &str) -> bool {
    let r = remote.trim();
    r.starts_with('/')
        || r.starts_with("\\\\")
        || (r.len() > 2
            && r.as_bytes()[1] == b':'
            && (r.as_bytes()[2] == b'\\' || r.as_bytes()[2] == b'/'))
}

/// How backups are going (shown in Settings).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct CloudStatus {
    pub last_upload: Option<chrono::DateTime<chrono::Local>>,
    pub uploaded: u64,
    pub waiting: usize,
    pub last_error: Option<String>,
}

pub static STATUS: std::sync::Mutex<CloudStatus> = std::sync::Mutex::new(CloudStatus {
    last_upload: None,
    uploaded: 0,
    waiting: 0,
    last_error: None,
});

/// rclone accounts set up with `rclone config`.
pub async fn remotes(cfg: &CloudConfig) -> Result<Vec<String>> {
    let out = rclone(cfg, &["listremotes"], 20).await?;
    Ok(out
        .lines()
        .map(|l| l.trim().trim_end_matches(':').to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

/// Copy one file into a synced folder or NAS share: written under a
/// temporary name (`.partial`), flushed to the disk, checked — same size
/// and the same SHA-256 as the original, read back from the destination —
/// and only then renamed to its real name. A copy that doesn't match is
/// removed and the upload fails (it's tried again later); the original is
/// never touched.
fn copy_local(src: &Path, dest: &Path) -> Result<()> {
    if let Some(p) = dest.parent() {
        std::fs::create_dir_all(p)?;
    }
    let tmp = dest.with_extension(format!("{}.partial", dest.extension().and_then(|e| e.to_str()).unwrap_or("")));
    let res = (|| -> Result<()> {
        let want = sha256_file(src)?;
        let size = std::fs::metadata(src)?.len();
        {
            let mut from = std::io::BufReader::with_capacity(1 << 20, std::fs::File::open(src)?);
            let mut to = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(&tmp)?);
            std::io::copy(&mut from, &mut to)?;
            use std::io::Write;
            to.flush()?;
            to.get_ref().sync_all()?;
        }
        let got_size = std::fs::metadata(&tmp)?.len();
        anyhow::ensure!(got_size == size, "the copy is {got_size} bytes, the original {size}");
        anyhow::ensure!(sha256_file(&tmp)? == want, "the copy doesn't match the original (checksum)");
        std::fs::rename(&tmp, dest)?;
        Ok(())
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

/// SHA-256 of a file, read a piece at a time.
fn sha256_file(p: &Path) -> Result<[u8; 32]> {
    use sha2::Digest;
    use std::io::Read;
    let mut f = std::io::BufReader::with_capacity(1 << 20, std::fs::File::open(p)?);
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

/// A NAS's shared folder that needs a login: connect to it (Windows), so
/// copies there work, also when GAZE runs as a service.
#[cfg(windows)]
async fn connect_share(cfg: &CloudConfig) {
    let r = cfg.remote.trim();
    if !r.starts_with("\\\\") || cfg.user.trim().is_empty() {
        return;
    }
    // \\server\share
    let share: String = r
        .split('\\')
        .filter(|p| !p.is_empty())
        .take(2)
        .collect::<Vec<_>>()
        .join("\\");
    let share = format!("\\\\{share}");
    let _ = crate::load::cmd("net")
        .args([
            "use",
            &share,
            &cfg.password,
            &format!("/user:{}", cfg.user.trim()),
            "/persistent:no",
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .await;
}
#[cfg(not(windows))]
async fn connect_share(_cfg: &CloudConfig) {}

/// Check that the destination is reachable.
pub async fn test(cfg: &CloudConfig) -> Result<String> {
    if crate::ftp::is_ftp(&cfg.remote) {
        let t = crate::ftp::parse(&cfg.remote)
            .ok_or_else(|| anyhow::anyhow!("enter the NAS like ftp://192.168.1.20/Cameras"))?;
        crate::ftp::test(&t, &cfg.user, &cfg.password).await?;
        return Ok(format!(
            "Connected to {} — GAZE can save in {}",
            t.host,
            if t.folder.is_empty() {
                "/".to_string()
            } else {
                format!("/{}", t.folder)
            }
        ));
    }
    if is_local(&cfg.remote) {
        connect_share(cfg).await;
        let dir = PathBuf::from(cfg.remote.trim());
        std::fs::create_dir_all(&dir)
            .map_err(|e| anyhow::anyhow!("can't use {}: {e}", dir.display()))?;
        let probe = dir.join(".gaze-test");
        std::fs::write(&probe, b"ok")
            .map_err(|e| anyhow::anyhow!("can't write in {}: {e}", dir.display()))?;
        let _ = std::fs::remove_file(probe);
        return Ok(format!(
            "{} is ready — your sync app uploads what GAZE copies there",
            dir.display()
        ));
    }
    let version = rclone(cfg, &["version"], 20).await?;
    let remote = cfg.remote.trim();
    if remote.is_empty() {
        bail!("enter a destination such as gdrive:NVR");
    }
    rclone(cfg, &["mkdir", remote], 60).await?;
    rclone(cfg, &["lsd", remote], 60).await?;
    Ok(format!(
        "{} — {remote} is reachable",
        version.lines().next().unwrap_or("rclone").trim()
    ))
}

fn uploaded_list(dir: &Path) -> HashSet<String> {
    std::fs::read_to_string(dir.join(".cloud-uploaded"))
        .map(|t| t.lines().map(String::from).collect())
        .unwrap_or_default()
}

fn mark_uploaded(dir: &Path, rel: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(".cloud-uploaded"))
    {
        let _ = writeln!(f, "{rel}");
    }
}

/// Files ready to upload: (path, relative name).
/// `mode`: "ai" = only alerts confirmed by object recognition, "events" =
/// every alert, "all" = alerts plus all continuous recordings.
fn pending(
    dir: &Path,
    mode: &str,
    store: Option<&crate::events::EventStore>,
    done: &HashSet<String>,
) -> Vec<(PathBuf, String)> {
    let all = mode == "all";
    // Event files are named after their event: "<event id>.mp4" / ".jpg".
    let verified = |name: &str| {
        let id = name.rsplit_once('.').map(|x| x.0).unwrap_or(name);
        store
            .and_then(|s| s.get(id))
            .is_some_and(|e| !e.labels.is_empty())
    };
    let mut out = Vec::new();
    let settled = |p: &Path| {
        p.metadata()
            .and_then(|m| m.modified())
            .map(|t| t.elapsed().unwrap_or_default() > Duration::from_secs(30))
            .unwrap_or(false)
    };
    if let Ok(rd) = std::fs::read_dir(dir.join("events")) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let rel = format!("events/{name}");
            if (name.ends_with(".mp4") || name.ends_with(".jpg"))
                && !done.contains(&rel)
                && settled(&e.path())
                && (mode != "ai" || verified(&name))
            {
                out.push((e.path(), rel));
            }
        }
    }
    if all {
        let recs = crate::storage::list(dir);
        // Skip the newest file: it may still be recording.
        for r in recs.iter().skip(1) {
            let p = dir.join(&r.name);
            if !done.contains(&r.name) && settled(&p) {
                out.push((p, r.name.clone()));
            }
        }
    }
    out
}

pub async fn run(mgr: Arc<Manager>) {
    let mut tick = tokio::time::interval(Duration::from_secs(60));
    let mut last_error = String::new();
    // After a failure (NAS off, network down): wait longer each time — 1, 2,
    // 4 … up to 30 minutes — instead of trying every minute; back to normal
    // after a copy works. Recording carries on locally meanwhile, and
    // nothing is marked as copied until it really is.
    let mut fails: u32 = 0;
    let mut next_try = tokio::time::Instant::now();
    loop {
        tick.tick().await;
        let cfg = mgr.config();
        if !cfg.cloud.enabled {
            fails = 0;
            continue;
        }
        if tokio::time::Instant::now() < next_try {
            continue;
        }
        let remote = cfg.cloud.remote.trim().trim_end_matches('/').to_string();
        if is_local(&remote) {
            connect_share(&cfg.cloud).await;
        }
        let stores = mgr.event_stores();
        for cam in &cfg.cameras {
            let dir = cfg.camera_dir(cam);
            let done = uploaded_list(&dir);
            let store = stores.get(&cam.id).map(|s| s.as_ref());
            let todo = pending(&dir, &cfg.cloud.upload, store, &done);
            STATUS.lock().unwrap().waiting = todo.len();
            let job = (!todo.is_empty()).then(|| {
                let j = crate::jobs::start("cloud", format!("Cloud backup — {}", cam.name));
                j.total(todo.len() as u64);
                j
            });
            for (path, rel) in todo {
                if let Some(j) = &job {
                    j.pause_point().await;
                    j.step(rel.clone());
                }
                let res = if crate::ftp::is_ftp(&remote) {
                    match crate::ftp::parse(&remote) {
                        Some(t) => {
                            crate::ftp::upload(
                                &t,
                                &cfg.cloud.user,
                                &cfg.cloud.password,
                                &path,
                                &format!("{}/{rel}", cam.id),
                            )
                            .await
                        }
                        None => Err(anyhow::anyhow!("the FTP address isn't valid")),
                    }
                } else if is_local(&remote) {
                    let dest = PathBuf::from(&remote).join(&cam.id).join(&rel);
                    let p = path.clone();
                    tokio::task::spawn_blocking(move || copy_local(&p, &dest))
                        .await
                        .map_err(anyhow::Error::from)
                        .and_then(|r| r)
                } else {
                    let dest = format!("{remote}/{}/{rel}", cam.id);
                    rclone(
                        &cfg.cloud,
                        &["copyto", &path.display().to_string(), &dest],
                        3600,
                    )
                    .await
                    .map(|_| ())
                };
                match res {
                    Ok(()) => {
                        fails = 0;
                        mark_uploaded(&dir, &rel);
                        if let Some(j) = &job {
                            j.done_one();
                        }
                        info!("[{}] cloud: uploaded {rel}", cam.name);
                        last_error.clear();
                        let mut st = STATUS.lock().unwrap();
                        st.last_upload = Some(chrono::Local::now());
                        st.uploaded += 1;
                        st.waiting = st.waiting.saturating_sub(1);
                        st.last_error = None;
                    }
                    Err(e) => {
                        let msg = format!("{e:#}");
                        if msg != last_error {
                            warn!("[{}] cloud upload failed: {msg}", cam.name);
                            last_error = msg.clone();
                        }
                        if let Some(j) = &job {
                            j.fail(&msg);
                        }
                        STATUS.lock().unwrap().last_error = Some(msg);
                        fails += 1;
                        let wait = Duration::from_secs((60u64 << (fails - 1).min(5)).min(1800));
                        next_try = tokio::time::Instant::now() + wait;
                        if fails > 1 {
                            warn!("cloud backup: failed {fails} times in a row — next try in {} min", wait.as_secs() / 60);
                        }
                        break;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_or_rclone() {
        assert!(is_local(r"C:\Users\me\Google Drive\GAZE"));
        assert!(is_local("D:/Backups"));
        assert!(is_local(r"\\NAS\cameras"));
        assert!(is_local("/home/me/Dropbox"));
        assert!(!is_local("gdrive:NVR"));
        assert!(!is_local("b2:bucket/cams"));
    }
    use crate::events::{Event, EventStore, Label};

    #[test]
    fn ai_mode_uploads_only_recognized_alerts() {
        let dir = std::env::temp_dir().join(format!("nvr-cloud-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = EventStore::open(&dir);
        let now = chrono::Local::now();
        let ev = |id: &str, labels: Vec<Label>| Event {
            id: id.into(),
            camera: "c".into(),
            start: now,
            end: now,
            labels,
            motion: true,
            thumb: Some(format!("{id}.jpg")),
            clip: Some(format!("{id}.mp4")),
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
            hd_checking: false,
            hd_snap: None,
            meaning: None,
        };
        store.save_detected(ev(
            "ev-person",
            vec![Label {
                label: "person".into(),
                score: 0.9,
                color: None,
                hd: None,
            }],
        ));
        store.save_detected(ev("ev-motion", vec![]));
        for f in [
            "ev-person.jpg",
            "ev-person.mp4",
            "ev-motion.jpg",
            "ev-motion.mp4",
        ] {
            let p = dir.join("events").join(f);
            std::fs::write(&p, b"x").unwrap();
            let old = std::time::SystemTime::now() - Duration::from_secs(120);
            std::fs::File::options()
                .write(true)
                .open(&p)
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        store.flush();
        let mut ai: Vec<String> = pending(&dir, "ai", Some(&store), &HashSet::new())
            .into_iter()
            .map(|x| x.1)
            .collect();
        ai.sort();
        assert_eq!(ai, ["events/ev-person.jpg", "events/ev-person.mp4"]);
        assert_eq!(
            pending(&dir, "events", Some(&store), &HashSet::new()).len(),
            4
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn copies_are_checked_before_they_count() {
        let dir = std::env::temp_dir().join(format!("gaze-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("a.mp4");
        let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&src, &data).unwrap();
        let dest = dir.join("nas").join("cam").join("a.mp4");
        copy_local(&src, &dest).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        assert!(!dest.with_extension("mp4.partial").exists());
        assert_eq!(sha256_file(&src).unwrap(), sha256_file(&dest).unwrap());
        // A missing original: nothing left behind.
        assert!(copy_local(&dir.join("gone.mp4"), &dir.join("nas").join("gone.mp4")).is_err());
        assert!(!dir.join("nas").join("gone.mp4").exists() && !dir.join("nas").join("gone.mp4.partial").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
