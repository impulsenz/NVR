//! Optional backup of recordings to cloud storage with rclone
//! (Google Drive, OneDrive, Dropbox, S3, Backblaze B2, ... — anything rclone supports).
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
        tokio::process::Command::new(&cfg.rclone_path)
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

/// Check that the destination is reachable.
pub async fn test(cfg: &CloudConfig) -> Result<String> {
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
            for (path, rel) in pending(&dir, &cfg.cloud.upload, store, &done) {
                let dest = format!("{remote}/{}/{rel}", cam.id);
                let src = path.display().to_string();
                match rclone(&cfg.cloud, &["copyto", &src, &dest], 3600).await {
                    Ok(_) => {
                        mark_uploaded(&dir, &rel);
                        info!("[{}] cloud: uploaded {rel}", cam.name);
                        last_error.clear();
                    }
                    Err(e) => {
                        let msg = format!("{e:#}");
                        if msg != last_error {
                            warn!("[{}] cloud upload failed: {msg}", cam.name);
                            last_error = msg;
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
        };
        store.save(ev(
            "ev-person",
            vec![Label {
                label: "person".into(),
                score: 0.9,
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
