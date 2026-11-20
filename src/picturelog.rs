//! The time-lapse, made as the camera records.
//!
//! Motion detection decodes each camera's sub-stream a few times a second
//! all day. One of those pictures every time-lapse interval is kept here
//! (small JPEGs named by the time they show) for a few hours, and scrub.rs
//! puts them into each hour's pages: no second decode of the recording.
//! Footage from before (or from a camera without motion detection) is read
//! from the file by scrub.rs instead.

use crate::imgutil::Frame;
use anyhow::Result;
use chrono::{DateTime, Local};
use std::path::{Path, PathBuf};
use std::time::Duration;

use std::sync::atomic::{AtomicU32, Ordering};

/// The time-lapse settings (Settings → Recording & storage), read by each
/// camera's motion detection as it goes: changed without restarting them.
static ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
static SECONDS: AtomicU32 = AtomicU32::new(10);
static WIDTH: AtomicU32 = AtomicU32::new(256);

pub fn configure(s: &crate::config::StorageConfig) {
    ON.store(s.timelapse, Ordering::Relaxed);
    SECONDS.store(s.timelapse_seconds.max(1), Ordering::Relaxed);
    WIDTH.store(s.timelapse_width, Ordering::Relaxed);
}

/// Time-lapse previews are on (Settings → Recording & storage): when off,
/// nothing is kept, made or served.
pub fn enabled() -> bool {
    ON.load(Ordering::Relaxed)
}

/// Seconds between time-lapse pictures.
pub fn seconds() -> u32 {
    SECONDS.load(Ordering::Relaxed).max(1)
}

/// Size of a time-lapse picture (16:9; bigger is sharper).
pub fn tile() -> (u32, u32) {
    let w = WIDTH.load(Ordering::Relaxed).clamp(128, 1024) & !1;
    (w, (w * 9 / 16) & !1)
}
/// Pictures are kept this long (a recording's pages are made well before).
const KEEP: Duration = Duration::from_secs(6 * 3600);

pub fn dir(camera_dir: &Path) -> PathBuf {
    camera_dir.join(".thumbs").join("live")
}

/// Keep a picture taken at `t`.
pub fn save(camera_dir: &Path, t: DateTime<Local>, f: &Frame) -> Result<()> {
    if !enabled() {
        return Ok(());
    }
    let (tw, th) = tile();
    let small = crate::imgutil::resize_rgb(&f.rgb, f.width, f.height, tw, th);
    // One flat colour (a frame that couldn't be decoded): not kept (the
    // pages use the picture before it).
    if crate::ffmpeg::flat_picture(&small) {
        return Ok(());
    }
    let d = dir(camera_dir);
    std::fs::create_dir_all(&d)?;
    let jpg = crate::imgutil::encode_jpeg(&small, tw, th, 80)?;
    let p = d.join(format!("{}.jpg", t.timestamp_millis()));
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, jpg)?;
    std::fs::rename(&tmp, &p)?;
    Ok(())
}

/// Remove pictures older than a few hours.
pub fn prune(camera_dir: &Path) {
    let cutoff = Local::now().timestamp_millis() - KEEP.as_millis() as i64;
    for (t, p) in list(camera_dir) {
        if t < cutoff {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// The kept pictures: (time in ms, file), oldest first.
/// The kept pictures from `from` to `to` (milliseconds), oldest first.
pub fn between(camera_dir: &Path, from: i64, to: i64) -> Vec<PathBuf> {
    list(camera_dir).into_iter().filter(|(t, _)| *t >= from && *t <= to).map(|(_, p)| p).collect()
}

fn list(camera_dir: &Path) -> Vec<(i64, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(dir(camera_dir)) else { return Vec::new() };
    let mut v: Vec<(i64, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            Some((name.strip_suffix(".jpg")?.parse().ok()?, e.path()))
        })
        .collect();
    v.sort_by_key(|x| x.0);
    v
}
