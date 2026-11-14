//! Crash and power-loss protection for recordings.
//!
//! Recordings are fragmented MP4: a header, then self-contained pieces
//! (`moof` + `mdat`) every keyframe. If the NVR or the computer stops
//! suddenly, only the piece being written can be incomplete. This module
//! finds such a file and cuts it back to the last complete piece, so it
//! always plays (losing at most a second or two at the very end).

use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use tracing::{info, warn};

#[derive(Debug, PartialEq)]
pub enum Check {
    /// The file ends on a complete piece.
    Ok,
    /// Incomplete data after `good_len` bytes.
    Truncated { good_len: u64 },
    /// Not an MP4 we can repair (or nothing usable).
    Unrecognized,
}

/// Walk the top-level boxes and find where the last complete piece ends.
pub fn check(path: &Path) -> std::io::Result<Check> {
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    let mut pos = 0u64;
    // End of the last box after which the file is valid (not a moof
    // waiting for its mdat).
    let mut good = 0u64;
    let mut seen_moov = false;
    let mut header = [0u8; 16];
    while pos < len {
        if len - pos < 8 {
            break;
        }
        f.seek(SeekFrom::Start(pos))?;
        f.read_exact(&mut header[..8])?;
        let size32 = u32::from_be_bytes(header[..4].try_into().unwrap()) as u64;
        let kind = [header[4], header[5], header[6], header[7]];
        let size = match size32 {
            0 => len - pos, // extends to end of file
            1 => {
                if len - pos < 16 {
                    break;
                }
                f.read_exact(&mut header[8..16])?;
                u64::from_be_bytes(header[8..16].try_into().unwrap())
            }
            n => n,
        };
        if size < 8 || !kind.iter().all(|c| c.is_ascii_alphanumeric() || *c == b' ') {
            break; // garbage
        }
        if pos + size > len {
            break; // box cut off
        }
        pos += size;
        match &kind {
            b"moov" => {
                seen_moov = true;
                good = pos;
            }
            // A fragment header is only useful together with its data.
            b"moof" => {}
            _ => good = pos,
        }
    }
    if !seen_moov {
        return Ok(Check::Unrecognized);
    }
    Ok(if good == len {
        Check::Ok
    } else {
        Check::Truncated { good_len: good }
    })
}

/// Repair a file if needed. Returns true if it was changed.
pub fn repair(path: &Path) -> bool {
    match check(path) {
        Ok(Check::Truncated { good_len }) => {
            let len = path.metadata().map(|m| m.len()).unwrap_or(0);
            match OpenOptions::new().write(true).open(path).and_then(|f| {
                f.set_len(good_len)?;
                f.sync_all()
            }) {
                Ok(()) => {
                    info!(
                        "repaired {} (removed {} incomplete bytes at the end)",
                        path.display(),
                        len - good_len
                    );
                    true
                }
                Err(e) => {
                    warn!("could not repair {}: {e}", path.display());
                    false
                }
            }
        }
        _ => false,
    }
}

/// Files being written get this ending until they are complete.
pub const PART_SUFFIX: &str = ".part.mp4";

/// Make a finished file permanent: flush it to disk, then give it its real
/// name in one step (a rename is atomic, so the file is either complete or
/// not there at all).
pub fn commit(part: &Path, dest: &Path) -> std::io::Result<()> {
    if let Ok(f) = std::fs::File::open(part) {
        let _ = f.sync_all();
    }
    std::fs::rename(part, dest)?;
    // Also flush the folder entry (where the platform allows it).
    #[cfg(unix)]
    if let Some(dir) = dest.parent()
        && let Ok(d) = std::fs::File::open(dir)
    {
        let _ = d.sync_all();
    }
    Ok(())
}

/// Check and repair the recordings in `dir` changed in the last `hours`.
pub fn repair_dir(dir: &Path, hours: u64) -> usize {
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(hours * 3600);
    let mut fixed = 0;
    let mut dirs = vec![
        dir.to_path_buf(),
        dir.join("events"),
        dir.join(".buffer"),
        dir.join("timeline"),
    ];
    while let Some(d) = dirs.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        // Only the files being written when GAZE stopped can be cut short:
        // the newest few in each folder (one per recorder writing there).
        // Checking every recent file read each one through, which on a
        // network drive (NAS) could hold up starting for minutes.
        let mut recent: Vec<(std::time::SystemTime, std::path::PathBuf)> = Vec::new();
        for e in rd.flatten() {
            let p = e.path();
            // Leftovers of files that were being written when it stopped.
            if p.to_string_lossy().ends_with(PART_SUFFIX) {
                info!("removing incomplete file {}", p.display());
                let _ = std::fs::remove_file(&p);
                continue;
            }
            if p.extension().is_none_or(|x| x != "mp4") {
                continue;
            }
            if let Ok(t) = e.metadata().and_then(|m| m.modified())
                && t >= cutoff
            {
                recent.push((t, p));
            }
        }
        recent.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, p) in recent.into_iter().take(4) {
            if repair(&p) {
                fixed += 1;
            }
        }
    }
    fixed
}

/// After a recorder process stopped unexpectedly: repair its own newest
/// files (only those starting with `prefix`, only in `dir`, so files other
/// recorders are still writing are never touched).
pub fn repair_recent(dir: &Path, prefix: &str, minutes: u64) -> usize {
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(minutes * 60);
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    rd.flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.starts_with(prefix)
                && name.ends_with(".mp4")
                && e.metadata()
                    .and_then(|m| m.modified())
                    .map(|t| t >= cutoff)
                    .unwrap_or(false)
        })
        .filter(|e| repair(&e.path()))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(kind: &[u8; 4], body: usize) -> Vec<u8> {
        let mut v = ((body + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend(std::iter::repeat_n(0u8, body));
        v
    }

    #[test]
    fn trims_incomplete_tail() {
        let dir = std::env::temp_dir().join(format!("nvr-mp4-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.mp4");
        let mut good = [
            bx(b"ftyp", 16),
            bx(b"moov", 100),
            bx(b"moof", 50),
            bx(b"mdat", 1000),
        ]
        .concat();
        let good_len = good.len() as u64;
        // A complete moof without its mdat, then a cut-off mdat.
        good.extend(bx(b"moof", 50));
        good.extend(&bx(b"mdat", 1000)[..300]);
        std::fs::write(&p, &good).unwrap();
        assert_eq!(check(&p).unwrap(), Check::Truncated { good_len });
        assert!(repair(&p));
        assert_eq!(std::fs::metadata(&p).unwrap().len(), good_len);
        assert_eq!(check(&p).unwrap(), Check::Ok);
        assert!(!repair(&p));
        // Not an MP4 / only a header: left alone.
        std::fs::write(&p, b"hello world, not a video").unwrap();
        assert_eq!(check(&p).unwrap(), Check::Unrecognized);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
