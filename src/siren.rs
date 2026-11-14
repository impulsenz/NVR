//! A deterrent siren through a camera's speaker: a loud wailing alarm with
//! spoken warnings in between ("The homeowner has been notified"), for a
//! set time or until stopped.

use std::collections::HashSet;
use std::sync::Mutex;

/// Cameras with the siren going (taking one out stops it).
static RUNNING: Mutex<Option<HashSet<String>>> = Mutex::new(None);

pub fn is_running(camera: &str) -> bool {
    RUNNING.lock().unwrap().as_ref().is_some_and(|s| s.contains(camera))
}

/// Mark the siren as going; false when it already is.
pub fn begin(camera: &str) -> bool {
    RUNNING.lock().unwrap().get_or_insert_with(HashSet::new).insert(camera.to_string())
}

pub fn stop(camera: &str) {
    if let Some(s) = RUNNING.lock().unwrap().as_mut() {
        s.remove(camera);
    }
}

pub fn running() -> Vec<String> {
    RUNNING.lock().unwrap().as_ref().map(|s| s.iter().cloned().collect()).unwrap_or_default()
}

/// The alarm itself, 8 kHz: a wail rising and falling between about 650
/// and 1500 Hz every 1.2 s, with a hard edge so small speakers sound loud.
pub fn wail(seconds: f32) -> Vec<i16> {
    let n = (seconds * 8000.0) as usize;
    let mut out = Vec::with_capacity(n);
    let mut phase = 0f32;
    for i in 0..n {
        let t = i as f32 / 8000.0;
        let cycle = (t / 1.2).fract();
        // Up for most of the cycle, quickly back down.
        let k = if cycle < 0.75 { cycle / 0.75 } else { 1.0 - (cycle - 0.75) / 0.25 };
        let f = 650.0 + 850.0 * k;
        phase = (phase + f / 8000.0).fract();
        let s = (phase * std::f32::consts::TAU).sin();
        // Soft-clipped: louder without crackling.
        let v = (s * 2.2).tanh() / 2.2f32.tanh();
        // A short fade in at the very start.
        let fade = (i as f32 / 400.0).min(1.0);
        out.push((v * fade * 30000.0) as i16);
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn wail_is_loud_and_long_enough() {
        let w = super::wail(2.0);
        assert_eq!(w.len(), 16000);
        assert!(w.iter().map(|s| s.unsigned_abs()).max().unwrap() > 25000);
    }

    #[test]
    fn running_set() {
        assert!(super::begin("x1"));
        assert!(!super::begin("x1"));
        assert!(super::is_running("x1"));
        super::stop("x1");
        assert!(!super::is_running("x1"));
    }
}
