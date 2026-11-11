//! Object recognition: the built-in YOLOX model, run on this computer's
//! graphics card or processor (downloaded on first use), or off.

use crate::config::AiConfig;
use crate::imgutil::Frame;
use anyhow::{Result, anyhow};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;
use tracing::{info, warn};

pub const COCO: [&str; 80] = [
    "person",
    "bicycle",
    "car",
    "motorcycle",
    "airplane",
    "bus",
    "train",
    "truck",
    "boat",
    "traffic light",
    "fire hydrant",
    "stop sign",
    "parking meter",
    "bench",
    "bird",
    "cat",
    "dog",
    "horse",
    "sheep",
    "cow",
    "elephant",
    "bear",
    "zebra",
    "giraffe",
    "backpack",
    "umbrella",
    "handbag",
    "tie",
    "suitcase",
    "frisbee",
    "skis",
    "snowboard",
    "sports ball",
    "kite",
    "baseball bat",
    "baseball glove",
    "skateboard",
    "surfboard",
    "tennis racket",
    "bottle",
    "wine glass",
    "cup",
    "fork",
    "knife",
    "spoon",
    "bowl",
    "banana",
    "apple",
    "sandwich",
    "orange",
    "broccoli",
    "carrot",
    "hot dog",
    "pizza",
    "donut",
    "cake",
    "chair",
    "couch",
    "potted plant",
    "bed",
    "dining table",
    "toilet",
    "tv",
    "laptop",
    "mouse",
    "remote",
    "keyboard",
    "cell phone",
    "microwave",
    "oven",
    "toaster",
    "sink",
    "refrigerator",
    "book",
    "clock",
    "vase",
    "scissors",
    "teddy bear",
    "hair drier",
    "toothbrush",
];

/// Objects most useful for home security, offered as alert choices.
/// Shown on every camera's AI tab (and the Alerts filter).
pub const MAIN_OBJECTS: &[&str] = &[
    "person",
    "car",
    "truck",
    "motorcycle",
    "bicycle",
    "dog",
    "cat",
];

/// "More things to watch for": the rest the built-in model knows, the
/// most useful for a home camera first.
pub fn more_objects() -> Vec<&'static str> {
    const FIRST: &[&str] = &[
        "bus",
        "bird",
        "horse",
        "cow",
        "sheep",
        "bear",
        "backpack",
        "handbag",
        "suitcase",
        "umbrella",
        "boat",
        "train",
        "airplane",
        "skateboard",
        "sports ball",
        "kite",
        "frisbee",
        "bench",
        "chair",
        "potted plant",
    ];
    let mut v: Vec<&str> = FIRST.to_vec();
    v.extend(
        COCO.iter()
            .filter(|o| !FIRST.contains(o) && !MAIN_OBJECTS.contains(o)),
    );
    v
}

pub const SECURITY_OBJECTS: &[&str] = &[
    "person",
    "car",
    "truck",
    "bus",
    "motorcycle",
    "bicycle",
    "dog",
    "cat",
    "bird",
    "horse",
    "cow",
    "sheep",
    "bear",
    "backpack",
    "suitcase",
    "handbag",
    "umbrella",
];

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct Detection {
    pub label: String,
    pub score: f32,
    /// Box, normalized 0..1 (x, y = top-left).
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug)]
pub enum AiError {
    /// Built-in model not ready yet.
    NotReady(String),
    Failed(anyhow::Error),
}

impl std::fmt::Display for AiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AiError::NotReady(s) => write!(f, "{s}"),
            AiError::Failed(e) => write!(f, "{e:#}"),
        }
    }
}

impl From<anyhow::Error> for AiError {
    fn from(e: anyhow::Error) -> Self {
        AiError::Failed(e)
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct AiStatus {
    pub provider: String,
    pub ready: bool,
    pub downloading: bool,
    pub download_percent: Option<f32>,
    pub error: Option<String>,
    pub requests: u64,
    pub failures: u64,
    pub last_ms: Option<u64>,
}

/// Holds the current AI engine; replaced when settings change.
pub struct AiHandle {
    inner: RwLock<Arc<Ai>>,
}

impl AiHandle {
    pub fn new(cfg: AiConfig, data_dir: &Path) -> Self {
        Self {
            inner: RwLock::new(Ai::new(cfg, data_dir)),
        }
    }
    pub fn get(&self) -> Arc<Ai> {
        self.inner.read().unwrap().clone()
    }
    pub fn replace(&self, cfg: AiConfig, data_dir: &Path) {
        *self.inner.write().unwrap() = Ai::new(cfg, data_dir);
    }
}

pub struct Ai {
    pub cfg: AiConfig,
    models_dir: PathBuf,
    model: Mutex<Option<Arc<Yolox>>>,
    /// Bigger model that double-checks possible alerts (optional).
    verifier: Mutex<Option<Arc<Yolox>>>,
    status: Mutex<AiStatus>,
    busy: tokio::sync::Semaphore,
}

impl Ai {
    fn new(cfg: AiConfig, data_dir: &Path) -> Arc<Self> {
        let status = AiStatus {
            provider: cfg.provider.clone(),
            ready: cfg.provider != "builtin",
            ..Default::default()
        };
        let ai = Arc::new(Self {
            // The built-in AI runs on this computer's processor: at most a
            // quarter of its cores at once, so recording and live view keep
            // their share.
            busy: tokio::sync::Semaphore::new(
                std::thread::available_parallelism()
                    .map(|n| n.get() / 4)
                    .unwrap_or(1)
                    .clamp(1, 4),
            ),
            cfg,
            models_dir: data_dir.join("models"),
            model: Mutex::new(None),
            verifier: Mutex::new(None),
            status: Mutex::new(status),
        });
        if ai.cfg.provider == "builtin" {
            ai.clone().prepare_builtin();
        }
        ai
    }

    pub fn enabled(&self) -> bool {
        self.cfg.provider != "none"
    }

    pub fn status(&self) -> AiStatus {
        self.status.lock().unwrap().clone()
    }

    fn model_path(&self) -> PathBuf {
        self.models_dir
            .join(format!("{}.onnx", self.cfg.builtin_model))
    }

    /// The model that double-checks possible alerts, if one is chosen and
    /// it differs from the main model.
    fn verify_name(&self) -> Option<String> {
        let v = self.cfg.verify_model.trim();
        (!v.is_empty() && v != self.cfg.builtin_model).then(|| v.to_string())
    }

    /// Load (downloading first if needed) the built-in model in the background.
    fn prepare_builtin(self: Arc<Self>) {
        tokio::spawn(async move {
            let path = self.model_path();
            if !path.exists()
                && let Err(e) = self.download_model(&self.cfg.builtin_model, &path).await
            {
                warn!("AI: model download failed: {e:#}");
                let mut s = self.status.lock().unwrap();
                s.downloading = false;
                s.error = Some(format!("model download failed: {e:#}"));
                return;
            }
            let p = path.clone();
            match tokio::task::spawn_blocking(move || Yolox::load(&p)).await {
                Ok(Ok(m)) => {
                    info!(
                        "AI: built-in model {} loaded ({}×{} input)",
                        self.cfg.builtin_model, m.size, m.size
                    );
                    *self.model.lock().unwrap() = Some(Arc::new(m));
                    {
                        let mut s = self.status.lock().unwrap();
                        s.ready = true;
                        s.error = None;
                    }
                    self.clone().prepare_verifier().await;
                }
                Ok(Err(e)) => {
                    warn!("AI: cannot load model {}: {e:#}", path.display());
                    self.status.lock().unwrap().error = Some(format!("cannot load model: {e:#}"));
                    let _ = std::fs::remove_file(&path); // probably a broken download
                }
                Err(e) => warn!("AI: {e}"),
            }
        });
    }

    /// Load the double-checking model (after the main one is ready).
    async fn prepare_verifier(self: Arc<Self>) {
        let Some(name) = self.verify_name() else {
            return;
        };
        let path = self.models_dir.join(format!("{name}.onnx"));
        if !path.exists()
            && let Err(e) = self.download_model(&name, &path).await
        {
            warn!("AI: download of the double-checking model failed: {e:#}");
            let mut s = self.status.lock().unwrap();
            s.downloading = false;
            s.error = Some(format!("double-checking model download failed: {e:#}"));
            return;
        }
        let p = path.clone();
        match tokio::task::spawn_blocking(move || Yolox::load(&p)).await {
            Ok(Ok(m)) => {
                info!("AI: {name} double-checks possible alerts");
                *self.verifier.lock().unwrap() = Some(Arc::new(m));
            }
            Ok(Err(e)) => {
                warn!("AI: cannot load {}: {e:#}", path.display());
                let _ = std::fs::remove_file(&path);
            }
            Err(e) => warn!("AI: {e}"),
        }
    }

    async fn download_model(&self, name: &str, path: &Path) -> Result<()> {
        let url = format!(
            "https://github.com/Megvii-BaseDetection/YOLOX/releases/download/0.1.1rc0/{name}.onnx"
        );
        info!("AI: downloading {url}");
        {
            let mut s = self.status.lock().unwrap();
            s.downloading = true;
            s.download_percent = Some(0.0);
        }
        std::fs::create_dir_all(&self.models_dir)?;
        let client = crate::jobs::download_client()?;
        let mut resp = client.get(&url).send().await?.error_for_status()?;
        let total = resp.content_length();
        let tmp = path.with_extension("part");
        let mut file = tokio::fs::File::create(&tmp).await?;
        let mut got = 0u64;
        let job = crate::jobs::start("download", format!("Downloading the {name} AI model"));
        while let Some(chunk) = resp.chunk().await? {
            tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
            got += chunk.len() as u64;
            job.bytes(got, total.unwrap_or(0));
            if let Some(t) = total {
                self.status.lock().unwrap().download_percent = Some(got as f32 * 100.0 / t as f32);
            }
        }
        tokio::io::AsyncWriteExt::flush(&mut file).await?;
        drop(file);
        crate::jobs::check_complete(&tmp, got, total.unwrap_or(0))?;
        tokio::fs::rename(&tmp, path).await?;
        info!(
            "AI: model saved to {} ({} MB)",
            path.display(),
            got / 1_000_000
        );
        let mut s = self.status.lock().unwrap();
        s.downloading = false;
        s.download_percent = None;
        Ok(())
    }

    /// Detect objects in a frame. Results are unfiltered (callers apply
    /// their own object list and confidence).
    pub async fn detect(&self, frame: &Frame) -> Result<Vec<Detection>, AiError> {
        if !self.enabled() {
            return Ok(vec![]);
        }
        let _permit = self
            .busy
            .acquire()
            .await
            .map_err(|e| AiError::Failed(e.into()))?;
        let t = Instant::now();
        let res = self.detect_builtin(frame).await;
        let mut s = self.status.lock().unwrap();
        s.requests += 1;
        s.last_ms = Some(t.elapsed().as_millis() as u64);
        match &res {
            Err(AiError::Failed(e)) => {
                s.failures += 1;
                s.error = Some(format!("{e:#}"));
            }
            _ => {}
        }
        res
    }

    async fn detect_builtin(&self, frame: &Frame) -> Result<Vec<Detection>, AiError> {
        let model = self.model.lock().unwrap().clone();
        let Some(model) = model else {
            let s = self.status();
            return Err(AiError::NotReady(match (s.downloading, s.error) {
                (true, _) => format!(
                    "downloading AI model ({:.0}%)",
                    s.download_percent.unwrap_or(0.0)
                ),
                (_, Some(e)) => e,
                _ => "AI model is loading".into(),
            }));
        };
        // Night boost: dark and infrared pictures are brightened first.
        let f = if self.cfg.night_boost {
            crate::imgutil::low_light_boost(frame).unwrap_or_else(|| frame.clone())
        } else {
            frame.clone()
        };
        let f2 = f.clone();
        let dets = tokio::task::spawn_blocking(move || model.detect(&f2, 0.25))
            .await
            .map_err(|e| AiError::Failed(e.into()))?
            .map_err(AiError::Failed)?;
        // Something that could become an alert: the bigger model takes a
        // second look and has the final say (fewer false alerts, and the
        // right label when the fast model mixes up e.g. a cat and a person).
        let verifier = self.verifier.lock().unwrap().clone();
        if let Some(v) = verifier
            && dets
                .iter()
                .any(|d| d.score >= 0.3 && SECURITY_OBJECTS.contains(&d.label.as_str()))
        {
            return tokio::task::spawn_blocking(move || v.detect(&f, 0.25))
                .await
                .map_err(|e| AiError::Failed(e.into()))?
                .map_err(AiError::Failed);
        }
        Ok(dets)
    }
}

/// The built-in YOLOX object detection model.
pub struct Yolox {
    net: crate::infer::Net,
    pub size: usize,
}

impl Yolox {
    pub fn load(path: &Path) -> Result<Self> {
        // Nano and Tiny take 416×416 pictures, the bigger models 640×640.
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let size = if name.contains("nano") || name.contains("tiny") {
            416
        } else {
            640
        };
        let net = crate::infer::load(path, &format!("object detection ({name})"), || {
            use tract_onnx::prelude::*;
            tract_onnx::onnx()
                .model_for_path(path)
                .map_err(|e| anyhow!("{e}"))?
                .with_input_fact(0, f32::fact([1, 3, size, size]).into())
                .map_err(|e| anyhow!("{e}"))?
                .into_optimized()
                .map_err(|e| anyhow!("{e}"))?
                .into_runnable()
                .map_err(|e| anyhow!("{e}"))
        })?;
        Ok(Self { net, size })
    }

    pub fn detect(&self, frame: &Frame, min_score: f32) -> Result<Vec<Detection>> {
        let size = self.size;
        let r = (size as f32 / frame.width as f32).min(size as f32 / frame.height as f32);
        let (nw, nh) = (
            ((frame.width as f32 * r) as u32).max(1),
            ((frame.height as f32 * r) as u32).max(1),
        );
        let resized = crate::imgutil::resize_rgb(&frame.rgb, frame.width, frame.height, nw, nh);
        // Letterboxed, BGR, planar, 0..255.
        let plane = size * size;
        let mut input = vec![114.0f32; 3 * plane];
        for y in 0..nh as usize {
            for x in 0..nw as usize {
                let i = (y * nw as usize + x) * 3;
                let o = y * size + x;
                input[o] = resized[i + 2] as f32;
                input[plane + o] = resized[i + 1] as f32;
                input[2 * plane + o] = resized[i] as f32;
            }
        }
        let out = self
            .net
            .run(crate::infer::Input::f32(&[1, 3, size, size], input))?;
        let data = out
            .first()
            .ok_or_else(|| anyhow!("unexpected model output"))?;
        let stride_len = 85;
        let mut dets = Vec::new();
        let mut idx = 0;
        for stride in [8usize, 16, 32] {
            let g = size / stride;
            for gy in 0..g {
                for gx in 0..g {
                    let row = &data[idx * stride_len..(idx + 1) * stride_len];
                    idx += 1;
                    let (mut best, mut class) = (0f32, 0usize);
                    for (c, &s) in row[5..].iter().enumerate() {
                        if s > best {
                            best = s;
                            class = c;
                        }
                    }
                    let score = row[4] * best;
                    if score < min_score {
                        continue;
                    }
                    let cx = (row[0] + gx as f32) * stride as f32 / r;
                    let cy = (row[1] + gy as f32) * stride as f32 / r;
                    let w = row[2].exp() * stride as f32 / r;
                    let h = row[3].exp() * stride as f32 / r;
                    let (fw, fh) = (frame.width as f32, frame.height as f32);
                    let x0 = ((cx - w / 2.0) / fw).clamp(0.0, 1.0);
                    let y0 = ((cy - h / 2.0) / fh).clamp(0.0, 1.0);
                    dets.push(Detection {
                        label: COCO[class].into(),
                        score,
                        x: x0,
                        y: y0,
                        w: (w / fw).min(1.0 - x0),
                        h: (h / fh).min(1.0 - y0),
                    });
                }
            }
        }
        Ok(nms(dets, 0.45))
    }
}

fn iou(a: &Detection, b: &Detection) -> f32 {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.w).min(b.x + b.w);
    let y1 = (a.y + a.h).min(b.y + b.h);
    let inter = (x1 - x0).max(0.0) * (y1 - y0).max(0.0);
    let union = a.w * a.h + b.w * b.h - inter;
    if union <= 0.0 { 0.0 } else { inter / union }
}

/// Two boxes that are most likely the same thing (a cat also detected as a
/// person, or the same object a moment later).
pub fn same_object(a: &Detection, b: &Detection) -> bool {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.w).min(b.x + b.w);
    let y1 = (a.y + a.h).min(b.y + b.h);
    let inter = (x1 - x0).max(0.0) * (y1 - y0).max(0.0);
    let smaller = (a.w * a.h).min(b.w * b.h);
    iou(a, b) > 0.45 || (smaller > 0.0 && inter / smaller > 0.75)
}

/// One answer per object: when the same object is recognized as two
/// things (cat 80%, person 60%), keep only the more confident one.
pub fn best_per_object(mut dets: Vec<Detection>) -> Vec<Detection> {
    dets.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut keep: Vec<Detection> = Vec::new();
    for d in dets {
        if !keep.iter().any(|k| same_object(k, &d)) {
            keep.push(d);
        }
    }
    keep
}

/// Non-maximum suppression per label.
pub fn nms(mut dets: Vec<Detection>, thr: f32) -> Vec<Detection> {
    dets.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut keep: Vec<Detection> = Vec::new();
    for d in dets {
        if !keep.iter().any(|k| k.label == d.label && iou(k, &d) > thr) {
            keep.push(d);
        }
    }
    keep
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_label_per_object() {
        let d = |l: &str, s: f32, x: f32| Detection {
            label: l.into(),
            score: s,
            x,
            y: 0.2,
            w: 0.2,
            h: 0.3,
        };
        let k = best_per_object(vec![
            d("person", 0.6, 0.1),
            d("cat", 0.8, 0.11),
            d("dog", 0.7, 0.6),
        ]);
        let labels: Vec<&str> = k.iter().map(|x| x.label.as_str()).collect();
        assert_eq!(labels, ["cat", "dog"]);
    }

    #[test]
    fn nms_merges_overlaps() {
        let d = |s: f32, x: f32| Detection {
            label: "person".into(),
            score: s,
            x,
            y: 0.1,
            w: 0.2,
            h: 0.5,
        };
        let k = nms(vec![d(0.8, 0.1), d(0.9, 0.11), d(0.7, 0.6)], 0.45);
        assert_eq!(k.len(), 2);
        assert_eq!(k[0].score, 0.9);
    }
}

#[cfg(test)]
mod model_tests {
    use super::*;

    /// Runs only when a model and a test picture (raw RGB + size) are provided:
    /// NVR_TEST_MODEL=path/yolox_nano.onnx NVR_TEST_RGB=path.rgb NVR_TEST_SIZE=1280x720
    #[test]
    fn yolox_finds_people() {
        let (Ok(model), Ok(rgb), Ok(size)) = (
            std::env::var("NVR_TEST_MODEL"),
            std::env::var("NVR_TEST_RGB"),
            std::env::var("NVR_TEST_SIZE"),
        ) else {
            return;
        };
        let (w, h) = size.split_once('x').unwrap();
        let frame = Frame {
            width: w.parse().unwrap(),
            height: h.parse().unwrap(),
            rgb: Arc::new(std::fs::read(rgb).unwrap()),
        };
        let m = Yolox::load(Path::new(&model)).unwrap();
        let d = m.detect(&frame, 0.4).unwrap();
        let people = d.iter().filter(|x| x.label == "person").count();
        eprintln!("{d:#?}");
        assert!(people >= 2, "expected at least 2 people, got {d:?}");
    }
}

#[cfg(test)]
mod night_tests {
    use super::*;

    /// Needs model files: NVR_TEST_AI_DIR with yolox_tiny.onnx and zidane.jpg.
    #[test]
    fn night_boost_helps_in_the_dark() {
        let Ok(dir) = std::env::var("NVR_TEST_AI_DIR") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        let m = Yolox::load(&dir.join("yolox_tiny.onnx")).unwrap();
        let img =
            crate::imgutil::decode_jpeg(&std::fs::read(dir.join("zidane.jpg")).unwrap()).unwrap();
        for k in [0.25f32, 0.12, 0.06] {
            // Night: dark and nearly grey (infrared), with sensor noise.
            let mut seed = 7u32;
            let rgb: Vec<u8> = img
                .rgb
                .chunks_exact(3)
                .flat_map(|p| {
                    let y = 0.3 * p[0] as f32 + 0.59 * p[1] as f32 + 0.11 * p[2] as f32;
                    seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
                    let noise = ((seed >> 16) % 7) as f32 - 3.0;
                    let v = (y * k + noise).clamp(0.0, 255.0) as u8;
                    [v, v, v]
                })
                .collect();
            let dark = Frame {
                width: img.width,
                height: img.height,
                rgb: Arc::new(rgb),
            };
            let best = |f: &Frame| {
                m.detect(f, 0.2)
                    .unwrap()
                    .iter()
                    .filter(|d| d.label == "person")
                    .map(|d| d.score)
                    .fold(0f32, f32::max)
            };
            let boosted = crate::imgutil::low_light_boost(&dark).expect("dark picture is boosted");
            let n = |f: &Frame| {
                m.detect(f, 0.3)
                    .unwrap()
                    .iter()
                    .filter(|d| d.label == "person")
                    .count()
            };
            println!(
                "brightness x{k}: person {:.2} → {:.2} with night boost; people found {} → {}",
                best(&dark),
                best(&boosted),
                n(&dark),
                n(&boosted)
            );
        }
        let bright = Frame {
            width: 64,
            height: 36,
            rgb: Arc::new((0..64 * 36 * 3).map(|i| (60 + i % 190) as u8).collect()),
        };
        assert!(
            crate::imgutil::low_light_boost(&bright).is_none(),
            "daylight picture left alone"
        );
    }
}
