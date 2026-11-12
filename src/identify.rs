//! After an alert ends: look for faces and number plates in a full-quality
//! picture of its best moment, and name the people and vehicles GAZE has
//! been taught (Settings > People & vehicles).

use crate::events::{Event, EventStore, FaceSeen, SeenBox};
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
    // Continuous recording, if it's the full-quality stream (not stream 2).
    let hd = rt.cfg.record_mode.continuous_stream() == Some(false) || !rt.cfg.has_sub();
    if !hd {
        return None;
    }
    let recs = crate::library::recordings(rt);
    let r = recs.iter().find(|r| r.start <= t && t <= r.end)?;
    Some((
        rt.dir.join(&r.name),
        ((t - r.start).num_milliseconds() as f64 / 1000.0).max(0.0),
    ))
}

/// One picture at `offset` seconds into a video, full size.
async fn grab(ffmpeg: &str, file: &std::path::Path, offset: f64) -> anyhow::Result<Frame> {
    let out = crate::load::background(&mut crate::load::cmd(ffmpeg))
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

/// Full-quality pictures spread over an alert (other than its best
/// moment), each with the objects the local AI finds in it. Without a
/// local AI the boxes are empty.
pub async fn moments(
    mgr: &Manager,
    rt: &CameraRuntime,
    e: &Event,
    n: usize,
) -> Vec<(Frame, Vec<SeenBox>)> {
    timed_moments(mgr, rt, e, n)
        .await
        .into_iter()
        .map(|(_, f, b)| (f, b))
        .collect()
}

/// [`moments`], with the time of each picture.
pub async fn timed_moments(
    mgr: &Manager,
    rt: &CameraRuntime,
    e: &Event,
    n: usize,
) -> Vec<(chrono::DateTime<chrono::Local>, Frame, Vec<SeenBox>)> {
    let cfg = mgr.config();
    let ai = mgr.ai.get();
    let local = rt.cfg.detect.ai && ai.cfg.provider == "builtin";
    let span = (e.end - e.start).num_milliseconds().max(1000);
    let mut out = Vec::new();
    for i in 1..=n {
        let t = e.start + chrono::Duration::milliseconds(span * i as i64 / (n as i64 + 1));
        if e.thumb_at
            .is_some_and(|b| (t - b).num_milliseconds().abs() < 700)
        {
            continue; // that's the picture already looked at
        }
        let Some((file, off)) = hd_source(rt, e, t) else {
            continue;
        };
        let Ok(frame) = grab(&cfg.server.ffmpeg_path, &file, off).await else {
            continue;
        };
        let boxes = if local {
            ai.detect(&frame)
                .await
                .unwrap_or_default()
                .into_iter()
                .filter(|d| {
                    d.score >= rt.cfg.detect.min_confidence.min(0.45)
                        && rt.cfg.detect.objects.contains(&d.label)
                })
                .map(|d| SeenBox {
                    label: d.label,
                    score: d.score,
                    x: d.x,
                    y: d.y,
                    w: d.w,
                    h: d.h,
                })
                .collect()
        } else {
            Vec::new()
        };
        out.push((t, frame, boxes));
    }
    out
}

/// Faces of the people (boxes) in one picture, added to the alert.
/// Returns how many new faces were found.
async fn faces_in(
    mgr: &Manager,
    store: &EventStore,
    e: &mut Event,
    frame: &Frame,
    boxes: &[SeenBox],
    tag: &str,
) -> anyhow::Result<usize> {
    let (fw, fh) = (frame.width as f32, frame.height as f32);
    // Faces already taken in this picture (overlapping person boxes would
    // otherwise find the same face twice).
    let mut taken: Vec<[f32; 4]> = Vec::new();
    let mut found = 0;
    for (bi, b) in boxes
        .iter()
        .enumerate()
        .filter(|(_, b)| b.label == "person")
    {
        let (x, y, w, h) = (b.x * fw, b.y * fh, b.w * fw, b.h * fh);
        // Faces are in the top part of a person.
        let (cx, cy) = (
            (x - w * 0.15).max(0.0).floor(),
            (y - h * 0.05).max(0.0).floor(),
        );
        let c = crate::recog::crop(frame, x - w * 0.15, y - h * 0.05, w * 1.3, h * 0.6);
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
            // The same person in another picture of this alert: keep one,
            // preferring the one that could be recognised.
            if let Some(same) = e.faces.iter_mut().find(|f| {
                crate::known::cosine(&crate::known::unpack(&f.print), &print) >= SAME_PERSON
            }) {
                if same.name.is_some() || name.is_none() {
                    continue;
                }
                same.name = name.clone();
            }
            let pic = crate::recog::crop(
                &c,
                face.x - face.w * 0.3,
                face.y - face.h * 0.3,
                face.w * 1.6,
                face.h * 1.6,
            )
            .fit(160);
            let file = format!("{}-face{tag}{bi}-{}.jpg", e.id, e.faces.len());
            std::fs::write(store.dir().join(&file), pic.jpeg(85)?)?;
            e.faces.retain(|f| {
                !(f.name.is_some()
                    && f.name == name
                    && crate::known::cosine(&crate::known::unpack(&f.print), &print) >= SAME_PERSON
                    && f.file != file)
            });
            e.faces.push(FaceSeen {
                file,
                print: crate::known::pack(&print),
                name,
            });
            found += 1;
        }
    }
    Ok(found)
}

/// Two faceprints this similar are the same person (looser than naming).
const SAME_PERSON: f32 = 0.5;

async fn identify(
    mgr: &Manager,
    rt: &CameraRuntime,
    store: &EventStore,
    mut e: Event,
) -> anyhow::Result<()> {
    let cfg = mgr.config();
    let (want_faces, want_plates) = (cfg.ai.faces, cfg.ai.plates);
    let t = e.thumb_at.unwrap_or(e.start + chrono::Duration::seconds(1));
    // Faces and plates only from full-quality video: the small stream 2
    // picture isn't sharp enough and gives wrong names.
    let Some((file, off)) = hd_source(rt, &e, t) else {
        e.identified = true;
        store.save(e);
        return Ok(());
    };
    let frame = grab(&cfg.server.ffmpeg_path, &file, off).await?;
    let (fw, fh) = (frame.width as f32, frame.height as f32);
    let mut notes: Vec<String> = Vec::new();
    if want_faces {
        let boxes = e.boxes.clone();
        faces_in(mgr, store, &mut e, &frame, &boxes, "").await?;
        // No face, or nobody recognised: the best moment may have been a
        // bad picture (turned away, blurred). Look through more of the
        // full-quality video.
        let person = e.boxes.iter().any(|b| b.label == "person")
            || e.labels.iter().any(|l| l.label == "person");
        if person && !e.faces.iter().any(|f| f.name.is_some()) {
            for (k, (f, boxes)) in moments(mgr, rt, &e, 6).await.into_iter().enumerate() {
                faces_in(mgr, store, &mut e, &f, &boxes, &format!("m{k}-")).await?;
                if e.faces.iter().any(|f| f.name.is_some()) {
                    break; // found someone we know
                }
            }
        }
        let mut named: Vec<String> = e.faces.iter().filter_map(|f| f.name.clone()).collect();
        named.dedup();
        for n in named {
            notes.push(format!("Recognized {n}"));
        }
        if e.faces.iter().any(|f| f.name.is_none()) {
            notes.push("Unknown face".into());
        }
    }
    for b in e.boxes.clone().iter() {
        let (x, y, w, h) = (b.x * fw, b.y * fh, b.w * fw, b.h * fh);
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

/// Replace alert pictures with full-quality ones from the recordings
/// (System → Tasks → Refresh alert pictures).
pub async fn refresh_pictures(mgr: Arc<Manager>, days: i64) {
    let cutoff = chrono::Local::now() - chrono::Duration::days(days);
    let ffmpeg = mgr.config().server.ffmpeg_path.clone();
    let mut todo = Vec::new();
    for rt in mgr.cameras() {
        for e in rt.events.all() {
            if e.start > cutoff && e.thumb.as_deref().is_some_and(|t| !t.ends_with("-hd.jpg")) {
                todo.push((rt.clone(), e));
            }
        }
    }
    todo.sort_by(|a, b| b.1.start.cmp(&a.1.start));
    let job = crate::jobs::start(
        "previews",
        format!("Sharper alert pictures (last {days} days)"),
    );
    job.total(todo.len() as u64);
    let (mut done, mut skipped) = (0, 0);
    for (rt, mut e) in todo {
        job.waiting();
        crate::load::wait_for_cpu(80, Duration::from_secs(300)).await;
        job.pause_point().await;
        job.step(format!(
            "{} — {}",
            rt.cfg.name,
            e.start.format("%d/%m %H:%M:%S")
        ));
        job.done_one();
        if sharpen(&ffmpeg, &rt, &mut e).await {
            rt.events.save(e);
            done += 1;
        } else {
            skipped += 1;
        }
    }
    info!("alert pictures: {done} made sharper from recordings, {skipped} without a recording");
}

/// A saved file with the moment `t` in it: the full-quality footage if
/// there is some, otherwise any recording of the camera (stream 2 or the
/// timeline copy).
fn saved_source(
    rt: &CameraRuntime,
    e: &Event,
    t: chrono::DateTime<chrono::Local>,
) -> Option<(PathBuf, f64)> {
    if let Some(s) = hd_source(rt, e, t) {
        return Some(s);
    }
    for (dir, recs) in [
        (rt.dir.clone(), crate::library::recordings(rt)),
        (
            crate::storage::timeline_dir(&rt.dir),
            crate::library::timeline_recordings(rt),
        ),
    ] {
        if let Some(r) = recs.iter().find(|r| r.start <= t && t <= r.end) {
            return Some((
                dir.join(&r.name),
                ((t - r.start).num_milliseconds() as f64 / 1000.0).max(0.0),
            ));
        }
    }
    None
}

/// Small grey pictures (ALIGN_W × ALIGN_H) of `file` from `from` seconds,
/// 4 a second for `secs` seconds.
const ALIGN_W: usize = 96;
const ALIGN_H: usize = 54;
async fn strip(ffmpeg: &str, file: &std::path::Path, from: f64, secs: f64) -> Vec<Vec<u8>> {
    let out = crate::load::background(&mut crate::load::cmd(ffmpeg))
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-ss",
            &format!("{from:.2}"),
            "-i",
        ])
        .arg(file)
        .args([
            "-t",
            &format!("{secs:.2}"),
            "-an",
            "-vf",
            &format!("fps=4,scale={ALIGN_W}:{ALIGN_H},format=gray"),
            "-f",
            "rawvideo",
            "pipe:1",
        ])
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await;
    out.map(|o| {
        o.stdout
            .chunks_exact(ALIGN_W * ALIGN_H)
            .map(<[u8]>::to_vec)
            .collect()
    })
    .unwrap_or_default()
}

fn grey_small(f: &Frame) -> Vec<u8> {
    let s = f.resize(ALIGN_W as u32, ALIGN_H as u32);
    s.rgb
        .chunks_exact(3)
        .map(|p| ((p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000) as u8)
        .collect()
}

/// How different two small grey pictures are (0 = the same), with the
/// part where the object was counting four times as much.
fn difference(a: &[u8], b: &[u8], focus: Option<[f32; 4]>) -> f64 {
    let inside = |i: usize| {
        focus.is_some_and(|[x, y, w, h]| {
            let (px, py) = (
                (i % ALIGN_W) as f32 / ALIGN_W as f32,
                (i / ALIGN_W) as f32 / ALIGN_H as f32,
            );
            px >= x && px <= x + w && py >= y && py <= y + h
        })
    };
    let (mut sum, mut n) = (0.0, 0.0);
    for (i, (p, q)) in a.iter().zip(b).enumerate() {
        let wgt = if inside(i) { 4.0 } else { 1.0 };
        sum += wgt * (*p as f64 - *q as f64).abs();
        n += wgt;
    }
    sum / n.max(1.0)
}

/// The moment in a saved file that matches the live picture of an alert.
/// The time of the live picture is only roughly known (the live stream
/// and the recording run a second or two apart, and the AI takes a
/// moment), so the few seconds around it are compared with the live
/// picture and the closest one is used. None when nothing there looks like
/// it (that moment isn't in the file properly).
async fn matching_moment(
    ffmpeg: &str,
    file: &std::path::Path,
    off: f64,
    live: &Frame,
    focus: Option<[f32; 4]>,
) -> Option<f64> {
    const BEFORE: f64 = 3.0;
    const AFTER: f64 = 2.0;
    let from = (off - BEFORE).max(0.0);
    let pics = strip(ffmpeg, file, from, off + AFTER - from).await;
    if pics.is_empty() {
        return None;
    }
    let want = grey_small(live);
    let (best, diff) = pics
        .iter()
        .enumerate()
        // When pictures look alike (nothing moving), the one nearest the
        // expected time.
        .map(|(k, p)| {
            let t = from + k as f64 * 0.25;
            (k, difference(p, &want, focus) + (t - off).abs() * 0.05)
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))?;
    // Grey levels apart on average: a different scene altogether.
    (diff < 40.0).then_some(from + best as f64 * 0.25)
}

/// Replace an alert's picture with the same moment from the saved
/// recording (full quality when there is one). False when that moment
/// isn't in a saved file (yet).
pub async fn sharpen(ffmpeg: &str, rt: &CameraRuntime, e: &mut Event) -> bool {
    let t = e.thumb_at.unwrap_or(e.start + chrono::Duration::seconds(1));
    let Some((file, off)) = saved_source(rt, e, t) else {
        return false;
    };
    let live = e
        .thumb
        .as_deref()
        .and_then(|n| std::fs::read(rt.events.dir().join(n)).ok())
        .and_then(|j| crate::imgutil::decode_jpeg(&j).ok());
    let off = match &live {
        Some(l) => match matching_moment(
            ffmpeg,
            &file,
            off,
            l,
            e.focus
                .or_else(|| e.boxes.first().map(|b| [b.x, b.y, b.w, b.h])),
        )
        .await
        {
            Some(o) => o,
            // Nothing in the recording looks like what was seen live: keep
            // the live picture rather than show the wrong moment.
            None => return false,
        },
        None => off,
    };
    let Ok(frame) = grab(ffmpeg, &file, off).await else {
        return false;
    };
    let name = format!("{}-hd.jpg", e.id);
    let dir = rt.events.dir();
    let tmp = dir.join(format!("{name}.tmp"));
    let Ok(jpg) = frame.fit(1280).jpeg(85) else {
        return false;
    };
    if std::fs::write(&tmp, jpg).is_err() || std::fs::rename(&tmp, dir.join(&name)).is_err() {
        return false;
    }
    if let Some(old) = e.thumb.replace(name) {
        let _ = std::fs::remove_file(dir.join(old));
    }
    true
}

/// Every alert's picture is made from the saved recording as soon as that
/// moment has been written (the live picture is only a stand-in until
/// then). Tried a few times, in case the file wasn't ready.
async fn sharpen_new(
    mgr: &Manager,
    tries: &mut std::collections::HashMap<String, (u8, chrono::DateTime<chrono::Local>)>,
) {
    let ffmpeg = mgr.config().server.ffmpeg_path;
    let now = chrono::Local::now();
    for rt in mgr.cameras() {
        let todo: Vec<Event> = rt
            .events
            .between(now - chrono::Duration::hours(24), now)
            .into_iter()
            .filter(|e| e.thumb.as_deref().is_some_and(|t| !t.ends_with("-hd.jpg")))
            // The moment of the picture has been written to the file.
            .filter(|e| (now - e.thumb_at.unwrap_or(e.start)).num_seconds() > 12)
            // Each picture (it can change while the alert goes on) at most
            // five times, a minute apart.
            .filter(|e| {
                let key = format!("{}:{}", e.id, e.thumb.as_deref().unwrap_or(""));
                tries
                    .get(&key)
                    .is_none_or(|(n, last)| *n < 5 && (now - *last).num_seconds() >= 60)
            })
            .take(10)
            .collect();
        for mut e in todo {
            let key = format!("{}:{}", e.id, e.thumb.as_deref().unwrap_or(""));
            let n = tries.get(&key).map(|x| x.0).unwrap_or(0);
            tries.insert(key, (n + 1, chrono::Local::now()));
            crate::load::wait_for_cpu(90, Duration::from_secs(30)).await;
            if sharpen(&ffmpeg, &rt, &mut e).await {
                // Keep what was added meanwhile (faces, notes).
                if let Some(mut cur) = rt.events.get(&e.id) {
                    cur.thumb = e.thumb;
                    rt.events.save(cur);
                }
            }
        }
    }
    if tries.len() > 5000 {
        tries.clear();
    }
}

/// Background job: alert pictures from the saved recordings.
async fn pictures_from_recordings(mgr: Arc<Manager>) {
    tokio::time::sleep(Duration::from_secs(20)).await;
    let mut tries = std::collections::HashMap::new();
    loop {
        sharpen_new(&mgr, &mut tries).await;
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

/// Background job: alerts with people or vehicles, newest first.
pub async fn run(mgr: Arc<Manager>) {
    tokio::spawn(pictures_from_recordings(mgr.clone()));
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
                            && !rt.in_progress(e)
                    })
                    .filter(|e| {
                        e.boxes
                            .iter()
                            .any(|b| b.label == "person" || VEHICLES.contains(&b.label.as_str()))
                    })
                    .collect();
                todo.sort_by(|a, b| b.start.cmp(&a.start));
                todo.truncate(20);
                if todo.is_empty() {
                    continue;
                }
                let job = crate::jobs::start("ai", format!("Faces & plates — {}", rt.cfg.name));
                job.total(todo.len() as u64);
                for e in todo {
                    job.waiting();
                    crate::load::wait_for_cpu(80, Duration::from_secs(300)).await;
                    job.pause_point().await;
                    job.step(format!("Alert at {}", e.start.format("%d/%m %H:%M:%S")));
                    let res = identify(&mgr, &rt, &store, e.clone()).await;
                    job.done_one();
                    if let Err(err) = res {
                        let msg = format!("{err:#}");
                        if msg != last_err {
                            warn!("faces & plates: {msg}");
                            last_err = msg.clone();
                        }
                        *mgr.recog.error.lock().unwrap() = Some(msg.clone());
                        // Don't try the same alert forever.
                        job.problem(&msg);
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
