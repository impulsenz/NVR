//! Words for what an alert shows, made when its picture is looked at for
//! description search (`clip.rs`) and kept with it: what was seen (person,
//! vehicle, dog …), the colour of each, whether it was night, who GAZE
//! recognised, and the camera. They are shown on the alert ("Looks like …")
//! and searched together with the picture's own meaning, so “red car” or
//! “dog at night” also finds what the words say, not only what a picture
//! model happens to pick up.

use crate::events::Event;
use crate::imgutil::Frame;
use chrono::Timelike;

/// What was found: words to search (lower case) and a sentence to read.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Described {
    pub words: Vec<String>,
    pub text: String,
}

/// The name of the colour that most of these pixels are (None if too few).
fn dominant_colour(frame: &Frame, x: f32, y: f32, w: f32, h: f32) -> Option<&'static str> {
    let (fw, fh) = (frame.width as f32, frame.height as f32);
    let (x0, y0) = ((x * fw).max(0.0) as usize, (y * fh).max(0.0) as usize);
    let (x1, y1) = (((x + w) * fw).min(fw) as usize, ((y + h) * fh).min(fh) as usize);
    let mut count = [0u32; 11];
    let mut total = 0u32;
    for py in (y0..y1).step_by(2) {
        for px in (x0..x1).step_by(2) {
            let i = (py * frame.width as usize + px) * 3;
            let Some(p) = frame.rgb.get(i..i + 3) else { continue };
            let (r, g, b) = (p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0);
            let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
            let (v, s) = (mx, if mx > 0.0 { (mx - mn) / mx } else { 0.0 });
            let d = mx - mn;
            let h = if d <= 0.0 {
                0.0
            } else if mx == r {
                60.0 * (((g - b) / d) % 6.0)
            } else if mx == g {
                60.0 * ((b - r) / d + 2.0)
            } else {
                60.0 * ((r - g) / d + 4.0)
            };
            let h = (h + 360.0) % 360.0;
            let c = if v < 0.2 {
                0 // black
            } else if s < 0.18 {
                if v > 0.75 { 1 } else { 2 } // white, grey
            } else if !(15.0..345.0).contains(&h) {
                3 // red
            } else if h < 42.0 {
                if v < 0.5 { 9 } else { 4 } // brown, orange
            } else if h < 70.0 {
                5 // yellow
            } else if h < 170.0 {
                6 // green
            } else if h < 260.0 {
                7 // blue
            } else {
                8 // purple / pink
            };
            count[c] += 1;
            total += 1;
        }
    }
    if total < 12 {
        return None;
    }
    const NAMES: [&str; 11] = ["black", "white", "grey", "red", "orange", "yellow", "green", "blue", "purple", "brown", ""];
    let (best, n) = count.iter().enumerate().max_by_key(|(_, n)| **n).map(|(i, n)| (i, *n))?;
    // Needs to be a good part of it (a mixed colour says nothing).
    (n * 100 / total >= 35 && !NAMES[best].is_empty()).then_some(NAMES[best])
}

/// Where the colour of a thing is looked for: the clothes of a person, the body of a vehicle.
fn colour_area(label: &str, b: &crate::events::SeenBox) -> (f32, f32, f32, f32) {
    match label {
        "person" => (b.x + b.w * 0.25, b.y + b.h * 0.2, b.w * 0.5, b.h * 0.4),
        "vehicle" => (b.x + b.w * 0.15, b.y + b.h * 0.3, b.w * 0.7, b.h * 0.5),
        _ => (b.x + b.w * 0.2, b.y + b.h * 0.2, b.w * 0.6, b.h * 0.6),
    }
}

/// Night, or the part of the day: by how dark (and how colourless: infrared)
/// the picture is, and by the clock.
fn time_of_day(frame: &Frame, hour: u32) -> &'static str {
    let n = (frame.rgb.len() / 3).max(1);
    let (mut luma, mut sat) = (0f64, 0f64);
    for p in frame.rgb.chunks_exact(3).step_by(7) {
        let (mx, mn) = (p[0].max(p[1]).max(p[2]) as f64, p[0].min(p[1]).min(p[2]) as f64);
        luma += mx;
        sat += if mx > 0.0 { (mx - mn) / mx } else { 0.0 };
    }
    let k = (n / 7).max(1) as f64;
    let (luma, sat) = (luma / k, sat / k);
    if luma < 55.0 || (sat < 0.1 && luma < 110.0) {
        return "night";
    }
    match hour {
        5..=8 => "morning",
        9..=16 => "daytime",
        17..=19 => "evening",
        _ => "night",
    }
}

/// Describe an alert from its picture, the boxes found in it, and what GAZE
/// recognised.
pub fn describe(e: &Event, frame: &Frame, camera: &str) -> Described {
    use std::collections::BTreeMap;
    let mut words: Vec<String> = Vec::new();
    let mut push = |w: &str| {
        let w = w.trim().to_lowercase();
        if !w.is_empty() && !words.contains(&w) {
            words.push(w);
        }
    };
    // What and how many; the colour of the biggest of each kind.
    let mut kinds: BTreeMap<String, (usize, f32, Option<&'static str>)> = BTreeMap::new();
    for b in e.boxes.iter().filter(|b| b.score >= 0.4 && b.label != "motion") {
        let label = crate::ai::canon(&b.label).to_string();
        let area = b.w * b.h;
        let (ax, ay, aw, ah) = colour_area(&label, b);
        let entry = kinds.entry(label.clone()).or_insert((0, 0.0, None));
        entry.0 += 1;
        if area > entry.1 {
            entry.1 = area;
            entry.2 = dominant_colour(frame, ax, ay, aw, ah);
        }
    }
    for l in &e.labels {
        let label = crate::ai::canon(&l.label).to_string();
        if label != "motion" {
            kinds.entry(label).or_insert((1, 0.0, None));
        }
    }
    let mut parts: Vec<String> = Vec::new();
    for (label, (n, _, colour)) in &kinds {
        push(label);
        if let Some(c) = colour {
            push(c);
        }
        let plain = match (label.as_str(), n, colour) {
            ("person", 1, Some(c)) => format!("a person in {c}"),
            ("person", 1, None) => "a person".to_string(),
            ("person", n, _) => format!("{n} people"),
            ("vehicle", 1, Some(c)) => format!("a {c} vehicle"),
            ("vehicle", 1, None) => "a vehicle".to_string(),
            ("vehicle", n, _) => format!("{n} vehicles"),
            (l, 1, Some(c)) => format!("a {l} ({c})"),
            (l, 1, None) => format!("a {l}"),
            (l, n, _) => format!("{n} {l}s"),
        };
        parts.push(plain);
    }
    let day = time_of_day(frame, e.start.hour());
    push(day);
    for f in &e.faces {
        if let Some(n) = &f.name {
            push(n);
            parts.push(format!("recognised {n}"));
        }
    }
    for p in &e.plates {
        push(p);
        parts.push(format!("plate {p}"));
    }
    for w in camera.split_whitespace() {
        push(w);
    }
    let when = match day {
        "night" => "at night",
        "evening" => "in the evening",
        "morning" => "in the morning",
        _ => "in daylight",
    };
    let what = if parts.is_empty() { "Movement".to_string() } else { parts.join(", ") };
    let mut text = format!("{} {when}", capital(&what));
    if !camera.is_empty() {
        text.push_str(&format!(" · {camera}"));
    }
    Described { words, text }
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// Words people use for what the descriptions say: each maps to the words
/// an alert's description would have. `None`: not something descriptions know.
pub fn known_term(w: &str) -> Option<&'static [&'static str]> {
    Some(match w {
        "person" | "people" | "man" | "men" | "woman" | "women" | "someone" | "somebody" | "guy" | "human" | "child" | "kid" | "boy" | "girl" | "visitor" | "intruder"
        | "burglar" | "pedestrian" => &["person"],
        "car" | "cars" | "truck" | "trucks" | "van" | "vans" | "bus" | "motorbike" | "motorcycle" | "vehicle" | "vehicles" | "ute" | "suv" | "lorry" => &["vehicle"],
        "dog" | "dogs" | "puppy" => &["dog"],
        "cat" | "cats" | "kitten" => &["cat"],
        "bird" | "birds" => &["bird"],
        "bicycle" | "bike" | "cyclist" => &["bicycle"],
        "night" | "dark" | "nighttime" | "midnight" => &["night"],
        "day" | "daytime" | "daylight" | "sunny" => &["daytime", "morning"],
        "morning" => &["morning"],
        "evening" | "dusk" | "sunset" => &["evening"],
        "red" | "orange" | "yellow" | "green" | "blue" | "purple" | "brown" | "black" | "white" | "grey" | "gray" | "silver" | "pink" => match w {
            "red" => &["red"],
            "orange" => &["orange"],
            "yellow" => &["yellow"],
            "green" => &["green"],
            "blue" => &["blue"],
            "purple" | "pink" => &["purple"],
            "brown" => &["brown"],
            "black" => &["black"],
            "white" => &["white"],
            _ => &["grey"],
        },
        _ => return None,
    })
}

/// Words in a question that say nothing about what is in the picture.
const FILLER: [&str; 24] = [
    "a", "an", "the", "of", "in", "at", "on", "with", "and", "photo", "picture", "image", "some", "any", "is", "are", "was", "there", "that", "has", "having", "wearing", "near",
    "by",
];

/// How a question fits an alert's description words: (known terms, how many
/// of them the alert has, the other words that only the picture can judge,
/// the matched words).
pub fn match_terms(query: &str, words: &[String]) -> (usize, usize, usize, Vec<String>) {
    let (mut known, mut hit, mut other) = (0, 0, 0);
    let mut why: Vec<String> = Vec::new();
    for w in query.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty() && !FILLER.contains(w)) {
        match known_term(w) {
            Some(opts) => {
                known += 1;
                if let Some(m) = opts.iter().find(|o| words.iter().any(|x| x == *o)) {
                    hit += 1;
                    if !why.contains(&m.to_string()) {
                        why.push(m.to_string());
                    }
                }
            }
            None => other += 1,
        }
    }
    (known, hit, other, why)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::SeenBox;
    use chrono::{Local, TimeZone};
    use std::sync::Arc;

    fn frame(bg: [u8; 3], patch: Option<([u8; 3], usize, usize, usize, usize)>) -> Frame {
        let (w, h) = (160usize, 90usize);
        let mut rgb = vec![0u8; w * h * 3];
        for p in rgb.chunks_mut(3) {
            p.copy_from_slice(&bg);
        }
        if let Some((c, x0, y0, x1, y1)) = patch {
            for y in y0..y1 {
                for x in x0..x1 {
                    rgb[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&c);
                }
            }
        }
        Frame { width: w as u32, height: h as u32, rgb: Arc::new(rgb) }
    }

    fn event(hour: u32, label: &str, b: [f32; 4]) -> Event {
        let mut e: Event = serde_json::from_value(serde_json::json!({"id": "e", "camera": "c", "start": Local.with_ymd_and_hms(2026, 11, 20, hour, 0, 0).unwrap(), "end": Local.with_ymd_and_hms(2026, 11, 20, hour, 1, 0).unwrap()})).unwrap();
        e.boxes = vec![SeenBox { label: label.into(), score: 0.9, x: b[0], y: b[1], w: b[2], h: b[3] }];
        e
    }

    #[test]
    fn a_red_car_in_daylight_is_described_as_one() {
        let f = frame([120, 140, 120], Some(([200, 30, 30], 40, 30, 120, 80)));
        let d = describe(&event(13, "car", [0.25, 0.3, 0.5, 0.55]), &f, "Driveway");
        for w in ["vehicle", "red", "daytime", "driveway"] {
            assert!(d.words.contains(&w.to_string()), "{w} in {:?}", d.words);
        }
        assert!(d.text.starts_with("A red vehicle in daylight"), "{}", d.text);
        // “red car” fits; “blue car” only half; “red dog at night” brings a word nobody knows.
        assert_eq!(match_terms("a red car", &d.words).0..=match_terms("a red car", &d.words).1, 2..=2);
        let (k, h, _, _) = match_terms("blue car", &d.words);
        assert_eq!((k, h), (2, 1));
    }

    #[test]
    fn a_dark_picture_is_night_whatever_the_clock_says() {
        let f = frame([20, 22, 25], None);
        let d = describe(&event(13, "dog", [0.3, 0.3, 0.2, 0.3]), &f, "Yard");
        assert!(d.words.contains(&"night".to_string()) && d.words.contains(&"dog".to_string()));
        assert!(d.text.contains("at night"));
        let (k, h, other, why) = match_terms("dog at night carrying a box", &d.words);
        assert_eq!((k, h), (2, 2));
        assert_eq!(other, 2, "carrying and box: only the picture can tell");
        assert_eq!(why, ["dog", "night"]);
    }

    #[test]
    fn recognised_people_and_plates_are_searchable_words() {
        let f = frame([150, 150, 150], None);
        let mut e = event(10, "person", [0.3, 0.2, 0.2, 0.6]);
        e.faces = vec![crate::events::FaceSeen { file: "f.jpg".into(), print: String::new(), name: Some("Mum".into()), confirmed: true }];
        e.plates = vec!["ABC123".into()];
        let d = describe(&e, &f, "Front Door");
        assert!(["mum", "abc123", "front", "door", "person"].iter().all(|w| d.words.contains(&w.to_string())), "{:?}", d.words);
        assert!(d.text.contains("recognised Mum") && d.text.contains("plate ABC123"));
    }
}
