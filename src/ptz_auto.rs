//! Moving cameras on their own (camera → Pan / tilt / zoom):
//! - Patrol: visit chosen presets in turn, a set time at each; paused while
//!   something is going on, and for a while after someone steers by hand.
//! - Follow: when the AI finds a person (or another chosen kind) off-centre,
//!   nudge the camera towards it; when it's gone, go back (first patrol
//!   preset, or the camera's home position).

use crate::manager::Manager;
use crate::onvif::Target;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// Someone steered this camera by hand: leave it alone for a while.
const HANDS_OFF: Duration = Duration::from_secs(120);
static MANUAL: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(Default::default);

pub fn manual(camera: &str) {
    MANUAL.lock().unwrap().insert(camera.to_string(), Instant::now());
}

#[derive(Default)]
struct State {
    next: usize,
    next_at: Option<Instant>,
    following: bool,
    last_seen: Option<Instant>,
    last_nudge: Option<Instant>,
}

/// How to nudge towards a box (x, y, w, h, 0..1): pan and tilt speeds
/// (-1..1), or None when it's near enough the middle.
pub fn nudge(b: (f32, f32, f32, f32)) -> Option<(f32, f32)> {
    let (cx, cy) = (b.0 + b.2 / 2.0, b.1 + b.3 / 2.0);
    let (dx, dy) = (cx - 0.5, cy - 0.45);
    let pan = if dx.abs() > 0.12 { (dx * 2.0).clamp(-1.0, 1.0) } else { 0.0 };
    // Down in the picture is down for the camera: tilt is the other way.
    let tilt = if dy.abs() > 0.18 { (-dy * 2.0).clamp(-1.0, 1.0) } else { 0.0 };
    (pan != 0.0 || tilt != 0.0).then_some((pan, tilt))
}

pub async fn run(mgr: Arc<Manager>) {
    let mut states: HashMap<String, State> = HashMap::new();
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let cfg = mgr.config();
        for cam in cfg.cameras.iter().filter(|c| c.enabled && c.ptz.enabled && (c.ptz.follow || (!c.ptz.patrol.is_empty() && c.ptz.patrol_seconds > 0))) {
            let st = states.entry(cam.id.clone()).or_default();
            if MANUAL.lock().unwrap().get(&cam.id).is_some_and(|t| t.elapsed() < HANDS_OFF) {
                st.next_at = Some(Instant::now() + Duration::from_secs(cam.ptz.patrol_seconds.max(5) as u64));
                continue;
            }
            let Ok(t) = Target::from(cam, &cam.ptz) else { continue };
            let onvif = mgr.onvif(&cam.id);
            let det = mgr.camera(&cam.id).and_then(|r| r.detector()).map(|d| d.status());
            let p = &cam.ptz;
            // Follow.
            if p.follow
                && let Some(d) = &det
            {
                let kinds: Vec<String> = if p.follow_objects.is_empty() { vec!["person".into()] } else { p.follow_objects.clone() };
                let target = d
                    .seen_at
                    .filter(|t| t.elapsed() < Duration::from_secs(2))
                    .and_then(|_| d.seen_now.iter().filter(|b| kinds.contains(&b.label)).max_by(|a, b| (a.w * a.h).total_cmp(&(b.w * b.h))));
                if let Some(b) = target {
                    st.last_seen = Some(Instant::now());
                    if !st.following {
                        st.following = true;
                        info!("[{}] following a {}", cam.name, b.label);
                    }
                    // One nudge, then let the AI look again.
                    if st.last_nudge.is_none_or(|n| n.elapsed() > Duration::from_millis(1300))
                        && let Some((pan, tilt)) = nudge((b.x, b.y, b.w, b.h))
                    {
                        st.last_nudge = Some(Instant::now());
                        let (sx, sy) = (if p.invert_pan { -1.0 } else { 1.0 }, if p.invert_tilt { -1.0 } else { 1.0 });
                        if let Err(e) = onvif.move_continuous(&t, pan * p.speed * sx, tilt * p.speed * sy, 0.0).await {
                            warn!("[{}] follow: {e:#}", cam.name);
                        }
                        tokio::time::sleep(Duration::from_millis(350)).await;
                        let _ = onvif.stop(&t).await;
                    }
                    continue;
                }
                if st.following && st.last_seen.is_some_and(|t| t.elapsed() > Duration::from_secs(10)) {
                    st.following = false;
                    info!("[{}] nothing to follow any more: going back", cam.name);
                    let back = match p.patrol.first() {
                        Some(tok) => onvif.goto_preset(&t, tok).await,
                        None => onvif.home(&t).await,
                    };
                    if let Err(e) = back {
                        warn!("[{}] going back: {e:#}", cam.name);
                    }
                    st.next_at = Some(Instant::now() + Duration::from_secs(p.patrol_seconds.max(5) as u64));
                }
                if st.following {
                    continue;
                }
            }
            // Patrol (paused while something is going on).
            if p.patrol.is_empty() || p.patrol_seconds == 0 || det.as_ref().is_some_and(|d| d.event_active) {
                continue;
            }
            if st.next_at.is_none_or(|n| Instant::now() >= n) {
                let tok = &p.patrol[st.next % p.patrol.len()];
                if let Err(e) = onvif.goto_preset(&t, tok).await {
                    warn!("[{}] patrol: {e:#}", cam.name);
                }
                st.next = (st.next + 1) % p.patrol.len();
                st.next_at = Some(Instant::now() + Duration::from_secs(p.patrol_seconds.max(5) as u64));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nudges_towards_whats_off_centre() {
        assert_eq!(nudge((0.4, 0.3, 0.2, 0.3)), None); // in the middle
        let (pan, tilt) = nudge((0.8, 0.3, 0.15, 0.3)).unwrap(); // to the right
        assert!(pan > 0.5 && tilt == 0.0);
        let (pan, tilt) = nudge((0.1, 0.7, 0.1, 0.25)).unwrap(); // bottom left
        assert!(pan < 0.0 && tilt < 0.0);
    }
}
