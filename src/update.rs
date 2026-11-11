//! Updates from GitHub releases (Settings → About).
//!
//! GitHub builds GAZE for Windows and Linux when a version is tagged
//! (.github/workflows/release.yml). GAZE checks the newest release now and
//! then; to update it downloads the program for this system, checks its
//! SHA-256, puts it in place of the running one (the old one is kept as
//! `nvr.old`) and restarts — as a Windows service too. With automatic
//! updates on, this happens at night when no alert is going on.

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Local, Timelike};
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

/// The release file for this system.
fn asset_name() -> Option<&'static str> {
    if cfg!(all(windows, target_arch = "x86_64")) {
        Some("nvr-windows-x64.exe")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("nvr-linux-x64")
    } else {
        None
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

/// Ask GitHub for the newest release.
pub async fn check(repo: &str) -> Result<Status> {
    with(|s| {
        s.state = "checking".into();
        s.error = None;
        s.repo = repo.to_string();
    });
    let res = async {
        let v: Value = client()?
            .get(format!(
                "{}/repos/{repo}/releases/latest",
                std::env::var("GAZE_UPDATE_API")
                    .unwrap_or_else(|_| "https://api.github.com".into())
            ))
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .context("cannot reach GitHub")?
            .error_for_status()
            .map_err(|e| {
                if e.status() == Some(reqwest::StatusCode::NOT_FOUND) {
                    anyhow!("no releases yet")
                } else {
                    e.into()
                }
            })?
            .json()
            .await?;
        let tag = v["tag_name"].as_str().unwrap_or("").to_string();
        let name = asset_name()
            .ok_or_else(|| anyhow!("no ready-made updates for this kind of computer"))?;
        let find = |n: &str| {
            v["assets"]
                .as_array()
                .and_then(|a| a.iter().find(|x| x["name"] == n))
                .and_then(|x| x["browser_download_url"].as_str())
                .map(String::from)
        };
        let newer =
            matches!((parse_version(&tag), parse_version(VERSION)), (Some(l), Some(c)) if l > c);
        let url = find(name);
        with(|s| {
            s.latest = Some(tag.trim_start_matches('v').to_string());
            s.notes = v["body"].as_str().unwrap_or("").to_string();
            s.published = v["published_at"].as_str().map(String::from);
            s.available = newer && url.is_some();
            s.url = url.unwrap_or_default();
            s.sha_url = find(&format!("{name}.sha256")).unwrap_or_default();
            s.checked = Some(Local::now());
        });
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

fn sibling(exe: &Path, suffix: &str) -> PathBuf {
    let name = exe.file_stem().and_then(|s| s.to_str()).unwrap_or("nvr");
    let ext = exe
        .extension()
        .and_then(|s| s.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    exe.with_file_name(format!("{name}{suffix}{ext}"))
}

/// The previous program, left behind by an update (deleted at start-up).
pub fn clean_old() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(sibling(&exe, ".old"));
        let _ = std::fs::remove_file(sibling(&exe, ".new"));
    }
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
        // A running program can be renamed (also on Windows), not overwritten.
        let old = sibling(&exe, ".old");
        let _ = std::fs::remove_file(&old);
        std::fs::rename(&exe, &old).context("could not move the running program aside")?;
        if let Err(e) = std::fs::rename(&new, &exe) {
            let _ = std::fs::rename(&old, &exe);
            return Err(anyhow::Error::from(e).context("could not put the new program in place"));
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

/// Background: check now and then; with automatic updates on, install at
/// night (2–5 am) when no alert is going on.
pub async fn run(mgr: Arc<crate::manager::Manager>) {
    clean_old();
    tokio::time::sleep(Duration::from_secs(120)).await;
    let mut last_check: Option<std::time::Instant> = None;
    loop {
        if last_check.is_none_or(|t| t.elapsed() > Duration::from_secs(6 * 3600)) {
            last_check = Some(std::time::Instant::now());
            match check(&repo(&mgr.config().update)).await {
                Ok(s) if s.available => {
                    info!("update available: GAZE {}", s.latest.unwrap_or_default())
                }
                Ok(_) => {}
                Err(e) => tracing::debug!("update check: {e:#}"),
            }
        }
        let st = status();
        let night = (2..5).contains(&Local::now().hour());
        let busy = mgr
            .cameras()
            .iter()
            .any(|c| c.detector().is_some_and(|d| d.status().event_active));
        if st.available
            && mgr.config().update.auto
            && night
            && !busy
            && st.state == "idle"
            && let Err(e) = install().await
        {
            warn!("automatic update failed: {e:#}");
        }
        tokio::time::sleep(Duration::from_secs(300)).await;
    }
}

#[cfg(test)]
mod tests {
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
