//! MQTT, for Home Assistant and other home automation (Settings → Smart
//! home). Per camera: motion and each watched object as on/off sensors,
//! whether the camera is online, the latest alert as JSON and its picture.
//! The Home / Away / Night mode can be read and switched. With Home
//! Assistant discovery on, everything appears in Home Assistant by itself.
//!
//! Topics (prefix "gaze" by default):
//!   gaze/available                  online / offline
//!   gaze/mode, gaze/mode/set        home / away / night
//!   gaze/<camera>/motion            ON / OFF
//!   gaze/<camera>/<object>          ON / OFF (person, car, …)
//!   gaze/<camera>/online            ON / OFF
//!   gaze/<camera>/event             JSON of each new alert
//!   gaze/<camera>/snapshot          the alert's picture (JPEG)

use crate::manager::Manager;
use rumqttc::{AsyncClient, Event, LastWill, MqttOptions, Packet, QoS};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{info, warn};

/// How the connection is going, for the settings page.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Status {
    pub connected: bool,
    pub error: Option<String>,
}
static STATUS: Mutex<Option<Status>> = Mutex::new(None);
pub fn status() -> Status {
    STATUS.lock().unwrap().clone().unwrap_or_default()
}
fn set_status(connected: bool, error: Option<String>) {
    *STATUS.lock().unwrap() = Some(Status { connected, error });
}

fn slug(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect()
}

/// Runs for as long as GAZE does; (re)connects when the settings change.
pub async fn run(mgr: Arc<Manager>) {
    loop {
        let cfg = mgr.config().mqtt;
        if !cfg.enabled || cfg.host.trim().is_empty() {
            set_status(false, None);
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }
        session(&mgr, &cfg).await;
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn session(mgr: &Arc<Manager>, cfg: &crate::config::MqttConfig) {
    let p = cfg.prefix.trim().trim_matches('/').to_string();
    let p = if p.is_empty() { "gaze".to_string() } else { p };
    let mut o = MqttOptions::new(format!("gaze-{}", std::process::id()), cfg.host.trim(), cfg.port);
    o.set_keep_alive(Duration::from_secs(30));
    o.set_max_packet_size(8 * 1024 * 1024, 8 * 1024 * 1024);
    o.set_last_will(LastWill::new(format!("{p}/available"), "offline", QoS::AtLeastOnce, true));
    if !cfg.username.is_empty() {
        o.set_credentials(cfg.username.clone(), cfg.password.clone());
    }
    let (client, mut events) = AsyncClient::new(o, 64);
    let started = cfg.clone();
    // Connection events (and mode commands) on their own task.
    let (tx, mut cmds) = tokio::sync::mpsc::channel::<Result<String, String>>(16);
    let set_topic = format!("{p}/mode/set");
    let pump = tokio::spawn(async move {
        loop {
            match events.poll().await {
                Ok(Event::Incoming(Packet::ConnAck(_))) => { let _ = tx.send(Ok(String::new())).await; }
                Ok(Event::Incoming(Packet::Publish(m))) if m.topic == set_topic => {
                    let _ = tx.send(Ok(String::from_utf8_lossy(&m.payload).trim().to_lowercase())).await;
                }
                Ok(_) => {}
                Err(e) => { let _ = tx.send(Err(e.to_string())).await; tokio::time::sleep(Duration::from_secs(5)).await; }
            }
        }
    });
    let mut connected = false;
    // Last published values, to send only changes.
    let mut sent: HashMap<String, String> = HashMap::new();
    let mut seen_events: HashSet<String> = HashSet::new();
    let mut first = true;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            c = cmds.recv() => match c {
                Some(Ok(cmd)) if cmd.is_empty() => {
                    // (Re)connected: announce, subscribe, send everything again.
                    connected = true;
                    set_status(true, None);
                    info!("MQTT: connected to {}:{}", started.host, started.port);
                    sent.clear();
                    let _ = client.publish(format!("{p}/available"), QoS::AtLeastOnce, true, "online").await;
                    let _ = client.subscribe(format!("{p}/mode/set"), QoS::AtLeastOnce).await;
                    if started.discovery {
                        discovery(mgr, &client, &p).await;
                    }
                }
                Some(Ok(cmd)) => {
                    match mgr.set_mode(&cmd).await {
                        Ok(()) => info!("mode: {cmd} (MQTT)"),
                        Err(e) => warn!("MQTT: {e:#}"),
                    }
                }
                Some(Err(e)) => {
                    if connected || first { warn!("MQTT: {e}"); }
                    connected = false;
                    first = false;
                    set_status(false, Some(e));
                }
                None => break,
            },
            _ = tick.tick() => {
                // Settings changed: start again with the new ones.
                if mgr.config().mqtt != started { break; }
                if !connected { continue; }
                publish_state(mgr, &client, &p, &mut sent, &mut seen_events).await;
            }
        }
    }
    let _ = client.publish(format!("{p}/available"), QoS::AtLeastOnce, true, "offline").await;
    let _ = client.disconnect().await;
    pump.abort();
}

async fn put(client: &AsyncClient, sent: &mut HashMap<String, String>, topic: String, value: String) {
    if sent.get(&topic) == Some(&value) {
        return;
    }
    if client.publish(topic.clone(), QoS::AtLeastOnce, true, value.clone()).await.is_ok() {
        sent.insert(topic, value);
    }
}

async fn publish_state(mgr: &Arc<Manager>, client: &AsyncClient, p: &str, sent: &mut HashMap<String, String>, seen: &mut HashSet<String>) {
    let cfg = mgr.config();
    let night = crate::notify::is_night(&cfg.notify, chrono::Local::now());
    put(client, sent, format!("{p}/mode"), cfg.modes.effective(night).to_string()).await;
    let now = chrono::Local::now();
    for rt in mgr.cameras() {
        let id = slug(&rt.cfg.id);
        let onoff = |b: bool| if b { "ON" } else { "OFF" }.to_string();
        let online = rt.relay_main.status().state == crate::relay::RelayState::Connected;
        put(client, sent, format!("{p}/{id}/online"), onoff(online)).await;
        let st = rt.detector().map(|d| d.status());
        let (motion, labels): (bool, Vec<String>) = match &st {
            Some(s) => (s.motion || s.event_active, s.active_labels.iter().map(|l| l.label.clone()).collect()),
            None => (false, Vec::new()),
        };
        put(client, sent, format!("{p}/{id}/motion"), onoff(motion)).await;
        for o in &rt.cfg.detect.objects {
            let on = st.as_ref().is_some_and(|s| s.event_active) && labels.iter().any(|l| l == o);
            put(client, sent, format!("{p}/{id}/{}", slug(o)), onoff(on)).await;
        }
        // New alerts: their details and picture.
        for e in rt.events.between(now - chrono::Duration::minutes(2), now) {
            if e.labels.is_empty() && !e.motion {
                continue;
            }
            let key = format!("{}|{}", rt.cfg.id, e.id);
            // An alert is announced once its picture is there.
            let Some(thumb) = e.thumb.as_ref() else { continue };
            if !seen.insert(key) {
                continue;
            }
            let ev = json!({
                "event_type": "alert",
                "id": e.id, "camera": rt.cfg.id, "camera_name": rt.cfg.name,
                "start": e.start, "end": e.end,
                "labels": e.labels.iter().map(|l| json!({"label": l.label, "score": l.score})).collect::<Vec<_>>(),
                "motion": e.motion, "notes": e.notes, "plates": e.plates,
                "faces": e.faces.iter().filter_map(|f| f.name.clone()).collect::<Vec<_>>(),
            });
            let _ = client.publish(format!("{p}/{id}/event"), QoS::AtLeastOnce, false, ev.to_string()).await;
            if let Ok(jpg) = std::fs::read(rt.events.dir().join(thumb)) {
                let _ = client.publish(format!("{p}/{id}/snapshot"), QoS::AtLeastOnce, true, jpg).await;
            }
        }
    }
    if seen.len() > 5000 {
        seen.clear();
    }
}

/// Home Assistant discovery: every sensor, the alert pictures and the mode
/// switch appear in Home Assistant by themselves, grouped per camera.
async fn discovery(mgr: &Arc<Manager>, client: &AsyncClient, p: &str) {
    let cfg = mgr.config();
    let avail = json!([{"topic": format!("{p}/available")}]);
    let gaze_dev = json!({"identifiers": [format!("{p}_nvr")], "name": cfg.server.name, "manufacturer": "GAZE", "model": "NVR", "sw_version": env!("CARGO_PKG_VERSION")});
    let msg = |topic: String, v: serde_json::Value| (topic, v.to_string());
    let mut out = vec![msg(
        format!("homeassistant/select/{p}_mode/config"),
        json!({"name": "Mode", "unique_id": format!("{p}_mode"), "state_topic": format!("{p}/mode"), "command_topic": format!("{p}/mode/set"),
               "options": ["home", "away", "night"], "icon": "mdi:shield-home", "availability": avail, "device": gaze_dev}),
    )];
    for c in &cfg.cameras {
        let id = slug(&c.id);
        let dev = json!({"identifiers": [format!("{p}_{id}")], "name": c.name, "manufacturer": "GAZE", "model": "Camera", "via_device": format!("{p}_nvr")});
        let uid = |k: &str| format!("{p}_{id}_{k}");
        out.push(msg(format!("homeassistant/binary_sensor/{}/config", uid("motion")),
            json!({"name": "Motion", "unique_id": uid("motion"), "state_topic": format!("{p}/{id}/motion"), "device_class": "motion", "availability": avail, "device": dev})));
        out.push(msg(format!("homeassistant/binary_sensor/{}/config", uid("online")),
            json!({"name": "Online", "unique_id": uid("online"), "state_topic": format!("{p}/{id}/online"), "device_class": "connectivity", "entity_category": "diagnostic", "availability": avail, "device": dev})));
        out.push(msg(format!("homeassistant/image/{}/config", uid("snapshot")),
            json!({"name": "Last alert", "unique_id": uid("snapshot"), "image_topic": format!("{p}/{id}/snapshot"), "content_type": "image/jpeg", "availability": avail, "device": dev})));
        out.push(msg(format!("homeassistant/event/{}/config", uid("alert")),
            json!({"name": "Alert", "unique_id": uid("alert"), "state_topic": format!("{p}/{id}/event"), "event_types": ["alert"],
                   "availability": avail, "device": dev})));
        for o in &c.detect.objects {
            let k = slug(o);
            out.push(msg(format!("homeassistant/binary_sensor/{}/config", uid(&k)),
                json!({"name": format!("{}{}", o[..1].to_uppercase(), &o[1..]), "unique_id": uid(&k), "state_topic": format!("{p}/{id}/{k}"), "device_class": "occupancy",
                       "icon": "mdi:cctv", "availability": avail, "device": dev})));
        }
    }
    for (t, v) in out {
        let _ = client.publish(t, QoS::AtLeastOnce, true, v).await;
    }
}
