//! Sound detection: listen to a camera's microphone for things that matter
//! when nobody is looking — glass breaking, a smoke alarm, a scream, a dog
//! barking, a gunshot, knocking. Uses YAMNet (Google, Apache-2.0: 521 kinds
//! of everyday sound, 16 MB), run on this computer every half second on the
//! last second of sound. What's heard makes an alert like a person seen.

use crate::config::CameraConfig;
use crate::manager::Manager;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tracing::{info, warn};

/// Google's YAMNet in ONNX form (a copy kept on Hugging Face, this exact
/// version), and its SHA-256.
const MODEL_URL: &str = "https://huggingface.co/andrelgomes/yamnet-onnx/resolve/8a03a1572569685c42fdbef54ff36435dbaaf689/yamnet.onnx";
const MODEL_SHA256: &str = "1510041dce24a2e9e84ec546807ac408ae496da6d1ed41bc3ccba649623f8e19";
/// The 521 kinds of sound, in the model's order.
const CLASSES: &str = include_str!("yamnet_classes.txt");
/// One look: 0.975 s of 16 kHz sound (one YAMNet frame).
const WINDOW: usize = 15_600;
/// A look every half second.
const HOP: usize = 8_000;

/// What can be listened for: id, name, and the YAMNet kinds of sound in it.
pub const KINDS: &[(&str, &str, &[&str])] = &[
    ("glass", "Glass breaking", &["Glass", "Shatter", "Chink, clink"]),
    ("alarm", "Smoke or fire alarm", &["Smoke detector, smoke alarm", "Fire alarm", "Alarm", "Buzzer", "Beep, bleep"]),
    ("scream", "Screaming or shouting", &["Screaming", "Shout", "Yell", "Children shouting", "Whimper"]),
    ("gunshot", "Gunshot or bang", &["Gunshot, gunfire", "Explosion", "Machine gun", "Fusillade", "Artillery fire", "Cap gun", "Burst, pop"]),
    ("dog", "Dog barking", &["Bark", "Dog", "Howl", "Growling", "Bow-wow", "Yip", "Whimper (dog)"]),
    ("baby", "Baby crying", &["Baby cry, infant cry"]),
    ("siren", "Siren", &["Siren", "Police car (siren)", "Ambulance (siren)", "Fire engine, fire truck (siren)", "Civil defense siren"]),
    ("car_alarm", "Car alarm", &["Car alarm"]),
    ("knock", "Knocking or doorbell", &["Knock", "Doorbell", "Ding-dong"]),
    ("speech", "Talking", &["Speech", "Conversation", "Narration, monologue"]),
];

fn class_names() -> &'static [&'static str] {
    static N: LazyLock<Vec<&'static str>> = LazyLock::new(|| CLASSES.lines().collect());
    &N
}

/// For each kind (as in [`KINDS`]), the model's class numbers.
fn kind_classes() -> &'static [Vec<usize>] {
    static K: LazyLock<Vec<Vec<usize>>> = LazyLock::new(|| {
        let names = class_names();
        KINDS
            .iter()
            .map(|(_, _, cls)| cls.iter().filter_map(|c| names.iter().position(|n| n == c)).collect())
            .collect()
    });
    &K
}

/// How sure, per kind, from the model's 521 scores.
fn kind_scores(scores: &[f32]) -> Vec<f32> {
    kind_classes()
        .iter()
        .map(|cls| cls.iter().map(|&i| scores.get(i).copied().unwrap_or(0.0)).fold(0.0, f32::max))
        .collect()
}

/// The score a kind must reach, from the camera's sensitivity (1–10).
pub fn threshold(sensitivity: u8) -> f32 {
    (0.8 - 0.06 * sensitivity.clamp(1, 10) as f32).max(0.15)
}

pub struct Yamnet {
    net: crate::infer::Net,
}

impl Yamnet {
    pub fn load(path: &Path) -> Result<Self> {
        let net = crate::infer::load(path, "sound detection", || {
            use tract_onnx::prelude::*;
            tract_onnx::onnx()
                .model_for_path(path)
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .with_input_fact(0, f32::fact([WINDOW]).into())
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .into_optimized()
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .into_runnable()
                .map_err(|e| anyhow::anyhow!("{e}"))
        })?;
        Ok(Self { net })
    }

    /// The 521 scores for one window of 16 kHz mono sound (−1…1).
    pub fn scores(&self, wave: &[f32]) -> Result<Vec<f32>> {
        let mut w = wave.to_vec();
        w.resize(WINDOW, 0.0);
        let out = self.net.run(crate::infer::Input::f32(&[WINDOW], w))?;
        let s = out.into_iter().next().context("no output")?;
        // One row of 521 per frame: the loudest of each.
        let mut best = vec![0f32; 521];
        for row in s.chunks(521) {
            for (b, x) in best.iter_mut().zip(row) {
                *b = b.max(*x);
            }
        }
        Ok(best)
    }
}

/// The model, loaded once (downloaded the first time).
static MODEL: LazyLock<tokio::sync::Mutex<Option<Arc<Yamnet>>>> = LazyLock::new(Default::default);

async fn model(models_dir: &Path) -> Result<Arc<Yamnet>> {
    let mut m = MODEL.lock().await;
    if let Some(y) = m.as_ref()
        && !y.net.misplaced()
    {
        return Ok(y.clone());
    }
    let path = models_dir.join("yamnet.onnx");
    if !path.exists() {
        download(&path).await?;
    }
    let p = path.clone();
    let y = Arc::new(tokio::task::spawn_blocking(move || Yamnet::load(&p)).await??);
    info!("sound detection: model loaded");
    *m = Some(y.clone());
    Ok(y)
}

async fn download(path: &Path) -> Result<()> {
    info!("sound detection: downloading the model (16 MB)");
    std::fs::create_dir_all(path.parent().context("no folder")?)?;
    let job = crate::jobs::start("download", "Downloading the sound detection model");
    let mut resp = crate::jobs::download_client()?.get(MODEL_URL).send().await?.error_for_status()?;
    let total = resp.content_length().unwrap_or(0);
    let tmp = path.with_extension("part");
    let mut bytes = Vec::with_capacity(total as usize);
    while let Some(c) = resp.chunk().await? {
        bytes.extend_from_slice(&c);
        job.bytes(bytes.len() as u64, total);
    }
    use sha2::{Digest, Sha256};
    let sum: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    anyhow::ensure!(sum == MODEL_SHA256, "the downloaded sound model isn't the expected file");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// What each camera is hearing (Camera → Audio shows it live).
#[derive(Clone, Default, serde::Serialize)]
pub struct Status {
    pub listening: bool,
    pub error: Option<String>,
    /// The loudest kinds of sound right now (name, score), any of the 521.
    pub now: Vec<(String, f32)>,
    /// The kinds listened for and how sure the last look was of each.
    pub kinds: Vec<(String, f32)>,
    pub last_heard: Option<String>,
    pub last_at: Option<chrono::DateTime<chrono::Local>>,
}

pub static STATUS: LazyLock<Mutex<HashMap<String, Status>>> = LazyLock::new(Default::default);

fn set(id: &str, f: impl FnOnce(&mut Status)) {
    f(STATUS.lock().unwrap().entry(id.to_string()).or_default());
}

/// Listen to one camera until its settings change or it's removed.
async fn listen(mgr: &Arc<Manager>, cam: &CameraConfig) -> Result<()> {
    let y = model(&models_dir(mgr)).await?;
    let rt = mgr.camera(&cam.id).context("camera not running")?;
    // The stream with sound: the main stream, unless only the sub-stream has it.
    let has = |a: Option<String>| a.is_some_and(|a| !a.is_empty());
    let relay = if rt.relay_main.audio().as_deref() == Some("") && has(rt.relay_sub.audio()) { rt.relay_sub.clone() } else { rt.relay_main.clone() };
    let ffmpeg = mgr.config().server.ffmpeg_path;
    let mut child = crate::load::cmd(&ffmpeg)
        .args(["-hide_banner", "-loglevel", "error"])
        .args(crate::relay::pipe_input_args_low_delay())
        .args(["-vn", "-map", "0:a:0", "-ac", "1", "-ar", "16000", "-f", "f32le", "pipe:1"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("starting ffmpeg")?;
    let stop = tokio_util::sync::CancellationToken::new();
    let _feed = crate::relay::feed(relay.subscribe(), child.stdin.take().context("no stdin")?, stop.clone());
    let _stop = stop.drop_guard();
    let mut out = child.stdout.take().context("no stdout")?;
    set(&cam.id, |s| {
        s.listening = true;
        s.error = None;
    });
    let want: Vec<usize> = KINDS.iter().enumerate().filter(|(_, k)| cam.sound.kinds.iter().any(|w| w == k.0)).map(|(i, _)| i).collect();
    let thr = threshold(cam.sound.sensitivity);
    let mut wave: Vec<f32> = Vec::with_capacity(WINDOW * 2);
    let mut raw: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; 16 * 1024];
    let mut last_alert: HashMap<usize, Instant> = HashMap::new();
    let mut checked = Instant::now();
    loop {
        let n = match tokio::time::timeout(Duration::from_secs(20), out.read(&mut buf)).await {
            Ok(Ok(0)) => anyhow::bail!("the camera isn't sending sound"),
            Ok(Ok(n)) => n,
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => anyhow::bail!("no sound from the camera for 20 s"),
        };
        raw.extend_from_slice(&buf[..n]);
        let whole = raw.len() / 4 * 4;
        wave.extend(raw[..whole].chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])));
        raw.drain(..whole);
        while wave.len() >= WINDOW {
            let w = wave[..WINDOW].to_vec();
            wave.drain(..HOP);
            // Stop when the settings change (checked now and then).
            if checked.elapsed() > Duration::from_secs(5) {
                checked = Instant::now();
                if mgr.config().cameras.iter().find(|c| c.id == cam.id) != Some(cam) {
                    return Ok(());
                }
                // The GPU became ready (or the processor was chosen): start
                // again with the model in its place.
                if y.net.misplaced() {
                    return Ok(());
                }
            }
            let y2 = y.clone();
            let scores = tokio::task::spawn_blocking(move || y2.scores(&w)).await??;
            let ks = kind_scores(&scores);
            let mut top: Vec<(usize, f32)> = scores.iter().copied().enumerate().collect();
            top.sort_by(|a, b| b.1.total_cmp(&a.1));
            set(&cam.id, |s| {
                s.now = top.iter().take(3).map(|(i, v)| (class_names()[*i].to_string(), *v)).collect();
                s.kinds = want.iter().map(|&i| (KINDS[i].0.to_string(), ks[i])).collect();
            });
            for &i in &want {
                if ks[i] < thr || last_alert.get(&i).is_some_and(|t| t.elapsed() < Duration::from_secs(30)) {
                    continue;
                }
                last_alert.insert(i, Instant::now());
                let what = KINDS[i].1;
                // (The same sound again in one alert: one note.)
                let note = format!("Heard: {}", what.to_lowercase());
                info!(target: "event", "[{}] heard {} ({:.0}%)", cam.name, what.to_lowercase(), ks[i] * 100.0);
                set(&cam.id, |s| {
                    s.last_heard = Some(what.to_string());
                    s.last_at = Some(chrono::Local::now());
                });
                if let Some(d) = rt.detector() {
                    d.external("sound", &note);
                }
            }
        }
    }
}

fn models_dir(mgr: &Manager) -> PathBuf {
    mgr.data_dir.join("models")
}

/// Keep a listener running for each camera with sound detection on.
pub async fn run(mgr: Arc<Manager>) {
    let mut tasks: HashMap<String, (CameraConfig, tokio::task::JoinHandle<()>)> = HashMap::new();
    loop {
        let cfg = mgr.config();
        let want: Vec<CameraConfig> = cfg.cameras.iter().filter(|c| c.enabled && c.sound.enabled && !c.sound.kinds.is_empty()).cloned().collect();
        tasks.retain(|id, (c, h)| {
            let keep = want.iter().any(|w| w == c) && !h.is_finished();
            if !keep {
                h.abort();
                if !want.iter().any(|w| &w.id == id) {
                    STATUS.lock().unwrap().remove(id);
                }
            }
            keep
        });
        for c in want {
            if tasks.contains_key(&c.id) {
                continue;
            }
            let (m, cam) = (mgr.clone(), c.clone());
            let h = tokio::spawn(async move {
                let mut wait = 5u64;
                loop {
                    match listen(&m, &cam).await {
                        Ok(()) => return,
                        Err(e) => {
                            warn!("[{}] sound detection: {e:#} — trying again in {wait} s", cam.name);
                            set(&cam.id, |s| {
                                s.listening = false;
                                s.error = Some(format!("{e:#}"));
                            });
                        }
                    }
                    tokio::time::sleep(Duration::from_secs(wait)).await;
                    wait = (wait * 2).min(300);
                }
            });
            tasks.insert(c.id.clone(), (c, h));
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_has_its_sounds() {
        assert_eq!(class_names().len(), 521);
        for (k, cls) in KINDS.iter().zip(kind_classes()) {
            assert_eq!(cls.len(), k.2.len(), "{}: a sound name isn't one of YAMNet's", k.0);
        }
    }

    /// With the model at $YAMNET (not downloaded in tests): a smoke-alarm
    /// beep is heard as an alarm, quiet as nothing.
    #[test]
    fn hears_an_alarm() {
        let Ok(p) = std::env::var("YAMNET") else { return };
        let y = Yamnet::load(Path::new(&p)).unwrap();
        let beep: Vec<f32> = (0..WINDOW).map(|i| if (i / 4000) % 2 == 0 { 0.5 * (i as f32 * 3100.0 * std::f32::consts::TAU / 16000.0).sin() } else { 0.0 }).collect();
        let k = kind_scores(&y.scores(&beep).unwrap());
        let alarm = KINDS.iter().position(|x| x.0 == "alarm").unwrap();
        assert!(k[alarm] > 0.3, "alarm {:.2}", k[alarm]);
        let quiet = kind_scores(&y.scores(&vec![0.0; WINDOW]).unwrap());
        assert!(quiet.iter().all(|&v| v < 0.1), "{quiet:?}");
    }
}
