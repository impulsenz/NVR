//! A small home NVR: records IP cameras to disk and serves a web interface.
// Windows: no console window (GAZE lives in the system tray instead).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod ai;
mod buffer;
mod cloud;
mod config;
mod detect;
mod discover;
mod draw;
mod events;
mod ffmpeg;
mod finalize;
mod fsbrowse;
mod ftp;
mod health;
mod hooks;
mod hls;
mod identify;
mod imgutil;
mod inbox;
mod infer;
mod launch;
mod jobs;
mod known;
mod library;
mod live;
mod load;
mod logs;
mod manager;
mod mp4fix;
mod mqtt;
mod notify;
mod onvif;
mod picturelog;
mod piper;
mod onvif_device;
mod onvif_events;
mod siren;
mod sound;
mod clip;
mod overview;
mod ptz_auto;
mod recog;
mod recorder;
mod relay;
mod remote;
mod rtmp;
mod seen;
mod stationary;
mod storage;
mod sysmon;
mod talk;
mod talkmix;
mod tapo;
mod tls;
mod tray;
mod tts;
mod update;
mod users;
mod web;
mod winservice;
mod zones;

use anyhow::{Context, Result};

use tracing::{error, info, warn};

/// Set by the tray menu or the service manager: stop.
static QUIT: tokio::sync::Notify = tokio::sync::Notify::const_new();
/// What to do after stopping (a tray::Action).
static AFTER: std::sync::Mutex<Option<tray::Action>> = std::sync::Mutex::new(None);
/// This program, as it was started (an update renames the running file).
static EXE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

fn exe() -> std::path::PathBuf {
    EXE.get()
        .cloned()
        .unwrap_or_else(|| std::env::current_exe().unwrap_or_default())
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?)
}

fn main() -> Result<()> {
    #[cfg(windows)]
    use winservice::Mode;
    let _ = EXE.set(std::env::current_exe()?);
    // An update GAZE wasn't allowed to put in place itself, with an
    // administrator's permission: `--swap-update <new> <program>`.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--swap-update") && args.len() >= 4 {
        let ok = update::swap_in(std::path::Path::new(&args[2]), std::path::Path::new(&args[3]));
        std::process::exit(if ok.is_ok() { 0 } else { 1 });
    }
    let (mode, path, dir) = winservice::parse_args();
    if let Some(d) = &dir {
        let _ = std::env::set_current_dir(d);
    } else {
        // Started by Windows (at sign-in, from a shortcut): the working
        // folder is then e.g. C:\Windows\System32, where "recordings" and
        // the other relative paths can't go. Work in the settings' folder.
        let exe_dir = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf()));
        let base = if path.is_absolute() {
            path.parent().map(|p| p.to_path_buf())
        } else if !path.exists() {
            exe_dir.filter(|d| d.join(&path).exists())
        } else {
            None
        };
        if let Some(b) = base {
            let _ = std::env::set_current_dir(b);
        }
    }
    #[cfg(windows)]
    match mode {
        Mode::Install => {
            let listen = config::Config::load_or_create(&path)
                .map(|(c, _)| c.server.listen)
                .unwrap_or_default();
            let dir = dir.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            std::process::exit(winservice::install(&path, &dir, &listen));
        }
        Mode::Uninstall => std::process::exit(winservice::uninstall()),
        Mode::RestartService => std::process::exit(winservice::restart()),
        Mode::Service => {
            // After an update: the new program restarts the service.
            update::set_restart(|| {
                let _ = std::process::Command::new(exe())
                    .arg("--restart-service")
                    .spawn();
            });
            let p = path.clone();
            let ok = winservice::run_service(
                move || {
                    if let Ok(rt) = runtime() {
                        let _ = rt.block_on(run(&p, false));
                    }
                },
                || QUIT.notify_one(),
            );
            if !ok {
                eprintln!(
                    "--service is used by the Windows service manager; start nvr.exe without it."
                );
            }
            return Ok(());
        }
        Mode::Companion | Mode::Normal => {
            if mode == Mode::Normal {
                // Already running (double-clicked again, or the shortcut):
                // open it instead of complaining that the port is taken.
                // Started by Windows at sign-in (--dir): nothing to do.
                let port = launch::port_of(&path);
                if launch::running(port) {
                    if dir.is_none() {
                        launch::open_ui(port, &std::path::absolute("nvr-data").unwrap_or_default());
                    }
                    return Ok(());
                }
            }
            // GAZE runs as a service (now, or about to): just the icon.
            if mode == Mode::Companion || winservice::installed() {
                return companion(&path, mode == Mode::Companion);
            }
            // The program is GAZE.exe (older downloads were called
            // nvr-windows-x64.exe): a copy under that name takes over.
            // A fresh download installs itself in its own program folder.
            if launch::install_if_downloaded(dir.is_some() || path != std::path::Path::new("nvr.toml")) {
                return Ok(());
            }
            if update::become_gaze_exe() {
                return Ok(());
            }
            if args.iter().any(|a| a == "--open") {
                launch::OPEN_WHEN_READY.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
    }
    #[cfg(not(windows))]
    let _ = mode;
    tray::attach_console();
    update::set_restart(|| {
        *AFTER.lock().unwrap() = Some(tray::Action::Restart);
        QUIT.notify_one();
    });
    let res = runtime()?.block_on(run(&path, true));
    if let Err(e) = &res {
        error!("{e:#}");
        tray::fatal(&format!("{e:#}"));
    }
    tray::remove();
    let exe = exe();
    match *AFTER.lock().unwrap() {
        Some(tray::Action::Restart) if res.is_ok() => {
            // Unix: replace this process (same process for systemd & co).
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                let e = std::process::Command::new(&exe)
                    .args(std::env::args_os().skip(1))
                    .exec();
                return Err(e.into());
            }
            #[cfg(not(unix))]
            std::process::Command::new(exe)
                .args(std::env::args_os().skip(1).filter(|a| a != "--open"))
                .spawn()?;
        }
        Some(tray::Action::BecomeService) => {
            // The service is taking over; the icon stays for this user.
            std::process::Command::new(exe)
                .arg("--companion")
                .arg(&path)
                .spawn()?;
        }
        _ => {}
    }
    res
}

/// Windows, when GAZE runs as a service: only the tray icon for the
/// signed-in user (open GAZE, restart, turn the service off).
#[cfg(windows)]
fn companion(path: &std::path::Path, just_installed: bool) -> Result<()> {
    // Right after "Run as a Windows service": wait for it to appear.
    let t = std::time::Instant::now();
    while just_installed
        && !winservice::installed()
        && t.elapsed() < std::time::Duration::from_secs(180)
    {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    if !winservice::installed() {
        return Ok(());
    }
    let (cfg, _) = config::Config::load_or_create(path)?;
    let base = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    let port = cfg
        .server
        .listen
        .rsplit(':')
        .next()
        .unwrap_or("8080")
        .to_string();
    tray::start(
        tray::TrayInfo {
            url: format!("http://localhost:{port}/"),
            recordings: std::path::absolute(&cfg.storage.dir).unwrap_or(cfg.storage.dir.clone()),
            log: std::path::absolute(base.join("nvr-data").join("nvr.log")).unwrap_or_default(),
            config: path.to_path_buf(),
            companion: true,
        },
        Box::new(|_| QUIT.notify_one()),
    );
    runtime()?.block_on(QUIT.notified());
    tray::remove();
    Ok(())
}

async fn run(path: &std::path::Path, with_tray: bool) -> Result<()> {
    let path = path.to_path_buf();
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

    // A settings file that can't be read: start in safe mode (nothing is
    // saved over it) so it can be fixed from the web page; a good one is
    // kept as the last good copy.
    let (mut cfg, needs_save) = match config::Config::load_or_create(&path) {
        Ok(x) => {
            if path.exists() {
                let _ = std::fs::copy(&path, path.with_extension("toml.good"));
            }
            x
        }
        Err(e) => {
            let problem = format!("{e:#}");
            error!("the settings file can't be used: {problem} — starting in safe mode (fix it on the web page)");
            config::Config::set_safe_mode(problem);
            (config::Config::rescue(&std::fs::read_to_string(&path).unwrap_or_default()), false)
        }
    };
    config::private_file(&path);
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
        // Video on the graphics card where possible (Settings → GPU
        // acceleration → Choose automatically).
        if cfg.hardware.auto {
            let hw = ffmpeg::auto_hardware(&cfg.server.ffmpeg_path, &caps, &cfg.hardware).await;
            info!(
                "video: decoding on {}, converting with {}",
                if hw.hwaccel == "none" {
                    "the processor (no GPU decoder works)".to_string()
                } else {
                    format!("the GPU ({})", hw.hwaccel)
                },
                hw.encoder
            );
            if hw != cfg.hardware {
                cfg.hardware = hw;
                cfg.save(&path)?;
            }
        }
    }
    info!(
        "{} camera(s); recordings in {}",
        cfg.cameras.len(),
        cfg.storage.dir.display()
    );

    let listen = cfg.server.listen.clone();
    let legacy_login = !cfg.server.password.is_empty();
    let mgr = manager::Manager::new(path.clone(), data_dir, cfg);
    if legacy_login {
        // The old login is now an account: take its password out of the file.
        mgr.config().save(&path)?;
    }
    remove_smart_search_files(&mgr);
    tokio::spawn(mgr.clone().housekeeping());
    tokio::spawn(mgr.monitor.clone().run());
    tokio::spawn(cloud::run(mgr.clone()));
    tokio::spawn(web::prebuild_timelapse(mgr.clone()));
    tokio::spawn(finalize::run(mgr.clone()));
    tokio::spawn(discover::daily(mgr.clone()));
    tokio::spawn(identify::run(mgr.clone()));
    tokio::spawn(talkmix::run(mgr.clone()));
    tokio::spawn(notify::run(mgr.clone()));
    tokio::spawn(health::run(mgr.clone()));
    tokio::spawn(mqtt::run(mgr.clone()));
    tokio::spawn(hooks::run(mgr.clone()));
    tokio::spawn(onvif_events::run(mgr.clone()));
    piper::init(&mgr.data_dir);
    tokio::spawn(piper::keep_ready(mgr.clone()));
    tokio::spawn(sound::run(mgr.clone()));
    tokio::spawn(clip::run(mgr.clone()));
    tokio::spawn(ptz_auto::run(mgr.clone()));
    tokio::spawn(rtmp::run(mgr.clone()));
    tokio::spawn(update::run(mgr.clone()));
    tokio::spawn(infer::run(mgr.clone()));

    let https_port = mgr.config().server.https_port;
    if https_port > 0 {
        let bind = listen.rsplit_once(':').map(|(h, _)| h.to_string()).unwrap_or_else(|| "0.0.0.0".into());
        tokio::spawn(tls::serve(mgr.clone(), bind, https_port));
    }
    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .with_context(|| format!("can't use {listen} for the web interface — is GAZE already running? (change [server] listen in the settings file to use another port)"))?;
    info!("web interface on http://{listen}");
    let port = listen.rsplit(':').next().unwrap_or("8080");
    let cfg = mgr.config();
    if with_tray {
        tray::start(
            tray::TrayInfo {
                url: format!("http://localhost:{port}/"),
                recordings: std::path::absolute(&cfg.storage.dir)
                    .unwrap_or(cfg.storage.dir.clone()),
                log: std::path::absolute(mgr.data_dir.join("nvr.log")).unwrap_or_default(),
                config: mgr.cfg_path.clone(),
                companion: false,
            },
            Box::new(|action| {
                *AFTER.lock().unwrap() = Some(action);
                QUIT.notify_one();
            }),
        );
    }
    #[cfg(windows)]
    if with_tray {
        launch::make_shortcuts(&std::path::absolute(&mgr.data_dir).unwrap_or(mgr.data_dir.clone()));
    }
    // Started from the shortcut: open GAZE now it's ready.
    if launch::OPEN_WHEN_READY.swap(false, std::sync::atomic::Ordering::SeqCst) {
        let (p, d): (u16, std::path::PathBuf) = (port.parse().unwrap_or(8080), mgr.data_dir.clone());
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(800));
            launch::open_ui(p, &std::path::absolute(&d).unwrap_or(d.clone()));
        });
    }

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

/// Smart search (descriptions) was removed in 0.5.7: delete its models
/// (600 MB to 1.7 GB) and search indexes.
fn remove_smart_search_files(mgr: &manager::Manager) {
    let mut freed = 0u64;
    let mut remove = |p: std::path::PathBuf| {
        let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        if std::fs::remove_file(&p).is_ok() {
            freed += size;
        }
    };
    if let Ok(rd) = std::fs::read_dir(mgr.data_dir.join("models")) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with("clip-") {
                remove(e.path());
            }
        }
    }
    for c in mgr.cameras() {
        if let Ok(rd) = std::fs::read_dir(c.events.dir()) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if n.starts_with("smart") && n.ends_with(".jsonl") {
                    remove(e.path());
                }
            }
        }
    }
    if freed > 0 {
        tracing::info!(
            "removed the old smart search files ({} MB freed)",
            freed / 1_048_576
        );
    }
}
