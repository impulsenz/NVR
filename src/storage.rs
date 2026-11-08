//! Recording files on disk: listing, deletion and retention clean-up.

use crate::config::Config;
use anyhow::{Result, bail};
use chrono::{DateTime, Local, NaiveDateTime, TimeZone};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize)]
pub struct Recording {
    pub name: String,
    pub size: u64,
    /// Start time parsed from the file name (falls back to modification time).
    pub start: DateTime<Local>,
    pub modified: DateTime<Local>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StorageInfo {
    pub count: usize,
    pub used_bytes: u64,
    pub disk_free_bytes: Option<u64>,
    pub disk_total_bytes: Option<u64>,
}

/// All recordings, newest first.
pub fn list(dir: &Path) -> Vec<Recording> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Recording> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            if !name.ends_with(".mp4") {
                return None;
            }
            let meta = e.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            let modified: DateTime<Local> = meta.modified().ok()?.into();
            let start = parse_start(&name).unwrap_or(modified);
            Some(Recording {
                name,
                size: meta.len(),
                start,
                modified,
            })
        })
        .collect();
    out.sort_by(|a, b| b.start.cmp(&a.start).then(b.name.cmp(&a.name)));
    out
}

/// File names look like `front-door_2026-11-08_14-05-00.mp4`.
fn parse_start(name: &str) -> Option<DateTime<Local>> {
    let stem = name.strip_suffix(".mp4")?;
    let ts = stem.get(stem.len().checked_sub(19)?..)?;
    let naive = NaiveDateTime::parse_from_str(ts, "%Y-%m-%d_%H-%M-%S").ok()?;
    Local.from_local_datetime(&naive).earliest()
}

/// Resolve a user-supplied file name to a path inside the recordings dir.
pub fn resolve(dir: &Path, name: &str) -> Result<PathBuf> {
    let valid = name.ends_with(".mp4")
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    if !valid {
        bail!("invalid recording name");
    }
    let path = dir.join(name);
    if !path.is_file() {
        bail!("recording not found");
    }
    Ok(path)
}

pub fn info(dir: &Path) -> StorageInfo {
    let recs = list(dir);
    let (free, total) = disk_space(dir);
    StorageInfo {
        count: recs.len(),
        used_bytes: recs.iter().map(|r| r.size).sum(),
        disk_free_bytes: free,
        disk_total_bytes: total,
    }
}

#[cfg(unix)]
fn disk_space(dir: &Path) -> (Option<u64>, Option<u64>) {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = CString::new(dir.as_os_str().as_bytes()) else {
        return (None, None);
    };
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid NUL-terminated path and `st` is a valid out-pointer.
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return (None, None);
    }
    let frsize = st.f_frsize as u64;
    (
        Some(st.f_bavail as u64 * frsize),
        Some(st.f_blocks as u64 * frsize),
    )
}

#[cfg(not(unix))]
fn disk_space(_dir: &Path) -> (Option<u64>, Option<u64>) {
    (None, None)
}

/// Apply retention rules once. The newest file (possibly still being written) is never deleted.
pub fn cleanup(cfg: &Config) -> usize {
    let dir = &cfg.recording.dir;
    let mut recs = list(dir);
    if recs.len() <= 1 {
        return 0;
    }
    recs.remove(0); // keep newest
    recs.reverse(); // oldest first

    let mut removed = 0;
    let mut remove = |r: &Recording, why: &str| match std::fs::remove_file(dir.join(&r.name)) {
        Ok(()) => {
            info!("retention: deleted {} ({why})", r.name);
            removed += 1;
            true
        }
        Err(e) => {
            warn!("retention: could not delete {}: {e}", r.name);
            false
        }
    };

    let mut kept = Vec::new();
    if cfg.recording.retention_days > 0 {
        let max_age = Duration::from_secs(cfg.recording.retention_days * 86_400);
        let now = SystemTime::now();
        for r in recs {
            let age = now
                .duration_since(SystemTime::from(r.modified))
                .unwrap_or_default();
            if age > max_age && remove(&r, "older than retention period") {
                continue;
            }
            kept.push(r);
        }
    } else {
        kept = recs;
    }

    if cfg.recording.max_storage_gb > 0.0 {
        let limit = (cfg.recording.max_storage_gb * 1024.0 * 1024.0 * 1024.0) as u64;
        let mut used: u64 = list(dir).iter().map(|r| r.size).sum();
        for r in &kept {
            if used <= limit {
                break;
            }
            if remove(r, "storage limit reached") {
                used = used.saturating_sub(r.size);
            }
        }
    }
    removed
}

pub async fn retention_task(cfg: Arc<Config>) {
    let mut tick = tokio::time::interval(Duration::from_secs(600));
    loop {
        tick.tick().await;
        let cfg = cfg.clone();
        let _ = tokio::task::spawn_blocking(move || cleanup(&cfg)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names() {
        let t = parse_start("front-door_2026-11-08_14-05-00.mp4").unwrap();
        assert_eq!(
            t.format("%Y-%m-%d %H:%M:%S").to_string(),
            "2026-11-08 14:05:00"
        );
        assert!(parse_start("garbage.mp4").is_none());
    }

    #[test]
    fn rejects_traversal() {
        let dir = std::env::temp_dir();
        assert!(resolve(&dir, "../etc/passwd.mp4").is_err());
        assert!(resolve(&dir, "a/b.mp4").is_err());
        assert!(resolve(&dir, "x.txt").is_err());
    }

    #[test]
    fn storage_limit_deletes_oldest_but_keeps_newest() {
        let dir = std::env::temp_dir().join(format!("nvr-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for t in ["10-00-00", "10-05-00", "10-10-00"] {
            std::fs::write(
                dir.join(format!("cam_2026-01-01_{t}.mp4")),
                vec![0u8; 1 << 20],
            )
            .unwrap();
        }
        let mut cfg = Config::default();
        cfg.recording.dir = dir.clone();
        cfg.recording.max_storage_gb = 1.5 / 1024.0; // 1.5 MB
        assert_eq!(cleanup(&cfg), 2);
        let left: Vec<_> = list(&dir).into_iter().map(|r| r.name).collect();
        assert_eq!(left, vec!["cam_2026-01-01_10-10-00.mp4"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
