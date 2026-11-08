//! Recording files on disk: listing, deletion and retention clean-up.
//! Layout: `<storage.dir>/<camera id>/<camera id>_YYYY-MM-DD_HH-MM-SS.mp4`.

use crate::config::StorageConfig;
use anyhow::{Result, bail};
use chrono::{DateTime, Local, NaiveDateTime, TimeZone};
use serde::Serialize;
use std::path::{Path, PathBuf};
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

fn valid_component(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('.')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

pub fn camera_dir(root: &Path, id: &str) -> Result<PathBuf> {
    if !valid_component(id) {
        bail!("invalid camera id");
    }
    Ok(root.join(id))
}

/// All recordings in one folder, newest first.
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

/// File names end in `_YYYY-MM-DD_HH-MM-SS.mp4`.
fn parse_start(name: &str) -> Option<DateTime<Local>> {
    let stem = name.strip_suffix(".mp4")?;
    let ts = stem.get(stem.len().checked_sub(19)?..)?;
    let naive = NaiveDateTime::parse_from_str(ts, "%Y-%m-%d_%H-%M-%S").ok()?;
    Local.from_local_datetime(&naive).earliest()
}

/// Resolve a user-supplied file name to a path inside `dir`.
pub fn resolve(dir: &Path, name: &str) -> Result<PathBuf> {
    if !name.ends_with(".mp4") || !valid_component(name) {
        bail!("invalid recording name");
    }
    let path = dir.join(name);
    if !path.is_file() {
        bail!("recording not found");
    }
    Ok(path)
}

/// Sub-folders of the recordings root (one per camera, including removed ones).
fn camera_dirs(root: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(root)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default()
}

/// Total size of one folder's recordings.
pub fn usage(dir: &Path) -> (usize, u64) {
    let recs = list(dir);
    (recs.len(), recs.iter().map(|r| r.size).sum())
}

pub fn info(root: &Path) -> StorageInfo {
    let (mut count, mut used) = (0, 0);
    for d in camera_dirs(root) {
        let (c, u) = usage(&d);
        count += c;
        used += u;
    }
    let (free, total) = disk_space(root);
    StorageInfo {
        count,
        used_bytes: used,
        disk_free_bytes: free,
        disk_total_bytes: total,
    }
}

#[cfg(unix)]
pub fn disk_space(dir: &Path) -> (Option<u64>, Option<u64>) {
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
pub fn disk_space(_dir: &Path) -> (Option<u64>, Option<u64>) {
    (None, None)
}

/// Move recordings from the 0.1 layout (files directly in the root) into
/// per-camera folders.
pub fn migrate_flat_files(root: &Path, camera_ids: &[String]) {
    for r in list(root) {
        if let Some(id) = camera_ids
            .iter()
            .find(|id| r.name.starts_with(&format!("{id}_")))
        {
            let dir = root.join(id);
            if std::fs::create_dir_all(&dir).is_ok() {
                let _ = std::fs::rename(root.join(&r.name), dir.join(&r.name));
            }
        }
    }
}

/// Apply retention rules once across all cameras. The newest file in each
/// camera folder (possibly still being written) is never deleted.
pub fn cleanup(cfg: &StorageConfig) -> usize {
    let root = &cfg.dir;
    let mut all: Vec<(PathBuf, Recording)> = Vec::new();
    for d in camera_dirs(root) {
        let mut recs = list(&d);
        if !recs.is_empty() {
            recs.remove(0); // keep newest
        }
        all.extend(recs.into_iter().map(|r| (d.join(&r.name), r)));
    }
    all.sort_by_key(|a| a.1.start); // oldest first

    let mut removed = 0;
    let mut remove = |path: &Path, why: &str| match std::fs::remove_file(path) {
        Ok(()) => {
            info!("retention: deleted {} ({why})", path.display());
            removed += 1;
            true
        }
        Err(e) => {
            warn!("retention: could not delete {}: {e}", path.display());
            false
        }
    };

    let mut kept = Vec::new();
    let max_age = Duration::from_secs(cfg.retention_days * 86_400);
    let now = SystemTime::now();
    for (path, r) in all {
        let age = now
            .duration_since(SystemTime::from(r.modified))
            .unwrap_or_default();
        if cfg.retention_days > 0 && age > max_age && remove(&path, "older than retention period") {
            continue;
        }
        kept.push((path, r));
    }

    let gb = |x: f64| (x * 1024.0 * 1024.0 * 1024.0) as u64;
    let mut kept = kept.into_iter();
    if cfg.max_storage_gb > 0.0 {
        let limit = gb(cfg.max_storage_gb);
        let mut used = info(root).used_bytes;
        while used > limit {
            let Some((path, r)) = kept.next() else { break };
            if remove(&path, "storage limit reached") {
                used = used.saturating_sub(r.size);
            }
        }
    }
    if cfg.min_free_gb > 0.0 {
        let min_free = gb(cfg.min_free_gb);
        while disk_space(root).0.is_some_and(|free| free < min_free) {
            let Some((path, _)) = kept.next() else { break };
            remove(&path, "disk almost full");
        }
    }
    removed
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
        assert!(camera_dir(&dir, "..").is_err());
        assert!(camera_dir(&dir, "a/b").is_err());
    }

    #[test]
    fn storage_limit_deletes_oldest_across_cameras_but_keeps_newest() {
        let root = std::env::temp_dir().join(format!("nvr-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (cam, t) in [
            ("a", "10-00-00"),
            ("b", "10-02-00"),
            ("a", "10-05-00"),
            ("b", "10-10-00"),
        ] {
            std::fs::create_dir_all(root.join(cam)).unwrap();
            std::fs::write(
                root.join(cam).join(format!("{cam}_2026-01-01_{t}.mp4")),
                vec![0u8; 1 << 20],
            )
            .unwrap();
        }
        let cfg = StorageConfig {
            dir: root.clone(),
            max_storage_gb: 2.5 / 1024.0, // 2.5 MB
            min_free_gb: 0.0,
            ..Default::default()
        };
        assert_eq!(cleanup(&cfg), 2);
        assert_eq!(list(&root.join("a")).len(), 1);
        assert_eq!(list(&root.join("b"))[0].name, "b_2026-01-01_10-10-00.mp4");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
