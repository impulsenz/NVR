//! A small home NVR: records IP cameras to disk and serves a web interface.

mod config;
mod ffmpeg;
mod live;
mod manager;
mod recorder;
mod storage;
mod web;

use anyhow::{Context, Result};
use std::path::PathBuf;
use tracing::{error, info, warn};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_ansi(use_color())
        .init();

    let path = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "nvr.toml".into()));
    let (cfg, needs_save) = config::Config::load_or_create(&path)?;
    if needs_save {
        cfg.save(&path)?;
        info!("wrote {}", path.display());
    }
    std::fs::create_dir_all(&cfg.storage.dir)
        .with_context(|| format!("creating {}", cfg.storage.dir.display()))?;
    let ids: Vec<String> = cfg.cameras.iter().map(|c| c.id.clone()).collect();
    storage::migrate_flat_files(&cfg.storage.dir, &ids);

    let caps = ffmpeg::capabilities(&cfg.server.ffmpeg_path).await;
    if caps.ffmpeg_version.is_empty() {
        warn!(
            "ffmpeg not found at \"{}\" — install ffmpeg or set the path in Settings",
            cfg.server.ffmpeg_path
        );
    } else {
        info!("{}", caps.ffmpeg_version);
    }
    info!(
        "{} camera(s); recordings in {}",
        cfg.cameras.len(),
        cfg.storage.dir.display()
    );

    let listen = cfg.server.listen.clone();
    let mgr = manager::Manager::new(path, cfg);
    tokio::spawn(mgr.clone().retention_task());

    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .with_context(|| format!("binding {listen}"))?;
    info!("web interface on http://{listen}");

    tokio::select! {
        res = axum::serve(listener, web::router(mgr.clone())) => {
            if let Err(e) = res { error!("server error: {e}"); }
        }
        _ = shutdown_signal() => info!("shutting down"),
    }
    mgr.shutdown().await;
    Ok(())
}

/// Colored log output only on a terminal that can show it. The Windows console
/// needs ANSI support switched on first, otherwise it prints codes like `←[2m`.
fn use_color() -> bool {
    use std::io::IsTerminal;
    if std::env::var_os("NO_COLOR").is_some() || !std::io::stdout().is_terminal() {
        return false;
    }
    #[cfg(windows)]
    return nu_ansi_term::enable_ansi_support().is_ok();
    #[cfg(not(windows))]
    true
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
