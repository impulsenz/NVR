//! The time-lapse, made as the camera records.
//!
//! Motion detection decodes each camera's sub-stream a few times a second
//! all day. One of those pictures every time-lapse interval is kept here
//! (small JPEGs named by the time they show), and a recording's time-lapse
//! pages are put together from them: no second decode of the recording.
//! Footage from before (or from a camera without motion detection) is still
//! done the old way, from the file (see web::ensure_sprite).

use crate::ffmpeg::SpriteInfo;
use crate::imgutil::Frame;
use anyhow::Result;
use chrono::{DateTime, Local};
use std::path::{Path, PathBuf};
use std::time::Duration;

use std::sync::atomic::{AtomicU32, Ordering};

/// The time-lapse settings (Settings → Recording & storage), read by each
/// camera's motion detection as it goes: changed without restarting them.
static SECONDS: AtomicU32 = AtomicU32::new(10);
static WIDTH: AtomicU32 = AtomicU32::new(256);

pub fn configure(s: &crate::config::StorageConfig) {
    SECONDS.store(s.timelapse_seconds.max(1), Ordering::Relaxed);
    WIDTH.store(s.timelapse_width, Ordering::Relaxed);
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

/// The pictures for a recording from `start` to `end`, one per `interval`:
/// the kept picture nearest to each moment (within most of an interval).
/// None when too few are kept to cover it (then it's made from the file).
fn pick(kept: &[(i64, PathBuf)], start: i64, end: i64, interval: f64) -> Option<Vec<PathBuf>> {
    let step = (interval * 1000.0) as i64;
    let slots = ((end - start) as f64 / step as f64).ceil().max(1.0) as i64;
    let near = step * 3 / 4;
    let mut out: Vec<Option<PathBuf>> = Vec::with_capacity(slots as usize);
    for n in 0..slots {
        let t = start + n * step;
        let i = kept.partition_point(|x| x.0 < t);
        let best = [i.checked_sub(1), Some(i)]
            .into_iter()
            .flatten()
            .filter_map(|j| kept.get(j))
            .min_by_key(|x| (x.0 - t).abs())
            .filter(|x| (x.0 - t).abs() <= near);
        out.push(best.map(|x| x.1.clone()));
    }
    let have = out.iter().filter(|x| x.is_some()).count();
    // Most of it, and its start (the pages' first picture is the file's start).
    if (have as f64) < out.len() as f64 * 0.8 || out.first().is_none_or(Option::is_none) {
        return None;
    }
    // A missing moment: the picture before it.
    let mut last = None;
    Some(
        out.into_iter()
            .map(|x| {
                if x.is_some() {
                    last.clone_from(&x);
                }
                x.or_else(|| last.clone())
            })
            .collect::<Option<Vec<_>>>()?,
    )
}

/// Time-lapse pages for a recording, put together from the kept pictures:
/// written as `<prefix>-000.jpg` … like the ones made from files. `end`: how
/// far the recording goes (for one still being recorded: now).
pub fn sprite(
    camera_dir: &Path,
    start: DateTime<Local>,
    end: DateTime<Local>,
    interval: f64,
    aspect: f64,
    prefix: &Path,
) -> Option<SpriteInfo> {
    let interval = interval.max(1.0);
    let t = tile();
    let kept = list(camera_dir);
    let pics = pick(&kept, start.timestamp_millis(), end.timestamp_millis(), interval)?;
    let tiles: Vec<Vec<u8>> = pics
        .iter()
        .map(|p| {
            let f = crate::imgutil::decode_jpeg(&std::fs::read(p).ok()?).ok()?;
            Some(if (f.width, f.height) == t {
                (*f.rgb).clone()
            } else {
                crate::imgutil::resize_rgb(&f.rgb, f.width, f.height, t.0, t.1)
            })
        })
        .collect::<Option<_>>()?;
    let (count, pages) = crate::ffmpeg::write_sprite_pages(prefix, tiles, t).ok()?;
    Some(SpriteInfo {
        interval,
        count,
        columns: crate::ffmpeg::SPRITE_COLUMNS,
        per_page: crate::ffmpeg::SPRITE_PER_PAGE,
        pages,
        tile_width: t.0,
        tile_height: t.1,
        aspect,
        duration: (end - start).num_milliseconds() as f64 / 1000.0,
        version: Local::now().timestamp() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_nearest_pictures_and_needs_most_of_them() {
        let p = |t: i64| (t, PathBuf::from(format!("{t}.jpg")));
        // Pictures every 10 s from 0 to 60 s, one missing (30 s), a bit late.
        let kept: Vec<_> = [0, 10_400, 19_800, 40_100, 50_000, 60_000].into_iter().map(p).collect();
        let got = pick(&kept, 0, 60_000, 10.0).unwrap();
        assert_eq!(got.len(), 6);
        assert_eq!(got[1], PathBuf::from("10400.jpg"));
        assert_eq!(got[3], PathBuf::from("19800.jpg")); // missing: the one before
        // Nothing near the start, or too much missing: made from the file.
        assert!(pick(&kept[2..], 0, 60_000, 10.0).is_none());
        assert!(pick(&kept[..2], 0, 60_000, 10.0).is_none());
    }
}
