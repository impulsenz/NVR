//! Other GAZE servers (Settings → System): their cameras on this Live page,
//! e.g. home and a holiday house, or home and the shop. Each is reached at
//! its own address with an API token made on that server.

use crate::config::RemoteServer;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

fn client(r: &RemoteServer, secs: u64) -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(secs))
        // A GAZE with its own certificate (not a trusted one), when allowed.
        .danger_accept_invalid_certs(r.trust_own_cert)
        .build()
}

fn base(r: &RemoteServer) -> String {
    r.url.trim().trim_end_matches('/').to_string()
}

/// One server's cameras (or why it can't be reached), kept a few seconds;
/// only those chosen to show here (all when none are chosen).
pub async fn cameras(r: &RemoteServer) -> serde_json::Value {
    let mut v = cameras_cached(r).await;
    if !r.cameras.is_empty()
        && let Some(a) = v["cameras"].as_array_mut()
    {
        a.retain(|c| c["id"].as_str().is_some_and(|id| r.cameras.iter().any(|x| x == id)));
    }
    v
}

/// Every camera it offers, asked now.
pub async fn cameras_fresh(r: &RemoteServer) -> serde_json::Value {
    CACHE.lock().unwrap().remove(&format!("{}|{}", base(r), r.token));
    cameras_cached(r).await
}

static CACHE: LazyLock<Mutex<HashMap<String, (Instant, serde_json::Value)>>> = LazyLock::new(Default::default);

async fn cameras_cached(r: &RemoteServer) -> serde_json::Value {
    let key = format!("{}|{}", base(r), r.token);
    if let Some((t, v)) = CACHE.lock().unwrap().get(&key)
        && t.elapsed() < Duration::from_secs(10)
    {
        return v.clone();
    }
    let res: anyhow::Result<serde_json::Value> = async {
        let resp = client(r, 8)?
            .get(format!("{}/api/status", base(r)))
            .bearer_auth(r.token.trim())
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("can't reach it ({})", if e.is_connect() || e.is_timeout() { "check the address, and that it's switched on" } else if e.to_string().contains("certificate") { "its certificate isn't trusted — tick \"trust its own certificate\"" } else { "connection failed" }))?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            anyhow::bail!("the token was refused — make a new one on that GAZE (👤 menu → API tokens)");
        }
        if !resp.status().is_success() {
            anyhow::bail!("it answered {}", resp.status());
        }
        let s: serde_json::Value = resp.json().await?;
        let cams: Vec<serde_json::Value> = s["cameras"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|c| c["enabled"].as_bool().unwrap_or(true))
                    .map(|c| serde_json::json!({"id": c["id"], "name": c["name"], "online": c["streams"]["main"]["state"] == "connected", "has_sub": c["streams"]["sub"].is_object()}))
                    .collect()
            })
            .unwrap_or_default();
        Ok(serde_json::json!({"ok": true, "server": s["name"], "cameras": cams}))
    }
    .await;
    let v = res.unwrap_or_else(|e| serde_json::json!({"ok": false, "error": format!("{e:#}"), "cameras": []}));
    CACHE.lock().unwrap().insert(key, (Instant::now(), v.clone()));
    v
}

/// A live picture of one of its cameras.
pub async fn frame(r: &RemoteServer, camera: &str) -> anyhow::Result<bytes::Bytes> {
    if !crate::storage::valid_component(camera) {
        anyhow::bail!("bad camera id");
    }
    let resp = client(r, 10)?
        .get(format!("{}/live/{}/frame.jpg", base(r), camera))
        .bearer_auth(r.token.trim())
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("it answered {}", resp.status());
    }
    Ok(resp.bytes().await?)
}

/// One of its cameras, moving (its picture stream).
pub async fn mjpeg(r: &RemoteServer, camera: &str) -> anyhow::Result<reqwest::Response> {
    if !crate::storage::valid_component(camera) {
        anyhow::bail!("bad camera id");
    }
    let c = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .danger_accept_invalid_certs(r.trust_own_cert)
        .build()?;
    let resp = c
        .get(format!("{}/live/{}/stream.mjpeg?fps=8", base(r), camera))
        .bearer_auth(r.token.trim())
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("it answered {}", resp.status());
    }
    Ok(resp)
}
