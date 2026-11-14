//! How busy the computer is right now, so work can go where there is room:
//! background jobs wait while the CPU is busy, the built-in AI checks less
//! often under heavy load, and video decoding for motion detection moves
//! between the CPU and the GPU depending on which one has capacity.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

static CPU: AtomicU32 = AtomicU32::new(0);
/// GPU % + 1 (0 = unknown).
static GPU: AtomicU32 = AtomicU32::new(0);

pub fn update(cpu: f32, gpu: Option<f32>) {
    CPU.store(cpu.round() as u32, Ordering::Relaxed);
    GPU.store(
        gpu.map(|g| g.round() as u32 + 1).unwrap_or(0),
        Ordering::Relaxed,
    );
}

pub fn cpu() -> u32 {
    CPU.load(Ordering::Relaxed)
}

pub fn gpu() -> Option<u32> {
    match GPU.load(Ordering::Relaxed) {
        0 => None,
        g => Some(g - 1),
    }
}

/// Smart load balancing (Settings → GPU acceleration).
static BALANCE: AtomicBool = AtomicBool::new(true);
/// Video jobs started on the CPU because the GPU was busy.
static MOVED: AtomicU64 = AtomicU64::new(0);

pub fn set_balance(on: bool) {
    BALANCE.store(on, Ordering::Relaxed);
}

pub fn balance() -> bool {
    BALANCE.load(Ordering::Relaxed)
}

pub fn moved_to_cpu() -> u64 {
    MOVED.load(Ordering::Relaxed)
}

/// Is the GPU too busy for new video work while the CPU has room? Counts
/// (and logs) the job `what` as moved to the CPU when so.
pub fn gpu_too_busy(what: &str) -> bool {
    let busy = balance() && gpu().is_some_and(|g| g >= 85) && cpu() < 75;
    if busy {
        MOVED.fetch_add(1, Ordering::Relaxed);
        tracing::info!(
            "load balancing: GPU at {}%, {what} runs on the CPU (CPU {}%)",
            gpu().unwrap_or(0),
            cpu()
        );
    }
    busy
}

/// Video work moved off the GPU (see [`gpu_too_busy`]) goes back once the
/// GPU has been quiet (under 50 %) for a minute, so the CPU is free again.
/// Asked every few seconds by the job that moved.
#[derive(Default)]
pub struct BackToGpu {
    since: Option<Instant>,
}

impl BackToGpu {
    pub fn ready(&mut self, what: &str) -> bool {
        if !(balance() && gpu().is_some_and(|g| g < 50)) {
            self.since = None;
            return false;
        }
        if self.since.get_or_insert_with(Instant::now).elapsed() < std::time::Duration::from_secs(60) {
            return false;
        }
        self.since = None;
        tracing::info!(
            "load balancing: GPU quiet ({}%), {what} moves back to the GPU",
            gpu().unwrap_or(0)
        );
        true
    }
}

/// Wait (up to `max`) while the CPU is busier than `limit` %.
pub async fn wait_for_cpu(limit: u32, max: std::time::Duration) {
    let start = Instant::now();
    while cpu() > limit && start.elapsed() < max {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
}

/// How much to stretch the built-in AI's check interval: 1× normally, more
/// when the processor is overloaded (the AI only adds to the backlog then).
pub fn ai_slowdown() -> f64 {
    match cpu() {
        0..=80 => 1.0,
        81..=90 => 2.0,
        _ => 4.0,
    }
}

/// Where a small (sub-stream) decoder should run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pick {
    Cpu,
    Gpu,
}

/// Decide CPU or GPU for decoding sub-streams. The CPU is normally cheaper
/// for small streams; the GPU takes over when the CPU is struggling and
/// the GPU has room, and gives the work back when the GPU is the busy one.
pub fn pick_decoder(current: Pick) -> Pick {
    let (c, g) = (cpu(), gpu());
    match current {
        Pick::Cpu if c >= 85 && g.is_none_or(|g| g < 60) => Pick::Gpu,
        Pick::Gpu if g.is_some_and(|g| g >= 90) && c < 60 => Pick::Cpu,
        p => p,
    }
}

/// A command that never opens a console window on Windows (GAZE runs
/// without one, so every ffmpeg would otherwise flash a black window).
pub fn cmd(program: impl AsRef<std::ffi::OsStr>) -> tokio::process::Command {
    #[allow(unused_mut)]
    let mut c = tokio::process::Command::new(program);
    #[cfg(windows)]
    c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    c
}

/// The same for programs run and waited on outside tokio.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn std_cmd(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    #[allow(unused_mut)]
    let mut c = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

/// Lower the priority of a background ffmpeg (previews, thumbnails), so it
/// never slows down recording or live view.
pub fn background(cmd: &mut tokio::process::Command) -> &mut tokio::process::Command {
    #[cfg(windows)]
    {
        const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(BELOW_NORMAL_PRIORITY_CLASS | CREATE_NO_WINDOW);
    }
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_choice() {
        update(95.0, Some(20.0));
        assert_eq!(pick_decoder(Pick::Cpu), Pick::Gpu);
        update(95.0, Some(80.0));
        assert_eq!(pick_decoder(Pick::Cpu), Pick::Cpu);
        update(40.0, Some(95.0));
        assert_eq!(pick_decoder(Pick::Gpu), Pick::Cpu);
        update(70.0, Some(40.0));
        assert_eq!(pick_decoder(Pick::Gpu), Pick::Gpu);
        update(50.0, None);
        assert_eq!(pick_decoder(Pick::Cpu), Pick::Cpu);
        // Moved off a busy GPU: back once it has been quiet for a minute.
        update(55.0, Some(70.0));
        let mut back = BackToGpu::default();
        assert!(!back.ready("test"));
        update(55.0, Some(30.0));
        assert!(!back.ready("test"), "not at once");
        back.since = Some(Instant::now() - std::time::Duration::from_secs(61));
        assert!(back.ready("test"));
        assert_eq!(ai_slowdown(), 1.0);
        // New video work: CPU when the GPU is busy and the CPU has room.
        update(40.0, Some(92.0));
        assert!(gpu_too_busy("test"));
        update(90.0, Some(92.0));
        assert!(!gpu_too_busy("test"), "both busy: stay where configured");
        update(40.0, Some(50.0));
        assert!(!gpu_too_busy("test"));
        set_balance(false);
        update(40.0, Some(99.0));
        assert!(!gpu_too_busy("test"), "balancing off");
        set_balance(true);
        update(0.0, None);
    }
}
