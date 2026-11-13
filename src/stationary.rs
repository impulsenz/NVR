//! Parked (stationary) objects: a car in the driveway shouldn't make an
//! alert every time a leaf moves. An object of a watched kind that stays in
//! the same place for longer than the set time is marked parked and no
//! longer alerts; when it moves (its box shifts) it alerts again.
//!
//! An object is forgotten only after the AI has looked at the picture
//! several times without finding it (so a car parked all night stays
//! parked through quiet hours, when the AI doesn't run).

use crate::ai::Detection;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Kinds that can park, by default.
pub const PARKING: &[&str] = &["vehicle", "bicycle", "boat"];
/// Misses (AI runs without finding it) before a parked object is forgotten.
const FORGET_AFTER: u32 = 10;
/// Sightings in a row somewhere else before a parked object counts as
/// moving (one odd box doesn't wake it up).
const MOVED_AFTER: u32 = 2;

/// Two boxes of the same object are the same place: they overlap well, or
/// (boxes of something partly out of the picture, or half hidden, jitter a
/// lot) the middle hardly moved and the size is about the same.
fn same_place(a: &[f32; 4], b: &[f32; 4]) -> bool {
    if iou(a, b) >= 0.45 {
        return true;
    }
    let (ca, cb) = (
        [a[0] + a[2] / 2.0, a[1] + a[3] / 2.0],
        [b[0] + b[2] / 2.0, b[1] + b[3] / 2.0],
    );
    let size = a[2].max(a[3]).max(b[2]).max(b[3]);
    let dist = ((ca[0] - cb[0]).powi(2) + (ca[1] - cb[1]).powi(2)).sqrt();
    let ratio = (a[2] * a[3]).max(1e-6) / (b[2] * b[3]).max(1e-6);
    dist < 0.2 * size && (0.5..=2.0).contains(&ratio)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Item {
    label: String,
    /// Where it stood when it stopped moving.
    b: [f32; 4],
    parked: bool,
    #[serde(skip, default = "Instant::now")]
    since: Instant,
    #[serde(skip)]
    misses: u32,
    /// Sightings in a row somewhere else (a parked object moving off).
    #[serde(skip)]
    moved: u32,
}

pub struct Stationary {
    items: Vec<Item>,
    file: Option<PathBuf>,
}

fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let ix = (a[0] + a[2]).min(b[0] + b[2]) - a[0].max(b[0]);
    let iy = (a[1] + a[3]).min(b[1] + b[3]) - a[1].max(b[1]);
    if ix <= 0.0 || iy <= 0.0 {
        return 0.0;
    }
    let i = ix * iy;
    i / (a[2] * a[3] + b[2] * b[3] - i).max(1e-6)
}

/// Car, truck and bus are often mistaken for one another: all one vehicle
/// (also what was remembered from before they were one kind).
fn group(label: &str) -> &str {
    crate::ai::canon(label)
}

impl Stationary {
    /// `file`: where parked objects are remembered across restarts.
    pub fn open(file: Option<PathBuf>) -> Self {
        let items: Vec<Item> = file
            .as_ref()
            .and_then(|f| std::fs::read(f).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            items: items.into_iter().filter(|i| i.parked).collect(),
            file,
        }
    }

    fn save(&self) {
        if let Some(f) = &self.file {
            let parked: Vec<&Item> = self.items.iter().filter(|i| i.parked).collect();
            let _ = std::fs::write(f, serde_json::to_vec(&parked).unwrap_or_default());
        }
    }

    /// What's parked now ("car", …), for the camera's status.
    pub fn parked(&self) -> Vec<String> {
        self.items
            .iter()
            .filter(|i| i.parked)
            .map(|i| group(&i.label).to_string())
            .collect()
    }

    /// Called with everything the AI found in a picture (of the kinds that
    /// can alert). Returns the detections that should still alert, and
    /// what changed ("car parked", "parked car moved").
    pub fn filter(
        &mut self,
        dets: Vec<Detection>,
        kinds: &[String],
        after: Duration,
    ) -> (Vec<Detection>, Vec<String>) {
        self.filter_at(dets, kinds, after, Instant::now())
    }

    fn filter_at(
        &mut self,
        dets: Vec<Detection>,
        kinds: &[String],
        after: Duration,
        now: Instant,
    ) -> (Vec<Detection>, Vec<String>) {
        let mut keep = Vec::new();
        let mut notes = Vec::new();
        let mut seen = vec![false; self.items.len()];
        let mut changed = false;
        for d in dets {
            if !kinds.iter().any(|k| group(k) == group(&d.label)) {
                keep.push(d);
                continue;
            }
            let b = [d.x, d.y, d.w, d.h];
            // The best matching tracked object of the same kind.
            let best = self
                .items
                .iter()
                .enumerate()
                .filter(|(i, it)| !seen[*i] && group(&it.label) == group(&d.label))
                .map(|(i, it)| (i, iou(&it.b, &b), same_place(&it.b, &b)))
                .filter(|(_, o, same)| *o > 0.1 || *same)
                .max_by(|a, b| {
                    (a.2, a.1)
                        .partial_cmp(&(b.2, b.1))
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            match best {
                Some((i, _, true)) => {
                    seen[i] = true;
                    let it = &mut self.items[i];
                    it.misses = 0;
                    it.moved = 0;
                    if it.parked {
                        continue; // still parked: no alert
                    }
                    if now.duration_since(it.since) >= after {
                        it.parked = true;
                        changed = true;
                        notes.push(format!(
                            "{} stopped here — ignored until it moves",
                            cap(&it.label)
                        ));
                        continue;
                    }
                    keep.push(d);
                }
                Some((i, _, false)) => {
                    seen[i] = true;
                    let it = &mut self.items[i];
                    it.misses = 0;
                    // Parked: only moving once it's seen elsewhere again.
                    if it.parked {
                        it.moved += 1;
                        if it.moved < MOVED_AFTER {
                            continue;
                        }
                        notes.push(format!("Parked {} is moving", it.label));
                        changed = true;
                    }
                    // It moved: alert again, start over.
                    *it = Item {
                        label: d.label.clone(),
                        b,
                        parked: false,
                        since: now,
                        misses: 0,
                        moved: 0,
                    };
                    keep.push(d);
                }
                None => {
                    self.items.push(Item {
                        label: d.label.clone(),
                        b,
                        parked: false,
                        since: now,
                        misses: 0,
                        moved: 0,
                    });
                    seen.push(true);
                    keep.push(d);
                }
            }
        }
        // Not found this time.
        let before = self.items.len();
        for (it, s) in self.items.iter_mut().zip(&seen) {
            if !s {
                it.misses += 1;
            }
        }
        self.items.retain(|it| it.misses < FORGET_AFTER);
        if self.items.len() != before {
            changed = true;
        }
        if changed {
            self.save();
        }
        (keep, notes)
    }
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_a_car_at_the_edge_still_parks() {
        // A car cut off by the top of the picture: its box jitters a lot.
        let mut s = Stationary::open(None);
        let kinds: Vec<String> = PARKING.iter().map(|s| s.to_string()).collect();
        let after = Duration::from_secs(60);
        let t0 = Instant::now();
        let car = |x: f32, w: f32, h: f32| Detection {
            label: "car".into(),
            score: 0.75,
            x,
            y: 0.0,
            w,
            h,
            ..Default::default()
        };
        assert_eq!(
            s.filter_at(vec![car(0.40, 0.20, 0.08)], &kinds, after, t0)
                .0
                .len(),
            1
        );
        // Jittery boxes of the same car over the next minutes: parked, then quiet.
        let looks = [
            (0.42, 0.17, 0.05),
            (0.39, 0.22, 0.09),
            (0.41, 0.18, 0.06),
            (0.40, 0.21, 0.07),
        ];
        let mut alerts = 0;
        for (i, (x, w, h)) in looks.iter().enumerate() {
            alerts += s
                .filter_at(
                    vec![car(*x, *w, *h)],
                    &kinds,
                    after,
                    t0 + Duration::from_secs(70 * (i as u64 + 1)),
                )
                .0
                .len();
        }
        assert_eq!(alerts, 0);
        assert_eq!(s.parked(), ["vehicle"]);
    }

    fn det(label: &str, x: f32) -> Detection {
        Detection {
            label: label.into(),
            score: 0.9,
            x,
            y: 0.5,
            w: 0.2,
            h: 0.2,
            ..Default::default()
        }
    }

    #[test]
    fn parked_cars_stop_alerting_until_they_move() {
        let mut s = Stationary::open(None);
        let kinds: Vec<String> = PARKING.iter().map(|s| s.to_string()).collect();
        let after = Duration::from_secs(60);
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        // Arrives: alerts.
        assert_eq!(
            s.filter_at(vec![det("car", 0.30)], &kinds, after, at(0))
                .0
                .len(),
            1
        );
        assert_eq!(
            s.filter_at(vec![det("car", 0.305)], &kinds, after, at(30))
                .0
                .len(),
            1
        );
        // Stood still for a minute: parked, no alert.
        let (k, n) = s.filter_at(vec![det("truck", 0.30)], &kinds, after, at(61));
        assert!(k.is_empty() && n[0].contains("stopped"));
        // A person walking past still alerts; the car stays quiet.
        let (k, _) = s.filter_at(
            vec![det("car", 0.31), det("person", 0.7)],
            &kinds,
            after,
            at(3600),
        );
        assert_eq!(
            k.iter().map(|d| d.label.as_str()).collect::<Vec<_>>(),
            ["person"]
        );
        // A few misses (someone in front of it) don't forget it.
        for i in 0..3 {
            s.filter_at(vec![], &kinds, after, at(3700 + i));
        }
        assert!(
            s.filter_at(vec![det("car", 0.30)], &kinds, after, at(3800))
                .0
                .is_empty()
        );
        // One odd box elsewhere doesn't wake it; seen moved again, it alerts.
        assert!(
            s.filter_at(vec![det("car", 0.42)], &kinds, after, at(3890))
                .0
                .is_empty()
        );
        let (k, n) = s.filter_at(vec![det("car", 0.45)], &kinds, after, at(3900));
        assert_eq!(k.len(), 1);
        assert!(n[0].contains("moving"));
        // Gone for good after enough misses.
        for i in 0..FORGET_AFTER as u64 {
            s.filter_at(vec![], &kinds, after, at(4000 + i));
        }
        assert!(s.items.is_empty());
    }
}
