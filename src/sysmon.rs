//! System monitor: CPU, RAM and GPU usage sampled every few seconds, with
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

const FINE_SECS: i64 = 5;
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
        let raw: Vec<Point> = if seconds <= 6 * 3600 {
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
                })
                .collect()
        } else {
            self.minutes
                .lock()
                .unwrap()
                .iter()
                .filter(|p| p.t >= from)
                .copied()
                .collect()
        };
        // Fine samples every 5 s, minute samples every 60 s.
        let step = if seconds <= 6 * 3600 { FINE_SECS } else { 60 };
        bucketize(&raw, from, now, points, step)
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
        let mut last_gpu: Option<GpuReading> = None;
        let mut n = 0u64;
        loop {
            tick.tick().await;
            n += 1;
            sys.refresh_cpu_usage();
            sys.refresh_memory();
            sys.refresh_processes(ProcessesToUpdate::All, true);
            let cores = sys.cpus().len().max(1);
            let mut nvr = 0.0f32;
            let mut nvr_mem = 0u64;
            if let Some(pid) = pid {
                for (p_pid, p) in sys.processes() {
                    if *p_pid == pid || p.parent() == Some(pid) {
                        nvr += p.cpu_usage();
                        nvr_mem += p.memory();
                    }
                }
            }
            // The Windows counters are slow to read; sample them less often.
            if source != GpuSource::WindowsCounters || n % 2 == 1 {
                last_gpu = read_gpu(source).await;
            }
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
    }
}

/// Group samples into equal time buckets from `from` to `now` (averages and
/// peaks). Buckets without data (NVR not running, computer off) are zero, so
/// the graph drops to 0 instead of drawing a line across the gap.
fn bucketize(raw: &[Point], from: i64, now: i64, points: usize, step: i64) -> Vec<Point> {
    let span = (now - from).max(1);
    let width = (span / points.max(1) as i64).max(step);
    let n = (span / width).max(1) as usize;
    let has_gpu = raw.iter().any(|p| p.gpu.is_some());
    let mut buckets: Vec<Vec<&Point>> = vec![Vec::new(); n];
    for p in raw {
        let i = ((p.t - from) / width).clamp(0, n as i64 - 1) as usize;
        buckets[i].push(p);
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
            }
        })
        .collect()
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
            let text = run_cmd("nvidia-smi", NVSMI_ARGS, 5).await?;
            parse_nvidia_smi(&text)
        }
        GpuSource::Sysfs => sysfs_busy().map(|util| GpuReading {
            util,
            mem: None,
            name: Some("GPU".into()),
            temp: None,
        }),
        GpuSource::WindowsCounters => read_windows_counters().await,
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

/// Windows: total CPU as Task Manager shows it ("% Processor Utility").
#[cfg(windows)]
async fn cpu_utility() -> Option<f32> {
    let text = run_cmd(
        "typeperf",
        &[
            "\\Processor Information(_Total)\\% Processor Utility",
            "-sc",
            "1",
        ],
        10,
    )
    .await?;
    parse_single_counter(&text).map(|v| v.min(100.0))
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
    let mut per_engine: std::collections::HashMap<String, f32> = std::collections::HashMap::new();
    for (name, val) in h.iter().zip(d.iter()).skip(1) {
        let Some(eng) = name.split("engtype_").nth(1) else {
            continue;
        };
        let eng = eng.split(')').next().unwrap_or(eng).to_string();
        let v: f32 = val.trim().parse().unwrap_or(0.0);
        *per_engine.entry(eng).or_default() += v;
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
        assert_eq!(parse_typeperf(text), Some(41.0));
    }

    #[test]
    fn buckets_with_peaks_and_zero_gaps() {
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
        };
        // Data in the first and last 10 s of a 40 s window; nothing in between.
        let raw = vec![p(1, 10.0), p(6, 30.0), p(31, 50.0), p(36, 70.0)];
        let b = bucketize(&raw, 0, 40, 4, 5);
        assert_eq!(b.len(), 4);
        assert_eq!((b[0].cpu, b[0].cpu_max), (20.0, 30.0));
        assert_eq!((b[1].cpu, b[1].ram), (0.0, 0.0)); // gap drops to zero
        assert_eq!(b[2].cpu, 0.0);
        assert_eq!(b[3].cpu_max, 70.0);
    }

    #[test]
    fn disk_space_works() {
        let (free, total) = disk_space(&std::env::temp_dir().join("does/not/exist")).unwrap();
        assert!(total > 0 && free <= total);
    }
}

/// Memory free for programs right now, in bytes.
pub fn free_memory() -> Option<u64> {
    let mut s = sysinfo::System::new().ok()?;
    s.refresh_memory();
    Some(s.available_memory()).filter(|m| *m > 0)
}
