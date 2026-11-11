//! Alert descriptions from a vision-language model (Settings → Object
//! recognition → Descriptions): several pictures of the alert, in time
//! order, are sent together and the model writes what happened in a
//! sentence or two. The built-in smart search model only picks from fixed
//! phrases ("Adult in a black top"); a vision model can say "A courier in a
//! hi-vis vest leaves a parcel at the door and walks back to a white van".
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
        "These {n} pictures come from one home security camera (\"{cam}\") during an alert that lasted {secs} seconds, \
         in time order. {known}\n\n\
         Describe what happens across the pictures in one or two plain sentences, at most 40 words, \
         so the homeowner can find it later by searching. Mention people (clothing colours, what they carry, \
         what they do: arriving, leaving, delivering, looking around), vehicles (colour, type, make if clearly visible) \
         and animals. Say where things happen (driveway, front door, gate) when that's clear. \
         Don't guess who someone is unless a name is given above, don't describe the timestamp overlay, \
         and don't start with \"The pictures show\". If nothing notable happens, say what moved (for example \"Trees moving in the wind\"). \
         Reply with the description only.",
        cam = a.camera,
        secs = a.seconds.max(1),
        known = known.join(" "),
    )
}

fn b64_jpeg(f: &Frame) -> Result<String> {
    Ok(base64::engine::general_purpose::STANDARD.encode(f.fit(768).jpeg(80)?))
}

/// Describe an alert from its pictures (in time order).
pub async fn describe(cfg: &AiConfig, frames: &[Frame], info: &AlertInfo) -> Result<String> {
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
    let res = match cfg.describe_with.as_str() {
        "claude" => claude(&client, cfg, &text, &images).await,
        "gemini" => gemini(&client, cfg, &text, &images).await,
        "openai" => openai(&client, cfg, &text, &images).await,
        other => Err(anyhow::anyhow!("unknown description provider {other:?}")),
    }
    .map(|t| tidy(&t));
    let mut u = USAGE.lock().unwrap();
    match &res {
        Ok(t) if t.is_empty() => {
            u.last_error = Some("the model answered with nothing".into());
            bail!("the model answered with nothing")
        }
        Ok(_) => u.last_error = None,
        Err(e) => u.last_error = Some(format!("{e:#}")),
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
    let v: Value = resp.json().await.unwrap_or(Value::Null);
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
    let url =
        format!("https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent");
    let resp = client
        .post(&url)
        .header("x-goog-api-key", cfg.describe_key.trim())
        .json(&json!({"contents": [{"parts": parts}]}))
        .send()
        .await
        .context("cannot reach Gemini")?;
    let status = resp.status();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
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
    let v: Value = resp.json().await.unwrap_or(Value::Null);
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
    fn daily_limit() {
        // Uses the shared counter: only check that the limit stops it.
        assert!(super::take_slot(0));
        let used = super::USAGE.lock().unwrap().used;
        assert!(!super::take_slot(used));
    }
}
