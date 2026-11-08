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
fn pending(dir: &Path, all: bool, done: &HashSet<String>) -> Vec<(PathBuf, String)> {
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
        for cam in &cfg.cameras {
            let dir = cfg.camera_dir(cam);
            let done = uploaded_list(&dir);
            for (path, rel) in pending(&dir, cfg.cloud.upload == "all", &done) {
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
