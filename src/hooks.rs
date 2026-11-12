//! Webhooks (Settings → Smart home): each new alert is sent as JSON to the
//! addresses set up — IFTTT, Node-RED, Zapier, a smart plug, your own
//! script. Each webhook can be limited to some cameras and kinds of alert.

use crate::config::Webhook;
use crate::manager::Manager;
use serde_json::json;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

/// Does this webhook want this alert?
pub fn wants(h: &Webhook, camera: &str, labels: &[String], motion: bool) -> bool {
    h.enabled
        && !h.url.trim().is_empty()
        && (h.cameras.is_empty() || h.cameras.iter().any(|c| c == camera))
        && (h.objects.is_empty() || labels.iter().any(|l| h.objects.contains(l)) || (motion && h.objects.iter().any(|o| o == "motion")))
}

pub async fn send(h: &Webhook, body: &serde_json::Value) -> anyhow::Result<()> {
    let client = reqwest::Client::builder().timeout(Duration::from_secs(15)).build()?;
    let r = client.post(h.url.trim()).json(body).send().await?;
    if !r.status().is_success() {
        anyhow::bail!("the address answered {}", r.status());
    }
    Ok(())
}

pub async fn run(mgr: Arc<Manager>) {
    let mut seen: HashSet<String> = HashSet::new();
    let started = chrono::Local::now();
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let cfg = mgr.config();
        if !cfg.webhooks.iter().any(|h| h.enabled) {
            continue;
        }
        let now = chrono::Local::now();
        for rt in mgr.cameras() {
            for e in rt.events.between(now - chrono::Duration::minutes(2), now) {
                // Announced once its picture is there; not what happened before GAZE started.
                let Some(thumb) = e.thumb.as_ref() else { continue };
                if e.start < started - chrono::Duration::seconds(30) || !seen.insert(format!("{}|{}", rt.cfg.id, e.id)) {
                    continue;
                }
                let labels: Vec<String> = e.labels.iter().map(|l| l.label.clone()).collect();
                for h in cfg.webhooks.iter().filter(|h| wants(h, &rt.cfg.id, &labels, e.motion)) {
                    let mut body = json!({
                        "event": "alert",
                        "id": e.id, "camera": rt.cfg.id, "camera_name": rt.cfg.name,
                        "start": e.start, "end": e.end,
                        "labels": e.labels.iter().map(|l| json!({"label": l.label, "score": l.score})).collect::<Vec<_>>(),
                        "what": if labels.is_empty() { "motion".to_string() } else { labels.join(", ") },
                        "notes": e.notes, "plates": e.plates,
                        "faces": e.faces.iter().filter_map(|f| f.name.clone()).collect::<Vec<_>>(),
                        "mode": cfg.modes.effective(crate::notify::is_night(&cfg.notify, now)),
                    });
                    if h.picture
                        && let Ok(jpg) = std::fs::read(rt.events.dir().join(thumb))
                    {
                        use base64::Engine;
                        body["picture_jpeg_base64"] = json!(base64::engine::general_purpose::STANDARD.encode(jpg));
                    }
                    let (h, body) = (h.clone(), body);
                    tokio::spawn(async move {
                        match send(&h, &body).await {
                            Ok(()) => info!(target: "webhook", "\"{}\": sent {} at {}", h.name, body["what"].as_str().unwrap_or(""), body["camera_name"].as_str().unwrap_or("")),
                            Err(e) => warn!(target: "webhook", "\"{}\": {e:#}", h.name),
                        }
                    });
                }
            }
        }
        if seen.len() > 5000 {
            seen.clear();
        }
    }
}
