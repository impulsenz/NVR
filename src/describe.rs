//! Alert descriptions from a vision-language model (Settings → Object
//! recognition → Descriptions): several pictures of the alert, in time
//! order, are sent together and the model writes what happened in a
//! sentence or two. The built-in smart search model only picks from fixed
//! phrases ("Adult in a black top"); a vision model can say "A courier in a
//! hi-vis vest leaves a parcel at the door and walks back to a white van".
//!
//! The same model judges whether the activity looks suspicious, for
//! notifications (Settings → Notifications).
//!
//! Providers: Claude (Anthropic API), Gemini, or any OpenAI-compatible
//! server, which includes Ollama and LM Studio running on your own computer
//! (nothing leaves the house).

use crate::config::AiConfig;
use crate::imgutil::Frame;
use anyhow::{Context, Result, bail};
use base64::Engine;
use chrono::{Local, NaiveDate};
use serde_json::{Value, json};
use std::sync::Mutex;
use std::time::Duration;

/// What the model is told about the alert besides the pictures.
pub struct AlertInfo {
    pub camera: String,
    pub seconds: i64,
    /// What the object detector found ("person", "car").
    pub objects: Vec<String>,
    /// Known people recognised by face.
    pub names: Vec<String>,
    pub plates: Vec<String>,
    /// Zone happenings ("Person in Porch").
    pub notes: Vec<String>,
    /// When it started, and whether that's at night (for judging).
    pub time: String,
    pub night: bool,
    /// The homeowner's own instructions (Settings).
    pub extra: String,
    /// Someone is waiting for the answer (a test button): don't wait long
    /// for a busy model.
    pub interactive: bool,
}

impl AlertInfo {
    pub fn of(e: &crate::events::Event, camera: &str, cfg: &crate::config::Config) -> Self {
        Self {
            camera: camera.to_string(),
            seconds: (e.end - e.start).num_seconds(),
            objects: e
                .labels
                .iter()
                .filter(|l| l.label != "motion")
                .map(|l| l.label.clone())
                .collect(),
            names: e.faces.iter().filter_map(|f| f.name.clone()).collect(),
            plates: e.plates.clone(),
            notes: e.notes.clone(),
            time: e.start.format("%H:%M").to_string(),
            night: crate::notify::is_night(&cfg.notify, e.start),
            extra: cfg.ai.describe_prompt.trim().to_string(),
            interactive: false,
        }
    }
}

/// The model's answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Described {
    /// What happened, in a sentence or two.
    pub text: String,
    /// Why it looks suspicious (None: it doesn't).
    pub suspicious: Option<String>,
}

#[derive(Default)]
struct Usage {
    day: Option<NaiveDate>,
    used: u32,
    last_error: Option<String>,
    gemini_model: Option<String>,
}

static USAGE: Mutex<Usage> = Mutex::new(Usage {
    day: None,
    used: 0,
    last_error: None,
    gemini_model: None,
});

/// For the settings page: how many alerts went to the model today, and the
/// last problem, if any.
pub fn status() -> Value {
    let u = USAGE.lock().unwrap();
    let today = Local::now().date_naive();
    json!({
        "used_today": if u.day == Some(today) { u.used } else { 0 },
        "error": u.last_error,
        "backlog_paused": !available(),
    })
}

/// A vision model is set up to write descriptions.
pub fn enabled(cfg: &AiConfig) -> bool {
    cfg.describe_with != "builtin" && !cfg.describe_with.is_empty()
}

/// Take one of today's allowance; false when it's used up.
fn take_slot(limit: u32) -> bool {
    let mut u = USAGE.lock().unwrap();
    let today = Local::now().date_naive();
    if u.day != Some(today) {
        u.day = Some(today);
        u.used = 0;
    }
    if limit > 0 && u.used >= limit {
        return false;
    }
    u.used += 1;
    true
}

fn prompt(n: usize, a: &AlertInfo) -> String {
    let mut known = Vec::new();
    if !a.objects.is_empty() {
        known.push(format!("The detector saw: {}.", a.objects.join(", ")));
    }
    if !a.names.is_empty() {
        known.push(format!(
            "Recognised by face (use these names): {}.",
            a.names.join(", ")
        ));
    }
    if !a.plates.is_empty() {
        known.push(format!("Number plates read: {}.", a.plates.join(", ")));
    }
    if !a.notes.is_empty() {
        known.push(format!("Zones: {}.", a.notes.join("; ")));
    }
    format!(
        "These {n} pictures come from one home security camera (\"{cam}\") during an alert that started at {time} ({light}) \
         and lasted {secs} seconds, in time order. {known}\n\n\
         Reply with JSON only: {{\"summary\": \"...\", \"suspicious\": true or false, \"reason\": \"...\"}}.\n\
         summary: what happens across the pictures in one or two plain sentences, at most 40 words, \
         so the homeowner can find it later by searching. Mention people (clothing colours, what they carry, \
         what they do: arriving, leaving, delivering, looking around), vehicles (colour, type, make if clearly visible) \
         and animals. Say where things happen (driveway, front door, gate) when that's clear. \
         Don't guess who someone is unless a name is given above, don't describe the timestamp overlay, \
         and don't start with \"The pictures show\". If nothing notable happens, say what moved (for example \"Trees moving in the wind\").\n\
         suspicious: true only for things like trying door handles, looking into cars or windows, lingering without reason \
         (especially at night), carrying things away from the property, climbing fences or hiding the face. \
         Deliveries, residents, visitors, passers-by and animals are normal.\n\
         reason: a few words, only when suspicious.{extra}",
        cam = a.camera,
        time = a.time,
        light = if a.night { "night" } else { "day" },
        secs = a.seconds.max(1),
        known = known.join(" "),
        extra = if a.extra.is_empty() {
            String::new()
        } else {
            format!(
                "\n\nThe homeowner adds (follow this where it applies):\n{}",
                a.extra
            )
        },
    )
}

/// Read the model's reply: JSON as asked, or plain text from a model that
/// didn't follow the format.
fn parse_reply(reply: &str) -> Described {
    let json = reply
        .find('{')
        .zip(reply.rfind('}'))
        .filter(|(a, b)| a < b)
        .and_then(|(a, b)| serde_json::from_str::<Value>(&reply[a..=b]).ok());
    match json {
        Some(v) if v["summary"].is_string() => Described {
            text: tidy(v["summary"].as_str().unwrap_or("")),
            suspicious: (v["suspicious"] == true || v["suspicious"] == "true").then(|| {
                let r = tidy(v["reason"].as_str().unwrap_or(""));
                let r = r.trim_end_matches('.');
                let mut c = r.chars();
                match c.next() {
                    Some(f) => f.to_uppercase().chain(c).collect(),
                    None => "Looks suspicious".into(),
                }
            }),
        },
        _ => Described {
            text: tidy(reply),
            suspicious: None,
        },
    }
}

fn b64_jpeg(f: &Frame) -> Result<String> {
    Ok(base64::engine::general_purpose::STANDARD.encode(f.fit(768).jpeg(80)?))
}

/// The model is busy or over its rate limit: try again after this long.
#[derive(Debug)]
pub struct Busy(pub Duration);

impl std::fmt::Display for Busy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the model is busy or over its rate limit (trying again in {} s)",
            self.0.as_secs()
        )
    }
}

impl std::error::Error for Busy {}

fn retry_after_header(resp: &reqwest::Response) -> Option<Duration> {
    resp.headers()
        .get("retry-after")?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

/// Too many requests or overloaded: how long to wait (the server's own
/// "retry after", Gemini's retryDelay, or 30 s).
fn busy(status: reqwest::StatusCode, retry_after: Option<Duration>, v: &Value) -> Option<Duration> {
    if !matches!(status.as_u16(), 429 | 503 | 529) {
        return None;
    }
    let gemini = v["error"]["details"].as_array().and_then(|d| {
        d.iter().find_map(|x| {
            x["retryDelay"]
                .as_str()?
                .trim_end_matches('s')
                .parse::<f64>()
                .ok()
                .map(|s| Duration::from_secs_f64(s.ceil()))
        })
    });
    Some(
        retry_after
            .or(gemini)
            .unwrap_or(Duration::from_secs(30))
            .clamp(Duration::from_secs(2), Duration::from_secs(300)),
    )
}

/// When the next request may go out (requests are spread out to stay
/// within the per-minute limit).
static NEXT_SLOT: tokio::sync::Mutex<Option<std::time::Instant>> =
    tokio::sync::Mutex::const_new(None);

async fn pace(per_minute: u32) {
    if per_minute == 0 {
        return;
    }
    let gap = Duration::from_secs_f64(60.0 / per_minute as f64);
    let mut next = NEXT_SLOT.lock().await;
    let now = std::time::Instant::now();
    if let Some(t) = *next
        && t > now
    {
        tokio::time::sleep(t - now).await;
    }
    *next = Some(std::time::Instant::now() + gap);
}

/// After a failure the background work waits before asking again (failed
/// alerts stay in the backlog and are described later).
static COOL_UNTIL: Mutex<Option<std::time::Instant>> = Mutex::new(None);

/// The background work may ask the model now (not cooling down after a
/// problem).
pub fn available() -> bool {
    COOL_UNTIL
        .lock()
        .unwrap()
        .is_none_or(|t| std::time::Instant::now() >= t)
}

/// Describe an alert from its pictures (in time order). Requests are spread
/// out to stay within the per-minute limit; when the model says it's busy
/// or over its limit, it's asked again after the time it gives.
pub async fn describe(cfg: &AiConfig, frames: &[Frame], info: &AlertInfo) -> Result<Described> {
    if frames.is_empty() {
        bail!("no pictures");
    }
    if !take_slot(cfg.describe_per_day) {
        bail!(
            "today's limit of {} described alerts is used up",
            cfg.describe_per_day
        );
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(120))
        .build()?;
    let text = prompt(frames.len(), info);
    let images: Vec<String> = frames.iter().map(b64_jpeg).collect::<Result<_>>()?;
    let mut attempt = 0;
    let res = loop {
        pace(cfg.describe_per_minute).await;
        let r = match cfg.describe_with.as_str() {
            "claude" => claude(&client, cfg, &text, &images).await,
            "gemini" => gemini(&client, cfg, &text, &images).await,
            "openai" => openai(&client, cfg, &text, &images).await,
            other => Err(anyhow::anyhow!("unknown description provider {other:?}")),
        };
        match &r {
            Err(e)
                if attempt < 2
                    && e.downcast_ref::<Busy>()
                        .is_some_and(|b| !info.interactive || b.0 <= Duration::from_secs(15)) =>
            {
                let wait = e.downcast_ref::<Busy>().unwrap().0;
                tracing::info!("vision model: {e}");
                tokio::time::sleep(wait).await;
                attempt += 1;
            }
            _ => break r.map(|t| parse_reply(&t)),
        }
    };
    let res = match res {
        Ok(d) if d.text.is_empty() => Err(anyhow::anyhow!("the model answered with nothing")),
        r => r,
    };
    let mut u = USAGE.lock().unwrap();
    match &res {
        Ok(_) => {
            u.last_error = None;
            *COOL_UNTIL.lock().unwrap() = None;
        }
        Err(e) => {
            u.last_error = Some(format!("{e:#}"));
            // Not counted against today's allowance.
            u.used = u.used.saturating_sub(1);
            if !info.interactive {
                let wait = e
                    .downcast_ref::<Busy>()
                    .map(|b| b.0)
                    .unwrap_or(Duration::from_secs(120));
                *COOL_UNTIL.lock().unwrap() = Some(std::time::Instant::now() + wait);
            }
        }
    }
    res
}

/// One line, no quotes or markdown, ends with a full stop.
fn tidy(t: &str) -> String {
    let t = t
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let t = t.trim_matches(|c: char| c == '"' || c == '*' || c.is_whitespace());
    let mut t: String = t.chars().take(400).collect();
    if !t.is_empty() && !t.ends_with(['.', '!', '?']) {
        t.push('.');
    }
    t
}

async fn claude(
    client: &reqwest::Client,
    cfg: &AiConfig,
    text: &str,
    images: &[String],
) -> Result<String> {
    let model = match cfg.describe_model.trim() {
        "" => "claude-opus-5-5",
        m => m,
    };
    let mut content: Vec<Value> = images
        .iter()
        .map(|d| {
            json!({"type": "image", "source": {"type": "base64", "media_type": "image/jpeg", "data": d}})
        })
        .collect();
    content.push(json!({"type": "text", "text": text}));
    let body = json!({
        "model": model,
        "max_tokens": 2000,
        // A short description: little thinking needed.
        "output_config": {"effort": "low"},
        // If a safety check declines, another model answers in the same call.
        "fallbacks": "default",
        "messages": [{"role": "user", "content": content}],
    });
    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", cfg.describe_key.trim())
        .header("anthropic-version", "2026-10-21")
        .header("anthropic-beta", "server-side-fallback-2026-07-01")
        .json(&body)
        .send()
        .await
        .context("cannot reach the Anthropic API")?;
    let status = resp.status();
    let retry_after = retry_after_header(&resp);
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    if let Some(wait) = busy(status, retry_after, &v) {
        return Err(Busy(wait).into());
    }
    if !status.is_success() {
        bail!(
            "Claude {status}: {}",
            v["error"]["message"].as_str().unwrap_or("request failed")
        );
    }
    if v["stop_reason"] == "refusal" {
        bail!("Claude declined to describe this alert");
    }
    Ok(v["content"]
        .as_array()
        .map(|c| {
            c.iter()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default())
}

async fn gemini(
    client: &reqwest::Client,
    cfg: &AiConfig,
    text: &str,
    images: &[String],
) -> Result<String> {
    let model = match cfg.describe_model.trim() {
        "" => {
            let cached = USAGE.lock().unwrap().gemini_model.clone();
            match cached {
                Some(m) => m,
                None => {
                    let lookup = AiConfig {
                        provider: "gemini".into(),
                        api_key: cfg.describe_key.clone(),
                        ..AiConfig::default()
                    };
                    let models = crate::ai::list_models(client, &lookup)
                        .await
                        .unwrap_or_default();
                    let m = crate::ai::pick_gemini(&models)
                        .unwrap_or_else(|| "gemini-flash-latest".into());
                    USAGE.lock().unwrap().gemini_model = Some(m.clone());
                    m
                }
            }
        }
        m => m.trim_start_matches("models/").to_string(),
    };
    let mut parts: Vec<Value> = images
        .iter()
        .map(|d| json!({"inline_data": {"mime_type": "image/jpeg", "data": d}}))
        .collect();
    parts.push(json!({"text": text}));
    // GAZE_GEMINI_BASE: a stand-in server for tests.
    let base = std::env::var("GAZE_GEMINI_BASE")
        .unwrap_or_else(|_| "https://generativelanguage.googleapis.com".into());
    let url = format!("{base}/v1beta/models/{model}:generateContent");
    let resp = client
        .post(&url)
        .header("x-goog-api-key", cfg.describe_key.trim())
        .json(&json!({"contents": [{"parts": parts}]}))
        .send()
        .await
        .context("cannot reach Gemini")?;
    let status = resp.status();
    let retry_after = retry_after_header(&resp);
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    if let Some(wait) = busy(status, retry_after, &v) {
        return Err(Busy(wait).into());
    }
    if !status.is_success() {
        if status == reqwest::StatusCode::NOT_FOUND {
            USAGE.lock().unwrap().gemini_model = None;
        }
        bail!(
            "Gemini {status}: {}",
            v["error"]["message"].as_str().unwrap_or("request failed")
        );
    }
    Ok(v["candidates"][0]["content"]["parts"]
        .as_array()
        .map(|p| p.iter().filter_map(|x| x["text"].as_str()).collect())
        .unwrap_or_default())
}

async fn openai(
    client: &reqwest::Client,
    cfg: &AiConfig,
    text: &str,
    images: &[String],
) -> Result<String> {
    let base = cfg.describe_url.trim().trim_end_matches('/');
    let base = if base.contains("://") {
        base.to_string()
    } else {
        format!("http://{base}")
    };
    let mut content = vec![json!({"type": "text", "text": text})];
    content.extend(images.iter().map(|d| {
        json!({"type": "image_url", "image_url": {"url": format!("data:image/jpeg;base64,{d}")}})
    }));
    let url = format!("{base}/chat/completions");
    let mut req = client.post(&url).json(&json!({
        "model": cfg.describe_model.trim(),
        "messages": [{"role": "user", "content": content}],
    }));
    if !cfg.describe_key.trim().is_empty() {
        req = req.bearer_auth(cfg.describe_key.trim());
    }
    let resp = req
        .send()
        .await
        .with_context(|| format!("cannot reach {url}"))?;
    let status = resp.status();
    let retry_after = retry_after_header(&resp);
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    if let Some(wait) = busy(status, retry_after, &v) {
        return Err(Busy(wait).into());
    }
    if !status.is_success() {
        bail!(
            "{status}: {}",
            v["error"]["message"]
                .as_str()
                .or(v["error"].as_str())
                .unwrap_or("request failed")
        );
    }
    Ok(v["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn tidies_replies() {
        assert_eq!(
            super::tidy("\"A man in a red jacket walks to the door\"\n"),
            "A man in a red jacket walks to the door."
        );
        assert_eq!(super::tidy("**Cat on wall.**"), "Cat on wall.");
        assert_eq!(super::tidy("  "), "");
    }

    #[test]
    fn reads_replies() {
        let d = super::parse_reply(
            "```json\n{\"summary\": \"A man tries the car door handle\", \"suspicious\": true, \"reason\": \"trying car doors\"}\n```",
        );
        assert_eq!(d.text, "A man tries the car door handle.");
        assert_eq!(d.suspicious.as_deref(), Some("Trying car doors"));
        let d = super::parse_reply(
            "{\"summary\": \"A cat walks past.\", \"suspicious\": false, \"reason\": \"\"}",
        );
        assert_eq!((d.text.as_str(), d.suspicious), ("A cat walks past.", None));
        let d = super::parse_reply("A courier leaves a parcel");
        assert_eq!(
            (d.text.as_str(), d.suspicious),
            ("A courier leaves a parcel.", None)
        );
    }

    #[test]
    fn reads_rate_limits() {
        use reqwest::StatusCode;
        let gem = serde_json::json!({"error": {"details": [{"@type": "x.RetryInfo", "retryDelay": "17s"}]}});
        assert_eq!(
            super::busy(StatusCode::TOO_MANY_REQUESTS, None, &gem),
            Some(std::time::Duration::from_secs(17))
        );
        assert_eq!(
            super::busy(
                StatusCode::TOO_MANY_REQUESTS,
                Some(std::time::Duration::from_secs(5)),
                &serde_json::Value::Null
            ),
            Some(std::time::Duration::from_secs(5))
        );
        assert_eq!(super::busy(StatusCode::BAD_REQUEST, None, &gem), None);
    }

    #[tokio::test]
    async fn requests_are_spaced_out() {
        let t = std::time::Instant::now();
        super::pace(600).await; // one every 0.1 s
        super::pace(600).await;
        super::pace(600).await;
        assert!(t.elapsed() >= std::time::Duration::from_millis(190));
    }

    #[test]
    fn daily_limit() {
        // Uses the shared counter: only check that the limit stops it.
        assert!(super::take_slot(0));
        let used = super::USAGE.lock().unwrap().used;
        assert!(!super::take_slot(used));
    }
}
