//! Faces and number plates, on this computer.
//!
//! - Faces: OpenCV's **YuNet** finds faces (and 5 landmarks), each face is
//!   straightened to the standard 112×112 position and **SFace** turns it
//!   into a 128-number "faceprint". Two faceprints of the same person are
//!   close (cosine ≥ 0.36, OpenCV's recommended threshold).
//! - Plates: **fast-alpr**'s YOLOv9 plate finder and its plate reader
//!   (CCT, Latin letters and digits, up to 9 characters).
//!
//! The models (≈50 MB together) download the first time they're needed,
//! load only while used and unload after a few idle minutes.

use crate::imgutil::Frame;
use anyhow::{Result, anyhow};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::info;

type Plan = Arc<tract_onnx::prelude::TypedSimplePlan>;

const YUNET: (&str, &str) = (
    "face-yunet-2023mar.onnx",
    "https://media.githubusercontent.com/media/opencv/opencv_zoo/main/models/face_detection_yunet/face_detection_yunet_2023mar.onnx",
);
const SFACE: (&str, &str) = (
    "face-sface-2021dec.onnx",
    "https://media.githubusercontent.com/media/opencv/opencv_zoo/main/models/face_recognition_sface/face_recognition_sface_2021dec.onnx",
);
const PLATE_DET: (&str, &str) = (
    "plate-yolov9t-384.onnx",
    "https://github.com/ankandrew/open-image-models/releases/download/assets/yolo-v9-t-384-license-plates-end2end.onnx",
);
const PLATE_OCR: (&str, &str) = (
    "plate-ocr-cct-xs-global.onnx",
    "https://github.com/ankandrew/fast-plate-ocr/releases/download/arg-plates/cct_xs_v1_global.onnx",
);
const OCR_ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ_";
/// Faceprints this close (cosine) are the same person.
pub const FACE_MATCH: f32 = 0.36;
const IDLE_UNLOAD: Duration = Duration::from_secs(600);

#[derive(Debug, Clone)]
pub struct Face {
    /// Box in the picture, pixels.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub score: f32,
    /// Eyes, nose tip, mouth corners (pixels).
    pub marks: [[f32; 2]; 5],
}

#[derive(Debug, Clone)]
pub struct Plate {
    pub text: String,
}

/// Plate readings less sure than this are ignored (a wrong plate is worse
/// than none: it could match someone else's car).
const PLATE_MIN_CONFIDENCE: f32 = 0.6;

#[derive(Default)]
struct Loaded {
    yunet: Option<Plan>,
    sface: Option<Plan>,
    plate_det: Option<Plan>,
    plate_ocr: Option<Plan>,
    used: Option<Instant>,
}

pub struct Recog {
    dir: PathBuf,
    loaded: Mutex<Loaded>,
    gate: tokio::sync::Mutex<()>,
    pub error: Mutex<Option<String>>,
    pub downloading: Mutex<Option<String>>,
}

impl Recog {
    pub fn new(data_dir: &Path) -> Arc<Self> {
        Arc::new(Self {
            dir: data_dir.join("models"),
            loaded: Mutex::default(),
            gate: tokio::sync::Mutex::new(()),
            error: Mutex::new(None),
            downloading: Mutex::new(None),
        })
    }

    pub fn unload_if_idle(&self) {
        let mut l = self.loaded.lock().unwrap();
        if l.used.is_some_and(|u| u.elapsed() > IDLE_UNLOAD) {
            *l = Loaded::default();
            info!("faces & plates: models unloaded (idle)");
        }
    }

    /// `u8_input`: the model takes this fixed shape of bytes (plate reader).
    async fn model(
        &self,
        which: (&str, &str),
        pick: fn(&mut Loaded) -> &mut Option<Plan>,
        u8_input: Option<[usize; 4]>,
    ) -> Result<Plan> {
        {
            let mut l = self.loaded.lock().unwrap();
            l.used = Some(Instant::now());
            if let Some(p) = pick(&mut l) {
                return Ok(p.clone());
            }
        }
        let _g = self.gate.lock().await;
        if let Some(p) = pick(&mut self.loaded.lock().unwrap()) {
            return Ok(p.clone());
        }
        let path = self.dir.join(which.0);
        if !path.exists() {
            *self.downloading.lock().unwrap() = Some(which.0.to_string());
            let r = download(which.1, &path).await;
            *self.downloading.lock().unwrap() = None;
            r.map_err(|e| anyhow!("downloading {}: {e:#}", which.0))?;
        }
        let p = path.clone();
        let plan = tokio::task::spawn_blocking(move || -> Result<Plan> {
            use tract_onnx::prelude::*;
            let mut m = tract_onnx::onnx()
                .with_ignore_value_info(true)
                .with_ignore_output_shapes(true)
                .model_for_path(&p)
                .map_err(|e| anyhow!("{e}"))?;
            if let Some(shape) = u8_input {
                m = m
                    .with_input_fact(0, u8::fact(shape).into())
                    .map_err(|e| anyhow!("{e}"))?;
            }
            m.into_optimized()
                .map_err(|e| anyhow!("{e}"))?
                .into_runnable()
                .map_err(|e| anyhow!("{e}"))
        })
        .await??;
        *pick(&mut self.loaded.lock().unwrap()) = Some(plan.clone());
        info!("faces & plates: {} loaded", which.0);
        Ok(plan)
    }

    /// Faces in a picture (best first).
    pub async fn faces(&self, f: &Frame) -> Result<Vec<Face>> {
        let plan = self.model(YUNET, |l| &mut l.yunet, None).await?;
        let f = f.clone();
        tokio::task::spawn_blocking(move || detect_faces(&plan, &f)).await?
    }

    /// Faceprint (unit length) of a face found by `faces`.
    pub async fn faceprint(&self, f: &Frame, face: &Face) -> Result<Vec<f32>> {
        let plan = self.model(SFACE, |l| &mut l.sface, None).await?;
        let aligned = align_face(f, &face.marks);
        tokio::task::spawn_blocking(move || {
            use tract_onnx::prelude::*;
            let mut data = vec![0f32; 3 * 112 * 112];
            for y in 0..112 {
                for x in 0..112 {
                    let i = (y * 112 + x) * 3;
                    for c in 0..3 {
                        data[c * 112 * 112 + y * 112 + x] = aligned[i + c] as f32;
                    }
                }
            }
            let t = Tensor::from_shape(&[1, 3, 112, 112], &data).map_err(|e| anyhow!("{e}"))?;
            let out = plan.run(tvec!(t.into())).map_err(|e| anyhow!("{e}"))?;
            let v: Vec<f32> = out[0]
                .to_plain_array_view::<f32>()
                .map_err(|e| anyhow!("{e}"))?
                .iter()
                .copied()
                .collect();
            Ok(crate::clip::normalize(v))
        })
        .await?
    }

    /// Number plates in a picture (usually a vehicle cut out of a frame).
    pub async fn plates(&self, f: &Frame) -> Result<Vec<Plate>> {
        let det = self.model(PLATE_DET, |l| &mut l.plate_det, None).await?;
        let ocr = self
            .model(PLATE_OCR, |l| &mut l.plate_ocr, Some([1, 64, 128, 3]))
            .await?;
        let f = f.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<Plate>> {
            let mut out = Vec::new();
            for (x, y, w, h, _) in detect_plates(&det, &f)? {
                let crop = crop(&f, x, y, w, h);
                if let Some((text, confidence)) = read_plate(&ocr, &crop)?
                    && text.len() >= 3
                    && confidence >= PLATE_MIN_CONFIDENCE
                {
                    out.push(Plate { text });
                }
            }
            Ok(out)
        })
        .await?
    }
}

async fn download(url: &str, path: &Path) -> Result<()> {
    info!("faces & plates: downloading {url}");
    crate::jobs::download("a faces & plates model", url, path, 10_000).await
}

/// Cut a box (pixels) out of a frame.
pub fn crop(f: &Frame, x: f32, y: f32, w: f32, h: f32) -> Frame {
    let x0 = (x.max(0.0) as u32).min(f.width.saturating_sub(2));
    let y0 = (y.max(0.0) as u32).min(f.height.saturating_sub(2));
    let x1 = ((x + w) as u32).clamp(x0 + 1, f.width);
    let y1 = ((y + h) as u32).clamp(y0 + 1, f.height);
    let (cw, ch) = (x1 - x0, y1 - y0);
    let mut rgb = Vec::with_capacity((cw * ch * 3) as usize);
    for row in y0..y1 {
        let s = ((row * f.width + x0) * 3) as usize;
        rgb.extend_from_slice(&f.rgb[s..s + (cw * 3) as usize]);
    }
    Frame {
        width: cw,
        height: ch,
        rgb: Arc::new(rgb),
    }
}

/// Letterbox into a square of `size` (top-left, rest grey). Returns pixels
/// and the scale used.
fn letterbox(f: &Frame, size: u32) -> (Vec<u8>, f32) {
    let r = (size as f32 / f.width as f32).min(size as f32 / f.height as f32);
    let (nw, nh) = (
        ((f.width as f32 * r) as u32).max(1),
        ((f.height as f32 * r) as u32).max(1),
    );
    let small = crate::imgutil::resize_rgb(&f.rgb, f.width, f.height, nw, nh);
    let mut out = vec![114u8; (size * size * 3) as usize];
    for y in 0..nh as usize {
        let s = y * nw as usize * 3;
        let d = y * size as usize * 3;
        out[d..d + nw as usize * 3].copy_from_slice(&small[s..s + nw as usize * 3]);
    }
    (out, r)
}

fn detect_faces(plan: &Plan, f: &Frame) -> Result<Vec<Face>> {
    use tract_onnx::prelude::*;
    const S: usize = 640;
    let (img, r) = letterbox(f, S as u32);
    // YuNet takes BGR, 0..255.
    let mut data = vec![0f32; 3 * S * S];
    for i in 0..S * S {
        data[i] = img[i * 3 + 2] as f32;
        data[S * S + i] = img[i * 3 + 1] as f32;
        data[2 * S * S + i] = img[i * 3] as f32;
    }
    let t = Tensor::from_shape(&[1, 3, S, S], &data).map_err(|e| anyhow!("{e}"))?;
    let out = plan.run(tvec!(t.into())).map_err(|e| anyhow!("{e}"))?;
    let v = |i: usize| -> Result<Vec<f32>> {
        Ok(out[i]
            .to_plain_array_view::<f32>()
            .map_err(|e| anyhow!("{e}"))?
            .iter()
            .copied()
            .collect())
    };
    let mut faces = Vec::new();
    // Outputs: cls ×3, obj ×3, bbox ×3, kps ×3 (strides 8, 16, 32).
    for (k, stride) in [8usize, 16, 32].into_iter().enumerate() {
        let (cls, obj, bbox, kps) = (v(k)?, v(3 + k)?, v(6 + k)?, v(9 + k)?);
        let cols = S / stride;
        for (idx, (&c, &o)) in cls.iter().zip(obj.iter()).enumerate() {
            let score = (c.clamp(0.0, 1.0) * o.clamp(0.0, 1.0)).sqrt();
            if score < 0.6 {
                continue;
            }
            let (row, col) = ((idx / cols) as f32, (idx % cols) as f32);
            let s = stride as f32;
            let b = &bbox[idx * 4..idx * 4 + 4];
            let (cx, cy) = ((col + b[0]) * s, (row + b[1]) * s);
            let (w, h) = (b[2].exp() * s, b[3].exp() * s);
            let mut marks = [[0f32; 2]; 5];
            for (n, m) in marks.iter_mut().enumerate() {
                *m = [
                    ((kps[idx * 10 + 2 * n] + col) * s) / r,
                    ((kps[idx * 10 + 2 * n + 1] + row) * s) / r,
                ];
            }
            faces.push(Face {
                x: (cx - w / 2.0) / r,
                y: (cy - h / 2.0) / r,
                w: w / r,
                h: h / r,
                score,
                marks,
            });
        }
    }
    // Overlapping finds of the same face: keep the best.
    faces.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut keep: Vec<Face> = Vec::new();
    for f in faces {
        if !keep
            .iter()
            .any(|k| iou(k.x, k.y, k.w, k.h, f.x, f.y, f.w, f.h) > 0.3)
        {
            keep.push(f);
        }
    }
    Ok(keep)
}

fn iou(ax: f32, ay: f32, aw: f32, ah: f32, bx: f32, by: f32, bw: f32, bh: f32) -> f32 {
    let x0 = ax.max(bx);
    let y0 = ay.max(by);
    let x1 = (ax + aw).min(bx + bw);
    let y1 = (ay + ah).min(by + bh);
    let i = (x1 - x0).max(0.0) * (y1 - y0).max(0.0);
    let u = aw * ah + bw * bh - i;
    if u <= 0.0 { 0.0 } else { i / u }
}

/// Straighten a face to the standard 112×112 position (eyes, nose and
/// mouth where the recogniser expects them), RGB pixels.
pub fn align_face(f: &Frame, marks: &[[f32; 2]; 5]) -> Vec<u8> {
    const DST: [[f32; 2]; 5] = [
        [38.2946, 51.6963],
        [73.5318, 51.5014],
        [56.0252, 71.7366],
        [41.5493, 92.3655],
        [70.7299, 92.2041],
    ];
    // Least-squares similarity transform (rotation + scale + shift) marks → DST.
    let n = 5.0;
    let (ms, md) = (
        [
            marks.iter().map(|p| p[0]).sum::<f32>() / n,
            marks.iter().map(|p| p[1]).sum::<f32>() / n,
        ],
        [
            DST.iter().map(|p| p[0]).sum::<f32>() / n,
            DST.iter().map(|p| p[1]).sum::<f32>() / n,
        ],
    );
    let (mut num_a, mut num_b, mut den) = (0f32, 0f32, 0f32);
    for (s, d) in marks.iter().zip(DST.iter()) {
        let (sx, sy, dx, dy) = (s[0] - ms[0], s[1] - ms[1], d[0] - md[0], d[1] - md[1]);
        num_a += dx * sx + dy * sy;
        num_b += dy * sx - dx * sy;
        den += sx * sx + sy * sy;
    }
    let (a, b) = if den > 0.0 {
        (num_a / den, num_b / den)
    } else {
        (1.0, 0.0)
    };
    let tx = md[0] - (a * ms[0] - b * ms[1]);
    let ty = md[1] - (b * ms[0] + a * ms[1]);
    let det = a * a + b * b;
    let mut out = vec![0u8; 112 * 112 * 3];
    for v in 0..112 {
        for u in 0..112 {
            // Back to the source picture.
            let (px, py) = (u as f32 - tx, v as f32 - ty);
            let (x, y) = ((a * px + b * py) / det, (-b * px + a * py) / det);
            if x < 0.0 || y < 0.0 || x >= (f.width - 1) as f32 || y >= (f.height - 1) as f32 {
                continue;
            }
            let (x0, y0) = (x as u32, y as u32);
            let (fx, fy) = (x - x0 as f32, y - y0 as f32);
            for c in 0..3 {
                let p = |xx: u32, yy: u32| f.rgb[((yy * f.width + xx) * 3) as usize + c] as f32;
                let val = p(x0, y0) * (1.0 - fx) * (1.0 - fy)
                    + p(x0 + 1, y0) * fx * (1.0 - fy)
                    + p(x0, y0 + 1) * (1.0 - fx) * fy
                    + p(x0 + 1, y0 + 1) * fx * fy;
                out[(v * 112 + u) * 3 + c] = val.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

/// Plate boxes (pixels, with score) — the model's output already has
/// overlapping boxes removed: rows of (batch, x1, y1, x2, y2, class, score).
fn detect_plates(plan: &Plan, f: &Frame) -> Result<Vec<(f32, f32, f32, f32, f32)>> {
    use tract_onnx::prelude::*;
    const S: usize = 384;
    let (img, r) = letterbox(f, S as u32);
    let mut data = vec![0f32; 3 * S * S];
    for i in 0..S * S {
        for c in 0..3 {
            data[c * S * S + i] = img[i * 3 + c] as f32 / 255.0;
        }
    }
    let t = Tensor::from_shape(&[1, 3, S, S], &data).map_err(|e| anyhow!("{e}"))?;
    let out = plan.run(tvec!(t.into())).map_err(|e| anyhow!("{e}"))?;
    let v: Vec<f32> = out[0]
        .cast_to::<f32>()
        .map_err(|e| anyhow!("{e}"))?
        .to_plain_array_view::<f32>()
        .map_err(|e| anyhow!("{e}"))?
        .iter()
        .copied()
        .collect();
    let mut plates = Vec::new();
    for row in v.chunks_exact(7) {
        let (x1, y1, x2, y2, score) = (row[1], row[2], row[3], row[4], row[6]);
        if score < 0.4 || x2 <= x1 || y2 <= y1 {
            continue;
        }
        // Add a little margin so the reader sees whole characters.
        let (pw, ph) = ((x2 - x1) / r, (y2 - y1) / r);
        plates.push((
            x1 / r - pw * 0.05,
            y1 / r - ph * 0.1,
            pw * 1.1,
            ph * 1.2,
            score,
        ));
    }
    Ok(plates)
}

fn read_plate(plan: &Plan, f: &Frame) -> Result<Option<(String, f32)>> {
    use tract_onnx::prelude::*;
    let small = crate::imgutil::resize_rgb(&f.rgb, f.width, f.height, 128, 64);
    let t = Tensor::from_shape(&[1, 64, 128, 3], &small).map_err(|e| anyhow!("{e}"))?;
    let out = plan.run(tvec!(t.into())).map_err(|e| anyhow!("{e}"))?;
    let p: Vec<f32> = out[0]
        .to_plain_array_view::<f32>()
        .map_err(|e| anyhow!("{e}"))?
        .iter()
        .copied()
        .collect();
    let n = OCR_ALPHABET.len();
    let (mut text, mut conf) = (String::new(), Vec::new());
    for slot in p.chunks_exact(n) {
        let (i, &best) = slot
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap();
        let ch = OCR_ALPHABET[i] as char;
        if ch != '_' {
            text.push(ch);
            conf.push(best);
        }
    }
    if text.is_empty() {
        return Ok(None);
    }
    let c = conf.iter().sum::<f32>() / conf.len() as f32;
    Ok((c >= 0.5).then_some((text, c)))
}

/// Plates that are the same allowing one misread character.
pub fn plate_matches(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_uppercase())
            .collect::<Vec<_>>()
    };
    let (a, b) = (norm(a), norm(b));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    // Edit distance ≤ 1.
    let (n, m) = (a.len(), b.len());
    let mut prev: Vec<usize> = (0..=m).collect();
    for i in 1..=n {
        let mut cur = vec![i; m + 1];
        for j in 1..=m {
            cur[j] = (prev[j] + 1)
                .min(cur[j - 1] + 1)
                .min(prev[j - 1] + (a[i - 1] != b[j - 1]) as usize);
        }
        prev = cur;
    }
    prev[m] <= 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plates_allow_one_misread() {
        assert!(plate_matches("ABC123", "abc-123"));
        assert!(plate_matches("ABC123", "A8C123"));
        assert!(!plate_matches("ABC123", "XYZ789"));
    }

    /// Needs NVR_TEST_FACE_DIR (the two face models) and NVR_TEST_AI_DIR (zidane.jpg).
    #[tokio::test]
    async fn finds_and_tells_apart_faces() {
        let (Ok(md), Ok(ad)) = (
            std::env::var("NVR_TEST_FACE_DIR"),
            std::env::var("NVR_TEST_AI_DIR"),
        ) else {
            return;
        };
        let data = std::env::temp_dir().join(format!("nvr-face-{}", std::process::id()));
        std::fs::create_dir_all(data.join("models")).unwrap();
        for (n, src) in [
            (YUNET.0, "face_detection_yunet_2023mar.onnx"),
            (SFACE.0, "face_recognition_sface_2021dec.onnx"),
        ] {
            std::fs::copy(Path::new(&md).join(src), data.join("models").join(n)).unwrap();
        }
        let r = Recog::new(&data);
        let img =
            crate::imgutil::decode_jpeg(&std::fs::read(Path::new(&ad).join("zidane.jpg")).unwrap())
                .unwrap();
        let faces = r.faces(&img).await.unwrap();
        for f in &faces {
            println!(
                "face at {:.0},{:.0} {:.0}×{:.0} score {:.2}",
                f.x, f.y, f.w, f.h, f.score
            );
        }
        assert!(faces.len() >= 2, "two men in the picture");
        let a = r.faceprint(&img, &faces[0]).await.unwrap();
        let b = r.faceprint(&img, &faces[1]).await.unwrap();
        // The same face from a slightly smaller picture still matches itself.
        let small = img.resize(img.width * 3 / 4, img.height * 3 / 4);
        let f2 = r.faces(&small).await.unwrap();
        let near = f2
            .iter()
            .min_by(|x, y| {
                ((x.x * 4.0 / 3.0 - faces[0].x).abs())
                    .total_cmp(&(y.x * 4.0 / 3.0 - faces[0].x).abs())
            })
            .unwrap();
        let a2 = r.faceprint(&small, near).await.unwrap();
        let cos = |p: &[f32], q: &[f32]| p.iter().zip(q).map(|(x, y)| x * y).sum::<f32>();
        println!(
            "same person {:.2}, different people {:.2}",
            cos(&a, &a2),
            cos(&a, &b)
        );
        assert!(cos(&a, &a2) > FACE_MATCH);
        assert!(cos(&a, &b) < FACE_MATCH);
        std::fs::remove_dir_all(&data).ok();
    }
}
