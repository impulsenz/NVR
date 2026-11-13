//! Updates from GitHub releases (Settings → About).
//!
//! GitHub builds GAZE for Windows and Linux when a version is tagged
//! (.github/workflows/release.yml). GAZE checks the newest release now and
//! then; to update it downloads the program for this system, checks its
//! SHA-256, puts it in place of the running one (the old one is kept as
//! `nvr.old`) and restarts — as a Windows service too. With automatic
//! updates on, this happens at night when no alert is going on.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Local};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tracing::{info, warn};

/// Where updates come from unless the settings say otherwise: the GitHub
/// repository this program was built from (set by the release workflow),
/// or the original GAZE repository.
pub const DEFAULT_REPO: &str = match option_env!("GAZE_UPDATE_REPO") {
    Some(r) => r,
    None => "impulsenz/NVR",
};

/// "owner/name" of the repository to update from.
pub fn repo(cfg: &crate::config::UpdateConfig) -> String {
    let r = cfg
        .repo
        .trim()
        .trim_start_matches("https://github.com/")
        .trim_end_matches('/');
    if r.split('/').count() == 2
        && r.split('/').all(|p| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        })
    {
        r.to_string()
    } else {
        DEFAULT_REPO.to_string()
    }
}
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The release file for this system: GAZE.exe, or the name it had before
/// 0.6.73 (releases carry both).
fn asset_names() -> &'static [&'static str] {
    if cfg!(all(windows, target_arch = "x86_64")) {
        &["GAZE.exe", "nvr-windows-x64.exe"]
    } else {
        &[]
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Status {
    pub current: String,
    /// Repository updates come from ("owner/name").
    pub repo: String,
    /// Newest version on GitHub.
    pub latest: Option<String>,
    pub notes: String,
    pub published: Option<String>,
    pub available: bool,
    pub checked: Option<DateTime<Local>>,
    /// "idle", "checking", "downloading", "installing", "restarting".
    pub state: String,
    pub done: u64,
    pub total: u64,
    pub error: Option<String>,
    #[serde(skip)]
    url: String,
    #[serde(skip)]
    sha_url: String,
}

static STATUS: Mutex<Option<Status>> = Mutex::new(None);
/// How to restart GAZE once the new program is in place (set by main).
static RESTART: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

pub fn set_restart(f: impl Fn() + Send + Sync + 'static) {
    let _ = RESTART.set(Box::new(f));
}

/// Restart GAZE now (as after an update).
pub fn restart_now() -> bool {
    match RESTART.get() {
        Some(f) => {
            f();
            true
        }
        None => false,
    }
}

fn with<R>(f: impl FnOnce(&mut Status) -> R) -> R {
    let mut g = STATUS.lock().unwrap();
    let s = g.get_or_insert_with(|| Status {
        current: VERSION.into(),
        repo: DEFAULT_REPO.into(),
        state: "idle".into(),
        ..Default::default()
    });
    f(s)
}

pub fn status() -> Status {
    with(|s| s.clone())
}

/// "v1.2.3" → (1, 2, 3).
pub fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let core = v.split(['-', '+']).next()?;
    let mut p = core.split('.').map(|x| x.parse::<u32>().ok());
    Some((
        p.next()??,
        p.next().flatten().unwrap_or(0),
        p.next().flatten().unwrap_or(0),
    ))
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("GAZE-updater/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(60))
        .build()?)
}

/// GitHub's normal web pages (the release page and downloads): not limited
/// like its API, which allows only 60 requests an hour per internet
/// connection when not signed in. `GAZE_UPDATE_WEB` points elsewhere for
/// tests.
fn web_base() -> String {
    std::env::var("GAZE_UPDATE_WEB").unwrap_or_else(|_| "https://github.com".into())
}

fn api_base() -> String {
    std::env::var("GAZE_UPDATE_API").unwrap_or_else(|_| "https://api.github.com".into())
}

/// GitHub said "too many requests": don't ask again before this.
static NOT_BEFORE: Mutex<Option<DateTime<Local>>> = Mutex::new(None);

/// When GitHub lets us ask again, from its answer (Retry-After or its
/// rate-limit reset time); 15 minutes if it doesn't say.
fn retry_time(resp: &reqwest::Response) -> DateTime<Local> {
    let h = |n: &str| {
        resp.headers()
            .get(n)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<i64>().ok())
    };
    let now = Local::now();
    if let Some(secs) = h("retry-after") {
        return now + chrono::Duration::seconds(secs.clamp(10, 3600));
    }
    if let Some(reset) = h("x-ratelimit-reset")
        && let Some(t) = DateTime::from_timestamp(reset, 0)
    {
        let t = t.with_timezone(&Local);
        if t > now {
            return t.min(now + chrono::Duration::hours(1));
        }
    }
    now + chrono::Duration::minutes(15)
}

fn limited(resp: &reqwest::Response) -> bool {
    resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS
        || (resp.status() == reqwest::StatusCode::FORBIDDEN
            && resp
                .headers()
                .get("x-ratelimit-remaining")
                .is_some_and(|v| v == "0"))
}

/// Ask GitHub for the newest release.
///
/// The newest version comes from the address github.com/<repo>/releases/latest
/// redirects to (a web page, not the limited API); the program and its
/// checksum are at fixed download addresses. The API is only asked once
/// per new version, for the release notes (optional).
pub async fn check(repo: &str) -> Result<Status> {
    if let Some(t) = *NOT_BEFORE.lock().unwrap()
        && t > Local::now()
    {
        bail!(
            "GitHub asked GAZE to wait before checking again (it limits checks from one internet connection); checking again at {}",
            t.format("%H:%M")
        );
    }
    with(|s| {
        s.state = "checking".into();
        s.error = None;
        s.repo = repo.to_string();
    });
    let res = async {
        let web = web_base();
        let no_redirect = reqwest::Client::builder()
            .user_agent(concat!("GAZE-updater/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let resp = no_redirect
            .get(format!("{web}/{repo}/releases/latest"))
            .send()
            .await
            .context("cannot reach GitHub")?;
        if limited(&resp) {
            let t = retry_time(&resp);
            *NOT_BEFORE.lock().unwrap() = Some(t);
            bail!(
                "GitHub limits how often one internet connection can check; checking again at {}",
                t.format("%H:%M")
            );
        }
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            bail!("repository {repo} not found (or it's private)");
        }
        let location = resp
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        // ".../releases/tag/v0.5.6"; without any release it points at ".../releases".
        let Some(tag) = location
            .rsplit_once("/releases/tag/")
            .map(|(_, t)| t.split(['?', '#']).next().unwrap_or(t).to_string())
            .filter(|t| !t.is_empty())
        else {
            bail!("no releases yet");
        };
        let tag = urlencoding_decode(&tag);
        let version = tag.trim_start_matches('v').to_string();
        let newer =
            matches!((parse_version(&tag), parse_version(VERSION)), (Some(l), Some(c)) if l > c);
        if asset_names().is_empty() && newer {
            bail!("no ready-made updates for this kind of computer");
        }
        let known = with(|s| s.latest.as_deref() == Some(version.as_str()) && s.checked.is_some());
        if known {
            with(|s| s.checked = Some(Local::now()));
            return anyhow::Ok(());
        }
        // A new version: is the program for this computer attached?
        // (its small checksum file; GitHub's file storage refuses HEAD requests)
        let mut url = String::new();
        let mut has_asset = false;
        if newer {
            for name in asset_names() {
                let u = format!("{web}/{repo}/releases/download/{tag}/{name}");
                if client()?.get(format!("{u}.sha256")).send().await.is_ok_and(|r| r.status().is_success()) {
                    (url, has_asset) = (u, true);
                    break;
                }
            }
        }
        // Release notes and date (optional; the API may be rate-limited).
        let (mut notes, mut published) = (String::new(), None);
        if newer
            && let Ok(r) = client()?
                .get(format!("{}/repos/{repo}/releases/tags/{tag}", api_base()))
                .header("Accept", "application/vnd.github+json")
                .send()
                .await
            && r.status().is_success()
            && let Ok(v) = r.json::<Value>().await
        {
            notes = v["body"].as_str().unwrap_or("").to_string();
            published = v["published_at"].as_str().map(String::from);
        }
        with(|s| {
            s.latest = Some(version);
            s.notes = notes;
            s.published = published;
            s.available = newer && has_asset;
            s.url = if has_asset {
                url.clone()
            } else {
                String::new()
            };
            s.sha_url = if has_asset {
                format!("{url}.sha256")
            } else {
                String::new()
            };
            s.checked = Some(Local::now());
        });
        if newer && !has_asset {
            bail!("GAZE {tag} has no ready-made program for this kind of computer yet");
        }
        anyhow::Ok(())
    }
    .await;
    with(|s| {
        s.state = "idle".into();
        if let Err(e) = &res {
            s.error = Some(format!("{e:#}"));
            s.checked = Some(Local::now());
        }
    });
    res.map(|_| status())
}

/// "%2B" → "+" in a tag taken from an address.
fn urlencoding_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn sibling(exe: &Path, suffix: &str) -> PathBuf {
    let name = exe.file_stem().and_then(|s| s.to_str()).unwrap_or("nvr");
    let ext = exe
        .extension()
        .and_then(|s| s.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    exe.with_file_name(format!("{name}{suffix}{ext}"))
}

/// The previous program, left behind by an update (deleted at start-up;
/// one still in use stays until next time).
pub fn clean_old() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(sibling(&exe, ".old"));
        for n in 2..10 {
            let _ = std::fs::remove_file(sibling(&exe, &format!(".old{n}")));
        }
        let _ = std::fs::remove_file(sibling(&exe, ".new"));
    }
}

/// Put the downloaded program (`new`) in the place of the running one
/// (`exe`): the running one is renamed aside (allowed on Windows too, not
/// overwriting). Tried a few times — antivirus programs often hold a new
/// file for a moment — and with another name for the old copy when an
/// earlier one is still in use (by a copy of GAZE that hasn't restarted).
pub fn swap_in(new: &Path, exe: &Path) -> Result<()> {
    let mut last = None;
    for attempt in 0..6 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(500));
        }
        let old = (1..10)
            .map(|n| sibling(exe, &if n == 1 { ".old".to_string() } else { format!(".old{n}") }))
            .find(|p| !p.exists() || std::fs::remove_file(p).is_ok());
        let Some(old) = old else { bail!("old copies of GAZE next to it are still in use: restart the computer, then try again") };
        match std::fs::rename(exe, &old) {
            Ok(()) => {
                if let Err(e) = std::fs::rename(new, exe) {
                    let _ = std::fs::rename(&old, exe);
                    return Err(anyhow::Error::from(e).context("could not put the new program in place"));
                }
                return Ok(());
            }
            Err(e) => last = Some(e),
        }
    }
    Err(anyhow::Error::from(last.unwrap()).context("could not move the running program aside"))
}

/// Download, check and install the newest release, then restart.
pub async fn install() -> Result<()> {
    let st = status();
    if !st.available {
        bail!("no update available");
    }
    if with(|s| s.state != "idle") {
        bail!("an update is already in progress");
    }
    let res = async {
        let exe = std::env::current_exe()?;
        let new = sibling(&exe, ".new");
        with(|s| {
            s.state = "downloading".into();
            s.done = 0;
            s.total = 0;
            s.error = None;
        });
        let job = crate::jobs::start(
            "download",
            format!("Downloading GAZE {}", st.latest.clone().unwrap_or_default()),
        );
        let c = client()?;
        let want = if st.sha_url.is_empty() {
            None
        } else {
            let t = c
                .get(&st.sha_url)
                .send()
                .await?
                .error_for_status()?
                .text()
                .await?;
            Some(t.split_whitespace().next().unwrap_or("").to_lowercase())
        };
        let mut resp = c
            .get(&st.url)
            .send()
            .await
            .context("cannot reach GitHub")?
            .error_for_status()?;
        let total = resp.content_length().unwrap_or(0);
        let mut file = tokio::fs::File::create(&new)
            .await
            .with_context(|| format!("writing {}", new.display()))?;
        let mut hash = Sha256::new();
        let mut got = 0u64;
        while let Some(chunk) = resp.chunk().await? {
            tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
            hash.update(&chunk);
            got += chunk.len() as u64;
            job.bytes(got, total);
            with(|s| {
                s.done = got;
                s.total = total;
            });
        }
        tokio::io::AsyncWriteExt::flush(&mut file).await?;
        drop(file);
        let have: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if let Some(w) = want
            && w != have
        {
            let _ = std::fs::remove_file(&new);
            bail!("the download is damaged (checksum differs); nothing was changed");
        }
        if got < 1_000_000 {
            let _ = std::fs::remove_file(&new);
            bail!("the download looks wrong ({got} bytes); nothing was changed");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755))?;
        }
        with(|s| s.state = "installing".into());
        let (n2, e2) = (new.clone(), exe.clone());
        let swapped = tokio::task::spawn_blocking(move || swap_in(&n2, &e2)).await?;
        if let Err(e) = swapped {
            // Windows said no (the program file belongs to an administrator,
            // or to the GAZE service, e.g. it was installed or last updated
            // by one): ask once for permission (the usual Windows prompt, on
            // this computer's screen) and let that do the swap.
            #[cfg(windows)]
            {
                info!("update: {e:#} — asking Windows for permission");
                let args = format!("--swap-update \"{}\" \"{}\"", new.display(), exe.display());
                let code = tokio::task::spawn_blocking(move || crate::winservice::elevate(&args, true)).await?;
                if code == Some(0) {
                    return anyhow::Ok(());
                }
                bail!(
                    "Windows didn't let GAZE replace its own program file ({}){}. Close any other copy of GAZE and try again, \
                     or install it by hand: download GAZE.exe from the releases page and put it in place of the old one while GAZE is stopped.",
                    exe.display(),
                    if code.is_none() { " and the permission prompt was refused" } else { "" }
                );
            }
            #[cfg(not(windows))]
            return Err(e);
        }
        anyhow::Ok(())
    }
    .await;
    match res {
        Ok(()) => {
            info!(
                "updated to GAZE {} — restarting",
                st.latest.unwrap_or_default()
            );
            with(|s| s.state = "restarting".into());
            // Let the web page see "restarting" before going away.
            tokio::time::sleep(Duration::from_secs(1)).await;
            match RESTART.get() {
                Some(r) => r(),
                None => warn!("updated; restart GAZE to use the new version"),
            }
            Ok(())
        }
        Err(e) => {
            with(|s| {
                s.state = "idle".into();
                s.error = Some(format!("{e:#}"));
            });
            Err(e)
        }
    }
}

/// Background: check for a new version once a day (soon after starting,
/// then every 24 hours; an hour later if a check failed). "Check for
/// updates" asks straight away. With automatic updates on, a version found
/// is installed as soon as no alert is going on.
pub async fn run(mgr: Arc<crate::manager::Manager>) {
    const DAY: Duration = Duration::from_secs(24 * 3600);
    clean_old();
    tokio::time::sleep(Duration::from_secs(20)).await;
    let mut announced: Option<String> = None;
    let mut next_check = tokio::time::Instant::now();
    loop {
        if tokio::time::Instant::now() >= next_check {
            next_check = tokio::time::Instant::now()
                + match check(&repo(&mgr.config().update)).await {
                    Ok(s) => {
                        if s.available && announced != s.latest {
                            info!(
                                "update available: GAZE {}",
                                s.latest.clone().unwrap_or_default()
                            );
                            announced = s.latest;
                        }
                        DAY
                    }
                    Err(e) => {
                        tracing::debug!("update check: {e:#}");
                        Duration::from_secs(3600)
                    }
                };
        }
        let st = status();
        let busy = mgr
            .cameras()
            .iter()
            .any(|c| c.detector().is_some_and(|d| d.status().event_active));
        if st.available
            && mgr.config().update.auto
            && !busy
            && st.state == "idle"
            && let Err(e) = install().await
        {
            warn!("automatic update failed: {e:#}");
        }
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

#[cfg(test)]
mod tests {
    /// Against the real GitHub: GAZE_TEST_GITHUB=impulsenz/NVR cargo test finds_latest
    #[tokio::test]
    async fn finds_latest_release_without_the_api() {
        let Ok(repo) = std::env::var("GAZE_TEST_GITHUB") else {
            return;
        };
        let _ = rustls::crypto::ring::default_provider().install_default();
        let s = super::check(&repo).await.unwrap();
        assert!(s.latest.is_some(), "{s:?}");
        println!(
            "latest: {:?} available: {} url: {}",
            s.latest, s.available, s.url
        );
    }

    #[test]
    fn decodes_tags() {
        assert_eq!(super::urlencoding_decode("v1.0.0%2Bbuild"), "v1.0.0+build");
        assert_eq!(super::urlencoding_decode("v0.5.6"), "v0.5.6");
    }

    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v0.5.0"), Some((0, 5, 0)));
        assert_eq!(parse_version("1.2"), Some((1, 2, 0)));
        assert_eq!(parse_version("v1.10.3-beta"), Some((1, 10, 3)));
        assert!(parse_version("v0.10.0") > parse_version("0.9.9"));
        assert_eq!(parse_version("latest"), None);
        let exe = Path::new("C:/GAZE/nvr.exe");
        assert_eq!(sibling(exe, ".old"), Path::new("C:/GAZE/nvr.old.exe"));
        assert_eq!(
            sibling(Path::new("/opt/gaze/nvr"), ".new"),
            Path::new("/opt/gaze/nvr.new")
        );
        let r = |s: &str| {
            repo(&crate::config::UpdateConfig {
                auto: true,
                repo: s.into(),
            })
        };
        assert_eq!(r("me/NVR"), "me/NVR");
        assert_eq!(r("https://github.com/me/my-gaze/"), "me/my-gaze");
        assert_eq!(r(""), DEFAULT_REPO);
        assert_eq!(r("me/NVR/../../x"), DEFAULT_REPO, "no paths");
        assert_eq!(r("me/N V R"), DEFAULT_REPO);
    }
}
