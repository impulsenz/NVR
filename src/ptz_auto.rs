//! Moving cameras on their own (camera → Pan / tilt / zoom):
//! - Roam: visit chosen presets in turn, staying a set time at each (0 =
//!   straight on to the next); paused for a while after someone steers by
//!   hand.
//! - Follow: when the AI finds a person (or another chosen kind) off-centre,
//!   nudge the camera towards it; when it's gone, carry on roaming, or go
//!   back (first roam preset, or the camera's home position).
//! While the camera moves itself, its movement isn't counted as motion.

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

/// The camera is moving by itself (roaming, following) or has only just
/// stopped: its own movement isn't motion in the picture.
static SETTLE: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(Default::default);
/// After a move ends, the picture settles for this long.
const SETTLE_AFTER: Duration = Duration::from_millis(1500);

pub fn settling(camera: &str) -> bool {
    SETTLE.lock().unwrap().get(camera).is_some_and(|t| Instant::now() < *t)
}

fn settle_until(camera: &str, t: Instant) {
    SETTLE.lock().unwrap().insert(camera.to_string(), t);
}

/// The roaming / tracking settings were changed: start afresh, now.
pub fn reset(camera: &str) {
    MANUAL.lock().unwrap().remove(camera);
    RESET.lock().unwrap().push(camera.to_string());
}
static RESET: LazyLock<Mutex<Vec<String>>> = LazyLock::new(Default::default);

#[derive(Default)]
struct State {
    next: usize,
    /// When to move to the next preset (None: now).
    next_at: Option<Instant>,
    /// A move to a preset is under way, since then.
    moving_since: Option<Instant>,
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
        tokio::time::sleep(Duration::from_millis(400)).await;
        for c in std::mem::take(&mut *RESET.lock().unwrap()) {
            states.remove(&c);
        }
        let cfg = mgr.config();
        for cam in cfg.cameras.iter().filter(|c| c.enabled && c.ptz.enabled && (c.ptz.follow || c.ptz.roaming())) {
            let st = states.entry(cam.id.clone()).or_default();
            let p = &cam.ptz;
            let stay = Duration::from_secs(p.patrol_seconds as u64);
            if MANUAL.lock().unwrap().get(&cam.id).is_some_and(|t| t.elapsed() < HANDS_OFF) {
                st.next_at = Some(Instant::now() + stay);
                st.moving_since = None;
                continue;
            }
            let Ok(t) = Target::from(cam, &cam.ptz) else { continue };
            let onvif = mgr.onvif(&cam.id);
            let det = mgr.camera(&cam.id).and_then(|r| r.detector()).map(|d| d.status());
            // Follow: a person (or what's chosen) found by the AI takes over
            // from roaming until it's gone.
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
                        st.moving_since = None;
                        // Stop where it is (a roam move may be under way).
                        let _ = onvif.stop(&t).await;
                        info!("[{}] following a {}", cam.name, b.label);
                    }
                    // One nudge, then let the AI look again.
                    if st.last_nudge.is_none_or(|n| n.elapsed() > Duration::from_millis(1300))
                        && let Some((pan, tilt)) = nudge((b.x, b.y, b.w, b.h))
                    {
                        st.last_nudge = Some(Instant::now());
                        settle_until(&cam.id, Instant::now() + Duration::from_millis(350) + SETTLE_AFTER);
                        let (sx, sy) = (if p.invert_pan { -1.0 } else { 1.0 }, if p.invert_tilt { -1.0 } else { 1.0 });
                        if let Err(e) = onvif.move_continuous(&t, pan * p.speed * sx, tilt * p.speed * sy, 0.0).await {
                            warn!("[{}] follow: {e:#}", cam.name);
                        }
                        tokio::time::sleep(Duration::from_millis(350)).await;
                        let _ = onvif.stop(&t).await;
                    }
                    continue;
                }
                if st.following && st.last_seen.is_some_and(|t| t.elapsed() > Duration::from_secs(8)) {
                    st.following = false;
                    info!("[{}] nobody to follow any more: going back", cam.name);
                    if p.roaming() {
                        // Carry on roaming, from the next preset.
                        st.next_at = None;
                    } else {
                        settle_until(&cam.id, Instant::now() + Duration::from_secs(6));
                        let back = match p.patrol.first() {
                            Some(tok) => onvif.goto_preset(&t, tok).await,
                            None => onvif.home(&t).await,
                        };
                        if let Err(e) = back {
                            warn!("[{}] going back: {e:#}", cam.name);
                        }
                    }
                }
                if st.following {
                    continue;
                }
            }
            if !p.roaming() {
                continue;
            }
            // A move under way: wait until the camera's there (it says so,
            // or after a few seconds when it doesn't), then stay.
            if let Some(since) = st.moving_since {
                let arrived = match onvif.moving(&t).await {
                    Some(m) => !m && since.elapsed() > Duration::from_millis(600),
                    None => since.elapsed() > Duration::from_secs(4),
                } || since.elapsed() > Duration::from_secs(20);
                if !arrived {
                    settle_until(&cam.id, Instant::now() + SETTLE_AFTER);
                    continue;
                }
                st.moving_since = None;
                st.next_at = Some(Instant::now() + stay);
                settle_until(&cam.id, Instant::now() + SETTLE_AFTER);
            }
            if st.next_at.is_none_or(|n| Instant::now() >= n) {
                let tok = &p.patrol[st.next % p.patrol.len()];
                settle_until(&cam.id, Instant::now() + Duration::from_secs(20));
                if let Err(e) = onvif.goto_preset(&t, tok).await {
                    warn!("[{}] roaming: {e:#}", cam.name);
                    st.next_at = Some(Instant::now() + Duration::from_secs(10));
                    settle_until(&cam.id, Instant::now());
                } else {
                    st.moving_since = Some(Instant::now());
                }
                st.next = (st.next + 1) % p.patrol.len();
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
