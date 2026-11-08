//! A small home NVR: records IP cameras to disk and serves a web interface.

mod ai;
mod buffer;
mod cloud;
mod config;
mod detect;
mod events;
mod ffmpeg;
mod fsbrowse;
mod imgutil;
mod library;
mod live;
mod logs;
mod manager;
mod onvif;
mod recorder;
mod relay;
mod storage;
mod sysmon;
mod web;

use anyhow::{Context, Result};
use std::path::PathBuf;
use tracing::{error, info, warn};

#[tokio::main]
async fn main() -> Result<()> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_ansi(use_color()),
        )
        .with(tracing_subscriber::filter::LevelFilter::INFO)
        .with(logs::LogLayer)
        .init();
    // HTTPS (cloud AI, model download) uses rustls with the ring crypto backend.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let path = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "nvr.toml".into()));
    let (cfg, needs_save) = config::Config::load_or_create(&path)?;
    // Logs, statistics and AI models live next to the config file.
    let data_dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."))
        .join("nvr-data");
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("creating {}", data_dir.display()))?;
    logs::buffer().set_file(data_dir.join("nvr.log"));
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
    let mgr = manager::Manager::new(path, data_dir, cfg);
    tokio::spawn(mgr.clone().housekeeping());
    tokio::spawn(mgr.monitor.clone().run());
    tokio::spawn(cloud::run(mgr.clone()));

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
