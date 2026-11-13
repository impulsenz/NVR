//! Phone notifications through ntfy (Settings → Notifications).
//!
//! 1. As soon as an alert recognizes a person or vehicle (the kinds chosen),
//!    its picture goes to the phone: "Front Door: Person".
//! 2. When the alert is over and faces and plates have been looked for, a
//!    summary follows: what was seen, for how long, who was recognised,
//!    plates, zones. Suspicious activity (lines crossed, lingering, items
//!    taken, people at night) is sent with high priority.
//!
//! At most one notification per camera per `gap_minutes` (a shorter gap at
//! night); the summary of an alert whose picture was sent always follows.

use crate::config::NotifyConfig;
use crate::events::Event;
use crate::manager::{CameraRuntime, Manager};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Local, NaiveTime};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

/// `notified` values.
const PICTURE_SENT: u8 = 1;
const DONE: u8 = 2;
/// Held back (waiting to see whether it's suspicious, or rate-limited).
const HELD: u8 = 4;

/// Header values must be ASCII: others are sent RFC 2047 encoded, which
/// ntfy decodes.
fn header(s: &str) -> String {
    let s = s.replace('\r', "");
    if s.is_ascii() {
        // ntfy turns a written "\n" into a new line.
        s.replace('\n', "\\n")
    } else {
        use base64::Engine;
        format!(
            "=?UTF-8?B?{}?=",
            base64::engine::general_purpose::STANDARD.encode(s)
        )
    }
}

pub struct Message<'a> {
    pub title: String,
    pub body: String,
    pub tags: &'a str,
    /// 1..5 (3 = normal).
    pub priority: u8,
    pub click: Option<String>,
    pub picture: Option<Vec<u8>>,
    /// ntfy action buttons ("http, Snooze 1 hour, https://…, method=POST; …").
    pub actions: Option<String>,
}

// ---------- snoozing a camera's notifications ----------

static SNOOZE: std::sync::Mutex<Option<HashMap<String, DateTime<Local>>>> = std::sync::Mutex::new(None);

/// Until when this camera's notifications are snoozed.
pub fn snoozed_until(camera: &str) -> Option<DateTime<Local>> {
    let s = SNOOZE.lock().unwrap();
    s.as_ref()?.get(camera).copied().filter(|t| *t > Local::now())
}

/// Snooze a camera's notifications for some minutes (0 = notify again now).
pub fn snooze(camera: &str, minutes: i64) {
    let mut s = SNOOZE.lock().unwrap();
    let m = s.get_or_insert_with(HashMap::new);
    if minutes <= 0 {
        m.remove(camera);
    } else {
        m.insert(camera.to_string(), Local::now() + chrono::Duration::minutes(minutes));
    }
}

/// Minutes from now until tomorrow morning (07:00).
pub fn until_morning() -> i64 {
    let now = Local::now();
    let t = (now + chrono::Duration::days(1)).date_naive().and_hms_opt(7, 0, 0).unwrap();
    let t = t.and_local_timezone(Local).single().unwrap_or(now + chrono::Duration::hours(12));
    ((t - now).num_minutes()).max(1)
}

/// The key that signs snooze links in notifications (made once, kept in
/// GAZE's data folder): a tap on the phone works without signing in, and
/// nobody can make such a link for another camera or time.
fn key(data_dir: &std::path::Path) -> Vec<u8> {
    let p = data_dir.join("notify.key");
    if let Ok(k) = std::fs::read(&p)
        && k.len() >= 32
    {
        return k;
    }
    use rand_core::RngCore;
    let mut k = vec![0u8; 32];
    rand_core::OsRng.fill_bytes(&mut k);
    let _ = std::fs::write(&p, &k);
    k
}

fn hmac(key: &[u8], msg: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut k = [0u8; 64];
    k[..key.len().min(64)].copy_from_slice(&key[..key.len().min(64)]);
    let (mut ipad, mut opad) = ([0x36u8; 64], [0x5cu8; 64]);
    for i in 0..64 {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let inner = Sha256::new().chain_update(ipad).chain_update(msg.as_bytes()).finalize();
    let outer = Sha256::new().chain_update(opad).chain_update(inner).finalize();
    outer.iter().map(|b| format!("{b:02x}")).collect()
}

/// A signed link that snoozes `camera` for `minutes` (valid for 2 days).
pub fn snooze_link(data_dir: &std::path::Path, base: &str, camera: &str, minutes: i64) -> String {
    let exp = (Local::now() + chrono::Duration::days(2)).timestamp();
    let sig = hmac(&key(data_dir), &format!("{camera}|{minutes}|{exp}"));
    format!("{base}/api/snooze?c={}&m={minutes}&e={exp}&s={sig}", crate::library::enc(camera))
}

/// Check a snooze link's signature.
pub fn snooze_ok(data_dir: &std::path::Path, camera: &str, minutes: i64, exp: i64, sig: &str) -> bool {
    let want = hmac(&key(data_dir), &format!("{camera}|{minutes}|{exp}"));
    exp > Local::now().timestamp() && want.len() == sig.len() && want.bytes().zip(sig.bytes()).fold(0, |a, (x, y)| a | (x ^ y)) == 0
}

/// Send one notification.
pub async fn send(cfg: &NotifyConfig, m: Message<'_>) -> Result<()> {
    if cfg.topic.trim().is_empty() {
        bail!("choose a topic first");
    }
    let url = format!(
        "{}/{}",
        cfg.server.trim().trim_end_matches('/'),
        cfg.topic.trim()
    );
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let mut req = client
        .put(&url)
        .header("Title", header(&m.title))
        .header("Tags", m.tags)
        .header("Priority", m.priority.to_string());
    if let Some(c) = &m.click {
        req = req.header("Click", header(c));
    }
    if let Some(a) = &m.actions {
        req = req.header("Actions", header(a));
    }
    if !cfg.token.trim().is_empty() {
        req = req.header("Authorization", format!("Bearer {}", cfg.token.trim()));
    }
    req = match m.picture {
        // With a picture the text goes in a header; the picture is the body.
        Some(p) => req
            .header("Message", header(&m.body))
            .header("Filename", "alert.jpg")
            .body(p),
        None => req.body(m.body),
    };
    let resp = req
        .send()
        .await
        .with_context(|| format!("cannot reach {}", cfg.server))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let v: Value = resp.json().await.unwrap_or(Value::Null);
        bail!(
            "ntfy {status}: {}",
            v["error"].as_str().unwrap_or("request refused")
        );
    }
    Ok(())
}

fn parse_hm(s: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(s.trim(), "%H:%M").ok()
}

/// Is `t` in the night hours (which may wrap past midnight)?
pub fn is_night(cfg: &NotifyConfig, t: DateTime<Local>) -> bool {
    let (Some(a), Some(b)) = (parse_hm(&cfg.night_from), parse_hm(&cfg.night_to)) else {
        return false;
    };
    let now = t.time();
    if a <= b {
        a <= now && now < b
    } else {
        now >= a || now < b
    }
}

/// Notes that are worth attention by themselves.
fn zone_alarm(e: &Event) -> Option<&str> {
    e.notes
        .iter()
        .find(|n| {
            let l = n.to_lowercase();
            l.contains("crossed")
                || l.contains("lingering")
                || l.contains("loiter")
                || l.contains("picked up")
                || l.contains("taken")
        })
        .map(String::as_str)
}

/// Why this alert deserves attention, if it does.
fn suspicious(cfg: &NotifyConfig, e: &Event) -> Option<String> {
    if !e.suspicious.is_empty() {
        return Some(e.suspicious.clone());
    }
    if let Some(n) = zone_alarm(e) {
        return Some(n.to_string());
    }
    if is_night(cfg, e.start) && e.labels.iter().any(|l| l.label == "person") {
        return Some("Person at night".into());
    }
    None
}

fn what(e: &Event) -> String {
    let mut v: Vec<String> = e
        .labels
        .iter()
        .filter(|l| l.label != "motion" && l.hd != Some(false))
        .map(|l| cap(&l.label))
        .collect();
    v.dedup();
    if v.is_empty() {
        "Motion".into()
    } else {
        v.join(", ")
    }
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

fn tags(e: &Event, sus: bool) -> &'static str {
    if sus {
        "rotating_light"
    } else if e.labels.iter().any(|l| l.label == "person") {
        "bust_in_silhouette"
    } else if e.labels.iter().any(|l| l.label == "package") {
        "package"
    } else if e
        .labels
        .iter()
        .any(|l| ["car", "truck", "bus", "motorcycle"].contains(&l.label.as_str()))
    {
        "car"
    } else {
        "camera"
    }
}

/// Where tapping the notification goes: that alert in GAZE. Without an
/// address in the settings, this computer's address on the home network.
/// The address the phone opens GAZE at.
fn base(cfg: &NotifyConfig, listen: &str) -> Option<String> {
    Some(match cfg.link.trim().trim_end_matches('/') {
        "" => {
            let port = listen.rsplit(':').next().unwrap_or("8080");
            format!("http://{}:{port}", lan_ip()?)
        }
        b => b.to_string(),
    })
}

fn link(cfg: &NotifyConfig, listen: &str, e: &Event) -> Option<String> {
    Some(format!("{}/#alerts?open={}~{}", base(cfg, listen)?, e.camera, e.id))
}

/// This computer's address on the local network.
pub fn lan_ip() -> Option<std::net::IpAddr> {
    // No packet is sent: connecting a UDP socket only picks the route.
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.168.0.1:9")
        .or_else(|_| s.connect("8.8.8.8:53"))
        .ok()?;
    let ip = s.local_addr().ok()?.ip();
    (!ip.is_unspecified() && !ip.is_loopback()).then_some(ip)
}

/// The best picture of an alert, as a JPEG.
fn picture(rt: &CameraRuntime, e: &Event) -> Option<Vec<u8>> {
    std::fs::read(rt.events.dir().join(e.thumb.as_ref()?)).ok()
}

/// The alert picture as the alert card shows it: a box around each
/// recognized object with "PERSON 92%".
fn alert_picture(rt: &CameraRuntime, e: &Event) -> Option<Vec<u8>> {
    let raw = picture(rt, e)?;
    let Ok(f) = crate::imgutil::decode_jpeg(&raw) else {
        return Some(raw);
    };
    let labels: Vec<String> = e
        .boxes
        .iter()
        .filter(|b| b.label != "motion")
        .map(|b| format!("{} {:.0}%", b.label, b.score * 100.0))
        .collect();
    let mut marks: Vec<crate::draw::Mark> = e
        .boxes
        .iter()
        .filter(|b| b.label != "motion")
        .zip(&labels)
        .map(|(b, l)| crate::draw::Mark {
            x: b.x,
            y: b.y,
            w: b.w,
            h: b.h,
            label: l,
        })
        .collect();
    let fallback;
    if marks.is_empty()
        && let (Some([x, y, w, h]), Some(l)) = (e.focus, e.labels.first())
    {
        fallback = format!("{} {:.0}%", l.label, l.score * 100.0);
        marks.push(crate::draw::Mark {
            x,
            y,
            w,
            h,
            label: &fallback,
        });
    }
    if marks.is_empty() {
        return Some(raw);
    }
    crate::draw::boxes(&f, &marks).jpeg(85).ok().or(Some(raw))
}

struct Notifier {
    /// When each camera last notified.
    last: HashMap<String, DateTime<Local>>,
    last_error: String,
    /// Descriptions made for alerts whose picture isn't sent yet (so a
    /// failed send doesn't ask the AI again).
    /// After a failed send: wait before trying again.
    retry_after: Option<std::time::Instant>,
}

impl Notifier {
    fn may_send(&self, cfg: &NotifyConfig, cam: &str, now: DateTime<Local>) -> bool {
        let gap = if is_night(cfg, now) {
            cfg.night_gap_minutes
        } else {
            cfg.gap_minutes
        } as i64;
        self.last
            .get(cam)
            .is_none_or(|t| (now - *t).num_seconds() >= gap * 60)
    }

    fn report(&mut self, r: Result<()>) -> bool {
        match r {
            Ok(()) => {
                self.last_error.clear();
                let mut st = STATUS.lock().unwrap();
                st.sent += 1;
                st.last_sent = Some(Local::now());
                st.last_error = None;
                true
            }
            Err(e) => {
                self.retry_after = Some(std::time::Instant::now() + Duration::from_secs(30));
                let msg = format!("{e:#}");
                if msg != self.last_error {
                    warn!("notifications: {msg}");
                    self.last_error = msg.clone();
                }
                STATUS.lock().unwrap().last_error = Some(msg);
                false
            }
        }
    }

    async fn round(&mut self, mgr: &Manager) {
        let cfg = mgr.config();
        let n = &cfg.notify;
        let now = Local::now();
        // Home / Away / Night.
        let mode = cfg.modes.effective(is_night(n, now)).to_string();
        for rt in mgr.cameras() {
            if !n.cameras.is_empty() && !n.cameras.contains(&rt.cfg.id) {
                continue;
            }
            if !cfg.modes.notifies(&mode, &rt.cfg.id) || snoozed_until(&rt.cfg.id).is_some() {
                // Not wanted in this mode: these alerts are never sent later.
                for mut e in rt.events.between(now - chrono::Duration::minutes(15), now) {
                    if e.notified != DONE {
                        e.notified = DONE;
                        rt.events.save(e);
                    }
                }
                continue;
            }
            for mut e in rt.events.between(now - chrono::Duration::minutes(15), now) {
                // Only what the full-quality check didn't turn down.
                if e.notified == DONE
                    || !e.labels.iter().any(|l| (n.objects.contains(&l.label) || l.label == "doorbell") && l.hd != Some(false))
                {
                    continue;
                }
                // Being checked in full quality: wait for the answer (a
                // few seconds at most).
                if e.hd_checking && (now - e.start).num_seconds() < 20 {
                    continue;
                }
                // 1. The picture, right away.
                if e.notified == 0 {
                    if (now - e.start).num_minutes() >= 3 {
                        // Too old (GAZE was just started): don't flood the phone.
                        e.notified = DONE;
                        rt.events.save(e);
                        continue;
                    }
                    let Some(pic) = picture(&rt, &e) else {
                        continue;
                    };
                    let sus = suspicious(n, &e);
                    if sus.is_none() && !self.may_send(n, &rt.cfg.id, now) {
                        // Within the quiet time: maybe later, if it turns
                        // out to be suspicious.
                        e.notified = HELD;
                        rt.events.save(e);
                        continue;
                    }
                    let time = e.start.format("%H:%M:%S");
                    let msg = Message {
                        title: format!("{}: {}", rt.cfg.name, what(&e)),
                        body: match &sus {
                            Some(r) => format!("⚠ {r} — {time}"),
                            None => format!("{} at {time}", what(&e)),
                        },
                        tags: tags(&e, sus.is_some()),
                        priority: if sus.is_some() { 4 } else { 3 },
                        click: link(n, &cfg.server.listen, &e),
                        picture: alert_picture(&rt, &e).or(Some(pic)),
                        // Snooze this camera straight from the notification.
                        // Open the alert, or snooze this camera, straight
                        // from the notification.
                        actions: base(n, &cfg.server.listen).map(|b| {
                            let l = |m| snooze_link(&mgr.data_dir, &b, &rt.cfg.id, m);
                            let open = link(n, &cfg.server.listen, &e).map(|u| format!("view, Open alert, {u}; ")).unwrap_or_default();
                            format!("{open}http, Snooze 1 hour, {}, method=POST, clear=true; http, Snooze until tomorrow, {}, method=POST, clear=true", l(60), l(until_morning()))
                        }),
                    };
                    if self.report(send(n, msg).await) {
                        info!("[{}] notification sent: {}", rt.cfg.name, what(&e));
                        self.last.insert(rt.cfg.id.clone(), now);
                        e.notified = if n.summary { PICTURE_SENT } else { DONE };
                        rt.events.save(e);
                    }
                    continue;
                }
                // 2. The summary, once the alert is over (and faces and
                // plates have been looked for).
                if rt.in_progress(&e) {
                    continue;
                }
                let waited = (now - e.end).num_seconds();
                let identified = !(cfg.ai.faces || cfg.ai.plates) || e.identified;
                if !identified && waited < 150 {
                    continue;
                }
                let sus = suspicious(n, &e);
                // Held back and nothing suspicious after all: stay quiet.
                // (Suspicious activity is sent even within the gap.)
                if e.notified == HELD && sus.is_none() {
                    e.notified = DONE;
                    rt.events.save(e);
                    continue;
                }
                if !n.summary && e.notified == PICTURE_SENT {
                    e.notified = DONE;
                    rt.events.save(e);
                    continue;
                }
                let story = format!(
                    "{} — {} s",
                    what(&e),
                    (e.end - e.start).num_seconds().max(1)
                );
                let mut body = story;
                let extra: Vec<&String> = e
                    .notes
                    .iter()
                    .filter(|x| !body.contains(x.as_str()))
                    .collect();
                if !extra.is_empty() {
                    body += &format!(
                        "\n{}",
                        extra
                            .iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join("; ")
                    );
                }
                if let Some(r) = &sus {
                    body = format!("⚠ {r}\n{body}");
                }
                let msg = Message {
                    title: format!(
                        "{} {}: {}",
                        rt.cfg.name,
                        e.start.format("%H:%M"),
                        if sus.is_some() {
                            "suspicious activity"
                        } else {
                            "what happened"
                        }
                    ),
                    body,
                    tags: tags(&e, sus.is_some()),
                    priority: if sus.is_some() { 5 } else { 2 },
                    click: link(n, &cfg.server.listen, &e),
                    picture: alert_picture(&rt, &e),
                    actions: None,
                };
                if self.report(send(n, msg).await) {
                    self.last.insert(rt.cfg.id.clone(), now);
                    e.notified = DONE;
                    rt.events.save(e);
                }
            }
        }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Status {
    pub sent: u64,
    pub last_sent: Option<DateTime<Local>>,
    pub last_error: Option<String>,
}

pub static STATUS: std::sync::Mutex<Status> = std::sync::Mutex::new(Status {
    sent: 0,
    last_sent: None,
    last_error: None,
});

pub async fn run(mgr: Arc<Manager>) {
    let mut n = Notifier {
        last: HashMap::new(),
        last_error: String::new(),
        retry_after: None,
    };
    loop {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let cfg = mgr.config();
        if n.retry_after.is_some_and(|t| t > std::time::Instant::now()) {
            continue;
        }
        if cfg.notify.enabled && !cfg.notify.topic.trim().is_empty() {
            n.round(&mgr).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snooze_links_are_signed() {
        let dir = std::env::temp_dir().join(format!("nvr-sn-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let l = snooze_link(&dir, "http://x", "front", 60);
        let q: HashMap<&str, &str> = l.split_once('?').unwrap().1.split('&').filter_map(|kv| kv.split_once('=')).collect();
        let (e, s) = (q["e"].parse::<i64>().unwrap(), q["s"]);
        assert!(snooze_ok(&dir, "front", 60, e, s));
        assert!(!snooze_ok(&dir, "garage", 60, e, s)); // another camera
        assert!(!snooze_ok(&dir, "front", 6000, e, s)); // longer
        assert!(!snooze_ok(&dir, "front", 60, e + 99999, s)); // later expiry
        assert!(!snooze_ok(&dir, "front", 60, Local::now().timestamp() - 5, &hmac(&key(&dir), &format!("front|60|{}", Local::now().timestamp() - 5)))); // expired
        snooze("front", 30);
        assert!(snoozed_until("front").is_some());
        snooze("front", 0);
        assert!(snoozed_until("front").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn night_hours_wrap_midnight() {
        let c = NotifyConfig::default();
        let at = |h, m| Local::now().with_hour(h).unwrap().with_minute(m).unwrap();
        use chrono::Timelike;
        assert!(is_night(&c, at(23, 0)));
        assert!(is_night(&c, at(2, 30)));
        assert!(!is_night(&c, at(6, 0)));
        assert!(!is_night(&c, at(14, 0)));
        assert_eq!(header("Front Door: Person"), "Front Door: Person");
        assert!(header("Porch — parcel").starts_with("=?UTF-8?B?"));
    }
}
