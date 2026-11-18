//! The daily digest (Settings → Notifications → Daily digest): one message
//! each morning with what happened in the last 24 hours and what needs
//! attention, through the same channels as alerts (phone and/or email).
//!
//! Built by counting what GAZE already stored: alerts, who and what was
//! recognised, activity after dark, plus problems that exist right now
//! (a camera offline, a drive nearly full or failing). No AI model, nothing
//! new recorded, and recording never depends on it.

use crate::events::Event;
use crate::manager::Manager;
use chrono::{DateTime, Local, NaiveTime};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

/// One camera's alerts in the period.
pub struct CamDay {
    pub name: String,
    pub events: Vec<Event>,
}

pub struct Digest {
    pub title: String,
    pub body: String,
    /// Alerts counted.
    pub alerts: usize,
    /// The most important alert picture: (index into the cameras, file name).
    pub picture: Option<(usize, String)>,
}

#[derive(Default, Clone, Copy)]
struct Kinds {
    person: bool,
    vehicle: bool,
    animal: bool,
    parcel: bool,
}

fn kinds(e: &Event) -> Kinds {
    let mut k = Kinds::default();
    for l in &e.labels {
        match l.label.as_str() {
            "person" => k.person = true,
            "vehicle" | "bicycle" => k.vehicle = true,
            "package" => k.parcel = true,
            _ => k.animal = true,
        }
    }
    k.parcel |= !e.items.is_empty();
    k
}

/// Something worth counting as an alert: found something, or moved.
fn is_alert(e: &Event) -> bool {
    !e.labels.is_empty() || e.motion
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Put the digest together. Pure, so it can be tested.
pub fn compose(
    server: &str,
    from: DateTime<Local>,
    to: DateTime<Local>,
    cams: &[CamDay],
    warnings: &[String],
    is_night: &dyn Fn(DateTime<Local>) -> bool,
) -> Digest {
    let in_period = |e: &&Event| is_alert(e) && e.start >= from && e.start < to;
    // "yesterday 22:14" / "today 03:40": the period spans midnight.
    let when = |t: DateTime<Local>| format!("{} {}", if t.date_naive() == to.date_naive() { "today" } else { "yesterday" }, t.format("%H:%M"));
    let (mut people, mut vehicles, mut animals, mut parcels) = (0, 0, 0, 0);
    let mut per_cam: Vec<(usize, &str)> = Vec::new();
    let mut who: BTreeMap<String, usize> = BTreeMap::new();
    let mut after_dark: Vec<(DateTime<Local>, String)> = Vec::new();
    let mut suspicious: Vec<(DateTime<Local>, String)> = Vec::new();
    // (rank, time, camera, thumb): the best picture wins.
    let mut best: Option<(u8, DateTime<Local>, usize, String)> = None;
    let mut alerts = 0;
    for (ci, cam) in cams.iter().enumerate() {
        let mine: Vec<&Event> = cam.events.iter().filter(in_period).collect();
        per_cam.push((mine.len(), cam.name.as_str()));
        for e in mine {
            alerts += 1;
            let k = kinds(e);
            people += k.person as usize;
            vehicles += k.vehicle as usize;
            animals += k.animal as usize;
            parcels += k.parcel as usize;
            for f in e.faces.iter().filter_map(|f| f.name.as_ref()) {
                *who.entry(f.clone()).or_default() += 1;
            }
            for p in &e.plates {
                *who.entry(format!("plate {p}")).or_default() += 1;
            }
            let night = is_night(e.start);
            if night && (k.person || k.vehicle) {
                let what = if k.person { "person" } else { "vehicle" };
                after_dark.push((e.start, format!("{} {}: {what}", when(e.start), cam.name)));
            }
            if !e.suspicious.is_empty() {
                suspicious.push((e.start, format!("{} {}: {}", when(e.start), cam.name, e.suspicious)));
            }
            if let Some(t) = &e.thumb {
                // A person at night matters most, then any person, then a vehicle.
                let rank = if k.person && night { 4 } else if k.person { 3 } else if k.vehicle { 2 } else { 1 } + if e.suspicious.is_empty() { 0 } else { 1 };
                if best.as_ref().is_none_or(|(r, at, _, _)| (rank, e.start) > (*r, *at)) {
                    best = Some((rank, e.start, ci, t.clone()));
                }
            }
        }
    }
    after_dark.sort();
    suspicious.sort();

    let mut lines: Vec<String> = Vec::new();
    let since = from.format("%H:%M").to_string();
    let title;
    if alerts == 0 {
        title = format!("{server}: a quiet day");
        lines.push(format!("No alerts in the 24 hours since {since} yesterday."));
    } else {
        title = format!("{server}: {}", plural(alerts, "alert", "alerts"));
        lines.push(format!("{} in the 24 hours since {since} yesterday.", plural(alerts, "alert", "alerts")));
        let mut kinds_line: Vec<String> = Vec::new();
        for (n, one, many) in [(people, "person", "people"), (vehicles, "vehicle", "vehicles"), (animals, "animal", "animals"), (parcels, "parcel", "parcels")] {
            if n > 0 {
                kinds_line.push(plural(n, one, many));
            }
        }
        if !kinds_line.is_empty() {
            lines.push(kinds_line.join(" · "));
        }
        if cams.len() > 1 {
            per_cam.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
            let (top, bottom) = (per_cam.first().unwrap(), per_cam.last().unwrap());
            if top.0 != bottom.0 {
                lines.push(format!("Busiest: {} ({}). Quietest: {} ({}).", top.1, top.0, bottom.1, bottom.0));
            }
        }
        if !who.is_empty() {
            let mut w: Vec<(&String, &usize)> = who.iter().collect();
            w.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
            let list: Vec<String> = w.iter().take(6).map(|(n, c)| if **c > 1 { format!("{n} ×{c}") } else { (*n).clone() }).collect();
            lines.push(format!("Recognised: {}.", list.join(", ")));
        }
        for (heading, items) in [("After dark:", &after_dark), ("Worth a look:", &suspicious)] {
            if !items.is_empty() {
                lines.push(heading.to_string());
                lines.extend(items.iter().take(5).map(|(_, t)| format!(" – {t}")));
                if items.len() > 5 {
                    lines.push(format!(" – and {} more", items.len() - 5));
                }
            }
        }
    }
    if !warnings.is_empty() {
        lines.push("Needs attention:".to_string());
        lines.extend(warnings.iter().map(|w| format!(" – {w}")));
    }
    Digest { title, body: lines.join("\n"), alerts, picture: best.map(|(_, _, ci, t)| (ci, t)) }
}

/// Problems that exist right now.
async fn warnings(mgr: &Manager) -> Vec<String> {
    let cfg = mgr.config();
    let mut w = Vec::new();
    for rt in mgr.cameras() {
        if !rt.cfg.enabled {
            continue;
        }
        let st = rt.relay_main.status();
        if st.state != crate::relay::RelayState::Connected && st.error.is_some() {
            w.push(format!("{}: camera offline", rt.cfg.name));
        }
    }
    for l in mgr.usage().locations.iter().filter(|l| crate::health::drive_low(cfg.storage.min_free_gb, l)) {
        w.push(format!("{}: {:.1} GB free of {:.0} GB", l.path.display(), l.free_bytes as f64 / 1e9, l.total_bytes as f64 / 1e9));
    }
    for d in crate::smart::overview().drives.iter().filter(|d| d.health == "failing" || d.health == "warning") {
        let what = d.problems.first().cloned().unwrap_or_default();
        w.push(format!("Drive {}: {}{}", d.name, if d.health == "failing" { "failing" } else { "needs attention" }, if what.is_empty() { String::new() } else { format!(" ({what})") }));
    }
    w
}

/// The digest for the 24 hours ending at `to`, ready to send.
pub async fn build(mgr: &Manager, to: DateTime<Local>) -> (Digest, Option<Vec<u8>>) {
    let cfg = mgr.config();
    let from = to - chrono::Duration::hours(24);
    let rts: Vec<_> = mgr.cameras().into_iter().filter(|rt| cfg.notify.cameras.is_empty() || cfg.notify.cameras.contains(&rt.cfg.id)).collect();
    let cams: Vec<CamDay> = rts.iter().map(|rt| CamDay { name: rt.cfg.name.clone(), events: rt.events.between(from, to) }).collect();
    let warn = warnings(mgr).await;
    let n = cfg.notify.clone();
    let d = compose(&cfg.server.name, from, to, &cams, &warn, &|t| crate::notify::is_night(&n, t));
    let picture = if cfg.notify.digest.picture {
        d.picture.as_ref().and_then(|(ci, thumb)| std::fs::read(rts[*ci].events.dir().join(thumb)).ok())
    } else {
        None
    };
    (d, picture)
}

pub async fn send(mgr: &Manager, to: DateTime<Local>) -> anyhow::Result<usize> {
    let cfg = mgr.config();
    let (d, picture) = build(mgr, to).await;
    let alerts = d.alerts;
    let msg = crate::notify::Message {
        title: d.title,
        body: d.body,
        tags: "calendar",
        priority: 3,
        click: (!cfg.notify.link.trim().is_empty()).then(|| cfg.notify.link.trim().to_string()),
        picture,
        actions: None,
    };
    crate::notify::send(&cfg.notify, msg).await?;
    Ok(alerts)
}

fn parse_time(s: &str) -> NaiveTime {
    NaiveTime::parse_from_str(s.trim(), "%H:%M").unwrap_or_else(|_| NaiveTime::from_hms_opt(8, 0, 0).unwrap())
}

/// A digest missed by more than this (GAZE was off at the time) isn't sent late.
const CATCH_UP: chrono::Duration = chrono::Duration::hours(6);

/// Runs for as long as GAZE does: once a day, at the chosen time.
pub async fn run(mgr: Arc<Manager>) {
    let marker = mgr.data_dir.join("digest-last");
    tokio::time::sleep(Duration::from_secs(45)).await;
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;
        let cfg = mgr.config();
        let (n, dg) = (&cfg.notify, &cfg.notify.digest);
        if !(n.enabled && dg.enabled && n.delivers()) {
            continue;
        }
        let now = Local::now();
        let Some(at) = now.date_naive().and_time(parse_time(&dg.time)).and_local_timezone(Local).earliest() else { continue };
        if now < at || now - at > CATCH_UP {
            continue;
        }
        let today = now.format("%Y-%m-%d").to_string();
        if std::fs::read_to_string(&marker).is_ok_and(|t| t.trim() == today) {
            continue;
        }
        let (d, picture) = build(&mgr, at).await;
        // Marked first: a failing channel must not be retried every minute.
        let _ = std::fs::write(&marker, &today);
        if d.alerts == 0 && !dg.when_quiet && !d.body.contains("Needs attention") {
            continue;
        }
        let msg = crate::notify::Message {
            title: d.title,
            body: d.body,
            tags: "calendar",
            priority: 3,
            click: (!n.link.trim().is_empty()).then(|| n.link.trim().to_string()),
            picture,
            actions: None,
        };
        if let Err(e) = crate::notify::send(n, msg).await {
            tracing::warn!("daily digest: {e:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A time on 2 June 2026 in this computer's own time zone (the digest
    /// speaks in local times, so the tests do too).
    fn t(h: u32, m: u32) -> DateTime<Local> {
        use chrono::TimeZone;
        Local.with_ymd_and_hms(2026, 6, 2, h, m, 0).earliest().unwrap()
    }

    fn ev(id: &str, start: DateTime<Local>, labels: &[&str]) -> Event {
        let labels: Vec<_> = labels.iter().map(|l| serde_json::json!({"label": l, "score": 0.9})).collect();
        serde_json::from_value(serde_json::json!({"id": id, "camera": "c", "start": start, "end": start, "labels": labels, "motion": true})).unwrap()
    }

    fn night(x: DateTime<Local>) -> bool {
        use chrono::Timelike;
        x.hour() >= 22 || x.hour() < 6
    }

    #[test]
    fn a_busy_day_is_counted_and_the_night_called_out() {
        let from = t(8, 0) - chrono::Duration::hours(24);
        let to = t(8, 0);
        let mut a = ev("a", from + chrono::Duration::hours(15), &["person"]); // 23:00 the day before
        a.faces = vec![serde_json::from_value(serde_json::json!({"file": "f.jpg", "print": "", "name": "Sam"})).unwrap()];
        a.thumb = Some("a.jpg".into());
        let mut b = ev("b", from + chrono::Duration::hours(2), &["vehicle"]);
        b.plates = vec!["ABC123".into()];
        let c = ev("c", from + chrono::Duration::hours(3), &["dog"]);
        let old = ev("old", from - chrono::Duration::hours(1), &["person"]); // before the period
        let cams = [CamDay { name: "Front".into(), events: vec![a, b, old] }, CamDay { name: "Back".into(), events: vec![c] }];
        let d = compose("Home", from, to, &cams, &["Garage: camera offline".into()], &night);
        assert_eq!(d.alerts, 3);
        assert_eq!(d.title, "Home: 3 alerts");
        assert!(d.body.contains("1 person · 1 vehicle · 1 animal"), "{}", d.body);
        assert!(d.body.contains("Busiest: Front (2). Quietest: Back (1)."), "{}", d.body);
        assert!(d.body.contains("Recognised: ABC123") || d.body.contains("Sam"), "{}", d.body);
        assert!(d.body.contains("plate ABC123") && d.body.contains("Sam"), "{}", d.body);
        assert!(d.body.contains("After dark:\n – yesterday 23:00 Front: person"), "{}", d.body);
        assert!(d.body.contains("Needs attention:\n – Garage: camera offline"));
        // The picture is the person at night, from the first camera.
        assert_eq!(d.picture, Some((0, "a.jpg".into())));
    }

    #[test]
    fn a_quiet_day_says_so_and_still_shows_problems() {
        let d = compose("Home", t(8, 0) - chrono::Duration::hours(24), t(8, 0), &[CamDay { name: "Front".into(), events: vec![] }], &[], &night);
        assert_eq!((d.alerts, d.title.as_str()), (0, "Home: a quiet day"));
        assert!(d.body.starts_with("No alerts in the 24 hours since 08:00 yesterday."));
        assert!(!d.body.contains("Needs attention"));
        let w = compose("Home", t(8, 0) - chrono::Duration::hours(24), t(8, 0), &[], &["Drive D: 1.0 GB free of 500 GB".into()], &night);
        assert!(w.body.contains("Needs attention:\n – Drive D: 1.0 GB free of 500 GB"));
        assert!(d.picture.is_none());
    }

    #[test]
    fn long_lists_are_cut_and_the_rest_counted() {
        let from = t(0, 0);
        let evs: Vec<Event> = (0..8).map(|i| ev(&format!("e{i}"), from + chrono::Duration::minutes(i * 10 + 1), &["person"])).collect();
        let d = compose("Home", from, t(8, 0), &[CamDay { name: "Gate".into(), events: evs }], &[], &|_| true);
        assert_eq!(d.alerts, 8);
        assert_eq!(d.body.matches(" – today 0").count(), 5, "{}", d.body);
        assert!(d.body.contains(" – and 3 more"), "{}", d.body);
    }

    #[test]
    fn plain_motion_and_the_time_setting_are_handled() {
        // Motion with no label is an alert but not a person, vehicle or animal.
        let from = t(0, 0);
        let mut m = ev("m", from + chrono::Duration::hours(1), &[]);
        m.motion = true;
        let d = compose("Home", from, t(8, 0), &[CamDay { name: "Gate".into(), events: vec![m] }], &[], &night);
        assert_eq!(d.alerts, 1);
        assert!(!d.body.contains('·'));
        // Times: valid ones as typed, anything else the default.
        assert_eq!(parse_time("07:30"), NaiveTime::from_hms_opt(7, 30, 0).unwrap());
        assert_eq!(parse_time(" 21:05 "), NaiveTime::from_hms_opt(21, 5, 0).unwrap());
        assert_eq!(parse_time("breakfast"), NaiveTime::from_hms_opt(8, 0, 0).unwrap());
    }
}
