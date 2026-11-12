//! Push to talk in the recordings: what was said through a camera's speaker
//! (chimes included) is kept as a sound file with its start time, and once
//! the recordings of that time are finished it's mixed into their sound
//! (main recording, the light timeline copy and alert clips), so both sides
//! of the conversation play back. Only the sound is re-encoded; the video is
//! copied as it is, and each file keeps its time (the timeline depends on it).

use crate::manager::{CameraRuntime, Manager};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Local, NaiveDateTime, TimeZone};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

const RATE: u32 = 8000;
/// Talk files are kept this long (mixed long before).
const KEEP_DAYS: i64 = 30;

pub fn talk_dir(camera_dir: &Path) -> PathBuf {
    camera_dir.join("talk")
}

/// Keep a talk session: 8 kHz mono samples that started playing at `start`.
pub fn save(camera_dir: &Path, start: DateTime<Local>, samples: &[i16]) -> Result<PathBuf> {
    let dir = talk_dir(camera_dir);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!(
        "talk_{}.wav",
        start.format("%Y-%m-%d_%H-%M-%S-%3f")
    ));
    let data_len = (samples.len() * 2) as u32;
    let mut b = Vec::with_capacity(44 + data_len as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&RATE.to_le_bytes());
    b.extend_from_slice(&(RATE * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        b.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(&path, b)?;
    Ok(path)
}

/// A kept talk session: file, start, end.
struct Session {
    path: PathBuf,
    name: String,
    start: DateTime<Local>,
    end: DateTime<Local>,
}

fn sessions(camera_dir: &Path) -> Vec<Session> {
    let Ok(rd) = std::fs::read_dir(talk_dir(camera_dir)) else {
        return Vec::new();
    };
    rd.flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let stamp = name.strip_prefix("talk_")?.strip_suffix(".wav")?;
            let naive = NaiveDateTime::parse_from_str(stamp, "%Y-%m-%d_%H-%M-%S-%3f").ok()?;
            let start = Local.from_local_datetime(&naive).earliest()?;
            let len = e.metadata().ok()?.len().saturating_sub(44);
            let secs = len as f64 / (RATE as f64 * 2.0);
            Some(Session {
                path: e.path(),
                name,
                start,
                end: start + chrono::Duration::milliseconds((secs * 1000.0) as i64),
            })
        })
        .collect()
}

/// A file the talk is mixed into: path, when it starts, when it ends.
struct Target {
    path: PathBuf,
    start: DateTime<Local>,
    end: DateTime<Local>,
}

fn targets(c: &CameraRuntime) -> Vec<Target> {
    let mut out = Vec::new();
    for r in crate::library::recordings(c) {
        out.push(Target {
            path: c.dir.join(&r.name),
            start: r.start,
            end: r.end,
        });
    }
    for r in crate::library::timeline_recordings(c) {
        out.push(Target {
            path: crate::storage::timeline_dir(&c.dir).join(&r.name),
            start: r.start,
            end: r.end,
        });
    }
    for e in c.events.all() {
        if let (Some(clip), Some(start)) = (&e.clip, e.clip_start) {
            out.push(Target {
                path: c.events.dir().join(clip),
                start,
                end: e.end + chrono::Duration::seconds(60),
            });
        }
    }
    out
}

fn done_file(camera_dir: &Path) -> PathBuf {
    talk_dir(camera_dir).join("mixed.txt")
}

fn load_done(camera_dir: &Path) -> HashSet<String> {
    std::fs::read_to_string(done_file(camera_dir))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

fn mark_done(camera_dir: &Path, key: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(done_file(camera_dir))
    {
        let _ = writeln!(f, "{key}");
    }
}

/// Whether a file has a sound track.
async fn has_audio(ffmpeg: &str, file: &Path) -> bool {
    let out = crate::load::cmd(ffmpeg)
        .args(["-hide_banner", "-i"])
        .arg(file)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await;
    out.is_ok_and(|o| String::from_utf8_lossy(&o.stderr).contains("Audio:"))
}

/// Mix a talk session into a file, starting `offset` seconds into the file
/// (negative: the talk began before the file). The file is replaced and
/// keeps its modification time.
async fn mix(ffmpeg: &str, target: &Path, talk: &Path, offset: f64) -> Result<()> {
    let modified = std::fs::metadata(target)?.modified()?;
    let talk_chain = if offset >= 0.0 {
        format!(
            "[1:a]aresample=16000,adelay={}:all=1[t]",
            (offset * 1000.0).round() as i64
        )
    } else {
        format!(
            "[1:a]atrim=start={:.3},asetpts=PTS-STARTPTS,aresample=16000[t]",
            -offset
        )
    };
    let audio = has_audio(ffmpeg, target).await;
    let filter = if audio {
        format!(
            "{talk_chain};[0:a]aresample=16000[c];[c][t]amix=inputs=2:duration=first:dropout_transition=0:normalize=0[a]"
        )
    } else {
        format!("{talk_chain};[t]apad[a]")
    };
    // Hidden while it's made (not listed as a recording).
    let tmp = target.with_file_name(format!(
        ".{}.talk-tmp",
        target.file_name().unwrap_or_default().to_string_lossy()
    ));
    let mut cmd = crate::load::cmd(ffmpeg);
    crate::load::background(&mut cmd)
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(target)
        .arg("-i")
        .arg(talk)
        .args([
            "-filter_complex",
            &filter,
            "-map",
            "0:v",
            "-map",
            "[a]",
            "-c:v",
            "copy",
        ])
        .args(crate::ffmpeg::hevc_tag(ffmpeg, target).await)
        .args(["-c:a", "aac", "-b:a", "64k", "-ac", "1"]);
    if !audio {
        cmd.arg("-shortest");
    }
    let out = cmd
        .args(["-f", "mp4"])
        .arg(&tmp)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .context("running ffmpeg")?;
    if !out.status.success() || std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0) == 0 {
        let _ = std::fs::remove_file(&tmp);
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    if let Ok(f) = std::fs::File::options().write(true).open(&tmp) {
        let _ = f.set_modified(modified);
    }
    // Fails while the file is open elsewhere (e.g. being watched on
    // Windows): tried again next round.
    if let Err(e) = std::fs::rename(&tmp, target) {
        let _ = std::fs::remove_file(&tmp);
        bail!("can't replace the file yet: {e}");
    }
    Ok(())
}

async fn round(m: &Manager) {
    let ffmpeg = m.config().server.ffmpeg_path;
    let now = Local::now();
    for c in m.cameras() {
        let list = sessions(&c.dir);
        if list.is_empty() {
            continue;
        }
        let done = load_done(&c.dir);
        let all = targets(&c);
        for s in &list {
            // Old talk files: mixed long ago (or nothing left to mix into).
            if (now - s.start).num_days() >= KEEP_DAYS {
                let _ = std::fs::remove_file(&s.path);
                continue;
            }
            for t in all.iter().filter(|t| t.start < s.end && t.end > s.start) {
                // Only finished files (not still being written).
                let quiet = std::fs::metadata(&t.path)
                    .and_then(|m| m.modified())
                    .is_ok_and(|m| m.elapsed().unwrap_or_default() > Duration::from_secs(90));
                if !quiet {
                    continue;
                }
                let key = format!("{}|{}", s.name, t.path.display());
                if done.contains(&key) {
                    continue;
                }
                crate::load::wait_for_cpu(85, Duration::from_secs(60)).await;
                let offset = (s.start - t.start).num_milliseconds() as f64 / 1000.0;
                match mix(&ffmpeg, &t.path, &s.path, offset).await {
                    Ok(()) => {
                        mark_done(&c.dir, &key);
                        info!(
                            "[{}] push-to-talk sound added to {}",
                            c.cfg.name,
                            t.path.file_name().unwrap_or_default().to_string_lossy()
                        );
                    }
                    Err(e) => warn!(
                        "[{}] couldn't add push-to-talk sound to {}: {e:#}",
                        c.cfg.name,
                        t.path.display()
                    ),
                }
            }
        }
    }
}

/// Background job: mix kept talk into the recordings once they're finished.
pub async fn run(m: Arc<Manager>) {
    tokio::time::sleep(Duration::from_secs(30)).await;
    loop {
        round(&m).await;
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn keeps_talk_with_its_start_time() {
        let dir = std::env::temp_dir().join(format!("gaze-talk-{}", std::process::id()));
        let start = chrono::Local::now();
        let p = super::save(&dir, start, &vec![1000i16; 16000]).unwrap();
        let s = super::sessions(&dir);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].path, p);
        assert!((s[0].start - start).num_milliseconds().abs() < 2);
        assert_eq!((s[0].end - s[0].start).num_milliseconds(), 2000);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
