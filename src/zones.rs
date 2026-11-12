//! Zones: named areas and lines on the picture.
//!
//! - **Area**: a polygon. Objects count as "in" it when the bottom middle
//!   of their box (their feet / wheels) is inside. Each area can be limited
//!   to some objects, can be *required* (alerts only for objects inside a
//!   required area), can flag **loitering** (an object staying longer than
//!   N seconds) and can watch for an **item left or taken** (the area looks
//!   different once the activity is over: a parcel dropped off, something
//!   stolen).
//! - **Line**: two points. An object whose path crosses it is reported,
//!   optionally only in one direction.
//!
//! Objects are followed from one AI check to the next by a small tracker
//! (same label, nearest position), which crossing and loitering need.

use crate::ai::Detection;
use crate::config::ZoneConfig;
use std::time::{Duration, Instant};

/// Point in polygon (ray casting); points are 0..1 of the picture.
pub fn inside(poly: &[[f32; 2]], p: [f32; 2]) -> bool {
    let mut c = false;
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            c = !c;
        }
        j = i;
    }
    c
}

/// Which side of line a→b point p is on (+1 / -1 / 0).
fn side(a: [f32; 2], b: [f32; 2], p: [f32; 2]) -> f32 {
    ((b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])).signum()
}

/// Does the move p→q cross the segment a→b? Returns the direction:
/// Some(true) = from the left of a→b to its right ("A to B" side), Some(false) the other way.
pub fn crossing(a: [f32; 2], b: [f32; 2], p: [f32; 2], q: [f32; 2]) -> Option<bool> {
    let (s1, s2) = (side(a, b, p), side(a, b, q));
    if s1 == 0.0 || s2 == 0.0 || s1 == s2 {
        return None;
    }
    // The move must also straddle the line's own extent.
    if side(p, q, a) == side(p, q, b) {
        return None;
    }
    Some(s1 < 0.0)
}

/// The point that decides "in a zone": bottom middle of the box.
pub fn foot(d: &Detection) -> [f32; 2] {
    [d.x + d.w / 2.0, (d.y + d.h).min(1.0)]
}

struct Track {
    label: String,
    pos: [f32; 2],
    seen: Instant,
    /// Since when it has been in each area (by zone index).
    entered: Vec<Option<Instant>>,
    /// Zones already reported for this object (enter / loiter / cross).
    told: Vec<String>,
    /// Speeds measured in each area that has a real length (km/h).
    speeds: Vec<Vec<f32>>,
}

/// The area's length along its longer side, in picture units (16 × 9),
/// and which way that is (true = left–right).
fn long_axis(points: &[[f32; 2]]) -> (f32, bool) {
    let (mut x0, mut x1, mut y0, mut y1) = (1f32, 0f32, 1f32, 0f32);
    for p in points {
        x0 = x0.min(p[0]);
        x1 = x1.max(p[0]);
        y0 = y0.min(p[1]);
        y1 = y1.max(p[1]);
    }
    let (w, h) = ((x1 - x0) * 16.0, (y1 - y0) * 9.0);
    if w >= h { (w, true) } else { (h, false) }
}

/// Speed (km/h) of something that moved from `a` to `b` in `secs`, in an
/// area whose long side is `metres` long.
pub fn speed_kmh(points: &[[f32; 2]], metres: f32, a: [f32; 2], b: [f32; 2], secs: f32) -> Option<f32> {
    let (len, across) = long_axis(points);
    if len <= 0.0 || secs <= 0.05 || metres <= 0.0 {
        return None;
    }
    let moved = if across { (b[0] - a[0]).abs() * 16.0 } else { (b[1] - a[1]).abs() * 9.0 };
    Some(moved / len * metres / secs * 3.6)
}

/// Follows objects between AI checks and reports zone happenings.
#[derive(Default)]
pub struct ZoneWatch {
    tracks: Vec<Track>,
}

/// What the zones made of one AI check.
#[derive(Default, Debug)]
pub struct ZoneReport {
    /// Detections that may raise an alert (required areas respected).
    pub keep: Vec<Detection>,
    /// Things to note on the alert ("Person entered Porch", "Car crossed Gate (in)").
    pub notes: Vec<String>,
    /// Zones with objects in them.
    pub zones: Vec<String>,
}

impl ZoneWatch {
    pub fn check(&mut self, zones: &[ZoneConfig], dets: Vec<Detection>) -> ZoneReport {
        let now = Instant::now();
        let mut r = ZoneReport::default();
        self.tracks
            .retain(|t| now.duration_since(t.seen) < Duration::from_secs(20));
        let required: Vec<usize> = (0..zones.len())
            .filter(|&i| zones[i].kind == "area" && zones[i].required)
            .collect();
        for d in dets {
            let p = foot(&d);
            // The same object as before: same label, nearest within reach.
            let ti = self
                .tracks
                .iter()
                .enumerate()
                .filter(|(_, t)| t.label == d.label)
                .map(|(i, t)| (i, (t.pos[0] - p[0]).hypot(t.pos[1] - p[1])))
                .filter(|(_, dist)| *dist < 0.35)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i);
            let ti = match ti {
                Some(i) => i,
                None => {
                    self.tracks.push(Track {
                        label: d.label.clone(),
                        pos: p,
                        seen: now,
                        entered: vec![None; zones.len()],
                        told: vec![],
                        speeds: vec![Vec::new(); zones.len()],
                    });
                    self.tracks.len() - 1
                }
            };
            let t = &mut self.tracks[ti];
            t.entered.resize(zones.len(), None);
            t.speeds.resize(zones.len(), Vec::new());
            let dt = now.duration_since(t.seen).as_secs_f32();
            let prev = t.pos;
            let name = cap(&d.label);
            let mut in_required = false;
            for (zi, z) in zones.iter().enumerate() {
                if !z.objects.is_empty() && !z.objects.contains(&d.label) {
                    continue;
                }
                if z.kind == "line" {
                    if z.points.len() == 2
                        && let Some(dir) = crossing(z.points[0], z.points[1], prev, p)
                    {
                        let ok = match z.direction.as_str() {
                            "ab" => dir,
                            "ba" => !dir,
                            _ => true,
                        };
                        let key = format!("cross:{}", z.name);
                        if ok && !t.told.contains(&key) {
                            t.told.push(key);
                            r.notes.push(format!(
                                "{name} crossed {} ({})",
                                z.name,
                                if dir { "A→B" } else { "B→A" }
                            ));
                            if !r.zones.contains(&z.name) {
                                r.zones.push(z.name.clone());
                            }
                        }
                    }
                    continue;
                }
                if !inside(&z.points, p) {
                    // Left an area with a real length: how fast it went.
                    if t.entered[zi].is_some() && !t.speeds[zi].is_empty() {
                        let mut v = std::mem::take(&mut t.speeds[zi]);
                        v.sort_by(f32::total_cmp);
                        let kmh = v[v.len() / 2];
                        let key = format!("speed:{}", z.name);
                        if !t.told.contains(&key) && kmh >= 1.0 {
                            t.told.push(key);
                            r.notes.push(format!("{name} going {:.0} km/h through {}", kmh, z.name));
                        }
                    }
                    t.entered[zi] = None;
                    continue;
                }
                // Moving inside an area with a real length: a speed sample.
                if z.distance_m > 0.0
                    && t.entered[zi].is_some()
                    && inside(&z.points, prev)
                    && let Some(kmh) = speed_kmh(&z.points, z.distance_m, prev, p, dt)
                    && kmh < 250.0
                {
                    t.speeds[zi].push(kmh);
                }
                if z.required {
                    in_required = true;
                }
                if !r.zones.contains(&z.name) {
                    r.zones.push(z.name.clone());
                }
                let since = *t.entered[zi].get_or_insert(now);
                let key = format!("in:{}", z.name);
                if !t.told.contains(&key) {
                    t.told.push(key);
                    r.notes.push(format!("{name} in {}", z.name));
                }
                if z.loiter_seconds > 0
                    && now.duration_since(since).as_secs() >= z.loiter_seconds as u64
                {
                    let key = format!("loiter:{}", z.name);
                    if !t.told.contains(&key) {
                        t.told.push(key);
                        r.notes.push(format!(
                            "{name} loitering in {} ({} s+)",
                            z.name, z.loiter_seconds
                        ));
                    }
                }
            }
            t.pos = p;
            t.seen = now;
            if required.is_empty() || in_required {
                r.keep.push(d);
            }
        }
        r
    }
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}


/// Item left / taken: a small grey picture of an area, compared before and
/// after an alert: the points of its box
/// (x0, y0, x1, y1; 0..1 of the picture) that are in the area.
pub fn area_signature_by(
    f: &crate::imgutil::Frame,
    (x0, y0, x1, y1): (f32, f32, f32, f32),
    contains: impl Fn([f32; 2]) -> bool,
) -> Vec<f32> {
    const N: usize = 32;
    let mut out = Vec::new();
    for gy in 0..N {
        for gx in 0..N {
            let p = [
                x0 + (x1 - x0) * (gx as f32 + 0.5) / N as f32,
                y0 + (y1 - y0) * (gy as f32 + 0.5) / N as f32,
            ];
            if !contains(p) {
                continue;
            }
            let x = ((p[0] * f.width as f32) as u32).min(f.width - 1);
            let y = ((p[1] * f.height as f32) as u32).min(f.height - 1);
            let i = ((y * f.width + x) * 3) as usize;
            out.push((f.rgb[i] as f32 + f.rgb[i + 1] as f32 + f.rgb[i + 2] as f32) / 3.0);
        }
    }
    out
}

/// How different an area looks (0..1 share of its points that changed
/// clearly, ignoring an overall brightness shift).
pub fn area_change(before: &[f32], after: &[f32]) -> f32 {
    if before.len() != after.len() || before.is_empty() {
        return 0.0;
    }
    let n = before.len() as f32;
    let shift = after.iter().sum::<f32>() / n - before.iter().sum::<f32>() / n;
    before
        .iter()
        .zip(after)
        .filter(|(b, a)| (*a - *b - shift).abs() > 28.0)
        .count() as f32
        / n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn det(label: &str, cx: f32, foot_y: f32) -> Detection {
        Detection {
            label: label.into(),
            score: 0.9,
            x: cx - 0.05,
            y: foot_y - 0.2,
            w: 0.1,
            h: 0.2,
        }
    }

    fn zone(name: &str, kind: &str, points: Vec<[f32; 2]>) -> ZoneConfig {
        ZoneConfig {
            name: name.into(),
            kind: kind.into(),
            points,
            ..Default::default()
        }
    }

    #[test]
    fn speed_through_an_area() {
        // A 20 m stretch of road across the whole picture.
        let road = vec![[0.0, 0.4], [1.0, 0.4], [1.0, 0.6], [0.0, 0.6]];
        // A third of the way across in a second: 6.7 m/s ≈ 24 km/h.
        let kmh = speed_kmh(&road, 20.0, [0.2, 0.5], [0.5333, 0.5], 1.0).unwrap();
        assert!((kmh - 24.0).abs() < 0.5, "{kmh}");
        assert!(speed_kmh(&road, 0.0, [0.2, 0.5], [0.5, 0.5], 1.0).is_none());
    }

    #[test]
    fn areas_lines_and_required() {
        let porch = zone(
            "Porch",
            "area",
            vec![[0.5, 0.5], [1.0, 0.5], [1.0, 1.0], [0.5, 1.0]],
        );
        assert!(inside(&porch.points, [0.7, 0.8]));
        assert!(!inside(&porch.points, [0.2, 0.8]));
        let mut gate = zone("Gate", "line", vec![[0.4, 0.0], [0.4, 1.0]]);
        gate.direction = "any".into();
        let zones = vec![porch.clone(), gate];
        let mut w = ZoneWatch::default();
        let r = w.check(&zones, vec![det("person", 0.3, 0.8)]);
        assert!(r.notes.is_empty(), "{:?}", r.notes);
        let r = w.check(&zones, vec![det("person", 0.6, 0.8)]);
        assert!(
            r.notes.iter().any(|n| n.starts_with("Person crossed Gate")),
            "{:?}",
            r.notes
        );
        assert!(
            r.notes.iter().any(|n| n == "Person in Porch"),
            "{:?}",
            r.notes
        );
        // Required area: objects outside it don't alert.
        let mut req = porch;
        req.required = true;
        let mut w = ZoneWatch::default();
        let r = w.check(&[req], vec![det("car", 0.2, 0.3), det("person", 0.8, 0.9)]);
        assert_eq!(r.keep.len(), 1);
        assert_eq!(r.keep[0].label, "person");
    }

    #[test]
    fn item_left_or_taken() {
        let poly = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let mk = |v: u8, box_: bool| {
            let (w, h) = (64u32, 36u32);
            let mut rgb = vec![v; (w * h * 3) as usize];
            if box_ {
                for y in 10..30 {
                    for x in 20..44 {
                        let i = ((y * w + x) * 3) as usize;
                        rgb[i..i + 3].copy_from_slice(&[180, 140, 90]);
                    }
                }
            }
            crate::imgutil::Frame {
                width: w,
                height: h,
                rgb: std::sync::Arc::new(rgb),
            }
        };
        let before = area_signature_by(&mk(60, false), (0.0, 0.0, 1.0, 1.0), |p| inside(&poly, p));
        assert!(
            area_change(&before, &area_signature_by(&mk(80, false), (0.0, 0.0, 1.0, 1.0), |p| inside(&poly, p))) < 0.05,
            "brightness change ignored"
        );
        assert!(
            area_change(&before, &area_signature_by(&mk(60, true), (0.0, 0.0, 1.0, 1.0), |p| inside(&poly, p))) > 0.15,
            "parcel noticed"
        );
    }
}
