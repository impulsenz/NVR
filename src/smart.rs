//! Drive health (Settings → System → Drive health): asks `smartctl`
//! (smartmontools, installed separately) how every drive on this computer
//! is doing, and tells you when one starts to fail, before the footage on
//! it is lost. Checked every few hours; read-only; recording never depends
//! on it. If smartctl isn't installed, or isn't allowed to read the drives
//! (it usually needs administrator / root rights), the page says so.

use crate::manager::Manager;
use crate::sync::MutexExt;
use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Hotter than this is a warning (spinning drives are rated to about 60 °C).
const HOT_C: i64 = 60;
const EVERY: Duration = Duration::from_secs(6 * 3600);

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct Drive {
    pub name: String,
    pub model: String,
    pub kind: String,
    pub size_gb: Option<u64>,
    pub temperature_c: Option<i64>,
    pub power_on_hours: Option<u64>,
    /// "ok", "warning", "failing", or "unknown" (the drive gives no SMART data).
    pub health: String,
    pub problems: Vec<String>,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Overview {
    /// smartctl was found.
    pub installed: bool,
    pub drives: Vec<Drive>,
    pub checked: Option<DateTime<Local>>,
    /// Why there is nothing to show, when there isn't.
    pub note: String,
    /// GAZE can install smartmontools itself (Windows).
    pub can_install: bool,
    /// Elsewhere: the command that installs it.
    pub install_hint: String,
    /// An install is running.
    pub installing: bool,
}

pub static STATUS: Mutex<Option<Overview>> = Mutex::new(None);

/// smartmontools 7.4 for Windows, from the project's own download address
/// (smartmontools.org links to it); it is only run if it matches this
/// SHA-256 fingerprint.
const WIN_INSTALLER_URL: &str = "https://downloads.sourceforge.net/project/smartmontools/smartmontools/7.4/smartmontools-7.4-1.win32-setup.exe";
const WIN_INSTALLER_SHA256: &str = "76880e7303403667739066782b1c458ede2b4462a8ebd6aeacf97efecd4bdde7";
static INSTALLING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn overview() -> Overview {
    STATUS.locked().clone().unwrap_or_default()
}

fn raw(attrs: &[Value], id: u64) -> Option<u64> {
    attrs.iter().find(|a| a["id"].as_u64() == Some(id)).and_then(|a| a["raw"]["value"].as_u64())
}

/// One drive from `smartctl -a -j` output. Pure, so it can be tested.
pub fn parse(name: &str, json: &str) -> Result<Drive> {
    let j: Value = serde_json::from_str(json).context("not JSON")?;
    let mut d = Drive { name: name.to_string(), health: "unknown".into(), ..Default::default() };
    d.model = j["model_name"].as_str().unwrap_or("").to_string();
    d.kind = if j["nvme_smart_health_information_log"].is_object() {
        "NVMe".into()
    } else if j["rotation_rate"].as_u64().is_some_and(|r| r > 0) {
        "HDD".into()
    } else if j["ata_smart_attributes"].is_object() {
        "SSD".into()
    } else {
        j["device"]["type"].as_str().unwrap_or("").to_string()
    };
    d.size_gb = j["user_capacity"]["bytes"].as_u64().map(|b| b / 1_000_000_000);
    d.temperature_c = j["temperature"]["current"].as_i64();
    d.power_on_hours = j["power_on_time"]["hours"].as_u64();

    // smartctl says why it couldn't read the drive.
    let errors: Vec<String> = j["smartctl"]["messages"]
        .as_array()
        .map(|m| m.iter().filter(|x| x["severity"] == "error").filter_map(|x| x["string"].as_str().map(String::from)).collect())
        .unwrap_or_default();
    let Some(passed) = j["smart_status"]["passed"].as_bool() else {
        d.note = errors.first().cloned().unwrap_or_else(|| "this drive gives no health data (a RAID volume, USB bridge or virtual disk often doesn't)".into());
        return Ok(d);
    };

    let mut failing = !passed;
    let mut warn: Vec<String> = Vec::new();
    if !passed {
        d.problems.push("The drive reports that it is failing: copy its footage off now".into());
    }
    if let Some(attrs) = j["ata_smart_attributes"]["table"].as_array() {
        for (id, what) in [(5, "reallocated sectors"), (197, "sectors waiting to be reallocated"), (198, "sectors that can't be read")] {
            if let Some(n) = raw(attrs, id).filter(|n| *n > 0) {
                warn.push(format!("{n} {what}"));
            }
        }
    }
    if let Some(n) = j["nvme_smart_health_information_log"].as_object() {
        if n.get("critical_warning").and_then(|v| v.as_u64()).is_some_and(|v| v != 0) {
            failing = true;
            d.problems.push("The drive raised a critical warning".into());
        }
        let spare = n.get("available_spare").and_then(|v| v.as_u64());
        let limit = n.get("available_spare_threshold").and_then(|v| v.as_u64());
        if let (Some(s), Some(l)) = (spare, limit)
            && s < l
        {
            failing = true;
            d.problems.push(format!("Spare capacity is down to {s}%"));
        }
        if let Some(p) = n.get("percentage_used").and_then(|v| v.as_u64()).filter(|p| *p >= 90) {
            warn.push(format!("{p}% of its rated life used"));
        }
        if let Some(e) = n.get("media_errors").and_then(|v| v.as_u64()).filter(|e| *e > 0) {
            warn.push(format!("{e} media errors"));
        }
    }
    if let Some(t) = d.temperature_c.filter(|t| *t >= HOT_C) {
        warn.push(format!("running hot ({t} °C)"));
    }
    d.problems.extend(warn.iter().map(|w| {
        let mut s = w.clone();
        if let Some(c) = s.get_mut(0..1) {
            c.make_ascii_uppercase();
        }
        s
    }));
    d.health = if failing {
        "failing"
    } else if !warn.is_empty() {
        "warning"
    } else {
        "ok"
    }
    .into();
    Ok(d)
}

fn program() -> String {
    #[cfg(windows)]
    {
        let p = r"C:\Program Files\smartmontools\bin\smartctl.exe";
        if std::path::Path::new(p).exists() {
            return p.to_string();
        }
    }
    "smartctl".into()
}

async fn smartctl(args: &[&str]) -> Result<String> {
    let out = tokio::time::timeout(Duration::from_secs(30), crate::load::cmd(program()).args(args).kill_on_drop(true).output())
        .await
        .context("smartctl took too long")?
        .context("smartctl")?;
    // (Its exit code is a bit mask of findings, not just success or failure.)
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Look at every drive smartctl can see.
pub async fn scan() -> Overview {
    let mut o = Overview { checked: Some(Local::now()), can_install: cfg!(windows), installing: INSTALLING.load(std::sync::atomic::Ordering::Relaxed), ..Default::default() };
    if !cfg!(windows) {
        o.install_hint = "sudo apt install smartmontools   (or your distribution's package manager)".into();
    }
    let list = match smartctl(&["--scan", "-j"]).await {
        Ok(t) => t,
        Err(e) => {
            let missing = e.root_cause().to_string().to_lowercase().contains("no such file") || e.root_cause().to_string().to_lowercase().contains("cannot find");
            o.note = if missing {
                "smartctl isn't installed. smartmontools is the free, standard program that reads a drive's own health report.".into()
            } else {
                format!("{e:#}")
            };
            return o;
        }
    };
    o.installed = true;
    let devices: Vec<(String, String)> = serde_json::from_str::<Value>(&list)
        .ok()
        .and_then(|v| v["devices"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|d| Some((d["name"].as_str()?.to_string(), d["type"].as_str().unwrap_or("auto").to_string())))
        .collect();
    if devices.is_empty() {
        o.note = "smartctl found no drives (it usually needs administrator / root rights to see them, and virtual disks have no health data).".into();
        return o;
    }
    for (name, kind) in devices {
        match smartctl(&["-a", "-j", "-d", &kind, &name]).await.and_then(|t| parse(&name, &t)) {
            Ok(d) => o.drives.push(d),
            Err(e) => o.drives.push(Drive { name, health: "unknown".into(), note: format!("{e:#}"), ..Default::default() }),
        }
    }
    o
}

/// Install smartmontools (Windows): download the installer, check its
/// fingerprint, run it quietly, then look at the drives. Elsewhere the
/// system's package manager does it (the page shows the command).
pub async fn install() -> Result<Overview> {
    use std::sync::atomic::Ordering;
    if !cfg!(windows) {
        anyhow::bail!("install smartmontools with your system's package manager, e.g. sudo apt install smartmontools");
    }
    if INSTALLING.swap(true, Ordering::SeqCst) {
        anyhow::bail!("it is being installed already");
    }
    let res = install_windows().await;
    INSTALLING.store(false, Ordering::SeqCst);
    res?;
    Ok(refresh().await)
}

async fn install_windows() -> Result<()> {
    use sha2::Digest;
    let file = std::env::temp_dir().join("gaze-smartmontools-setup.exe");
    let job = crate::jobs::start("download", "Installing smartmontools (drive health)");
    let part = file.with_extension("part");
    let mut resp = crate::jobs::download_client()?.get(WIN_INSTALLER_URL).send().await.context("can't reach the smartmontools download")?.error_for_status()?;
    let total = resp.content_length().unwrap_or(0);
    let mut out = tokio::fs::File::create(&part).await?;
    let (mut got, mut hash) = (0u64, sha2::Sha256::new());
    while let Some(chunk) = resp.chunk().await? {
        tokio::io::AsyncWriteExt::write_all(&mut out, &chunk).await?;
        hash.update(&chunk);
        got += chunk.len() as u64;
        job.bytes(got, total);
    }
    tokio::io::AsyncWriteExt::flush(&mut out).await?;
    drop(out);
    let print: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if print != WIN_INSTALLER_SHA256 {
        let _ = std::fs::remove_file(&part);
        anyhow::bail!("the download isn't the smartmontools installer GAZE expects (its fingerprint differs): nothing was run");
    }
    tokio::fs::rename(&part, &file).await?;
    job.step("Running the installer (it may ask for administrator permission)");
    // /S: no questions asked. Installs to C:\Program Files\smartmontools.
    let run = tokio::time::timeout(Duration::from_secs(240), crate::load::cmd(&file).arg("/S").kill_on_drop(true).status()).await;
    let _ = std::fs::remove_file(&file);
    match run {
        Err(_) => anyhow::bail!("the installer didn't finish (it may be waiting for administrator permission on the screen of this computer)"),
        Ok(Err(e)) => anyhow::bail!("the installer couldn't start: {e}"),
        Ok(Ok(_)) => {}
    }
    if smartctl(&["--version"]).await.map(|t| t.contains("smartctl")).unwrap_or(false) {
        Ok(())
    } else {
        anyhow::bail!("the installer ran but smartctl isn't there: it needs administrator rights. Run GAZE as administrator, or install smartmontools from smartmontools.org yourself")
    }
}

/// Rescan now (the page's button) and keep the answer.
pub async fn refresh() -> Overview {
    let o = scan().await;
    *STATUS.locked() = Some(o.clone());
    o
}

/// Runs for as long as GAZE does: a look soon after start, then every few hours.
pub async fn run(mgr: Arc<Manager>) {
    tokio::time::sleep(Duration::from_secs(90)).await;
    let mut told: HashSet<String> = HashSet::new();
    loop {
        let o = refresh().await;
        let cfg = mgr.config();
        let n = &cfg.notify;
        if n.enabled && n.health && n.delivers() {
            for d in o.drives.iter().filter(|d| d.health == "failing" || d.health == "warning") {
                let key = format!("{}|{}|{}", d.name, d.model, d.problems.join(";"));
                if !told.insert(key) {
                    continue;
                }
                let bad = d.health == "failing";
                let msg = crate::notify::Message {
                    title: format!("{}: drive {} {}", cfg.server.name, d.name, if bad { "is failing" } else { "needs attention" }),
                    body: format!("{} {}\n{}", d.model, d.kind, d.problems.join("\n")),
                    tags: if bad { "rotating_light" } else { "warning" },
                    priority: if bad { 5 } else { 4 },
                    click: None,
                    picture: None,
                    actions: None,
                };
                if let Err(e) = crate::notify::send(n, msg).await {
                    tracing::warn!("drive health: notification: {e:#}");
                }
            }
        }
        tokio::time::sleep(EVERY).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ata(attrs: Value, passed: bool, temp: i64) -> String {
        json!({"model_name": "ST4000VX016", "user_capacity": {"bytes": 4_000_787_030_016u64}, "rotation_rate": 5900,
               "smart_status": {"passed": passed}, "temperature": {"current": temp}, "power_on_time": {"hours": 12345},
               "ata_smart_attributes": {"table": attrs}})
        .to_string()
    }

    fn attr(id: u64, raw: u64) -> Value {
        json!({"id": id, "name": "x", "raw": {"value": raw, "string": raw.to_string()}})
    }

    #[test]
    fn a_healthy_hard_drive() {
        let d = parse("/dev/sda", &ata(json!([attr(5, 0), attr(197, 0), attr(198, 0)]), true, 38)).unwrap();
        assert_eq!((d.health.as_str(), d.kind.as_str()), ("ok", "HDD"));
        assert_eq!((d.size_gb, d.temperature_c, d.power_on_hours), (Some(4000), Some(38), Some(12345)));
        assert!(d.problems.is_empty());
    }

    #[test]
    fn sectors_going_bad_is_a_warning_and_a_failed_status_is_failing() {
        let d = parse("/dev/sda", &ata(json!([attr(5, 12), attr(197, 3)]), true, 41)).unwrap();
        assert_eq!(d.health, "warning");
        assert_eq!(d.problems, vec!["12 reallocated sectors", "3 sectors waiting to be reallocated"]);
        let f = parse("/dev/sda", &ata(json!([]), false, 41)).unwrap();
        assert_eq!(f.health, "failing");
        assert!(f.problems[0].contains("failing"));
    }

    #[test]
    fn a_hot_drive_warns() {
        let d = parse("/dev/sda", &ata(json!([attr(5, 0)]), true, 63)).unwrap();
        assert_eq!(d.health, "warning");
        assert!(d.problems[0].contains("63"));
    }

    #[test]
    fn nvme_wear_and_critical_warnings() {
        let mk = |warn: u64, used: u64, spare: u64| {
            json!({"model_name": "WD Red SN700", "smart_status": {"passed": true}, "temperature": {"current": 45},
                   "nvme_smart_health_information_log": {"critical_warning": warn, "percentage_used": used, "available_spare": spare,
                                                         "available_spare_threshold": 10, "media_errors": 0}})
            .to_string()
        };
        assert_eq!(parse("/dev/nvme0", &mk(0, 20, 100)).unwrap().health, "ok");
        assert_eq!(parse("/dev/nvme0", &mk(0, 95, 100)).unwrap().health, "warning");
        assert_eq!(parse("/dev/nvme0", &mk(1, 20, 100)).unwrap().health, "failing");
        assert_eq!(parse("/dev/nvme0", &mk(0, 20, 5)).unwrap().health, "failing");
        assert_eq!(parse("/dev/nvme0", &mk(0, 20, 100)).unwrap().kind, "NVMe");
    }

    /// What smartctl 7.4 really prints for a disk it can't read (seen on a
    /// virtual disk): not an error for us, just "unknown" with its reason.
    #[test]
    fn a_disk_with_no_health_data_is_unknown_with_the_reason() {
        let real = r#"{"json_format_version":[1,0],"smartctl":{"version":[7,4],"messages":[{"string":"/dev/vda: Unable to detect device type","severity":"error"}],"exit_status":1},"local_time":{"time_t":1}}"#;
        let d = parse("/dev/vda", real).unwrap();
        assert_eq!(d.health, "unknown");
        assert_eq!(d.note, "/dev/vda: Unable to detect device type");
        assert!(parse("/dev/x", "garbage").is_err());
    }

    #[tokio::test]
    async fn scanning_without_drives_or_without_smartctl_says_why() {
        // On a computer with no readable drives this is a note, never a panic.
        let o = scan().await;
        assert!(o.installed || o.note.contains("smartctl"));
        if o.installed && o.drives.is_empty() {
            assert!(o.note.contains("no drives"));
        }
    }
}
