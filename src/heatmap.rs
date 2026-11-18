//! Where things happen on a camera (Settings → camera → Motion → "Where
//! alerts happened"): the boxes stored with each alert (what was detected,
//! as fractions of the picture) added up into a coarse grid. Painting ignore
//! areas over the hot patches that shouldn't count (a swaying tree, a road)
//! is then a matter of looking, not guessing.
//!
//! Only reads alerts that are already stored: nothing is recorded for it and
//! recording and detection never use it. It shows where alerts were *raised*:
//! a movement that was already ignored left no alert and isn't in it.

use crate::events::Event;
use chrono::{DateTime, Local};
use serde::Serialize;

pub const COLS: usize = 64;
pub const ROWS: usize = 36;

/// Which detections to count.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    All,
    People,
    Vehicles,
    Animals,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "" | "all" => Kind::All,
            "people" | "person" => Kind::People,
            "vehicles" | "vehicle" => Kind::Vehicles,
            "animals" | "animal" => Kind::Animals,
            _ => return None,
        })
    }

    fn takes(self, label: &str) -> bool {
        let vehicle = matches!(label, "vehicle" | "bicycle");
        match self {
            Kind::All => true,
            Kind::People => label == "person",
            Kind::Vehicles => vehicle,
            // Everything else that isn't a parcel: dogs, cats, birds, horses …
            Kind::Animals => label != "person" && !vehicle && label != "package",
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Heatmap {
    pub cols: usize,
    pub rows: usize,
    /// Row by row, left to right: how many detections covered each cell.
    pub cells: Vec<u32>,
    pub max: u32,
    /// Alerts in the period that had a picture with something in it.
    pub alerts: usize,
    /// Detections counted.
    pub detections: usize,
}

/// Add up the detections of the alerts in `events` that started in
/// `from..=to`. A detection covers every cell its box touches.
pub fn build(events: &[Event], from: DateTime<Local>, to: DateTime<Local>, kind: Kind) -> Heatmap {
    let mut cells = vec![0u32; COLS * ROWS];
    let (mut alerts, mut detections) = (0, 0);
    for e in events.iter().filter(|e| e.start >= from && e.start <= to) {
        let mut any = false;
        for b in e.boxes.iter().filter(|b| kind.takes(&b.label)) {
            if ![b.x, b.y, b.w, b.h].iter().all(|v| v.is_finite()) || b.w <= 0.0 || b.h <= 0.0 {
                continue;
            }
            let (x0, y0) = (b.x.clamp(0.0, 1.0), b.y.clamp(0.0, 1.0));
            let (x1, y1) = ((b.x + b.w).clamp(0.0, 1.0), (b.y + b.h).clamp(0.0, 1.0));
            if x1 <= x0 || y1 <= y0 {
                continue; // wholly outside the picture
            }
            let (c0, c1) = (((x0 * COLS as f32) as usize).min(COLS - 1), (((x1 * COLS as f32).ceil() as usize).max(1) - 1).min(COLS - 1));
            let (r0, r1) = (((y0 * ROWS as f32) as usize).min(ROWS - 1), (((y1 * ROWS as f32).ceil() as usize).max(1) - 1).min(ROWS - 1));
            for r in r0..=r1 {
                for c in c0..=c1 {
                    cells[r * COLS + c] += 1;
                }
            }
            detections += 1;
            any = true;
        }
        if any {
            alerts += 1;
        }
    }
    let max = cells.iter().copied().max().unwrap_or(0);
    Heatmap { cols: COLS, rows: ROWS, cells, max, alerts, detections }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::SeenBox;

    fn event(start: DateTime<Local>, boxes: Vec<(&str, f32, f32, f32, f32)>) -> Event {
        let mut e: Event = serde_json::from_value(serde_json::json!({"id": "e", "camera": "c", "start": start, "end": start})).unwrap();
        e.boxes = boxes.into_iter().map(|(l, x, y, w, h)| SeenBox { label: l.into(), score: 0.9, x, y, w, h }).collect();
        e
    }

    fn at(h: i64) -> DateTime<Local> {
        chrono::DateTime::parse_from_rfc3339("2026-06-01T12:00:00+00:00").unwrap().with_timezone(&Local) - chrono::Duration::hours(h)
    }

    #[test]
    fn a_box_heats_exactly_the_cells_it_covers() {
        // The left half, top half of the picture.
        let h = build(&[event(at(1), vec![("person", 0.0, 0.0, 0.5, 0.5)])], at(24), at(0), Kind::All);
        assert_eq!((h.alerts, h.detections, h.max), (1, 1, 1));
        let hot = |r: usize, c: usize| h.cells[r * COLS + c];
        assert_eq!(hot(0, 0), 1);
        assert_eq!(hot(ROWS / 2 - 1, COLS / 2 - 1), 1);
        assert_eq!(hot(ROWS / 2, 0), 0, "below the box");
        assert_eq!(hot(0, COLS / 2), 0, "right of the box");
        assert_eq!(h.cells.iter().filter(|v| **v > 0).count(), (ROWS / 2) * (COLS / 2));
    }

    #[test]
    fn overlapping_detections_add_up_and_the_period_and_kind_are_respected() {
        let evs = [
            event(at(1), vec![("person", 0.4, 0.4, 0.2, 0.2)]),
            event(at(2), vec![("person", 0.4, 0.4, 0.2, 0.2), ("vehicle", 0.0, 0.8, 0.3, 0.2)]),
            event(at(100), vec![("person", 0.4, 0.4, 0.2, 0.2)]), // too long ago
            event(at(3), vec![("dog", 0.9, 0.9, 0.1, 0.1)]),
        ];
        let all = build(&evs, at(24), at(0), Kind::All);
        assert_eq!((all.alerts, all.detections), (3, 4));
        assert_eq!(all.max, 2, "two people in the same place");
        let people = build(&evs, at(24), at(0), Kind::People);
        assert_eq!((people.alerts, people.detections, people.max), (2, 2, 2));
        let vehicles = build(&evs, at(24), at(0), Kind::Vehicles);
        assert_eq!((vehicles.alerts, vehicles.detections), (1, 1));
        let animals = build(&evs, at(24), at(0), Kind::Animals);
        assert_eq!((animals.alerts, animals.detections), (1, 1));
        // The older alert counts when the period reaches it.
        assert_eq!(build(&evs, at(200), at(0), Kind::People).detections, 3);
    }

    #[test]
    fn rubbish_and_edge_boxes_never_panic_or_miscount() {
        let evs = [event(at(1), vec![
            ("person", f32::NAN, 0.0, 0.1, 0.1),
            ("person", 0.0, 0.0, 0.0, 0.5),   // no width
            ("person", 1.5, 1.5, 0.2, 0.2),   // wholly outside
            ("person", -0.5, -0.5, 0.6, 0.6), // partly outside: its inside part counts
            ("person", 0.95, 0.95, 0.5, 0.5), // at the bottom-right corner
        ])];
        let h = build(&evs, at(24), at(0), Kind::All);
        assert_eq!(h.detections, 2);
        assert!(h.cells[0] > 0 && h.cells[COLS * ROWS - 1] > 0);
        // Nothing to show is an empty (not failing) map.
        let none = build(&[], at(24), at(0), Kind::All);
        assert_eq!((none.alerts, none.max, none.cells.len()), (0, 0, COLS * ROWS));
    }

    #[test]
    fn kinds_are_parsed_and_unknown_ones_refused() {
        assert_eq!(Kind::parse(""), Some(Kind::All));
        assert_eq!(Kind::parse("people"), Some(Kind::People));
        assert_eq!(Kind::parse("animal"), Some(Kind::Animals));
        assert_eq!(Kind::parse("cars"), None);
    }
}
