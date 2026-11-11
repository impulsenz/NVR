//! Where the AI models run: on the graphics card (ONNX Runtime with
//! DirectML, which works with any DirectX 12 GPU on Windows: NVIDIA, AMD,
//! Intel), or on the processor (tract, built in).
//!
//! The GPU is used unless the settings say "CPU" (Settings → Object
//! recognition → Run AI on). The GPU runtime (Microsoft's ONNX Runtime with
//! DirectML, about 25 MB) is downloaded the first time from NuGet. Models
//! that can't run on the GPU (no DirectX 12 GPU, runtime missing, a model
//! the GPU can't handle) run on the processor, and the reason is shown.

use anyhow::{Context, Result, anyhow, bail};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// GPU runtime versions, for the log and the download.
#[allow(dead_code)]
const _VERSIONS: (&str, &str) = (ORT_VERSION, DML_VERSION);
use tracing::{info, warn};

pub type Plan = Arc<tract_onnx::prelude::TypedSimplePlan>;

/// ONNX Runtime with DirectML, from NuGet (the last version with DirectML).
const ORT_VERSION: &str = "1.24.4";
const DML_VERSION: &str = "1.15.4";

/// Run AI on the GPU (unless the settings say CPU).
static USE_GPU: AtomicBool = AtomicBool::new(true);
/// The GPU runtime, once loaded.
static RUNTIME: OnceLock<ort::environment::Environment> = OnceLock::new();
/// Why the GPU runtime couldn't be loaded (tried again later).
static RUNTIME_ERR: Mutex<Option<String>> = Mutex::new(None);
/// The GPU runtime is wanted (at start-up, or the settings were changed to GPU).
pub static GPU_WANTED: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// Background: make the GPU runtime ready when wanted, then load the AI
/// models again so they run on it.
pub async fn run(mgr: Arc<crate::manager::Manager>) {
    loop {
        if use_gpu() && RUNTIME.get().is_none() && prepare(&mgr.data_dir).await {
            mgr.reload_models();
        }
        // Tried and failed (e.g. no internet): try again in an hour, or when asked.
        tokio::select! {
            _ = GPU_WANTED.notified() => {}
            _ = tokio::time::sleep(std::time::Duration::from_secs(3600)) => {}
        }
    }
}

/// One preparation at a time.
static PREPARING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
/// For the settings page: what runs where.
static STATUS: Mutex<Option<String>> = Mutex::new(None);

pub fn set_use_gpu(on: bool) {
    USE_GPU.store(on, Ordering::Relaxed);
}

pub fn use_gpu() -> bool {
    USE_GPU.load(Ordering::Relaxed)
}

/// What the settings page shows ("GPU (DirectML)", or why it's the CPU).
pub fn status() -> String {
    if !use_gpu() {
        return "CPU (chosen in the settings)".into();
    }
    if let Some(s) = STATUS.lock().unwrap().clone() {
        return s;
    }
    if RUNTIME.get().is_some() {
        return "GPU (DirectML)".into();
    }
    match RUNTIME_ERR.lock().unwrap().clone() {
        Some(e) => format!("CPU — the GPU can't be used: {e}"),
        None => "CPU until the GPU runtime is ready (downloading it the first time)".into(),
    }
}

fn note(s: String) {
    *STATUS.lock().unwrap() = Some(s);
}

/// One input for a model: its shape and data.
pub struct Input {
    pub shape: Vec<usize>,
    pub data: Data,
}

pub enum Data {
    F32(Vec<f32>),
    I64(Vec<i64>),
    U8(Vec<u8>),
}

impl Input {
    pub fn f32(shape: &[usize], data: Vec<f32>) -> Self {
        Self {
            shape: shape.to_vec(),
            data: Data::F32(data),
        }
    }
    pub fn i64(shape: &[usize], data: Vec<i64>) -> Self {
        Self {
            shape: shape.to_vec(),
            data: Data::I64(data),
        }
    }
    pub fn u8(shape: &[usize], data: Vec<u8>) -> Self {
        Self {
            shape: shape.to_vec(),
            data: Data::U8(data),
        }
    }
}

/// A loaded model.
pub enum Net {
    Cpu(Plan),
    Gpu(Mutex<ort::session::Session>),
}

impl Net {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn on_gpu(&self) -> bool {
        matches!(self, Net::Gpu(_))
    }

    /// Run with one input; every output comes back as f32 numbers.
    pub fn run(&self, input: Input) -> Result<Vec<Vec<f32>>> {
        match self {
            Net::Cpu(plan) => run_tract(plan, input),
            Net::Gpu(s) => run_ort(&mut s.lock().unwrap(), input),
        }
    }
}

fn run_tract(plan: &Plan, input: Input) -> Result<Vec<Vec<f32>>> {
    use tract_onnx::prelude::*;
    let t: Tensor = match input.data {
        Data::F32(d) => Tensor::from_shape(&input.shape, &d),
        Data::I64(d) => Tensor::from_shape(&input.shape, &d),
        Data::U8(d) => Tensor::from_shape(&input.shape, &d),
    }
    .map_err(|e| anyhow!("{e}"))?;
    let out = plan.run(tvec!(t.into())).map_err(|e| anyhow!("{e}"))?;
    out.iter()
        .map(|o| {
            Ok(o.cast_to::<f32>()
                .map_err(|e| anyhow!("{e}"))?
                .to_plain_array_view::<f32>()
                .map_err(|e| anyhow!("{e}"))?
                .iter()
                .copied()
                .collect())
        })
        .collect()
}

fn run_ort(s: &mut ort::session::Session, input: Input) -> Result<Vec<Vec<f32>>> {
    use ort::value::Tensor;
    let shape: Vec<i64> = input.shape.iter().map(|&x| x as i64).collect();
    let value: ort::session::SessionInputValue = match input.data {
        Data::F32(d) => Tensor::from_array((shape, d))?.into(),
        Data::I64(d) => Tensor::from_array((shape, d))?.into(),
        Data::U8(d) => Tensor::from_array((shape, d))?.into(),
    };
    let outputs = s.run([value])?;
    let mut res = Vec::new();
    for (_, v) in outputs.iter() {
        let data: Vec<f32> = if let Ok((_, d)) = v.try_extract_tensor::<f32>() {
            d.to_vec()
        } else if let Ok((_, d)) = v.try_extract_tensor::<i64>() {
            d.iter().map(|&x| x as f32).collect()
        } else if let Ok((_, d)) = v.try_extract_tensor::<i32>() {
            d.iter().map(|&x| x as f32).collect()
        } else if let Ok((_, d)) = v.try_extract_tensor::<u8>() {
            d.iter().map(|&x| x as f32).collect()
        } else {
            bail!("unexpected kind of model output");
        };
        res.push(data);
    }
    Ok(res)
}

/// Load a model: on the GPU when that's wanted and works, otherwise on the
/// processor with `cpu` (tract).
pub fn load(path: &Path, what: &str, cpu: impl FnOnce() -> Result<Plan>) -> Result<Net> {
    if use_gpu()
        && let Some(env) = RUNTIME.get()
    {
        match gpu_session(env, path) {
            Ok(s) => {
                info!("AI: {what} runs on the GPU");
                record(what, true, String::new());
                return Ok(Net::Gpu(Mutex::new(s)));
            }
            Err(e) => {
                warn!("AI: {what} can't run on the GPU ({e:#}); using the processor");
                note(format!("GPU (DirectML), but {what} runs on the CPU: {e:#}"));
                record(
                    what,
                    false,
                    format!("the GPU couldn't run this model: {e:#}"),
                );
                return Ok(Net::Cpu(cpu()?));
            }
        }
    }
    let why = if !use_gpu() {
        "processor chosen in the settings".to_string()
    } else {
        match RUNTIME_ERR.lock().unwrap().clone() {
            Some(e) => format!("the GPU can't be used: {e}"),
            None => {
                "the GPU runtime wasn't ready yet when it loaded (it moves to the GPU once ready)"
                    .into()
            }
        }
    };
    record(what, false, why);
    Ok(Net::Cpu(cpu()?))
}

/// Each model's place (for Settings → Tasks → What runs where).
#[derive(Clone, serde::Serialize)]
pub struct Placed {
    pub what: String,
    pub gpu: bool,
    pub why: String,
}

static PLACED: Mutex<Vec<Placed>> = Mutex::new(Vec::new());

fn record(what: &str, gpu: bool, why: String) {
    let mut p = PLACED.lock().unwrap();
    p.retain(|x| x.what != what);
    p.push(Placed {
        what: what.to_string(),
        gpu,
        why,
    });
}

/// Models loaded so far and where they run.
pub fn placed() -> Vec<Placed> {
    PLACED.lock().unwrap().clone()
}

fn gpu_session(env: &ort::environment::Environment, path: &Path) -> Result<ort::session::Session> {
    use ort::session::{Session, builder::GraphOptimizationLevel};
    let builder = Session::builder(env)?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| anyhow!("{e}"))?;
    // Testing on a computer without DirectML: GAZE_ORT_CPU=1 runs the same
    // path on ONNX Runtime's processor backend.
    let mut builder = if std::env::var_os("GAZE_ORT_CPU").is_some() {
        builder
    } else {
        // DirectML needs these two off.
        builder
            .with_memory_pattern(false)
            .map_err(|e| anyhow!("{e}"))?
            .with_parallel_execution(false)
            .map_err(|e| anyhow!("{e}"))?
            .with_execution_providers([ort::ep::DirectML::default().build().error_on_failure()])
            .map_err(|e| anyhow!("{e}"))?
    };
    Ok(builder.commit_from_file(path)?)
}

/// Make the GPU runtime ready (download it the first time). Returns whether
/// it's usable; models loaded before this ran on the processor.
pub async fn prepare(data_dir: &Path) -> bool {
    let _one = PREPARING.lock().await;
    if RUNTIME.get().is_some() {
        return true;
    }
    let res = async {
        let lib = runtime_files(data_dir).await?;
        let lib2 = lib.clone();
        let env = tokio::task::spawn_blocking(move || -> Result<ort::environment::Environment> {
            #[cfg(windows)]
            preload_directml(&lib2)?;
            Ok(ort::init_from(&lib2)
                .map_err(|e| anyhow!("{e}"))?
                .with_name("GAZE")
                .build()?)
        })
        .await??;
        anyhow::Ok(env)
    }
    .await;
    match res {
        Ok(env) => {
            info!("AI: GPU runtime ready (ONNX Runtime {ORT_VERSION} with DirectML)");
            *RUNTIME_ERR.lock().unwrap() = None;
            *STATUS.lock().unwrap() = None;
            let _ = RUNTIME.set(env);
            true
        }
        Err(e) => {
            warn!("AI: the GPU can't be used ({e:#}); AI runs on the processor");
            *RUNTIME_ERR.lock().unwrap() = Some(format!("{e:#}"));
            false
        }
    }
}

/// The runtime library (downloaded and unpacked the first time).
async fn runtime_files(data_dir: &Path) -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("GAZE_ORT_DYLIB") {
        return Ok(PathBuf::from(p)); // testing: a runtime library given by hand
    }
    if !cfg!(all(windows, target_arch = "x86_64")) {
        bail!("GPU AI is only set up for Windows (64-bit)");
    }
    let dir = data_dir
        .join("runtime")
        .join(format!("onnxruntime-dml-{ORT_VERSION}"));
    let ort_dll = dir.join("onnxruntime.dll");
    let dml_dll = dir.join("DirectML.dll");
    if ort_dll.exists() && dml_dll.exists() {
        return Ok(ort_dll);
    }
    std::fs::create_dir_all(&dir)?;
    for (pkg, ver, inside, out) in [
        (
            "microsoft.ml.onnxruntime.directml",
            ORT_VERSION,
            "runtimes/win-x64/native/onnxruntime.dll",
            &ort_dll,
        ),
        (
            "microsoft.ai.directml",
            DML_VERSION,
            "bin/x64-win/DirectML.dll",
            &dml_dll,
        ),
    ] {
        if out.exists() {
            continue;
        }
        let url = format!("https://api.nuget.org/v3-flatcontainer/{pkg}/{ver}/{pkg}.{ver}.nupkg");
        let nupkg = dir.join(format!("{pkg}.nupkg"));
        crate::jobs::download("the GPU runtime for AI", &url, &nupkg, 1_000_000).await?;
        let (nupkg2, inside, out2) = (nupkg.clone(), inside.to_string(), out.clone());
        tokio::task::spawn_blocking(move || extract(&nupkg2, &inside, &out2)).await??;
        let _ = std::fs::remove_file(&nupkg);
    }
    Ok(ort_dll)
}

/// One file out of a NuGet package (a zip).
fn extract(zip_path: &Path, inside: &str, out: &Path) -> Result<()> {
    let mut z = zip::ZipArchive::new(std::fs::File::open(zip_path)?)
        .context("reading the downloaded package")?;
    let mut f = z
        .by_name(inside)
        .with_context(|| format!("{inside} is not in the package"))?;
    let tmp = out.with_extension("part");
    let mut w = std::fs::File::create(&tmp)?;
    std::io::copy(&mut f, &mut w)?;
    drop(w);
    std::fs::rename(&tmp, out)?;
    Ok(())
}

/// ONNX Runtime needs this DirectML version; Windows has an older one of
/// its own. Loading ours first makes ONNX Runtime use it.
#[cfg(windows)]
fn preload_directml(ort_dll: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let dml = ort_dll.with_file_name("DirectML.dll");
    let w: Vec<u16> = dml.as_os_str().encode_wide().chain(Some(0)).collect();
    let h = unsafe { windows_sys::Win32::System::LibraryLoader::LoadLibraryW(w.as_ptr()) };
    if h.is_null() {
        bail!(
            "could not load {}: {}",
            dml.display(),
            std::io::Error::last_os_error()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tract_plan(path: &Path, fact: Option<(&[usize], bool)>) -> Result<Plan> {
        use tract_onnx::prelude::*;
        let mut m = tract_onnx::onnx()
            .with_ignore_value_info(true)
            .with_ignore_output_shapes(true)
            .model_for_path(path)
            .map_err(|e| anyhow!("{e}"))?;
        if let Some((shape, bytes)) = fact {
            m = if bytes {
                m.with_input_fact(0, u8::fact(shape).into())
            } else {
                m.with_input_fact(0, f32::fact(shape).into())
            }
            .map_err(|e| anyhow!("{e}"))?;
        }
        m.into_optimized()
            .map_err(|e| anyhow!("{e}"))?
            .into_runnable()
            .map_err(|e| anyhow!("{e}"))
    }

    fn both(path: &Path, fact: Option<(&[usize], bool)>) -> (Net, Net) {
        set_use_gpu(true);
        let a = load(path, "test", || tract_plan(path, fact)).unwrap();
        set_use_gpu(false);
        let b = load(path, "test", || tract_plan(path, fact)).unwrap();
        set_use_gpu(true);
        assert!(a.on_gpu() && !b.on_gpu());
        (a, b)
    }

    fn close(a: &[Vec<f32>], b: &[Vec<f32>], what: &str) {
        assert_eq!(a.len(), b.len(), "{what}: number of outputs");
        for (x, y) in a.iter().zip(b) {
            assert_eq!(x.len(), y.len(), "{what}: output size");
            let worst = x
                .iter()
                .zip(y)
                .map(|(p, q)| (p - q).abs() / (1.0 + q.abs()))
                .fold(0f32, f32::max);
            assert!(worst < 2e-3, "{what}: outputs differ by {worst}");
        }
        eprintln!("{what}: {} output(s) match", a.len());
    }

    /// The ONNX Runtime path gives the same answers as tract. Needs
    /// GAZE_ORT_DYLIB (an ONNX Runtime library) and NVR_TEST_MODELS (a folder
    /// with yolox_tiny.onnx and the downloaded GAZE models); on a computer
    /// without DirectML also GAZE_ORT_CPU=1.
    #[tokio::test]
    async fn onnx_runtime_matches_tract() {
        let (Some(_), Ok(dir)) = (
            std::env::var_os("GAZE_ORT_DYLIB"),
            std::env::var("NVR_TEST_MODELS"),
        ) else {
            return;
        };
        let dir = PathBuf::from(dir);
        assert!(prepare(&dir).await, "runtime: {}", status());
        let pic: Vec<f32> = (0..3 * 416 * 416)
            .map(|i| ((i * 37) % 255) as f32)
            .collect();
        let (g, c) = both(
            &dir.join("yolox_tiny.onnx"),
            Some((&[1, 3, 416, 416], false)),
        );
        close(
            &g.run(Input::f32(&[1, 3, 416, 416], pic.clone())).unwrap(),
            &c.run(Input::f32(&[1, 3, 416, 416], pic)).unwrap(),
            "object detection",
        );
        let img: Vec<f32> = (0..3 * 224 * 224)
            .map(|i| ((i % 97) as f32 / 97.0) - 0.5)
            .collect();
        let (g, c) = both(
            &dir.join("clip-vit-b32-visual.onnx"),
            Some((&[1, 3, 224, 224], false)),
        );
        close(
            &g.run(Input::f32(&[1, 3, 224, 224], img.clone())).unwrap(),
            &c.run(Input::f32(&[1, 3, 224, 224], img)).unwrap(),
            "smart search pictures",
        );
        let ids: Vec<i64> = (0..77)
            .map(|i| {
                if i == 0 {
                    49406
                } else if i < 6 {
                    320 + i
                } else if i == 6 {
                    49407
                } else {
                    0
                }
            })
            .collect();
        set_use_gpu(true);
        let g = load(
            &dir.join("clip-vit-b32-textual.onnx"),
            "test",
            || unreachable!(),
        )
        .unwrap();
        let c = Net::Cpu({
            use tract_onnx::prelude::*;
            tract_onnx::onnx()
                .model_for_path(dir.join("clip-vit-b32-textual.onnx"))
                .unwrap()
                .with_input_fact(0, i64::fact([1, 77]).into())
                .unwrap()
                .into_optimized()
                .unwrap()
                .into_runnable()
                .unwrap()
        });
        close(
            &g.run(Input::i64(&[1, 77], ids.clone())).unwrap(),
            &c.run(Input::i64(&[1, 77], ids)).unwrap(),
            "smart search words",
        );
        let face: Vec<f32> = (0..3 * 640 * 640)
            .map(|i| ((i * 13) % 255) as f32)
            .collect();
        let (g, c) = both(&dir.join("face-yunet-2023mar.onnx"), None);
        close(
            &g.run(Input::f32(&[1, 3, 640, 640], face.clone())).unwrap(),
            &c.run(Input::f32(&[1, 3, 640, 640], face)).unwrap(),
            "faces",
        );
        let plate: Vec<u8> = (0..64 * 128 * 3).map(|i| (i % 251) as u8).collect();
        let (g, c) = both(
            &dir.join("plate-ocr-cct-xs-global.onnx"),
            Some((&[1, 64, 128, 3], true)),
        );
        close(
            &g.run(Input::u8(&[1, 64, 128, 3], plate.clone())).unwrap(),
            &c.run(Input::u8(&[1, 64, 128, 3], plate)).unwrap(),
            "number plates",
        );
    }
}
