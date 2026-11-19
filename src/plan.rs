//! The floor plan (Plan page): a picture of your property or building (a
//! floor plan, a drawing, an aerial or satellite image you supply), with the
//! cameras placed on it, how each one looks at the ground (so a person seen
//! by a camera lands at the right place on the plan), which cameras are
//! neighbours, and the settings that tune how walks are followed.
//!
//! Kept in `nvr-data/plan.json`; the picture in `nvr-data/plan.jpg`. GAZE never
//! fetches map imagery: you bring the picture (and are bound by the terms of
//! wherever it came from). Uploaded pictures are re-encoded to JPEG by the
//! page, which also drops any location data hidden in them.
//!
//! Nothing here touches recording, detection or alerts.

use crate::geo::{Homography, Pt};
use crate::sync::RwLockExt;
use anyhow::{Result, bail};
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// How walks are followed and joined across cameras (Plan → Tuning). Every
/// default is a middle-of-the-road guess for a house and garden; the page
/// explains each one.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Tuning {
    /// Follow people (off by default: it stores where people walked).
    pub enabled: bool,
    /// What to follow (the AI's names: "person", "dog", "vehicle" …).
    pub labels: Vec<String>,
    /// Only follow detections at least this sure (0..1).
    pub min_score: f32,
    /// How far (as a fraction of the picture's width) someone may move between
    /// two looks and still be the same person.
    pub match_radius: f32,
    /// A walk on one camera ends when its person isn't seen for this long.
    pub track_gap_seconds: f32,
    /// Walks shorter than this are ignored (a shadow, a flicker).
    pub min_track_seconds: f32,
    /// 0 = follow the raw positions, 1 = a very smooth path.
    pub smoothing: f32,
    /// The longest gap between leaving one camera and appearing on the next.
    pub max_gap_seconds: f32,
    /// Cameras whose views overlap: the next may start this long *before* the
    /// last one lost them.
    pub overlap_seconds: f32,
    /// Fastest believable speed between two cameras (m/s: 1.4 walking, 3 jogging).
    pub max_speed_mps: f32,
    /// Extra distance allowed on top of speed × time (calibration is never perfect).
    pub slack_m: f32,
    /// A join must score at least this (0..1) to count as the same person.
    pub min_link_score: f32,
    /// How much each clue counts when joining: timing, distance, clothing colour.
    pub time_weight: f32,
    pub distance_weight: f32,
    pub colour_weight: f32,
    /// Allow joins between any two cameras, not only the neighbours drawn on the plan.
    pub link_all_pairs: bool,
    /// How long walks are kept.
    pub keep_days: u32,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            enabled: false,
            labels: vec!["person".into()],
            min_score: 0.5,
            match_radius: 0.2,
            track_gap_seconds: 4.0,
            min_track_seconds: 1.0,
            smoothing: 0.4,
            max_gap_seconds: 30.0,
            overlap_seconds: 3.0,
            max_speed_mps: 3.0,
            slack_m: 3.0,
            min_link_score: 0.5,
            time_weight: 1.0,
            distance_weight: 1.0,
            colour_weight: 0.5,
            link_all_pairs: false,
            keep_days: 14,
        }
    }
}

impl Tuning {
    pub fn validate(&mut self) -> Result<()> {
        let range = |v: f32, lo: f32, hi: f32, what: &str| -> Result<()> {
            if !v.is_finite() || v < lo || v > hi {
                bail!("{what}: between {lo} and {hi}");
            }
            Ok(())
        };
        range(self.min_score, 0.05, 1.0, "minimum confidence")?;
        range(self.match_radius, 0.02, 1.0, "match radius")?;
        range(self.track_gap_seconds, 0.5, 60.0, "walk gap")?;
        range(self.min_track_seconds, 0.0, 60.0, "shortest walk")?;
        range(self.smoothing, 0.0, 1.0, "smoothing")?;
        range(self.max_gap_seconds, 1.0, 600.0, "longest gap between cameras")?;
        range(self.overlap_seconds, 0.0, 60.0, "overlap")?;
        range(self.max_speed_mps, 0.3, 15.0, "fastest speed")?;
        range(self.slack_m, 0.0, 100.0, "extra distance")?;
        range(self.min_link_score, 0.0, 1.0, "minimum join score")?;
        for (w, n) in [(self.time_weight, "time weight"), (self.distance_weight, "distance weight"), (self.colour_weight, "colour weight")] {
            range(w, 0.0, 10.0, n)?;
        }
        if self.time_weight + self.distance_weight + self.colour_weight <= 0.0 {
            bail!("at least one of timing, distance and colour must count");
        }
        if !(1..=365).contains(&self.keep_days) {
            bail!("keep walks for 1 to 365 days");
        }
        self.labels.retain(|l| !l.trim().is_empty());
        self.labels.iter_mut().for_each(|l| *l = l.trim().to_lowercase());
        self.labels.dedup();
        if self.labels.is_empty() || self.labels.len() > 10 {
            bail!("follow 1 to 10 kinds of object");
        }
        Ok(())
    }
}

/// Four spots on the camera's picture and the same four spots on the plan.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Calib {
    pub picture: [[f64; 2]; 4],
    pub plan: [[f64; 2]; 4],
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlanCamera {
    pub id: String,
    /// Where it hangs (fractions of the plan).
    pub x: f64,
    pub y: f64,
    /// Which way it faces, in degrees clockwise from the top of the plan.
    #[serde(default)]
    pub rotation_deg: f64,
    /// How wide it sees (degrees).
    #[serde(default = "default_fov")]
    pub fov_deg: f64,
    /// How far it sees well (metres, for the drawn cone).
    #[serde(default = "default_range")]
    pub range_m: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calib: Option<Calib>,
    /// How big its icon is drawn, as a multiple of the plan's icon size (1 = as the plan).
    #[serde(default = "one")]
    pub size: f64,
}

fn one() -> f64 {
    1.0
}

fn default_fov() -> f64 {
    90.0
}
fn default_range() -> f64 {
    15.0
}

/// A wall drawn on the plan (fractions of the plan): a camera's drawn view
/// stops at it, because a camera can't see through a wall.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Wall {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
}

/// Most walls a plan may hold.
pub const MAX_WALLS: usize = 1500;

/// Two cameras whose views meet: someone leaving one may appear on the other.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Link {
    pub a: String,
    pub b: String,
    /// How long the walk between them usually takes, if you know.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub travel_seconds: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Plan {
    /// The picture is there (`plan.jpg`).
    pub has_image: bool,
    pub image_w: u32,
    pub image_h: u32,
    /// Pixels of the picture per metre; 0 = not set.
    pub px_per_m: f64,
    pub cameras: Vec<PlanCamera>,
    pub links: Vec<Link>,
    /// Walls: what a camera's drawn view can't pass.
    pub walls: Vec<Wall>,
    /// How big camera icons are drawn on the map (1 = normal; 0 in an old file = normal).
    pub icon_scale: f64,
    pub tuning: Tuning,
    pub updated: Option<DateTime<Local>>,
}

impl Plan {
    pub fn camera(&self, id: &str) -> Option<&PlanCamera> {
        self.cameras.iter().find(|c| c.id == id)
    }

    /// The mapping from a camera's picture to the plan, if it's calibrated.
    pub fn homography_for(&self, id: &str) -> Option<Homography> {
        let c = self.camera(id)?.calib.as_ref()?;
        let p = |a: [f64; 2]| -> Pt { (a[0], a[1]) };
        crate::geo::homography(c.picture.map(p), c.plan.map(p))
    }

    pub fn linked(&self, a: &str, b: &str) -> Option<&Link> {
        self.links.iter().find(|l| (l.a == a && l.b == b) || (l.a == b && l.b == a))
    }

    pub fn metres(&self, a: Pt, b: Pt) -> Option<f64> {
        (self.has_image && self.px_per_m > 0.0).then(|| crate::geo::metres(a, b, self.image_w as f64, self.image_h as f64, self.px_per_m))
    }

    fn validate(&mut self) -> Result<()> {
        let unit = |v: f64| v.is_finite() && (0.0..=1.0).contains(&v);
        if !self.px_per_m.is_finite() || self.px_per_m < 0.0 || self.px_per_m > 100_000.0 {
            bail!("scale: pixels per metre");
        }
        if self.cameras.len() > 64 {
            bail!("at most 64 cameras on a plan");
        }
        let mut seen = std::collections::HashSet::new();
        for c in &mut self.cameras {
            if !seen.insert(c.id.clone()) || c.id.is_empty() {
                bail!("each camera can be on the plan once");
            }
            if !unit(c.x) || !unit(c.y) {
                bail!("{}: place it on the plan", c.id);
            }
            if !c.rotation_deg.is_finite() {
                bail!("{}: direction", c.id);
            }
            c.rotation_deg = c.rotation_deg.rem_euclid(360.0);
            if !c.fov_deg.is_finite() || !(5.0..=360.0).contains(&c.fov_deg) {
                bail!("{}: field of view between 5 and 360 degrees", c.id);
            }
            if !c.range_m.is_finite() || !(1.0..=500.0).contains(&c.range_m) {
                bail!("{}: range between 1 and 500 metres", c.id);
            }
            if !c.size.is_finite() || !(0.2..=8.0).contains(&c.size) {
                bail!("{}: icon size between 20% and 800%", c.id);
            }
            if let Some(k) = &c.calib {
                if !k.picture.iter().chain(k.plan.iter()).all(|p| unit(p[0]) && unit(p[1])) {
                    bail!("{}: calibration points must be inside the picture and the plan", c.id);
                }
                let p = |a: [f64; 2]| -> Pt { (a[0], a[1]) };
                if crate::geo::homography(k.picture.map(p), k.plan.map(p)).is_none() {
                    bail!("{}: the four calibration spots can't be in a line, or two the same", c.id);
                }
            }
        }
        if self.links.len() > 256 {
            bail!("too many links");
        }
        if self.walls.len() > MAX_WALLS {
            bail!("at most {MAX_WALLS} walls");
        }
        if self.icon_scale == 0.0 {
            self.icon_scale = 1.0;
        }
        if !self.icon_scale.is_finite() || !(0.2..=8.0).contains(&self.icon_scale) {
            bail!("camera icon size between 20% and 800%");
        }
        for w in &self.walls {
            if ![w.x1, w.y1, w.x2, w.y2].iter().all(|v| unit(*v)) {
                bail!("walls must be drawn inside the plan");
            }
            if (w.x1 - w.x2).abs() < 1e-6 && (w.y1 - w.y2).abs() < 1e-6 {
                bail!("a wall needs two different ends");
            }
        }
        for l in &mut self.links {
            if l.a == l.b || !seen.contains(&l.a) || !seen.contains(&l.b) {
                bail!("a link joins two different cameras that are on the plan");
            }
            if l.travel_seconds.is_some_and(|t| !t.is_finite() || !(0.0..=3600.0).contains(&t)) {
                bail!("travel time: 0 to 3600 seconds");
            }
        }
        self.tuning.validate()
    }
}

pub struct PlanStore {
    file: PathBuf,
    image: PathBuf,
    plan: RwLock<Plan>,
}

/// Largest picture accepted (the page shrinks and re-encodes before upload).
pub const MAX_IMAGE_BYTES: usize = 12 * 1024 * 1024;

impl PlanStore {
    pub fn open(data_dir: &Path) -> Self {
        let file = data_dir.join("plan.json");
        let mut plan: Plan = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let image = data_dir.join("plan.jpg");
        plan.has_image = image.exists() && plan.image_w > 0;
        Self { file, image, plan: RwLock::new(plan) }
    }

    pub fn get(&self) -> Plan {
        self.plan.reader().clone()
    }

    pub fn tuning(&self) -> Tuning {
        self.plan.reader().tuning.clone()
    }

    pub fn image_path(&self) -> Option<PathBuf> {
        self.plan.reader().has_image.then(|| self.image.clone())
    }

    fn save(&self, p: &Plan) -> Result<()> {
        let b = serde_json::to_vec_pretty(p)?;
        let tmp = self.file.with_extension("json.tmp");
        std::fs::write(&tmp, b)?;
        std::fs::rename(&tmp, &self.file)?;
        Ok(())
    }

    /// Replace the plan's contents (not its picture). Invalid → nothing changes.
    pub fn update(&self, mut new: Plan) -> Result<Plan> {
        new.validate()?;
        let mut cur = self.plan.writer();
        new.has_image = cur.has_image;
        new.image_w = cur.image_w;
        new.image_h = cur.image_h;
        new.updated = Some(Local::now());
        self.save(&new)?;
        *cur = new.clone();
        Ok(new)
    }

    /// Store the plan's picture (a JPEG). Its size comes from the file itself.
    pub fn set_image(&self, jpeg: &[u8]) -> Result<Plan> {
        if jpeg.len() > MAX_IMAGE_BYTES {
            bail!("the picture is too big (more than {} MB)", MAX_IMAGE_BYTES / 1024 / 1024);
        }
        if jpeg.len() < 4 || jpeg[..2] != [0xFF, 0xD8] {
            bail!("the picture must be a JPEG (the page converts other formats for you)");
        }
        let mut d = jpeg_decoder::Decoder::new(jpeg);
        d.read_info().map_err(|e| anyhow::anyhow!("not a readable picture: {e}"))?;
        let info = d.info().ok_or_else(|| anyhow::anyhow!("not a readable picture"))?;
        let (w, h) = (info.width as u32, info.height as u32);
        if w < 64 || h < 64 || w > 8000 || h > 8000 {
            bail!("the picture must be between 64 and 8000 pixels on each side");
        }
        let tmp = self.image.with_extension("jpg.tmp");
        std::fs::write(&tmp, jpeg)?;
        std::fs::rename(&tmp, &self.image)?;
        let mut cur = self.plan.writer();
        // A different picture has a different shape and scale.
        let reshaped = cur.image_w != w || cur.image_h != h;
        cur.has_image = true;
        cur.image_w = w;
        cur.image_h = h;
        if reshaped {
            cur.px_per_m = 0.0;
            // Calibrations and walls are tied to what the picture shows.
            cur.walls.clear();
            for c in &mut cur.cameras {
                c.calib = None;
            }
        }
        cur.updated = Some(Local::now());
        let p = cur.clone();
        self.save(&p)?;
        Ok(p)
    }

    pub fn remove_image(&self) -> Result<Plan> {
        let _ = std::fs::remove_file(&self.image);
        let mut cur = self.plan.writer();
        cur.has_image = false;
        cur.image_w = 0;
        cur.image_h = 0;
        cur.px_per_m = 0.0;
        cur.walls.clear();
        for c in &mut cur.cameras {
            c.calib = None;
        }
        cur.updated = Some(Local::now());
        let p = cur.clone();
        self.save(&p)?;
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam(id: &str) -> PlanCamera {
        PlanCamera { id: id.into(), x: 0.5, y: 0.5, rotation_deg: 0.0, fov_deg: 90.0, range_m: 15.0, calib: None, size: 1.0 }
    }

    fn jpeg(w: u32, h: u32) -> Vec<u8> {
        crate::imgutil::Frame { width: w, height: h, rgb: std::sync::Arc::new(vec![128; (w * h * 3) as usize]) }.jpeg(80).unwrap()
    }

    #[test]
    fn defaults_are_valid_and_a_bad_setting_is_refused_with_its_name() {
        let mut t = Tuning::default();
        t.validate().unwrap();
        assert!(!t.enabled, "following people is off until asked for");
        t.max_speed_mps = 100.0;
        assert!(t.validate().unwrap_err().to_string().contains("fastest speed"));
        let mut t = Tuning { time_weight: 0.0, distance_weight: 0.0, colour_weight: 0.0, ..Default::default() };
        assert!(t.validate().is_err());
        let mut t = Tuning { labels: vec![" Person ".into(), "person".into(), "".into()], ..Default::default() };
        t.validate().unwrap();
        assert_eq!(t.labels, vec!["person"]);
        assert!(Tuning { labels: vec![], ..Default::default() }.validate().is_err());
        assert!(Tuning { keep_days: 0, ..Default::default() }.validate().is_err());
        // Old and partial files still load: missing settings take their defaults.
        let p: Plan = serde_json::from_str(r#"{"cameras":[{"id":"a","x":0.1,"y":0.2}]}"#).unwrap();
        assert_eq!(p.tuning, Tuning::default());
        assert_eq!((p.cameras[0].fov_deg, p.cameras[0].range_m), (90.0, 15.0));
    }

    #[test]
    fn the_plan_is_checked_before_it_is_kept() {
        let dir = std::env::temp_dir().join(format!("nvr-plan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let s = PlanStore::open(&dir);
        let ok = Plan { cameras: vec![cam("a"), PlanCamera { x: 0.2, ..cam("b") }], links: vec![Link { a: "a".into(), b: "b".into(), travel_seconds: Some(8.0) }], ..Default::default() };
        s.update(ok.clone()).unwrap();
        for (what, bad) in [
            ("same camera twice", Plan { cameras: vec![cam("a"), cam("a")], ..Default::default() }),
            ("off the plan", Plan { cameras: vec![PlanCamera { x: 1.5, ..cam("a") }], ..Default::default() }),
            ("link to a camera not on the plan", Plan { cameras: vec![cam("a")], links: vec![Link { a: "a".into(), b: "z".into(), travel_seconds: None }], ..Default::default() }),
            ("link to itself", Plan { cameras: vec![cam("a")], links: vec![Link { a: "a".into(), b: "a".into(), travel_seconds: None }], ..Default::default() }),
            ("absurd field of view", Plan { cameras: vec![PlanCamera { fov_deg: 1.0, ..cam("a") }], ..Default::default() }),
            ("nan", Plan { cameras: vec![PlanCamera { x: f64::NAN, ..cam("a") }], ..Default::default() }),
        ] {
            assert!(s.update(bad).is_err(), "{what} should be refused");
        }
        // Refused changes leave the saved plan alone, and it survives a restart.
        assert_eq!(s.get().cameras.len(), 2);
        assert_eq!(PlanStore::open(&dir).get().links.len(), 1);
        // Four calibration spots in a line fix nothing.
        let line = Calib { picture: [[0.1, 0.5], [0.3, 0.5], [0.6, 0.5], [0.9, 0.5]], plan: [[0.1, 0.1], [0.2, 0.1], [0.3, 0.1], [0.4, 0.1]] };
        assert!(s.update(Plan { cameras: vec![PlanCamera { calib: Some(line), ..cam("a") }], ..Default::default() }).is_err());
        let good = Calib { picture: [[0.1, 0.9], [0.9, 0.9], [0.7, 0.4], [0.3, 0.4]], plan: [[0.2, 0.8], [0.6, 0.8], [0.6, 0.4], [0.2, 0.4]] };
        let p = s.update(Plan { cameras: vec![PlanCamera { calib: Some(good), ..cam("a") }], ..Default::default() }).unwrap();
        let h = p.homography_for("a").unwrap();
        let on_plan = crate::geo::apply(&h, (0.1, 0.9)).unwrap();
        assert!((on_plan.0 - 0.2).abs() < 1e-6 && (on_plan.1 - 0.8).abs() < 1e-6);
        assert!(p.homography_for("b").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_picture_is_checked_sized_and_a_new_shape_clears_the_scale_and_calibrations() {
        let dir = std::env::temp_dir().join(format!("nvr-plan-img-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let s = PlanStore::open(&dir);
        assert!(s.set_image(b"GIF89a....").is_err(), "not a JPEG");
        assert!(s.set_image(&[0xFF, 0xD8, 0xFF, 0x00]).is_err(), "a JPEG header with nothing in it");
        assert!(s.set_image(&jpeg(32, 32)).is_err(), "too small");
        assert!(s.image_path().is_none());
        let p = s.set_image(&jpeg(400, 300)).unwrap();
        assert_eq!((p.has_image, p.image_w, p.image_h), (true, 400, 300));
        assert!(s.image_path().unwrap().exists());
        let calib = Calib { picture: [[0.1, 0.9], [0.9, 0.9], [0.7, 0.4], [0.3, 0.4]], plan: [[0.2, 0.8], [0.6, 0.8], [0.6, 0.4], [0.2, 0.4]] };
        s.update(Plan { px_per_m: 20.0, cameras: vec![PlanCamera { calib: Some(calib), ..cam("a") }], ..Default::default() }).unwrap();
        // 400 px at 20 px/m: the plan is 20 m wide.
        let p = s.get();
        assert!((p.metres((0.0, 0.0), (1.0, 0.0)).unwrap() - 20.0).abs() < 1e-9);
        // The same shape again keeps them; another shape can't (the spots moved).
        let p = s.set_image(&jpeg(400, 300)).unwrap();
        assert_eq!(p.px_per_m, 20.0);
        assert!(p.cameras[0].calib.is_some());
        let p = s.set_image(&jpeg(300, 400)).unwrap();
        assert_eq!(p.px_per_m, 0.0);
        assert!(p.cameras[0].calib.is_none());
        let p = s.remove_image().unwrap();
        assert!(!p.has_image && s.image_path().is_none());
        // Saving the plan can't give it a picture it doesn't have.
        let p = s.update(Plan { has_image: true, image_w: 999, ..Default::default() }).unwrap();
        assert!(!p.has_image && p.image_w == 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn icon_sizes_are_checked_and_an_old_plan_without_them_gets_normal_size() {
        let dir = std::env::temp_dir().join(format!("nvr-plan-icons-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let s = PlanStore::open(&dir);
        // Nothing saved for the sizes (an old plan.json, or a plan sent without them): normal.
        let p = s.update(Plan { cameras: vec![PlanCamera { size: 1.0, ..cam("a") }], ..Default::default() }).unwrap();
        assert_eq!(p.icon_scale, 1.0);
        let old: PlanCamera = serde_json::from_str(r#"{"id":"b","x":0.5,"y":0.5}"#).unwrap();
        assert_eq!(old.size, 1.0);
        // Bigger and smaller, kept.
        let p = s.update(Plan { icon_scale: 2.5, cameras: vec![PlanCamera { size: 0.5, ..cam("a") }], ..Default::default() }).unwrap();
        assert_eq!((p.icon_scale, p.cameras[0].size), (2.5, 0.5));
        assert_eq!(s.get().icon_scale, 2.5);
        // Absurd sizes refused.
        for bad in [0.01, 50.0, f64::NAN] {
            assert!(s.update(Plan { icon_scale: bad, ..Default::default() }).is_err(), "{bad}");
            assert!(s.update(Plan { cameras: vec![PlanCamera { size: bad, ..cam("a") }], ..Default::default() }).is_err(), "{bad}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn walls_are_checked_kept_and_cleared_with_the_picture() {
        let dir = std::env::temp_dir().join(format!("nvr-plan-walls-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let s = PlanStore::open(&dir);
        let w = |a: f64, b: f64, c: f64, d: f64| Wall { x1: a, y1: b, x2: c, y2: d };
        s.set_image(&jpeg(400, 300)).unwrap();
        let p = s.update(Plan { walls: vec![w(0.1, 0.1, 0.9, 0.1), w(0.9, 0.1, 0.9, 0.8)], ..Default::default() }).unwrap();
        assert_eq!(p.walls.len(), 2);
        assert_eq!(PlanStore::open(&dir).get().walls.len(), 2, "kept across a restart");
        for (what, bad) in [
            ("outside the plan", vec![w(0.1, 0.1, 1.5, 0.1)]),
            ("no length", vec![w(0.3, 0.3, 0.3, 0.3)]),
            ("not a number", vec![w(f64::NAN, 0.1, 0.5, 0.5)]),
            ("too many", (0..MAX_WALLS + 1).map(|i| w(0.1, 0.1, 0.2, 0.1 + (i % 50) as f64 * 0.01)).collect()),
        ] {
            assert!(s.update(Plan { walls: bad, ..Default::default() }).is_err(), "{what} should be refused");
        }
        assert_eq!(s.get().walls.len(), 2, "a refused change leaves the walls alone");
        // The same shape keeps them; another shape (new picture) clears them, as it clears calibrations.
        assert_eq!(s.set_image(&jpeg(400, 300)).unwrap().walls.len(), 2);
        assert!(s.set_image(&jpeg(300, 400)).unwrap().walls.is_empty());
        s.update(Plan { walls: vec![w(0.1, 0.1, 0.9, 0.1)], ..Default::default() }).unwrap();
        assert!(s.remove_image().unwrap().walls.is_empty());
        // Older plan files have no walls.
        let p: Plan = serde_json::from_str(r#"{"cameras":[]}"#).unwrap();
        assert!(p.walls.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

