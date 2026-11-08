//! HTTP interface: the web UI, JSON API, live video and file playback.

use crate::config::{CameraConfig, Config, DashboardConfig, mask_url};
use crate::ffmpeg;
use crate::library::{self, Query as LibQuery};
use crate::live::Quality;
use crate::logs::{self, LogQuery};
use crate::manager::{Manager, SettingsUpdate};
use crate::storage;
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{
        DefaultBodyLimit, Path, Query, Request, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post, put},
};
use base64::Engine;
use chrono::{DateTime, Local, NaiveTime};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tower::ServiceExt;
use tower_http::services::ServeFile;

type App = State<Arc<Manager>>;

pub fn router(mgr: Arc<Manager>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/status", get(status))
        .route("/api/system", get(system))
        .route("/api/monitor", get(monitor))
        .route("/api/usage", get(usage))
        .route("/api/logs", get(get_logs))
        .route("/api/logs/download", get(download_logs))
        .route("/api/settings", get(get_settings).put(put_settings))
        .route("/api/dashboard", put(put_dashboard))
        .route("/api/config/export", get(export_config))
        .route("/api/config/import", post(import_config))
        .route("/api/cameras", get(list_cameras).post(add_camera))
        .route("/api/cameras/{id}", put(update_camera).delete(delete_camera))
        .route("/api/cameras/{id}/pause", post(pause))
        .route("/api/cameras/{id}/resume", post(resume))
        .route("/api/cameras/{id}/recordings/{name}", axum::routing::delete(delete_recording))
        .route("/api/cameras/{id}/events/{eid}", axum::routing::delete(delete_event))
        .route("/api/recordings", get(search))
        .route("/api/timeline", get(timeline))
        .route("/api/export", get(export_clip))
        .route("/api/sprite/{id}/{kind}/{name}", get(sprite_info))
        .route("/api/probe", post(probe))
        .route("/api/preview", post(start_preview))
        .route("/api/preview/{token}", get(preview_stream))
        .route("/api/hw-test", post(hw_test))
        .route("/api/ai/test", post(ai_test))
        .route("/api/cloud/test", post(cloud_test))
        .route("/api/live/ws", get(live_ws))
        .route("/media/{id}/rec/{name}", get(serve_recording))
        .route("/media/{id}/events/{name}", get(serve_event_file))
        .route("/media/{id}/thumb/{name}", get(recording_thumb))
        .route("/media/{id}/sprite/{kind}/{name}", get(sprite_image))
        .route("/live/{id}/stream.mjpeg", get(live_mjpeg))
        .route("/live/{id}/frame.jpg", get(live_frame))
        .route("/snapshot/{id}", get(snapshot))
        .layer(DefaultBodyLimit::max(10 * 1024 * 1024))
        .layer(middleware::from_fn_with_state(mgr.clone(), basic_auth))
        .with_state(mgr)
}

fn err(code: StatusCode, e: impl std::fmt::Display) -> Response {
    (code, Json(json!({ "error": e.to_string() }))).into_response()
}

fn bad(e: anyhow::Error) -> Response {
    err(StatusCode::BAD_REQUEST, format!("{e:#}"))
}

macro_rules! camera {
    ($m:expr, $id:expr) => {
        match $m.camera(&$id) {
            Some(c) => c,
            None => return err(StatusCode::NOT_FOUND, "camera not found"),
        }
    };
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../static/index.html"))
}

// ---------- status & system ----------

async fn status(State(m): App) -> Response {
    let cfg = m.config();
    let cameras: Vec<_> = m
        .cameras()
        .iter()
        .map(|c| {
            json!({
                "id": c.cfg.id,
                "name": c.cfg.name,
                "enabled": c.cfg.enabled,
                "record_mode": c.cfg.record_mode,
                "has_sub": c.cfg.has_sub(),
                "url": mask_url(&c.cfg.url),
                "transcode": c.cfg.reencode(),
                "detection": c.cfg.detection_enabled(),
                "ai": c.cfg.detect.ai,
                "recorder": c.recorder.as_ref().map(|r| r.status()),
                "buffer": c.buffer_recorder.as_ref().map(|r| r.status()),
                "detect": c.detector().map(|d| d.status()),
                "live_fps": { "sub": c.live_sub.fps(), "main": c.live_main.fps() },
                "viewers": c.live_sub.viewers() + c.live_main.viewers(),
                "info": { "main": *c.info_main.lock().unwrap(), "sub": *c.info_sub.lock().unwrap() },
            })
        })
        .collect();
    let (warnings, errors) = logs::buffer().problems_since(Local::now() - chrono::Duration::hours(1));
    Json(json!({
        "name": cfg.server.name,
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_seconds": m.started.elapsed().as_secs(),
        "display": cfg.display,
        "live": cfg.live,
        "dashboard": cfg.dashboard,
        "cameras": cameras,
        "ai": m.ai.get().status(),
        "problems": { "warnings": warnings, "errors": errors },
        "monitor": m.monitor.current(),
    }))
    .into_response()
}

async fn system(State(m): App) -> Response {
    let cfg = m.config();
    let caps = ffmpeg::capabilities(&cfg.server.ffmpeg_path).await;
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "config_path": m.cfg_path.canonicalize().unwrap_or(m.cfg_path.clone()),
        "data_dir": m.data_dir.canonicalize().unwrap_or(m.data_dir.clone()),
        "storage_dir": cfg.storage.dir.canonicalize().unwrap_or(cfg.storage.dir.clone()),
        "log_file": logs::buffer().file_path(),
        "listen": m.bound_listen,
        "uptime_seconds": m.started.elapsed().as_secs(),
        "capabilities": caps,
        "ai_objects": crate::ai::SECURITY_OBJECTS,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct MonitorQuery {
    #[serde(default = "default_range")]
    range: i64,
}
fn default_range() -> i64 {
    3600
}

async fn monitor(State(m): App, Query(q): Query<MonitorQuery>) -> Response {
    let range = q.range.clamp(300, 30 * 86_400);
    Json(json!({ "current": m.monitor.current(), "history": m.monitor.history(range, 360), "range": range })).into_response()
}

async fn usage(State(m): App) -> Response {
    Json(m.usage()).into_response()
}

async fn get_logs(Query(q): Query<LogQuery>) -> Response {
    Json(logs::buffer().query(&q)).into_response()
}

async fn download_logs(req: Request) -> Response {
    match logs::buffer().file_path() {
        Some(p) => serve_file(p, req, Some("nvr.log".into()), true).await,
        None => err(StatusCode::NOT_FOUND, "no log file"),
    }
}

// ---------- settings ----------

async fn get_settings(State(m): App) -> Response {
    let mut cfg = m.config();
    let has_password = !cfg.server.password.is_empty();
    let has_key = !cfg.ai.api_key.is_empty();
    cfg.server.password.clear();
    cfg.ai.api_key.clear();
    let mut v = serde_json::to_value(&cfg).unwrap();
    v["server"]["has_password"] = json!(has_password);
    v["ai"]["has_key"] = json!(has_key);
    v.as_object_mut().unwrap().remove("cameras");
    Json(v).into_response()
}

async fn put_settings(State(m): App, Json(up): Json<SettingsUpdate>) -> Response {
    match m.update_settings(up).await {
        Ok(restart) => Json(json!({ "ok": true, "restart_required": restart })).into_response(),
        Err(e) => bad(e),
    }
}

async fn put_dashboard(State(m): App, Json(d): Json<DashboardConfig>) -> Response {
    match m.update_dashboard(d).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => bad(e),
    }
}

#[derive(Deserialize)]
struct ExportCfgQuery {
    #[serde(default)]
    passwords: bool,
}

/// Download the whole setup (cameras, settings, layout) as a file.
async fn export_config(State(m): App, Query(q): Query<ExportCfgQuery>) -> Response {
    let mut cfg = m.config();
    if !q.passwords {
        cfg.server.password.clear();
        cfg.server.username.clear();
        cfg.ai.api_key.clear();
        for c in &mut cfg.cameras {
            c.url = mask_url(&c.url).replace(":****@", ":PASSWORD@");
            c.live_url = mask_url(&c.live_url).replace(":****@", ":PASSWORD@");
        }
    }
    match cfg.to_toml() {
        Ok(text) => {
            let name = format!("nvr-backup-{}.toml", Local::now().format("%Y-%m-%d"));
            (
                [
                    (header::CONTENT_TYPE, "application/toml".to_string()),
                    (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\"")),
                ],
                text,
            )
                .into_response()
        }
        Err(e) => bad(e),
    }
}

#[derive(Deserialize)]
struct ImportQuery {
    #[serde(default = "yes")]
    keep_login: bool,
}
fn yes() -> bool {
    true
}

async fn import_config(State(m): App, Query(q): Query<ImportQuery>, body: String) -> Response {
    let cfg = match Config::from_toml(&body) {
        Ok((c, _)) => c,
        Err(e) => return bad(e),
    };
    let n = cfg.cameras.len();
    match m.import(cfg, q.keep_login).await {
        Ok(restart) => Json(json!({ "ok": true, "cameras": n, "restart_required": restart })).into_response(),
        Err(e) => bad(e),
    }
}

// ---------- cameras ----------

async fn list_cameras(State(m): App) -> Response {
    Json(m.config().cameras).into_response()
}

async fn add_camera(State(m): App, Json(cam): Json<CameraConfig>) -> Response {
    match m.add_camera(cam).await {
        Ok(c) => (StatusCode::CREATED, Json(c)).into_response(),
        Err(e) => bad(e),
    }
}

async fn update_camera(State(m): App, Path(id): Path<String>, Json(cam): Json<CameraConfig>) -> Response {
    match m.update_camera(&id, cam).await {
        Ok(c) => Json(c).into_response(),
        Err(e) => bad(e),
    }
}

#[derive(Deserialize)]
struct DeleteQuery {
    #[serde(default)]
    delete_recordings: bool,
}

async fn delete_camera(State(m): App, Path(id): Path<String>, Query(q): Query<DeleteQuery>) -> Response {
    match m.delete_camera(&id, q.delete_recordings).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => bad(e),
    }
}

async fn pause(State(m): App, Path(id): Path<String>) -> Response {
    let c = camera!(m, id);
    c.pause();
    StatusCode::NO_CONTENT.into_response()
}

async fn resume(State(m): App, Path(id): Path<String>) -> Response {
    let c = camera!(m, id);
    c.resume();
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
struct StreamReq {
    url: String,
    #[serde(default)]
    rtsp_transport: String,
}

async fn probe(State(m): App, Json(req): Json<StreamReq>) -> Response {
    if req.url.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "enter a stream URL first");
    }
    let ffmpeg = m.config().server.ffmpeg_path;
    Json(ffmpeg::probe(&ffmpeg, req.url.trim(), &req.rtsp_transport).await).into_response()
}

/// Register a temporary live preview for a stream that is not saved yet.
async fn start_preview(State(m): App, Json(req): Json<StreamReq>) -> Response {
    if req.url.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "enter a stream URL first");
    }
    let token = format!("{:x}{:x}", Local::now().timestamp_nanos_opt().unwrap_or(0), std::process::id());
    m.previews.lock().unwrap().insert(token.clone(), (req.url.trim().to_string(), req.rtsp_transport, Instant::now()));
    Json(json!({ "url": format!("api/preview/{token}") })).into_response()
}

/// Live MJPEG of a preview stream (ends when the browser disconnects).
async fn preview_stream(State(m): App, Path(token): Path<String>) -> Response {
    let Some((url, transport, _)) = m.previews.lock().unwrap().get(&token).cloned() else {
        return err(StatusCode::NOT_FOUND, "preview expired");
    };
    let ffmpeg = m.config().server.ffmpeg_path;
    let child = tokio::process::Command::new(&ffmpeg)
        .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-fflags", "nobuffer"])
        .args(ffmpeg::input_args(&url, &transport))
        .args(["-an", "-vf", "fps=5,scale='min(640,iw)':-2", "-q:v", "6", "-t", "600", "-f", "image2pipe", "-c:v", "mjpeg", "pipe:1"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let mut stdout = child.stdout.take().unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(2);
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let _child = child; // killed when this task ends
        let mut buf = bytes::BytesMut::new();
        let mut chunk = vec![0u8; 65536];
        let mut scan = 0;
        loop {
            let n = match tokio::time::timeout(Duration::from_secs(20), stdout.read(&mut chunk)).await {
                Ok(Ok(n)) if n > 0 => n,
                _ => return,
            };
            buf.extend_from_slice(&chunk[..n]);
            while let Some(frame) = crate::live::next_jpeg(&mut buf, &mut scan) {
                if tx.send(Ok(multipart_part(&frame))).await.is_err() {
                    return;
                }
            }
        }
    });
    mjpeg_response(futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx)))
}

async fn hw_test(State(m): App, Json(hw): Json<crate::config::HardwareConfig>) -> Response {
    let ffmpeg = m.config().server.ffmpeg_path;
    Json(ffmpeg::test_hardware(&ffmpeg, &hw).await).into_response()
}

#[derive(Deserialize)]
struct AiTestReq {
    camera: String,
    /// Settings to test (unsaved); uses the saved ones when absent.
    ai: Option<crate::config::AiConfig>,
}

/// Run object recognition once on a camera's current picture.
async fn ai_test(State(m): App, Json(req): Json<AiTestReq>) -> Response {
    let c = camera!(m, req.camera);
    let frame = match c.detector().and_then(|d| d.latest_frame()) {
        Some(f) => f,
        None => match grab_frame(&m, &c).await {
            Ok(f) => f,
            Err(e) => return bad(e),
        },
    };
    let ai = match req.ai {
        Some(mut cfg) => {
            if cfg.api_key.is_empty() {
                cfg.api_key = m.config().ai.api_key;
            }
            let h = crate::ai::AiHandle::new(cfg, &m.data_dir);
            h.get()
        }
        None => m.ai.get(),
    };
    // A freshly created built-in engine needs a moment to load its model.
    let t = Instant::now();
    let mut res = ai.detect(&frame).await;
    while matches!(res, Err(crate::ai::AiError::NotReady(_))) && t.elapsed() < Duration::from_secs(120) {
        tokio::time::sleep(Duration::from_millis(500)).await;
        res = ai.detect(&frame).await;
    }
    let image = frame.jpeg(85).map(|j| format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(j)));
    match res {
        Ok(d) => Json(json!({ "ok": true, "detections": d, "image": image.ok(), "ms": t.elapsed().as_millis() as u64 }))
            .into_response(),
        Err(e) => Json(json!({ "ok": false, "error": e.to_string(), "image": image.ok() })).into_response(),
    }
}

/// One decoded RGB frame from the sub-stream, for AI tests when detection is off.
async fn grab_frame(m: &Manager, c: &crate::manager::CameraRuntime) -> anyhow::Result<crate::imgutil::Frame> {
    let ffmpeg = m.config().server.ffmpeg_path;
    let (w, h) = (640u32, 360u32);
    let out = tokio::time::timeout(
        Duration::from_secs(20),
        tokio::process::Command::new(ffmpeg)
            .args(["-hide_banner", "-loglevel", "error"])
            .args(ffmpeg::input_args(c.cfg.live_url(), &c.cfg.rtsp_transport))
            .args(["-frames:v", "1", "-vf", &format!("scale={w}:{h}"), "-pix_fmt", "rgb24", "-f", "rawvideo", "pipe:1"])
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    if out.stdout.len() != (w * h * 3) as usize {
        anyhow::bail!("could not get a picture from the camera");
    }
    Ok(crate::imgutil::Frame { width: w, height: h, rgb: Arc::new(out.stdout) })
}

async fn cloud_test(Json(c): Json<crate::config::CloudConfig>) -> Response {
    match crate::cloud::test(&c).await {
        Ok(msg) => Json(json!({ "ok": true, "message": msg })).into_response(),
        Err(e) => Json(json!({ "ok": false, "message": format!("{e:#}") })).into_response(),
    }
}

// ---------- recordings ----------

#[derive(Deserialize)]
struct SearchQuery {
    #[serde(default)]
    cameras: String,
    from: Option<DateTime<chrono::FixedOffset>>,
    to: Option<DateTime<chrono::FixedOffset>>,
    tod_from: Option<String>,
    tod_to: Option<String>,
    #[serde(default)]
    types: String,
    limit: Option<usize>,
}

fn split_list(s: &str) -> Vec<String> {
    s.split(',').map(str::trim).filter(|x| !x.is_empty()).map(String::from).collect()
}

fn range(from: Option<DateTime<chrono::FixedOffset>>, to: Option<DateTime<chrono::FixedOffset>>) -> (DateTime<Local>, DateTime<Local>) {
    let to = to.map(|t| t.with_timezone(&Local)).unwrap_or_else(Local::now);
    let from = from.map(|t| t.with_timezone(&Local)).unwrap_or(to - chrono::Duration::days(1));
    (from, to)
}

async fn search(State(m): App, Query(q): Query<SearchQuery>) -> Response {
    let (from, to) = range(q.from, q.to);
    let t = |s: &Option<String>| s.as_deref().and_then(|x| NaiveTime::parse_from_str(x, "%H:%M").ok());
    let tod = match (t(&q.tod_from), t(&q.tod_to)) {
        (Some(a), Some(b)) => Some((a, b)),
        (Some(a), None) => Some((a, NaiveTime::from_hms_opt(23, 59, 59).unwrap())),
        (None, Some(b)) => Some((NaiveTime::MIN, b)),
        _ => None,
    };
    let lq = LibQuery {
        cameras: split_list(&q.cameras),
        from,
        to,
        tod,
        types: split_list(&q.types).into_iter().collect(),
        limit: q.limit.unwrap_or(500).min(5000),
    };
    let items = tokio::task::spawn_blocking(move || library::search(&m, &lq)).await.unwrap_or_default();
    Json(items).into_response()
}

async fn timeline(State(m): App, Query(q): Query<SearchQuery>) -> Response {
    let (from, to) = range(q.from, q.to);
    let cams = split_list(&q.cameras);
    let t = tokio::task::spawn_blocking(move || library::timeline(&m, &cams, from, to)).await.unwrap_or_default();
    Json(t).into_response()
}

#[derive(Deserialize)]
struct ExportQuery {
    camera: String,
    from: DateTime<chrono::FixedOffset>,
    to: DateTime<chrono::FixedOffset>,
}

/// Cut a time range out of the recordings and download it as one MP4.
async fn export_clip(State(m): App, Query(q): Query<ExportQuery>, req: Request) -> Response {
    let c = camera!(m, q.camera);
    let (from, to) = (q.from.with_timezone(&Local), q.to.with_timezone(&Local));
    if to <= from {
        return err(StatusCode::BAD_REQUEST, "the end must be after the start");
    }
    if (to - from).num_hours() > 24 {
        return err(StatusCode::BAD_REQUEST, "exports are limited to 24 hours");
    }
    let parts = library::export_parts(&c, from, to);
    if parts.is_empty() {
        return err(StatusCode::NOT_FOUND, "no recordings in that time range");
    }
    let name = format!("{}_{}_to_{}.mp4", c.cfg.id, from.format("%Y-%m-%d_%H-%M-%S"), to.format("%H-%M-%S"));
    let out = m.data_dir.join("tmp").join(&name);
    let ffmpeg = m.config().server.ffmpeg_path;
    if let Err(e) = ffmpeg::export(&ffmpeg, &parts, &out).await {
        return bad(e);
    }
    tracing::info!("[{}] exported clip {name}", c.cfg.name);
    serve_file(out, req, Some(name), true).await
}

async fn delete_recording(State(m): App, Path((id, name)): Path<(String, String)>) -> Response {
    let c = camera!(m, id);
    if storage::resolve(&c.dir, &name).is_err() {
        return err(StatusCode::NOT_FOUND, "recording not found");
    }
    match storage::delete_recording(&c.dir, &name) {
        Ok(()) => {
            tracing::info!("[{}] deleted recording {name}", c.cfg.name);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn delete_event(State(m): App, Path((id, eid)): Path<(String, String)>) -> Response {
    let c = camera!(m, id);
    if c.events.delete(&eid) {
        StatusCode::NO_CONTENT.into_response()
    } else {
        err(StatusCode::NOT_FOUND, "event not found")
    }
}

/// Serve a file with range support. It is sent as a download when `name` is
/// given and either `force` is set or the URL has `?download`.
async fn serve_file(path: PathBuf, req: Request, name: Option<String>, force: bool) -> Response {
    let download = force || req.uri().query().is_some_and(|q| q.contains("download"));
    match ServeFile::new(path).oneshot(req).await {
        Ok(res) => {
            let mut res = res.map(Body::new);
            if let Some(name) = name.filter(|_| download) {
                if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
                    res.headers_mut().insert(header::CONTENT_DISPOSITION, v);
                }
            }
            res
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn serve_recording(State(m): App, Path((id, name)): Path<(String, String)>, req: Request) -> Response {
    let c = camera!(m, id);
    match storage::resolve(&c.dir, &name) {
        Ok(p) => serve_file(p, req, Some(name), false).await,
        Err(e) => err(StatusCode::NOT_FOUND, e),
    }
}

async fn serve_event_file(State(m): App, Path((id, name)): Path<(String, String)>, req: Request) -> Response {
    let c = camera!(m, id);
    match storage::resolve(c.events.dir(), &name) {
        Ok(p) => serve_file(p, req, Some(name), false).await,
        Err(e) => err(StatusCode::NOT_FOUND, e),
    }
}

/// Resolve "rec"/"clip" + name to a file of the camera.
fn media_path(c: &crate::manager::CameraRuntime, kind: &str, name: &str) -> anyhow::Result<PathBuf> {
    match kind {
        "clip" => storage::resolve(c.events.dir(), name),
        _ => storage::resolve(&c.dir, name),
    }
}

/// Only a couple of preview jobs at a time.
static THUMB_JOBS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(3);

fn fresh(cache: &std::path::Path, src: &std::path::Path) -> bool {
    let (Ok(c), Ok(s)) = (std::fs::metadata(cache), std::fs::metadata(src)) else { return false };
    match (c.modified(), s.modified()) {
        // A file still being written changes; refresh its preview every minute.
        (Ok(ct), Ok(st)) => ct + Duration::from_secs(60) >= st,
        _ => false,
    }
}

async fn recording_thumb(State(m): App, Path((id, name)): Path<(String, String)>, req: Request) -> Response {
    let c = camera!(m, id);
    let Ok(src) = storage::resolve(&c.dir, &name) else { return err(StatusCode::NOT_FOUND, "not found") };
    let cache = c.dir.join(".thumbs").join(format!("{name}.jpg"));
    if !fresh(&cache, &src) {
        let _p = THUMB_JOBS.acquire().await;
        if !fresh(&cache, &src) {
            let _ = std::fs::create_dir_all(c.dir.join(".thumbs"));
            if let Err(e) = ffmpeg::thumbnail(&m.config().server.ffmpeg_path, &src, &cache).await {
                return err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"));
            }
        }
    }
    serve_file(cache, req, None, false).await
}

async fn ensure_sprite(m: &Manager, c: &crate::manager::CameraRuntime, kind: &str, name: &str) -> anyhow::Result<(PathBuf, ffmpeg::SpriteInfo)> {
    let src = media_path(c, kind, name)?;
    let dir = src.parent().unwrap().join(".thumbs");
    let img = dir.join(format!("{name}.sprite.jpg"));
    let meta = dir.join(format!("{name}.sprite.json"));
    let load = || -> Option<ffmpeg::SpriteInfo> { serde_json::from_slice(&std::fs::read(&meta).ok()?).ok() };
    if fresh(&img, &src) {
        if let Some(i) = load() {
            return Ok((img, i));
        }
    }
    let _p = THUMB_JOBS.acquire().await;
    if fresh(&img, &src) {
        if let Some(i) = load() {
            return Ok((img, i));
        }
    }
    std::fs::create_dir_all(&dir)?;
    let info = ffmpeg::sprite(&m.config().server.ffmpeg_path, &src, &img).await?;
    std::fs::write(&meta, serde_json::to_vec(&info)?)?;
    Ok((img, info))
}

async fn sprite_info(State(m): App, Path((id, kind, name)): Path<(String, String, String)>) -> Response {
    let c = camera!(m, id);
    match ensure_sprite(&m, &c, &kind, &name).await {
        Ok((_, info)) => Json(json!({
            "info": info,
            "url": format!("media/{id}/sprite/{kind}/{name}"),
        }))
        .into_response(),
        Err(e) => err(StatusCode::NOT_FOUND, format!("{e:#}")),
    }
}

async fn sprite_image(State(m): App, Path((id, kind, name)): Path<(String, String, String)>, req: Request) -> Response {
    let c = camera!(m, id);
    match ensure_sprite(&m, &c, &kind, &name).await {
        Ok((img, _)) => serve_file(img, req, None, false).await,
        Err(e) => err(StatusCode::NOT_FOUND, format!("{e:#}")),
    }
}

// ---------- live ----------

fn multipart_part(frame: &[u8]) -> Bytes {
    let head = format!("--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n", frame.len());
    let mut part = Vec::with_capacity(head.len() + frame.len() + 2);
    part.extend_from_slice(head.as_bytes());
    part.extend_from_slice(frame);
    part.extend_from_slice(b"\r\n");
    part.into()
}

fn mjpeg_response(stream: impl futures_util::Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static) -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, "multipart/x-mixed-replace; boundary=frame")
        .header(header::CACHE_CONTROL, "no-cache, no-store")
        .body(Body::from_stream(stream))
        .unwrap()
}

#[derive(Deserialize)]
struct QualityQuery {
    #[serde(default = "sub")]
    q: Quality,
}
fn sub() -> Quality {
    Quality::Sub
}

async fn live_mjpeg(State(m): App, Path(id): Path<String>, Query(q): Query<QualityQuery>) -> Response {
    let c = camera!(m, id);
    let mut rx = c.live(q.q).subscribe();
    drop(c);
    let (tx, mut out) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(2);
    tokio::spawn(async move {
        while rx.changed().await.is_ok() {
            let Some((_, frame)) = rx.borrow_and_update().clone() else { continue };
            if tx.send(Ok(multipart_part(&frame))).await.is_err() {
                break;
            }
        }
    });
    mjpeg_response(futures_util::stream::poll_fn(move |cx| out.poll_recv(cx)))
}

async fn live_frame(State(m): App, Path(id): Path<String>, Query(q): Query<QualityQuery>) -> Response {
    let c = camera!(m, id);
    match c.live(q.q).frame().await {
        Ok(jpg) => ([(header::CONTENT_TYPE, "image/jpeg"), (header::CACHE_CONTROL, "no-cache, no-store")], jpg)
            .into_response(),
        Err(e) => err(StatusCode::SERVICE_UNAVAILABLE, format!("{e:#}")),
    }
}

async fn snapshot(State(m): App, Path(id): Path<String>) -> Response {
    let c = camera!(m, id);
    match c.live_main.snapshot().await {
        Ok(jpg) => {
            let name = format!(
                "attachment; filename=\"{}_{}.jpg\"",
                c.cfg.id,
                Local::now().format("%Y-%m-%d_%H-%M-%S")
            );
            let mut res = ([(header::CONTENT_TYPE, "image/jpeg"), (header::CACHE_CONTROL, "no-cache")], jpg)
                .into_response();
            if let Ok(v) = HeaderValue::from_str(&name) {
                res.headers_mut().insert(header::CONTENT_DISPOSITION, v);
            }
            res
        }
        Err(e) => err(StatusCode::BAD_GATEWAY, format!("{e:#}")),
    }
}

/// All live video goes over one WebSocket: the browser says which cameras it
/// wants at which quality and frame rate; the server pushes JPEG frames.
/// Binary message layout: [quality: 0 sub / 1 main][id length][id][JPEG].
async fn live_ws(State(m): App, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| live_socket(socket, m))
}

#[derive(Deserialize, Clone, PartialEq)]
struct WsSub {
    camera: String,
    q: Quality,
    fps: f32,
}

#[derive(Deserialize)]
struct WsMsg {
    subs: Vec<WsSub>,
}

async fn live_socket(mut socket: WebSocket, m: Arc<Manager>) {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Bytes>(16);
    let mut tasks: HashMap<(String, u8), (WsSub, tokio::task::JoinHandle<()>)> = HashMap::new();
    loop {
        tokio::select! {
            msg = socket.recv() => {
                let Some(Ok(msg)) = msg else { break };
                let Message::Text(text) = msg else { continue };
                let Ok(req) = serde_json::from_str::<WsMsg>(&text) else { continue };
                let wanted: HashMap<(String, u8), WsSub> = req
                    .subs
                    .into_iter()
                    .map(|s| ((s.camera.clone(), (s.q == Quality::Main) as u8), s))
                    .collect();
                tasks.retain(|k, (sub, h)| {
                    let keep = wanted.get(k) == Some(sub);
                    if !keep {
                        h.abort();
                    }
                    keep
                });
                for (k, sub) in wanted {
                    if tasks.contains_key(&k) {
                        continue;
                    }
                    let h = tokio::spawn(feed(m.clone(), sub.clone(), tx.clone()));
                    tasks.insert(k, (sub, h));
                }
            }
            Some(frame) = rx.recv() => {
                if socket.send(Message::Binary(frame)).await.is_err() {
                    break;
                }
            }
        }
    }
    for (_, (_, h)) in tasks {
        h.abort();
    }
}

/// Push one camera's frames to a socket, at most `fps` per second.
async fn feed(m: Arc<Manager>, sub: WsSub, tx: tokio::sync::mpsc::Sender<Bytes>) {
    let min_gap = Duration::from_secs_f32(1.0 / sub.fps.clamp(0.2, 30.0));
    let qb = (sub.q == Quality::Main) as u8;
    let id = sub.camera.as_bytes();
    loop {
        // The camera may be restarted by a settings change: re-subscribe.
        let Some(c) = m.camera(&sub.camera) else { return };
        let mut rx = c.live(sub.q).subscribe();
        drop(c);
        let mut last = Instant::now() - min_gap;
        // Send the latest frame immediately, if there is one.
        let mut first = true;
        loop {
            if !first && rx.changed().await.is_err() {
                break;
            }
            let frame = rx.borrow_and_update().clone();
            if first {
                first = false;
                if frame.as_ref().is_none_or(|f| f.0.elapsed() > Duration::from_secs(5)) {
                    continue;
                }
            }
            let Some((_, jpg)) = frame else { continue };
            if last.elapsed() < min_gap {
                continue;
            }
            last = Instant::now();
            let mut msg = Vec::with_capacity(jpg.len() + id.len() + 2);
            msg.push(qb);
            msg.push(id.len() as u8);
            msg.extend_from_slice(id);
            msg.extend_from_slice(&jpg);
            // Drop frames rather than queue them if the browser is slow.
            if tx.try_send(msg.into()).is_err() && tx.is_closed() {
                return;
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Optional HTTP basic auth for every route. Reads the current settings so
/// login changes apply immediately.
async fn basic_auth(State(m): App, req: Request, next: Next) -> Response {
    let server = m.config().server;
    if server.username.is_empty() {
        return next.run(req).await;
    }
    let expected = format!("{}:{}", server.username, server.password);
    let ok = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64.trim()).ok())
        .is_some_and(|creds| constant_time_eq(&creds, expected.as_bytes()));
    if ok {
        next.run(req).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Basic realm=\"NVR\", charset=\"UTF-8\"")],
            "authentication required",
        )
            .into_response()
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
