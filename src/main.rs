//! A small home NVR: records IP cameras to disk and serves a web interface.
// Windows: no console window (GAZE lives in the system tray instead).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod ai;
mod buffer;
mod clip;
mod cloud;
mod config;
mod detect;
mod discover;
mod events;
mod ffmpeg;
mod finalize;
mod fsbrowse;
mod identify;
mod imgutil;
mod jobs;
mod known;
mod library;
mod live;
mod load;
mod logs;
mod manager;
mod mp4fix;
mod onvif;
mod onvif_device;
mod recog;
mod recorder;
mod relay;
mod seen;
mod smart;
mod storage;
mod sysmon;
mod talk;
mod tray;
mod users;
mod web;
mod zones;

use anyhow::{Context, Result};
use std::path::PathBuf;
use tracing::{error, info, warn};

/// Set by the tray menu: stop (and maybe start again).
static QUIT: tokio::sync::Notify = tokio::sync::Notify::const_new();
static RESTART: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[tokio::main]
async fn main() -> Result<()> {
    tray::attach_console();
    let res = run().await;
    if let Err(e) = &res {
        error!("{e:#}");
        tray::fatal(&format!("{e:#}"));
    }
    tray::remove();
    if res.is_ok() && RESTART.load(std::sync::atomic::Ordering::SeqCst) {
        let exe = std::env::current_exe()?;
        std::process::Command::new(exe)
            .args(std::env::args_os().skip(1))
            .spawn()?;
    }
    res
}

async fn run() -> Result<()> {
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
    // After a crash or power cut: cut incomplete data off the end of the
    // newest recordings so every file plays.
    let mut fixed = 0;
    for cam in &cfg.cameras {
        fixed += mp4fix::repair_dir(&cfg.camera_dir(cam), 72);
    }
    if fixed > 0 {
        warn!("repaired {fixed} recording(s) left incomplete by an unexpected stop");
    }

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
    tokio::spawn(web::prebuild_timelapse(mgr.clone()));
    tokio::spawn(smart::run(mgr.clone()));
    tokio::spawn(finalize::run(mgr.clone()));
    tokio::spawn(discover::daily(mgr.clone()));
    tokio::spawn(identify::run(mgr.clone()));

    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .with_context(|| format!("can't use {listen} for the web interface — is GAZE already running? (change [server] listen in the settings file to use another port)"))?;
    info!("web interface on http://{listen}");
    let port = listen.rsplit(':').next().unwrap_or("8080");
    let cfg = mgr.config();
    tray::start(
        tray::TrayInfo {
            url: format!("http://localhost:{port}/"),
            recordings: std::path::absolute(&cfg.storage.dir).unwrap_or(cfg.storage.dir.clone()),
            log: std::path::absolute(mgr.data_dir.join("nvr.log")).unwrap_or_default(),
            config: mgr.cfg_path.clone(),
        },
        Box::new(|restart| {
            RESTART.store(restart, std::sync::atomic::Ordering::SeqCst);
            QUIT.notify_one();
        }),
    );

    tokio::select! {
        res = axum::serve(listener, web::router(mgr.clone()).into_make_service_with_connect_info::<std::net::SocketAddr>()) => {
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
    tokio::select! {
        _ = os_signal() => {}
        _ = QUIT.notified() => {}
    }
}

async fn os_signal() {
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
