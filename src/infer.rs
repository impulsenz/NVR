//! Where the AI models run: on an accelerator through ONNX Runtime, or on the
//! processor (tract, built in).
//!
//! Accelerators (`Accel`): **DirectML** on Windows (any DirectX 12 GPU),
//! **CUDA** on Linux with an NVIDIA card (needs the CUDA 12 and cuDNN 9
//! libraries installed on the computer), **OpenVINO** on Linux (Intel
//! graphics, NPU, or just the Intel/AMD processor, which it runs faster than
//! tract). Each needs its own ONNX Runtime build, downloaded the first time
//! (DirectML from NuGet; the other two from PyPI wheels pinned by SHA-256).
//! One runtime per process: switching accelerator takes a restart.
//!
//! The accelerator is used unless the settings say "CPU" (Settings → Object
//! recognition → Run AI on). Models that can't run on it (runtime missing,
//! no suitable hardware, a model it can't handle) run on the processor, and
//! the reason is shown.

use crate::sync::MutexExt;
use anyhow::{Context, Result, anyhow, bail};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use tracing::{info, warn};

pub type Plan = Arc<tract_onnx::prelude::TypedSimplePlan>;

/// ONNX Runtime with DirectML, from NuGet (the last version with DirectML).
const ORT_VERSION: &str = "1.24.4";
const DML_VERSION: &str = "1.15.4";
/// SHA-256 of the two NuGet packages (they match the SHA-512 NuGet itself
/// publishes for these versions).
const ORT_DML_SHA256: &str = "57e9f11b73437bef7a309496135d4c1f96b1a8e9ddba60013fa27bfc1d788681";
const DML_SHA256: &str = "4e7cb7ddce8cf837a7a75dc029209b520ca0101470fcdf275c1f49736a3615b9";
/// ONNX Runtime for Linux, from PyPI wheels (only the native libraries are
/// used; the wheel is a zip). Pinned: the address and the SHA-256.
const LINUX_ORT_VERSION: &str = "1.24.1";
const OPENVINO_WHEEL: (&str, &str) = (
    "https://files.pythonhosted.org/packages/50/cf/17ba72de2df0fcba349937d2788f154397bbc2d1a2d67772a97e26f6bc5f/onnxruntime_openvino-1.24.1-cp312-cp312-manylinux_2_28_x86_64.whl",
    "d617fac2f59a6ab5ea59a788c3e1592240a129642519aaeaa774761dfe35150e",
);
const CUDA_WHEEL: (&str, &str) = (
    "https://files.pythonhosted.org/packages/db/a8/fb1a36a052321a839cc9973f6cfd630709412a24afff2d7315feb3efc4b8/onnxruntime_gpu-1.24.1-cp312-cp312-manylinux_2_27_x86_64.manylinux_2_28_x86_64.whl",
    "710bf83751e6761584ad071102af3cbffd4b42bb77b2e3caacfb54ffbaa0666b",
);

/// The ways to run a model faster than the processor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Accel {
    DirectMl,
    Cuda,
    OpenVino,
}

impl Accel {
    pub fn name(self) -> &'static str {
        match self {
            Accel::DirectMl => "directml",
            Accel::Cuda => "cuda",
            Accel::OpenVino => "openvino",
        }
    }
    fn label(self) -> String {
        match self {
            Accel::DirectMl => "GPU (DirectML)".into(),
            Accel::Cuda => "NVIDIA GPU (CUDA)".into(),
            Accel::OpenVino => format!("OpenVINO ({})", openvino_device()),
        }
    }
}

/// What this computer can offer in the settings (besides "auto").
pub fn choices() -> Vec<&'static str> {
    if cfg!(all(windows, target_arch = "x86_64")) {
        vec![Accel::DirectMl.name()]
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        vec![Accel::Cuda.name(), Accel::OpenVino.name()]
    } else {
        vec![]
    }
}

/// Is a CUDA 12 + cuDNN 9 installation findable on this computer?
fn cuda_libraries_present() -> bool {
    let mut dirs: Vec<PathBuf> = std::env::var("LD_LIBRARY_PATH").unwrap_or_default().split(':').filter(|d| !d.is_empty()).map(PathBuf::from).collect();
    for d in ["/usr/lib/x86_64-linux-gnu", "/usr/lib64", "/usr/lib", "/usr/local/cuda/lib64", "/usr/local/cuda/targets/x86_64-linux/lib", "/lib/x86_64-linux-gnu"] {
        dirs.push(PathBuf::from(d));
    }
    let has = |name: &str| dirs.iter().any(|d| d.join(name).exists());
    Path::new("/proc/driver/nvidia/version").exists() && has("libcudnn.so.9") && has("libcublas.so.12")
}

/// Which accelerator a setting means on this computer. `auto`: DirectML on
/// Windows; on Linux CUDA when an NVIDIA card with its libraries is there,
/// otherwise OpenVINO.
pub fn resolve(setting: &str) -> Result<Accel> {
    let linux64 = cfg!(all(target_os = "linux", target_arch = "x86_64"));
    let win64 = cfg!(all(windows, target_arch = "x86_64"));
    match setting {
        "directml" if win64 => Ok(Accel::DirectMl),
        "directml" => bail!("DirectML is only available on Windows"),
        "cuda" if linux64 => Ok(Accel::Cuda),
        "cuda" => bail!("CUDA support is only set up for Linux (64-bit)"),
        "openvino" if linux64 => Ok(Accel::OpenVino),
        "openvino" => bail!("OpenVINO support is only set up for Linux (64-bit)"),
        _ if win64 => Ok(Accel::DirectMl),
        _ if linux64 => Ok(if cuda_libraries_present() { Accel::Cuda } else { Accel::OpenVino }),
        _ => bail!("GPU AI is only set up for Windows and 64-bit Linux"),
    }
}

/// Run AI on the GPU (unless the settings say CPU).
static USE_GPU: AtomicBool = AtomicBool::new(true);
/// The GPU runtime, once loaded, and which accelerator it is for.
static RUNTIME: OnceLock<ort::environment::Environment> = OnceLock::new();
static LOADED: OnceLock<Accel> = OnceLock::new();
/// What the settings ask for: accelerator ("auto", "directml", "cuda",
/// "openvino") and the OpenVINO device ("AUTO:GPU,CPU", "GPU", "CPU", "NPU").
static WANT: Mutex<(String, String)> = Mutex::new((String::new(), String::new()));
/// Where models are cached (OpenVINO compiles a model for the device once).
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();
/// Why the GPU runtime couldn't be loaded (tried again later).
static RUNTIME_ERR: Mutex<Option<String>> = Mutex::new(None);
/// When Windows last reset the graphics card under a running model ("device
/// removed / suspended", e.g. a driver timeout). That model can't be used
/// again, so everything runs on the processor for a while, then the GPU is
/// tried again.
static GPU_LOST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
static GPU_LOST_NOTE: tokio::sync::Notify = tokio::sync::Notify::const_new();
const GPU_RETRY: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// Is the GPU to be used now (not right after it was reset)?
fn gpu_ok() -> bool {
    GPU_LOST.locked().is_none_or(|t| t.elapsed() > GPU_RETRY)
}

/// Does this error say the graphics card was reset or removed?
fn device_lost(msg: &str) -> bool {
    let m = msg.to_lowercase();
    ["887a0005", "887a0006", "887a0007", "device instance has been suspended", "device removed", "getdeviceremovedreason", "dxgi_error_device"].iter().any(|k| m.contains(k))
}

fn mark_gpu_lost(why: &str) {
    let mut l = GPU_LOST.locked();
    if l.is_some_and(|t| t.elapsed() < GPU_RETRY) {
        return;
    }
    *l = Some(std::time::Instant::now());
    drop(l);
    warn!("AI: the graphics card was reset ({why}); the AI runs on the processor for 30 minutes, then tries the GPU again");
    GPU_LOST_NOTE.notify_one();
}

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
            _ = GPU_LOST_NOTE.notified() => {
                // Models on the reset GPU are dead: load them again (now on
                // the processor), and once more on the GPU after the pause.
                mgr.reload_models();
                let m = mgr.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(GPU_RETRY + std::time::Duration::from_secs(5)).await;
                    if gpu_ok() {
                        m.reload_models();
                    }
                });
            }
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

/// Which accelerator and OpenVINO device the settings ask for.
pub fn set_accelerator(accel: &str, openvino_device: &str) {
    *WANT.locked() = (accel.to_string(), openvino_device.to_string());
}

fn wanted() -> Result<Accel> {
    let a = WANT.locked().0.clone();
    resolve(&a)
}

/// The OpenVINO device in use ("AUTO:GPU,CPU": the graphics if there is
/// one, else the processor).
fn openvino_device() -> String {
    let d = WANT.locked().1.trim().to_string();
    if d.is_empty() { "AUTO:GPU,CPU".into() } else { d }
}

pub fn use_gpu() -> bool {
    USE_GPU.load(Ordering::Relaxed)
}

/// What the settings page shows ("GPU (DirectML)", or why it's the CPU).
pub fn status() -> String {
    if !use_gpu() {
        return "CPU (chosen in the settings)".into();
    }
    if !gpu_ok() {
        return "CPU — the graphics card was reset by Windows (driver timeout); the GPU is tried again later".into();
    }
    if let Some(s) = STATUS.locked().clone() {
        return s;
    }
    if let Some(loaded) = LOADED.get() {
        // A different accelerator was chosen after this one loaded: it
        // can't be swapped in a running process.
        if let Ok(w) = wanted()
            && w != *loaded
        {
            return format!("{} — restart GAZE to use {}", loaded.label(), w.label());
        }
        return loaded.label();
    }
    match RUNTIME_ERR.locked().clone() {
        Some(e) => format!("CPU — the accelerator can't be used: {e}"),
        None => "CPU until the AI runtime is ready (downloading it the first time)".into(),
    }
}

fn note(s: String) {
    *STATUS.locked() = Some(s);
}

/// One input for a model: its shape and data.
pub struct Input {
    pub shape: Vec<usize>,
    pub data: Data,
}

pub enum Data {
    F32(Vec<f32>),
    U8(Vec<u8>),
    I64(Vec<i64>),
}

impl Input {
    pub fn f32(shape: &[usize], data: Vec<f32>) -> Self {
        Self { shape: shape.to_vec(), data: Data::F32(data) }
    }
    pub fn i64(shape: &[usize], data: Vec<i64>) -> Self {
        Self { shape: shape.to_vec(), data: Data::I64(data) }
    }
    pub fn u8(shape: &[usize], data: Vec<u8>) -> Self {
        Self { shape: shape.to_vec(), data: Data::U8(data) }
    }
}

/// A loaded model.
pub enum Net {
    Cpu(Plan),
    Gpu(Mutex<ort::session::Session>),
}

impl Net {
    /// Loaded somewhere other than where it should run now: on the
    /// processor while the GPU has become ready (it was loaded before the
    /// GPU runtime), or on the GPU after the processor was chosen. Kept
    /// models check this and load again.
    pub fn misplaced(&self) -> bool {
        match self {
            Net::Cpu(_) => use_gpu() && gpu_ok() && RUNTIME.get().is_some() && RUNTIME_ERR.locked().is_none(),
            Net::Gpu(_) => !use_gpu() || !gpu_ok(),
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn on_gpu(&self) -> bool {
        matches!(self, Net::Gpu(_))
    }

    /// Run with one input; every output comes back as f32 numbers.
    pub fn run(&self, input: Input) -> Result<Vec<Vec<f32>>> {
        match self {
            Net::Cpu(plan) => run_tract(plan, input),
            Net::Gpu(s) => {
                let r = run_ort(&mut s.locked(), input);
                if let Err(e) = &r {
                    let t = format!("{e:#}");
                    if device_lost(&t) {
                        mark_gpu_lost(&t);
                    }
                }
                r
            }
        }
    }
}

fn run_tract(plan: &Plan, input: Input) -> Result<Vec<Vec<f32>>> {
    use tract_onnx::prelude::*;
    let t: Tensor = match input.data {
        Data::F32(d) => Tensor::from_shape(&input.shape, &d),
        Data::U8(d) => Tensor::from_shape(&input.shape, &d),
        Data::I64(d) => Tensor::from_shape(&input.shape, &d),
    }
    .map_err(|e| anyhow!("{e}"))?;
    let out = plan.run(tvec!(t.into())).map_err(|e| anyhow!("{e}"))?;
    out.iter().map(|o| Ok(o.cast_to::<f32>().map_err(|e| anyhow!("{e}"))?.to_plain_array_view::<f32>().map_err(|e| anyhow!("{e}"))?.iter().copied().collect())).collect()
}

fn run_ort(s: &mut ort::session::Session, input: Input) -> Result<Vec<Vec<f32>>> {
    use ort::value::Tensor;
    let shape: Vec<i64> = input.shape.iter().map(|&x| x as i64).collect();
    let value: ort::session::SessionInputValue = match input.data {
        Data::F32(d) => Tensor::from_array((shape, d))?.into(),
        Data::U8(d) => Tensor::from_array((shape, d))?.into(),
        Data::I64(d) => Tensor::from_array((shape, d))?.into(),
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
        && gpu_ok()
        && let Some(env) = RUNTIME.get()
    {
        match gpu_session(env, path) {
            Ok(s) => {
                info!("AI: {what} runs on the GPU");
                record(what, true, String::new());
                return Ok(Net::Gpu(Mutex::new(s)));
            }
            Err(e) => {
                let label = LOADED.get().map(|a| a.label()).unwrap_or_default();
                warn!("AI: {what} can't run on the {label} ({e:#}); using the processor");
                note(format!("{label}, but {what} runs on the CPU: {e:#}"));
                record(what, false, format!("the GPU couldn't run this model: {e:#}"));
                return Ok(Net::Cpu(cpu()?));
            }
        }
    }
    let why = if !use_gpu() {
        "processor chosen in the settings".to_string()
    } else if !gpu_ok() {
        "the graphics card was reset by Windows; the GPU is tried again later".to_string()
    } else {
        match RUNTIME_ERR.locked().clone() {
            Some(e) => format!("the GPU can't be used: {e}"),
            None => "the GPU runtime wasn't ready yet when it loaded (it moves to the GPU once ready)".into(),
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
    let mut p = PLACED.locked();
    p.retain(|x| x.what != what);
    p.push(Placed { what: what.to_string(), gpu, why });
}

/// Models loaded so far and where they run.
pub fn placed() -> Vec<Placed> {
    PLACED.locked().clone()
}

fn gpu_session(env: &ort::environment::Environment, path: &Path) -> Result<ort::session::Session> {
    use ort::session::{Session, builder::GraphOptimizationLevel};
    let accel = *LOADED.get().ok_or_else(|| anyhow!("the AI runtime isn't loaded"))?;
    fn e<T: std::fmt::Display>(x: T) -> anyhow::Error {
        anyhow!("{x}")
    }
    let builder = Session::builder(env)?.with_optimization_level(GraphOptimizationLevel::Level3).map_err(e)?;
    // Testing on a computer without the hardware: GAZE_ORT_CPU=1 runs the
    // same path on ONNX Runtime's processor backend.
    let mut builder = if std::env::var_os("GAZE_ORT_CPU").is_some() {
        builder
    } else {
        match accel {
            // DirectML needs these two off.
            Accel::DirectMl => builder
                .with_memory_pattern(false)
                .map_err(e)?
                .with_parallel_execution(false)
                .map_err(e)?
                .with_execution_providers([ort::ep::DirectML::default().build().error_on_failure()])
                .map_err(e)?,
            Accel::Cuda => builder.with_execution_providers([ort::ep::CUDA::default().build().error_on_failure()]).map_err(e)?,
            Accel::OpenVino => {
                let mut ep = ort::ep::OpenVINO::default().with_device_type(openvino_device());
                if let Some(d) = DATA_DIR.get() {
                    let cache = d.join("runtime").join("openvino-cache");
                    let _ = std::fs::create_dir_all(&cache);
                    ep = ep.with_cache_dir(cache.display().to_string());
                }
                builder.with_execution_providers([ep.build().error_on_failure()]).map_err(e)?
            }
        }
    };
    Ok(builder.commit_from_file(path)?)
}

/// Make the AI runtime ready (download it the first time). Returns whether
/// it's usable; models loaded before this ran on the processor.
pub async fn prepare(data_dir: &Path) -> bool {
    let _one = PREPARING.lock().await;
    if RUNTIME.get().is_some() {
        return true;
    }
    let _ = DATA_DIR.set(data_dir.to_path_buf());
    let res = async {
        let accel = wanted()?;
        let lib = runtime_files(data_dir, accel).await?;
        let lib2 = lib.clone();
        let env = tokio::task::spawn_blocking(move || -> Result<ort::environment::Environment> {
            #[cfg(windows)]
            preload_directml(&lib2)?;
            Ok(ort::init_from(&lib2).map_err(|e| anyhow!("{e}"))?.with_name("GAZE").build()?)
        })
        .await??;
        anyhow::Ok((env, accel))
    }
    .await;
    match res {
        Ok((env, accel)) => {
            info!("AI: runtime ready (ONNX Runtime with {})", accel.label());
            *RUNTIME_ERR.locked() = None;
            *STATUS.locked() = None;
            let _ = LOADED.set(accel);
            let _ = RUNTIME.set(env);
            true
        }
        Err(e) => {
            warn!("AI: the accelerator can't be used ({e:#}); AI runs on the processor");
            *RUNTIME_ERR.locked() = Some(format!("{e:#}"));
            false
        }
    }
}

/// The runtime library for an accelerator (downloaded and unpacked the first time).
async fn runtime_files(data_dir: &Path, accel: Accel) -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("GAZE_ORT_DYLIB") {
        return Ok(PathBuf::from(p)); // testing: a runtime library given by hand
    }
    match accel {
        Accel::DirectMl => directml_files(data_dir).await,
        Accel::OpenVino => wheel_files(data_dir, "openvino", OPENVINO_WHEEL, "AI runtime (OpenVINO)").await,
        Accel::Cuda => wheel_files(data_dir, "cuda", CUDA_WHEEL, "AI runtime (CUDA)").await,
    }
}

/// ONNX Runtime from a PyPI wheel: downloaded, checked against the pinned
/// SHA-256, and its native libraries unpacked next to each other (the
/// runtime finds its provider libraries beside itself).
async fn wheel_files(data_dir: &Path, kind: &str, wheel: (&str, &str), what: &str) -> Result<PathBuf> {
    let dir = data_dir.join("runtime").join(format!("onnxruntime-{kind}-{LINUX_ORT_VERSION}"));
    let lib = dir.join(format!("libonnxruntime.so.{LINUX_ORT_VERSION}"));
    let done = dir.join(".complete");
    if lib.exists() && done.exists() {
        return Ok(lib);
    }
    std::fs::create_dir_all(&dir)?;
    let whl = dir.join("runtime.whl");
    crate::jobs::download(what, wheel.0, &whl, 10_000_000).await?;
    let (whl2, dir2, want) = (whl.clone(), dir.clone(), wheel.1.to_string());
    tokio::task::spawn_blocking(move || -> Result<()> {
        let got = crate::jobs::sha256_file(&whl2)?;
        if got != want {
            let _ = std::fs::remove_file(&whl2);
            bail!("the downloaded {} runtime does not match its checksum (got {got}); it was discarded", "ONNX");
        }
        let n = unpack_native_libs(&whl2, &dir2)?;
        if n == 0 {
            bail!("no libraries found in the downloaded runtime");
        }
        std::fs::write(dir2.join(".complete"), b"ok")?;
        Ok(())
    })
    .await??;
    let _ = std::fs::remove_file(&whl);
    if !lib.exists() {
        bail!("{} is not in the downloaded runtime", lib.display());
    }
    Ok(lib)
}

/// Every native library in `onnxruntime/capi/` of a wheel (not the Python
/// binding), written flat into `out`. Returns how many.
fn unpack_native_libs(wheel: &Path, out: &Path) -> Result<usize> {
    let mut z = zip::ZipArchive::new(std::fs::File::open(wheel)?).context("reading the downloaded runtime")?;
    let mut n = 0;
    for i in 0..z.len() {
        let mut f = z.by_index(i)?;
        let name = f.name().map(|n| n.to_string()).unwrap_or_default();
        let Some(file) = name.strip_prefix("onnxruntime/capi/") else { continue };
        if file.contains('/') || file.contains("pybind11") || !(file.ends_with(".so") || file.contains(".so.")) {
            continue;
        }
        let dest = out.join(file);
        let tmp = dest.with_extension("part");
        let mut w = std::fs::File::create(&tmp)?;
        std::io::copy(&mut f, &mut w)?;
        drop(w);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
        }
        std::fs::rename(&tmp, &dest)?;
        n += 1;
    }
    Ok(n)
}

/// ONNX Runtime with DirectML (Windows), from two NuGet packages.
async fn directml_files(data_dir: &Path) -> Result<PathBuf> {
    if !cfg!(all(windows, target_arch = "x86_64")) {
        bail!("DirectML is only available on Windows (64-bit)");
    }
    let dir = data_dir.join("runtime").join(format!("onnxruntime-dml-{ORT_VERSION}"));
    let ort_dll = dir.join("onnxruntime.dll");
    let dml_dll = dir.join("DirectML.dll");
    if ort_dll.exists() && dml_dll.exists() {
        return Ok(ort_dll);
    }
    std::fs::create_dir_all(&dir)?;
    for (pkg, ver, inside, out, sha) in [
        ("microsoft.ml.onnxruntime.directml", ORT_VERSION, "runtimes/win-x64/native/onnxruntime.dll", &ort_dll, ORT_DML_SHA256),
        ("microsoft.ai.directml", DML_VERSION, "bin/x64-win/DirectML.dll", &dml_dll, DML_SHA256),
    ] {
        if out.exists() {
            continue;
        }
        let url = format!("https://api.nuget.org/v3-flatcontainer/{pkg}/{ver}/{pkg}.{ver}.nupkg");
        let nupkg = dir.join(format!("{pkg}.nupkg"));
        crate::jobs::download("the GPU runtime for AI", &url, &nupkg, 1_000_000).await?;
        let nupkg1 = nupkg.clone();
        tokio::task::spawn_blocking(move || crate::jobs::verify_sha256(&nupkg1, "GPU runtime package", sha)).await??;
        let (nupkg2, inside, out2) = (nupkg.clone(), inside.to_string(), out.clone());
        tokio::task::spawn_blocking(move || extract(&nupkg2, &inside, &out2)).await??;
        let _ = std::fs::remove_file(&nupkg);
    }
    Ok(ort_dll)
}

/// One file out of a NuGet package (a zip).
fn extract(zip_path: &Path, inside: &str, out: &Path) -> Result<()> {
    let mut z = zip::ZipArchive::new(std::fs::File::open(zip_path)?).context("reading the downloaded package")?;
    let mut f = z.by_name(inside).with_context(|| format!("{inside} is not in the package"))?;
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
        bail!("could not load {}: {}", dml.display(), std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_reset_graphics_card_is_recognised() {
        assert!(super::device_lost(
            "ExecutionProvider.cpp(952) Exception(49555) tid(861c) 887A0005 The GPU device instance has been suspended. Use GetDeviceRemovedReason to determine the appropriate action."
        ));
        assert!(!super::device_lost("unexpected kind of model output"));
    }

    use super::*;

    fn tract_plan(path: &Path, fact: Option<(&[usize], bool)>) -> Result<Plan> {
        use tract_onnx::prelude::*;
        let mut m = tract_onnx::onnx().with_ignore_value_info(true).with_ignore_output_shapes(true).model_for_path(path).map_err(|e| anyhow!("{e}"))?;
        if let Some((shape, bytes)) = fact {
            m = if bytes { m.with_input_fact(0, u8::fact(shape).into()) } else { m.with_input_fact(0, f32::fact(shape).into()) }.map_err(|e| anyhow!("{e}"))?;
        }
        m.into_optimized().map_err(|e| anyhow!("{e}"))?.into_runnable().map_err(|e| anyhow!("{e}"))
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
            let worst = x.iter().zip(y).map(|(p, q)| (p - q).abs() / (1.0 + q.abs())).fold(0f32, f32::max);
            assert!(worst < 2e-3, "{what}: outputs differ by {worst}");
        }
        eprintln!("{what}: {} output(s) match", a.len());
    }

    /// The ONNX Runtime path gives the same answers as tract. Needs
    /// GAZE_ORT_DYLIB (an ONNX Runtime library) or GAZE_TEST_ACCEL, and NVR_TEST_MODELS (a folder
    /// with yolox_tiny.onnx and the downloaded GAZE models); on a computer
    /// without DirectML also GAZE_ORT_CPU=1.
    #[tokio::test]
    async fn onnx_runtime_matches_tract() {
        // GAZE_TEST_ACCEL=openvino|cuda (with GAZE_TEST_OV_DEVICE, default CPU)
        // downloads that runtime as GAZE would and tests it, instead of a
        // library given by GAZE_ORT_DYLIB.
        let accel = std::env::var("GAZE_TEST_ACCEL").ok();
        let (true, Ok(dir)) = (accel.is_some() || std::env::var_os("GAZE_ORT_DYLIB").is_some(), std::env::var("NVR_TEST_MODELS")) else {
            return;
        };
        let _ = rustls::crypto::ring::default_provider().install_default();
        if let Some(a) = &accel {
            set_accelerator(a, &std::env::var("GAZE_TEST_OV_DEVICE").unwrap_or_else(|_| "CPU".into()));
        }
        let dir = PathBuf::from(dir);
        assert!(prepare(&dir).await, "runtime: {}", status());
        let pic: Vec<f32> = (0..3 * 416 * 416).map(|i| ((i * 37) % 255) as f32).collect();
        let (g, c) = both(&dir.join("yolox_tiny.onnx"), Some((&[1, 3, 416, 416], false)));
        close(&g.run(Input::f32(&[1, 3, 416, 416], pic.clone())).unwrap(), &c.run(Input::f32(&[1, 3, 416, 416], pic)).unwrap(), "object detection");
        let face: Vec<f32> = (0..3 * 640 * 640).map(|i| ((i * 13) % 255) as f32).collect();
        let (g, c) = both(&dir.join("face-yunet-2023mar.onnx"), None);
        close(&g.run(Input::f32(&[1, 3, 640, 640], face.clone())).unwrap(), &c.run(Input::f32(&[1, 3, 640, 640], face)).unwrap(), "faces");
        let plate: Vec<u8> = (0..64 * 128 * 3).map(|i| (i % 251) as u8).collect();
        let (g, c) = both(&dir.join("plate-ocr-cct-xs-global.onnx"), Some((&[1, 64, 128, 3], true)));
        close(&g.run(Input::u8(&[1, 64, 128, 3], plate.clone())).unwrap(), &c.run(Input::u8(&[1, 64, 128, 3], plate)).unwrap(), "number plates");
    }
}
