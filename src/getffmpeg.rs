//! ffmpeg comes with GAZE on Windows: when it isn't installed, GAZE starts
//! anyway and downloads it (a build with NVIDIA, Intel, AMD and DirectX
//! video support: github.com/BtbN/FFmpeg-Builds, GPL), puts it next to
//! itself, and restarts with it. An ffmpeg already there is used at once.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[cfg_attr(not(windows), allow(dead_code))]
const BUILD: &str = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip";

/// ffmpeg is missing (and, on Windows, being fetched).
pub static MISSING: AtomicBool = AtomicBool::new(false);

/// Where GAZE keeps its own ffmpeg: a folder next to the program (or in
/// its data folder when that isn't writable).
pub fn own_dir(data_dir: &Path) -> PathBuf {
    let exe_dir = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf));
    match exe_dir {
        Some(d) if writable(&d) => d.join("ffmpeg"),
        _ => data_dir.join("ffmpeg"),
    }
}

fn writable(d: &Path) -> bool {
    let t = d.join(".gaze-write-test");
    let ok = std::fs::write(&t, b"x").is_ok();
    let _ = std::fs::remove_file(&t);
    ok
}

/// GAZE's own ffmpeg, if it's there.
pub fn own_ffmpeg(data_dir: &Path) -> Option<PathBuf> {
    let name = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    [own_dir(data_dir), data_dir.join("ffmpeg")].into_iter().map(|d| d.join(name)).find(|p| p.exists())
}

/// Download ffmpeg into GAZE's own folder; returns the path of ffmpeg.exe.
#[cfg_attr(not(windows), allow(dead_code))]
pub async fn fetch(data_dir: &Path) -> Result<PathBuf> {
    if !cfg!(windows) {
        bail!("install ffmpeg with your system's package manager");
    }
    let dir = own_dir(data_dir);
    std::fs::create_dir_all(&dir)?;
    let zip = dir.join("ffmpeg-download.zip");
    crate::jobs::download("ffmpeg (needed to record and play video)", BUILD, &zip, 20_000_000).await?;
    let (z2, d2) = (zip.clone(), dir.clone());
    tokio::task::spawn_blocking(move || -> Result<()> {
        let mut a = zip::ZipArchive::new(std::fs::File::open(&z2)?).context("reading the ffmpeg download")?;
        let mut got = 0;
        for i in 0..a.len() {
            let mut f = a.by_index(i)?;
            let name = f.name()?.to_string();
            let base = name.rsplit('/').next().unwrap_or("").to_string();
            if name.contains("/bin/") && (base == "ffmpeg.exe" || base == "ffprobe.exe") {
                let out = d2.join(&base);
                let tmp = out.with_extension("part");
                std::io::copy(&mut f, &mut std::fs::File::create(&tmp)?)?;
                std::fs::rename(&tmp, &out)?;
                got += 1;
            }
        }
        anyhow::ensure!(got >= 1, "ffmpeg.exe wasn't in the download");
        Ok(())
    })
    .await??;
    let _ = std::fs::remove_file(&zip);
    Ok(dir.join("ffmpeg.exe"))
}

/// Started when ffmpeg is missing: fetch it, use it, restart.
#[cfg_attr(not(windows), allow(dead_code))]
pub async fn fetch_and_use(m: Arc<crate::manager::Manager>) {
    tracing::warn!("ffmpeg is missing: downloading it (about 200 MB)");
    match fetch(&m.data_dir).await {
        Ok(p) => {
            let p = std::path::absolute(&p).unwrap_or(p);
            tracing::info!("ffmpeg downloaded to {}: restarting to use it", p.display());
            if let Err(e) = m.set_ffmpeg_path(&p.to_string_lossy()) {
                tracing::warn!("couldn't save the ffmpeg path: {e:#}");
                return;
            }
            MISSING.store(false, Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            crate::update::restart_now();
        }
        Err(e) => tracing::error!("couldn't download ffmpeg: {e:#} — install it from ffmpeg.org and set its path in Settings → System"),
    }
}
