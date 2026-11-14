//! System monitor: CPU, RAM and GPU usage sampled every second, with
//! history (fine-grained for 6 hours, per-minute for 30 days; the per-minute
//! history is saved to disk so it survives restarts).

use serde::Serialize;
use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;
use sysinfo::{ProcessesToUpdate, System};
use tracing::{info, warn};

const FINE_SECS: i64 = 1;
const FINE_KEEP: usize = 6 * 3600 / FINE_SECS as usize;
const MINUTE_KEEP: usize = 30 * 24 * 60;

#[derive(Debug, Clone, Copy, Serialize, Default)]
pub struct Sample {
    /// Unix time (seconds).
    pub t: i64,
    /// Whole-system CPU %.
    pub cpu: f32,
    /// CPU % used by the NVR and its ffmpeg processes (of the whole machine).
    pub nvr_cpu: f32,
    /// RAM in use, %.
    pub ram: f32,
    /// RAM used by the NVR and its ffmpeg processes, % of the computer's RAM.
    pub nvr_ram: f32,
    /// GPU utilisation %, if a GPU can be read.
    pub gpu: Option<f32>,
    /// GPU memory in use, %.
    pub gpu_mem: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Current {
    pub sample: Sample,
    pub cpu_name: String,
    pub cores: usize,
    pub ram_used: u64,
    pub ram_total: u64,
    /// Bytes of RAM used by the NVR and its ffmpeg processes.
    pub nvr_ram_used: u64,
    pub gpu_name: Option<String>,
    pub gpu_temp: Option<f32>,
    /// How the GPU is read, e.g. "nvidia-smi".
    pub gpu_source: Option<String>,
    /// Video jobs moved to the CPU because the GPU was busy.
    pub moved_to_cpu: u64,
    /// Where GAZE's processor time goes: (what, % of the whole machine),
    /// busiest first.
    pub breakdown: Vec<(String, f32)>,
}

/// What an ffmpeg started by GAZE is doing, from its command line.
fn job_of(args: &str) -> &'static str {
    let a = args;
    if a.contains("f32le") || a.contains("s16le") {
        "Sound detection (listening)"
    } else if a.contains("rawvideo") {
        "Motion detection (decoding video)"
    } else if a.contains("image2pipe") || a.contains("mjpeg") {
        "Live pictures (JPEG)"
    } else if a.contains("libx264") || a.contains("nvenc") || a.contains("_qsv") || a.contains("_amf") || a.contains("h264_mf") || a.contains("_vaapi") {
        "Converting video (H.264)"
    } else if a.contains("segment") {
        "Recording"
    } else if a.contains("frag_keyframe") {
        "Live video and playback"
    } else if a.contains("rtsp") || a.contains("relay") {
        "Camera connections"
    } else {
        "Other video work"
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Point {
    pub t: i64,
    pub cpu: f32,
    pub cpu_max: f32,
    pub nvr_cpu: f32,
    pub ram: f32,
    pub nvr_ram: f32,
    pub gpu: Option<f32>,
    pub gpu_max: Option<f32>,
    pub gpu_mem: Option<f32>,
    /// No readings for this time (GAZE wasn't running, or the computer was off).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub gap: bool,
}

pub struct Monitor {
    fine: Mutex<VecDeque<Sample>>,
    minutes: Mutex<VecDeque<Point>>,
    current: Mutex<Current>,
    file: PathBuf,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum GpuSource {
    NvidiaSmi,
    Sysfs,
    WindowsCounters,
    None,
}

struct GpuReading {
    util: f32,
    mem: Option<f32>,
    name: Option<String>,
    temp: Option<f32>,
}

impl Monitor {
    pub fn new(data_dir: &std::path::Path) -> Self {
        let file = data_dir.join("system-history.csv");
        let minutes = load_history(&file);
        Self {
            fine: Mutex::new(VecDeque::new()),
            minutes: Mutex::new(minutes),
            current: Mutex::new(Current::default()),
            file,
        }
    }

    pub fn current(&self) -> Current {
        self.current.lock().unwrap().clone()
    }

    /// History for the last `seconds`, reduced to at most `points` points.
    pub fn history(&self, seconds: i64, points: usize) -> Vec<Point> {
        let now = chrono::Utc::now().timestamp();
        let from = now - seconds;
        // The fine samples (every 5 s) are only kept since GAZE started:
        // before them, the per-minute history (kept on disk) fills in.
        let fine: Vec<Point> = if seconds <= 6 * 3600 {
            self.fine
                .lock()
                .unwrap()
                .iter()
                .filter(|s| s.t >= from)
                .map(|s| Point {
                    t: s.t,
                    cpu: s.cpu,
                    cpu_max: s.cpu,
                    nvr_cpu: s.nvr_cpu,
                    ram: s.ram,
                    nvr_ram: s.nvr_ram,
                    gpu: s.gpu,
                    gpu_max: s.gpu,
                    gpu_mem: s.gpu_mem,
                    gap: false,
                })
                .collect()
        } else {
            Vec::new()
        };
        let until = fine.first().map(|p| p.t).unwrap_or(i64::MAX);
        let mut raw: Vec<Point> = self
            .minutes
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.t >= from && p.t < until)
            .copied()
            .collect();
        raw.extend(fine);
        bucketize(&raw, from, now, points, 60)
    }

    /// Sampling loop; runs for the life of the program.
    pub async fn run(self: std::sync::Arc<Self>) {
        let mut sys = match System::new() {
            Ok(s) => s,
            Err(e) => {
                warn!("system monitor unavailable: {e}");
                return;
            }
        };
        sys.refresh_cpu_all();
        let pid = sysinfo::get_current_pid().ok();
        let source = detect_gpu().await;
        info!("system monitor: GPU readings via {source:?}");
        let mut tick = tokio::time::interval(Duration::from_secs(FINE_SECS as u64));
        // The card's real name (Windows counters only say "GPU").
        #[cfg(windows)]
        let card_name = windows_gpu_name().await;
        #[cfg(not(windows))]
        let card_name: Option<String> = None;
        let mut minute: Vec<Sample> = Vec::new();
        loop {
            tick.tick().await;
            sys.refresh_cpu_usage();
            sys.refresh_memory();
            sys.refresh_processes(ProcessesToUpdate::All, true);
            let cores = sys.cpus().len().max(1);
            let mut nvr = 0.0f32;
            let mut nvr_mem = 0u64;
            let mut jobs: std::collections::HashMap<&'static str, f32> = Default::default();
            if let Some(pid) = pid {
                for (p_pid, p) in sys.processes() {
                    if *p_pid == pid || p.parent() == Some(pid) {
                        nvr += p.cpu_usage();
                        nvr_mem += p.memory();
                        let what = if *p_pid == pid {
                            "GAZE itself (motion analysis, AI on the processor, pictures)"
                        } else {
                            let args: Vec<String> = p.cmd().iter().map(|a| a.to_string_lossy().into_owned()).collect();
                            job_of(&args.join(" "))
                        };
                        *jobs.entry(what).or_default() += p.cpu_usage();
                    }
                }
            }
            let last_gpu = read_gpu(source).await;
            let total = sys.total_memory().max(1);
            // Windows' Task Manager shows "% Processor Utility" (it counts
            // turbo clock speeds), which can differ a lot from plain busy
            // time: use the same figure, and scale the NVR's share to match.
            let mut cpu = sys.global_cpu_usage();
            let mut nvr_cpu = nvr / cores as f32;
            if let Some(u) = cpu_utility().await {
                if cpu > 0.5 {
                    nvr_cpu *= u / cpu;
                }
                cpu = u;
            }
            let s = Sample {
                t: chrono::Utc::now().timestamp(),
                cpu: cpu.min(100.0),
                nvr_cpu: nvr_cpu.min(100.0),
                ram: sys.used_memory() as f32 * 100.0 / total as f32,
                nvr_ram: (nvr_mem as f32 * 100.0 / total as f32).min(100.0),
                gpu: last_gpu.as_ref().map(|g| g.util),
                gpu_mem: last_gpu.as_ref().and_then(|g| g.mem),
            };
            crate::load::update(s.cpu, s.gpu);
            *self.current.lock().unwrap() = Current {
                sample: s,
                cpu_name: sys
                    .cpus()
                    .first()
                    .map(|c| c.brand().trim().to_string())
                    .unwrap_or_default(),
                cores,
                ram_used: sys.used_memory(),
                ram_total: total,
                nvr_ram_used: nvr_mem,
                gpu_name: card_name
                    .clone()
                    .or_else(|| last_gpu.as_ref().and_then(|g| g.name.clone())),
                gpu_temp: last_gpu.as_ref().and_then(|g| g.temp),
                gpu_source: (source != GpuSource::None).then(|| format!("{source:?}")),
                moved_to_cpu: crate::load::moved_to_cpu(),
                breakdown: {
                    // (Scaled like the NVR's share above: % of the machine.)
                    let k = if nvr > 0.0 { nvr_cpu / nvr } else { 1.0 / cores as f32 };
                    let mut b: Vec<(String, f32)> = jobs.into_iter().map(|(w, v)| (w.to_string(), v * k)).filter(|x| x.1 >= 0.1).collect();
                    b.sort_by(|a, c| c.1.total_cmp(&a.1));
                    b
                },
            };
            {
                let mut f = self.fine.lock().unwrap();
                f.push_back(s);
                while f.len() > FINE_KEEP {
                    f.pop_front();
                }
            }
            minute.push(s);
            if minute.len() >= (60 / FINE_SECS) as usize {
                let p = aggregate(&minute);
                minute.clear();
                {
                    let mut m = self.minutes.lock().unwrap();
                    m.push_back(p);
                    while m.len() > MINUTE_KEEP {
                        m.pop_front();
                    }
                }
                self.append(&p);
            }
        }
    }

    fn append(&self, p: &Point) {
        let line = format!(
            "{},{:.1},{:.1},{:.1},{:.1},{},{},{},{:.2}\n",
            p.t,
            p.cpu,
            p.cpu_max,
            p.nvr_cpu,
            p.ram,
            opt(p.gpu),
            opt(p.gpu_max),
            opt(p.gpu_mem),
            p.nvr_ram
        );
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file)
        {
            let _ = f.write_all(line.as_bytes());
        }
        // Keep the file from growing forever: rewrite it once a day.
        if p.t % 86_400 < 60 {
            let m = self.minutes.lock().unwrap();
            let text: String = m
                .iter()
                .map(|p| {
                    format!(
                        "{},{:.1},{:.1},{:.1},{:.1},{},{},{},{:.2}\n",
                        p.t,
                        p.cpu,
                        p.cpu_max,
                        p.nvr_cpu,
                        p.ram,
                        opt(p.gpu),
                        opt(p.gpu_max),
                        opt(p.gpu_mem),
                        p.nvr_ram
                    )
                })
                .collect();
            let _ = std::fs::write(&self.file, text);
        }
    }
}

fn opt(v: Option<f32>) -> String {
    v.map(|x| format!("{x:.1}")).unwrap_or_default()
}

fn load_history(file: &std::path::Path) -> VecDeque<Point> {
    let cutoff = chrono::Utc::now().timestamp() - (MINUTE_KEEP as i64) * 60;
    let Ok(text) = std::fs::read_to_string(file) else {
        return VecDeque::new();
    };
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(',').collect();
            if f.len() < 8 {
                return None;
            }
            let num = |s: &str| s.parse::<f32>().ok();
            Some(Point {
                t: f[0].parse().ok()?,
                cpu: num(f[1])?,
                cpu_max: num(f[2])?,
                nvr_cpu: num(f[3])?,
                ram: num(f[4])?,
                nvr_ram: f.get(8).and_then(|x| num(x)).unwrap_or(0.0),
                gpu: num(f[5]),
                gpu_max: num(f[6]),
                gpu_mem: num(f[7]),
                gap: false,
            })
        })
        .filter(|p| p.t >= cutoff)
        .collect()
}

fn aggregate(s: &[Sample]) -> Point {
    let n = s.len().max(1) as f32;
    let avg = |f: &dyn Fn(&Sample) -> f32| s.iter().map(f).sum::<f32>() / n;
    let gpus: Vec<f32> = s.iter().filter_map(|x| x.gpu).collect();
    let mems: Vec<f32> = s.iter().filter_map(|x| x.gpu_mem).collect();
    Point {
        t: s.last().map(|x| x.t).unwrap_or(0),
        cpu: avg(&|x| x.cpu),
        cpu_max: s.iter().map(|x| x.cpu).fold(0.0, f32::max),
        nvr_cpu: avg(&|x| x.nvr_cpu),
        ram: avg(&|x| x.ram),
        nvr_ram: avg(&|x| x.nvr_ram),
        gpu: (!gpus.is_empty()).then(|| gpus.iter().sum::<f32>() / gpus.len() as f32),
        gpu_max: gpus.iter().copied().reduce(f32::max),
        gpu_mem: (!mems.is_empty()).then(|| mems.iter().sum::<f32>() / mems.len() as f32),
        gap: false,
    }
}

/// Group samples into equal time buckets from `from` to `now` (averages and
/// peaks). A bucket without data takes the reading next to it when that's
/// within `step` seconds (per-minute history in buckets smaller than a
/// minute); otherwise it's a gap (GAZE not running, computer off), drawn as
/// a break rather than as 0 %.
fn bucketize(raw: &[Point], from: i64, now: i64, points: usize, step: i64) -> Vec<Point> {
    let span = (now - from).max(1);
    let width = (span / points.max(1) as i64).max(1);
    let n = (span / width).max(1) as usize;
    let has_gpu = raw.iter().any(|p| p.gpu.is_some());
    let mut buckets: Vec<Vec<&Point>> = vec![Vec::new(); n];
    for p in raw {
        let i = ((p.t - from) / width).clamp(0, n as i64 - 1) as usize;
        buckets[i].push(p);
    }
    // Empty buckets close to a reading (within `step`): that reading.
    let reach = (step / width) as usize;
    let filled: Vec<usize> = (0..n).filter(|&i| !buckets[i].is_empty()).collect();
    for i in 0..n {
        if buckets[i].is_empty()
            && let Some(&j) = filled.iter().min_by_key(|&&j| j.abs_diff(i))
            && j.abs_diff(i) <= reach
        {
            buckets[i] = buckets[j].clone();
        }
    }
    buckets
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let t = from + (i as i64 + 1) * width;
            if c.is_empty() {
                let zero = has_gpu.then_some(0.0);
                return Point {
                    t,
                    cpu: 0.0,
                    cpu_max: 0.0,
                    nvr_cpu: 0.0,
                    ram: 0.0,
                    nvr_ram: 0.0,
                    gpu: zero,
                    gpu_max: zero,
                    gpu_mem: zero,
                    gap: true,
                };
            }
            let k = c.len() as f32;
            let gpus: Vec<f32> = c.iter().filter_map(|p| p.gpu).collect();
            Point {
                t,
                cpu: c.iter().map(|p| p.cpu).sum::<f32>() / k,
                cpu_max: c.iter().map(|p| p.cpu_max).fold(0.0, f32::max),
                nvr_cpu: c.iter().map(|p| p.nvr_cpu).sum::<f32>() / k,
                ram: c.iter().map(|p| p.ram).sum::<f32>() / k,
                nvr_ram: c.iter().map(|p| p.nvr_ram).sum::<f32>() / k,
                gpu: (!gpus.is_empty())
                    .then(|| gpus.iter().sum::<f32>() / gpus.len() as f32)
                    .or(has_gpu.then_some(0.0)),
                gpu_max: c.iter().filter_map(|p| p.gpu_max).reduce(f32::max),
                gpu_mem: c.iter().filter_map(|p| p.gpu_mem).reduce(f32::max),
                gap: false,
            }
        })
        .collect()
}

/// A counter program kept running, printing a reading every second
/// (typeperf, nvidia-smi): starting one for every reading cost CPU time
/// that then showed up in the very reading (CPU looked busier than Task
/// Manager said). Its column header line and latest line are kept.
#[derive(Default)]
struct Stream {
    header: Option<String>,
    last: Option<(std::time::Instant, String)>,
}

type StreamRef = std::sync::Arc<Mutex<Stream>>;

fn stream(cmd: &'static str, args: Vec<String>, restart_every: Option<Duration>) -> StreamRef {
    let st: StreamRef = Default::default();
    let out = st.clone();
    tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;
        loop {
            let child = crate::load::cmd(cmd)
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn();
            let Ok(mut child) = child else {
                tokio::time::sleep(Duration::from_secs(30)).await;
                continue;
            };
            let Some(stdout) = child.stdout.take() else { continue };
            let mut lines = tokio::io::BufReader::new(stdout).lines();
            let started = std::time::Instant::now();
            let mut first_quoted = true;
            while let Ok(Ok(Some(l))) = tokio::time::timeout(Duration::from_secs(10), lines.next_line()).await {
                let l = l.trim().to_string();
                if l.is_empty() {
                    continue;
                }
                let mut s = st.lock().unwrap();
                // typeperf: the first quoted line names the columns.
                if l.starts_with('"') && first_quoted {
                    first_quoted = false;
                    s.header = Some(l);
                    continue;
                }
                s.last = Some((std::time::Instant::now(), l));
                drop(s);
                if restart_every.is_some_and(|d| started.elapsed() > d) {
                    break;
                }
            }
            let _ = child.kill().await;
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
    out
}

/// The latest reading of a stream (header and line together, as one
/// sample's output), if it's recent.
fn latest(s: &StreamRef) -> Option<String> {
    let s = s.lock().unwrap();
    let (at, l) = s.last.as_ref()?;
    if at.elapsed() > Duration::from_secs(5) {
        return None;
    }
    Some(match &s.header {
        Some(h) => format!("{h}\n{l}"),
        None => l.clone(),
    })
}

static STREAMS: std::sync::OnceLock<Mutex<std::collections::HashMap<&'static str, StreamRef>>> = std::sync::OnceLock::new();

fn named_stream(name: &'static str, cmd: &'static str, args: &[&str], restart_every: Option<Duration>) -> StreamRef {
    let mut m = STREAMS.get_or_init(Default::default).lock().unwrap();
    m.entry(name)
        .or_insert_with(|| stream(cmd, args.iter().map(|a| a.to_string()).collect(), restart_every))
        .clone()
}

async fn run_cmd(cmd: &str, args: &[&str], secs: u64) -> Option<String> {
    let out = tokio::time::timeout(
        Duration::from_secs(secs),
        crate::load::cmd(cmd)
            .args(args)
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

const NVSMI_ARGS: &[&str] = &[
    "--query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu",
    "--format=csv,noheader,nounits",
];

async fn detect_gpu() -> GpuSource {
    if run_cmd("nvidia-smi", NVSMI_ARGS, 10).await.is_some() {
        return GpuSource::NvidiaSmi;
    }
    if cfg!(target_os = "linux") && sysfs_busy().is_some() {
        return GpuSource::Sysfs;
    }
    if cfg!(windows) && read_windows_counters().await.is_some() {
        return GpuSource::WindowsCounters;
    }
    GpuSource::None
}

async fn read_gpu(source: GpuSource) -> Option<GpuReading> {
    match source {
        GpuSource::NvidiaSmi => {
            let mut a = NVSMI_ARGS.to_vec();
            a.extend(["-l", "1"]);
            let st = named_stream("nvsmi", "nvidia-smi", &a, None);
            parse_nvidia_smi(&latest(&st)?)
        }
        GpuSource::Sysfs => sysfs_busy().map(|util| GpuReading {
            util,
            mem: None,
            name: Some("GPU".into()),
            temp: None,
        }),
        GpuSource::WindowsCounters => {
            // (Restarted every minute: programs that started using the
            // graphics card since are counted from then.)
            let st = named_stream("gpuperf", "typeperf", &["\\GPU Engine(*)\\Utilization Percentage", "-si", "1"], Some(Duration::from_secs(60)));
            match latest(&st) {
                Some(t) => parse_typeperf(&t).map(|util| GpuReading { util, mem: None, name: Some("GPU".into()), temp: None }),
                None => None,
            }
        }
        GpuSource::None => None,
    }
}

/// First GPU line of `nvidia-smi --query-gpu=... --format=csv,noheader,nounits`.
fn parse_nvidia_smi(text: &str) -> Option<GpuReading> {
    let line = text.lines().find(|l| !l.trim().is_empty())?;
    let f: Vec<&str> = line.split(',').map(str::trim).collect();
    let num = |i: usize| f.get(i).and_then(|s| s.parse::<f32>().ok());
    let mem = match (num(2), num(3)) {
        (Some(u), Some(t)) if t > 0.0 => Some(u * 100.0 / t),
        _ => None,
    };
    Some(GpuReading {
        util: num(1)?,
        mem,
        name: f.first().map(|s| s.to_string()),
        temp: num(4),
    })
}

/// AMD (and some Intel) GPUs on Linux expose a busy percentage.
fn sysfs_busy() -> Option<f32> {
    let rd = std::fs::read_dir("/sys/class/drm").ok()?;
    rd.flatten()
        .filter_map(|e| std::fs::read_to_string(e.path().join("device/gpu_busy_percent")).ok())
        .filter_map(|s| s.trim().parse::<f32>().ok())
        .reduce(f32::max)
}

/// Windows: total CPU as Task Manager shows it ("% Processor Utility"),
/// read straight from Windows (by the counter's English name, so it works
/// on Windows in any language: through typeperf it could fail on some, and
/// the plain busy time read instead didn't match Task Manager).
#[cfg(windows)]
mod pdh {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::System::Performance::*;

    pub static LAST: Mutex<Option<(Instant, f32)>> = Mutex::new(None);

    pub fn start() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            let _ = std::thread::Builder::new().name("cpu-counter".into()).spawn(run);
        });
    }

    fn run() {
        unsafe {
            let mut q: PDH_HQUERY = std::ptr::null_mut();
            if PdhOpenQueryW(std::ptr::null(), 0, &mut q) != 0 {
                tracing::warn!("CPU counter: can't open a query");
                return;
            }
            let path: Vec<u16> = "\\Processor Information(_Total)\\% Processor Utility\0".encode_utf16().collect();
            let mut c: PDH_HCOUNTER = std::ptr::null_mut();
            if PdhAddEnglishCounterW(q, path.as_ptr(), 0, &mut c) != 0 {
                tracing::warn!("CPU counter: % Processor Utility isn't available");
                return;
            }
            PdhCollectQueryData(q);
            loop {
                std::thread::sleep(Duration::from_secs(1));
                if PdhCollectQueryData(q) != 0 {
                    continue;
                }
                let mut v: PDH_FMT_COUNTERVALUE = std::mem::zeroed();
                // 0x8000: PDH_FMT_NOCAP100 (it can go over 100 with turbo).
                if PdhGetFormattedCounterValue(c, PDH_FMT_DOUBLE | 0x8000, std::ptr::null_mut(), &mut v) == 0 && v.CStatus <= 1 {
                    *LAST.lock().unwrap() = Some((Instant::now(), v.Anonymous.doubleValue as f32));
                }
            }
        }
    }

    pub fn latest() -> Option<f32> {
        start();
        LAST.lock().unwrap().filter(|(t, _)| t.elapsed() < Duration::from_secs(4)).map(|(_, v)| v)
    }
}

#[cfg(windows)]
async fn cpu_utility() -> Option<f32> {
    if let Some(v) = pdh::latest() {
        return Some(v.min(100.0));
    }
    let st = named_stream("cpuperf", "typeperf", &["\\Processor Information(_Total)\\% Processor Utility", "-si", "1"], None);
    parse_single_counter(&latest(&st)?).map(|v| v.min(100.0))
}

#[cfg(not(windows))]
async fn cpu_utility() -> Option<f32> {
    None
}

/// The value of a one-counter `typeperf -sc 1` output.
#[cfg_attr(not(windows), allow(dead_code))]
fn parse_single_counter(text: &str) -> Option<f32> {
    let data = text
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with('"'))
        .nth(1)?;
    data.rsplit("\",\"")
        .next()?
        .trim_matches('"')
        .trim()
        .parse()
        .ok()
}

/// The graphics card's name on Windows (any vendor).
#[cfg(windows)]
pub async fn windows_gpu_name() -> Option<String> {
    let text = run_cmd(
        "powershell",
        &["-NoProfile", "-Command", "(Get-CimInstance Win32_VideoController | Select-Object -ExpandProperty Name) -join ' + '"],
        20,
    )
    .await?;
    let n = text.trim().to_string();
    (!n.is_empty()).then_some(n)
}

/// Windows "GPU Engine" performance counters (works for any GPU vendor):
/// utilisation of the busiest engine type, as Task Manager shows it.
async fn read_windows_counters() -> Option<GpuReading> {
    let text = run_cmd(
        "typeperf",
        &["\\GPU Engine(*)\\Utilization Percentage", "-sc", "1"],
        15,
    )
    .await?;
    let util = parse_typeperf(&text)?;
    Some(GpuReading {
        util,
        mem: None,
        name: Some("GPU".into()),
        temp: None,
    })
}

fn parse_typeperf(text: &str) -> Option<f32> {
    let mut lines = text.lines().map(str::trim).filter(|l| l.starts_with('"'));
    let header = lines.next()?;
    let data = lines.next()?;
    let split = |l: &str| {
        l.split("\",\"")
            .map(|s| s.trim_matches('"').to_string())
            .collect::<Vec<_>>()
    };
    let (h, d) = (split(header), split(data));
    // One counter per process and engine: add up the processes using each
    // engine ("luid_…_phys_…_eng_N_engtype_X"), then the busiest engine is
    // the GPU's usage (as Task Manager shows it). Engines of the same type
    // on other adapters or with another number are separate: adding them
    // together (as before) made a mostly idle GPU look 100% busy.
    let mut per_engine: std::collections::HashMap<String, f32> = std::collections::HashMap::new();
    for (name, val) in h.iter().zip(d.iter()).skip(1) {
        let Some(inst) = name.split("GPU Engine(").nth(1) else {
            continue;
        };
        let inst = inst.split(')').next().unwrap_or(inst);
        // Drop the process part ("pid_1234_").
        let engine = inst
            .split_once("_luid_")
            .map(|(_, e)| e)
            .unwrap_or(inst)
            .to_string();
        let v: f32 = val.trim().parse().unwrap_or(0.0);
        *per_engine.entry(engine).or_default() += v;
    }
    per_engine
        .values()
        .copied()
        .reduce(f32::max)
        .map(|v| v.min(100.0))
}

/// Free and total bytes of the disk holding `path` (works for network shares too).
pub fn disk_space(path: &std::path::Path) -> Option<(u64, u64)> {
    let mut p = path.to_path_buf();
    // The folder may not exist yet: use the nearest existing parent.
    while !p.exists() {
        if !p.pop() {
            p = PathBuf::from(".");
            break;
        }
    }
    match fs4::statvfs(&p) {
        Ok(s) => Some((s.available_space(), s.total_space())),
        Err(e) => {
            warn!("cannot read disk space for {}: {e}", p.display());
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nvidia_smi() {
        let g = parse_nvidia_smi("NVIDIA GeForce RTX 3060, 37, 1024, 12288, 54\n").unwrap();
        assert_eq!(g.util, 37.0);
        assert_eq!(g.name.as_deref(), Some("NVIDIA GeForce RTX 3060"));
        assert!((g.mem.unwrap() - 8.33).abs() < 0.1);
        assert_eq!(g.temp, Some(54.0));
    }

    #[test]
    fn parses_single_counter() {
        let text = "\r\n\"(PDH-CSV 4.0)\",\"\\\\PC\\Processor Information(_Total)\\% Processor Utility\"\r\n\"11/10/2026 09:00:00.000\",\"37.5\"\r\nExiting, please wait...";
        assert_eq!(super::parse_single_counter(text), Some(37.5));
    }

    #[test]
    fn parses_typeperf() {
        let text = r#"
"(PDH-CSV 4.0)","\\PC\GPU Engine(pid_1_luid_0x0_phys_0_eng_0_engtype_3D)\Utilization Percentage","\\PC\GPU Engine(pid_2_luid_0x0_phys_0_eng_0_engtype_3D)\Utilization Percentage","\\PC\GPU Engine(pid_2_luid_0x0_phys_0_eng_3_engtype_VideoDecode)\Utilization Percentage"
"11/08/2026 06:35:39.686","12.5","20.0","41.0"
The command completed successfully."#;
        // The 3D engine is used by two processes (12.5 + 20): 41 is busiest.
        assert_eq!(parse_typeperf(text), Some(41.0));
        // Two video decoders and two adapters at 60 % each: 60 %, not 100 %.
        let two = r#"
"(PDH-CSV 4.0)","\\PC\GPU Engine(pid_9_luid_0x1_phys_0_eng_3_engtype_VideoDecode)\Utilization Percentage","\\PC\GPU Engine(pid_9_luid_0x1_phys_0_eng_4_engtype_VideoDecode)\Utilization Percentage","\\PC\GPU Engine(pid_9_luid_0x2_phys_0_eng_3_engtype_VideoDecode)\Utilization Percentage"
"11/08/2026 06:35:39.686","60","60","60"
"#;
        assert_eq!(parse_typeperf(two), Some(60.0));
    }

    #[test]
    fn buckets_with_peaks_and_gaps() {
        let p = |t: i64, cpu: f32| Point {
            t,
            cpu,
            cpu_max: cpu,
            nvr_cpu: 0.0,
            ram: 50.0,
            nvr_ram: 1.0,
            gpu: None,
            gpu_max: None,
            gpu_mem: None,
            gap: false,
        };
        // Data in the first and last 10 s of a 40 s window; nothing in between.
        let raw = vec![p(1, 10.0), p(6, 30.0), p(31, 50.0), p(36, 70.0)];
        let b = bucketize(&raw, 0, 40, 4, 5);
        assert_eq!(b.len(), 4);
        assert_eq!((b[0].cpu, b[0].cpu_max), (20.0, 30.0));
        assert!(b[1].gap && b[2].gap); // no readings: a gap (not 0 %)
        assert_eq!(b[3].cpu_max, 70.0);
        // Per-minute readings in 10 s buckets: no gaps between them.
        let mins = vec![p(60, 10.0), p(120, 20.0), p(180, 30.0)];
        let b = bucketize(&mins, 60, 180, 12, 60);
        assert!(b.iter().all(|x| !x.gap));
    }

    #[test]
    fn disk_space_works() {
        let (free, total) = disk_space(&std::env::temp_dir().join("does/not/exist")).unwrap();
        assert!(total > 0 && free <= total);
    }
}
