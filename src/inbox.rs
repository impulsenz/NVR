//! Settings → People & vehicles → "New faces & plates": faces nobody has
//! been named for and number plates of no known vehicle, from recent
//! alerts. The same person seen in many alerts is one entry; assigning it
//! names every alert they're in and teaches GAZE their face.

use crate::events::Event;
use crate::known::{cosine, unpack};
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Faceprints this similar are taken to be the same person.
const SAME: f32 = 0.5;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FaceRef {
    pub camera: String,
    pub event: String,
    pub face: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct FaceGroup {
    /// Picture of the face (media URL).
    pub picture: String,
    pub count: usize,
    pub last: DateTime<Local>,
    pub members: Vec<FaceRef>,
    #[serde(skip)]
    print: Vec<f32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlateGroup {
    pub plate: String,
    pub count: usize,
    pub last: DateTime<Local>,
    /// The newest alert with it (picture).
    pub camera: String,
    pub event: String,
    pub picture: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Dismissed {
    #[serde(default)]
    pub faces: Vec<String>,
    #[serde(default)]
    pub plates: Vec<String>,
}

fn file(data_dir: &Path) -> PathBuf {
    data_dir.join("known-dismissed.json")
}

pub fn dismissed(data_dir: &Path) -> Dismissed {
    std::fs::read(file(data_dir))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save_dismissed(data_dir: &Path, d: &Dismissed) -> anyhow::Result<()> {
    std::fs::write(file(data_dir), serde_json::to_vec_pretty(d)?)?;
    Ok(())
}

/// Group the unnamed faces of these alerts (newest first) by person.
pub fn group_faces(events: &[Event], dismissed: &Dismissed) -> Vec<FaceGroup> {
    let gone: Vec<Vec<f32>> = dismissed.faces.iter().map(|p| unpack(p)).collect();
    let mut evs: Vec<&Event> = events.iter().collect();
    evs.sort_by(|a, b| b.start.cmp(&a.start));
    let mut groups: Vec<FaceGroup> = Vec::new();
    for e in evs {
        for (i, f) in e.faces.iter().enumerate() {
            if f.name.is_some() {
                continue;
            }
            let p = unpack(&f.print);
            if gone.iter().any(|g| cosine(g, &p) >= SAME) {
                continue;
            }
            let r = FaceRef {
                camera: e.camera.clone(),
                event: e.id.clone(),
                face: i,
            };
            match groups.iter_mut().find(|g| cosine(&g.print, &p) >= SAME) {
                Some(g) => {
                    g.count += 1;
                    if g.members.len() < 200 {
                        g.members.push(r);
                    }
                }
                None => groups.push(FaceGroup {
                    picture: format!("media/{}/events/{}", e.camera, f.file),
                    count: 1,
                    last: e.start,
                    members: vec![r],
                    print: p,
                }),
            }
        }
    }
    groups.truncate(60);
    groups
}

/// Number plates read in these alerts that belong to no known vehicle.
pub fn group_plates(
    events: &[Event],
    dismissed: &Dismissed,
    known: impl Fn(&str) -> bool,
) -> Vec<PlateGroup> {
    let mut evs: Vec<&Event> = events.iter().collect();
    evs.sort_by(|a, b| b.start.cmp(&a.start));
    let mut groups: Vec<PlateGroup> = Vec::new();
    for e in evs {
        for p in &e.plates {
            if known(p)
                || dismissed
                    .plates
                    .iter()
                    .any(|d| crate::recog::plate_matches(d, p))
            {
                continue;
            }
            match groups
                .iter_mut()
                .find(|g| crate::recog::plate_matches(&g.plate, p))
            {
                Some(g) => g.count += 1,
                None => groups.push(PlateGroup {
                    plate: p.clone(),
                    count: 1,
                    last: e.start,
                    camera: e.camera.clone(),
                    event: e.id.clone(),
                    picture: e
                        .thumb
                        .as_ref()
                        .map(|t| format!("media/{}/events/{t}", e.camera)),
                }),
            }
        }
    }
    groups.truncate(60);
    groups
}

/// The faceprint of a face in an alert (for dismissing).
pub fn print_of(e: &Event, face: usize) -> Option<String> {
    e.faces.get(face).map(|f| f.print.clone())
}

/// Mark a whole group as this person: returns the prints to learn (a few,
/// spread over the group).
pub fn learn_set(prints: &[Vec<f32>]) -> Vec<Vec<f32>> {
    match prints.len() {
        0 => vec![],
        1..=3 => prints.to_vec(),
        n => vec![
            prints[0].clone(),
            prints[n / 2].clone(),
            prints[n - 1].clone(),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::FaceSeen;
    use crate::known::pack;

    fn ev(id: &str, mins: i64, faces: Vec<(&[f32], Option<&str>)>, plates: &[&str]) -> Event {
        let mut e: Event = serde_json::from_value(serde_json::json!({
            "id": id, "camera": "front", "start": Local::now() - chrono::Duration::minutes(mins),
            "end": Local::now(), "labels": [], "motion": true
        }))
        .unwrap();
        e.faces = faces
            .into_iter()
            .enumerate()
            .map(|(i, (p, n))| FaceSeen {
                file: format!("{id}-{i}.jpg"),
                print: pack(p),
                name: n.map(String::from),
                confirmed: false,
            })
            .collect();
        e.plates = plates.iter().map(|s| s.to_string()).collect();
        e
    }

    #[test]
    fn groups_people_and_plates() {
        let a: Vec<f32> =
            crate::recog::normalize((0..128).map(|i| (i as f32 * 0.37).sin()).collect());
        let a2: Vec<f32> = crate::recog::normalize(
            a.iter()
                .enumerate()
                .map(|(i, x)| x + 0.02 * (i as f32).cos())
                .collect(),
        );
        let b: Vec<f32> =
            crate::recog::normalize((0..128).map(|i| (i as f32 * 1.91).cos()).collect());
        let events = vec![
            ev("e1", 30, vec![(&a, None), (&b, None)], &["ABC123"]),
            ev("e2", 10, vec![(&a2, None)], &["A8C123", "XYZ9"]),
            ev("e3", 5, vec![(&b, Some("Mum"))], &["MUM1"]),
        ];
        let d = Dismissed::default();
        let g = group_faces(&events, &d);
        assert_eq!(g.len(), 2, "two unknown people");
        assert_eq!(g[0].count, 2, "the same person in two alerts");
        assert_eq!(g[0].members[0].event, "e2", "newest first");
        let p = group_plates(&events, &d, |p| p == "MUM1");
        assert_eq!(
            p.iter()
                .map(|x| (x.plate.as_str(), x.count))
                .collect::<Vec<_>>(),
            [("A8C123", 2), ("XYZ9", 1)]
        );
        let d = Dismissed {
            faces: vec![pack(&b)],
            plates: vec!["XYZ9".into()],
        };
        assert_eq!(group_faces(&events, &d).len(), 1);
        assert_eq!(group_plates(&events, &d, |_| false).len(), 2);
    }
}
