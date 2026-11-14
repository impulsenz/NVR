//! Text to speech for a camera's speaker: type words on the live view and
//! the camera says them. Windows' own voice (System.Speech, nothing to
//! install); elsewhere espeak-ng / espeak, or ffmpeg's flite voice.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

/// The longest text spoken at once.
pub const MAX_CHARS: usize = 400;

/// Say `text`: 8 kHz mono samples, ready for the speaker.
pub async fn speak(ffmpeg: &str, text: &str) -> Result<Vec<i16>> {
    let text: String = text.chars().filter(|c| !c.is_control()).take(MAX_CHARS).collect();
    let text = text.trim().to_string();
    if text.is_empty() {
        bail!("type something to say first");
    }
    let wav = std::env::temp_dir().join(format!("gaze-tts-{}.wav", std::process::id() ^ rand_id()));
    let made = voice(ffmpeg, &text, &wav).await;
    let out = match made {
        Ok(()) => to_samples(ffmpeg, &wav).await,
        Err(e) => Err(e),
    };
    let _ = std::fs::remove_file(&wav);
    let s = out?;
    if s.is_empty() {
        bail!("the voice made no sound");
    }
    Ok(s)
}

fn rand_id() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0)
}

/// Make a WAV file of the text being said.
async fn voice(ffmpeg: &str, text: &str, wav: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        let _ = ffmpeg;
        // The text and file name go in through the environment, never into
        // the script itself.
        let script = "Add-Type -AssemblyName System.Speech; \
            $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
            $s.Rate = 0; $s.Volume = 100; \
            $s.SetOutputToWaveFile($env:GAZE_TTS_FILE); \
            $s.Speak($env:GAZE_TTS_TEXT); $s.Dispose()";
        let out = crate::load::cmd("powershell")
            .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
            .env("GAZE_TTS_TEXT", text)
            .env("GAZE_TTS_FILE", wav)
            .output()
            .await
            .context("couldn't start Windows' voice (PowerShell)")?;
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
        for prog in ["espeak-ng", "espeak"] {
            let r = crate::load::cmd(prog).arg("-w").arg(wav).arg("--").arg(text).output().await;
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

/// Any WAV to 8 kHz mono 16-bit samples, a little louder for small speakers.
async fn to_samples(ffmpeg: &str, wav: &PathBuf) -> Result<Vec<i16>> {
    let o = crate::load::cmd(ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(wav)
        .args(["-af", "loudnorm=I=-14:TP=-1.5,aresample=8000", "-ac", "1", "-ar", "8000", "-f", "s16le", "-"])
        .output()
        .await
        .context("couldn't start ffmpeg")?;
    if !o.status.success() {
        bail!("ffmpeg: {}", String::from_utf8_lossy(&o.stderr).trim());
    }
    Ok(o.stdout.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect())
}
