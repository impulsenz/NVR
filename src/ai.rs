//! Object recognition. Providers:
//! - builtin:   YOLOX model run on this computer's CPU (downloaded on first use)
//! - deepstack: CodeProject.AI Server / DeepStack on this or another computer
//! - ollama:    a vision model (e.g. llava, qwen2.5vl) served by Ollama on your network
//! - gemini:    Google Gemini API (has a free tier)
//! - openai:    any OpenAI-compatible API (OpenAI, OpenRouter, LM Studio, vLLM, ...)

use crate::config::AiConfig;
use crate::imgutil::Frame;
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
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
    /// Short description from a language model ("white delivery van").
    pub desc: String,
}

#[derive(Debug)]
pub enum AiError {
    /// Skipped because of the requests-per-minute limit (not an error).
    RateLimited,
    /// Built-in model not ready yet.
    NotReady(String),
    Failed(anyhow::Error),
}

impl std::fmt::Display for AiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AiError::RateLimited => write!(f, "skipped (requests-per-minute limit)"),
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
    client: reqwest::Client,
    model: Mutex<Option<Arc<Yolox>>>,
    /// Bigger model that double-checks possible alerts (optional).
    verifier: Mutex<Option<Arc<Yolox>>>,
    status: Mutex<AiStatus>,
    recent: Mutex<VecDeque<Instant>>,
    /// Gemini model picked automatically (when none is set).
    gemini_auto: Mutex<Option<String>>,
    busy: tokio::sync::Semaphore,
}

impl Ai {
    fn new(cfg: AiConfig, data_dir: &Path) -> Arc<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(cfg.timeout_seconds.max(1)))
            .build()
            .unwrap_or_default();
        let status = AiStatus {
            provider: cfg.provider.clone(),
            ready: cfg.provider != "builtin",
            ..Default::default()
        };
        let ai = Arc::new(Self {
            // The built-in AI runs on this computer's processor: at most a
            // quarter of its cores at once, so recording and live view keep
            // their share.
            busy: tokio::sync::Semaphore::new(if cfg.provider == "builtin" {
                std::thread::available_parallelism()
                    .map(|n| n.get() / 4)
                    .unwrap_or(1)
                    .clamp(1, 4)
            } else {
                4
            }),
            cfg,
            models_dir: data_dir.join("models"),
            client,
            model: Mutex::new(None),
            verifier: Mutex::new(None),
            status: Mutex::new(status),
            recent: Mutex::new(VecDeque::new()),
            gemini_auto: Mutex::new(None),
        });
        if ai.cfg.provider == "builtin" {
            ai.clone().prepare_builtin();
        }
        ai
    }

    pub fn enabled(&self) -> bool {
        self.cfg.provider != "none"
    }

    /// Runs on this computer's processor (not a server or cloud service).
    pub fn is_local(&self) -> bool {
        self.cfg.provider == "builtin"
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
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(600))
            .build()?;
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

    fn take_rate_slot(&self) -> bool {
        if self.cfg.max_per_minute == 0 {
            return true;
        }
        let mut r = self.recent.lock().unwrap();
        while r
            .front()
            .is_some_and(|t| t.elapsed() > Duration::from_secs(60))
        {
            r.pop_front();
        }
        if r.len() as u32 >= self.cfg.max_per_minute {
            return false;
        }
        r.push_back(Instant::now());
        true
    }

    /// Detect objects in a frame. Results are unfiltered (callers apply
    /// their own object list and confidence).
    pub async fn detect(&self, frame: &Frame) -> Result<Vec<Detection>, AiError> {
        if !self.enabled() {
            return Ok(vec![]);
        }
        if !self.take_rate_slot() {
            return Err(AiError::RateLimited);
        }
        let _permit = self
            .busy
            .acquire()
            .await
            .map_err(|e| AiError::Failed(e.into()))?;
        let t = Instant::now();
        let res = match self.cfg.provider.as_str() {
            "builtin" => self.detect_builtin(frame).await,
            "deepstack" => self.detect_deepstack(frame).await.map_err(AiError::from),
            "ollama" => self.detect_ollama(frame).await.map_err(AiError::from),
            "gemini" => self.detect_gemini(frame).await.map_err(AiError::from),
            "openai" => self.detect_openai(frame).await.map_err(AiError::from),
            p => Err(AiError::Failed(anyhow!("unknown provider {p}"))),
        };
        let mut s = self.status.lock().unwrap();
        s.requests += 1;
        s.last_ms = Some(t.elapsed().as_millis() as u64);
        match &res {
            Err(AiError::Failed(e)) => {
                s.failures += 1;
                s.error = Some(format!("{e:#}"));
            }
            Ok(_) if self.cfg.provider != "builtin" => s.error = None,
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

    fn jpeg_b64(frame: &Frame) -> Result<String> {
        let small = frame.fit(1024);
        Ok(base64::engine::general_purpose::STANDARD.encode(small.jpeg(85)?))
    }

    fn base_url(&self) -> String {
        let u = self.cfg.url.trim().trim_end_matches('/').to_string();
        if u.contains("://") {
            u
        } else {
            format!("http://{u}")
        }
    }

    async fn detect_deepstack(&self, frame: &Frame) -> Result<Vec<Detection>> {
        let jpg = frame.jpeg(90)?;
        let part = reqwest::multipart::Part::bytes(jpg)
            .file_name("frame.jpg")
            .mime_str("image/jpeg")?;
        let form = reqwest::multipart::Form::new()
            .part("image", part)
            .text("min_confidence", "0.3");
        let url = format!("{}/v1/vision/detection", self.base_url());
        let mut req = self.client.post(&url).multipart(form);
        if !self.cfg.api_key.is_empty() {
            req = req.query(&[("api_key", &self.cfg.api_key)]);
        }
        let v: Value = req
            .send()
            .await
            .with_context(|| format!("cannot reach {url}"))?
            .error_for_status()?
            .json()
            .await?;
        if v["success"] == json!(false) {
            bail!(
                "AI server error: {}",
                v["error"].as_str().unwrap_or("unknown")
            );
        }
        let (fw, fh) = (frame.width as f32, frame.height as f32);
        Ok(v["predictions"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|p| {
                        let n = |k: &str| p[k].as_f64().map(|x| x as f32);
                        let (x0, y0, x1, y1) = (n("x_min")?, n("y_min")?, n("x_max")?, n("y_max")?);
                        Some(Detection {
                            label: normalize_label(p["label"].as_str()?),
                            score: n("confidence")?,
                            x: x0 / fw,
                            y: y0 / fh,
                            w: (x1 - x0) / fw,
                            h: (y1 - y0) / fh,
                            desc: String::new(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn detect_ollama(&self, frame: &Frame) -> Result<Vec<Detection>> {
        let model = if self.cfg.model.is_empty() {
            "qwen2.5vl"
        } else {
            &self.cfg.model
        };
        let body = json!({
            "model": model,
            "stream": false,
            "format": "json",
            "options": {"temperature": 0},
            "messages": [{"role": "user", "content": llm_prompt(), "images": [Self::jpeg_b64(frame)?]}],
        });
        let url = format!("{}/api/chat", self.base_url());
        let v: Value = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("cannot reach {url}"))?
            .error_for_status()
            .context("Ollama error (is the model pulled? try: ollama pull qwen2.5vl)")?
            .json()
            .await?;
        parse_llm(v["message"]["content"].as_str().unwrap_or(""))
    }

    /// The Gemini model to use: the one set, or the newest available Flash
    /// model (Google retires old versions, so names are not hard-coded).
    async fn gemini_model(&self) -> Result<String> {
        if !self.cfg.model.trim().is_empty() {
            return Ok(self
                .cfg
                .model
                .trim()
                .trim_start_matches("models/")
                .to_string());
        }
        if let Some(m) = self.gemini_auto.lock().unwrap().clone() {
            return Ok(m);
        }
        let models = list_models(&self.client, &self.cfg)
            .await
            .unwrap_or_default();
        let m = pick_gemini(&models).unwrap_or_else(|| "gemini-flash-latest".into());
        info!("AI: using Gemini model {m}");
        *self.gemini_auto.lock().unwrap() = Some(m.clone());
        Ok(m)
    }

    async fn detect_gemini(&self, frame: &Frame) -> Result<Vec<Detection>> {
        let body = json!({
            "contents": [{"parts": [
                {"inline_data": {"mime_type": "image/jpeg", "data": Self::jpeg_b64(frame)?}},
                {"text": llm_prompt()},
            ]}],
            "generationConfig": {"responseMimeType": "application/json", "temperature": 0},
        });
        // If an automatically picked model has been retired, pick again once.
        for attempt in 0..2 {
            let model = self.gemini_model().await?;
            let url = format!(
                "https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent"
            );
            let resp = self
                .client
                .post(&url)
                .header("x-goog-api-key", self.cfg.api_key.trim())
                .json(&body)
                .send()
                .await
                .context("cannot reach Gemini")?;
            let status = resp.status();
            let v: Value = resp.json().await.unwrap_or(Value::Null);
            if status == reqwest::StatusCode::NOT_FOUND
                && self.cfg.model.trim().is_empty()
                && attempt == 0
            {
                warn!("AI: Gemini model {model} is not available; choosing another");
                *self.gemini_auto.lock().unwrap() = None;
                continue;
            }
            if !status.is_success() {
                let msg = v["error"]["message"].as_str().unwrap_or("request failed");
                if status == reqwest::StatusCode::NOT_FOUND {
                    bail!(
                        "Gemini model \"{model}\" is not available: {msg} — clear the model name to pick one automatically, or use “List models”."
                    );
                }
                bail!("Gemini {status}: {msg}");
            }
            let text: String = v["candidates"][0]["content"]["parts"]
                .as_array()
                .map(|p| p.iter().filter_map(|x| x["text"].as_str()).collect())
                .unwrap_or_default();
            return parse_llm(&text);
        }
        bail!("no Gemini model available for this API key")
    }

    async fn detect_openai(&self, frame: &Frame) -> Result<Vec<Detection>> {
        let model = if self.cfg.model.is_empty() {
            "gpt-4o-mini"
        } else {
            &self.cfg.model
        };
        let body = json!({
            "model": model,
            "temperature": 0,
            "messages": [{"role": "user", "content": [
                {"type": "text", "text": llm_prompt()},
                {"type": "image_url", "image_url": {"url": format!("data:image/jpeg;base64,{}", Self::jpeg_b64(frame)?)}},
            ]}],
        });
        let url = format!("{}/chat/completions", self.base_url());
        let mut req = self.client.post(&url).json(&body);
        if !self.cfg.api_key.is_empty() {
            req = req.bearer_auth(self.cfg.api_key.trim());
        }
        let resp = req
            .send()
            .await
            .with_context(|| format!("cannot reach {url}"))?;
        let status = resp.status();
        let v: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            bail!(
                "{status}: {}",
                v["error"]["message"].as_str().unwrap_or("request failed")
            );
        }
        parse_llm(v["choices"][0]["message"]["content"].as_str().unwrap_or(""))
    }
}

/// Model names offered by the configured provider (for the settings page).
pub async fn list_models(client: &reqwest::Client, cfg: &AiConfig) -> Result<Vec<String>> {
    let base = {
        let u = cfg.url.trim().trim_end_matches('/').to_string();
        if u.contains("://") {
            u
        } else {
            format!("http://{u}")
        }
    };
    match cfg.provider.as_str() {
        "gemini" => {
            let v: Value = client
                .get("https://generativelanguage.googleapis.com/v1beta/models?pageSize=1000")
                .header("x-goog-api-key", cfg.api_key.trim())
                .send()
                .await
                .context("cannot reach Gemini")?
                .error_for_status()
                .context("Gemini refused the request (check the API key)")?
                .json()
                .await?;
            Ok(v["models"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter(|m| {
                            m["supportedGenerationMethods"]
                                .as_array()
                                .is_some_and(|x| x.iter().any(|y| y == "generateContent"))
                        })
                        .filter_map(|m| m["name"].as_str())
                        .map(|n| n.trim_start_matches("models/").to_string())
                        .collect()
                })
                .unwrap_or_default())
        }
        "ollama" => {
            let v: Value = client
                .get(format!("{base}/api/tags"))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            Ok(v["models"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|m| m["name"].as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default())
        }
        "openai" => {
            let mut req = client.get(format!("{base}/models"));
            if !cfg.api_key.is_empty() {
                req = req.bearer_auth(cfg.api_key.trim());
            }
            let v: Value = req.send().await?.error_for_status()?.json().await?;
            Ok(v["data"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|m| m["id"].as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default())
        }
        _ => Ok(vec![]),
    }
}

/// Newest stable general-purpose Flash model, e.g. "gemini-3.8-flash".
pub fn pick_gemini(models: &[String]) -> Option<String> {
    let version = |n: &str| -> f32 {
        n.trim_start_matches("gemini-")
            .split('-')
            .next()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0)
    };
    let skip = [
        "lite",
        "image",
        "tts",
        "live",
        "audio",
        "embedding",
        "exp",
        "thinking",
        "native",
        "robotics",
        "computer",
    ];
    let mut c: Vec<&String> = models
        .iter()
        .filter(|m| {
            m.starts_with("gemini-") && m.contains("flash") && !skip.iter().any(|s| m.contains(s))
        })
        .collect();
    // Stable before preview (previews change and are retired quickly), then
    // the newest version, then the shortest name.
    c.sort_by(|a, b| {
        a.contains("preview")
            .cmp(&b.contains("preview"))
            .then(version(b).total_cmp(&version(a)))
            .then(a.len().cmp(&b.len()))
    });
    c.first().map(|s| s.to_string())
}

fn llm_prompt() -> String {
    format!(
        "You are the object detector of a home security camera. List every clearly visible object in the image \
         that is one of: {}. Reply with JSON only, in this form: \
         {{\"objects\": [{{\"label\": \"car\", \"confidence\": 0.9, \"box_2d\": [ymin, xmin, ymax, xmax], \
         \"color\": \"white\", \"description\": \"white delivery van\"}}]}} \
         where box_2d is scaled 0 to 1000, label is one of the words above, color is the object's main colour \
         in one plain word, and description is 2 to 6 words that would help someone search for it later \
         (colour, kind, clothing, what it carries). Reply {{\"objects\": []}} if there are none.",
        SECURITY_OBJECTS.join(", ")
    )
}

/// Map common synonyms onto COCO labels.
pub fn normalize_label(l: &str) -> String {
    let l = l.trim().to_lowercase();
    let l = l.trim_end_matches('s');
    match l {
        "people" | "pedestrian" | "man" | "woman" | "child" | "human" | "persons" => {
            "person".into()
        }
        "vehicle" | "automobile" | "suv" | "van" | "sedan" => "car".into(),
        "motorbike" | "scooter" => "motorcycle".into(),
        "bike" => "bicycle".into(),
        "puppy" => "dog".into(),
        "kitten" => "cat".into(),
        "lorry" | "pickup" | "pickup truck" => "truck".into(),
        "bu" => "bus".into(), // "bus" after trimming "s"
        other => {
            if COCO.contains(&other) {
                other.into()
            } else if COCO.contains(&format!("{other}s").as_str()) {
                format!("{other}s")
            } else {
                other.into()
            }
        }
    }
}

/// Parse a vision-LLM reply (JSON, possibly wrapped in text or code fences).
pub fn parse_llm(text: &str) -> Result<Vec<Detection>> {
    let start = text.find(['{', '[']).ok_or_else(|| {
        anyhow!(
            "AI reply had no JSON: {}",
            text.chars().take(120).collect::<String>()
        )
    })?;
    let end = text.rfind(['}', ']']).map(|i| i + 1).unwrap_or(text.len());
    let v: Value =
        serde_json::from_str(&text[start..end]).context("AI reply was not valid JSON")?;
    let items = match &v {
        Value::Array(a) => a.clone(),
        Value::Object(o) => o
            .get("objects")
            .or_else(|| o.get("detections"))
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default(),
        _ => vec![],
    };
    Ok(items
        .iter()
        .filter_map(|it| {
            let label = it["label"]
                .as_str()
                .or_else(|| it["name"].as_str())
                .or_else(|| it["object"].as_str())?;
            let mut score = it["confidence"]
                .as_f64()
                .or_else(|| it["score"].as_f64())
                .unwrap_or(0.8) as f32;
            if score > 1.0 {
                score /= 100.0;
            }
            let (mut x, mut y, mut w, mut h) = (0.0, 0.0, 1.0, 1.0);
            if let Some(b) = it["box_2d"].as_array().or_else(|| it["box"].as_array()) {
                let b: Vec<f32> = b
                    .iter()
                    .filter_map(|v| v.as_f64().map(|v| v as f32))
                    .collect();
                if b.len() == 4 {
                    let scale = if b.iter().any(|v| *v > 1.0) {
                        1000.0
                    } else {
                        1.0
                    };
                    let (y0, x0, y1, x1) = (b[0] / scale, b[1] / scale, b[2] / scale, b[3] / scale);
                    x = x0.clamp(0.0, 1.0);
                    y = y0.clamp(0.0, 1.0);
                    w = (x1 - x0).clamp(0.0, 1.0 - x);
                    h = (y1 - y0).clamp(0.0, 1.0 - y);
                }
            }
            let mut desc = it["description"].as_str().unwrap_or("").trim().to_string();
            if let Some(c) = it["color"].as_str().or_else(|| it["colour"].as_str()) {
                let c = c.trim().to_lowercase();
                if !c.is_empty() && !desc.to_lowercase().contains(&c) {
                    desc = format!("{c} {desc}").trim().to_string();
                }
            }
            Some(Detection {
                label: normalize_label(label),
                score: score.clamp(0.0, 1.0),
                x,
                y,
                w,
                h,
                desc: desc.chars().take(80).collect(),
            })
        })
        .collect())
}

// ---------- built-in YOLOX ----------

type Plan = std::sync::Arc<tract_onnx::prelude::TypedSimplePlan>;

pub struct Yolox {
    plan: Plan,
    pub size: usize,
}

impl Yolox {
    pub fn load(path: &Path) -> Result<Self> {
        use tract_onnx::prelude::*;
        let model = tract_onnx::onnx()
            .model_for_path(path)
            .map_err(|e| anyhow!("{e}"))?
            .into_optimized()
            .map_err(|e| anyhow!("{e}"))?;
        let size = model
            .input_fact(0)
            .ok()
            .and_then(|f| f.shape.as_concrete().and_then(|s| s.get(2).copied()))
            .unwrap_or(416);
        let plan = model.into_runnable().map_err(|e| anyhow!("{e}"))?;
        Ok(Self { plan, size })
    }

    pub fn detect(&self, frame: &Frame, min_score: f32) -> Result<Vec<Detection>> {
        use tract_onnx::prelude::*;
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
        let tensor = Tensor::from_shape(&[1, 3, size, size], &input).map_err(|e| anyhow!("{e}"))?;
        let out = self
            .plan
            .run(tvec!(tensor.into()))
            .map_err(|e| anyhow!("{e}"))?;
        let view = out[0]
            .to_plain_array_view::<f32>()
            .map_err(|e| anyhow!("{e}"))?;
        let data = view
            .as_slice()
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
                        desc: String::new(),
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
    fn parses_llm_replies() {
        let d = parse_llm("```json\n{\"objects\": [{\"label\": \"People\", \"confidence\": 92, \"box_2d\": [100, 200, 600, 400]}]}\n```").unwrap();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].label, "person");
        assert!((d[0].score - 0.92).abs() < 1e-6);
        assert!((d[0].x - 0.2).abs() < 1e-6 && (d[0].w - 0.2).abs() < 1e-6);
        assert!((d[0].y - 0.1).abs() < 1e-6 && (d[0].h - 0.5).abs() < 1e-6);
        assert_eq!(parse_llm("[{\"name\":\"car\"}]").unwrap()[0].label, "car");
        assert!(parse_llm("{\"objects\": []}").unwrap().is_empty());
        assert!(parse_llm("no idea").is_err());
    }

    #[test]
    fn picks_newest_gemini_flash() {
        let m: Vec<String> = [
            "gemini-2.5-flash",
            "gemini-3.8-flash",
            "gemini-3.8-flash-lite",
            "gemini-3.9-flash-preview-0901",
            "gemini-3.8-pro",
            "gemini-3.8-flash-image",
            "gemini-flash-latest",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(pick_gemini(&m).as_deref(), Some("gemini-3.8-flash"));
        let previews: Vec<String> = m.into_iter().filter(|x| x.contains("preview")).collect();
        assert_eq!(
            pick_gemini(&previews).as_deref(),
            Some("gemini-3.9-flash-preview-0901")
        );
        assert_eq!(pick_gemini(&[]), None);
    }

    #[test]
    fn labels() {
        assert_eq!(normalize_label("Cars"), "car");
        assert_eq!(normalize_label("bus"), "bus");
        assert_eq!(normalize_label("Bicycle"), "bicycle");
        assert_eq!(normalize_label("vehicle"), "car");
    }

    #[test]
    fn one_label_per_object() {
        let d = |l: &str, s: f32, x: f32| Detection {
            label: l.into(),
            score: s,
            x,
            y: 0.2,
            w: 0.2,
            h: 0.3,
            desc: String::new(),
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
            desc: String::new(),
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
