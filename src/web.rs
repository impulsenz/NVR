//! HTTP interface: the web UI, JSON API, live video and file playback.

use crate::config::{CameraConfig, mask_url};
use crate::ffmpeg;
use crate::manager::{Manager, SettingsUpdate};
use crate::storage;
use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use base64::Engine;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;
use tower_http::services::ServeFile;

type App = State<Arc<Manager>>;

pub fn router(mgr: Arc<Manager>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/status", get(status))
        .route("/api/system", get(system))
        .route("/api/settings", get(get_settings).put(put_settings))
        .route("/api/cameras", get(list_cameras).post(add_camera))
        .route(
            "/api/cameras/{id}",
            axum::routing::put(update_camera).delete(delete_camera),
        )
        .route("/api/cameras/{id}/pause", post(pause))
        .route("/api/cameras/{id}/resume", post(resume))
        .route("/api/cameras/{id}/recordings", get(recordings))
        .route(
            "/api/cameras/{id}/recordings/{name}",
            axum::routing::delete(delete_recording),
        )
        .route("/api/probe", post(probe))
        .route("/api/hw-test", post(hw_test))
        .route("/recordings/{id}/{name}", get(serve_recording))
        .route("/live/{id}/stream.mjpeg", get(live_mjpeg))
        .route("/live/{id}/frame.jpg", get(live_frame))
        .route("/snapshot/{id}", get(snapshot))
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

async fn status(State(m): App) -> Response {
    let cfg = m.config();
    let root = cfg.storage.dir.clone();
    let storage = tokio::task::spawn_blocking(move || storage::info(&root))
        .await
        .ok();
    let cameras: Vec<_> = m
        .cameras()
        .iter()
        .map(|c| {
            json!({
                "id": c.cfg.id,
                "name": c.cfg.name,
                "enabled": c.cfg.enabled,
                "record_mode": c.cfg.record_mode,
                "url": mask_url(&c.cfg.url),
                "transcode": c.cfg.transcode,
                "recorder": c.recorder.status(),
                "viewers": c.live.viewers(),
            })
        })
        .collect();
    Json(json!({
        "name": cfg.server.name,
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_seconds": m.started.elapsed().as_secs(),
        "cameras": cameras,
        "storage": storage,
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
        "storage_dir": cfg.storage.dir.canonicalize().unwrap_or(cfg.storage.dir.clone()),
        "listen": m.bound_listen,
        "uptime_seconds": m.started.elapsed().as_secs(),
        "capabilities": caps,
    }))
    .into_response()
}

async fn get_settings(State(m): App) -> Response {
    let mut cfg = m.config();
    let has_password = !cfg.server.password.is_empty();
    cfg.server.password.clear();
    let mut v = serde_json::to_value(&cfg).unwrap();
    v["server"]["has_password"] = json!(has_password);
    v.as_object_mut().unwrap().remove("cameras");
    Json(v).into_response()
}

async fn put_settings(State(m): App, Json(up): Json<SettingsUpdate>) -> Response {
    match m.update_settings(up).await {
        Ok(restart) => Json(json!({ "ok": true, "restart_required": restart })).into_response(),
        Err(e) => bad(e),
    }
}

/// Full camera configs (including stream URLs) for the settings page.
async fn list_cameras(State(m): App) -> Response {
    Json(m.config().cameras).into_response()
}

async fn add_camera(State(m): App, Json(cam): Json<CameraConfig>) -> Response {
    match m.add_camera(cam).await {
        Ok(c) => (StatusCode::CREATED, Json(c)).into_response(),
        Err(e) => bad(e),
    }
}

async fn update_camera(
    State(m): App,
    Path(id): Path<String>,
    Json(cam): Json<CameraConfig>,
) -> Response {
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

async fn delete_camera(
    State(m): App,
    Path(id): Path<String>,
    Query(q): Query<DeleteQuery>,
) -> Response {
    match m.delete_camera(&id, q.delete_recordings).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => bad(e),
    }
}

async fn pause(State(m): App, Path(id): Path<String>) -> Response {
    let c = camera!(m, id);
    c.recorder.pause();
    Json(c.recorder.status()).into_response()
}

async fn resume(State(m): App, Path(id): Path<String>) -> Response {
    let c = camera!(m, id);
    c.recorder.resume();
    Json(c.recorder.status()).into_response()
}

#[derive(Deserialize)]
struct ProbeReq {
    url: String,
    #[serde(default)]
    rtsp_transport: String,
}

async fn probe(State(m): App, Json(req): Json<ProbeReq>) -> Response {
    if req.url.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "enter a stream URL first");
    }
    let ffmpeg = m.config().server.ffmpeg_path;
    Json(ffmpeg::probe(&ffmpeg, req.url.trim(), &req.rtsp_transport).await).into_response()
}

async fn hw_test(State(m): App, Json(hw): Json<crate::config::HardwareConfig>) -> Response {
    let ffmpeg = m.config().server.ffmpeg_path;
    Json(ffmpeg::test_hardware(&ffmpeg, &hw).await).into_response()
}

async fn recordings(State(m): App, Path(id): Path<String>) -> Response {
    let c = camera!(m, id);
    let dir = c.dir();
    let list = tokio::task::spawn_blocking(move || storage::list(&dir))
        .await
        .unwrap_or_default();
    Json(list).into_response()
}

async fn delete_recording(State(m): App, Path((id, name)): Path<(String, String)>) -> Response {
    let c = camera!(m, id);
    let path = match storage::resolve(&c.dir(), &name) {
        Ok(p) => p,
        Err(e) => return err(StatusCode::NOT_FOUND, e),
    };
    match tokio::fs::remove_file(&path).await {
        Ok(()) => {
            tracing::info!("deleted recording {name}");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

/// Serves a recording with HTTP range support so the browser can seek.
async fn serve_recording(
    State(m): App,
    Path((id, name)): Path<(String, String)>,
    req: Request,
) -> Response {
    let root = m.config().storage.dir;
    let path = match storage::camera_dir(&root, &id).and_then(|d| storage::resolve(&d, &name)) {
        Ok(p) => p,
        Err(e) => return err(StatusCode::NOT_FOUND, e),
    };
    let download = req.uri().query().is_some_and(|q| q.contains("download"));
    match ServeFile::new(path).oneshot(req).await {
        Ok(res) => {
            let mut res = res.map(Body::new);
            if download
                && let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\""))
            {
                res.headers_mut().insert(header::CONTENT_DISPOSITION, v);
            }
            res
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn live_mjpeg(State(m): App, Path(id): Path<String>) -> Response {
    let c = camera!(m, id);
    let mut rx = c.live.subscribe();
    drop(c);
    let (tx, mut out) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(2);
    tokio::spawn(async move {
        while rx.changed().await.is_ok() {
            let Some((_, frame)) = rx.borrow_and_update().clone() else {
                continue;
            };
            let head = format!(
                "--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
                frame.len()
            );
            let mut part = Vec::with_capacity(head.len() + frame.len() + 2);
            part.extend_from_slice(head.as_bytes());
            part.extend_from_slice(&frame);
            part.extend_from_slice(b"\r\n");
            if tx.send(Ok(part.into())).await.is_err() {
                break;
            }
        }
    });
    let stream = futures_util::stream::poll_fn(move |cx| out.poll_recv(cx));
    Response::builder()
        .header(
            header::CONTENT_TYPE,
            "multipart/x-mixed-replace; boundary=frame",
        )
        .header(header::CACHE_CONTROL, "no-cache, no-store")
        .body(Body::from_stream(stream))
        .unwrap()
}

async fn live_frame(State(m): App, Path(id): Path<String>) -> Response {
    let c = camera!(m, id);
    match c.live.frame().await {
        Ok(jpg) => (
            [
                (header::CONTENT_TYPE, "image/jpeg"),
                (header::CACHE_CONTROL, "no-cache, no-store"),
            ],
            jpg,
        )
            .into_response(),
        Err(e) => err(StatusCode::SERVICE_UNAVAILABLE, format!("{e:#}")),
    }
}

async fn snapshot(State(m): App, Path(id): Path<String>) -> Response {
    let c = camera!(m, id);
    match c.live.snapshot().await {
        Ok(jpg) => {
            let name = format!(
                "attachment; filename=\"{}_{}.jpg\"",
                c.cfg.id,
                chrono::Local::now().format("%Y-%m-%d_%H-%M-%S")
            );
            let mut res = (
                [
                    (header::CONTENT_TYPE, "image/jpeg"),
                    (header::CACHE_CONTROL, "no-cache"),
                ],
                jpg,
            )
                .into_response();
            if let Ok(v) = HeaderValue::from_str(&name) {
                res.headers_mut().insert(header::CONTENT_DISPOSITION, v);
            }
            res
        }
        Err(e) => err(StatusCode::BAD_GATEWAY, format!("{e:#}")),
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
        .and_then(|b64| {
            base64::engine::general_purpose::STANDARD
                .decode(b64.trim())
                .ok()
        })
        .is_some_and(|creds| constant_time_eq(&creds, expected.as_bytes()));
    if ok {
        next.run(req).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            [(
                header::WWW_AUTHENTICATE,
                "Basic realm=\"NVR\", charset=\"UTF-8\"",
            )],
            "authentication required",
        )
            .into_response()
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
