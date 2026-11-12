//! The overview: every camera in one picture (or only those with something
//! going on), for a wall tablet, a slow connection or Home Assistant. Made
//! from the frames motion detection already decodes, so it costs almost
//! nothing. A private link (with a key) works without signing in.

use crate::manager::{CameraRuntime, Manager};
use std::sync::Arc;

/// The overview picture (JPEG), `w`×`h`. `active_only`: only cameras with
/// motion or an alert right now (all of them when everything is quiet).
/// `allowed`: the cameras the viewer may see (None = all).
pub fn picture(mgr: &Manager, allowed: Option<&[String]>, active_only: bool, w: u32, h: u32) -> Option<Vec<u8>> {
    let cams: Vec<(Arc<CameraRuntime>, crate::imgutil::Frame, bool)> = mgr
        .cameras()
        .into_iter()
        .filter(|c| c.cfg.enabled && allowed.is_none_or(|a| a.contains(&c.cfg.id)))
        .filter_map(|c| {
            let d = c.detector()?;
            let f = d.latest_frame()?;
            let st = d.status();
            let busy = st.event_active || st.motion;
            Some((c, f, busy))
        })
        .collect();
    if cams.is_empty() {
        return None;
    }
    let busy: Vec<_> = cams.iter().filter(|c| c.2).collect();
    let show: Vec<&(Arc<CameraRuntime>, crate::imgutil::Frame, bool)> = if active_only && !busy.is_empty() { busy } else { cams.iter().collect() };
    let n = show.len() as u32;
    let cols = (n as f32).sqrt().ceil() as u32;
    let rows = n.div_ceil(cols);
    let (cw, ch) = (w / cols, h / rows);
    let mut px = vec![0u8; (w * h * 3) as usize];
    let scale = ((cw / 220) as i32).clamp(1, 3);
    for (i, (rt, f, active)) in show.iter().enumerate() {
        let (cx, cy) = ((i as u32 % cols) * cw, (i as u32 / cols) * ch);
        let cell = f.resize(cw, ch);
        for y in 0..ch {
            let src = (y * cw * 3) as usize;
            let dst = (((cy + y) * w + cx) * 3) as usize;
            px[dst..dst + (cw * 3) as usize].copy_from_slice(&cell.rgb[src..src + (cw * 3) as usize]);
        }
        let name = if *active { format!("* {}", rt.cfg.name) } else { rt.cfg.name.clone() };
        let th = 11 * scale;
        crate::draw::caption(&mut px, w, h, cx as i32 + 4 * scale, (cy + ch) as i32 - th - 4 * scale, &name, scale);
    }
    crate::imgutil::encode_jpeg(&px, w, h, 80).ok()
}

/// The key in the private overview link (made once; a new one stops the old link).
pub fn key(data_dir: &std::path::Path, renew: bool) -> String {
    let p = data_dir.join("overview.key");
    if !renew
        && let Ok(k) = std::fs::read_to_string(&p)
        && k.trim().len() >= 32
    {
        return k.trim().to_string();
    }
    use rand_core::RngCore;
    let mut b = [0u8; 24];
    rand_core::OsRng.fill_bytes(&mut b);
    let k: String = b.iter().map(|x| format!("{x:02x}")).collect();
    let _ = std::fs::write(&p, &k);
    k
}

/// Does this request carry the private overview key?
pub fn key_ok(data_dir: &std::path::Path, given: &str) -> bool {
    let want = key(data_dir, false);
    want.len() == given.len() && want.bytes().zip(given.bytes()).fold(0, |a, (x, y)| a | (x ^ y)) == 0
}
