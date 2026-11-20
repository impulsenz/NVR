//! Long-term time-lapse: one picture a day from a camera at a time you choose
//! (say 12:00), kept for as long as you like, and a video made from them on
//! request, so a garden, a building site or a view through the seasons can be
//! watched over months or years.
//!
//! Each entry (Settings → Time-lapse, `Config.longlapse`) is a camera and a
//! time of day. The picture is the camera's full-quality (main stream) picture
//! of that moment, saved as `nvr-data/longlapse/<entry id>/<YYYY-MM-DD>.jpg`
//! (about 0.3 to 2 MB a day). Nothing deletes them but you. If the camera
//! can't be reached at that time it is tried again every 5 minutes for 3
//! hours, then that day is left out. Videos go in `…/videos/`. It never
//! touches recordings, and recording never depends on it.

use crate::config::LongLapse;
use crate::manager::Manager;
use crate::sync::MutexExt;
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Local, NaiveDate, NaiveTime};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long after the chosen time a missing picture is still tried for.
const LATE: chrono::Duration = chrono::Duration::hours(3);

pub fn root(data: &Path) -> PathBuf {
    data.join("longlapse")
}

/// A folder name that can't climb out (an entry id from the settings).
fn safe_id(id: &str) -> Option<&str> {
    (!id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')).then_some(id)
}

pub fn dir(data: &Path, id: &str) -> Option<PathBuf> {
    safe_id(id).map(|i| root(data).join(i))
}

/// "2026-11-20.jpg" → its date (only that exact shape is a picture of ours).
pub fn date_of(name: &str) -> Option<NaiveDate> {
    let stem = name.strip_suffix(".jpg")?;
    (stem.len() == 10).then(|| NaiveDate::parse_from_str(stem, "%Y-%m-%d").ok()).flatten()
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Pic {
    pub name: String,
    pub date: NaiveDate,
    pub size: u64,
}

/// The pictures of an entry, oldest first.
pub fn pictures(data: &Path, id: &str) -> Vec<Pic> {
    let Some(d) = dir(data, id) else { return Vec::new() };
    let mut v: Vec<Pic> = std::fs::read_dir(d)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            Some(Pic { date: date_of(&name)?, size: e.metadata().ok()?.len(), name })
        })
        .collect();
    v.sort_by_key(|p| p.date);
    v
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Video {
    pub name: String,
    pub size: u64,
    pub made: Option<DateTime<Local>>,
}

pub fn videos(data: &Path, id: &str) -> Vec<Video> {
    let Some(d) = dir(data, id).map(|d| d.join("videos")) else { return Vec::new() };
    let mut v: Vec<Video> = std::fs::read_dir(d)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            let md = e.metadata().ok()?;
            is_video_name(&name).then(|| Video { name, size: md.len(), made: md.modified().ok().map(DateTime::<Local>::from) })
        })
        .collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.made));
    v
}

/// A video of ours ("2026-01-01_to_2026-11-20_12fps.mp4"): nothing else is served or deleted.
pub fn is_video_name(name: &str) -> bool {
    name.ends_with(".mp4") && !name.contains("part") && name.len() <= 64 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

pub fn note_error(id: &str, msg: String) {
    set_state(id, |s| s.error = Some(msg));
}

/// Is a picture due now? After the chosen time, not more than `LATE` after it, and none today yet.
pub fn due(now: DateTime<Local>, time: NaiveTime, have_today: bool) -> bool {
    if have_today {
        return false;
    }
    let Some(at) = now.date_naive().and_time(time).and_local_timezone(Local).earliest() else { return false };
    now >= at && now <= at + LATE
}

/// What the page shows about each entry's last try.
#[derive(Debug, Clone, Default, Serialize)]
pub struct State {
    pub last_ok: Option<DateTime<Local>>,
    pub error: Option<String>,
}
static STATE: Mutex<Option<HashMap<String, State>>> = Mutex::new(None);
static TRIED: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);
/// Videos being made (entry id → what it is doing).
static MAKING: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

pub fn state(id: &str) -> State {
    STATE.locked().as_ref().and_then(|m| m.get(id).cloned()).unwrap_or_default()
}
fn set_state(id: &str, f: impl FnOnce(&mut State)) {
    f(STATE.locked().get_or_insert_with(HashMap::new).entry(id.to_string()).or_default());
}
pub fn making(id: &str) -> Option<String> {
    MAKING.locked().as_ref().and_then(|m| m.get(id).cloned())
}

/// Take today's picture for an entry now (replacing one already taken today).
pub async fn capture(mgr: &Manager, e: &LongLapse) -> Result<PathBuf> {
    let rt = mgr.camera(&e.camera).context("that camera isn't there any more")?;
    let jpg = rt.live_main.snapshot().await.context("no picture from the camera")?;
    let dir = dir(&mgr.data_dir, &e.id).context("unusable entry")?;
    std::fs::create_dir_all(&dir)?;
    let today = Local::now().date_naive();
    let path = dir.join(format!("{}.jpg", today.format("%Y-%m-%d")));
    let tmp = dir.join("taking.part");
    std::fs::write(&tmp, &jpg)?;
    std::fs::rename(&tmp, &path)?;
    // (The small copy for the page's thumbnails is made again.)
    let _ = std::fs::remove_file(dir.join("thumbs").join(path.file_name().unwrap_or_default()));
    set_state(&e.id, |s| {
        s.last_ok = Some(Local::now());
        s.error = None;
    });
    tracing::info!("[{}] long-term time-lapse: picture for {} saved ({} KB)", rt.cfg.name, today, jpg.len() / 1024);
    Ok(path)
}

/// Runs for as long as GAZE does: takes each entry's picture when it is due.
pub async fn run(mgr: Arc<Manager>) {
    tokio::time::sleep(Duration::from_secs(40)).await;
    loop {
        let cfg = mgr.config();
        for e in cfg.longlapse.iter().filter(|e| e.enabled) {
            let Ok(time) = NaiveTime::parse_from_str(&e.time, "%H:%M") else { continue };
            let have = dir(&mgr.data_dir, &e.id).is_some_and(|d| d.join(format!("{}.jpg", Local::now().format("%Y-%m-%d"))).exists());
            if !due(Local::now(), time, have) {
                continue;
            }
            // Not again within 5 minutes of a try.
            let recent = TRIED.locked().as_ref().and_then(|m| m.get(&e.id)).is_some_and(|t| t.elapsed() < Duration::from_secs(300));
            if recent {
                continue;
            }
            TRIED.locked().get_or_insert_with(HashMap::new).insert(e.id.clone(), Instant::now());
            if let Err(err) = capture(&mgr, e).await {
                let msg = format!("{err:#}");
                // (Said in the log once an hour per entry: it is tried every 5 minutes.)
                if state(&e.id).error.as_deref() != Some(&msg) {
                    tracing::warn!("long-term time-lapse ({} at {}): {msg} — trying again in 5 minutes", e.camera, e.time);
                }
                set_state(&e.id, |s| s.error = Some(msg));
            }
        }
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}

/// What a video is made of.
#[derive(Debug, Clone)]
pub struct VideoOpts {
    pub fps: u32,
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
}

/// The file name a video of these pictures gets (made again under the same name).
pub fn video_name(first: NaiveDate, last: NaiveDate, fps: u32) -> String {
    format!("{}_to_{}_{}fps.mp4", first.format("%Y-%m-%d"), last.format("%Y-%m-%d"), fps)
}

/// Make a video of an entry's pictures (in the date range), one picture per frame.
pub async fn make_video(mgr: &Manager, id: &str, opts: VideoOpts) -> Result<PathBuf> {
    let dir = dir(&mgr.data_dir, id).context("unusable entry")?;
    let fps = opts.fps.clamp(1, 60);
    let pics: Vec<Pic> = pictures(&mgr.data_dir, id).into_iter().filter(|p| opts.from.is_none_or(|f| p.date >= f) && opts.to.is_none_or(|t| p.date <= t)).collect();
    if pics.len() < 2 {
        bail!("a video needs at least two pictures; there {} {} in that period", if pics.len() == 1 { "is" } else { "are" }, pics.len());
    }
    {
        let mut m = MAKING.locked();
        let m = m.get_or_insert_with(HashMap::new);
        if m.contains_key(id) {
            bail!("a video of this time-lapse is already being made");
        }
        m.insert(id.to_string(), format!("{} pictures", pics.len()));
    }
    let res = make_video_inner(mgr, &dir, &pics, fps).await;
    MAKING.locked().as_mut().map(|m| m.remove(id));
    res
}

/// The ffmpeg list of pictures: each shown for one frame.
fn concat_list(pics: &[PathBuf], fps: u32) -> String {
    let mut s = String::new();
    for p in pics {
        s.push_str(&format!("file '{}'\nduration {:.6}\n", p.display().to_string().replace('\\', "/").replace('\'', "'\\''"), 1.0 / fps as f64));
    }
    // (The last picture is listed again: concat drops the duration of the final one.)
    if let Some(p) = pics.last() {
        s.push_str(&format!("file '{}'\n", p.display().to_string().replace('\\', "/").replace('\'', "'\\''")));
    }
    s
}

async fn make_video_inner(mgr: &Manager, dir: &Path, pics: &[Pic], fps: u32) -> Result<PathBuf> {
    let vdir = dir.join("videos");
    std::fs::create_dir_all(&vdir)?;
    let job = crate::jobs::start("longlapse", format!("Long-term time-lapse video ({} pictures)", pics.len()));
    job.total(pics.len() as u64);
    // The size of the first picture (later ones that differ are fitted inside it).
    let first = crate::imgutil::decode_jpeg(&std::fs::read(dir.join(&pics[0].name))?)?;
    let (w, h) = ((first.width.min(3840) & !1).max(2), (first.height.min(2160) & !1).max(2));
    let list = vdir.join("list.part.txt");
    std::fs::write(&list, concat_list(&pics.iter().map(|p| dir.join(&p.name)).collect::<Vec<_>>(), fps))?;
    let out = vdir.join(video_name(pics[0].date, pics[pics.len() - 1].date, fps));
    let tmp = out.with_extension("part.mp4");
    let ffmpeg = mgr.config().server.ffmpeg_path.clone();
    let vf = format!("scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:black,format=yuv420p");
    job.step("Making the video");
    let mut cmd = crate::load::cmd(&ffmpeg);
    crate::load::background(&mut cmd);
    cmd.args(["-hide_banner", "-loglevel", "error", "-y", "-f", "concat", "-safe", "0", "-i"])
        .arg(&list)
        .args(["-vf", &vf, "-r", &fps.to_string(), "-c:v", "libx264", "-preset", "medium", "-crf", "20", "-movflags", "+faststart", "-f", "mp4"])
        .arg(&tmp)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let run = tokio::time::timeout(Duration::from_secs(3600), cmd.output()).await;
    let _ = std::fs::remove_file(&list);
    let o = match run {
        Err(_) => {
            let _ = std::fs::remove_file(&tmp);
            bail!("making the video took more than an hour")
        }
        Ok(r) => r.context("could not run ffmpeg")?,
    };
    if !o.status.success() {
        let _ = std::fs::remove_file(&tmp);
        bail!("ffmpeg: {}", String::from_utf8_lossy(&o.stderr).lines().next().unwrap_or("it failed"));
    }
    std::fs::rename(&tmp, &out)?;
    job.done_one();
    tracing::info!("long-term time-lapse: video made from {} pictures ({})", pics.len(), out.file_name().and_then(|n| n.to_str()).unwrap_or(""));
    Ok(out)
}

/// A small copy of a picture for the page's gallery (made once, kept).
pub fn thumbnail(dir: &Path, name: &str) -> Result<PathBuf> {
    date_of(name).context("not a picture of this time-lapse")?;
    let thumb = dir.join("thumbs").join(name);
    if thumb.is_file() {
        return Ok(thumb);
    }
    let f = crate::imgutil::decode_jpeg(&std::fs::read(dir.join(name))?)?;
    let w = 320u32.min(f.width).max(2);
    let h = ((f.height as f64 * w as f64 / f.width.max(1) as f64).round() as u32).max(2);
    let small = f.resize(w, h);
    std::fs::create_dir_all(dir.join("thumbs"))?;
    std::fs::write(&thumb, small.jpeg(80)?)?;
    Ok(thumb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn a_picture_is_due_after_its_time_for_three_hours_and_once_a_day() {
        let t = |h, m| Local.with_ymd_and_hms(2026, 11, 20, h, m, 0).unwrap();
        let noon = NaiveTime::from_hms_opt(12, 0, 0).unwrap();
        assert!(!due(t(11, 59), noon, false), "not yet");
        assert!(due(t(12, 0), noon, false));
        assert!(due(t(14, 59), noon, false), "late, still tried");
        assert!(!due(t(15, 1), noon, false), "too late: that day is left out");
        assert!(!due(t(12, 30), noon, true), "already taken today");
    }

    #[test]
    fn only_our_own_picture_names_count_and_entry_ids_cannot_climb_out() {
        assert_eq!(date_of("2026-11-20.jpg"), NaiveDate::from_ymd_opt(2026, 11, 20));
        for bad in ["2026-11-20.png", "2026-13-40.jpg", "../2026-11-20.jpg", "x.jpg", "2026-11-20.jpg.exe", "20261120.jpg"] {
            assert!(date_of(bad).is_none(), "{bad}");
        }
        assert!(safe_id("ll-1700000000-1").is_some());
        for bad in ["", "../x", "a/b", "a b", "a\\b"] {
            assert!(safe_id(bad).is_none(), "{bad:?}");
        }
    }

    #[tokio::test]
    async fn pictures_are_listed_in_date_order_and_a_video_is_made_from_them() {
        let ff = "ffmpeg";
        let data = std::env::temp_dir().join(format!("nvr-longlapse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        let d = dir(&data, "ll-test").unwrap();
        std::fs::create_dir_all(&d).unwrap();
        // Five days of made-up pictures (colours changing), named out of order.
        for (i, day) in [5u32, 2, 4, 1, 3].iter().enumerate() {
            let rgb = vec![(40 * i) as u8; 160 * 90 * 3];
            let jpg = crate::imgutil::encode_jpeg(&rgb, 160, 90, 80).unwrap();
            std::fs::write(d.join(format!("2026-11-{:02}.jpg", day)), jpg).unwrap();
        }
        std::fs::write(d.join("notes.txt"), "x").unwrap();
        let p = pictures(&data, "ll-test");
        assert_eq!(p.len(), 5);
        assert!(p.windows(2).all(|w| w[0].date < w[1].date), "oldest first");
        // The list ffmpeg reads: one line per picture, quotes safe, the last listed again.
        let list = concat_list(&[PathBuf::from("/x/it's/2026-11-01.jpg"), PathBuf::from("/x/2026-11-02.jpg")], 10);
        assert!(list.contains("it'\\''s") && list.matches("file '").count() == 3 && list.contains("duration 0.100000"), "{list}");
        // A thumbnail is made once.
        let t = thumbnail(&d, "2026-11-01.jpg").unwrap();
        assert!(t.is_file() && thumbnail(&d, "2026-11-01.jpg").unwrap() == t);
        assert!(thumbnail(&d, "notes.txt").is_err());
        // The video (when this computer has ffmpeg with libx264).
        if std::process::Command::new(ff).arg("-version").output().is_ok() {
            let mgr_cfg = crate::config::Config::default();
            // (make_video_inner needs a Manager only for the ffmpeg path: run the same command here.)
            let list_path = d.join("videos").join("l.txt");
            std::fs::create_dir_all(d.join("videos")).unwrap();
            std::fs::write(&list_path, concat_list(&p.iter().map(|x| d.join(&x.name)).collect::<Vec<_>>(), 5)).unwrap();
            let out = d.join("videos").join("v.mp4");
            let ok = std::process::Command::new(&mgr_cfg.server.ffmpeg_path)
                .args(["-v", "error", "-y", "-f", "concat", "-safe", "0", "-i"])
                .arg(&list_path)
                .args(["-vf", "scale=160:90,format=yuv420p", "-r", "5", "-c:v", "libx264"])
                .arg(&out)
                .status();
            if ok.is_ok_and(|s| s.success()) {
                assert!(std::fs::metadata(&out).unwrap().len() > 500);
                let dur = std::process::Command::new("ffprobe").args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"]).arg(&out).output();
                if let Ok(o) = dur {
                    let secs: f64 = String::from_utf8_lossy(&o.stdout).trim().parse().unwrap_or(0.0);
                    assert!((0.9..=1.3).contains(&secs), "five pictures at 5 per second: {secs}");
                }
            }
        }
        assert_eq!(video_name(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(), NaiveDate::from_ymd_opt(2026, 11, 20).unwrap(), 12), "2026-01-01_to_2026-11-20_12fps.mp4");
        let _ = std::fs::remove_dir_all(&data);
    }
}
