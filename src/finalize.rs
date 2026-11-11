//! Make finished recordings quick to open: recordings are written as
//! fragmented MP4 (safe if the power goes), which a browser has to scan
//! through to seek. A complete file is copied (without re-encoding) into a
//! normal MP4 with an index, under a temporary name and then swapped in,
//! keeping the file's time stamp.
//!
//! To spare the disk, only footage that's likely to be watched is done:
//! - in the background, recordings with an alert in them;
//! - any other recording the first time someone opens it.
//!
//! The index stays at the end of the file (no "faststart"): browsers fetch
//! it with one extra request, and it saves writing the whole file twice.

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

async fn finalize(ffmpeg: &str, path: &Path, urgent: bool) -> anyhow::Result<()> {
    let modified = std::fs::metadata(path)?.modified()?;
    let part = path.with_file_name(format!(
        ".{}{}",
        path.file_stem().unwrap_or_default().to_string_lossy(),
        crate::mp4fix::PART_SUFFIX
    ));
    let mut cmd = crate::load::cmd(ffmpeg);
    if !urgent {
        crate::load::background(&mut cmd);
    }
    let out = cmd
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(path)
        .args(["-map", "0", "-c", "copy"])
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

/// Finished: not written to for two minutes, and not the newest file of its
/// folder (which may still be recorded).
fn finished(path: &Path) -> bool {
    let old_enough = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|t| SystemTime::now().duration_since(t).unwrap_or_default() > Duration::from_secs(120))
        .unwrap_or(false);
    old_enough
        && path.parent().is_some_and(|dir| {
            crate::storage::list(dir).first().is_some_and(|newest| {
                path.file_name()
                    .is_some_and(|n| n.to_string_lossy() != newest.name)
            })
        })
}

/// One at a time per file, however many requests ask for it.
static BUSY: std::sync::Mutex<
    Option<std::collections::HashMap<std::path::PathBuf, Arc<tokio::sync::Mutex<()>>>>,
> = std::sync::Mutex::new(None);

fn file_lock(path: &Path) -> Arc<tokio::sync::Mutex<()>> {
    BUSY.lock()
        .unwrap()
        .get_or_insert_with(Default::default)
        .entry(path.to_path_buf())
        .or_default()
        .clone()
}

/// Files that couldn't be made quick to open, and when.
static FAILED: std::sync::Mutex<
    Option<std::collections::HashMap<std::path::PathBuf, std::time::Instant>>,
> = std::sync::Mutex::new(None);

fn failed_recently(path: &Path) -> bool {
    let mut g = FAILED.lock().unwrap();
    let m = g.get_or_insert_with(Default::default);
    m.retain(|_, t| t.elapsed() < Duration::from_secs(600));
    m.contains_key(path)
}

/// Someone is opening this recording: make it quick to open first, waiting
/// at most `wait` (it carries on in the background after that).
pub async fn prepare_for_viewing(ffmpeg: &str, path: &Path, wait: Duration) {
    if !is_fragmented(path) || failed_recently(path) || !finished(path) {
        return;
    }
    let lock = file_lock(path);
    let (ffmpeg, path) = (ffmpeg.to_string(), path.to_path_buf());
    let task = tokio::spawn(async move {
        let _g = lock.lock().await;
        if is_fragmented(&path) {
            match finalize(&ffmpeg, &path, true).await {
                Ok(()) => info!("made {} quick to open", path.display()),
                Err(e) => {
                    // On Windows a file being played can't be swapped: don't
                    // copy it again for every request while it's open.
                    FAILED
                        .lock()
                        .unwrap()
                        .get_or_insert_with(Default::default)
                        .insert(path.clone(), std::time::Instant::now());
                    warn!("could not finalize {}: {e:#}", path.display())
                }
            }
        }
    });
    let _ = tokio::time::timeout(wait, task).await;
}

/// Background job: finalize finished recordings (and timeline copies),
/// newest first, at low priority.
pub async fn run(mgr: Arc<Manager>) {
    tokio::time::sleep(Duration::from_secs(90)).await;
    let mut announced = false;
    loop {
        let ffmpeg = mgr.config().server.ffmpeg_path.clone();
        let mut done = 0;
        let mut job: Option<crate::jobs::Job> = None;
        for c in mgr.cameras() {
            for dir in [c.dir.clone(), crate::storage::timeline_dir(&c.dir)] {
                let recs = crate::storage::list(&dir);
                // Only footage with an alert in it (the rest is done when
                // opened): the alerts of the time these files cover.
                let alerts: Vec<_> = match recs.iter().take(501).next_back() {
                    Some(oldest) => c
                        .events
                        .between(oldest.start, chrono::Local::now())
                        .into_iter()
                        .map(|e| (e.start, e.end))
                        .collect(),
                    None => Vec::new(),
                };
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
                    if !old_enough
                        || !alerts.iter().any(|(s, e)| *e >= r.start && *s <= r.end)
                        || !is_fragmented(&p)
                    {
                        continue;
                    }
                    let j = job.get_or_insert_with(|| {
                        crate::jobs::start("maintenance", "Making alert recordings quick to open")
                    });
                    j.waiting();
                    crate::load::wait_for_cpu(80, Duration::from_secs(300)).await;
                    j.pause_point().await;
                    j.step(format!("{} — {}", c.cfg.name, r.name));
                    j.done_one();
                    if !announced {
                        info!("making recordings with alerts quick to open (background)");
                        announced = true;
                    }
                    let lock = file_lock(&p);
                    let _g = lock.lock().await;
                    if !is_fragmented(&p) {
                        continue;
                    }
                    match finalize(&ffmpeg, &p, false).await {
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
