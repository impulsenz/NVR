//! Make finished recordings quick to open: recordings are written as
//! fragmented MP4 (safe if the power goes), which a browser has to scan
//! through to seek. Once a file is complete it is rewritten (without
//! re-encoding) as a normal MP4 with its index at the front, under a
//! temporary name and then swapped in, keeping the file's time stamp.

use crate::manager::Manager;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tracing::{info, warn};

/// True for a fragmented MP4 (has `moof` boxes at the top level).
pub fn is_fragmented(path: &Path) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let mut pos = 0u64;
    let mut h = [0u8; 16];
    for _ in 0..8 {
        if pos + 8 > len
            || f.seek(SeekFrom::Start(pos)).is_err()
            || f.read_exact(&mut h[..8]).is_err()
        {
            return false;
        }
        let size32 = u32::from_be_bytes(h[..4].try_into().unwrap()) as u64;
        let kind = &h[4..8];
        if kind == b"moof" {
            return true;
        }
        let size = match size32 {
            1 => {
                if f.read_exact(&mut h[8..16]).is_err() {
                    return false;
                }
                u64::from_be_bytes(h[8..16].try_into().unwrap())
            }
            0 => return false,
            n => n,
        };
        if size < 8 {
            return false;
        }
        pos += size;
    }
    false
}

async fn finalize(ffmpeg: &str, path: &Path) -> anyhow::Result<()> {
    let modified = std::fs::metadata(path)?.modified()?;
    let part = path.with_file_name(format!(
        ".{}{}",
        path.file_stem().unwrap_or_default().to_string_lossy(),
        crate::mp4fix::PART_SUFFIX
    ));
    let out = crate::load::background(&mut crate::load::cmd(ffmpeg))
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(path)
        .args(["-map", "0", "-c", "copy", "-movflags", "+faststart"])
        .args(crate::ffmpeg::hevc_tag(ffmpeg, path).await)
        .args(["-f", "mp4"])
        .arg(&part)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await?;
    if !out.status.success() || !part.exists() {
        let _ = std::fs::remove_file(&part);
        anyhow::bail!(
            "{}",
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .next()
                .unwrap_or("ffmpeg failed")
        );
    }
    // Same end time as before (the file's time stamp is its end time).
    std::fs::File::options()
        .write(true)
        .open(&part)?
        .set_modified(modified)?;
    // On Windows this fails while the file is being played; tried again later.
    if let Err(e) = crate::mp4fix::commit(&part, path) {
        let _ = std::fs::remove_file(&part);
        return Err(e.into());
    }
    Ok(())
}

/// Background job: finalize finished recordings (and timeline copies),
/// newest first, at low priority.
pub async fn run(mgr: Arc<Manager>) {
    tokio::time::sleep(Duration::from_secs(90)).await;
    let mut announced = false;
    loop {
        let ffmpeg = mgr.config().server.ffmpeg_path.clone();
        let mut done = 0;
        for c in mgr.cameras() {
            for dir in [c.dir.clone(), crate::storage::timeline_dir(&c.dir)] {
                let recs = crate::storage::list(&dir);
                // The newest file may still be written: never touch it.
                for r in recs.iter().skip(1).take(500) {
                    let p = dir.join(&r.name);
                    let old_enough = std::fs::metadata(&p)
                        .and_then(|m| m.modified())
                        .map(|t| {
                            SystemTime::now().duration_since(t).unwrap_or_default()
                                > Duration::from_secs(120)
                        })
                        .unwrap_or(false);
                    if !old_enough || !is_fragmented(&p) {
                        continue;
                    }
                    crate::load::wait_for_cpu(80, Duration::from_secs(300)).await;
                    if !announced {
                        info!("making finished recordings quick to open (background)");
                        announced = true;
                    }
                    match finalize(&ffmpeg, &p).await {
                        Ok(()) => done += 1,
                        Err(e) => warn!("could not finalize {}: {e:#}", p.display()),
                    }
                }
            }
        }
        if done > 0 {
            info!("{done} recording(s) made quick to open");
        }
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn detects_fragmented_files() {
        let dir = std::env::temp_dir().join(format!("nvr-frag-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bx = |k: &[u8; 4], n: usize| {
            let mut v = ((n + 8) as u32).to_be_bytes().to_vec();
            v.extend_from_slice(k);
            v.extend(std::iter::repeat_n(0u8, n));
            v
        };
        let frag = dir.join("f.mp4");
        std::fs::write(
            &frag,
            [
                bx(b"ftyp", 8),
                bx(b"moov", 20),
                bx(b"moof", 10),
                bx(b"mdat", 10),
            ]
            .concat(),
        )
        .unwrap();
        let plain = dir.join("p.mp4");
        std::fs::write(
            &plain,
            [bx(b"ftyp", 8), bx(b"moov", 20), bx(b"mdat", 10)].concat(),
        )
        .unwrap();
        assert!(super::is_fragmented(&frag));
        assert!(!super::is_fragmented(&plain));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
