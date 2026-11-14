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

// ---------- cameras brought in from another server ----------

/// A camera here whose footage is on another server.
#[derive(Clone)]
pub struct Imported {
    /// Its id and name here.
    pub id: String,
    pub name: String,
    pub server: RemoteServer,
    /// Its id there.
    pub remote_id: String,
}

/// Every imported camera (with its server still in the list).
pub fn imported(cfg: &crate::config::Config) -> Vec<Imported> {
    cfg.cameras
        .iter()
        .filter_map(|c| {
            let src = c.remote.as_ref()?;
            let server = cfg.remotes.iter().find(|r| base(r) == src.server.trim().trim_end_matches('/'))?.clone();
            Some(Imported { id: c.id.clone(), name: c.name.clone(), server, remote_id: src.camera.clone() })
        })
        .collect()
}

pub fn find(cfg: &crate::config::Config, id: &str) -> Option<Imported> {
    imported(cfg).into_iter().find(|i| i.id == id)
}

/// The server's address as saved with an imported camera.
pub fn address(r: &RemoteServer) -> String {
    base(r)
}

/// Ask that server for something (a path with its query, e.g.
/// "api/timeline?…"), passing on the range asked for (video).
pub async fn fetch(r: &RemoteServer, method: reqwest::Method, path: &str, range: Option<&str>, body: Option<Vec<u8>>) -> anyhow::Result<reqwest::Response> {
    let c = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .danger_accept_invalid_certs(r.trust_own_cert)
        .build()?;
    let mut q = c.request(method, format!("{}/{}", base(r), path.trim_start_matches('/'))).bearer_auth(r.token.trim());
    if let Some(range) = range {
        q = q.header(reqwest::header::RANGE, range);
    }
    if let Some(b) = body {
        q = q.header(reqwest::header::CONTENT_TYPE, "application/json").body(b);
    }
    Ok(q.send().await?)
}

/// JSON that came from there, made to read as this server's: its camera
/// id becomes the one here, and addresses of its files point here (they
/// are passed on from there).
pub fn localize(v: &mut serde_json::Value, imp: &Imported) {
    use serde_json::Value;
    match v {
        Value::Object(map) => {
            for (k, x) in map.iter_mut() {
                match x {
                    Value::String(s) if k == "camera" && *s == imp.remote_id => *s = imp.id.clone(),
                    Value::String(s) if k == "camera_name" => *s = imp.name.clone(),
                    _ => localize(x, imp),
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| localize(x, imp)),
        Value::String(s) => {
            if s.starts_with("media/") || s.starts_with("api/") || s.starts_with("/media/") {
                *s = localize_text(s, imp);
            }
        }
        _ => {}
    }
}

/// Addresses in text (a video playlist) pointed here.
pub fn localize_text(s: &str, imp: &Imported) -> String {
    let (r, l) = (crate::library::enc(&imp.remote_id), crate::library::enc(&imp.id));
    let mut out = s.to_string();
    for p in ["media/", "api/hls-file/", "api/hls/", "api/sprite/"] {
        out = out.replace(&format!("{p}{r}/"), &format!("{p}{l}/"));
    }
    out
}

/// Read JSON from there (a list of alerts, a timeline), localized.
pub async fn json(imp: &Imported, path: &str) -> anyhow::Result<serde_json::Value> {
    let resp = tokio::time::timeout(Duration::from_secs(10), fetch(&imp.server, reqwest::Method::GET, path, None, None)).await??;
    if !resp.status().is_success() {
        anyhow::bail!("{} answered {}", imp.server.name, resp.status());
    }
    let mut v: serde_json::Value = resp.json().await?;
    localize(&mut v, imp);
    Ok(v)
}

#[cfg(test)]
mod tests {
    #[test]
    fn localizes() {
        let imp = super::Imported {
            id: "shop-door".into(),
            name: "Shop door".into(),
            server: Default::default(),
            remote_id: "door".into(),
        };
        let mut v = serde_json::json!([{"camera": "door", "camera_name": "Door", "name": "x.mp4",
            "play": {"url": "media/door/rec/x.mp4"}, "event": {"camera": "door", "thumb": "e.jpg"}}]);
        super::localize(&mut v, &imp);
        assert_eq!(v[0]["camera"], "shop-door");
        assert_eq!(v[0]["camera_name"], "Shop door");
        assert_eq!(v[0]["name"], "x.mp4");
        assert_eq!(v[0]["play"]["url"], "media/shop-door/rec/x.mp4");
        assert_eq!(v[0]["event"]["camera"], "shop-door");
        let t = super::localize_text("#EXT-X-MAP:URI=\"../../../../media/door/rec/a.mp4\"", &imp);
        assert!(t.contains("media/shop-door/rec/a.mp4"));
    }
}
