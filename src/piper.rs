//! Extra voices: Piper, a free neural text-to-speech engine that runs on
//! this computer (github.com/rhasspy/piper, MIT licence; voices from
//! huggingface.co/rhasspy/piper-voices). Downloaded on request (the engine
//! about 20 MB once, each voice about 60 MB), kept under nvr-data/voices.

use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const ENGINE: &str = "https://github.com/rhasspy/piper/releases/download/2023.11.14-2/";
const VOICE_BASE: &str = "https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/";

/// (id, name, language, path on the voice site without the extension)
pub const CATALOG: &[(&str, &str, &str, &str)] = &[
    ("en_GB-alan-medium", "Alan (British man)", "en-GB", "en/en_GB/alan/medium/en_GB-alan-medium"),
    ("en_GB-jenny_dioco-medium", "Jenny (British woman)", "en-GB", "en/en_GB/jenny_dioco/medium/en_GB-jenny_dioco-medium"),
    ("en_GB-cori-high", "Cori (British woman, high quality)", "en-GB", "en/en_GB/cori/high/en_GB-cori-high"),
    ("en_GB-northern_english_male-medium", "Northern English man", "en-GB", "en/en_GB/northern_english_male/medium/en_GB-northern_english_male-medium"),
    ("en_US-lessac-medium", "Lessac (American woman)", "en-US", "en/en_US/lessac/medium/en_US-lessac-medium"),
    ("en_US-amy-medium", "Amy (American woman)", "en-US", "en/en_US/amy/medium/en_US-amy-medium"),
    ("en_US-hfc_female-medium", "HFC (American woman)", "en-US", "en/en_US/hfc_female/medium/en_US-hfc_female-medium"),
    ("en_US-ryan-medium", "Ryan (American man)", "en-US", "en/en_US/ryan/medium/en_US-ryan-medium"),
    ("en_US-joe-medium", "Joe (American man)", "en-US", "en/en_US/joe/medium/en_US-joe-medium"),
    ("en_US-hfc_male-medium", "HFC (American man)", "en-US", "en/en_US/hfc_male/medium/en_US-hfc_male-medium"),
    ("de_DE-thorsten-medium", "Thorsten (German)", "de-DE", "de/de_DE/thorsten/medium/de_DE-thorsten-medium"),
    ("fr_FR-siwis-medium", "Siwis (French)", "fr-FR", "fr/fr_FR/siwis/medium/fr_FR-siwis-medium"),
    ("es_ES-davefx-medium", "Davefx (Spanish)", "es-ES", "es/es_ES/davefx/medium/es_ES-davefx-medium"),
];

static DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn init(data_dir: &Path) {
    let _ = DIR.set(std::path::absolute(data_dir.join("voices")).unwrap_or(data_dir.join("voices")));
}

fn dir() -> PathBuf {
    DIR.get().cloned().unwrap_or_else(|| PathBuf::from("nvr-data/voices"))
}

fn exe() -> PathBuf {
    dir().join("piper").join(if cfg!(windows) { "piper.exe" } else { "piper" })
}

fn model(id: &str) -> PathBuf {
    dir().join(format!("{id}.onnx"))
}

#[derive(Serialize)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub lang: String,
    pub installed: bool,
    pub downloading: bool,
}

static BUSY: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

pub fn installed(id: &str) -> bool {
    exe().exists() && model(id).exists() && model(id).with_extension("onnx.json").exists()
}

pub fn catalog() -> Vec<Entry> {
    let busy = BUSY.lock().unwrap().clone();
    CATALOG
        .iter()
        .map(|(id, name, lang, _)| Entry {
            id: id.to_string(),
            name: name.to_string(),
            lang: lang.to_string(),
            installed: installed(id),
            downloading: busy.iter().any(|b| b == id),
        })
        .collect()
}

/// Download the engine (first time) and a voice, as a task with progress.
pub async fn download(id: &str) -> Result<()> {
    let Some((_, name, _, path)) = CATALOG.iter().find(|v| v.0 == id) else { bail!("unknown voice") };
    {
        let mut b = BUSY.lock().unwrap();
        if b.iter().any(|x| x == id) {
            bail!("already downloading");
        }
        b.push(id.to_string());
    }
    let res = async {
        std::fs::create_dir_all(dir())?;
        if !exe().exists() {
            let (file, n) = if cfg!(windows) { ("piper_windows_amd64.zip", 10_000_000) } else { ("piper_linux_x86_64.tar.gz", 10_000_000) };
            let arc = dir().join(file);
            crate::jobs::download("the voice engine (Piper)", &format!("{ENGINE}{file}"), &arc, n).await?;
            let (a, d) = (arc.clone(), dir());
            tokio::task::spawn_blocking(move || unpack(&a, &d)).await??;
            let _ = std::fs::remove_file(&arc);
            if !exe().exists() {
                bail!("the voice engine wasn't in the download");
            }
        }
        let m = model(id);
        crate::jobs::download(&format!("the voice {name}"), &format!("{VOICE_BASE}{path}.onnx.json"), &m.with_extension("onnx.json"), 500).await?;
        crate::jobs::download(&format!("the voice {name}"), &format!("{VOICE_BASE}{path}.onnx"), &m, 5_000_000).await?;
        anyhow::Ok(())
    }
    .await;
    BUSY.lock().unwrap().retain(|x| x != id);
    res
}

fn unpack(archive: &Path, to: &Path) -> Result<()> {
    if archive.extension().is_some_and(|e| e == "zip") {
        let mut z = zip::ZipArchive::new(std::fs::File::open(archive)?).context("reading the voice engine download")?;
        z.extract(to).context("unpacking the voice engine")?;
    } else {
        let ok = std::process::Command::new("tar").arg("-xzf").arg(archive).arg("-C").arg(to).status()?.success();
        if !ok {
            bail!("unpacking the voice engine failed");
        }
    }
    Ok(())
}

pub fn delete(id: &str) -> Result<()> {
    if !CATALOG.iter().any(|v| v.0 == id) {
        bail!("unknown voice");
    }
    let m = model(id);
    let _ = std::fs::remove_file(m.with_extension("onnx.json"));
    std::fs::remove_file(&m).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) })?;
    Ok(())
}

/// Say `text` with a downloaded voice into a WAV file. `rate`: -10..10.
pub async fn speak(id: &str, text: &str, rate: i32, wav: &Path) -> Result<()> {
    if !installed(id) {
        bail!("the voice {id} isn't downloaded (Settings → Voice & talking)");
    }
    // Longer = slower.
    let length = 1.0 / 1.08f64.powi(rate.clamp(-10, 10));
    let mut child = crate::load::cmd(exe())
        .current_dir(exe().parent().unwrap_or(Path::new(".")))
        .arg("--model")
        .arg(model(id))
        .arg("--output_file")
        .arg(wav)
        .arg("--length_scale")
        .arg(format!("{length:.3}"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("couldn't start the voice engine")?;
    if let Some(mut i) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        i.write_all(text.replace('\n', " ").as_bytes()).await?;
        i.write_all(b"\n").await?;
    }
    let out = tokio::time::timeout(std::time::Duration::from_secs(60), child.wait_with_output())
        .await
        .context("the voice engine took too long")??;
    if !out.status.success() || !wav.exists() {
        bail!("the voice engine failed: {}", String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or(""));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn catalog_ids_match_paths() {
        for (id, _, _, path) in super::CATALOG {
            assert!(path.ends_with(id), "{id}");
        }
    }
}
