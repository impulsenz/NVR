//! A small home NVR: records one IP camera to disk and serves a web interface.

mod config;
mod live;
mod recorder;
mod storage;
mod web;

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_target(false).init();

    let path = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "nvr.toml".into()));
    if !path.exists() {
        std::fs::write(&path, config::EXAMPLE_CONFIG)
            .with_context(|| format!("writing example config to {}", path.display()))?;
        eprintln!(
            "Created an example config at {}.\nEdit it with your camera's RTSP URL, then run nvr again.",
            path.display()
        );
        return Ok(());
    }
    let cfg = Arc::new(config::Config::load(&path)?);
    std::fs::create_dir_all(&cfg.recording.dir)
        .with_context(|| format!("creating {}", cfg.recording.dir.display()))?;
    check_ffmpeg(&cfg.server.ffmpeg_path).await?;

    info!(
        "camera \"{}\" at {}",
        cfg.camera.name,
        config::mask_url(&cfg.camera.url)
    );
    info!("recordings in {}", cfg.recording.dir.display());

    let recorder = recorder::Recorder::new(cfg.clone());
    tokio::spawn(recorder.clone().run());
    tokio::spawn(storage::retention_task(cfg.clone()));

    let state = web::AppState {
        cfg: cfg.clone(),
        recorder: recorder.clone(),
        live: live::Live::new(cfg.clone()),
    };
    let listener = tokio::net::TcpListener::bind(&cfg.server.listen)
        .await
        .with_context(|| format!("binding {}", cfg.server.listen))?;
    info!("web interface on http://{}", cfg.server.listen);

    tokio::select! {
        res = axum::serve(listener, web::router(state)) => {
            if let Err(e) = res { error!("server error: {e}"); }
        }
        _ = shutdown_signal() => info!("shutting down"),
    }
    recorder.shutdown().await;
    Ok(())
}

async fn check_ffmpeg(path: &str) -> Result<()> {
    let out = tokio::process::Command::new(path)
        .arg("-version")
        .output()
        .await
        .with_context(|| {
            format!("ffmpeg not found at \"{path}\" — install ffmpeg or set server.ffmpeg_path")
        })?;
    let first = String::from_utf8_lossy(&out.stdout);
    info!("{}", first.lines().next().unwrap_or("ffmpeg"));
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = ctrl_c.await;
}
