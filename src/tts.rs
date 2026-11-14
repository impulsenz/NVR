//! Text to speech for a camera's speaker: type words on the live view and
//! the camera says them. On Windows, the voices built into Windows: the
//! classic desktop voices (System.Speech, "sapi:NAME") and the newer ones
//! added in Settings → Time & language → Speech ("onecore:NAME"). Elsewhere
//! espeak-ng / espeak, or ffmpeg's flite voice.

use crate::config::TtsConfig;
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::path::Path;

/// The longest text spoken at once.
pub const MAX_CHARS: usize = 400;

#[derive(Debug, Clone, Serialize)]
pub struct Voice {
    /// What's saved in the settings.
    pub id: String,
    pub name: String,
    /// Language, e.g. "en-NZ".
    pub lang: String,
}

/// Say `text`: 8 kHz mono samples, ready for the speaker.
pub async fn speak(ffmpeg: &str, text: &str, t: &TtsConfig) -> Result<Vec<i16>> {
    let text: String = text.chars().filter(|c| !c.is_control()).take(MAX_CHARS).collect();
    let text = text.trim().to_string();
    if text.is_empty() {
        bail!("type something to say first");
    }
    let wav = std::env::temp_dir().join(format!("gaze-tts-{}-{}.wav", std::process::id(), rand_id()));
    let made = voice(ffmpeg, &text, t, &wav).await;
    let out = match made {
        Ok(()) => to_samples(ffmpeg, &wav, t.volume).await,
        Err(e) => Err(e),
    };
    let _ = std::fs::remove_file(&wav);
    let mut s = out?;
    if s.is_empty() {
        bail!("the voice made no sound");
    }
    if t.chime {
        let mut c = chime();
        c.append(&mut s);
        s = c;
    }
    Ok(s)
}

fn rand_id() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0)
}

/// Two soft rising tones and a short pause (8 kHz).
fn chime() -> Vec<i16> {
    let mut out = Vec::new();
    for (f, ms) in [(660.0f32, 180), (880.0, 260)] {
        let n = 8 * ms;
        for i in 0..n {
            let t = i as f32 / 8000.0;
            let env = (1.0 - i as f32 / n as f32).powf(1.5) * (i as f32 / 80.0).min(1.0);
            out.push(((t * f * std::f32::consts::TAU).sin() * env * 9000.0) as i16);
        }
    }
    out.extend(std::iter::repeat_n(0, 8 * 150));
    out
}

/// A 16-bit mono WAV file of 8 kHz samples (to hear a voice in the browser).
pub fn wav(samples: &[i16]) -> Vec<u8> {
    let data = samples.len() as u32 * 2;
    let mut b = Vec::with_capacity(44 + data as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&8000u32.to_le_bytes());
    b.extend_from_slice(&16000u32.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        b.extend_from_slice(&s.to_le_bytes());
    }
    b
}

#[cfg(windows)]
const WINRT_HELPER: &str = r#"
Add-Type -AssemblyName System.Runtime.WindowsRuntime
$asTask = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object { $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1' })[0]
function Await($op, $type) { $t = $asTask.MakeGenericMethod($type).Invoke($null, @($op)); $t.Wait(-1) | Out-Null; $t.Result }
[Windows.Media.SpeechSynthesis.SpeechSynthesizer, Windows.Media.SpeechSynthesis, ContentType = WindowsRuntime] | Out-Null
[Windows.Storage.Streams.DataReader, Windows.Storage.Streams, ContentType = WindowsRuntime] | Out-Null
"#;

#[cfg(windows)]
async fn powershell(script: &str, env: &[(&str, String)]) -> Result<std::process::Output> {
    let mut c = crate::load::cmd("powershell");
    c.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script]);
    for (k, v) in env {
        c.env(k, v);
    }
    c.output().await.context("couldn't start Windows' voice (PowerShell)")
}

/// The voices this computer has: downloaded ones first, then its own.
pub async fn voices() -> Vec<Voice> {
    // Downloadable ones too: chosen, they download by themselves.
    let mut v: Vec<Voice> = crate::piper::CATALOG
        .iter()
        .map(|(id, name, lang, _)| Voice {
            id: format!("piper:{id}"),
            name: if crate::piper::installed(id) { format!("{name} — natural voice") } else { format!("{name} — natural voice (downloads when chosen)") },
            lang: lang.to_string(),
        })
        .collect();
    v.extend(system_voices().await);
    v
}

async fn system_voices() -> Vec<Voice> {
    #[cfg(windows)]
    {
        let mut out = Vec::new();
        let sapi = "Add-Type -AssemblyName System.Speech; \
            (New-Object System.Speech.Synthesis.SpeechSynthesizer).GetInstalledVoices() | Where-Object { $_.Enabled } | \
            ForEach-Object { $_.VoiceInfo.Name + '|' + $_.VoiceInfo.Culture.Name }";
        let one = format!(
            "{WINRT_HELPER}\n[Windows.Media.SpeechSynthesis.SpeechSynthesizer]::AllVoices | ForEach-Object {{ $_.DisplayName + '|' + $_.Language }}"
        );
        for (kind, script) in [("onecore", one.as_str()), ("sapi", sapi)] {
            if let Ok(o) = powershell(script, &[]).await {
                for l in String::from_utf8_lossy(&o.stdout).lines() {
                    if let Some((name, lang)) = l.trim().split_once('|') {
                        let name = name.trim();
                        // The same voice in both lists: the newer one is kept.
                        let base = name.trim_end_matches(" Desktop");
                        if name.is_empty() || out.iter().any(|v: &Voice| v.name.trim_end_matches(" Desktop") == base) {
                            continue;
                        }
                        out.push(Voice { id: format!("{kind}:{name}"), name: name.to_string(), lang: lang.trim().to_string() });
                    }
                }
            }
        }
        out
    }
    #[cfg(not(windows))]
    {
        let mut out = Vec::new();
        for prog in ["espeak-ng", "espeak"] {
            if let Ok(o) = crate::load::cmd(prog).arg("--voices").output().await
                && o.status.success()
            {
                for l in String::from_utf8_lossy(&o.stdout).lines().skip(1) {
                    let f: Vec<&str> = l.split_whitespace().collect();
                    if f.len() >= 4 {
                        out.push(Voice { id: f[1].to_string(), name: f[3].replace('_', " "), lang: f[1].to_string() });
                    }
                }
                break;
            }
        }
        out
    }
}

/// Make a WAV file of the text being said.
async fn voice(ffmpeg: &str, text: &str, t: &TtsConfig, wav: &Path) -> Result<()> {
    // A downloaded voice; not here yet: fetched now, the computer's own
    // voice speaks meanwhile.
    let fallback;
    let t = if let Some(id) = t.voice.strip_prefix("piper:") {
        if crate::piper::ensure(id) {
            return crate::piper::speak(id, text, t.rate, wav).await;
        }
        fallback = TtsConfig { voice: String::new(), ..t.clone() };
        &fallback
    } else {
        t
    };
    #[cfg(windows)]
    {
        let _ = ffmpeg;
        let file = wav.display().to_string();
        // The text, voice and file name go in through the environment, never
        // into the script itself.
        if let Some(name) = t.voice.strip_prefix("onecore:") {
            let script = format!(
                "{WINRT_HELPER}
$s = New-Object Windows.Media.SpeechSynthesis.SpeechSynthesizer
$v = [Windows.Media.SpeechSynthesis.SpeechSynthesizer]::AllVoices | Where-Object {{ $_.DisplayName -eq $env:GAZE_TTS_VOICE }} | Select-Object -First 1
if ($v) {{ $s.Voice = $v }}
$s.Options.SpeakingRate = [double]$env:GAZE_TTS_RATE
$st = Await ($s.SynthesizeTextToStreamAsync($env:GAZE_TTS_TEXT)) ([Windows.Media.SpeechSynthesis.SpeechSynthesisStream])
$size = [uint32]$st.Size
$r = New-Object Windows.Storage.Streams.DataReader($st.GetInputStreamAt(0))
Await ($r.LoadAsync($size)) ([uint32]) | Out-Null
$b = New-Object byte[] $size
$r.ReadBytes($b)
[IO.File]::WriteAllBytes($env:GAZE_TTS_FILE, $b)"
            );
            let rate = 1.08f64.powi(t.rate.clamp(-10, 10));
            let out = powershell(
                &script,
                &[
                    ("GAZE_TTS_TEXT", text.to_string()),
                    ("GAZE_TTS_FILE", file.clone()),
                    ("GAZE_TTS_VOICE", name.to_string()),
                    ("GAZE_TTS_RATE", format!("{rate:.3}")),
                ],
            )
            .await?;
            if out.status.success() && wav.exists() {
                return Ok(());
            }
            tracing::warn!("Windows voice \"{name}\" failed, using the default one: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        let script = "Add-Type -AssemblyName System.Speech; \
            $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
            if ($env:GAZE_TTS_VOICE) { try { $s.SelectVoice($env:GAZE_TTS_VOICE) } catch {} }; \
            $s.Rate = [int]$env:GAZE_TTS_RATE; $s.Volume = 100; \
            $s.SetOutputToWaveFile($env:GAZE_TTS_FILE); \
            $s.Speak($env:GAZE_TTS_TEXT); $s.Dispose()";
        let out = powershell(
            script,
            &[
                ("GAZE_TTS_TEXT", text.to_string()),
                ("GAZE_TTS_FILE", file),
                ("GAZE_TTS_VOICE", t.voice.strip_prefix("sapi:").unwrap_or("").to_string()),
                ("GAZE_TTS_RATE", t.rate.clamp(-10, 10).to_string()),
            ],
        )
        .await?;
        if !out.status.success() || !wav.exists() {
            bail!(
                "Windows' voice failed: {}",
                String::from_utf8_lossy(&out.stderr).lines().find(|l| !l.trim().is_empty()).unwrap_or("no voice installed")
            );
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let speed = (175 + t.rate.clamp(-10, 10) * 12).to_string();
        for prog in ["espeak-ng", "espeak"] {
            let mut c = crate::load::cmd(prog);
            c.arg("-s").arg(&speed);
            if !t.voice.is_empty() {
                c.arg("-v").arg(&t.voice);
            }
            let r = c.arg("-w").arg(wav).arg("--").arg(text).output().await;
            if let Ok(o) = r
                && o.status.success()
                && wav.exists()
            {
                return Ok(());
            }
        }
        // ffmpeg's own voice, when it was built with it.
        let esc: String = text
            .chars()
            .map(|c| match c {
                '\'' | ':' | '\\' | ',' | ';' | '[' | ']' | '=' => ' ',
                c => c,
            })
            .collect();
        let o = crate::load::cmd(ffmpeg)
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg(format!("flite=text='{esc}'"))
            .arg(wav)
            .output()
            .await
            .context("couldn't start ffmpeg")?;
        if o.status.success() && wav.exists() {
            return Ok(());
        }
        bail!("no text-to-speech voice on this computer (install espeak-ng)")
    }
}

/// Any WAV to 8 kHz mono 16-bit samples: evened out for small speakers,
/// then at the chosen loudness.
async fn to_samples(ffmpeg: &str, wav: &Path, volume: u32) -> Result<Vec<i16>> {
    let vol = volume.clamp(10, 100) as f32 / 100.0;
    let o = crate::load::cmd(ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(wav)
        .arg("-af")
        .arg(format!("loudnorm=I=-14:TP=-1.5,aresample=8000,volume={vol:.2}"))
        .args(["-ac", "1", "-ar", "8000", "-f", "s16le", "-"])
        .output()
        .await
        .context("couldn't start ffmpeg")?;
    if !o.status.success() {
        bail!("ffmpeg: {}", String::from_utf8_lossy(&o.stderr).trim());
    }
    Ok(o.stdout.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect())
}

#[cfg(test)]
mod tests {
    #[test]
    fn wav_header() {
        let w = super::wav(&[0, 1, -1]);
        assert_eq!(w.len(), 50);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes([w[40], w[41], w[42], w[43]]), 6);
    }

    #[test]
    fn chime_is_short() {
        let c = super::chime();
        assert!(c.len() > 4000 && c.len() < 8000);
    }
}
