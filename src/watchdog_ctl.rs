//! The optional GAZE Watchdog (`GAZE-Watchdog.exe`, its own program):
//! installed, set up, watched and updated from Settings → Watchdog.
//!
//! GAZE and the watchdog only talk through files in `<data>/watchdog/`:
//! GAZE writes `settings.toml` and one-word command files in `commands/`;
//! the watchdog writes `status.json` and `watchdog.log`, and its program
//! lives there too. GAZE leaves a `gaze-quit` marker when it's closed on
//! purpose, so the watchdog doesn't open it again against your wish.

use crate::manager::Manager;
use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const ASSET: &str = "GAZE-Watchdog.exe";

/// The settings the watchdog understands (anything else is dropped).
const KEYS: &[&str] = &[
    "enabled",
    "gaze_exe",
    "gaze_args",
    "launch",
    "service_name",
    "check_url",
    "auto_url",
    "check_interval_s",
    "timeout_s",
    "failures_before_restart",
    "startup_grace_s",
    "restart_delay_s",
    "leave_closed_on_quit",
    "max_restarts",
    "within_minutes",
    "cooldown_minutes",
    "kill_timeout_s",
    "auto_update",
    "update_check_hours",
];

pub fn dir(data: &Path) -> PathBuf {
    data.join("watchdog")
}

pub fn exe_path(data: &Path) -> PathBuf {
    dir(data).join(ASSET)
}

/// GAZE was closed on purpose (Quit): the watchdog leaves it closed.
pub fn mark_quit(data: &Path) {
    if exe_path(data).exists() {
        let _ = std::fs::create_dir_all(dir(data));
        let _ = std::fs::write(dir(data).join("gaze-quit"), chrono::Local::now().to_rfc3339());
    }
}

/// GAZE started: no longer "quit".
pub fn clear_quit(data: &Path) {
    let _ = std::fs::remove_file(dir(data).join("gaze-quit"));
}

pub fn installed(data: &Path) -> bool {
    exe_path(data).exists()
}

fn status(data: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(dir(data).join("status.json")).ok()?).ok()
}

/// Is the watchdog running? (its status was written in the last 20 seconds)
fn alive(st: &Option<Value>) -> bool {
    st.as_ref().and_then(|s| s["updated"].as_i64()).is_some_and(|t| chrono::Utc::now().timestamp_millis() - t < 20_000)
}

/// The web port GAZE listens on, from "0.0.0.0:8080".
fn port(listen: &str) -> u16 {
    listen.rsplit(':').next().and_then(|p| p.trim().parse().ok()).unwrap_or(8080)
}

/// What the watchdog checks: GAZE's own page on this computer.
pub fn check_url(listen: &str) -> String {
    format!("http://127.0.0.1:{}/api/auth/state", port(listen))
}

/// Settings for a new install (or what's missing from the saved ones).
fn defaults(mgr: &Manager) -> Map<String, Value> {
    let cfg = mgr.config();
    let exe = std::env::current_exe().unwrap_or_default();
    let config = std::path::absolute(&mgr.cfg_path).unwrap_or(mgr.cfg_path.clone());
    let cwd = std::env::current_dir().unwrap_or_default();
    let mut m = Map::new();
    m.insert("enabled".into(), json!(true));
    m.insert("gaze_exe".into(), json!(exe.to_string_lossy()));
    m.insert("gaze_args".into(), json!([config.to_string_lossy(), "--dir", cwd.to_string_lossy()]));
    m.insert("launch".into(), json!("auto"));
    m.insert("service_name".into(), json!("GAZE"));
    m.insert("check_url".into(), json!(check_url(&cfg.server.listen)));
    m.insert("auto_url".into(), json!(true));
    for (k, v) in [
        ("check_interval_s", 10),
        ("timeout_s", 8),
        ("failures_before_restart", 3),
        ("startup_grace_s", 120),
        ("restart_delay_s", 10),
        ("max_restarts", 5),
        ("within_minutes", 15),
        ("cooldown_minutes", 10),
        ("kill_timeout_s", 20),
        ("update_check_hours", 12),
    ] {
        m.insert(k.into(), json!(v));
    }
    m.insert("leave_closed_on_quit".into(), json!(false));
    m.insert("auto_update".into(), json!(false));
    m
}

fn read_settings(data: &Path, mgr: &Manager) -> Map<String, Value> {
    let mut m = defaults(mgr);
    if let Ok(t) = std::fs::read_to_string(dir(data).join("settings.toml"))
        && let Ok(v) = toml::from_str::<toml::Table>(&t)
        && let Ok(Value::Object(o)) = serde_json::to_value(v)
    {
        for (k, val) in o {
            if KEYS.contains(&k.as_str()) {
                m.insert(k, val);
            }
        }
    }
    m
}

fn write_settings(data: &Path, m: &Map<String, Value>) -> Result<()> {
    std::fs::create_dir_all(dir(data))?;
    let table: toml::Table = serde_json::from_value(Value::Object(m.clone())).context("the settings can't be saved")?;
    let tmp = dir(data).join("settings.toml.tmp");
    std::fs::write(&tmp, toml::to_string_pretty(&table)?)?;
    std::fs::rename(&tmp, dir(data).join("settings.toml"))?;
    Ok(())
}

/// Save settings from the browser: only the known ones, with the right kinds of value.
pub fn save_settings(data: &Path, mgr: &Manager, input: &Value) -> Result<()> {
    let mut m = read_settings(data, mgr);
    let Some(o) = input.as_object() else { bail!("settings expected") };
    for (k, v) in o {
        if !KEYS.contains(&k.as_str()) {
            continue;
        }
        let want = &m[k];
        let ok = match want {
            Value::Bool(_) => v.is_boolean(),
            Value::Number(_) => v.as_u64().is_some(),
            Value::String(_) => v.is_string(),
            Value::Array(_) => v.as_array().is_some_and(|a| a.iter().all(Value::is_string)),
            _ => false,
        };
        if !ok {
            bail!("“{k}” has the wrong kind of value");
        }
        m.insert(k.clone(), v.clone());
    }
    write_settings(data, &m)
}

/// A command for the watchdog (`restart-gaze`, `check-update`, `update`, `stop`).
pub fn command(data: &Path, cmd: &str) -> Result<()> {
    if !["restart-gaze", "check-update", "update", "stop"].contains(&cmd) {
        bail!("unknown command");
    }
    let d = dir(data).join("commands");
    std::fs::create_dir_all(&d)?;
    std::fs::write(d.join(format!("{}-{cmd}", chrono::Local::now().format("%Y%m%d%H%M%S%3f"))), cmd)?;
    // Stopped by you: GAZE doesn't start it again by itself (until you start it).
    if cmd == "stop" {
        let _ = std::fs::write(dir(data).join("stopped-on-purpose"), chrono::Local::now().to_rfc3339());
        tracing::info!("watchdog: stopped on request (GAZE won't start it again until you do)");
    }
    Ok(())
}

fn log_tail(data: &Path, lines: usize) -> Vec<String> {
    let t = std::fs::read_to_string(dir(data).join("watchdog.log")).unwrap_or_default();
    let all: Vec<&str> = t.lines().collect();
    all[all.len().saturating_sub(lines)..].iter().map(|s| s.to_string()).collect()
}

/// "Don't ask me again": the page stops offering to install the watchdog.
pub fn dont_ask(data: &Path) {
    let _ = std::fs::write(data.join("watchdog-dont-ask"), chrono::Local::now().to_rfc3339());
    tracing::info!("watchdog: the offer to install it won't be shown again (Settings → Watchdog still installs it)");
}

/// What GAZE knows about keeping the watchdog running, for the title bar and the log.
#[derive(Default)]
struct Auto {
    /// When the watchdog was last seen running, and when it was first noticed missing.
    down_since: Option<std::time::Instant>,
    /// Attempts to start it since it went down, the last one, and why it failed.
    attempts: u32,
    last_try: Option<std::time::Instant>,
    error: Option<String>,
}
static AUTO: std::sync::Mutex<Auto> = std::sync::Mutex::new(Auto { down_since: None, attempts: 0, last_try: None, error: None });

/// For the title bar: is the watchdog installed, and running? (read from its status file at most every 2 s)
pub fn summary(data: &Path) -> Value {
    static CACHE: std::sync::Mutex<Option<(std::time::Instant, Value)>> = std::sync::Mutex::new(None);
    if let Some((at, v)) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
        && at.elapsed() < Duration::from_secs(2)
    {
        return v.clone();
    }
    let on_purpose = dir(data).join("stopped-on-purpose").exists();
    let installed = installed(data);
    let ask_install = cfg!(windows) && !installed && !data.join("watchdog-dont-ask").exists();
    let running = installed && alive(&status(data));
    let a = AUTO.lock().unwrap_or_else(|e| e.into_inner());
    let v = json!({
        "supported": cfg!(windows),
        "installed": installed,
        // The page offers to install it (a box with Install / No, and "don't ask me again").
        "ask_install": ask_install,
        "running": running,
        "stopped_on_purpose": on_purpose,
        "down_seconds": (!running).then(|| a.down_since.map(|t| t.elapsed().as_secs())).flatten(),
        "attempts": a.attempts,
        "error": a.error,
    });
    drop(a);
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((std::time::Instant::now(), v.clone()));
    v
}

/// Everything Settings → Watchdog shows.
pub fn overview(data: &Path, mgr: &Manager) -> Value {
    let st = status(data);
    let mut settings = read_settings(data, mgr);
    // (What a new install would use, for the "reset" button.)
    let defaults = defaults(mgr);
    if !installed(data) {
        settings = defaults.clone();
    }
    json!({
        "supported": cfg!(windows),
        "installed": installed(data),
        "running": alive(&st),
        "exe": exe_path(data),
        "status": st,
        "settings": settings,
        "defaults": defaults,
        "gaze_version": env!("CARGO_PKG_VERSION"),
        "log": log_tail(data, 80),
    })
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("GAZE-updater/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(300))
        .build()?)
}

/// The watchdog's program from the newest release, checked against its
/// checksum, put in `<data>/watchdog/`.
async fn download(data: &Path, repo: &str) -> Result<()> {
    if !cfg!(all(windows, target_arch = "x86_64")) {
        bail!("the watchdog is for Windows");
    }
    let web = std::env::var("GAZE_UPDATE_WEB").unwrap_or_else(|_| "https://github.com".into());
    let base = format!("{web}/{repo}/releases/latest/download/{ASSET}");
    let c = client()?;
    let want = c.get(format!("{base}.sha256")).send().await.context("cannot reach GitHub")?.error_for_status().context("the newest release has no watchdog yet")?.text().await?;
    let want = want.split_whitespace().next().unwrap_or("").to_lowercase();
    let bytes = c.get(&base).send().await?.error_for_status().context("the newest release has no watchdog yet")?.bytes().await?;
    use sha2::Digest;
    let got: String = sha2::Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    if got != want {
        bail!("the download doesn't match its checksum: nothing was installed");
    }
    std::fs::create_dir_all(dir(data))?;
    let tmp = dir(data).join("GAZE-Watchdog.download");
    std::fs::write(&tmp, &bytes)?;
    let exe = exe_path(data);
    // A running copy is moved aside (allowed on Windows) and deleted later.
    if exe.exists() {
        let old = dir(data).join("GAZE-Watchdog.old.exe");
        let _ = std::fs::remove_file(&old);
        std::fs::rename(&exe, &old).context("the running watchdog couldn't be replaced: stop it first")?;
    }
    std::fs::rename(&tmp, &exe)?;
    Ok(())
}

/// Run the watchdog program with these arguments and read what it says.
async fn run_exe(exe: &Path, args: &[&str]) -> Result<String> {
    run_exe_for(exe, args, 60).await
}

async fn run_exe_for(exe: &Path, args: &[&str], secs: u64) -> Result<String> {
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args(args).stdin(std::process::Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let out = tokio::time::timeout(Duration::from_secs(secs), cmd.output()).await.context("the watchdog didn't answer")??;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() { Ok(text) } else { bail!("{}", if text.is_empty() { "it failed".to_string() } else { text }) }
}

/// Start the watchdog now (detached: it carries on when GAZE closes).
pub fn start(data: &Path) -> Result<()> {
    let _ = std::fs::remove_file(dir(data).join("stopped-on-purpose"));
    let exe = exe_path(data);
    if !exe.exists() {
        bail!("the watchdog isn't installed");
    }
    // Its own Windows service: started through Windows.
    if status(data).is_some_and(|s| s["startup"] == "service") && cfg!(windows) {
        let o = std::process::Command::new("sc").args(["start", "GazeWatchdog"]).output().context("couldn't start the service")?;
        if !o.status.success() && o.status.code() != Some(1056) {
            bail!("the watchdog service didn't start: {}", String::from_utf8_lossy(&o.stdout).trim());
        }
        return Ok(());
    }
    let abs = std::path::absolute(data).unwrap_or(data.to_path_buf());
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--run").arg("--data").arg(abs).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0000_0008 | 0x0000_0200);
    }
    cmd.spawn().context("couldn't start the watchdog")?;
    Ok(())
}

/// Download the watchdog, set it up and start it. `mode`: "login" (starts
/// when someone signs in) or "boot" (starts with the computer; needs
/// administrator rights).
pub async fn install(mgr: &Manager, mode: &str) -> Result<String> {
    if !["login", "service"].contains(&mode) {
        bail!("choose how the watchdog starts");
    }
    let data = std::path::absolute(&mgr.data_dir).unwrap_or(mgr.data_dir.clone());
    let repo = crate::update::repo(&mgr.config().update);
    // A running one is stopped first.
    if alive(&status(&data)) {
        let _ = command(&data, "stop");
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    download(&data, &repo).await?;
    if !dir(&data).join("settings.toml").exists() {
        write_settings(&data, &defaults(mgr))?;
    }
    // (A Windows service asks for administrator rights; give it time to be answered.)
    let msg = run_exe_for(&exe_path(&data), &["--install", "--mode", mode, "--data", &data.to_string_lossy()], 180).await?;
    if mode == "login" {
        start(&data)?;
    }
    tracing::info!("watchdog installed ({mode})");
    Ok(msg)
}

/// Take the watchdog out of Windows' start-up and stop it (its settings stay).
pub async fn uninstall(mgr: &Manager) -> Result<()> {
    let data = std::path::absolute(&mgr.data_dir).unwrap_or(mgr.data_dir.clone());
    if exe_path(&data).exists() {
        let _ = run_exe(&exe_path(&data), &["--uninstall", "--data", &data.to_string_lossy()]).await;
    }
    let _ = command(&data, "stop");
    tokio::time::sleep(Duration::from_secs(2)).await;
    for f in ["GAZE-Watchdog.exe", "GAZE-Watchdog.old.exe", "GAZE-Watchdog.new.exe"] {
        let _ = std::fs::remove_file(dir(&data).join(f));
    }
    let _ = std::fs::remove_file(dir(&data).join("status.json"));
    tracing::info!("watchdog removed");
    Ok(())
}

/// Keeps the watchdog's address for GAZE (and GAZE's program) up to date
/// when GAZE's web port changes, and tidies after an update.
pub async fn run(mgr: Arc<Manager>) {
    let mut tick = tokio::time::interval(Duration::from_secs(15));
    let mut n = 0u64;
    let mut was_running: Option<bool> = None;
    loop {
        tick.tick().await;
        n += 1;
        let data = std::path::absolute(&mgr.data_dir).unwrap_or(mgr.data_dir.clone());
        if !installed(&data) {
            was_running = None;
            *AUTO.lock().unwrap_or_else(|e| e.into_inner()) = Auto::default();
            continue;
        }
        // Is it running? Said in the log when that changes, and started again when it isn't (unless you stopped it).
        let running = alive(&status(&data));
        let on_purpose = dir(&data).join("stopped-on-purpose").exists();
        {
            let mut a = AUTO.lock().unwrap_or_else(|e| e.into_inner());
            if running {
                if was_running == Some(false) {
                    tracing::info!(
                        "watchdog: running again{}",
                        if a.attempts > 0 { format!(" (after {} attempt{} to start it)", a.attempts, if a.attempts == 1 { "" } else { "s" }) } else { String::new() }
                    );
                }
                *a = Auto::default();
            } else if was_running != Some(false) || a.down_since.is_none() {
                a.down_since.get_or_insert_with(std::time::Instant::now);
                if was_running == Some(true) {
                    tracing::warn!("watchdog: it has stopped — no sign of life from it for 20 seconds{}", if on_purpose { " (stopped on request)" } else { "" });
                } else if was_running.is_none() {
                    tracing::warn!("watchdog: it isn't running{}", if on_purpose { " (stopped on request)" } else { "" });
                }
            }
        }
        was_running = Some(running);
        if !running && !on_purpose {
            // After a minute without it, and then at most every 2 minutes (three tries; then only the title bar's button).
            let try_now = {
                let a = AUTO.lock().unwrap_or_else(|e| e.into_inner());
                a.down_since.is_some_and(|t| t.elapsed() >= Duration::from_secs(45)) && a.attempts < 3 && a.last_try.is_none_or(|t| t.elapsed() >= Duration::from_secs(120))
            };
            if try_now {
                let attempt = {
                    let mut a = AUTO.lock().unwrap_or_else(|e| e.into_inner());
                    a.attempts += 1;
                    a.last_try = Some(std::time::Instant::now());
                    a.attempts
                };
                tracing::warn!("watchdog: not running — GAZE is starting it (attempt {attempt} of 3)");
                match start(&data) {
                    Ok(()) => {
                        AUTO.lock().unwrap_or_else(|e| e.into_inner()).error = None;
                        tracing::info!("watchdog: start requested — waiting for it to report in");
                    }
                    Err(e) => {
                        AUTO.lock().unwrap_or_else(|x| x.into_inner()).error = Some(format!("{e:#}"));
                        tracing::warn!(
                            "watchdog: could not start it: {e:#}{}",
                            if attempt >= 3 { " — GAZE won't try again by itself; click the Watchdog button at the top to start it" } else { "" }
                        );
                    }
                }
            }
        }
        // Once a minute: keep its address for GAZE up to date, and tidy after an update.
        if n % 4 != 1 {
            continue;
        }
        let _ = std::fs::remove_file(dir(&data).join("GAZE-Watchdog.old.exe"));
        let mut m = read_settings(&data, &mgr);
        let url = check_url(&mgr.config().server.listen);
        let mut changed = false;
        if m["auto_url"] == json!(true) && m["check_url"] != json!(url) {
            m.insert("check_url".into(), json!(url));
            changed = true;
        }
        if changed {
            let _ = write_settings(&data, &m);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_follows_the_web_port() {
        assert_eq!(check_url("0.0.0.0:8080"), "http://127.0.0.1:8080/api/auth/state");
        assert_eq!(check_url("[::]:9000"), "http://127.0.0.1:9000/api/auth/state");
        assert_eq!(check_url("nonsense"), "http://127.0.0.1:8080/api/auth/state");
    }

    #[test]
    fn quit_marker_only_when_installed() {
        let d = std::env::temp_dir().join(format!("gaze-wdc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        mark_quit(&d);
        assert!(!dir(&d).join("gaze-quit").exists());
        std::fs::create_dir_all(dir(&d)).unwrap();
        std::fs::write(exe_path(&d), b"x").unwrap();
        mark_quit(&d);
        assert!(dir(&d).join("gaze-quit").exists());
        clear_quit(&d);
        assert!(!dir(&d).join("gaze-quit").exists());
        command(&d, "restart-gaze").unwrap();
        assert!(command(&d, "rm -rf").is_err());
        assert_eq!(std::fs::read_dir(dir(&d).join("commands")).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }
}
