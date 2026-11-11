//! Optional backup of recordings to the cloud, two ways:
//! - **a synced folder** on this computer (Google Drive, OneDrive or Dropbox
//!   for desktop): files are copied there and that app uploads them;
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

/// Copy one file into a synced folder (whole file first, then renamed).
fn copy_local(src: &Path, dest: &Path) -> Result<()> {
    if let Some(p) = dest.parent() {
        std::fs::create_dir_all(p)?;
    }
    let tmp = dest.with_extension("part");
    std::fs::copy(src, &tmp)?;
    std::fs::rename(&tmp, dest)?;
    Ok(())
}

/// Check that the destination is reachable.
pub async fn test(cfg: &CloudConfig) -> Result<String> {
    if is_local(&cfg.remote) {
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
    loop {
        tick.tick().await;
        let cfg = mgr.config();
        if !cfg.cloud.enabled {
            continue;
        }
        let remote = cfg.cloud.remote.trim().trim_end_matches('/').to_string();
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
                let res = if is_local(&remote) {
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
            desc: String::new(),
            boxes: Vec::new(),
            summary: String::new(),
            notes: Vec::new(),
            thumb_at: None,
            faces: Vec::new(),
            plates: Vec::new(),
            identified: false,
            items: Vec::new(),
            notified: 0,
            story: String::new(),
            suspicious: String::new(),
        };
        store.save(ev(
            "ev-person",
            vec![Label {
                label: "person".into(),
                score: 0.9,
                color: None,
            }],
        ));
        store.save(ev("ev-motion", vec![]));
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
}
