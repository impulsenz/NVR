//! Known people and vehicles: what GAZE has been taught to recognise.
//!
//! A **person** has faceprints (from photos you upload, or faces you name
//! in alerts). A **vehicle** has number plates and photos. Kept in
//! `nvr-data/known.json`, photos in `nvr-data/known/<id>/`.

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Known {
    pub id: String,
    /// "person" or "vehicle".
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub plates: Vec<String>,
    /// Faceprints, packed.
    #[serde(default)]
    pub faces: Vec<String>,
    /// Faces that were wrongly taken for this person (corrected on an
    /// alert): a face closer to one of these isn't them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub not_faces: Vec<String>,
    /// Photo file names.
    #[serde(default)]
    pub photos: Vec<String>,
}

/// Shown to the browser (without the vectors).
#[derive(Debug, Clone, Serialize)]
pub struct KnownInfo {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub plates: Vec<String>,
    pub faces: usize,
    pub photos: Vec<String>,
}

pub struct KnownStore {
    file: PathBuf,
    pub dir: PathBuf,
    items: RwLock<Vec<Known>>,
}

/// Unit vector ↔ compact text (one signed byte per number).
pub fn pack(v: &[f32]) -> String {
    use base64::Engine;
    let b: Vec<u8> = v
        .iter()
        .map(|x| ((x * 127.0).round().clamp(-127.0, 127.0) as i8) as u8)
        .collect();
    base64::engine::general_purpose::STANDARD.encode(b)
}

pub fn unpack(s: &str) -> Vec<f32> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map(|b| b.into_iter().map(|x| x as i8 as f32 / 127.0).collect())
        .unwrap_or_default()
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut d, mut na, mut nb) = (0f32, 0f32, 0f32);
    for (x, y) in a.iter().zip(b) {
        d += x * y;
        na += x * x;
        nb += y * y;
    }
    d / (na.sqrt() * nb.sqrt()).max(1e-6)
}

impl KnownStore {
    pub fn open(data_dir: &Path) -> Self {
        let file = data_dir.join("known.json");
        let items = std::fs::read(&file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            file,
            dir: data_dir.join("known"),
            items: RwLock::new(items),
        }
    }

    fn save(&self) -> Result<()> {
        let b = serde_json::to_vec_pretty(&*self.items.read().unwrap())?;
        let tmp = self.file.with_extension("json.tmp");
        std::fs::write(&tmp, b)?;
        std::fs::rename(&tmp, &self.file)?;
        Ok(())
    }

    /// For a backup: the list (with faceprints) and every photo, as
    /// (path inside the backup, bytes).
    pub fn export(&self) -> (Vec<u8>, Vec<(String, Vec<u8>)>) {
        let items = self.items.read().unwrap().clone();
        let mut photos = Vec::new();
        for k in &items {
            for p in &k.photos {
                if let Ok(b) = std::fs::read(self.dir.join(&k.id).join(p)) {
                    photos.push((format!("known/{}/{p}", k.id), b));
                }
            }
        }
        (serde_json::to_vec_pretty(&items).unwrap_or_default(), photos)
    }

    /// From a backup: people and vehicles in it replace those with the same
    /// id here (others here are kept); `photo(id, name)` gives a photo's
    /// bytes. Returns how many were brought in.
    pub fn import(&self, json: &[u8], photo: impl Fn(&str, &str) -> Option<Vec<u8>>) -> Result<usize> {
        let incoming: Vec<Known> = serde_json::from_slice(json)?;
        let ok = |s: &str| !s.is_empty() && s.len() <= 80 && !s.contains(['/', '\\', ':']) && !s.starts_with('.');
        let mut n = 0;
        {
            let mut items = self.items.write().unwrap();
            for mut k in incoming {
                if !ok(&k.id) {
                    continue;
                }
                let dir = self.dir.join(&k.id);
                k.photos.retain(|p| {
                    ok(p) && photo(&k.id, p).is_some_and(|b| std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join(p), b)).is_ok())
                });
                items.retain(|x| x.id != k.id);
                items.push(k);
                n += 1;
            }
        }
        self.save()?;
        Ok(n)
    }

    pub fn list(&self) -> Vec<KnownInfo> {
        self.items
            .read()
            .unwrap()
            .iter()
            .map(|k| KnownInfo {
                id: k.id.clone(),
                kind: k.kind.clone(),
                name: k.name.clone(),
                plates: k.plates.clone(),
                faces: k.faces.len(),
                photos: k.photos.clone(),
            })
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<Known> {
        self.items
            .read()
            .unwrap()
            .iter()
            .find(|k| k.id == id)
            .cloned()
    }

    pub fn create(&self, kind: &str, name: &str, plates: Vec<String>) -> Result<String> {
        let name = name.trim();
        if name.is_empty() {
            bail!("enter a name");
        }
        if kind != "person" && kind != "vehicle" {
            bail!("unknown kind");
        }
        let base: String = name
            .to_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let mut id = base.trim_matches('-').to_string();
        if id.is_empty() {
            id = "known".into();
        }
        {
            let mut items = self.items.write().unwrap();
            let mut n = 2;
            let root = id.clone();
            while items.iter().any(|k| k.id == id) {
                id = format!("{root}-{n}");
                n += 1;
            }
            items.push(Known {
                id: id.clone(),
                kind: kind.into(),
                name: name.into(),
                plates: clean_plates(plates),
                ..Default::default()
            });
        }
        self.save()?;
        Ok(id)
    }

    pub fn update(&self, id: &str, name: Option<&str>, plates: Option<Vec<String>>) -> Result<()> {
        {
            let mut items = self.items.write().unwrap();
            let k = items
                .iter_mut()
                .find(|k| k.id == id)
                .ok_or_else(|| anyhow!("not found"))?;
            if let Some(n) = name.map(str::trim).filter(|n| !n.is_empty()) {
                k.name = n.into();
            }
            if let Some(p) = plates {
                k.plates = clean_plates(p);
            }
        }
        self.save()
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        self.items.write().unwrap().retain(|k| k.id != id);
        let _ = std::fs::remove_dir_all(self.dir.join(id));
        self.save()
    }

    /// Teach: add a faceprint / plate / photo.
    pub fn add(
        &self,
        id: &str,
        face: Option<&[f32]>,
        plate: Option<&str>,
        photo: Option<&[u8]>,
    ) -> Result<()> {
        {
            let mut items = self.items.write().unwrap();
            let k = items
                .iter_mut()
                .find(|k| k.id == id)
                .ok_or_else(|| anyhow!("not found"))?;
            if let Some(f) = face {
                k.faces.push(pack(f));
            }
            if let Some(p) = plate {
                let p = clean_plates(vec![p.into()]);
                for x in p {
                    if !k.plates.contains(&x) {
                        k.plates.push(x);
                    }
                }
            }
            if let Some(bytes) = photo {
                let dir = self.dir.join(&k.id);
                std::fs::create_dir_all(&dir)?;
                let name = format!(
                    "{}.jpg",
                    k.photos.len()
                        + 1
                        + k.photos
                            .iter()
                            .filter_map(|p| p.trim_end_matches(".jpg").parse::<usize>().ok())
                            .max()
                            .unwrap_or(0)
                );
                std::fs::write(dir.join(&name), bytes)?;
                k.photos.push(name);
            }
        }
        self.save()
    }

    /// "That's not them": the person called `name` was wrongly matched to
    /// this face. Faceprints of theirs learned from it (or nearly the same)
    /// are dropped, and it's remembered as not them.
    pub fn not_this(&self, name: &str, print: &[f32]) -> Result<()> {
        {
            let mut items = self.items.write().unwrap();
            let Some(k) = items
                .iter_mut()
                .find(|k| k.kind == "person" && k.name == name)
            else {
                return Ok(());
            };
            k.faces.retain(|f| cosine(&unpack(f), print) < 0.9);
            k.not_faces.push(pack(print));
            if k.not_faces.len() > 100 {
                k.not_faces.remove(0);
            }
        }
        self.save()
    }

    /// The known person this faceprint belongs to (name, similarity).
    pub fn match_face(&self, print: &[f32]) -> Option<(String, f32)> {
        self.items
            .read()
            .unwrap()
            .iter()
            .filter(|k| k.kind == "person")
            .filter_map(|k| {
                let best = k
                    .faces
                    .iter()
                    .map(|f| cosine(&unpack(f), print))
                    .fold(f32::MIN, f32::max);
                // Corrected before: this looks more like a face that was
                // wrongly taken for them than like them.
                let wrong = k
                    .not_faces
                    .iter()
                    .map(|f| cosine(&unpack(f), print))
                    .fold(f32::MIN, f32::max);
                (best >= crate::recog::FACE_MATCH && wrong < best).then(|| (k.name.clone(), best))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// The known vehicle with this plate.
    pub fn match_plate(&self, plate: &str) -> Option<String> {
        self.items
            .read()
            .unwrap()
            .iter()
            .filter(|k| k.kind == "vehicle")
            .find(|k| {
                k.plates
                    .iter()
                    .any(|p| crate::recog::plate_matches(p, plate))
            })
            .map(|k| k.name.clone())
    }

    pub fn photo_path(&self, id: &str, name: &str) -> Option<PathBuf> {
        let k = self.get(id)?;
        k.photos
            .iter()
            .any(|p| p == name)
            .then(|| self.dir.join(id).join(name))
    }
}

fn clean_plates(p: Vec<String>) -> Vec<String> {
    p.into_iter()
        .map(|s| {
            s.chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .map(|c| c.to_ascii_uppercase())
                .collect::<String>()
        })
        .filter(|s| s.len() >= 2)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrected_faces_stop_matching() {
        let dir = std::env::temp_dir().join(format!("nvr-known-fix-{}", std::process::id()));
        let s = KnownStore::open(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mum = s.create("person", "Mum", vec![]).unwrap();
        let mum_face: Vec<f32> =
            crate::recog::normalize((0..128).map(|i| (i as f32 * 0.37).sin()).collect());
        s.add(&mum, Some(&mum_face), None, None).unwrap();
        // A visitor who looks a bit like Mum: taken for her.
        let visitor: Vec<f32> = crate::recog::normalize(
            (0..128)
                .map(|i| (i as f32 * 0.37).sin() + 0.6 * (i as f32 * 2.3).cos())
                .collect(),
        );
        assert_eq!(s.match_face(&visitor).map(|x| x.0).as_deref(), Some("Mum"));
        // "Not Mum": no longer taken for her; Mum herself still is.
        s.not_this("Mum", &visitor).unwrap();
        assert!(s.match_face(&visitor).is_none());
        assert_eq!(s.match_face(&mum_face).map(|x| x.0).as_deref(), Some("Mum"));
        // Named as a new person: matched to them.
        let gran = s.create("person", "Gran", vec![]).unwrap();
        s.add(&gran, Some(&visitor), None, None).unwrap();
        assert_eq!(s.match_face(&visitor).map(|x| x.0).as_deref(), Some("Gran"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn people_and_vehicles() {
        let dir = std::env::temp_dir().join(format!("nvr-known-{}", std::process::id()));
        let s = KnownStore::open(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mum = s.create("person", "Mum", vec![]).unwrap();
        let car = s
            .create("vehicle", "Dad's car", vec!["abc 123".into()])
            .unwrap();
        let face: Vec<f32> =
            crate::recog::normalize((0..128).map(|i| (i as f32 * 0.37).sin()).collect());
        s.add(&mum, Some(&face), None, Some(b"jpg")).unwrap();
        assert_eq!(s.match_face(&face).unwrap().0, "Mum");
        let other: Vec<f32> =
            crate::recog::normalize((0..128).map(|i| (i as f32 * 1.91).cos()).collect());
        assert!(s.match_face(&other).is_none());
        assert_eq!(s.match_plate("ABC123").as_deref(), Some("Dad's car"));
        assert_eq!(s.match_plate("A8C123").as_deref(), Some("Dad's car"));
        let s2 = KnownStore::open(&dir);
        assert_eq!(s2.list().len(), 2);
        s2.delete(&car).unwrap();
        assert!(s2.match_plate("ABC123").is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
