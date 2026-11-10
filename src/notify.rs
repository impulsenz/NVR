//! Phone notifications through ntfy (Settings → Notifications).
//!
//! 1. As soon as an alert recognizes a person or vehicle (the kinds chosen),
//!    its picture goes to the phone: "Front Door: Person".
//! 2. When the alert is over and GAZE has described it (smart search, faces
//!    and plates, and optionally Google Gemini looking at several moments of
//!    the HD video), a summary follows: "A delivery driver leaves a parcel at
//!    the door". Suspicious activity is sent with high priority.
//!
//! At most one notification per camera per `gap_minutes` (a shorter gap at
//! night); the summary of an alert whose picture was sent always follows.

use crate::config::NotifyConfig;
use crate::events::Event;
use crate::manager::{CameraRuntime, Manager};
use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Local, NaiveTime};
use serde_json::{Value, json};
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
        .filter(|l| l.label != "motion")
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

fn link(cfg: &NotifyConfig, e: &Event) -> Option<String> {
    let base = cfg.link.trim().trim_end_matches('/');
    (!base.is_empty()).then(|| format!("{base}/#alerts?open={}~{}", e.camera, e.id))
}

/// Google Gemini looks at a few moments of the alert: what happened, and
/// does it look suspicious?
async fn ask_gemini(
    cfg: &NotifyConfig,
    cam: &str,
    e: &Event,
    pictures: &[Vec<u8>],
    night: bool,
) -> Result<(String, Option<String>)> {
    use base64::Engine;
    let prompt = format!(
        "You watch a home security camera called \"{cam}\". These {} pictures are moments, in order, of one alert at {} ({}). \
         Detected: {}. {}\n\
         Reply with JSON only: {{\"summary\": \"one or two short sentences saying what happens, e.g. 'A delivery driver leaves a parcel at the front door and walks back to the van.'\", \
         \"suspicious\": true or false, \"reason\": \"a few words, only if suspicious\"}}. \
         Suspicious means things like trying door handles, looking into cars or windows, lingering without reason (especially at night), \
         carrying things away from the property, climbing fences, hiding the face. Deliveries, residents, visitors, passers-by and animals are normal.",
        pictures.len(),
        e.start.format("%H:%M"),
        if night { "night" } else { "day" },
        what(e),
        if e.notes.is_empty() {
            String::new()
        } else {
            format!("Notes: {}.", e.notes.join("; "))
        },
    );
    let mut parts: Vec<Value> = pictures
        .iter()
        .map(|p| json!({"inline_data": {"mime_type": "image/jpeg", "data": base64::engine::general_purpose::STANDARD.encode(p)}}))
        .collect();
    parts.push(json!({"text": prompt}));
    let body = json!({
        "contents": [{"parts": parts}],
        "generationConfig": {"responseMimeType": "application/json", "temperature": 0.2},
    });
    let model = if cfg.gemini_model.trim().is_empty() {
        "gemini-2.5-flash"
    } else {
        cfg.gemini_model.trim()
    };
    let base = std::env::var("GAZE_GEMINI_BASE")
        .unwrap_or_else(|_| "https://generativelanguage.googleapis.com".into());
    let resp = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()?
        .post(format!("{base}/v1beta/models/{model}:generateContent"))
        .header("x-goog-api-key", cfg.gemini_key.trim())
        .json(&body)
        .send()
        .await
        .context("cannot reach Gemini")?;
    let status = resp.status();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        bail!(
            "Gemini {status}: {}",
            v["error"]["message"].as_str().unwrap_or("request failed")
        );
    }
    let text: String = v["candidates"][0]["content"]["parts"]
        .as_array()
        .map(|p| p.iter().filter_map(|x| x["text"].as_str()).collect())
        .unwrap_or_default();
    let start = text
        .find('{')
        .ok_or_else(|| anyhow!("Gemini gave no answer"))?;
    let end = text
        .rfind('}')
        .ok_or_else(|| anyhow!("Gemini gave no answer"))?;
    let r: Value = serde_json::from_str(&text[start..=end]).context("Gemini's answer")?;
    let summary = r["summary"].as_str().unwrap_or("").trim().to_string();
    let sus = r["suspicious"].as_bool().unwrap_or(false).then(|| {
        let reason = r["reason"].as_str().unwrap_or("").trim();
        if reason.is_empty() {
            "Looks suspicious".to_string()
        } else {
            cap(reason)
        }
    });
    Ok((summary, sus))
}

/// The best picture of an alert, as a JPEG.
fn picture(rt: &CameraRuntime, e: &Event) -> Option<Vec<u8>> {
    std::fs::read(rt.events.dir().join(e.thumb.as_ref()?)).ok()
}

struct Notifier {
    /// When each camera last notified.
    last: HashMap<String, DateTime<Local>>,
    last_error: String,
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
        for rt in mgr.cameras() {
            if !n.cameras.is_empty() && !n.cameras.contains(&rt.cfg.id) {
                continue;
            }
            for mut e in rt.events.between(now - chrono::Duration::minutes(15), now) {
                if e.notified == DONE || !e.labels.iter().any(|l| n.objects.contains(&l.label)) {
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
                    if sus.is_none() && (n.only_suspicious || !self.may_send(n, &rt.cfg.id, now)) {
                        // Maybe later, if it turns out to be suspicious.
                        e.notified = HELD;
                        rt.events.save(e);
                        continue;
                    }
                    let msg = Message {
                        title: format!("{}: {}", rt.cfg.name, what(&e)),
                        body: match &sus {
                            Some(r) => format!("⚠ {r} — {}", e.start.format("%H:%M:%S")),
                            None => format!("{} at {}", what(&e), e.start.format("%H:%M:%S")),
                        },
                        tags: tags(&e, sus.is_some()),
                        priority: if sus.is_some() { 4 } else { 3 },
                        click: link(n, &e),
                        picture: Some(pic),
                    };
                    if self.report(send(n, msg).await) {
                        info!("[{}] notification sent: {}", rt.cfg.name, what(&e));
                        self.last.insert(rt.cfg.id.clone(), now);
                        e.notified = if n.summary { PICTURE_SENT } else { DONE };
                        rt.events.save(e);
                    }
                    continue;
                }
                // 2. The summary, once the alert is over and described.
                if rt.in_progress(&e) {
                    continue;
                }
                let waited = (now - e.end).num_seconds();
                let described = (!cfg.ai.smart_search || !e.summary.is_empty())
                    && (!(cfg.ai.faces || cfg.ai.plates) || e.identified);
                if !described && waited < 150 {
                    continue;
                }
                if e.notified == HELD && !n.gemini && suspicious(n, &e).is_none() {
                    e.notified = DONE;
                    rt.events.save(e);
                    continue;
                }
                let night = is_night(n, e.start);
                if n.gemini && !n.gemini_key.trim().is_empty() && e.story.is_empty() {
                    let mut pics: Vec<Vec<u8>> = picture(&rt, &e).into_iter().collect();
                    for (f, _) in crate::identify::moments(mgr, &rt, &e, 3).await {
                        if let Ok(j) = f.fit(960).jpeg(80) {
                            pics.push(j);
                        }
                    }
                    match ask_gemini(n, &rt.cfg.name, &e, &pics, night).await {
                        Ok((story, sus)) => {
                            e.story = story;
                            e.suspicious = sus.unwrap_or_default();
                            if !e.story.is_empty() {
                                info!(target: "event", "[{}] Gemini: {}{}", rt.cfg.name, e.story,
                                    if e.suspicious.is_empty() { String::new() } else { format!(" (suspicious: {})", e.suspicious) });
                            }
                        }
                        Err(err) => {
                            self.report(Err(err));
                            e.story = "-".into(); // don't ask again
                        }
                    }
                    rt.events.save(e.clone());
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
                let story = if !e.story.is_empty() && e.story != "-" {
                    e.story.clone()
                } else if !e.summary.is_empty() {
                    e.summary.clone()
                } else {
                    format!(
                        "{} — {} s",
                        what(&e),
                        (e.end - e.start).num_seconds().max(1)
                    )
                };
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
                    click: link(n, &e),
                    picture: picture(&rt, &e),
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
    };
    loop {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let cfg = mgr.config();
        if cfg.notify.enabled && !cfg.notify.topic.trim().is_empty() {
            n.round(&mgr).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
