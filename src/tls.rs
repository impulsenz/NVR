//! The secure (https) web interface, next to the plain one: a certificate
//! of the user's own (e.g. from Tailscale), or one GAZE makes for this
//! computer's name and addresses (browsers warn about that one once).

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{info, warn};

/// The names a certificate GAZE makes is for: this computer's name,
/// localhost and every address it has.
fn names() -> Vec<String> {
    let mut n = vec!["localhost".to_string(), "127.0.0.1".to_string()];
    if let Some(h) = std::env::var("COMPUTERNAME").ok().or_else(|| std::fs::read_to_string("/etc/hostname").ok()) {
        let h = h.trim().to_lowercase();
        if !h.is_empty() {
            n.push(h.clone());
            n.push(format!("{h}.local"));
        }
    }
    if let Ok(ifs) = if_addrs::get_if_addrs() {
        for i in ifs {
            if !i.is_loopback() {
                n.push(i.ip().to_string());
            }
        }
    }
    n.sort();
    n.dedup();
    n
}

fn own_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("https")
}

/// The certificate and key (PEM) to use, and a word on where they came from.
pub fn certificate(data_dir: &Path, cfg: &crate::config::ServerConfig) -> Result<(Vec<u8>, Vec<u8>, String)> {
    if !cfg.https_cert.trim().is_empty() && !cfg.https_key.trim().is_empty() {
        let c = std::fs::read(cfg.https_cert.trim()).with_context(|| format!("reading the certificate {}", cfg.https_cert.trim()))?;
        let k = std::fs::read(cfg.https_key.trim()).with_context(|| format!("reading the key {}", cfg.https_key.trim()))?;
        return Ok((c, k, "your certificate".into()));
    }
    let dir = own_dir(data_dir);
    let (cp, kp, np) = (dir.join("cert.pem"), dir.join("key.pem"), dir.join("names.txt"));
    let want = names().join("\n");
    // Made again when this computer gets a new name or address.
    if cp.is_file() && kp.is_file() && std::fs::read_to_string(&np).is_ok_and(|h| h == want) {
        return Ok((std::fs::read(&cp)?, std::fs::read(&kp)?, "GAZE's own certificate".into()));
    }
    let mut params = rcgen::CertificateParams::new(names())?;
    params.distinguished_name.push(rcgen::DnType::CommonName, "GAZE");
    params.distinguished_name.push(rcgen::DnType::OrganizationName, "GAZE (this computer)");
    params.not_before = rcgen::date_time_ymd(2025, 1, 1);
    params.not_after = rcgen::date_time_ymd(2045, 1, 1);
    let key = rcgen::KeyPair::generate()?;
    let cert = params.self_signed(&key)?;
    std::fs::create_dir_all(&dir)?;
    std::fs::write(&cp, cert.pem())?;
    std::fs::write(&kp, key.serialize_pem())?;
    std::fs::write(&np, &want)?;
    info!("made a certificate for https: {}", names().join(", "));
    Ok((cert.pem().into_bytes(), key.serialize_pem().into_bytes(), "GAZE's own certificate".into()))
}

/// What the settings page shows about https.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Status {
    pub on: bool,
    pub port: u16,
    pub source: String,
    pub error: Option<String>,
    /// A trusted certificate (no browser warning).
    pub trusted: bool,
}

static STATUS: std::sync::Mutex<Option<Status>> = std::sync::Mutex::new(None);

pub fn status() -> Status {
    STATUS.lock().unwrap().clone().unwrap_or_default()
}

/// Serve the web interface over https on `port` until GAZE stops.
pub async fn serve(mgr: Arc<crate::manager::Manager>, bind: String, port: u16) {
    let cfg = mgr.config().server;
    let set = |s: Status| *STATUS.lock().unwrap() = Some(s);
    let (cert, key, source) = match certificate(&mgr.data_dir, &cfg) {
        Ok(x) => x,
        Err(e) => {
            warn!("https is off: {e:#}");
            set(Status { port, error: Some(format!("{e:#}")), ..Default::default() });
            return;
        }
    };
    let rustls = match axum_server::tls_rustls::RustlsConfig::from_pem(cert, key).await {
        Ok(c) => c,
        Err(e) => {
            warn!("https is off: the certificate can't be used ({e})");
            set(Status { port, error: Some(format!("the certificate can't be used: {e}")), ..Default::default() });
            return;
        }
    };
    let addr: std::net::SocketAddr = match format!("{bind}:{port}").parse() {
        Ok(a) => a,
        Err(e) => {
            set(Status { port, error: Some(e.to_string()), ..Default::default() });
            return;
        }
    };
    let trusted = !cfg.https_cert.trim().is_empty();
    set(Status { on: true, port, source: source.clone(), error: None, trusted });
    // A Tailscale certificate lasts 90 days: fetched again every week and
    // swapped in without a restart.
    if cfg.https_cert.contains("tailscale.crt") {
        let (r, dir) = (rustls.clone(), mgr.data_dir.clone());
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(7 * 86_400)).await;
                match from_tailscale(&dir).await {
                    Ok((c, k, _)) => {
                        if let Err(e) = r.reload_from_pem_file(&c, &k).await {
                            warn!("https: the renewed Tailscale certificate can't be used: {e}");
                        } else {
                            info!("https: Tailscale certificate renewed");
                        }
                    }
                    Err(e) => warn!("https: renewing the Tailscale certificate: {e:#}"),
                }
            }
        });
    }
    info!("secure web interface on https://{addr} ({source})");
    // Marked as https for the rest of GAZE (secure sign-in cookie, links).
    let app = crate::web::router(mgr.clone()).layer(axum::middleware::from_fn(mark_https));
    if let Err(e) = axum_server::bind_rustls(addr, rustls)
        .serve(app.into_make_service_with_connect_info::<std::net::SocketAddr>())
        .await
    {
        warn!("https stopped: {e}");
        set(Status { port, error: Some(format!("can't use port {port}: {e}")), ..Default::default() });
    }
}

async fn mark_https(mut req: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {
    req.headers_mut().insert("x-gaze-https", axum::http::HeaderValue::from_static("1"));
    next.run(req).await
}

/// A certificate from Tailscale for this computer's Tailscale name
/// (trusted everywhere, and the way in from outside the home).
pub async fn from_tailscale(data_dir: &Path) -> Result<(String, String, String)> {
    let exe = if cfg!(windows) { "C:\\Program Files\\Tailscale\\tailscale.exe" } else { "tailscale" };
    let exe = if Path::new(exe).exists() { exe.to_string() } else { "tailscale".to_string() };
    let out = tokio::process::Command::new(&exe).args(["status", "--json"]).output().await
        .context("Tailscale isn't installed on this computer (tailscale.com/download)")?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).context("Tailscale didn't answer — is it signed in?")?;
    let name = v["Self"]["DNSName"].as_str().unwrap_or("").trim_end_matches('.').to_string();
    if name.is_empty() {
        anyhow::bail!("this computer has no Tailscale name yet — turn on MagicDNS and HTTPS certificates in the Tailscale admin page");
    }
    let dir = own_dir(data_dir);
    std::fs::create_dir_all(&dir)?;
    let (c, k) = (dir.join("tailscale.crt"), dir.join("tailscale.key"));
    let out = tokio::process::Command::new(&exe)
        .args(["cert", "--cert-file"]).arg(&c).arg("--key-file").arg(&k).arg(&name)
        .output().await?;
    if !out.status.success() {
        let e = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("Tailscale couldn't make a certificate: {} (turn on HTTPS certificates in the Tailscale admin page, DNS tab)", e.trim());
    }
    Ok((c.to_string_lossy().into(), k.to_string_lossy().into(), name))
}
