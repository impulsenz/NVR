//! Folder browsing for choosing where recordings are saved (the web page
//! runs on another device, so it cannot use the computer's own folder picker),
//! and moving existing recordings to a new folder.

use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

#[derive(Debug, Serialize)]
pub struct Place {
    pub name: String,
    pub path: String,
    pub free_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct Listing {
    /// Full path of the folder shown (empty = list of drives).
    pub path: String,
    pub parent: Option<String>,
    pub folders: Vec<Place>,
    /// Drives and common places.
    pub places: Vec<Place>,
    pub writable: bool,
    pub free_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
}

fn place(name: String, p: &Path) -> Place {
    let space = crate::sysmon::disk_space(p);
    Place {
        name,
        path: p.display().to_string(),
        free_bytes: space.map(|s| s.0),
        total_bytes: space.map(|s| s.1),
    }
}

/// Drives (mount points) and handy starting folders.
pub fn places() -> Vec<Place> {
    let mut out: Vec<Place> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut add = |name: String, p: PathBuf, out: &mut Vec<Place>| {
        if p.is_dir() && seen.insert(p.clone()) {
            out.push(place(name, &p));
        }
    };
    if let Ok(dir) = std::env::current_dir() {
        add("NVR folder".into(), dir, &mut out);
    }
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        add("Home".into(), PathBuf::from(home), &mut out);
    }
    if let Ok(disks) = sysinfo::Disks::new_with_refreshed_list() {
        for d in &disks {
            let mp = d.mount_point().to_path_buf();
            let s = mp.display().to_string();
            // Skip system/virtual mounts on Linux and macOS.
            if [
                "/boot",
                "/proc",
                "/sys",
                "/dev",
                "/run",
                "/snap",
                "/var/lib",
                "/System/Volumes",
            ]
            .iter()
            .any(|x| s.starts_with(x))
            {
                continue;
            }
            let label = d.name().to_string_lossy().trim().to_string();
            let name = if label.is_empty() || label == s {
                s.clone()
            } else {
                format!("{s} ({label})")
            };
            add(name, mp, &mut out);
        }
    }
    out
}

fn writable(dir: &Path) -> bool {
    let probe = dir.join(".nvr-write-test");
    let ok = std::fs::write(&probe, b"ok").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

pub fn list(path: &str) -> Result<Listing> {
    let places = places();
    if path.trim().is_empty() {
        return Ok(Listing {
            path: String::new(),
            parent: None,
            folders: vec![],
            places,
            writable: false,
            free_bytes: None,
            total_bytes: None,
        });
    }
    let p = PathBuf::from(path.trim());
    let p = if p.is_relative() {
        std::env::current_dir()?.join(p)
    } else {
        p
    };
    if !p.is_dir() {
        bail!("folder not found: {}", p.display());
    }
    let p = std::fs::canonicalize(&p).unwrap_or(p);
    let mut folders: Vec<Place> = std::fs::read_dir(&p)
        .with_context(|| format!("cannot open {}", p.display()))?
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let hidden = name.starts_with('.')
                || name.starts_with('$')
                || name == "System Volume Information";
            (!hidden).then(|| Place {
                path: e.path().display().to_string(),
                name,
                free_bytes: None,
                total_bytes: None,
            })
        })
        .take(3000)
        .collect();
    folders.sort_by_key(|f| f.name.to_lowercase());
    let space = crate::sysmon::disk_space(&p);
    Ok(Listing {
        path: display(&p),
        parent: p.parent().map(display),
        writable: writable(&p),
        folders,
        places,
        free_bytes: space.map(|s| s.0),
        total_bytes: space.map(|s| s.1),
    })
}

/// Windows canonical paths start with \\?\ — show them normally.
fn display(p: &Path) -> String {
    let s = p.display().to_string();
    s.strip_prefix(r"\\?\UNC\")
        .map(|r| format!(r"\\{r}"))
        .or_else(|| s.strip_prefix(r"\\?\").map(String::from))
        .unwrap_or(s)
}

pub fn mkdir(parent: &str, name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty()
        || name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|'])
        || name == "."
        || name == ".."
    {
        bail!("invalid folder name");
    }
    let p = Path::new(parent.trim()).join(name);
    std::fs::create_dir_all(&p).with_context(|| format!("cannot create {}", p.display()))?;
    Ok(display(&p))
}

#[derive(Debug, Default, Serialize)]
pub struct MoveReport {
    pub files: u64,
    pub bytes: u64,
    pub skipped: u64,
}

/// Move everything in `src` into `dst` (merging), then remove `src`.
/// Renames when on the same drive, otherwise copies and deletes.
pub fn move_tree(src: &Path, dst: &Path) -> Result<MoveReport> {
    let mut r = MoveReport::default();
    if !src.exists() || src == dst {
        return Ok(r);
    }
    if dst.starts_with(src) {
        bail!("the new folder cannot be inside the old one");
    }
    move_inner(src, dst, &mut r)?;
    let _ = remove_empty_dirs(src);
    info!(
        "moved {} files ({} MB) from {} to {}",
        r.files,
        r.bytes / 1_000_000,
        src.display(),
        dst.display()
    );
    Ok(r)
}

fn move_inner(src: &Path, dst: &Path, r: &mut MoveReport) -> Result<()> {
    std::fs::create_dir_all(dst).with_context(|| format!("cannot create {}", dst.display()))?;
    for e in std::fs::read_dir(src)?.flatten() {
        let from = e.path();
        let to = dst.join(e.file_name());
        if e.file_type()?.is_dir() {
            move_inner(&from, &to, r)?;
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        let len = e.metadata().map(|m| m.len()).unwrap_or(0);
        if to.exists() {
            // Lists kept by the NVR are merged; other duplicates stay where they are.
            if name == "events.jsonl" || name == ".cloud-uploaded" {
                let text = std::fs::read(&from)?;
                use std::io::Write;
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(&to)?
                    .write_all(&text)?;
                std::fs::remove_file(&from)?;
            } else {
                warn!(
                    "not moving {}: {} already exists",
                    from.display(),
                    to.display()
                );
                r.skipped += 1;
            }
            continue;
        }
        if std::fs::rename(&from, &to).is_err() {
            // Different drive: copy, then delete the original.
            std::fs::copy(&from, &to).with_context(|| format!("copying {}", from.display()))?;
            std::fs::remove_file(&from)?;
        }
        r.files += 1;
        r.bytes += len;
    }
    Ok(())
}

fn remove_empty_dirs(dir: &Path) -> std::io::Result<()> {
    for e in std::fs::read_dir(dir)?.flatten() {
        if e.file_type()?.is_dir() {
            let _ = remove_empty_dirs(&e.path());
        }
    }
    std::fs::remove_dir(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_and_moves() {
        let base = std::env::temp_dir().join(format!("nvr-fs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let src = base.join("old/cam");
        std::fs::create_dir_all(src.join("events")).unwrap();
        std::fs::write(src.join("cam_2026-01-01_00-00-00.mp4"), b"video").unwrap();
        std::fs::write(src.join("events/events.jsonl"), b"{\"a\":1}\n").unwrap();
        let dst = base.join("new/cam");
        std::fs::create_dir_all(dst.join("events")).unwrap();
        std::fs::write(dst.join("events/events.jsonl"), b"{\"b\":2}\n").unwrap();

        let l = list(base.to_str().unwrap()).unwrap();
        assert!(l.writable);
        assert_eq!(
            l.folders
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
            ["new", "old"]
        );
        assert!(!places().is_empty());
        assert!(mkdir(base.to_str().unwrap(), "../x").is_err());
        assert!(
            mkdir(base.to_str().unwrap(), "made")
                .unwrap()
                .ends_with("made")
        );

        let r = move_tree(&src, &dst).unwrap();
        assert_eq!(r.files, 1);
        assert!(!src.exists());
        assert!(dst.join("cam_2026-01-01_00-00-00.mp4").exists());
        assert_eq!(
            std::fs::read_to_string(dst.join("events/events.jsonl")).unwrap(),
            "{\"b\":2}\n{\"a\":1}\n"
        );
        assert!(move_tree(&base.join("new"), &base.join("new/inner")).is_err());
        std::fs::remove_dir_all(&base).unwrap();
    }
}
