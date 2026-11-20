//! Search by description: "a red car", "someone with a ladder", "a man in a
//! hoodie at night". CLIP (OpenAI, MIT licence) turns each alert's picture
//! and your words into numbers that are close when they mean the same, so
//! alerts can be found by what's in them without anyone labelling them.
//! Runs on this computer; the models (420 MB) download the first time.

use crate::sync::MutexExt;
use anyhow::{Context, Result, anyhow};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

/// CLIP ViT-B/32 (Xenova's ONNX export, this exact version; the words
/// model 8-bit):
/// (file, name in the models folder, SHA-256).
const REV: &str = "6b7c523703ae26e39be4b03774966f8571c7f523";
const FILES: [(&str, &str, &str); 3] = [
    // (The 8-bit picture model can't run on the processor here: full size.)
    ("onnx/vision_model.onnx", "clip-vision.onnx", "fd6e1402a588279d1723c7534d4bcba5bc0b14b47dfab0e46f8c47b8270d7d40"),
    ("onnx/text_model_quantized.onnx", "clip-text.onnx", "73baab855d406190da9faa498cfedf65f15cf309f4cc7385b7b032e6d08e5c3a"),
    ("tokenizer.json", "clip-tokenizer.json", "f7f3b7af117d467b58374797691a6438d3e6b9e9cef800dfd5dced7f697a90cd"),
];
const SIZE: usize = 224;
/// Words are cut to this many pieces (CLIP's limit).
const TOKENS: usize = 77;
const START: i64 = 49406;
const END: i64 = 49407;

// ---------- words → numbers (CLIP's byte-pair tokenizer) ----------

struct Bpe {
    vocab: HashMap<String, i64>,
    ranks: HashMap<(String, String), usize>,
    bytes: [char; 256],
}

impl Bpe {
    fn load(path: &Path) -> Result<Self> {
        let j: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
        let m = &j["model"];
        let vocab = m["vocab"].as_object().context("no vocabulary")?.iter().filter_map(|(k, v)| Some((k.clone(), v.as_i64()?))).collect();
        let ranks = m["merges"]
            .as_array()
            .context("no merges")?
            .iter()
            .enumerate()
            .filter_map(|(i, x)| {
                let (a, b) = match x {
                    serde_json::Value::String(s) => s.split_once(' ')?,
                    serde_json::Value::Array(a) => (a.first()?.as_str()?, a.get(1)?.as_str()?),
                    _ => return None,
                };
                Some(((a.to_string(), b.to_string()), i))
            })
            .collect();
        Ok(Self { vocab, ranks, bytes: byte_chars() })
    }

    fn word(&self, w: &str) -> Vec<i64> {
        let mut parts: Vec<String> = w.bytes().map(|b| self.bytes[b as usize].to_string()).collect();
        if let Some(l) = parts.last_mut() {
            l.push_str("</w>");
        }
        loop {
            let best = (0..parts.len().saturating_sub(1)).filter_map(|i| self.ranks.get(&(parts[i].clone(), parts[i + 1].clone())).map(|r| (*r, i))).min();
            let Some((_, i)) = best else { break };
            let b = parts.remove(i + 1);
            parts[i].push_str(&b);
        }
        parts.iter().filter_map(|p| self.vocab.get(p).copied()).collect()
    }

    /// Start, the words' pieces, end; padded with "end" to 77.
    fn encode(&self, text: &str) -> Vec<i64> {
        let mut ids = vec![START];
        for w in split_words(&text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()) {
            ids.extend(self.word(&w));
        }
        ids.truncate(TOKENS - 1);
        ids.push(END);
        ids.resize(TOKENS, END);
        ids
    }
}

/// GPT-2's map from bytes to printable characters.
fn byte_chars() -> [char; 256] {
    let mut out = ['\0'; 256];
    let mut n = 0u32;
    for b in 0..256u32 {
        let printable = (33..=126).contains(&b) || (161..=172).contains(&b) || (174..=255).contains(&b);
        out[b as usize] = if printable {
            char::from_u32(b).unwrap()
        } else {
            n += 1;
            char::from_u32(255 + n).unwrap()
        };
    }
    out
}

/// CLIP's word split: runs of letters, single digits, runs of other
/// symbols, and English endings ('s, 't, 're, 've, 'm, 'll, 'd) on their own.
fn split_words(t: &str) -> Vec<String> {
    let c: Vec<char> = t.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
            continue;
        }
        if ch == '\'' {
            let rest: String = c[i + 1..c.len().min(i + 3)].iter().collect();
            if let Some(e) =
                ["ll", "re", "ve"].iter().find(|e| rest.starts_with(**e)).map(|e| e.len()).or_else(|| ["s", "t", "m", "d"].iter().find(|e| rest.starts_with(**e)).map(|_| 1))
            {
                out.push(c[i..=i + e].iter().collect());
                i += e + 1;
                continue;
            }
        }
        let start = i;
        if ch.is_alphabetic() {
            while i < c.len() && c[i].is_alphabetic() {
                i += 1;
            }
        } else if ch.is_numeric() {
            i += 1;
        } else {
            while i < c.len() && !c[i].is_whitespace() && !c[i].is_alphanumeric() {
                i += 1;
            }
        }
        out.push(c[start..i].iter().collect());
    }
    out
}

// ---------- the models ----------

pub struct Clip {
    vision: crate::infer::Net,
    text: crate::infer::Net,
    bpe: Bpe,
}

fn load_net(path: &Path, what: &str, int: bool) -> Result<crate::infer::Net> {
    crate::infer::load(path, what, || {
        use tract_onnx::prelude::*;
        let m = tract_onnx::onnx().model_for_path(path).map_err(|e| anyhow!("{e}"))?;
        let m = if int { m.with_input_fact(0, i64::fact([1, TOKENS]).into()) } else { m.with_input_fact(0, f32::fact([1, 3, SIZE, SIZE]).into()) }.map_err(|e| anyhow!("{e}"))?;
        m.into_optimized().map_err(|e| anyhow!("{e}"))?.into_runnable().map_err(|e| anyhow!("{e}"))
    })
}

fn unit(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
    v.iter_mut().for_each(|x| *x /= n);
    v
}

impl Clip {
    pub fn load(dir: &Path) -> Result<Self> {
        Ok(Self {
            vision: load_net(&dir.join(FILES[0].1), "description search (pictures)", false)?,
            text: load_net(&dir.join(FILES[1].1), "description search (words)", true)?,
            bpe: Bpe::load(&dir.join(FILES[2].1))?,
        })
    }

    /// A picture's meaning (512 numbers, length 1).
    pub fn picture(&self, f: &crate::imgutil::Frame) -> Result<Vec<f32>> {
        // Shortest side to 224, the middle 224×224, CLIP's colour scaling.
        let s = SIZE as f32 / f.width.min(f.height).max(1) as f32;
        let (w, h) = (((f.width as f32 * s).round() as u32).max(SIZE as u32), ((f.height as f32 * s).round() as u32).max(SIZE as u32));
        let r = f.resize(w, h);
        let (x0, y0) = ((w as usize - SIZE) / 2, (h as usize - SIZE) / 2);
        const MEAN: [f32; 3] = [0.481_454_66, 0.457_827_5, 0.408_210_73];
        const STD: [f32; 3] = [0.268_629_54, 0.261_302_6, 0.275_777_1];
        let mut data = vec![0f32; 3 * SIZE * SIZE];
        for y in 0..SIZE {
            for x in 0..SIZE {
                let p = ((y0 + y) * w as usize + x0 + x) * 3;
                for c in 0..3 {
                    data[c * SIZE * SIZE + y * SIZE + x] = (r.rgb[p + c] as f32 / 255.0 - MEAN[c]) / STD[c];
                }
            }
        }
        let out = self.vision.run(crate::infer::Input::f32(&[1, 3, SIZE, SIZE], data))?;
        Ok(unit(out.into_iter().next().context("no output")?))
    }

    /// A picture's meaning with the whole picture squeezed to a square (no
    /// edges cut off, as `picture` does): for learned states, where the
    /// piece of the scene that matters can be anywhere in the picture.
    pub fn region(&self, f: &crate::imgutil::Frame) -> Result<Vec<f32>> {
        const MEAN: [f32; 3] = [0.481_454_66, 0.457_827_5, 0.408_210_73];
        const STD: [f32; 3] = [0.268_629_54, 0.261_302_6, 0.275_777_1];
        let r = f.resize(SIZE as u32, SIZE as u32);
        let mut data = vec![0f32; 3 * SIZE * SIZE];
        for y in 0..SIZE {
            for x in 0..SIZE {
                let p = (y * SIZE + x) * 3;
                for c in 0..3 {
                    data[c * SIZE * SIZE + y * SIZE + x] = (r.rgb[p + c] as f32 / 255.0 - MEAN[c]) / STD[c];
                }
            }
        }
        let out = self.vision.run(crate::infer::Input::f32(&[1, 3, SIZE, SIZE], data))?;
        Ok(unit(out.into_iter().next().context("no output")?))
    }

    /// Words' meaning (512 numbers, length 1).
    pub fn words(&self, text: &str) -> Result<Vec<f32>> {
        let ids = self.bpe.encode(text);
        let out = self.text.run(crate::infer::Input::i64(&[1, TOKENS], ids))?;
        Ok(unit(out.into_iter().next().context("no output")?))
    }
}

/// The models, loaded once (downloaded the first time).
static MODEL: LazyLock<tokio::sync::Mutex<Option<Arc<Clip>>>> = LazyLock::new(Default::default);

pub async fn model(models_dir: &Path) -> Result<Arc<Clip>> {
    let mut m = MODEL.lock().await;
    if let Some(c) = m.as_ref()
        && !c.vision.misplaced()
    {
        return Ok(c.clone());
    }
    for (src, name, sum) in FILES {
        let p = models_dir.join(name);
        if !p.exists() {
            download(src, &p, sum).await?;
        }
    }
    let d: PathBuf = models_dir.to_path_buf();
    let c = Arc::new(tokio::task::spawn_blocking(move || Clip::load(&d)).await??);
    tracing::info!("description search: models loaded");
    *m = Some(c.clone());
    Ok(c)
}

/// Loaded already (searching right away doesn't wait for a download)?
pub fn ready(models_dir: &Path) -> bool {
    FILES.iter().all(|f| models_dir.join(f.1).exists())
}

async fn download(src: &str, path: &Path, sum: &str) -> Result<()> {
    let url = format!("https://huggingface.co/Xenova/clip-vit-base-patch32/resolve/{REV}/{src}");
    tracing::info!("description search: downloading {src}");
    std::fs::create_dir_all(path.parent().context("no folder")?)?;
    let job = crate::jobs::start("download", "Downloading the description search model");
    let mut resp = crate::jobs::download_client()?.get(&url).send().await?.error_for_status()?;
    let total = resp.content_length().unwrap_or(0);
    let mut bytes = Vec::with_capacity(total as usize);
    while let Some(c) = resp.chunk().await? {
        bytes.extend_from_slice(&c);
        job.bytes(bytes.len() as u64, total);
    }
    use sha2::{Digest, Sha256};
    let got: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    anyhow::ensure!(got == sum, "the downloaded file isn't the expected one ({src})");
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

// ---------- alerts: index and search ----------

/// Description search, for Settings → Object recognition and the Alerts page.
#[derive(Clone, Default, serde::Serialize)]
pub struct Status {
    pub ready: bool,
    pub downloading: bool,
    /// Alert pictures still to look at.
    pub waiting: usize,
    pub error: Option<String>,
}

pub static STATUS: LazyLock<std::sync::Mutex<Status>> = LazyLock::new(Default::default);

fn models_dir(mgr: &crate::manager::Manager) -> PathBuf {
    mgr.data_dir.join("models")
}

/// While description search is on: look at every alert picture (the last
/// 30 days, newest first; new alerts within a minute or so) once.
pub async fn run(mgr: Arc<crate::manager::Manager>) {
    use std::time::Duration;
    tokio::time::sleep(Duration::from_secs(20)).await;
    loop {
        if !mgr.config().ai.search {
            *STATUS.locked() = Status::default();
            tokio::time::sleep(Duration::from_secs(10)).await;
            continue;
        }
        STATUS.locked().downloading = !ready(&models_dir(&mgr));
        let c = match model(&models_dir(&mgr)).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("description search: {e:#}");
                *STATUS.locked() = Status { error: Some(format!("{e:#}")), ..Default::default() };
                tokio::time::sleep(Duration::from_secs(300)).await;
                continue;
            }
        };
        let now = chrono::Local::now();
        let mut todo = Vec::new();
        for rt in mgr.cameras() {
            for e in rt.events.between(now - chrono::Duration::days(30), now - chrono::Duration::seconds(60)) {
                if let Some(t) = &e.thumb
                    && !e.meaning.as_ref().is_some_and(|m| &m.thumb == t && m.ver >= crate::events::MEANING_VERSION)
                {
                    todo.push((rt.clone(), e.id.clone(), t.clone(), e.start));
                }
            }
        }
        todo.sort_by_key(|x| std::cmp::Reverse(x.3));
        {
            let mut s = STATUS.locked();
            *s = Status { ready: true, waiting: todo.len(), ..Default::default() };
        }
        let job = (todo.len() > 20).then(|| {
            let j = crate::jobs::start("ai", "Preparing description search for older alerts");
            j.total(todo.len() as u64);
            j
        });
        for (rt, id, thumb, _) in todo {
            if !mgr.config().ai.search {
                break;
            }
            crate::load::wait_for_cpu(85, Duration::from_secs(60)).await;
            let (c2, path) = (c.clone(), rt.events.dir().join(&thumb));
            let (ev, cam_name) = (rt.events.get(&id), rt.cfg.name.clone());
            let t2 = thumb.clone();
            let v = tokio::task::spawn_blocking(move || -> Result<crate::events::Meaning> { meaning_of(&c2, &path, t2, ev.as_ref(), &cam_name) }).await;
            match v {
                Ok(Ok(m)) => rt.events.set_meaning(&id, m),
                Ok(Err(e)) => tracing::debug!("description search: {id}: {e:#}"),
                Err(_) => {}
            }
            if let Some(j) = &job {
                j.done_one();
            }
            let mut s = STATUS.locked();
            s.waiting = s.waiting.saturating_sub(1);
        }
        drop(job);
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}

/// What an alert picture means: the picture as a whole, a close-up of each
/// thing found in it (or both sides of it, which the whole-picture reading
/// cuts off), and words for what it shows.
fn meaning_of(c: &Clip, path: &Path, thumb: String, e: Option<&crate::events::Event>, camera: &str) -> Result<crate::events::Meaning> {
    use crate::events::{MEANING_VERSION, Meaning, MeaningPart};
    let frame = crate::imgutil::decode_jpeg(&std::fs::read(path)?)?;
    let v = c.picture(&frame)?;
    let (fw, fh) = (frame.width as f32, frame.height as f32);
    let mut parts: Vec<MeaningPart> = Vec::new();
    let mut crops: Vec<(String, crate::imgutil::Frame)> = Vec::new();
    if let Some(e) = e {
        let mut boxes: Vec<&crate::events::SeenBox> = e.boxes.iter().filter(|b| b.score >= 0.4 && b.label != "motion" && b.w > 0.0 && b.h > 0.0).collect();
        boxes.sort_by(|a, b| (b.w * b.h).total_cmp(&(a.w * a.h)));
        for b in boxes.into_iter().take(4) {
            // A little around it, so it is seen in its place.
            let (px, py) = (b.w * 0.12, b.h * 0.12);
            let (x, y, w, h) = (((b.x - px) * fw).max(0.0), ((b.y - py) * fh).max(0.0), (b.w + 2.0 * px) * fw, (b.h + 2.0 * py) * fh);
            if w >= 24.0 && h >= 24.0 {
                crops.push((crate::ai::canon(&b.label).to_string(), crate::recog::crop(&frame, x, y, w, h)));
            }
        }
    }
    if crops.is_empty() {
        crops.push(("left".into(), crate::recog::crop(&frame, 0.0, 0.0, fw * 0.45, fh)));
        crops.push(("right".into(), crate::recog::crop(&frame, fw * 0.55, 0.0, fw * 0.45, fh)));
    }
    for (label, crop) in crops {
        if let Ok(p) = c.region(&crop) {
            parts.push(MeaningPart { label, v: crate::known::pack(&p) });
        }
    }
    let d = e.map(|e| crate::describe::describe(e, &frame, camera)).unwrap_or_default();
    Ok(Meaning { thumb, v: crate::known::pack(&v), ver: MEANING_VERSION, parts, words: d.words, text: d.text })
}

/// Plain descriptions any picture fits a little: a picture is a match when
/// it fits the words looked for better than all of these (CLIP's raw
/// closeness is small and much the same for every picture, so it's only
/// meaningful compared with something).
const NEUTRAL: [&str; 4] = ["a photo of something", "a photo of a place", "a security camera photo", "a blank picture"];

/// The picture's judgement (`share`, `clear`) adjusted by the plain facts of
/// the question: `known` of its words are facts the description knows (red,
/// vehicle, night …), `hit` of them are in the alert's description, `other`
/// are not facts at all (“carrying a box”: only the picture can tell).
fn with_words(share: f32, clear: bool, known: usize, hit: usize, other: usize) -> (f32, bool) {
    if known == 0 {
        return (share, clear);
    }
    if hit == known && other == 0 {
        // Everything asked is a plain fact and the alert has all of it.
        (share.max(0.6), true)
    } else if hit == known {
        // The facts fit; the rest is for the picture.
        ((share * 1.5).min(0.99), clear)
    } else if hit == 0 && other == 0 {
        // A word that matters is nowhere in it.
        (share * 0.3, false)
    } else {
        (share * (0.6 + 0.4 * hit as f32 / known as f32), clear)
    }
}

/// One alert that matches a description.
pub struct Hit {
    pub camera: String,
    pub id: String,
    /// How sure, 0 to 1.
    pub score: f32,
    /// The words of its description that fit the question (red, vehicle, night …).
    pub why: Vec<String>,
    /// What the alert shows, as a sentence.
    pub text: String,
}

/// Alerts that match `query`, best first, and how many were looked at. With
/// no clear match, the closest few (`closest` = true). Only cameras in
/// `allowed` (None: all).
///
/// A match is judged three ways, the best counting: the alert picture as a
/// whole, a close-up of each thing found in it (and both sides of the
/// picture), and the words describing it (`describe.rs`: colours, vehicle,
/// person, night …), which settle the parts of a question that are plain
/// facts, leaving the rest (“carrying a box”) to the pictures.
pub async fn search(mgr: &crate::manager::Manager, allowed: Option<&[String]>, query: &str, days: i64) -> Result<(Vec<Hit>, usize, bool)> {
    let q = query.trim();
    anyhow::ensure!(!q.is_empty(), "type what you're looking for");
    anyhow::ensure!(ready(&models_dir(mgr)), "description search is still downloading its model");
    let c = model(&models_dir(mgr)).await?;
    // CLIP matches "a photo of …" best; two ways of saying it, averaged.
    let (t1, t2) = if q.to_lowercase().starts_with("a photo") { (q.to_string(), q.to_string()) } else { (format!("a photo of {q}"), format!("a security camera photo of {q}")) };
    let c2 = c.clone();
    let w = tokio::task::spawn_blocking(move || -> Result<Vec<f32>> {
        let (a, b) = (c2.words(&t1)?, c2.words(&t2)?);
        Ok(unit(a.iter().zip(&b).map(|(x, y)| x + y).collect()))
    })
    .await??;
    let neutral = {
        static N: tokio::sync::OnceCell<Vec<Vec<f32>>> = tokio::sync::OnceCell::const_new();
        let c3 = c.clone();
        N.get_or_try_init(|| async move { tokio::task::spawn_blocking(move || NEUTRAL.iter().map(|t| c3.words(t)).collect::<Result<Vec<_>>>()).await? }).await?.clone()
    };
    let dot = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
    // As CLIP decides between descriptions (×100, softmax): the share the
    // words get against the plain ones; clear when they beat every one.
    let judge = |v: &[f32]| -> (f32, bool, f32) {
        let q = 100.0 * dot(v, &w);
        let others: Vec<f32> = neutral.iter().map(|n| 100.0 * dot(v, n)).collect();
        let top = others.iter().copied().fold(q, f32::max);
        let share = (q - top).exp() / (std::iter::once(q).chain(others.iter().copied()).map(|x| (x - top).exp()).sum::<f32>());
        (share, others.iter().all(|&o| q > o) && share >= 0.3, q)
    };
    let now = chrono::Local::now();
    let mut hits: Vec<(Hit, bool, f32)> = Vec::new();
    let mut looked = 0;
    for rt in mgr.cameras() {
        if allowed.is_some_and(|a| !a.contains(&rt.cfg.id)) {
            continue;
        }
        for e in rt.events.between(now - chrono::Duration::days(days.clamp(1, 3650)), now) {
            let Some(m) = &e.meaning else { continue };
            looked += 1;
            let mut best = judge(&crate::known::unpack(&m.v));
            for p in &m.parts {
                let j = judge(&crate::known::unpack(&p.v));
                if j.0 > best.0 {
                    best = j;
                }
            }
            let (mut share, mut clear, qv) = best;
            let mut why = Vec::new();
            if !m.words.is_empty() {
                let (known, hit, other, matched) = crate::describe::match_terms(q, &m.words);
                why = matched;
                (share, clear) = with_words(share, clear, known, hit, other);
            }
            hits.push((Hit { camera: rt.cfg.id.clone(), id: e.id.clone(), score: share, why, text: m.text.clone() }, clear, qv));
        }
    }
    let closest = !hits.iter().any(|h| h.1);
    if closest {
        // Nothing clearly: the few that come nearest.
        hits.sort_by(|a, b| b.2.total_cmp(&a.2));
        hits.truncate(12);
    } else {
        hits.retain(|h| h.1);
        hits.sort_by(|a, b| b.0.score.total_cmp(&a.0.score));
        hits.truncate(500);
    }
    Ok((hits.into_iter().map(|h| h.0).collect(), looked, closest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plain_facts_of_a_question_settle_what_the_picture_cannot() {
        // “red car”: both facts in the alert's words: a match, however the picture reads.
        assert_eq!(with_words(0.1, false, 2, 2, 0), (0.6, true));
        // “white car” where it is red: not there at all, however small the doubt.
        let (s, c) = with_words(0.5, true, 2, 1, 0);
        assert!(s < 0.5 && c, "half of it fits: {s}");
        let (s, c) = with_words(0.5, true, 1, 0, 0);
        assert!(s < 0.2 && !c, "the word isn't there: {s}");
        // “someone carrying a box”: the person fits, the box is the picture's to judge.
        let (s, c) = with_words(0.4, true, 1, 1, 2);
        assert!(s > 0.4 && c);
        // Nothing the descriptions know: unchanged.
        assert_eq!(with_words(0.4, true, 0, 0, 3), (0.4, true));
    }

    #[test]
    fn splits_words_like_clip() {
        assert_eq!(split_words("a man's red car, 2 dogs!!"), ["a", "man", "'s", "red", "car", ",", "2", "dogs", "!!"]);
    }

    /// With the models in $CLIP_DIR (not downloaded in tests): words are
    /// tokenized as CLIP does, and a picture is closest to its description.
    #[test]
    fn pictures_and_words_meet() {
        let Ok(d) = std::env::var("CLIP_DIR") else { return };
        let c = Clip::load(Path::new(&d)).unwrap();
        // From the reference tokenizer: "a photo of a cat".
        assert_eq!(&c.bpe.encode("a photo of a cat")[..7], &[49406, 320, 1125, 539, 320, 2368, 49407]);
        let f = crate::imgutil::decode_jpeg(&std::fs::read(std::env::var("CLIP_PIC").unwrap()).unwrap()).unwrap();
        let p = c.picture(&f).unwrap();
        let score = |t: &str| c.words(t).unwrap().iter().zip(&p).map(|(a, b)| a * b).sum::<f32>();
        let (good, bad) = (score(&std::env::var("CLIP_GOOD").unwrap()), score("a bowl of fruit on a table"));
        assert!(good > bad + 0.03, "{good} vs {bad}");
    }
}
