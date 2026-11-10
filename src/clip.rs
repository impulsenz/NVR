//! Smart search with CLIP: a model that turns pictures and text into
//! vectors with the same meaning, so "kid wearing a red backpack" can be
//! matched against alert pictures without a fixed list of labels.
//!
//! Model: OpenAI CLIP ViT-B/32 as ONNX (the copies hosted by Jina's
//! clip-as-service), run with tract on the CPU (≈0.1 s per picture on a
//! 4-core PC). It is loaded only while it is being used and unloaded after
//! a few idle minutes, so it costs no memory the rest of the time.

use crate::imgutil::Frame;
use anyhow::{Context, Result, anyhow};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::{info, warn};

const BASE: &str = "https://clip-as-service.s3.us-east-2.amazonaws.com/models/onnx";
/// Smart-search models: (name, description). Bigger = more accurate, slower.
pub const MODELS: &[(&str, &str)] = &[
    (
        "ViT-B-32",
        "Standard — about 0.1 s per alert on a 4-core PC, 600 MB download",
    ),
    (
        "ViT-B-16",
        "Detailed — about 0.4 s per alert, 600 MB download",
    ),
    (
        "ViT-L-14",
        "Most accurate — about 2 s per alert, 1.7 GB download; for powerful PCs",
    ),
];
const VOCAB_URL: &str =
    "https://raw.githubusercontent.com/openai/CLIP/main/clip/bpe_simple_vocab_16e6.txt.gz";
const CONTEXT: usize = 77;
pub const DIM: usize = 512;
const IDLE_UNLOAD: Duration = Duration::from_secs(600);

/// Descriptions of "nothing of interest". A search matches an alert picture
/// only if it fits better than these do (CLIP gives every picture some
/// similarity to everything, e.g. a lawn scores a little for "a dog").
pub const NEUTRAL: &[&str] = &[
    "an empty driveway",
    "an empty yard with grass",
    "a garden with plants and trees",
    "an empty street",
    "a wall",
    "a fence",
    "a dark night scene",
];

type Plan = Arc<tract_onnx::prelude::TypedSimplePlan>;

struct Loaded {
    visual: Option<Plan>,
    textual: Option<Plan>,
    tokenizer: Option<Arc<Tokenizer>>,
    used: Instant,
}

#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct ClipStatus {
    pub downloading: bool,
    pub download_percent: Option<f32>,
    pub error: Option<String>,
    pub indexed: usize,
}

pub struct Clip {
    dir: PathBuf,
    /// Which CLIP model (see MODELS).
    model: Mutex<String>,
    loaded: Mutex<Loaded>,
    /// One download / load at a time.
    gate: tokio::sync::Mutex<()>,
    pub status: Mutex<ClipStatus>,
    text_cache: Mutex<HashMap<String, Vec<f32>>>,
}

impl Clip {
    pub fn new(data_dir: &Path) -> Arc<Self> {
        Arc::new(Self {
            dir: data_dir.join("models"),
            model: Mutex::new("ViT-B-32".into()),
            loaded: Mutex::new(Loaded {
                visual: None,
                textual: None,
                tokenizer: None,
                used: Instant::now(),
            }),
            gate: tokio::sync::Mutex::new(()),
            status: Mutex::new(ClipStatus::default()),
            text_cache: Mutex::new(HashMap::new()),
        })
    }

    fn path(&self, name: &str) -> PathBuf {
        // "ViT-B-32" → "vit-b32" (the standard model keeps its old file names).
        let slug = self
            .model()
            .to_lowercase()
            .replace('-', "")
            .replace("vit", "vit-");
        self.dir.join(format!("clip-{slug}-{name}"))
    }

    pub fn model(&self) -> String {
        self.model.lock().unwrap().clone()
    }

    /// Switch model (unloads the old one; searches use its own index).
    pub fn set_model(&self, name: &str) {
        let name = if MODELS.iter().any(|m| m.0 == name) {
            name
        } else {
            "ViT-B-32"
        };
        let mut m = self.model.lock().unwrap();
        if *m != name {
            *m = name.to_string();
            let mut l = self.loaded.lock().unwrap();
            l.visual = None;
            l.textual = None;
            self.text_cache.lock().unwrap().clear();
            info!("smart search: using the {name} model");
        }
    }

    /// The index file for the current model.
    pub fn index_file(&self) -> String {
        match self.model().as_str() {
            "ViT-B-32" => "smart.jsonl".into(),
            m => format!("smart-{m}.jsonl"),
        }
    }

    pub fn status(&self) -> ClipStatus {
        self.status.lock().unwrap().clone()
    }

    /// Free the models after a while without use.
    pub fn unload_if_idle(&self) {
        let mut l = self.loaded.lock().unwrap();
        if (l.visual.is_some() || l.textual.is_some()) && l.used.elapsed() > IDLE_UNLOAD {
            l.visual = None;
            l.textual = None;
            info!("smart search: model unloaded (idle)");
        }
    }

    async fn download(&self, url: &str, path: &Path) -> Result<()> {
        if path.exists() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.dir)?;
        info!("smart search: downloading {url}");
        self.status.lock().unwrap().downloading = true;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(1800))
            .build()?;
        let mut resp = client.get(url).send().await?.error_for_status()?;
        let total = resp.content_length();
        let tmp = path.with_extension("part");
        let mut file = tokio::fs::File::create(&tmp).await?;
        let mut got = 0u64;
        while let Some(chunk) = resp.chunk().await? {
            tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
            got += chunk.len() as u64;
            if let Some(t) = total {
                self.status.lock().unwrap().download_percent = Some(got as f32 * 100.0 / t as f32);
            }
        }
        tokio::io::AsyncWriteExt::flush(&mut file).await?;
        drop(file);
        tokio::fs::rename(&tmp, path).await?;
        let mut s = self.status.lock().unwrap();
        s.downloading = false;
        s.download_percent = None;
        Ok(())
    }

    async fn ensure(&self, visual: bool) -> Result<(Option<Plan>, Option<Plan>, Arc<Tokenizer>)> {
        {
            let mut l = self.loaded.lock().unwrap();
            if let Some(t) = l.tokenizer.clone()
                && (if visual {
                    l.visual.is_some()
                } else {
                    l.textual.is_some()
                })
            {
                l.used = Instant::now();
                return Ok((l.visual.clone(), l.textual.clone(), t));
            }
        }
        let _g = self.gate.lock().await;
        let res = async {
            // The word list is the same for every CLIP model.
            let vocab = self.dir.join("clip-vit-b32-vocab.txt.gz");
            self.download(VOCAB_URL, &vocab)
                .await
                .context("downloading the word list")?;
            let which = if visual { "visual" } else { "textual" };
            let model = self.path(&format!("{which}.onnx"));
            self.download(&format!("{BASE}/{}/{which}.onnx", self.model()), &model)
                .await
                .with_context(|| format!("downloading the smart-search model ({which})"))?;
            let need_tok = self.loaded.lock().unwrap().tokenizer.is_none();
            let (plan, tok) =
                tokio::task::spawn_blocking(move || -> Result<(Plan, Option<Tokenizer>)> {
                    let plan = load(&model, visual)?;
                    let tok = if need_tok {
                        Some(Tokenizer::from_gz(&std::fs::read(&vocab)?)?)
                    } else {
                        None
                    };
                    Ok((plan, tok))
                })
                .await??;
            let mut l = self.loaded.lock().unwrap();
            if visual {
                l.visual = Some(plan);
            } else {
                l.textual = Some(plan);
            }
            if let Some(t) = tok {
                l.tokenizer = Some(Arc::new(t));
            }
            l.used = Instant::now();
            info!("smart search: {which} model loaded");
            anyhow::Ok((
                l.visual.clone(),
                l.textual.clone(),
                l.tokenizer.clone().unwrap(),
            ))
        }
        .await;
        let mut s = self.status.lock().unwrap();
        s.downloading = false;
        match &res {
            Ok(_) => s.error = None,
            Err(e) => {
                warn!("smart search: {e:#}");
                s.error = Some(format!("{e:#}"));
            }
        }
        res
    }

    /// Vector for a picture (unit length).
    pub async fn embed_image(&self, f: &Frame) -> Result<Vec<f32>> {
        let (v, _, _) = self.ensure(true).await?;
        let v = v.ok_or_else(|| anyhow!("model not loaded"))?;
        let input = preprocess(f);
        tokio::task::spawn_blocking(move || run(&v, input)).await?
    }

    /// Vectors of the "nothing of interest" descriptions.
    pub async fn neutral(&self) -> Result<Vec<Vec<f32>>> {
        let mut out = Vec::with_capacity(NEUTRAL.len());
        for n in NEUTRAL {
            out.push(self.embed_text(n).await?);
        }
        Ok(out)
    }

    /// Vector for a search text (unit length), cached.
    pub async fn embed_text(&self, text: &str) -> Result<Vec<f32>> {
        let key = text.trim().to_lowercase();
        if let Some(v) = self.text_cache.lock().unwrap().get(&key) {
            return Ok(v.clone());
        }
        let (_, t, tok) = self.ensure(false).await?;
        let t = t.ok_or_else(|| anyhow!("model not loaded"))?;
        // "a photo of …" is how CLIP was trained to read descriptions.
        let ids = tok.encode(&format!("a photo of {key}"));
        let input: tract_onnx::prelude::Tensor =
            tract_onnx::prelude::tract_ndarray::Array2::from_shape_vec(
                (1, CONTEXT),
                ids.into_iter().map(|x| x as i64).collect(),
            )?
            .into();
        let v = tokio::task::spawn_blocking(move || run(&t, input)).await??;
        let mut c = self.text_cache.lock().unwrap();
        if c.len() > 200 {
            c.clear();
        }
        c.insert(key, v.clone());
        Ok(v)
    }
}

fn load(path: &Path, visual: bool) -> Result<Plan> {
    use tract_onnx::prelude::*;
    let m = tract_onnx::onnx()
        .model_for_path(path)
        .map_err(|e| anyhow!("{e}"))?;
    let m = if visual {
        m.with_input_fact(0, f32::fact([1, 3, 224, 224]).into())
    } else {
        m.with_input_fact(0, i64::fact([1, CONTEXT]).into())
    }
    .map_err(|e| anyhow!("{e}"))?;
    Ok(m.into_optimized()
        .map_err(|e| anyhow!("{e}"))?
        .into_runnable()
        .map_err(|e| anyhow!("{e}"))?)
}

fn run(plan: &Plan, input: tract_onnx::prelude::Tensor) -> Result<Vec<f32>> {
    use tract_onnx::prelude::*;
    let out = plan.run(tvec!(input.into())).map_err(|e| anyhow!("{e}"))?;
    let v: Vec<f32> = out[0]
        .to_plain_array_view::<f32>()
        .map_err(|e| anyhow!("{e}"))?
        .iter()
        .copied()
        .collect();
    Ok(normalize(v))
}

pub fn normalize(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
    v.iter_mut().for_each(|x| *x /= n);
    v
}

/// CLIP's picture input: shortest side to 224, centre crop, normalized.
fn preprocess(f: &Frame) -> tract_onnx::prelude::Tensor {
    const MEAN: [f32; 3] = [0.481_454_66, 0.457_827_5, 0.408_210_73];
    const STD: [f32; 3] = [0.268_629_54, 0.261_302_58, 0.275_777_1];
    let s = 224.0 / f.width.min(f.height).max(1) as f32;
    let (w, h) = (
        ((f.width as f32 * s).round() as u32).max(224),
        ((f.height as f32 * s).round() as u32).max(224),
    );
    let r = crate::imgutil::resize_rgb(&f.rgb, f.width, f.height, w, h);
    let (x0, y0) = ((w - 224) / 2, (h - 224) / 2);
    let mut data = vec![0f32; 3 * 224 * 224];
    for y in 0..224usize {
        for x in 0..224usize {
            let i = (((y0 as usize + y) * w as usize) + x0 as usize + x) * 3;
            for c in 0..3 {
                data[c * 224 * 224 + y * 224 + x] = (r[i + c] as f32 / 255.0 - MEAN[c]) / STD[c];
            }
        }
    }
    tract_onnx::prelude::Tensor::from_shape(&[1, 3, 224, 224], &data).unwrap()
}

// ------------------------------------------------------------- tokenizer

/// CLIP's byte-pair tokenizer.
pub struct Tokenizer {
    encoder: HashMap<String, u32>,
    ranks: HashMap<(String, String), usize>,
    byte_char: [char; 256],
}

impl Tokenizer {
    pub fn from_gz(gz: &[u8]) -> Result<Self> {
        use std::io::Read;
        let mut text = String::new();
        flate2::read::GzDecoder::new(gz).read_to_string(&mut text)?;
        Self::from_merges(&text)
    }

    pub fn from_merges(text: &str) -> Result<Self> {
        let (byte_char, order) = bytes_to_unicode();
        let merges: Vec<(String, String)> = text
            .lines()
            .skip(1)
            .take(49152 - 256 - 2)
            .filter_map(|l| {
                l.split_once(' ')
                    .map(|(a, b)| (a.to_string(), b.to_string()))
            })
            .collect();
        // The single characters come in CLIP's order (printable bytes first).
        let mut vocab: Vec<String> = order.iter().map(|c| c.to_string()).collect();
        vocab.extend(order.iter().map(|c| format!("{c}</w>")));
        vocab.extend(merges.iter().map(|(a, b)| format!("{a}{b}")));
        vocab.push("<|startoftext|>".into());
        vocab.push("<|endoftext|>".into());
        if vocab.len() != 49408 {
            anyhow::bail!("unexpected word list ({} entries)", vocab.len());
        }
        Ok(Self {
            encoder: vocab
                .into_iter()
                .enumerate()
                .map(|(i, v)| (v, i as u32))
                .collect(),
            ranks: merges
                .into_iter()
                .enumerate()
                .map(|(i, m)| (m, i))
                .collect(),
            byte_char,
        })
    }

    fn bpe(&self, token: &str) -> Vec<String> {
        let chars: Vec<char> = token.chars().collect();
        let mut word: Vec<String> = chars.iter().map(|c| c.to_string()).collect();
        if let Some(last) = word.last_mut() {
            last.push_str("</w>");
        }
        loop {
            let best = word
                .windows(2)
                .enumerate()
                .filter_map(|(i, p)| {
                    self.ranks
                        .get(&(p[0].clone(), p[1].clone()))
                        .map(|r| (*r, i))
                })
                .min();
            let Some((_, _)) = best else { break };
            let (a, b) = {
                let (_, i) = best.unwrap();
                (word[i].clone(), word[i + 1].clone())
            };
            let mut out = Vec::with_capacity(word.len());
            let mut i = 0;
            while i < word.len() {
                if i + 1 < word.len() && word[i] == a && word[i + 1] == b {
                    out.push(format!("{a}{b}"));
                    i += 2;
                } else {
                    out.push(word[i].clone());
                    i += 1;
                }
            }
            word = out;
            if word.len() == 1 {
                break;
            }
        }
        word
    }

    /// Token ids with start/end markers, padded to 77.
    pub fn encode(&self, text: &str) -> Vec<u32> {
        let text = text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        let mut ids = vec![49406];
        for piece in pre_tokenize(&text) {
            let mapped: String = piece.bytes().map(|b| self.byte_char[b as usize]).collect();
            for t in self.bpe(&mapped) {
                if let Some(id) = self.encoder.get(&t) {
                    ids.push(*id);
                }
            }
        }
        ids.truncate(CONTEXT - 1);
        ids.push(49407);
        ids.resize(CONTEXT, 0);
        ids
    }
}

/// Split text like CLIP's pattern: contractions, letter runs, single digits,
/// runs of other symbols.
fn pre_tokenize(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '\'' {
            let rest: String = chars[i + 1..chars.len().min(i + 3)].iter().collect();
            if let Some(k) = ["re", "ve", "ll", "s", "t", "m", "d"]
                .iter()
                .find(|k| rest.starts_with(**k))
            {
                out.push(format!("'{k}"));
                i += 1 + k.len();
                continue;
            }
        }
        if c.is_alphabetic() {
            let s = i;
            while i < chars.len() && chars[i].is_alphabetic() {
                i += 1;
            }
            out.push(chars[s..i].iter().collect());
        } else if c.is_numeric() {
            out.push(c.to_string());
            i += 1;
        } else {
            let s = i;
            while i < chars.len() && !chars[i].is_whitespace() && !chars[i].is_alphanumeric() {
                i += 1;
                if chars.get(i) == Some(&'\'') {
                    break;
                }
            }
            if i == s {
                i += 1;
            }
            out.push(chars[s..i].iter().collect());
        }
    }
    out
}

/// (byte → character, the characters in CLIP's vocabulary order).
fn bytes_to_unicode() -> ([char; 256], Vec<char>) {
    let mut bs: Vec<u32> = (b'!' as u32..=b'~' as u32)
        .chain(0xA1..=0xAC)
        .chain(0xAE..=0xFF)
        .collect();
    let mut cs = bs.clone();
    let mut n = 0;
    for b in 0..256u32 {
        if !bs.contains(&b) {
            bs.push(b);
            cs.push(256 + n);
            n += 1;
        }
    }
    let mut map = ['\0'; 256];
    let mut order = Vec::with_capacity(256);
    for (b, c) in bs.iter().zip(cs) {
        let ch = char::from_u32(c).unwrap();
        map[*b as usize] = ch;
        order.push(ch);
    }
    (map, order)
}

// ------------------------------------------------------------- storage

/// A vector stored compactly: 512 signed bytes, base64.
pub fn pack(v: &[f32]) -> String {
    use base64::Engine;
    let b: Vec<u8> = v
        .iter()
        .map(|x| ((x * 127.0).round().clamp(-127.0, 127.0) as i8) as u8)
        .collect();
    base64::engine::general_purpose::STANDARD.encode(b)
}

pub fn unpack(s: &str) -> Option<Vec<i8>> {
    use base64::Engine;
    let b = base64::engine::general_purpose::STANDARD.decode(s).ok()?;
    (b.len() == DIM || b.len() == 768).then(|| b.into_iter().map(|x| x as i8).collect())
}

/// Similarity of a text vector and a stored picture vector (−1..1).
pub fn similarity(q: &[f32], v: &[i8]) -> f32 {
    q.iter().zip(v).map(|(a, b)| a * *b as f32).sum::<f32>() / 127.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_like_clip() {
        assert_eq!(
            pre_tokenize("a kid's red backpack, 2 dogs!"),
            ["a", "kid", "'s", "red", "backpack", ",", "2", "dogs", "!"]
        );
        let v = normalize(vec![3.0, 4.0]);
        assert!((v[0] - 0.6).abs() < 1e-6);
        let q = normalize((0..DIM).map(|i| (i as f32).sin()).collect());
        let back = unpack(&pack(&q)).unwrap();
        assert!(similarity(&q, &back) > 0.99);
    }

    /// Needs the models: NVR_TEST_CLIP_DIR with visual.onnx, textual.onnx,
    /// bpe.txt.gz and NVR_TEST_AI_DIR with zidane.jpg, bus.jpg, dog.jpg.
    #[test]
    fn finds_pictures_by_description() {
        let (Ok(cd), Ok(ad)) = (
            std::env::var("NVR_TEST_CLIP_DIR"),
            std::env::var("NVR_TEST_AI_DIR"),
        ) else {
            return;
        };
        let (cd, ad) = (PathBuf::from(cd), PathBuf::from(ad));
        let tok = Tokenizer::from_gz(&std::fs::read(cd.join("bpe.txt.gz")).unwrap()).unwrap();
        let ids = tok.encode("a photo of a red van");
        println!("tokens: {:?}", &ids[..9]);
        // "a", "photo", "of" have well-known ids; then the end marker and padding.
        assert_eq!(&ids[..5], &[49406, 320, 1125, 539, 320]);
        assert_eq!(ids[7], 49407);
        assert_eq!(ids.len(), CONTEXT);
        let vis = load(&cd.join("visual.onnx"), true).unwrap();
        let txt = load(&cd.join("textual.onnx"), false).unwrap();
        let pics: Vec<(&str, Vec<f32>)> = ["zidane", "bus", "dog"]
            .iter()
            .map(|n| {
                let f = crate::imgutil::decode_jpeg(
                    &std::fs::read(ad.join(format!("{n}.jpg"))).unwrap(),
                )
                .unwrap();
                (*n, run(&vis, preprocess(&f)).unwrap())
            })
            .collect();
        for (q, want) in [
            ("two men in suits", "zidane"),
            ("a man wearing a tie", "zidane"),
            ("a blue bus", "bus"),
            ("people waiting at a bus stop", "bus"),
            ("a dog", "dog"),
            ("a bicycle", "dog"),
        ] {
            let ids = tok.encode(&format!("a photo of {q}"));
            let input: tract_onnx::prelude::Tensor =
                tract_onnx::prelude::tract_ndarray::Array2::from_shape_vec(
                    (1, CONTEXT),
                    ids.into_iter().map(|x| x as i64).collect(),
                )
                .unwrap()
                .into();
            let t = run(&txt, input).unwrap();
            let scores: Vec<(&str, f32)> = pics
                .iter()
                .map(|(n, v)| (*n, t.iter().zip(v).map(|(a, b)| a * b).sum()))
                .collect();
            println!("{q:30} {scores:?}");
            let best = scores.iter().max_by(|a, b| a.1.total_cmp(&b.1)).unwrap().0;
            assert_eq!(best, want, "{q}");
        }
        // A plain green frame and a noisy grass-like frame must not match objects.
        let mut seed = 1u32;
        let noise: Vec<u8> = (0..320 * 180)
            .flat_map(|_| {
                seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
                let v = ((seed >> 16) % 60) as u8;
                [30 + v / 2, 90 + v, 30 + v / 3]
            })
            .collect();
        let empties = [
            (
                "green",
                Frame {
                    width: 320,
                    height: 180,
                    rgb: Arc::new([60u8, 80, 60].repeat(320 * 180)),
                },
            ),
            (
                "grass",
                Frame {
                    width: 320,
                    height: 180,
                    rgb: Arc::new(noise),
                },
            ),
        ];
        let empty_vecs: Vec<(&str, Vec<f32>)> = empties
            .iter()
            .map(|(n, f)| (*n, run(&vis, preprocess(f)).unwrap()))
            .collect();
        let txt_vec = |q: &str| {
            let ids = tok.encode(&format!("a photo of {q}"));
            let input: tract_onnx::prelude::Tensor =
                tract_onnx::prelude::tract_ndarray::Array2::from_shape_vec(
                    (1, CONTEXT),
                    ids.into_iter().map(|x| x as i64).collect(),
                )
                .unwrap()
                .into();
            run(&txt, input).unwrap()
        };
        let neutral: Vec<Vec<f32>> = NEUTRAL.iter().map(|q| txt_vec(q)).collect();
        let all: Vec<(&str, Vec<f32>)> = pics.iter().cloned().chain(empty_vecs).collect();
        for q in [
            "a dog",
            "a person",
            "a red car",
            "two men in suits",
            "a bus",
            "a cat",
        ] {
            let t = txt_vec(q);
            let line: Vec<String> = all
                .iter()
                .map(|(n, v)| {
                    let s: f32 = t.iter().zip(v).map(|(a, b)| a * b).sum();
                    let base = neutral
                        .iter()
                        .map(|nv| nv.iter().zip(v).map(|(a, b)| a * b).sum::<f32>())
                        .fold(f32::MIN, f32::max);
                    format!("{n} {s:.3}/{:+.3}", s - base)
                })
                .collect();
            println!("{q:18} {}", line.join("  "));
        }
    }
}
