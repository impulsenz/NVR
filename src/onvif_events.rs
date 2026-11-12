//! The camera's own detection (ONVIF events): what the camera itself
//! reports — its person / vehicle detection, line crossing, a doorbell
//! press, tampering, and (if wanted) its own motion detection — becomes an
//! alert in GAZE, as if GAZE had seen it. Uses a pull-point subscription
//! (the camera keeps the events until asked), which works through NAT and
//! firewalls, unlike the camera calling GAZE.

use crate::config::CameraConfig;
use crate::manager::Manager;
use crate::onvif::{Target, attr_values, capability_xaddr, security_header, tag_text};
use anyhow::{Result, bail};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{info, warn};

/// What the camera reported last, per camera (for its settings page).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Status {
    pub connected: bool,
    pub error: Option<String>,
    pub last: Option<String>,
    pub last_at: Option<chrono::DateTime<chrono::Local>>,
}
static STATUS: Mutex<Option<HashMap<String, Status>>> = Mutex::new(None);
pub fn status(camera: &str) -> Option<Status> {
    STATUS.lock().unwrap().as_ref()?.get(camera).cloned()
}
fn set(camera: &str, f: impl FnOnce(&mut Status)) {
    let mut s = STATUS.lock().unwrap();
    f(s.get_or_insert_with(HashMap::new).entry(camera.to_string()).or_default());
}

/// What an event means for GAZE: (label, note). None = not used.
pub fn meaning(topic: &str, motion_too: bool) -> Option<(&'static str, &'static str)> {
    let t = topic.to_lowercase();
    let has = |w: &[&str]| w.iter().any(|x| t.contains(x));
    if has(&["doorbell", "visitorcall", "visitor", "callbutton", "ring"]) {
        Some(("doorbell", "Doorbell pressed"))
    } else if has(&["people", "person", "human", "face"]) {
        Some(("person", "The camera detected a person"))
    } else if has(&["vehicle", "car", "licenseplate"]) {
        Some(("car", "The camera detected a vehicle"))
    } else if has(&["pet", "animal", "dog", "cat"]) {
        Some(("dog", "The camera detected an animal"))
    } else if has(&["linedetector", "linecross", "crossed", "tripwire"]) {
        Some(("motion", "The camera saw a line being crossed"))
    } else if has(&["fielddetector", "intrusion", "regionentrance", "objectsinside"]) {
        Some(("motion", "The camera saw someone enter an area"))
    } else if has(&["tamper", "videoloss", "globalscenechange", "defocus"]) {
        Some(("motion", "The camera may have been covered, moved or tampered with"))
    } else if motion_too && has(&["motion", "cellmotion"]) {
        Some(("motion", ""))
    } else {
        None
    }
}

/// The notification messages in a PullMessages answer: (topic, active).
pub fn parse_messages(xml: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    for part in xml.split("NotificationMessage>").skip(1).step_by(2) {
        let topic = tag_text(part, "Topic").unwrap_or_default();
        if topic.is_empty() {
            continue;
        }
        // The state is in the Data block (Source names the video source).
        let data = part.find("Data").map(|i| &part[i..]).unwrap_or(part);
        let vals = attr_values(data, "SimpleItem", "Value");
        let on = vals.iter().any(|v| matches!(v.to_lowercase().as_str(), "true" | "1" | "on" | "active"));
        out.push((topic.rsplit('}').next().unwrap_or(&topic).trim().to_string(), on));
    }
    out
}

struct Client {
    http: reqwest::Client,
    t: Target,
}

impl Client {
    /// A SOAP call with WS-Addressing (pull points need it).
    async fn call(&self, url: &str, action: &str, body: &str) -> Result<String> {
        let created = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
        let mut nonce = [0u8; 16];
        use rand_core::RngCore;
        rand_core::OsRng.fill_bytes(&mut nonce);
        let sec = security_header(&self.t.user, &self.t.pass, &nonce, &created);
        // The security header element, with WS-Addressing next to it.
        let wsa = format!("<wsa:Action>{action}</wsa:Action><wsa:To>{url}</wsa:To>");
        let sec = if sec.is_empty() { format!("<s:Header>{wsa}</s:Header>") } else { sec.replacen("<s:Header>", &format!("<s:Header>{wsa}"), 1) };
        let env = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope" xmlns:wsa="http://www.w3.org/2005/08/addressing" xmlns:tt="http://www.onvif.org/ver10/schema" xmlns:wsnt="http://docs.oasis-open.org/wsn/b-2" xmlns:tev="http://www.onvif.org/ver10/events/wsdl">{sec}<s:Body>{body}</s:Body></s:Envelope>"#
        );
        let r = self
            .http
            .post(url)
            .header("Content-Type", format!("application/soap+xml; charset=utf-8; action=\"{action}\""))
            .body(env)
            .send()
            .await?;
        let status = r.status();
        let text = r.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("{}", tag_text(&text, "Text").unwrap_or_else(|| status.to_string()));
        }
        Ok(text)
    }

    /// Our address for the camera's event service and subscription
    /// (cameras sometimes report an internal address: keep its path).
    fn fix(&self, u: &str) -> String {
        match u.split_once("://").and_then(|(_, r)| r.find('/').map(|i| r[i..].to_string())) {
            Some(path) => format!("http://{}:{}{}", self.t.host, self.t.port, path),
            None => u.to_string(),
        }
    }
}

/// One camera: subscribe, then keep pulling events until it fails or the
/// settings change.
async fn watch(mgr: &Arc<Manager>, cam: &CameraConfig) -> Result<()> {
    let t = Target::from(cam, &cam.ptz)?;
    let c = Client { http: reqwest::Client::builder().timeout(Duration::from_secs(25)).build()?, t };
    let caps = c
        .call(&c.t.device_url(), "http://www.onvif.org/ver10/device/wsdl/GetCapabilities",
            r#"<GetCapabilities xmlns="http://www.onvif.org/ver10/device/wsdl"><Category>Events</Category></GetCapabilities>"#)
        .await?;
    let events = capability_xaddr(&caps, "Events").ok_or_else(|| anyhow::anyhow!("this camera doesn't offer ONVIF events"))?;
    let events = c.fix(&events);
    let sub = c
        .call(&events, "http://www.onvif.org/ver10/events/wsdl/EventPortType/CreatePullPointSubscriptionRequest",
            "<tev:CreatePullPointSubscription><tev:InitialTerminationTime>PT120S</tev:InitialTerminationTime></tev:CreatePullPointSubscription>")
        .await?;
    let addr = tag_text(&sub, "Address").map(|a| c.fix(&a)).ok_or_else(|| anyhow::anyhow!("the camera didn't give a subscription address"))?;
    set(&cam.id, |s| { s.connected = true; s.error = None; });
    info!("[{}] listening to the camera's own detection (ONVIF events)", cam.name);
    let mut renewed = std::time::Instant::now();
    // What's going on now, per topic (only a change from off to on counts).
    let mut on: HashMap<String, bool> = HashMap::new();
    loop {
        if mgr.config().cameras.iter().find(|x| x.id == cam.id) != Some(cam) {
            let _ = c.call(&addr, "http://docs.oasis-open.org/wsn/bw-2/SubscriptionManager/UnsubscribeRequest", "<wsnt:Unsubscribe/>").await;
            return Ok(());
        }
        if renewed.elapsed() > Duration::from_secs(60) {
            c.call(&addr, "http://docs.oasis-open.org/wsn/bw-2/SubscriptionManager/RenewRequest",
                "<wsnt:Renew><wsnt:TerminationTime>PT120S</wsnt:TerminationTime></wsnt:Renew>").await?;
            renewed = std::time::Instant::now();
        }
        let xml = c
            .call(&addr, "http://www.onvif.org/ver10/events/wsdl/PullPointSubscription/PullMessagesRequest",
                "<tev:PullMessages><tev:Timeout>PT10S</tev:Timeout><tev:MessageLimit>32</tev:MessageLimit></tev:PullMessages>")
            .await?;
        for (topic, active) in parse_messages(&xml) {
            let was = on.insert(topic.clone(), active).unwrap_or(false);
            if !active || was {
                continue;
            }
            let Some((label, note)) = meaning(&topic, cam.onvif_motion) else { continue };
            set(&cam.id, |s| { s.last = Some(topic.clone()); s.last_at = Some(chrono::Local::now()); });
            if let Some(d) = mgr.camera(&cam.id).and_then(|r| r.detector()) {
                d.external(label, note);
            }
        }
    }
}

pub async fn run(mgr: Arc<Manager>) {
    let mut tasks: HashMap<String, (CameraConfig, tokio::task::JoinHandle<()>)> = HashMap::new();
    loop {
        let cfg = mgr.config();
        let want: Vec<CameraConfig> = cfg.cameras.iter().filter(|c| c.enabled && c.onvif_events).cloned().collect();
        tasks.retain(|id, (c, h)| {
            let keep = want.iter().any(|w| w == c) && !h.is_finished();
            if !keep {
                h.abort();
                if !want.iter().any(|w| &w.id == id) {
                    STATUS.lock().unwrap().as_mut().map(|m| m.remove(id));
                }
            }
            keep
        });
        for c in want {
            if tasks.contains_key(&c.id) {
                continue;
            }
            let (m, cam) = (mgr.clone(), c.clone());
            let h = tokio::spawn(async move {
                let mut wait = 5u64;
                loop {
                    if let Err(e) = watch(&m, &cam).await {
                        warn!("[{}] camera's own detection: {e:#} — trying again in {wait} s", cam.name);
                        set(&cam.id, |s| { s.connected = false; s.error = Some(format!("{e:#}")); });
                    } else if m.config().cameras.iter().find(|x| x.id == cam.id) != Some(&cam) {
                        return;
                    }
                    tokio::time::sleep(Duration::from_secs(wait)).await;
                    wait = (wait * 2).min(300);
                }
            });
            tasks.insert(c.id.clone(), (c, h));
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_camera_events() {
        let xml = r#"<s:Body><tev:PullMessagesResponse><wsnt:NotificationMessage><wsnt:Topic Dialect="x">tns1:RuleEngine/PeopleDetector/People</wsnt:Topic><wsnt:Message><tt:Message UtcTime="t"><tt:Source><tt:SimpleItem Name="VideoSourceConfigurationToken" Value="vsconf"/></tt:Source><tt:Data><tt:SimpleItem Name="IsPeople" Value="true"/></tt:Data></tt:Message></wsnt:Message></wsnt:NotificationMessage><wsnt:NotificationMessage><wsnt:Topic Dialect="x">tns1:RuleEngine/CellMotionDetector/Motion</wsnt:Topic><wsnt:Message><tt:Message><tt:Data><tt:SimpleItem Name="IsMotion" Value="false"/></tt:Data></tt:Message></wsnt:Message></wsnt:NotificationMessage></tev:PullMessagesResponse></s:Body>"#;
        let m = parse_messages(xml);
        assert_eq!(m, vec![("tns1:RuleEngine/PeopleDetector/People".to_string(), true), ("tns1:RuleEngine/CellMotionDetector/Motion".to_string(), false)]);
        assert_eq!(meaning(&m[0].0, false).unwrap().0, "person");
        assert!(meaning(&m[1].0, false).is_none());
        assert_eq!(meaning(&m[1].0, true).unwrap().0, "motion");
        assert_eq!(meaning("tns1:Device/Trigger/VisitorCall", false).unwrap().0, "doorbell");
        assert_eq!(meaning("tns1:RuleEngine/LineDetector/Crossed", false).unwrap().1, "The camera saw a line being crossed");
    }
}
