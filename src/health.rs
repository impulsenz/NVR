//! Health notifications (Settings → Notifications): a camera that went
//! offline (and when it's back), recording that stopped, and the recordings
//! drive running out of space. Sent through the same ntfy topic as alerts,
//! once per problem, so a dead camera doesn't go unnoticed for days.

use crate::manager::Manager;
use crate::notify::{Message, send};
use crate::recorder::RecState;
use crate::relay::RelayState;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// Offline this long before telling (cameras drop for a moment now and then).
const OFFLINE_AFTER: Duration = Duration::from_secs(120);
/// Not recording this long while it should be.
const NOT_RECORDING_AFTER: Duration = Duration::from_secs(180);
/// The disk warning at most this often.
const DISK_EVERY: Duration = Duration::from_secs(12 * 3600);

#[derive(Default)]
struct Watch {
    /// Since when the problem has been seen, and whether it was told.
    since: Option<Instant>,
    told: bool,
}

fn mins(d: Duration) -> String {
    let m = (d.as_secs() + 30) / 60;
    if m < 60 { format!("{m} min") } else { format!("{} h {} min", m / 60, m % 60) }
}

pub async fn run(mgr: Arc<Manager>) {
    let mut offline: HashMap<String, Watch> = HashMap::new();
    let mut not_rec: HashMap<String, Watch> = HashMap::new();
    let mut disk_told: Option<Instant> = None;
    tokio::time::sleep(Duration::from_secs(60)).await;
    loop {
        tokio::time::sleep(Duration::from_secs(30)).await;
        let cfg = mgr.config();
        let n = &cfg.notify;
        let on = n.enabled && n.health && !n.topic.trim().is_empty();
        let now = Instant::now();
        let mut out: Vec<Message> = Vec::new();
        for rt in mgr.cameras() {
            let c = &rt.cfg;
            if !c.enabled || (!n.cameras.is_empty() && !n.cameras.contains(&c.id)) {
                offline.remove(&c.id);
                not_rec.remove(&c.id);
                continue;
            }
            // Offline: the camera connection keeps failing.
            let st = rt.relay_main.status();
            // Not connected, and the last try failed (between tries the
            // connection is idle or connecting, and keeps the error).
            let down = st.state != RelayState::Connected && st.error.is_some();
            let w = offline.entry(c.id.clone()).or_default();
            if down {
                let since = *w.since.get_or_insert(now);
                if !w.told && now - since >= OFFLINE_AFTER {
                    w.told = true;
                    info!(target: "health", "[{}] offline for {} — telling", c.name, mins(now - since));
                    out.push(Message {
                        title: format!("{} is offline", c.name),
                        body: format!("GAZE can't reach this camera ({}). Nothing is being recorded from it. Check its power and network.", st.error.clone().unwrap_or_else(|| "no answer".into())),
                        tags: "warning,camera",
                        priority: 4,
                        click: None,
                        picture: None,
                        actions: None,
                    });
                }
            } else if st.state == RelayState::Connected {
                if let Some(since) = w.since.take()
                    && std::mem::take(&mut w.told)
                {
                    out.push(Message {
                        title: format!("{} is back online", c.name),
                        body: format!("Offline for {}.", mins(now - since)),
                        tags: "white_check_mark,camera",
                        priority: 3,
                        click: None,
                        picture: None,
                        actions: None,
                    });
                }
            }
            // Not recording while it should be (and the camera itself is fine).
            let w = not_rec.entry(c.id.clone()).or_default();
            let failing = !down
                && rt.recorder.as_ref().is_some_and(|r| {
                    let s = r.status();
                    s.scheduled && !s.paused && matches!(s.state, RecState::Retrying | RecState::Connecting)
                });
            if failing {
                let since = *w.since.get_or_insert(now);
                if !w.told && now - since >= NOT_RECORDING_AFTER {
                    w.told = true;
                    let why = rt.recorder.as_ref().and_then(|r| r.status().last_error).unwrap_or_else(|| "it keeps restarting".into());
                    out.push(Message {
                        title: format!("{} stopped recording", c.name),
                        body: format!("The camera is connected, but its recording keeps failing: {why}"),
                        tags: "warning,red_circle",
                        priority: 4,
                        click: None,
                        picture: None,
                        actions: None,
                    });
                }
            } else if w.since.take().is_some() && std::mem::take(&mut w.told) {
                out.push(Message {
                    title: format!("{} is recording again", c.name),
                    body: String::new(),
                    tags: "white_check_mark,red_circle",
                    priority: 3,
                    click: None,
                    picture: None,
                    actions: None,
                });
            }
        }
        // The recordings drive: free space below what's to be kept free (the
        // oldest recordings can't be cleared fast enough), or under 2 GB.
        let usage = mgr.usage();
        let low: Vec<_> = usage
            .locations
            .iter()
            .filter(|l| l.total_bytes > 0)
            .filter(|l| {
                let keep = (cfg.storage.min_free_gb * 1e9) as u64;
                l.free_bytes < keep.max(2_000_000_000).min(l.total_bytes / 10).max(500_000_000)
            })
            .collect();
        if !low.is_empty() && disk_told.is_none_or(|t| now - t >= DISK_EVERY) {
            disk_told = Some(now);
            let l = low[0];
            out.push(Message {
                title: "Recordings drive nearly full".into(),
                body: format!(
                    "{} has {:.1} GB free of {:.0} GB. GAZE deletes the oldest recordings to make room; if the drive is also used for other files, free some space or set a smaller Maximum storage (Settings → Recording & storage).",
                    l.path.display(),
                    l.free_bytes as f64 / 1e9,
                    l.total_bytes as f64 / 1e9
                ),
                tags: "warning,floppy_disk",
                priority: 4,
                click: None,
                picture: None,
                actions: None,
            });
        } else if low.is_empty() {
            disk_told = None;
        }
        if !on {
            continue;
        }
        for m in out {
            let title = m.title.clone();
            if let Err(e) = send(n, m).await {
                warn!(target: "health", "couldn't send \"{title}\": {e:#}");
            }
        }
    }
}
