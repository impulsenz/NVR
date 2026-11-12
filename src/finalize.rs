//! Keep recordings in the form that plays smoothly.
//!
//! Recordings are written as fragmented MP4: safe if the power goes, and
//! the form the continuous player streams from (see hls.rs: every piece of
//! a file is indexed and fetched on its own). Earlier versions copied
//! recordings with alerts in them, and any recording opened, into a normal
//! MP4 (one index for the whole file); those can't be streamed in pieces.
//! This background job copies them back (without re-encoding, keeping the
//! file's time stamp), newest first, at low priority. Copies made for
//! push-to-talk sound by older versions are done the same way.

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

/// The ffmpeg settings for a fragmented MP4 like the recorder writes.
pub const FRAGMENTED: [&str; 2] = ["-movflags", "+frag_keyframe+empty_moov+default_base_moof"];

/// Copy a normal MP4 into a fragmented one, swapped in under the same name.
async fn refragment(ffmpeg: &str, path: &Path) -> anyhow::Result<()> {
    let modified = std::fs::metadata(path)?.modified()?;
    let part = path.with_file_name(format!(
        ".{}{}",
        path.file_stem().unwrap_or_default().to_string_lossy(),
        crate::mp4fix::PART_SUFFIX
    ));
    let mut cmd = crate::load::cmd(ffmpeg);
    crate::load::background(&mut cmd);
    let out = cmd
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(path)
        .args(["-map", "0", "-c", "copy"])
        .args(crate::ffmpeg::hevc_tag(ffmpeg, path).await)
        .args(FRAGMENTED)
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

/// Background job: copy normal-MP4 recordings (and timeline copies) back to
/// fragmented ones, newest first, at low priority.
pub async fn run(mgr: Arc<Manager>) {
    tokio::time::sleep(Duration::from_secs(90)).await;
    // Files that couldn't be swapped (e.g. open on Windows): tried later.
    let mut failed: std::collections::HashMap<std::path::PathBuf, std::time::Instant> = Default::default();
    loop {
        let ffmpeg = mgr.config().server.ffmpeg_path.clone();
        let mut done = 0;
        let mut job: Option<crate::jobs::Job> = None;
        failed.retain(|_, t| t.elapsed() < Duration::from_secs(1800));
        for c in mgr.cameras() {
            for dir in [c.dir.clone(), crate::storage::timeline_dir(&c.dir)] {
                // The newest file may still be written: never touch it.
                for r in crate::storage::list(&dir).iter().skip(1) {
                    let p = dir.join(&r.name);
                    let old_enough = std::fs::metadata(&p)
                        .and_then(|m| m.modified())
                        .map(|t| SystemTime::now().duration_since(t).unwrap_or_default() > Duration::from_secs(120))
                        .unwrap_or(false);
                    if !old_enough || failed.contains_key(&p) || is_fragmented(&p) {
                        continue;
                    }
                    let j = job.get_or_insert_with(|| {
                        crate::jobs::start("maintenance", "Preparing recordings for smooth playback")
                    });
                    j.waiting();
                    crate::load::wait_for_cpu(80, Duration::from_secs(300)).await;
                    j.pause_point().await;
                    j.step(format!("{} — {}", c.cfg.name, r.name));
                    j.done_one();
                    match refragment(&ffmpeg, &p).await {
                        Ok(()) => done += 1,
                        Err(e) => {
                            failed.insert(p.clone(), std::time::Instant::now());
                            warn!("could not prepare {} for smooth playback: {e:#}", p.display())
                        }
                    }
                }
            }
        }
        if done > 0 {
            info!("{done} recording(s) prepared for smooth playback");
        }
        tokio::time::sleep(Duration::from_secs(300)).await;
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
