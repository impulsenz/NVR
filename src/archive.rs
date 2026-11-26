//! Deep storage: a second, bigger place (a NAS or another drive) for older
//! footage.
//!
//! Recordings stay on the local disk for a while (24 hours by default, or
//! what each camera says). After that each file is moved to
//! `<deep storage>/<camera folder>/`: copied, checked (size and checksum,
//! see `cloud::copy_local`) and only then taken off the local disk. Deep
//! storage has its own keep time and size limit; footage held in an
//! Evidence Locker case is never deleted from it.
//!
//! What's being moved shows in System → Tasks (files left, size, speed).

use crate::config::{ArchivePlan, Config};
use crate::manager::Manager;
use crate::storage::{self, Recording};
use crate::sync::MutexExt;
use chrono::{DateTime, Duration as CDuration, Local};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use tracing::{info, warn};

/// "Move now" wakes the mover.
pub static RUN_NOW: LazyLock<tokio::sync::Notify> = LazyLock::new(tokio::sync::Notify::new);

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ArchiveStatus {
    pub last_run: Option<DateTime<Local>>,
    pub moved_files: u64,
    pub moved_bytes: u64,
    pub deleted_files: u64,
    pub last_error: Option<String>,
    pub running: bool,
}
pub static STATUS: LazyLock<Mutex<ArchiveStatus>> = LazyLock::new(Default::default);

/// Files due to move: older than the plan's local time, not the newest one
/// (it may still be recording) and not touched in the last minute.
pub fn due(dir: &Path, plan: &ArchivePlan, now: DateTime<Local>) -> Vec<Recording> {
    let cutoff = now - CDuration::hours(plan.after_hours as i64);
    let settled = now - CDuration::seconds(60);
    storage::list(dir).into_iter().skip(1).filter(|r| r.end < cutoff && r.end < settled).collect()
}

/// Everything but the newest recording (it may still be written), settled for a minute, whatever its age: what the
/// mover takes when the local disk needs room (see `storage::cleanup`: footage is never deleted for room before it was moved).
pub fn due_now(dir: &Path, now: DateTime<Local>) -> Vec<Recording> {
    let settled = now - CDuration::seconds(60);
    storage::list(dir).into_iter().skip(1).filter(|r| r.end < settled).collect()
}

/// Copy a recording's cached previews (`.thumbs/<name>.*`) next to where the recording went, the pages
/// first and the file that says they are complete last. Returns how many were copied.
pub fn copy_previews(from_dir: &Path, to_dir: &Path, name: &str) -> usize {
    let (src, dst) = (from_dir.join(".thumbs"), to_dir.join(".thumbs"));
    let prefix = format!("{name}.");
    let Ok(rd) = std::fs::read_dir(&src) else { return 0 };
    let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(&prefix))).collect();
    if files.is_empty() || std::fs::create_dir_all(&dst).is_err() {
        return 0;
    }
    // (".sprite.json" last: a preview is only counted as there once it exists.)
    files.sort_by_key(|p| p.to_string_lossy().ends_with(".sprite.json"));
    let mut n = 0;
    for f in files {
        let Some(file_name) = f.file_name() else { continue };
        let tmp = dst.join(format!("{}.part", file_name.to_string_lossy()));
        if std::fs::copy(&f, &tmp).is_ok() && std::fs::rename(&tmp, dst.join(file_name)).is_ok() {
            n += 1;
        } else {
            let _ = std::fs::remove_file(&tmp);
        }
    }
    n
}

/// Move one recording: copied and checked, then taken off the local disk.
pub fn move_one(dir: &Path, plan: &ArchivePlan, r: &Recording) -> anyhow::Result<()> {
    let src = dir.join(&r.name);
    let dest = plan.dest.join(&r.name);
    // A copy that was finished before (the local file couldn't be removed
    // then) is only checked for its size.
    let have = std::fs::metadata(&dest).map(|m| m.len() == r.size).unwrap_or(false);
    if !have {
        crate::cloud::copy_local(&src, &dest)?;
    }
    // Its time-lapse previews go with it (they are tiny next to the video): deleting them here made every
    // moved file's previews be decoded again from the NAS. A preview that can't be copied is made again later.
    let copied = copy_previews(dir, &plan.dest, &r.name);
    if copied > 0 {
        tracing::debug!("deep storage: {} preview file(s) of {} moved with it", copied, r.name);
    }
    storage::delete_recording(dir, &r.name)?;
    Ok(())
}

/// Deep storage's own clean-up: older than its keep time, then the oldest
/// above its size limit. Evidence Locker footage stays.
pub fn retention(cfg: &Config) -> u64 {
    let now = Local::now();
    let mut removed = 0;
    let mut all: Vec<(String, std::path::PathBuf, Recording)> = Vec::new();
    for cam in &cfg.cameras {
        let Some(plan) = cfg.archive_for(cam) else { continue };
        let locked = |r: &Recording| crate::cases::locks_time(&cam.id, r.start, r.end).is_some();
        for r in storage::list(&plan.dest) {
            let old = plan.retention_days > 0 && r.end < now - CDuration::days(plan.retention_days as i64);
            if old && !locked(&r) {
                if storage::delete_recording(&plan.dest, &r.name).is_ok() {
                    info!("deep storage: deleted {} (older than {} days)", plan.dest.join(&r.name).display(), plan.retention_days);
                    removed += 1;
                }
                continue;
            }
            if !locked(&r) {
                all.push((cam.id.clone(), plan.dest.clone(), r));
            }
        }
    }
    let max = cfg.storage.archive.max_gb;
    if max > 0.0 {
        let limit = (max * 1_000_000_000.0) as u64;
        let mut used: u64 = all.iter().map(|x| x.2.size).sum();
        all.sort_by_key(|x| x.2.start);
        for (_, dir, r) in all {
            if used <= limit {
                break;
            }
            if storage::delete_recording(&dir, &r.name).is_ok() {
                used = used.saturating_sub(r.size);
                removed += 1;
                info!("deep storage: deleted {} (size limit)", dir.join(&r.name).display());
            }
        }
    }
    removed
}

/// For the settings page: what's where, camera by camera.
pub fn overview(cfg: &Config) -> Value {
    let now = Local::now();
    let a = &cfg.storage.archive;
    let reachable = a.enabled && !a.dir.as_os_str().is_empty() && std::fs::read_dir(&a.dir).is_ok();
    let cams: Vec<Value> = cfg
        .cameras
        .iter()
        .map(|c| {
            let dir = cfg.camera_dir(c);
            let plan = cfg.archive_for(c);
            let local = storage::list(&dir);
            let arch = plan.as_ref().map(|p| storage::list(&p.dest)).unwrap_or_default();
            let waiting = plan.as_ref().map(|p| due(&dir, p, now)).unwrap_or_default();
            json!({
                "id": c.id, "name": c.name,
                "active": plan.is_some(),
                "after_hours": plan.as_ref().map(|p| p.after_hours),
                "retention_days": plan.as_ref().map(|p| p.retention_days),
                "local": {"files": local.len(), "bytes": local.iter().map(|r| r.size).sum::<u64>(), "oldest": local.last().map(|r| r.start)},
                "archive": {"files": arch.len(), "bytes": arch.iter().map(|r| r.size).sum::<u64>(), "oldest": arch.last().map(|r| r.start), "newest": arch.first().map(|r| r.end)},
                "waiting": {"files": waiting.len(), "bytes": waiting.iter().map(|r| r.size).sum::<u64>()},
            })
        })
        .collect();
    let free = if reachable { fs4::available_space(&a.dir).ok() } else { None };
    json!({"enabled": a.enabled, "reachable": reachable, "free_bytes": free, "status": STATUS.locked().clone(), "cameras": cams})
}

/// Tell `storage` where each camera's older footage is.
pub fn sync_registry(cfg: &Config) {
    for cam in &cfg.cameras {
        storage::set_archive(&cfg.camera_dir(cam), cfg.archive_for(cam).map(|p| p.dest));
    }
}

pub async fn run(mgr: Arc<Manager>) {
    let mut fails: u32 = 0;
    let mut next_try = tokio::time::Instant::now();
    let mut last_retention = std::time::Instant::now() - Duration::from_secs(3600);
    // Playing footage that moved needs to know where it went, from the start.
    sync_registry(&mgr.config());
    // A first look soon after start-up.
    tokio::time::sleep(Duration::from_secs(20)).await;
    loop {
        let forced = tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(60)) => false,
            _ = RUN_NOW.notified() => true,
        };
        let cfg = mgr.config();
        sync_registry(&cfg);
        if !cfg.storage.archive.enabled || cfg.storage.archive.dir.as_os_str().is_empty() {
            fails = 0;
            continue;
        }
        if forced {
            fails = 0;
            next_try = tokio::time::Instant::now();
        }
        if tokio::time::Instant::now() < next_try {
            continue;
        }
        let a = cfg.storage.archive.clone();
        crate::cloud::connect_share_to(&a.dir.to_string_lossy(), &a.user, &a.password).await;
        if let Err(e) = std::fs::create_dir_all(&a.dir) {
            let msg = format!("can't use {}: {e}", a.dir.display());
            STATUS.locked().last_error = Some(msg.clone());
            fails += 1;
            next_try = tokio::time::Instant::now() + Duration::from_secs((60u64 << (fails - 1).min(5)).min(1800));
            warn!("deep storage: {msg}");
            continue;
        }
        STATUS.locked().running = true;
        let urgent = storage::take_urgent_move();
        'cams: for cam in &cfg.cameras {
            let Some(plan) = cfg.archive_for(cam) else { continue };
            let dir = cfg.camera_dir(cam);
            // The local disk short of room (or the clean-up asked to hurry): move the oldest files now, whatever
            // their age, and stop once there is room again.
            let hurry = urgent || storage::room_needed(&cfg, &dir).is_some_and(|(free, reserve, _)| free < reserve);
            let todo = if hurry { due_now(&dir, Local::now()) } else { due(&dir, &plan, Local::now()) };
            if todo.is_empty() {
                continue;
            }
            let total: u64 = todo.iter().map(|r| r.size).sum();
            let job = crate::jobs::start("archive", format!("Moving to deep storage — {}", cam.name));
            job.total(todo.len() as u64);
            job.transfer(0, total, 0);
            let began = std::time::Instant::now();
            let mut done_bytes = 0u64;
            let n = todo.len();
            // Oldest first.
            for (i, r) in todo.into_iter().rev().enumerate() {
                job.pause_point().await;
                job.step(format!("{} (file {} of {n})", r.name, i + 1));
                let (d2, p2, r2) = (dir.clone(), plan.clone(), r.clone());
                let res = tokio::task::spawn_blocking(move || move_one(&d2, &p2, &r2)).await.map_err(anyhow::Error::from).and_then(|x| x);
                match res {
                    Ok(()) => {
                        fails = 0;
                        done_bytes += r.size;
                        job.done_one();
                        let secs = began.elapsed().as_secs_f64().max(0.001);
                        job.transfer(done_bytes, total, (done_bytes as f64 / secs) as u64);
                        let mut st = STATUS.locked();
                        st.moved_files += 1;
                        st.moved_bytes += r.size;
                        st.last_error = None;
                        drop(st);
                        // Enough room again: the rest moves when it is due.
                        if hurry && storage::room_needed(&cfg, &dir).is_some_and(|(free, _, target)| free >= target) {
                            info!("[{}] deep storage: the local disk has room again", cam.name);
                            break;
                        }
                    }
                    Err(e) => {
                        let msg = format!("{e:#}");
                        warn!("[{}] deep storage: {msg}", cam.name);
                        job.fail(&msg);
                        STATUS.locked().last_error = Some(msg);
                        fails += 1;
                        next_try = tokio::time::Instant::now() + Duration::from_secs((60u64 << (fails - 1).min(5)).min(1800));
                        break 'cams;
                    }
                }
            }
        }
        if last_retention.elapsed() > Duration::from_secs(600) || forced {
            last_retention = std::time::Instant::now();
            let cfg2 = cfg.clone();
            if let Ok(n) = tokio::task::spawn_blocking(move || retention(&cfg2)).await {
                STATUS.locked().deleted_files += n;
            }
        }
        let mut st = STATUS.locked();
        st.running = false;
        st.last_run = Some(Local::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previews_go_with_the_recording_and_are_complete_when_they_arrive() {
        let base = std::env::temp_dir().join(format!("nvr-arch-prev-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (local, deep) = (base.join("local"), base.join("deep"));
        std::fs::create_dir_all(local.join(".thumbs")).unwrap();
        std::fs::create_dir_all(&deep).unwrap();
        let name = "cam_2026-11-19_10-00-00.mp4";
        for f in [
            format!("{name}.sprite-000.jpg"),
            format!("{name}.sprite-001.jpg"),
            format!("{name}.sprite.json"),
            format!("{name}.jpg"),
            "other_2026-11-19_10-00-00.mp4.sprite.json".to_string(),
        ] {
            std::fs::write(local.join(".thumbs").join(f), b"x").unwrap();
        }
        assert_eq!(copy_previews(&local, &deep, name), 4, "this recording's four files, not the other one's");
        let got: Vec<String> = std::fs::read_dir(deep.join(".thumbs")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        assert_eq!(got.len(), 4, "{got:?}");
        assert!(got.iter().all(|f| !f.ends_with(".part")), "nothing half-copied is left: {got:?}");
        assert_eq!(copy_previews(&local, &deep, "nothing_2026-11-19_10-00-00.mp4"), 0);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn old_files_move_new_stay_and_retention_deletes() {
        let base = std::env::temp_dir().join(format!("gaze-arch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (local, deep) = (base.join("local").join("Cam"), base.join("deep").join("Cam"));
        std::fs::create_dir_all(&local).unwrap();
        let now = Local::now();
        let mk = |ago_h: i64, content: &[u8]| {
            let t = now - CDuration::hours(ago_h);
            let name = format!("Cam_{}.mp4", t.format("%Y-%m-%d_%H-%M-%S"));
            let p = local.join(&name);
            std::fs::write(&p, content).unwrap();
            let f = std::fs::File::options().write(true).open(&p).unwrap();
            f.set_modified(std::time::SystemTime::from(t)).unwrap();
            name
        };
        let newest = mk(1, b"new");
        let day_old = mk(30, b"thirty hours");
        let two_days = mk(50, b"fifty hours");
        let plan = ArchivePlan { after_hours: 24, retention_days: 2, dest: deep.clone() };
        storage::list(&local); // (cached lists are read afresh below)
        let due_now = due(&local, &plan, now);
        let names: Vec<&str> = due_now.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&day_old.as_str()) && names.contains(&two_days.as_str()) && !names.contains(&newest.as_str()), "{names:?}");
        for r in &due_now {
            move_one(&local, &plan, r).unwrap();
        }
        assert!(local.join(&newest).exists());
        assert!(!local.join(&day_old).exists() && deep.join(&day_old).exists());
        assert_eq!(std::fs::read(deep.join(&two_days)).unwrap(), b"fifty hours");
        // Moved footage is still found: listed, and read from deep storage.
        assert!(storage::resolve(&local, &day_old).is_err());
        storage::set_archive(&local, Some(deep.clone()));
        assert_eq!(storage::resolve(&local, &day_old).unwrap(), deep.join(&day_old));
        assert_eq!(storage::resolve(&local, &newest).unwrap(), local.join(&newest));
        let names: Vec<String> = storage::list_archived(&local).into_iter().map(|r| r.name).collect();
        assert!(names.contains(&day_old) && names.contains(&two_days) && !names.contains(&newest), "{names:?}");
        storage::set_archive(&local, None);
        let _ = std::fs::remove_dir_all(&base);
    }
}
