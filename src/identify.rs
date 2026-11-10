//! After an alert ends: look for faces and number plates in a full-quality
//! picture of its best moment, and name the people and vehicles GAZE has
//! been taught (Settings > People & vehicles).

use crate::events::{Event, EventStore, FaceSeen};
use crate::imgutil::Frame;
use crate::manager::{CameraRuntime, Manager};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

const VEHICLES: &[&str] = &["car", "truck", "bus", "motorcycle"];

/// The full-quality file and position for a moment of an alert.
fn hd_source(
    rt: &CameraRuntime,
    e: &Event,
    t: chrono::DateTime<chrono::Local>,
) -> Option<(PathBuf, f64)> {
    if let (Some(clip), Some(cs)) = (&e.clip, e.clip_start) {
        let off = (t - cs).num_milliseconds() as f64 / 1000.0;
        return Some((rt.events.dir().join(clip), off.max(0.0)));
    }
    // Continuous recording (full quality unless the camera records stream 2 only).
    let recs = crate::library::recordings(rt);
    let r = recs.iter().find(|r| r.start <= t && t <= r.end)?;
    Some((
        rt.dir.join(&r.name),
        ((t - r.start).num_milliseconds() as f64 / 1000.0).max(0.0),
    ))
}

/// One picture at `offset` seconds into a video, full size.
async fn grab(ffmpeg: &str, file: &std::path::Path, offset: f64) -> anyhow::Result<Frame> {
    let out = crate::load::background(&mut tokio::process::Command::new(ffmpeg))
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-ss",
            &format!("{offset:.2}"),
            "-i",
        ])
        .arg(file)
        .args([
            "-frames:v",
            "1",
            "-f",
            "image2pipe",
            "-c:v",
            "mjpeg",
            "-q:v",
            "2",
            "pipe:1",
        ])
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await?;
    anyhow::ensure!(
        out.status.success() && !out.stdout.is_empty(),
        "no picture at {offset:.1} s"
    );
    crate::imgutil::decode_jpeg(&out.stdout)
}

fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let ix = (a[0] + a[2]).min(b[0] + b[2]) - a[0].max(b[0]);
    let iy = (a[1] + a[3]).min(b[1] + b[3]) - a[1].max(b[1]);
    if ix <= 0.0 || iy <= 0.0 {
        return 0.0;
    }
    let i = ix * iy;
    i / (a[2] * a[3] + b[2] * b[3] - i)
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

async fn identify(
    mgr: &Manager,
    rt: &CameraRuntime,
    store: &EventStore,
    mut e: Event,
) -> anyhow::Result<()> {
    let cfg = mgr.config();
    let (want_faces, want_plates) = (cfg.ai.faces, cfg.ai.plates);
    let t = e.thumb_at.unwrap_or(e.start + chrono::Duration::seconds(1));
    let frame = match hd_source(rt, &e, t) {
        Some((file, off)) => grab(&cfg.server.ffmpeg_path, &file, off).await?,
        None => {
            // No recording at that moment: the alert picture is all there is.
            let thumb = e
                .thumb
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("no picture"))?;
            crate::imgutil::decode_jpeg(&std::fs::read(store.dir().join(thumb))?)?
        }
    };
    let (fw, fh) = (frame.width as f32, frame.height as f32);
    let mut notes: Vec<String> = Vec::new();
    // Faces already taken (picture coordinates): overlapping person boxes
    // would otherwise find the same face twice.
    let mut taken: Vec<[f32; 4]> = Vec::new();
    for (bi, b) in e.boxes.clone().iter().enumerate() {
        let (x, y, w, h) = (b.x * fw, b.y * fh, b.w * fw, b.h * fh);
        if want_faces && b.label == "person" {
            // Faces are in the top part of a person.
            let (cx, cy) = (
                (x - w * 0.15).max(0.0).floor(),
                (y - h * 0.05).max(0.0).floor(),
            );
            let c = crate::recog::crop(&frame, x - w * 0.15, y - h * 0.05, w * 1.3, h * 0.6);
            for face in mgr.recog.faces(&c).await? {
                if face.score < 0.75 || face.w < 32.0 {
                    continue; // too unsure or too small to recognise
                }
                let r = [face.x + cx, face.y + cy, face.w, face.h];
                if taken.iter().any(|t| iou(t, &r) > 0.3) {
                    continue;
                }
                taken.push(r);
                let print = mgr.recog.faceprint(&c, &face).await?;
                let name = mgr.known.match_face(&print).map(|(n, _)| n);
                // A picture of the face for "who is this?".
                let pic = crate::recog::crop(
                    &c,
                    face.x - face.w * 0.3,
                    face.y - face.h * 0.3,
                    face.w * 1.6,
                    face.h * 1.6,
                )
                .fit(160);
                let file = format!("{}-face{}-{}.jpg", e.id, bi, e.faces.len());
                std::fs::write(store.dir().join(&file), pic.jpeg(85)?)?;
                match &name {
                    Some(n) => notes.push(format!("Recognized {n}")),
                    None => notes.push("Unknown face".into()),
                }
                e.faces.push(FaceSeen {
                    file,
                    print: crate::known::pack(&print),
                    name,
                });
            }
        }
        if VEHICLES.contains(&b.label.as_str())
            && cfg.ai.smart_search
            && mgr.known.list().iter().any(|k| k.looks > 0)
        {
            // A taught vehicle that looks the same (CLIP).
            let c = crate::recog::crop(&frame, x, y, w, h);
            if let Ok(v) = mgr.clip.embed_image(&c).await
                && let Some((n, _)) = mgr.known.match_look(&v)
            {
                notes.push(format!("Looks like {n}"));
            }
        }
        if want_plates && VEHICLES.contains(&b.label.as_str()) {
            let c = crate::recog::crop(&frame, x, y, w, h);
            for p in mgr.recog.plates(&c).await? {
                if e.plates.contains(&p.text) {
                    continue;
                }
                let owner = mgr.known.match_plate(&p.text);
                notes.push(match &owner {
                    Some(o) => format!("Plate {} — {o}", p.text),
                    None => format!("{} plate {}", cap(&b.label), p.text),
                });
                e.plates.push(p.text);
            }
        }
    }
    // "Recognized Mum" once, however many pictures of her there are.
    let mut seen = std::collections::HashSet::new();
    notes.retain(|n| seen.insert(n.clone()));
    for n in &notes {
        if !e.notes.contains(n) {
            e.notes.push(n.clone());
        }
    }
    if !notes.is_empty() {
        info!(target: "event", "[{}] {}", rt.cfg.name, notes.join("; "));
    }
    e.identified = true;
    store.save(e);
    Ok(())
}

/// Background job: alerts with people or vehicles, newest first.
pub async fn run(mgr: Arc<Manager>) {
    tokio::time::sleep(Duration::from_secs(40)).await;
    let mut last_err = String::new();
    loop {
        let cfg = mgr.config();
        if cfg.ai.faces || cfg.ai.plates {
            let cutoff = chrono::Local::now() - chrono::Duration::days(7);
            for rt in mgr.cameras() {
                let store = rt.events.clone();
                let mut todo: Vec<Event> = store
                    .all()
                    .into_iter()
                    .filter(|e| {
                        !e.identified
                            && e.start > cutoff
                            && (chrono::Local::now() - e.end).num_seconds() > 20
                    })
                    .filter(|e| {
                        e.boxes
                            .iter()
                            .any(|b| b.label == "person" || VEHICLES.contains(&b.label.as_str()))
                    })
                    .collect();
                todo.sort_by(|a, b| b.start.cmp(&a.start));
                for e in todo.into_iter().take(20) {
                    crate::load::wait_for_cpu(80, Duration::from_secs(300)).await;
                    if let Err(err) = identify(&mgr, &rt, &store, e.clone()).await {
                        let msg = format!("{err:#}");
                        if msg != last_err {
                            warn!("faces & plates: {msg}");
                            last_err = msg.clone();
                        }
                        *mgr.recog.error.lock().unwrap() = Some(msg);
                        // Don't try the same alert forever.
                        let mut done = e;
                        done.identified = true;
                        store.save(done);
                    }
                }
            }
        }
        mgr.recog.unload_if_idle();
        tokio::time::sleep(Duration::from_secs(15)).await;
    }
}
