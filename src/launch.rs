//! Starting GAZE like a normal Windows program: a downloaded GAZE.exe puts
//! itself in its own program folder, with Start menu and desktop shortcuts;
//! the shortcut opens GAZE (the installed app, or the browser), starting it
//! first when it isn't running, and never complains that it already is.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

/// Started from the shortcut (`--open`): open GAZE once it's listening.
pub static OPEN_WHEN_READY: AtomicBool = AtomicBool::new(false);

/// The web port from the settings file (8080 if there's none yet).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn port_of(config: &Path) -> u16 {
    std::fs::read_to_string(config)
        .ok()
        .and_then(|t| {
            t.lines()
                .map(str::trim)
                .find(|l| l.starts_with("listen") && l.contains('='))
                .and_then(|l| l.rsplit(':').next())
                .map(|p| p.trim_matches(|c: char| !c.is_ascii_digit()).to_string())
        })
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080)
}

/// Is a GAZE already answering on this port?
#[cfg_attr(not(windows), allow(dead_code))]
pub fn running(port: u16) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(500)) else {
        return false;
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = s.write_all(b"GET / HTTP/1.0\r\nHost: localhost\r\n\r\n");
    let mut buf = Vec::new();
    let _ = s.take(64 * 1024).read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).contains("<title>GAZE</title>")
}

/// Open GAZE for the person: the installed GAZE app when there is one,
/// otherwise the web page in the default browser.
pub fn open_ui(port: u16, data_dir: &Path) {
    let url = format!("http://localhost:{port}/");
    #[cfg(windows)]
    {
        let target = app_shortcut(data_dir).map(|p| p.display().to_string()).unwrap_or(url);
        let _ = crate::load::std_cmd("explorer").arg(target).spawn();
    }
    #[cfg(not(windows))]
    let _ = (url, data_dir);
}

/// A shortcut made by Edge or Chrome for the installed GAZE app (it starts
/// the browser with `--app-id`).
#[cfg(windows)]
fn app_shortcut(data_dir: &Path) -> Option<PathBuf> {
    let mut places: Vec<PathBuf> = Vec::new();
    if let Some(a) = std::env::var_os("APPDATA") {
        let p = PathBuf::from(a).join("Microsoft\\Windows\\Start Menu\\Programs");
        places.push(p.join("GAZE.lnk"));
        places.push(p.join("Chrome Apps\\GAZE.lnk"));
        places.push(p.join("Edge Apps\\GAZE.lnk"));
    }
    places.push(data_dir.join("gaze-app.lnk"));
    places.into_iter().find(|p| is_app_shortcut(p))
}

#[cfg_attr(not(windows), allow(dead_code))]
fn is_app_shortcut(p: &Path) -> bool {
    let Ok(b) = std::fs::read(p) else { return false };
    let wide: Vec<u8> = "--app-id".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    b.windows(wide.len()).any(|w| w == wide.as_slice()) || b.windows(8).any(|w| w == b"--app-id")
}

/// GAZE's own program folder: %LOCALAPPDATA%\Programs\GAZE.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn program_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Programs").join("GAZE"))
}

/// A freshly downloaded GAZE.exe (no settings next to it, e.g. in
/// Downloads) copies itself into its program folder and starts from there.
/// Returns true when this copy should stop. An existing set-up (settings
/// next to the program) stays where it is.
#[cfg(windows)]
pub fn install_if_downloaded(config_given: bool) -> bool {
    use tracing::info;
    let Ok(exe) = std::env::current_exe() else { return false };
    let Some(here) = exe.parent() else { return false };
    let Some(dest) = program_dir() else { return false };
    if config_given || here.join("nvr.toml").exists() || here.join("nvr-data").exists() {
        return false;
    }
    if same_dir(here, &dest) {
        return false;
    }
    if std::fs::create_dir_all(&dest).is_err() {
        return false;
    }
    let target = dest.join("GAZE.exe");
    let installed_running = dest.join("nvr.toml").exists() && running(port_of(&dest.join("nvr.toml")));
    if installed_running {
        // Already installed and running: just open it.
        open_ui(port_of(&dest.join("nvr.toml")), &dest.join("nvr-data"));
        return true;
    }
    // This download replaces the installed program (the same or newer).
    if std::fs::copy(&exe, &target).is_err() && !target.exists() {
        return false;
    }
    info!("installed GAZE in {}", dest.display());
    crate::load::std_cmd(&target).arg("--open").current_dir(&dest).spawn().is_ok()
}

#[cfg(windows)]
fn same_dir(a: &Path, b: &Path) -> bool {
    let n = |p: &Path| std::fs::canonicalize(p).unwrap_or(p.to_path_buf()).to_string_lossy().to_lowercase();
    n(a) == n(b)
}

/// Start menu shortcut (kept pointing at this program). On the first start
/// and the first after each update, a desktop shortcut too when the desktop
/// has none to this program. An app shortcut Edge or Chrome put on the
/// desktop under the same name is kept aside, to open the app with.
#[cfg(windows)]
pub fn make_shortcuts(data_dir: &Path) {
    let Ok(exe) = std::env::current_exe() else { return };
    let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    // The version that last looked at the desktop.
    let flag = data_dir.join("desktop-shortcut");
    let version = env!("CARGO_PKG_VERSION");
    let desktop = std::fs::read_to_string(&flag).map(|v| v.trim() != version).unwrap_or(true);
    let script = r#"
$ErrorActionPreference = 'Stop'
$w = New-Object -ComObject WScript.Shell
function Make($path) {
  $s = $w.CreateShortcut($path)
  $s.TargetPath = $env:GAZE_EXE; $s.Arguments = '--open'; $s.WorkingDirectory = $env:GAZE_DIR
  $s.IconLocation = "$($env:GAZE_EXE),0"; $s.Description = 'GAZE - home cameras'; $s.Save()
}
$menu = Join-Path ([Environment]::GetFolderPath('Programs')) 'GAZE'
New-Item -ItemType Directory -Force -Path $menu | Out-Null
Make (Join-Path $menu 'GAZE.lnk')
if ($env:GAZE_DESKTOP -eq '1') {
  # Already a shortcut to this program on the desktop (any name)?
  $have = $false
  foreach ($dir in @([Environment]::GetFolderPath('Desktop'), [Environment]::GetFolderPath('CommonDesktopDirectory'))) {
    if (-not $dir -or -not (Test-Path $dir)) { continue }
    foreach ($l in Get-ChildItem -LiteralPath $dir -Filter *.lnk -ErrorAction SilentlyContinue) {
      try { if ($w.CreateShortcut($l.FullName).TargetPath -ieq $env:GAZE_EXE) { $have = $true } } catch {}
    }
  }
  if ($have) { exit 0 }
  $d = Join-Path ([Environment]::GetFolderPath('Desktop')) 'GAZE.lnk'
  if (Test-Path $d) {
    $t = [Text.Encoding]::Unicode.GetString([IO.File]::ReadAllBytes($d))
    if ($t.Contains('--app-id')) { Copy-Item $d (Join-Path $env:GAZE_DATA 'gaze-app.lnk') -Force }
  }
  Make $d
}
"#;
    let data = data_dir.to_path_buf();
    std::thread::spawn(move || {
        let _ = std::fs::create_dir_all(&data);
        let ok = crate::load::std_cmd("powershell")
            .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
            .env("GAZE_EXE", &exe)
            .env("GAZE_DIR", &dir)
            .env("GAZE_DATA", &data)
            .env("GAZE_DESKTOP", if desktop { "1" } else { "0" })
            .status()
            .is_ok_and(|s| s.success());
        if ok && desktop {
            let _ = std::fs::write(&flag, version);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_from_settings() {
        let d = std::env::temp_dir().join(format!("gaze-launch-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("nvr.toml");
        std::fs::write(&f, "[server]\nlisten = \"0.0.0.0:8096\"\n").unwrap();
        assert_eq!(port_of(&f), 8096);
        assert_eq!(port_of(&d.join("none.toml")), 8080);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn app_shortcut_needs_app_id() {
        let d = std::env::temp_dir().join(format!("gaze-lnk-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let a = d.join("a.lnk");
        let wide: Vec<u8> = "x --profile-directory=Default --app-id=abc".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        std::fs::write(&a, wide).unwrap();
        assert!(is_app_shortcut(&a));
        let b = d.join("b.lnk");
        std::fs::write(&b, b"GAZE.exe --open").unwrap();
        assert!(!is_app_shortcut(&b));
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[cfg(test)]
mod live_check {
    /// Answers "is GAZE running" only for a GAZE page.
    #[test]
    fn running_needs_gaze() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for (i, s) in l.incoming().enumerate() {
                use std::io::{Read, Write};
                let mut s = s.unwrap();
                let mut b = [0u8; 512];
                let _ = s.read(&mut b);
                let body = if i == 0 { "<title>GAZE</title>" } else { "<title>Other</title>" };
                let _ = write!(s, "HTTP/1.0 200 OK\r\n\r\n{body}");
            }
        });
        assert!(super::running(port));
        assert!(!super::running(port));
    }
}
