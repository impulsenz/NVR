//! HTTP interface: the web UI, a small JSON API, live MJPEG and file playback.

use crate::config::{Config, mask_url};
use crate::live::Live;
use crate::recorder::Recorder;
use crate::storage;
use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use base64::Engine;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::broadcast::error::RecvError;
use tower::ServiceExt;
use tower_http::services::ServeFile;

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub recorder: Arc<Recorder>,
    pub live: Arc<Live>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/status", get(status))
        .route("/api/record/start", post(record_start))
        .route("/api/record/stop", post(record_stop))
        .route("/api/recordings", get(recordings))
        .route(
            "/api/recordings/{name}",
            axum::routing::delete(delete_recording),
        )
        .route("/recordings/{name}", get(serve_recording))
        .route("/live.mjpeg", get(live_mjpeg))
        .route("/snapshot.jpg", get(snapshot))
        .layer(middleware::from_fn_with_state(state.clone(), basic_auth))
        .with_state(state)
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../static/index.html"))
}

async fn status(State(s): State<AppState>) -> impl IntoResponse {
    let dir = s.cfg.recording.dir.clone();
    let storage = tokio::task::spawn_blocking(move || storage::info(&dir))
        .await
        .ok();
    Json(json!({
        "camera": {
            "name": s.cfg.camera.name,
            "url": mask_url(&s.cfg.camera.url),
        },
        "recorder": s.recorder.status(),
        "storage": storage,
        "retention": {
            "days": s.cfg.recording.retention_days,
            "max_storage_gb": s.cfg.recording.max_storage_gb,
            "segment_seconds": s.cfg.recording.segment_seconds,
        },
        "live_viewers": s.live.viewers(),
    }))
}

async fn record_start(State(s): State<AppState>) -> impl IntoResponse {
    s.recorder.start();
    Json(s.recorder.status())
}

async fn record_stop(State(s): State<AppState>) -> impl IntoResponse {
    s.recorder.stop();
    Json(s.recorder.status())
}

async fn recordings(State(s): State<AppState>) -> impl IntoResponse {
    let dir = s.cfg.recording.dir.clone();
    let list = tokio::task::spawn_blocking(move || storage::list(&dir))
        .await
        .unwrap_or_default();
    Json(list)
}

async fn delete_recording(State(s): State<AppState>, Path(name): Path<String>) -> Response {
    let path = match storage::resolve(&s.cfg.recording.dir, &name) {
        Ok(p) => p,
        Err(e) => return (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    };
    match tokio::fs::remove_file(&path).await {
        Ok(()) => {
            tracing::info!("deleted recording {name}");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Serves a recording with HTTP range support so the browser can seek.
async fn serve_recording(
    State(s): State<AppState>,
    Path(name): Path<String>,
    req: Request,
) -> Response {
    let path = match storage::resolve(&s.cfg.recording.dir, &name) {
        Ok(p) => p,
        Err(e) => return (StatusCode::NOT_FOUND, e.to_string()).into_response(),
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
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn live_mjpeg(State(s): State<AppState>) -> Response {
    let mut rx = s.live.subscribe();
    let stream = async_stream(move |yield_| async move {
        loop {
            match rx.recv().await {
                Ok(frame) => {
                    let head = format!(
                        "--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
                        frame.len()
                    );
                    let mut part = Vec::with_capacity(head.len() + frame.len() + 2);
                    part.extend_from_slice(head.as_bytes());
                    part.extend_from_slice(&frame);
                    part.extend_from_slice(b"\r\n");
                    if yield_
                        .send(Ok::<_, std::io::Error>(bytes::Bytes::from(part)))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            }
        }
    });
    Response::builder()
        .header(
            header::CONTENT_TYPE,
            "multipart/x-mixed-replace; boundary=frame",
        )
        .header(header::CACHE_CONTROL, "no-cache, no-store")
        .body(Body::from_stream(stream))
        .unwrap()
}

async fn snapshot(State(s): State<AppState>) -> Response {
    match s.live.snapshot().await {
        Ok(jpg) => {
            let name = format!(
                "attachment; filename=\"{}_{}.jpg\"",
                s.cfg.camera_slug(),
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
        Err(e) => (StatusCode::BAD_GATEWAY, format!("{e:#}")).into_response(),
    }
}

/// Optional HTTP basic auth for every route.
async fn basic_auth(State(s): State<AppState>, req: Request, next: Next) -> Response {
    let (user, pass) = (&s.cfg.server.username, &s.cfg.server.password);
    if user.is_empty() && pass.is_empty() {
        return next.run(req).await;
    }
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
        .and_then(|raw| String::from_utf8(raw).ok())
        .is_some_and(|creds| {
            constant_time_eq(creds.as_bytes(), format!("{user}:{pass}").as_bytes())
        });
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

/// Tiny channel-backed stream builder (avoids an extra dependency).
fn async_stream<T, F, Fut>(f: F) -> impl futures_util::Stream<Item = T>
where
    T: Send + 'static,
    F: FnOnce(tokio::sync::mpsc::Sender<T>) -> Fut,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let (tx, mut rx) = tokio::sync::mpsc::channel::<T>(2);
    tokio::spawn(f(tx));
    futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx))
}
