// GAZE Watchdog: keeps GAZE running.
//
// An optional, independent program that comes with GAZE. It watches GAZE and
// starts it again when it has crashed or stopped answering. It starts with
// Windows (when someone signs in, or at boot), so after a restart of the
// computer it opens GAZE too. Only for GAZE.
//
// It talks to GAZE only through files in GAZE's data folder
// (`<data>/watchdog/`): GAZE's Settings → Watchdog page writes
// `settings.toml` and small command files; the watchdog writes
// `status.json` and `watchdog.log`. It has its own updates (GitHub
// releases, `GAZE-Watchdog.exe`), which GAZE can start from the browser.
//
//   gaze-watchdog --run --data <folder>          watch GAZE (what start-up runs)
//   gaze-watchdog --install --mode login|boot    start with Windows
//   gaze-watchdog --uninstall                    remove it from start-up
//   gaze-watchdog --status                       print what it is doing
#![cfg_attr(windows, windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const TASK_NAME: &str = "GazeWatchdog";
const ASSET: &str = "GAZE-Watchdog.exe";
const DEFAULT_REPO: &str = match option_env!("GAZE_UPDATE_REPO") {
    Some(r) => r,
    None => "impulsenz/NVR",
};

// ---------------------------------------------------------------- settings

/// What the watchdog does (edited in GAZE: Settings → Watchdog).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Watching on or off (off: it stays running but does nothing).
    pub enabled: bool,
    /// GAZE's program.
    pub gaze_exe: String,
    /// What GAZE is started with (its settings file and folder).
    pub gaze_args: Vec<String>,
    /// "auto" (the Windows service when GAZE is one), "program" or "service".
    pub launch: String,
    pub service_name: String,
    /// An address GAZE answers on.
    pub check_url: String,
    /// Keep `check_url` in step with GAZE's own web address (GAZE does that).
    pub auto_url: bool,
    pub check_interval_s: u64,
    pub timeout_s: u64,
    /// Checks in a row that must fail before GAZE counts as stalled.
    pub failures_before_restart: u32,
    /// After starting GAZE: how long it may take to answer.
    pub startup_grace_s: u64,
    /// Wait this long before starting a GAZE that has gone.
    pub restart_delay_s: u64,
    /// Also start GAZE again after it was closed on purpose (Quit).
    pub restart_on_clean_exit: bool,
    /// No more than this many restarts in `within_minutes` …
    pub max_restarts: u32,
    pub within_minutes: u64,
    /// … then wait this long before trying again.
    pub cooldown_minutes: u64,
    /// How long GAZE may take to close before it's ended by force.
    pub kill_timeout_s: u64,
    /// Install new watchdog versions by itself.
    pub auto_update: bool,
    pub update_check_hours: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: true,
            gaze_exe: String::new(),
            gaze_args: Vec::new(),
            launch: "auto".into(),
            service_name: "GAZE".into(),
            check_url: "http://127.0.0.1:8080/api/auth/state".into(),
            auto_url: true,
            check_interval_s: 10,
            timeout_s: 8,
            failures_before_restart: 3,
            startup_grace_s: 120,
            restart_delay_s: 10,
            restart_on_clean_exit: false,
            max_restarts: 5,
            within_minutes: 15,
            cooldown_minutes: 10,
            kill_timeout_s: 20,
            auto_update: false,
            update_check_hours: 12,
        }
    }
}

impl Settings {
    /// Numbers kept within sensible limits.
    pub fn clamped(mut self) -> Self {
        self.check_interval_s = self.check_interval_s.clamp(2, 3600);
        self.timeout_s = self.timeout_s.clamp(1, 120);
        self.failures_before_restart = self.failures_before_restart.clamp(1, 100);
        self.startup_grace_s = self.startup_grace_s.clamp(10, 3600);
        self.restart_delay_s = self.restart_delay_s.clamp(0, 600);
        self.max_restarts = self.max_restarts.clamp(1, 100);
        self.within_minutes = self.within_minutes.clamp(1, 1440);
        self.cooldown_minutes = self.cooldown_minutes.clamp(1, 1440);
        self.kill_timeout_s = self.kill_timeout_s.clamp(1, 300);
        self.update_check_hours = self.update_check_hours.clamp(1, 720);
        if !["auto", "program", "service"].contains(&self.launch.as_str()) {
            self.launch = "auto".into();
        }
        if self.service_name.trim().is_empty() {
            self.service_name = "GAZE".into();
        }
        self
    }
}

fn dir_of(data: &Path) -> PathBuf {
    data.join("watchdog")
}

fn load_settings(data: &Path) -> Settings {
    std::fs::read_to_string(dir_of(data).join("settings.toml"))
        .ok()
        .and_then(|t| toml::from_str::<Settings>(&t).ok())
        .unwrap_or_default()
        .clamped()
}

// ------------------------------------------------------------------ status

/// What the watchdog is doing (read by GAZE).
#[derive(Debug, Clone, Serialize, Default)]
pub struct Status {
    pub version: String,
    pub pid: u32,
    pub started: i64,
    /// When this was written (unix ms): GAZE treats it as stopped when old.
    pub updated: i64,
    /// "watching", "starting", "waiting", "stopped" (GAZE was quit on
    /// purpose), "cooldown", "paused" (switched off), "updating".
    pub state: String,
    pub message: String,
    pub gaze_running: bool,
    pub gaze_pid: Option<u32>,
    pub last_check_ok: Option<bool>,
    pub last_check_at: Option<i64>,
    pub last_check_ms: Option<u64>,
    pub consecutive_failures: u32,
    pub last_restart_at: Option<i64>,
    pub last_restart_reason: String,
    pub restarts_total: u64,
    pub restarts_recent: usize,
    pub launch: String,
    /// "login", "boot" or "none".
    pub startup: String,
    pub latest: Option<String>,
    pub update_available: bool,
    pub update_checked: Option<i64>,
    pub update_error: Option<String>,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn write_status(data: &Path, st: &Status) {
    let dir = dir_of(data);
    let _ = std::fs::create_dir_all(&dir);
    let tmp = dir.join("status.json.tmp");
    if std::fs::write(&tmp, serde_json::to_vec_pretty(st).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, dir.join("status.json"));
    }
}

// --------------------------------------------------------------------- log

fn log(data: &Path, msg: &str) {
    use std::io::Write;
    let dir = dir_of(data);
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join("watchdog.log");
    // Kept small: the old log is set aside at 1 MB.
    if std::fs::metadata(&file).map(|m| m.len() > 1_000_000).unwrap_or(false) {
        let _ = std::fs::rename(&file, dir.join("watchdog.log.1"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&file) {
        let _ = writeln!(f, "{}  {msg}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    }
}

// ----------------------------------------------------------- the decisions

/// What to do after looking at GAZE.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Nothing,
    /// GAZE isn't running: start it.
    Start(String),
    /// GAZE is running but stalled: end it and start it again.
    Restart(String),
}

/// The watchdog's memory between looks. Pure (no clock, no files) so it can
/// be tested: `now` is passed in.
#[derive(Debug, Default)]
pub struct Monitor {
    pub fails: u32,
    /// GAZE may be silent until this moment (it was just started).
    pub grace_until: Option<Instant>,
    /// When GAZE was first seen gone (it's started after `restart_delay_s`).
    pub gone_since: Option<Instant>,
    pub restarts: VecDeque<Instant>,
    pub cooldown_until: Option<Instant>,
    pub state: String,
    pub message: String,
}

impl Monitor {
    pub fn recent(&mut self, s: &Settings, now: Instant) -> usize {
        let w = Duration::from_secs(s.within_minutes * 60);
        while self.restarts.front().is_some_and(|t| now.duration_since(*t) > w) {
            self.restarts.pop_front();
        }
        self.restarts.len()
    }

    /// GAZE was started (or ended and started again) now.
    pub fn started(&mut self, s: &Settings, now: Instant) {
        self.restarts.push_back(now);
        self.grace_until = Some(now + Duration::from_secs(s.startup_grace_s));
        self.fails = 0;
        self.gone_since = None;
    }

    /// One look: is its program running, did it answer, was it quit on
    /// purpose (`quit_marker`)?
    pub fn look(&mut self, s: &Settings, running: bool, answered: bool, quit_marker: bool, now: Instant) -> Action {
        if !s.enabled {
            self.state = "paused".into();
            self.message = "Watching is switched off".into();
            self.fails = 0;
            self.gone_since = None;
            return Action::Nothing;
        }
        if let Some(t) = self.cooldown_until {
            if now < t {
                self.state = "cooldown".into();
                self.message = format!("Too many restarts: waiting {} more minute(s) before trying again", (t - now).as_secs().div_ceil(60));
                return Action::Nothing;
            }
            self.cooldown_until = None;
            self.restarts.clear();
        }
        if running {
            self.gone_since = None;
            if answered {
                self.fails = 0;
                self.grace_until = None;
                self.state = "watching".into();
                self.message = "GAZE is running and answering".into();
                return Action::Nothing;
            }
            // Running but silent.
            if self.grace_until.is_some_and(|g| now < g) {
                self.state = "starting".into();
                self.message = "Waiting for GAZE to start answering".into();
                return Action::Nothing;
            }
            self.fails += 1;
            if self.fails < s.failures_before_restart {
                self.state = "watching".into();
                self.message = format!("GAZE didn't answer ({} of {})", self.fails, s.failures_before_restart);
                return Action::Nothing;
            }
            return self.act(s, now, Action::Restart(format!("GAZE stopped answering ({} checks in a row)", self.fails)));
        }
        // Not running.
        if quit_marker && !s.restart_on_clean_exit {
            self.state = "stopped".into();
            self.message = "GAZE was closed on purpose (the watchdog leaves it closed)".into();
            self.gone_since = None;
            return Action::Nothing;
        }
        let since = *self.gone_since.get_or_insert(now);
        if now.duration_since(since) < Duration::from_secs(s.restart_delay_s) {
            self.state = "waiting".into();
            self.message = "GAZE isn't running: starting it in a moment".into();
            return Action::Nothing;
        }
        self.act(s, now, Action::Start("GAZE was not running".into()))
    }

    /// The restart limit: too many in a short time and it waits.
    fn act(&mut self, s: &Settings, now: Instant, a: Action) -> Action {
        if self.recent(s, now) >= s.max_restarts as usize {
            self.cooldown_until = Some(now + Duration::from_secs(s.cooldown_minutes * 60));
            self.state = "cooldown".into();
            self.message = format!("{} restarts in {} minutes: waiting {} minutes before trying again", s.max_restarts, s.within_minutes, s.cooldown_minutes);
            return Action::Nothing;
        }
        self.state = "starting".into();
        a
    }
}

// ---------------------------------------------------------------- processes

fn new_system() -> Option<sysinfo::System> {
    sysinfo::System::new().ok()
}

/// GAZE's processes (by its program, or by its name when the path can't be read).
pub fn find_gaze(exe: &str) -> Vec<sysinfo::Pid> {
    use sysinfo::ProcessesToUpdate;
    if exe.trim().is_empty() {
        return Vec::new();
    }
    let want = Path::new(exe);
    let name = want.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let Some(mut sys) = new_system() else { return Vec::new() };
    sys.refresh_processes(ProcessesToUpdate::All, true);
    let me = std::process::id();
    sys.processes()
        .values()
        .filter(|p| p.pid().as_u32() != me && p.status() != sysinfo::ProcessStatus::Zombie)
        .filter(|p| match p.exe() {
            Some(e) => e.to_string_lossy().eq_ignore_ascii_case(exe) || (e.file_name() == want.file_name() && e.parent() == want.parent()),
            None => p.name().to_string_lossy().eq_ignore_ascii_case(&name),
        })
        .map(|p| p.pid())
        .collect()
}

fn kill_pids(pids: &[sysinfo::Pid]) {
    use sysinfo::ProcessesToUpdate;
    let Some(mut sys) = new_system() else { return };
    sys.refresh_processes(ProcessesToUpdate::All, true);
    for p in pids {
        if let Some(pr) = sys.process(*p) {
            pr.kill();
        }
    }
}

#[cfg(windows)]
fn hide(cmd: &mut std::process::Command) -> &mut std::process::Command {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000) // no console window
}
#[cfg(not(windows))]
fn hide(cmd: &mut std::process::Command) -> &mut std::process::Command {
    cmd
}

/// Is GAZE installed as a Windows service?
fn service_exists(name: &str) -> bool {
    cfg!(windows) && hide(std::process::Command::new("sc").args(["query", name])).output().map(|o| o.status.success()).unwrap_or(false)
}

fn use_service(s: &Settings) -> bool {
    match s.launch.as_str() {
        "service" => cfg!(windows),
        "program" => false,
        _ => service_exists(&s.service_name),
    }
}

/// Start GAZE: its service, or its program (detached, so it carries on if
/// the watchdog is updated).
pub fn start_gaze(s: &Settings) -> Result<(), String> {
    if use_service(s) {
        let o = hide(std::process::Command::new("sc").args(["start", &s.service_name])).output().map_err(|e| format!("sc start: {e}"))?;
        let text = String::from_utf8_lossy(&o.stdout).to_string();
        // 1056: already running.
        return if o.status.success() || o.status.code() == Some(1056) { Ok(()) } else { Err(format!("starting the GAZE service failed: {}", text.trim())) };
    }
    if s.gaze_exe.trim().is_empty() {
        return Err("GAZE's program isn't set (Settings → Watchdog in GAZE)".into());
    }
    if !Path::new(&s.gaze_exe).exists() {
        return Err(format!("{} doesn't exist", s.gaze_exe));
    }
    let mut cmd = std::process::Command::new(&s.gaze_exe);
    cmd.args(&s.gaze_args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    if let Some(d) = Path::new(&s.gaze_exe).parent().filter(|d| d.is_dir()) {
        cmd.current_dir(d);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0000_0008 | 0x0000_0200); // detached, own process group
    }
    cmd.spawn().map(|_| ()).map_err(|e| format!("couldn't start {}: {e}", s.gaze_exe))
}

/// End GAZE (asked first, then by force after `kill_timeout_s`).
pub fn stop_gaze(s: &Settings) {
    if use_service(s) {
        let _ = hide(std::process::Command::new("sc").args(["stop", &s.service_name])).output();
    }
    let pids = find_gaze(&s.gaze_exe);
    if pids.is_empty() {
        return;
    }
    let until = Instant::now() + Duration::from_secs(s.kill_timeout_s);
    while Instant::now() < until && !find_gaze(&s.gaze_exe).is_empty() {
        std::thread::sleep(Duration::from_millis(500));
        // A stalled program won't close by itself: a service that was
        // asked to stop and didn't, a program that never was.
        if !use_service(s) {
            break;
        }
    }
    let left = find_gaze(&s.gaze_exe);
    kill_pids(&left);
    let until = Instant::now() + Duration::from_secs(10);
    while Instant::now() < until && !find_gaze(&s.gaze_exe).is_empty() {
        std::thread::sleep(Duration::from_millis(300));
    }
}

// ------------------------------------------------------------ the check

/// Does GAZE answer? Anything but a server error counts (GAZE may send you
/// to its https address). Returns how long it took.
pub async fn check(url: &str, timeout: Duration) -> Result<u64, String> {
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .danger_accept_invalid_certs(true) // GAZE's own certificate, on this computer
        .user_agent(concat!("GAZE-Watchdog/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    let t = Instant::now();
    match client.get(url).send().await {
        Ok(r) if r.status().is_server_error() => Err(format!("GAZE answered with an error ({})", r.status())),
        Ok(_) => Ok(t.elapsed().as_millis() as u64),
        Err(e) if e.is_timeout() => Err(format!("no answer within {} seconds", timeout.as_secs())),
        Err(e) => Err(format!("{e}")),
    }
}

// -------------------------------------------------------------- start-up

/// How the watchdog starts with Windows: "login", "boot" or "none".
fn startup_state() -> String {
    if !cfg!(windows) {
        return "none".into();
    }
    let ok = |c: &mut std::process::Command| hide(c).output().map(|o| o.status.success()).unwrap_or(false);
    if ok(std::process::Command::new("schtasks").args(["/Query", "/TN", TASK_NAME])) {
        "boot".into()
    } else if ok(std::process::Command::new("reg").args(["query", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run", "/v", TASK_NAME])) {
        "login".into()
    } else {
        "none".into()
    }
}

fn install_startup(mode: &str, data: &Path) -> Result<String, String> {
    if !cfg!(windows) {
        return Err("The watchdog's start-up is for Windows".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let run = format!("\"{}\" --run --data \"{}\"", exe.display(), data.display());
    uninstall_startup();
    let out = match mode {
        "boot" => hide(std::process::Command::new("schtasks").args(["/Create", "/TN", TASK_NAME, "/TR", &run, "/SC", "ONSTART", "/RU", "SYSTEM", "/RL", "HIGHEST", "/F"])).output(),
        "login" => hide(std::process::Command::new("reg").args(["add", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run", "/v", TASK_NAME, "/t", "REG_SZ", "/d", &run, "/f"])).output(),
        _ => return Err("mode must be login or boot".into()),
    }
    .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(if mode == "boot" { "The watchdog starts when the computer starts".into() } else { "The watchdog starts when you sign in".into() })
    } else {
        let t = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        Err(if mode == "boot" { format!("Starting at boot needs administrator rights (run GAZE as administrator, or choose “when I sign in”): {}", t.trim()) } else { t.trim().to_string() })
    }
}

fn uninstall_startup() {
    if cfg!(windows) {
        let _ = hide(std::process::Command::new("schtasks").args(["/Delete", "/TN", TASK_NAME, "/F"])).output();
        let _ = hide(std::process::Command::new("reg").args(["delete", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run", "/v", TASK_NAME, "/f"])).output();
    }
}

// --------------------------------------------------------------- updates

fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let mut p = v.trim().trim_start_matches('v').split('.').map(|x| x.split(['-', '+']).next().and_then(|n| n.parse::<u32>().ok()));
    Some((p.next()??, p.next().flatten().unwrap_or(0), p.next().flatten().unwrap_or(0)))
}

fn web_base() -> String {
    std::env::var("GAZE_UPDATE_WEB").unwrap_or_else(|_| "https://github.com".into())
}

fn repo() -> String {
    std::env::var("GAZE_UPDATE_REPO").unwrap_or_else(|_| DEFAULT_REPO.to_string())
}

/// The newest release's version (from github.com's own page, which isn't
/// limited like its API).
async fn latest_version() -> Result<String, String> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("GAZE-Watchdog/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())?;
    let r = client.get(format!("{}/{}/releases/latest", web_base(), repo())).send().await.map_err(|e| format!("can't reach GitHub: {e}"))?;
    let loc = r.headers().get("location").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    loc.rsplit_once("/releases/tag/").map(|(_, t)| t.split(['?', '#']).next().unwrap_or(t).trim_start_matches('v').to_string()).filter(|t| !t.is_empty()).ok_or_else(|| "no releases yet".to_string())
}

/// Download the new watchdog next to this one, check it, put it in place
/// and start it (this one then ends).
async fn install_update(data: &Path) -> Result<String, String> {
    let latest = latest_version().await?;
    let newer = matches!((parse_version(&latest), parse_version(VERSION)), (Some(l), Some(c)) if l > c);
    if !newer {
        return Ok(format!("The watchdog is up to date (version {VERSION})"));
    }
    if !cfg!(all(windows, target_arch = "x86_64")) {
        return Err("Updates of the watchdog are for Windows".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let base = format!("{}/{}/releases/download/v{latest}/{ASSET}", web_base(), repo());
    let client = reqwest::Client::builder().user_agent(concat!("GAZE-Watchdog/", env!("CARGO_PKG_VERSION"))).connect_timeout(Duration::from_secs(30)).timeout(Duration::from_secs(300)).build().map_err(|e| e.to_string())?;
    let want = client.get(format!("{base}.sha256")).send().await.map_err(|e| e.to_string())?.error_for_status().map_err(|e| format!("no checksum for {latest}: {e}"))?.text().await.map_err(|e| e.to_string())?;
    let want = want.split_whitespace().next().unwrap_or("").to_lowercase();
    let bytes = client.get(&base).send().await.map_err(|e| e.to_string())?.error_for_status().map_err(|e| format!("no watchdog in release {latest}: {e}"))?.bytes().await.map_err(|e| e.to_string())?;
    let got = {
        use sha2::Digest;
        sha2::Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect::<String>()
    };
    if got != want {
        return Err("the download doesn't match its checksum: nothing was changed".into());
    }
    let new = exe.with_file_name("GAZE-Watchdog.new.exe");
    std::fs::write(&new, &bytes).map_err(|e| e.to_string())?;
    // The running program is renamed aside (allowed on Windows), the new one takes its name.
    let old = exe.with_file_name("GAZE-Watchdog.old.exe");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).map_err(|e| format!("couldn't move the running watchdog aside: {e}"))?;
    if let Err(e) = std::fs::rename(&new, &exe) {
        let _ = std::fs::rename(&old, &exe);
        return Err(format!("couldn't put the new watchdog in place: {e}"));
    }
    log(data, &format!("updated to {latest}: starting the new version"));
    let mut cmd = std::process::Command::new(&exe);
    cmd.args(["--run", "--data"]).arg(data).args(["--wait-pid", &std::process::id().to_string()]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0000_0008 | 0x0000_0200);
    }
    cmd.spawn().map_err(|e| format!("couldn't start the new watchdog: {e}"))?;
    Ok(format!("Updated to {latest}"))
}

// ----------------------------------------------------------------- the run

/// Only one watchdog at a time.
fn take_lock(data: &Path, wait_pid: Option<u32>) -> bool {
    use sysinfo::{Pid, ProcessesToUpdate};
    let dir = dir_of(data);
    let _ = std::fs::create_dir_all(&dir);
    if let Some(p) = wait_pid {
        // The old version is closing: wait for it.
        for _ in 0..40 {
            let Some(mut sys) = new_system() else { break };
            sys.refresh_processes(ProcessesToUpdate::Some(&[Pid::from_u32(p)]), true);
            if sys.process(Pid::from_u32(p)).is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    let lock = dir.join("watchdog.lock");
    if let Some(pid) = std::fs::read_to_string(&lock).ok().and_then(|t| t.trim().parse::<u32>().ok()) {
        let mut sys = new_system();
        if let Some(sys) = sys.as_mut() { sys.refresh_processes(ProcessesToUpdate::Some(&[Pid::from_u32(pid)]), true); }
        if pid != std::process::id() && sys.as_ref().and_then(|s| s.process(Pid::from_u32(pid))).is_some_and(|p| p.name().to_string_lossy().to_lowercase().contains("watchdog")) {
            return false;
        }
    }
    std::fs::write(&lock, std::process::id().to_string()).is_ok()
}

fn quit_marker(data: &Path) -> PathBuf {
    dir_of(data).join("gaze-quit")
}

/// Command files GAZE leaves in `commands/`: one word each.
fn take_commands(data: &Path) -> Vec<String> {
    let dir = dir_of(data).join("commands");
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        let mut files: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        files.sort();
        for p in files {
            if let Ok(t) = std::fs::read_to_string(&p) {
                out.push(t.trim().to_string());
            }
            let _ = std::fs::remove_file(&p);
        }
    }
    out
}

async fn run(data: PathBuf, wait_pid: Option<u32>) {
    if !take_lock(&data, wait_pid) {
        eprintln!("The watchdog is already running.");
        return;
    }
    let started = now_ms();
    log(&data, &format!("watchdog {VERSION} started"));
    // A new run: what was quit before doesn't count.
    let _ = std::fs::remove_file(quit_marker(&data));
    let mut settings = load_settings(&data);
    let mut settings_mtime = std::fs::metadata(dir_of(&data).join("settings.toml")).and_then(|m| m.modified()).ok();
    let mut mon = Monitor::default();
    let mut st = Status { version: VERSION.into(), pid: std::process::id(), started, startup: startup_state(), ..Default::default() };
    let (mut last_check, mut last_status, mut last_update) = (Instant::now() - Duration::from_secs(3600), Instant::now() - Duration::from_secs(3600), Instant::now() - Duration::from_secs(10 * 86400));
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let mut force: Option<String> = None;
    let mut recheck_update = true;
    loop {
        tick.tick().await;
        // New settings from GAZE.
        let m = std::fs::metadata(dir_of(&data).join("settings.toml")).and_then(|m| m.modified()).ok();
        if m != settings_mtime {
            settings_mtime = m;
            settings = load_settings(&data);
            log(&data, "settings changed");
        }
        for c in take_commands(&data) {
            match c.as_str() {
                "restart-gaze" => force = Some("restart asked for in GAZE".into()),
                "check-update" => recheck_update = true,
                "update" => {
                    st.state = "updating".into();
                    st.message = "Updating the watchdog".into();
                    write_status(&data, &st);
                    match install_update(&data).await {
                        Ok(msg) if msg.starts_with("Updated") => {
                            let _ = std::fs::remove_file(dir_of(&data).join("watchdog.lock"));
                            return;
                        }
                        Ok(msg) => st.update_error = Some(msg),
                        Err(e) => {
                            log(&data, &format!("update failed: {e}"));
                            st.update_error = Some(e);
                        }
                    }
                }
                "stop" => {
                    log(&data, "stopped by GAZE");
                    let _ = std::fs::remove_file(dir_of(&data).join("watchdog.lock"));
                    st.state = "stopped".into();
                    st.message = "The watchdog was stopped".into();
                    st.updated = 0;
                    write_status(&data, &st);
                    return;
                }
                _ => {}
            }
        }
        // Updates: looked for now and then; installed by itself if chosen.
        if recheck_update || last_update.elapsed() > Duration::from_secs(settings.update_check_hours * 3600) {
            recheck_update = false;
            last_update = Instant::now();
            match latest_version().await {
                Ok(v) => {
                    st.update_available = matches!((parse_version(&v), parse_version(VERSION)), (Some(l), Some(c)) if l > c);
                    st.latest = Some(v);
                    st.update_error = None;
                }
                Err(e) => st.update_error = Some(e),
            }
            st.update_checked = Some(now_ms());
            if st.update_available && settings.auto_update {
                force = None;
                match install_update(&data).await {
                    Ok(msg) if msg.starts_with("Updated") => {
                        let _ = std::fs::remove_file(dir_of(&data).join("watchdog.lock"));
                        return;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        log(&data, &format!("update failed: {e}"));
                        st.update_error = Some(e);
                    }
                }
            }
        }
        let due = last_check.elapsed() >= Duration::from_secs(settings.check_interval_s) || force.is_some();
        if due {
            last_check = Instant::now();
            let pids = find_gaze(&settings.gaze_exe);
            let running = !pids.is_empty() || (use_service(&settings) && service_running(&settings.service_name));
            let mut answered = false;
            if running {
                match check(&settings.check_url, Duration::from_secs(settings.timeout_s)).await {
                    Ok(ms) => {
                        answered = true;
                        st.last_check_ms = Some(ms);
                    }
                    Err(e) => {
                        mon.message = e.clone();
                        if mon.fails == 0 {
                            log(&data, &format!("check failed: {e}"));
                        }
                    }
                }
                st.last_check_ok = Some(answered);
                st.last_check_at = Some(now_ms());
            }
            // GAZE is back: a quit marker from before no longer counts.
            if running {
                let _ = std::fs::remove_file(quit_marker(&data));
            }
            let marker = quit_marker(&data).exists();
            let action = if let Some(why) = force.take() { if running { Action::Restart(why) } else { Action::Start(why) } } else { mon.look(&settings, running, answered, marker, Instant::now()) };
            let was_waiting = mon.message.clone();
            match action {
                Action::Nothing => {}
                Action::Start(why) | Action::Restart(why) => {
                    let restarting = running;
                    log(&data, &format!("{} GAZE: {why}", if restarting { "restarting" } else { "starting" }));
                    if restarting {
                        let s2 = settings.clone();
                        let _ = tokio::task::spawn_blocking(move || stop_gaze(&s2)).await;
                    }
                    match start_gaze(&settings) {
                        Ok(()) => {
                            mon.started(&settings, Instant::now());
                            st.restarts_total += 1;
                            st.last_restart_at = Some(now_ms());
                            st.last_restart_reason = why;
                            mon.state = "starting".into();
                            mon.message = "GAZE was started: waiting for it to answer".into();
                        }
                        Err(e) => {
                            log(&data, &format!("could not start GAZE: {e}"));
                            // Tried again at the next look (counts toward the limit).
                            mon.restarts.push_back(Instant::now());
                            mon.state = "waiting".into();
                            mon.message = e;
                        }
                    }
                }
            }
            if mon.message.is_empty() {
                mon.message = was_waiting;
            }
        }
        if last_status.elapsed() >= Duration::from_secs(3) || due {
            last_status = Instant::now();
            let pids = find_gaze(&settings.gaze_exe);
            st.updated = now_ms();
            st.state = if mon.state.is_empty() { "watching".into() } else { mon.state.clone() };
            st.message = mon.message.clone();
            st.gaze_running = !pids.is_empty();
            st.gaze_pid = pids.first().map(|p| p.as_u32());
            st.consecutive_failures = mon.fails;
            st.restarts_recent = mon.recent(&settings, Instant::now());
            st.launch = if use_service(&settings) { "service".into() } else { "program".into() };
            if due {
                st.startup = startup_state();
            }
            write_status(&data, &st);
        }
    }
}

#[cfg(windows)]
fn service_running(name: &str) -> bool {
    hide(std::process::Command::new("sc").args(["query", name])).output().map(|o| String::from_utf8_lossy(&o.stdout).contains("RUNNING")).unwrap_or(false)
}
#[cfg(not(windows))]
fn service_running(_name: &str) -> bool {
    false
}

fn arg_value(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // HTTPS (update checks) uses rustls with the ring crypto backend.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let data = arg_value(&args, "--data").map(PathBuf::from).unwrap_or_else(|| std::env::current_exe().ok().and_then(|e| e.parent().and_then(|p| p.parent()).map(|p| p.to_path_buf())).unwrap_or_default());
    if args.iter().any(|a| a == "--version") {
        println!("{VERSION}");
    } else if args.iter().any(|a| a == "--install") {
        let mode = arg_value(&args, "--mode").unwrap_or_else(|| "login".into());
        match install_startup(&mode, &data) {
            Ok(m) => println!("{m}"),
            Err(e) => {
                println!("{e}");
                std::process::exit(1);
            }
        }
    } else if args.iter().any(|a| a == "--uninstall") {
        uninstall_startup();
        println!("The watchdog no longer starts with Windows");
    } else if args.iter().any(|a| a == "--status") {
        println!("{}", std::fs::read_to_string(dir_of(&data).join("status.json")).unwrap_or_else(|_| "{}".into()));
    } else {
        let wait = arg_value(&args, "--wait-pid").and_then(|p| p.parse().ok());
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("runtime");
        rt.block_on(run(data, wait));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s() -> Settings {
        Settings { restart_delay_s: 0, failures_before_restart: 3, startup_grace_s: 30, max_restarts: 3, within_minutes: 10, cooldown_minutes: 5, ..Default::default() }
    }

    #[test]
    fn a_stalled_gaze_is_restarted_after_enough_failed_checks() {
        let (cfg, mut m, t0) = (s(), Monitor::default(), Instant::now());
        assert_eq!(m.look(&cfg, true, true, false, t0), Action::Nothing);
        assert_eq!(m.look(&cfg, true, false, false, t0), Action::Nothing);
        assert_eq!(m.look(&cfg, true, false, false, t0), Action::Nothing);
        assert!(matches!(m.look(&cfg, true, false, false, t0), Action::Restart(_)));
        // An answer clears the count.
        let mut m = Monitor::default();
        m.look(&cfg, true, false, false, t0);
        m.look(&cfg, true, true, false, t0);
        assert_eq!(m.fails, 0);
    }

    #[test]
    fn a_gaze_that_was_just_started_may_be_silent() {
        let (cfg, mut m, t0) = (s(), Monitor::default(), Instant::now());
        m.started(&cfg, t0);
        for i in 0..10 {
            assert_eq!(m.look(&cfg, true, false, false, t0 + Duration::from_secs(i)), Action::Nothing);
        }
        // After the grace time the failures count.
        let late = t0 + Duration::from_secs(31);
        m.look(&cfg, true, false, false, late);
        m.look(&cfg, true, false, false, late);
        assert!(matches!(m.look(&cfg, true, false, false, late), Action::Restart(_)));
    }

    #[test]
    fn a_gaze_that_is_gone_is_started_after_the_delay_unless_quit_on_purpose() {
        let mut cfg = s();
        cfg.restart_delay_s = 10;
        let (mut m, t0) = (Monitor::default(), Instant::now());
        assert_eq!(m.look(&cfg, false, false, false, t0), Action::Nothing);
        assert_eq!(m.look(&cfg, false, false, false, t0 + Duration::from_secs(5)), Action::Nothing);
        assert!(matches!(m.look(&cfg, false, false, false, t0 + Duration::from_secs(11)), Action::Start(_)));
        // Quit on purpose: left alone, unless the setting says otherwise.
        let mut m = Monitor::default();
        assert_eq!(m.look(&cfg, false, false, true, t0 + Duration::from_secs(60)), Action::Nothing);
        assert_eq!(m.state, "stopped");
        cfg.restart_on_clean_exit = true;
        cfg.restart_delay_s = 0;
        assert!(matches!(m.look(&cfg, false, false, true, t0 + Duration::from_secs(61)), Action::Start(_)));
    }

    #[test]
    fn too_many_restarts_make_it_wait() {
        let (cfg, mut m, t0) = (s(), Monitor::default(), Instant::now());
        for i in 0..3 {
            assert!(matches!(m.look(&cfg, false, false, false, t0 + Duration::from_secs(i)), Action::Start(_)));
            m.started(&cfg, t0 + Duration::from_secs(i));
        }
        assert_eq!(m.look(&cfg, false, false, false, t0 + Duration::from_secs(4)), Action::Nothing);
        assert_eq!(m.state, "cooldown");
        // After the cooldown it tries again.
        assert!(matches!(m.look(&cfg, false, false, false, t0 + Duration::from_secs(5 * 60 + 10)), Action::Start(_)));
    }

    #[test]
    fn switched_off_does_nothing() {
        let mut cfg = s();
        cfg.enabled = false;
        let mut m = Monitor::default();
        assert_eq!(m.look(&cfg, false, false, false, Instant::now()), Action::Nothing);
        assert_eq!(m.state, "paused");
    }

    #[test]
    fn settings_are_kept_within_limits_and_round_trip() {
        let bad = Settings { check_interval_s: 0, timeout_s: 9999, failures_before_restart: 0, launch: "x".into(), service_name: " ".into(), ..Default::default() }.clamped();
        assert_eq!((bad.check_interval_s, bad.timeout_s, bad.failures_before_restart, bad.launch.as_str(), bad.service_name.as_str()), (2, 120, 1, "auto", "GAZE"));
        let t = toml::to_string(&Settings::default()).unwrap();
        assert_eq!(toml::from_str::<Settings>(&t).unwrap(), Settings::default());
        // A file with only some settings still works.
        assert!(toml::from_str::<Settings>("check_interval_s = 30").unwrap().enabled);
    }

    #[test]
    fn versions_compare() {
        assert!(parse_version("v1.2.0") > parse_version("1.1.9"));
        assert!(parse_version("1.1.5") == parse_version("1.1.5"));
    }

    #[tokio::test]
    async fn the_check_tells_answers_from_silence() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut n = 0;
            loop {
                let (mut sock, _) = l.accept().await.unwrap();
                n += 1;
                let mut buf = [0u8; 512];
                let _ = sock.read(&mut buf).await;
                let reply: &[u8] = match n { 1 => b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n", 2 => b"HTTP/1.1 302 Found\r\nlocation: https://x/\r\ncontent-length: 0\r\n\r\n", 3 => b"HTTP/1.1 500 Oops\r\ncontent-length: 0\r\n\r\n", _ => { tokio::time::sleep(Duration::from_secs(5)).await; b"" } };
                let _ = sock.write_all(reply).await;
            }
        });
        let url = format!("http://127.0.0.1:{port}/api/auth/state");
        assert!(check(&url, Duration::from_secs(2)).await.is_ok());
        assert!(check(&url, Duration::from_secs(2)).await.is_ok()); // a redirect is an answer
        assert!(check(&url, Duration::from_secs(2)).await.is_err()); // a server error isn't
        assert!(check(&url, Duration::from_secs(1)).await.unwrap_err().contains("no answer")); // silence
        assert!(check("http://127.0.0.1:1/", Duration::from_secs(1)).await.is_err()); // nothing listening
    }

    #[test]
    #[cfg(unix)]
    fn it_finds_starts_and_ends_a_program() {
        let dir = std::env::temp_dir().join(format!("gaze-wd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A stand-in for GAZE: a copy of `sleep` with its own name.
        let exe = dir.join("fakegaze");
        std::fs::copy("/bin/sleep", &exe).unwrap();
        let cfg = Settings { gaze_exe: exe.to_string_lossy().to_string(), gaze_args: vec!["30".into()], launch: "program".into(), kill_timeout_s: 2, ..Default::default() };
        assert!(find_gaze(&cfg.gaze_exe).is_empty());
        start_gaze(&cfg).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(find_gaze(&cfg.gaze_exe).len(), 1);
        stop_gaze(&cfg);
        assert!(find_gaze(&cfg.gaze_exe).is_empty());
        assert!(start_gaze(&Settings { gaze_exe: String::new(), launch: "program".into(), ..Default::default() }).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn status_and_commands_go_through_files() {
        let dir = std::env::temp_dir().join(format!("gaze-wd2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_status(&dir, &Status { version: "1".into(), state: "watching".into(), ..Default::default() });
        let t = std::fs::read_to_string(dir.join("watchdog").join("status.json")).unwrap();
        assert!(t.contains("\"watching\""));
        let c = dir.join("watchdog").join("commands");
        std::fs::create_dir_all(&c).unwrap();
        std::fs::write(c.join("0001"), "restart-gaze\n").unwrap();
        std::fs::write(c.join("0002"), "check-update").unwrap();
        assert_eq!(take_commands(&dir), vec!["restart-gaze", "check-update"]);
        assert!(take_commands(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
