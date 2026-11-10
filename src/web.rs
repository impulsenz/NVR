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
        .route(
            "/app.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../static/app.js"),
                )
            }),
        )
        .route(
            "/app.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../static/app.css"),
                )
            }),
        )
        .route("/api/status", get(status))
        .route("/api/system", get(system))
        .route("/api/monitor", get(monitor))
        .route("/api/usage", get(usage))
        .route("/api/logs", get(get_logs))
        .route("/api/logs/download", get(download_logs))
        .route("/api/settings", get(get_settings).put(put_settings))
        .route("/api/dashboard", put(put_dashboard))
        .route("/api/profiles", get(get_profiles).put(put_profiles))
        .route("/api/client", get(client_info))
        .route("/api/alerts/seen", post(alerts_seen))
        .route("/api/alerts/seen-all", post(alerts_seen_all))
        .route("/api/alerts/unseen", post(alerts_unseen))
        .route("/api/config/export", get(export_config))
        .route("/api/config/import", post(import_config))
        .route("/api/cameras", get(list_cameras).post(add_camera))
        .route(
            "/api/cameras/{id}",
            put(update_camera).delete(delete_camera),
        )
        .route("/api/cameras/{id}/pause", post(pause))
        .route("/api/cameras/{id}/view", put(set_view))
        .route("/api/cameras/{id}/ptz/move", post(ptz_move))
        .route("/api/cameras/{id}/device", get(device_get))
        .route("/api/cameras/{id}/device/network", put(device_network))
        .route("/api/cameras/{id}/device/time", put(device_time))
        .route("/api/cameras/{id}/device/video", put(device_video))
        .route("/api/cameras/{id}/device/imaging", put(device_imaging))
        .route("/api/cameras/{id}/device/reboot", post(device_reboot))
        .route("/api/cameras/{id}/ptz/stop", post(ptz_stop))
        .route(
            "/api/cameras/{id}/ptz/presets",
            get(ptz_presets).post(ptz_save_preset),
        )
        .route("/api/cameras/{id}/ptz/goto", post(ptz_goto))
        .route("/api/cameras/{id}/ptz/remove", post(ptz_remove))
        .route("/api/ptz/test", post(ptz_test))
        .route("/api/cameras/{id}/resume", post(resume))
        .route(
            "/api/cameras/{id}/recordings/{name}",
            axum::routing::delete(delete_recording),
        )
        .route(
            "/api/cameras/{id}/events/{eid}",
            axum::routing::delete(delete_event),
        )
        .route("/api/recordings", get(search))
        .route("/api/timeline", get(timeline))
        .route("/api/export", get(export_clip))
        .route("/api/sprite/{id}/{kind}/{name}", get(sprite_info))
        .route("/api/probe", post(probe))
        .route("/api/fs", get(fs_list))
        .route("/api/fs/mkdir", post(fs_mkdir))
        .route("/api/preview", post(start_preview))
        .route("/api/preview/{token}", get(preview_stream))
        .route("/api/hw-test", post(hw_test))
        .route("/api/ai/test", post(ai_test))
        .route("/api/ai/models", post(ai_models))
        .route("/api/cameras/{id}/motion", get(camera_motion))
        .route("/api/cloud/test", post(cloud_test))
        .route("/api/live/ws", get(live_ws))
        .route("/media/{id}/rec/{name}", get(serve_recording))
        .route("/media/{id}/events/{name}", get(serve_event_file))
        .route("/media/{id}/thumb/{name}", get(recording_thumb))
        .route("/media/{id}/play/{kind}/{name}", get(play_converted))
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
            // Options that change without a restart come from the current config.
            let cur = cfg
                .cameras
                .iter()
                .find(|x| x.id == c.cfg.id)
                .unwrap_or(&c.cfg);
            json!({
                "osd": cur.osd,
                "view": cur.view,
                "ptz": cur.ptz.enabled,
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
                "info": { "main": c.relay_main.info(), "sub": c.relay_sub.info() },
                "streams": {
                    "main": c.relay_main.status(),
                    "sub": c.cfg.has_sub().then(|| c.relay_sub.status()),
                },
            })
        })
        .collect();
    let (warnings, errors) =
        logs::buffer().problems_since(Local::now() - chrono::Duration::hours(1));
    Json(json!({
        "name": cfg.server.name,
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_seconds": m.started.elapsed().as_secs(),
        "display": cfg.display,
        "live": cfg.live,
        "dashboard": cfg.dashboard,
        "cameras": cameras,
        "ai": m.ai.get().status(),
        "smart_search": m.clip.status(),
        "alerts_unseen": library::unseen_count(&m),
        "smart_search_on": cfg.ai.smart_search,
        "problems": { "warnings": warnings, "errors": errors, "error_ids": logs::buffer().error_ids_since(Local::now() - chrono::Duration::days(1)) },
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
        "addresses": local_addresses(),
        "uptime_seconds": m.started.elapsed().as_secs(),
        "capabilities": caps,
        "ai_objects": crate::ai::SECURITY_OBJECTS,
    }))
    .into_response()
}

/// This computer's network (IPv4) addresses, main one first.
pub fn local_addresses() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    // The address used to reach other networks (no packet is sent).
    for probe in ["8.8.8.8:80", "192.168.1.1:80", "10.0.0.1:80"] {
        if let Ok(sock) = std::net::UdpSocket::bind("0.0.0.0:0")
            && sock.connect(probe).is_ok()
            && let Ok(a) = sock.local_addr()
        {
            let ip = a.ip().to_string();
            if !a.ip().is_loopback() && !a.ip().is_unspecified() && !out.contains(&ip) {
                out.push(ip);
            }
        }
    }
    out
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

/// Where this browser connects from, so it can use the data saver
/// automatically away from home (Tailscale, the internet).
async fn client_info(
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: axum::http::HeaderMap,
) -> Response {
    let mut ip = addr.ip();
    // Behind a proxy on this computer (e.g. `tailscale serve`): the real client.
    if ip.is_loopback()
        && let Some(f) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .and_then(|v| v.trim().parse().ok())
    {
        ip = f;
    }
    Json(json!({ "ip": ip.to_string(), "remote": is_remote(ip) })).into_response()
}

/// Not on the home network: Tailscale addresses and anything public.
pub fn is_remote(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    let ip = match ip {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    };
    match ip {
        IpAddr::V4(v) => {
            let o = v.octets();
            // Tailscale (100.64.0.0/10) counts as remote.
            if o[0] == 100 && (64..128).contains(&o[1]) {
                return true;
            }
            !(v.is_loopback() || v.is_private() || v.is_link_local() || v.is_unspecified())
        }
        IpAddr::V6(v) => {
            let s = v.segments();
            // Tailscale fd7a:115c:a1e0::/48 counts as remote.
            if s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0 {
                return true;
            }
            !(v.is_loopback()
                || (s[0] & 0xfe00) == 0xfc00
                || (s[0] & 0xffc0) == 0xfe80
                || v.is_unspecified())
        }
    }
}

#[derive(Deserialize)]
struct SeenReq {
    #[serde(default)]
    ids: Vec<String>,
    #[serde(default)]
    camera: String,
    #[serde(default)]
    id: String,
}

async fn alerts_seen(State(m): App, Json(r): Json<SeenReq>) -> Response {
    m.seen.mark(&r.ids);
    StatusCode::NO_CONTENT.into_response()
}

async fn alerts_seen_all(State(m): App) -> Response {
    m.seen.mark_all();
    StatusCode::NO_CONTENT.into_response()
}

async fn alerts_unseen(State(m): App, Json(r): Json<SeenReq>) -> Response {
    let Some(c) = m.camera(&r.camera) else {
        return err(StatusCode::NOT_FOUND, "camera not found");
    };
    match c.events.get(&r.id) {
        Some(e) => {
            m.seen.unmark(&e.id, e.start);
            StatusCode::NO_CONTENT.into_response()
        }
        None => err(StatusCode::NOT_FOUND, "alert not found"),
    }
}

async fn get_profiles(State(m): App) -> Response {
    Json(m.config().profiles).into_response()
}

async fn put_profiles(State(m): App, Json(p): Json<Vec<crate::config::Profile>>) -> Response {
    match m.update_profiles(p).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
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
                    (
                        header::CONTENT_DISPOSITION,
                        format!("attachment; filename=\"{name}\""),
                    ),
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
        Ok(restart) => {
            Json(json!({ "ok": true, "cameras": n, "restart_required": restart })).into_response()
        }
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

#[derive(Deserialize)]
struct MoveQuery {
    #[serde(default)]
    move_recordings: bool,
}

async fn update_camera(
    State(m): App,
    Path(id): Path<String>,
    Query(q): Query<MoveQuery>,
    Json(cam): Json<CameraConfig>,
) -> Response {
    match m.update_camera(&id, cam, q.move_recordings).await {
        Ok(c) => Json(c).into_response(),
        Err(e) => bad(e),
    }
}

#[derive(Deserialize)]
struct FsQuery {
    #[serde(default)]
    path: String,
}

/// Folders on the NVR computer, for choosing where recordings go.
async fn fs_list(Query(q): Query<FsQuery>) -> Response {
    match tokio::task::spawn_blocking(move || crate::fsbrowse::list(&q.path)).await {
        Ok(Ok(l)) => Json(l).into_response(),
        Ok(Err(e)) => bad(e),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[derive(Deserialize)]
struct MkdirReq {
    parent: String,
    name: String,
}

async fn fs_mkdir(Json(r): Json<MkdirReq>) -> Response {
    match crate::fsbrowse::mkdir(&r.parent, &r.name) {
        Ok(p) => Json(json!({ "path": p })).into_response(),
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

async fn set_view(
    State(m): App,
    Path(id): Path<String>,
    Json(v): Json<crate::config::ViewConfig>,
) -> Response {
    match m.set_camera_view(&id, v).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => bad(e),
    }
}

// ---------- PTZ ----------

/// ONVIF connection for camera settings: the address and login from the
/// PTZ tab, or else from the stream URL (PTZ need not be turned on).
fn device_target(m: &Manager, id: &str) -> anyhow::Result<crate::onvif::Target> {
    let cfg = m.config();
    let cam = cfg
        .cameras
        .iter()
        .find(|c| c.id == id)
        .ok_or_else(|| anyhow::anyhow!("camera not found"))?;
    crate::onvif::Target::from(cam, &cam.ptz)
}

async fn device_get(State(m): App, Path(id): Path<String>) -> Response {
    let t = match device_target(&m, &id) {
        Ok(t) => t,
        Err(e) => return bad(e),
    };
    let r = crate::onvif::Onvif::default().device_report(&t).await;
    Json(json!({ "report": r, "address": format!("{}:{}", t.host, t.port), "computer_utc": chrono::Utc::now() })).into_response()
}

#[derive(Deserialize)]
struct NetworkReq {
    #[serde(flatten)]
    net: crate::onvif_device::Network,
    /// Also change the camera's stream addresses in the NVR.
    #[serde(default)]
    update_nvr: bool,
}

async fn device_network(
    State(m): App,
    Path(id): Path<String>,
    Json(r): Json<NetworkReq>,
) -> Response {
    let res = async {
        let t = device_target(&m, &id)?;
        let onvif = crate::onvif::Onvif::default();
        let reboot = onvif.set_network(&t, &r.net).await?;
        // Some cameras only switch address after restarting: restart it now,
        // on the address it still has.
        let mut restarted = false;
        if reboot {
            restarted = onvif.reboot(&t).await.is_ok();
        }
        let new_ip = r.net.ip.trim().to_string();
        let mut updated = false;
        if r.update_nvr && !r.net.dhcp && !new_ip.is_empty() && new_ip != t.host {
            updated = m.change_camera_host(&id, &t.host, &new_ip).await?;
        }
        anyhow::Ok(json!({ "ok": true, "reboot_needed": reboot, "restarted": restarted, "nvr_updated": updated }))
    }
    .await;
    match res {
        Ok(v) => Json(v).into_response(),
        Err(e) => bad(e),
    }
}

#[derive(Deserialize)]
struct TimeReq {
    #[serde(flatten)]
    time: crate::onvif_device::TimeSettings,
    /// Set the camera's clock to this computer's time now.
    #[serde(default)]
    sync_clock: bool,
}

async fn device_time(State(m): App, Path(id): Path<String>, Json(r): Json<TimeReq>) -> Response {
    let res = async {
        let t = device_target(&m, &id)?;
        crate::onvif::Onvif::default()
            .set_time(&t, &r.time, r.sync_clock.then(chrono::Utc::now))
            .await
    }
    .await;
    match res {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => bad(e),
    }
}

async fn device_video(
    State(m): App,
    Path(id): Path<String>,
    Json(r): Json<crate::onvif_device::VideoChange>,
) -> Response {
    let res = async {
        let t = device_target(&m, &id)?;
        crate::onvif::Onvif::default().set_video(&t, &r).await?;
        // New resolution or format: start recording into a new file with it.
        let m2 = m.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            m2.reconnect_camera(&id).await;
        });
        anyhow::Ok(())
    }
    .await;
    match res {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => bad(e),
    }
}

async fn device_imaging(
    State(m): App,
    Path(id): Path<String>,
    Json(r): Json<crate::onvif_device::ImagingChange>,
) -> Response {
    let res = async {
        let t = device_target(&m, &id)?;
        crate::onvif::Onvif::default().set_imaging(&t, &r).await
    }
    .await;
    match res {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => bad(e),
    }
}

async fn device_reboot(State(m): App, Path(id): Path<String>) -> Response {
    let res = async {
        let t = device_target(&m, &id)?;
        crate::onvif::Onvif::default().reboot(&t).await
    }
    .await;
    match res {
        Ok(msg) => Json(json!({ "ok": true, "message": msg })).into_response(),
        Err(e) => bad(e),
    }
}

fn ptz_target(m: &Manager, id: &str) -> anyhow::Result<crate::onvif::Target> {
    let cfg = m.config();
    let cam = cfg
        .cameras
        .iter()
        .find(|c| c.id == id)
        .ok_or_else(|| anyhow::anyhow!("camera not found"))?;
    if !cam.ptz.enabled {
        anyhow::bail!("PTZ is not turned on for this camera (camera settings → PTZ)");
    }
    crate::onvif::Target::from(cam, &cam.ptz)
}

#[derive(Deserialize)]
struct PtzMove {
    #[serde(default)]
    pan: f32,
    #[serde(default)]
    tilt: f32,
    #[serde(default)]
    zoom: f32,
}

async fn ptz_move(State(m): App, Path(id): Path<String>, Json(mv): Json<PtzMove>) -> Response {
    let r = async {
        let t = ptz_target(&m, &id)?;
        let cfg = m.config();
        let p = &cfg.cameras.iter().find(|c| c.id == id).unwrap().ptz;
        let (sx, sy) = (
            if p.invert_pan { -1.0 } else { 1.0 },
            if p.invert_tilt { -1.0 } else { 1.0 },
        );
        m.onvif(&id)
            .move_continuous(
                &t,
                mv.pan * p.speed * sx,
                mv.tilt * p.speed * sy,
                mv.zoom * p.speed,
            )
            .await
    }
    .await;
    match r {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => bad(e),
    }
}

async fn ptz_stop(State(m): App, Path(id): Path<String>) -> Response {
    let r = async { m.onvif(&id).stop(&ptz_target(&m, &id)?).await }.await;
    match r {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => bad(e),
    }
}

async fn ptz_presets(State(m): App, Path(id): Path<String>) -> Response {
    let r = async { m.onvif(&id).presets(&ptz_target(&m, &id)?).await }.await;
    match r {
        Ok(p) => Json(p).into_response(),
        Err(e) => bad(e),
    }
}

#[derive(Deserialize)]
struct PresetReq {
    #[serde(default)]
    token: String,
    #[serde(default)]
    name: String,
}

async fn ptz_goto(State(m): App, Path(id): Path<String>, Json(r): Json<PresetReq>) -> Response {
    let res = async {
        m.onvif(&id)
            .goto_preset(&ptz_target(&m, &id)?, &r.token)
            .await
    }
    .await;
    match res {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => bad(e),
    }
}

async fn ptz_save_preset(
    State(m): App,
    Path(id): Path<String>,
    Json(r): Json<PresetReq>,
) -> Response {
    if r.name.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "enter a name for the preset");
    }
    let res = async {
        m.onvif(&id)
            .set_preset(&ptz_target(&m, &id)?, r.name.trim())
            .await
    }
    .await;
    match res {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => bad(e),
    }
}

async fn ptz_remove(State(m): App, Path(id): Path<String>, Json(r): Json<PresetReq>) -> Response {
    let res = async {
        m.onvif(&id)
            .remove_preset(&ptz_target(&m, &id)?, &r.token)
            .await
    }
    .await;
    match res {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => bad(e),
    }
}

#[derive(Deserialize)]
struct PtzTestReq {
    camera: CameraConfig,
}

/// Check PTZ settings from the camera editor (before saving).
async fn ptz_test(Json(r): Json<PtzTestReq>) -> Response {
    let res = async {
        let t = crate::onvif::Target::from(&r.camera, &r.camera.ptz)?;
        crate::onvif::Onvif::default().presets(&t).await
    }
    .await;
    match res {
        Ok(p) => Json(json!({ "ok": true, "presets": p })).into_response(),
        Err(e) => Json(json!({ "ok": false, "error": format!("{e:#}") })).into_response(),
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

/// A running camera connection for this URL, if the NVR already has one.
/// Using it avoids opening another session on the camera (many cameras
/// only allow two or three).
fn running_stream(m: &Manager, url: &str) -> Option<(Arc<crate::manager::CameraRuntime>, Quality)> {
    m.cameras().into_iter().find_map(|c| {
        if c.cfg.url == url {
            Some((c, Quality::Main))
        } else if c.cfg.has_sub() && c.cfg.live_url == url {
            Some((c, Quality::Sub))
        } else {
            None
        }
    })
}

async fn probe(State(m): App, Json(req): Json<StreamReq>) -> Response {
    if req.url.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "enter a stream URL first");
    }
    let ffmpeg = m.config().server.ffmpeg_path;
    if let Some((c, q)) = running_stream(&m, req.url.trim()) {
        let relay = if q == Quality::Main {
            c.relay_main.clone()
        } else {
            c.relay_sub.clone()
        };
        let _connect = relay.subscribe();
        let info = relay.wait_info(Duration::from_secs(20)).await;
        let mut res = ffmpeg::ProbeResult {
            ok: info.is_some(),
            ..Default::default()
        };
        match info {
            Some(i) => res.streams.push(i),
            None => {
                res.error = Some(
                    relay
                        .status()
                        .error
                        .unwrap_or_else(|| "no picture from the camera yet".into()),
                )
            }
        }
        return Json(res).into_response();
    }
    Json(ffmpeg::probe(&ffmpeg, req.url.trim(), &req.rtsp_transport).await).into_response()
}

/// Register a temporary live preview for a stream that is not saved yet.
async fn start_preview(State(m): App, Json(req): Json<StreamReq>) -> Response {
    if req.url.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "enter a stream URL first");
    }
    // Already connected: show the NVR's own live stream instead of a new session.
    if let Some((c, q)) = running_stream(&m, req.url.trim()) {
        let q = if q == Quality::Main { "main" } else { "sub" };
        return Json(json!({ "url": format!("live/{}/stream.mjpeg?q={q}", c.cfg.id) }))
            .into_response();
    }
    let token = format!(
        "{:x}{:x}",
        Local::now().timestamp_nanos_opt().unwrap_or(0),
        std::process::id()
    );
    m.previews.lock().unwrap().insert(
        token.clone(),
        (
            req.url.trim().to_string(),
            req.rtsp_transport,
            Instant::now(),
        ),
    );
    Json(json!({ "url": format!("api/preview/{token}") })).into_response()
}

/// Live MJPEG of a preview stream (ends when the browser disconnects).
async fn preview_stream(State(m): App, Path(token): Path<String>) -> Response {
    let Some((url, transport, _)) = m.previews.lock().unwrap().get(&token).cloned() else {
        return err(StatusCode::NOT_FOUND, "preview expired");
    };
    let ffmpeg = m.config().server.ffmpeg_path;
    let child = tokio::process::Command::new(&ffmpeg)
        .args([
            "-hide_banner",
            "-nostdin",
            "-loglevel",
            "error",
            "-fflags",
            "nobuffer",
        ])
        .args(ffmpeg::input_args(&url, &transport))
        .args([
            "-an",
            "-vf",
            "fps=5,scale='min(640,iw)':-2",
            "-q:v",
            "6",
            "-t",
            "600",
            "-f",
            "image2pipe",
            "-c:v",
            "mjpeg",
            "pipe:1",
        ])
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
            let n = match tokio::time::timeout(Duration::from_secs(20), stdout.read(&mut chunk))
                .await
            {
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

/// Models available from the chosen AI provider (key from the form or the saved one).
async fn ai_models(State(m): App, Json(mut cfg): Json<crate::config::AiConfig>) -> Response {
    if cfg.api_key.is_empty() {
        cfg.api_key = m.config().ai.api_key;
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .unwrap_or_default();
    match crate::ai::list_models(&client, &cfg).await {
        Ok(mut list) => {
            list.sort();
            let suggested = (cfg.provider == "gemini")
                .then(|| crate::ai::pick_gemini(&list))
                .flatten();
            Json(json!({ "models": list, "suggested": suggested })).into_response()
        }
        Err(e) => bad(e),
    }
}

/// Live motion readout for one camera (polled while tuning or showing boxes).
async fn camera_motion(State(m): App, Path(id): Path<String>) -> Response {
    let c = camera!(m, id);
    let d = c.detector().map(|d| d.status());
    Json(json!({
        "running": d.as_ref().is_some_and(|d| d.running),
        "moving": d.as_ref().is_some_and(|d| d.motion),
        "objects": d.as_ref().map(|d| d.objects.clone()).unwrap_or_default(),
        "biggest_px": d.as_ref().map(|d| d.biggest_px).unwrap_or(0),
        "peak_px": d.as_ref().map(|d| d.peak_px).unwrap_or(0),
        "min_object_px": c.cfg.detect.min_object_px,
        "labels": d.as_ref().map(|d| d.active_labels.clone()).unwrap_or_default(),
    }))
    .into_response()
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
    while matches!(res, Err(crate::ai::AiError::NotReady(_)))
        && t.elapsed() < Duration::from_secs(120)
    {
        tokio::time::sleep(Duration::from_millis(500)).await;
        res = ai.detect(&frame).await;
    }
    let image = frame.jpeg(85).map(|j| {
        format!(
            "data:image/jpeg;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(j)
        )
    });
    match res {
        Ok(d) => Json(json!({ "ok": true, "detections": d, "image": image.ok(), "ms": t.elapsed().as_millis() as u64 }))
            .into_response(),
        Err(e) => Json(json!({ "ok": false, "error": e.to_string(), "image": image.ok() })).into_response(),
    }
}

/// One decoded RGB frame from the sub-stream, for AI tests when detection is off.
async fn grab_frame(
    m: &Manager,
    c: &crate::manager::CameraRuntime,
) -> anyhow::Result<crate::imgutil::Frame> {
    let ffmpeg = m.config().server.ffmpeg_path;
    let (w, h) = (640u32, 360u32);
    let rgb = crate::relay::grab_rgb(&c.relay_sub, &ffmpeg, w, h).await?;
    Ok(crate::imgutil::Frame {
        width: w,
        height: h,
        rgb: Arc::new(rgb),
    })
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
    /// Words to search for ("red van", "black cat").
    #[serde(default)]
    q: String,
    /// "new" or "viewed" alerts only.
    #[serde(default)]
    seen: String,
    limit: Option<usize>,
}

fn split_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(String::from)
        .collect()
}

fn range(
    from: Option<DateTime<chrono::FixedOffset>>,
    to: Option<DateTime<chrono::FixedOffset>>,
) -> (DateTime<Local>, DateTime<Local>) {
    let to = to
        .map(|t| t.with_timezone(&Local))
        .unwrap_or_else(Local::now);
    let from = from
        .map(|t| t.with_timezone(&Local))
        .unwrap_or(to - chrono::Duration::days(1));
    (from, to)
}

async fn search(State(m): App, Query(q): Query<SearchQuery>) -> Response {
    let (from, to) = range(q.from, q.to);
    let t = |s: &Option<String>| {
        s.as_deref()
            .and_then(|x| NaiveTime::parse_from_str(x, "%H:%M").ok())
    };
    let tod = match (t(&q.tod_from), t(&q.tod_to)) {
        (Some(a), Some(b)) => Some((a, b)),
        (Some(a), None) => Some((a, NaiveTime::from_hms_opt(23, 59, 59).unwrap())),
        (None, Some(b)) => Some((NaiveTime::MIN, b)),
        _ => None,
    };
    // Smart search: the words as a CLIP vector (the model loads on first use).
    let mut smart_note = "off".to_string();
    let smart = if m.config().ai.smart_search && !q.q.trim().is_empty() {
        let words = async {
            anyhow::Ok(crate::smart::Query {
                text: m.clip.embed_text(q.q.trim()).await?,
                neutral: m.clip.neutral().await?,
            })
        };
        match words.await {
            Ok(v) => {
                smart_note = "on".into();
                Some((m.smart.clone(), v))
            }
            Err(e) => {
                smart_note = format!("unavailable: {e:#}");
                None
            }
        }
    } else {
        None
    };
    let lq = LibQuery {
        cameras: split_list(&q.cameras),
        from,
        to,
        tod,
        types: split_list(&q.types).into_iter().collect(),
        text: library::parse_text(&q.q),
        smart,
        seen: match q.seen.as_str() {
            "new" => Some(false),
            "viewed" => Some(true),
            _ => None,
        },
        limit: q.limit.unwrap_or(500).min(5000),
    };
    let items = tokio::task::spawn_blocking(move || library::search(&m, &lq))
        .await
        .unwrap_or_default();
    let mut res = Json(items).into_response();
    if let Ok(v) = HeaderValue::from_str(
        &smart_note
            .replace(['\r', '\n'], " ")
            .chars()
            .filter(|c| c.is_ascii())
            .take(300)
            .collect::<String>(),
    ) {
        res.headers_mut().insert("x-smart-search", v);
    }
    res
}

async fn timeline(State(m): App, Query(q): Query<SearchQuery>) -> Response {
    let (from, to) = range(q.from, q.to);
    let cams = split_list(&q.cameras);
    let t = tokio::task::spawn_blocking(move || library::timeline(&m, &cams, from, to))
        .await
        .unwrap_or_default();
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
    let name = format!(
        "{}_{}_to_{}.mp4",
        c.cfg.id,
        from.format("%Y-%m-%d_%H-%M-%S"),
        to.format("%H-%M-%S")
    );
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
            if let Some(name) = name.filter(|_| download)
                && let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\""))
            {
                res.headers_mut().insert(header::CONTENT_DISPOSITION, v);
            }
            res
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn serve_recording(
    State(m): App,
    Path((id, name)): Path<(String, String)>,
    req: Request,
) -> Response {
    let c = camera!(m, id);
    match storage::resolve(&c.dir, &name) {
        Ok(p) => serve_file(p, req, Some(name), false).await,
        Err(e) => err(StatusCode::NOT_FOUND, e),
    }
}

async fn serve_event_file(
    State(m): App,
    Path((id, name)): Path<(String, String)>,
    req: Request,
) -> Response {
    let c = camera!(m, id);
    match storage::resolve(c.events.dir(), &name) {
        Ok(p) => serve_file(p, req, Some(name), false).await,
        Err(e) => err(StatusCode::NOT_FOUND, e),
    }
}

#[derive(Deserialize)]
struct PlayQuery {
    #[serde(default)]
    t: f64,
    /// Data saver: small, low-bitrate stream for slow or metered connections.
    #[serde(default)]
    lite: bool,
}

/// Play a recording converted to H.264 on the fly, for browsers that cannot
/// play the camera's own format (usually H.265). Starts at `t` seconds; the
/// player asks again with a new `t` when seeking.
async fn play_converted(
    State(m): App,
    Path((id, kind, name)): Path<(String, String, String)>,
    Query(q): Query<PlayQuery>,
) -> Response {
    let c = camera!(m, id);
    let src = match media_path(&c, &kind, &name) {
        Ok(p) => p,
        Err(e) => return err(StatusCode::NOT_FOUND, e),
    };
    let cfg = m.config();
    let hw = &cfg.hardware;
    let gpu = hw.uses_gpu();
    let encoder = if gpu {
        hw.resolved_encoder()
    } else {
        "libx264"
    };
    let (pre, mut out) = ffmpeg::encoder_args(encoder, hw, false);
    if encoder == "libx264" {
        // Speed matters more than size here.
        if let Some(i) = out.iter().position(|a| a == "veryfast") {
            out[i] = "superfast".into();
        }
    }
    let mut cmd = tokio::process::Command::new(&cfg.server.ffmpeg_path);
    cmd.args(["-hide_banner", "-loglevel", "error"]);
    if gpu {
        cmd.args(ffmpeg::decode_args(hw, false));
    }
    let child = cmd
        .args(pre)
        .args(["-ss", &format!("{:.3}", q.t.max(0.0)), "-i"])
        .arg(&src)
        .args([
            "-map",
            "0:v:0",
            "-map",
            "0:a:0?",
            "-vf",
            if q.lite {
                "fps='min(15,source_fps)',scale='min(854,iw)':-2"
            } else {
                "scale='min(1920,iw)':-2"
            },
        ])
        .args(out)
        // Data saver: about 0.7 Mbit/s instead of several (later options win).
        .args(if q.lite {
            vec![
                "-b:v", "600k", "-maxrate", "700k", "-bufsize", "1400k", "-c:a", "aac", "-b:a",
                "48k", "-ac", "1",
            ]
        } else {
            vec!["-c:a", "aac", "-b:a", "96k"]
        })
        .args([
            "-f",
            "mp4",
            "-movflags",
            "frag_keyframe+empty_moov+default_base_moof",
            "pipe:1",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let stdout = child.stdout.take().unwrap();
    // The ffmpeg process lives as long as the response body (browser disconnect ends it).
    let stream = futures_util::stream::unfold(
        (tokio_util::io::ReaderStream::new(stdout), child),
        |(mut r, c)| async move {
            use futures_util::StreamExt;
            r.next().await.map(|x| (x, (r, c)))
        },
    );
    Response::builder()
        .header(header::CONTENT_TYPE, "video/mp4")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from_stream(stream))
        .unwrap()
}

/// Resolve "rec"/"clip" + name to a file of the camera.
fn media_path(
    c: &crate::manager::CameraRuntime,
    kind: &str,
    name: &str,
) -> anyhow::Result<PathBuf> {
    match kind {
        "clip" => storage::resolve(c.events.dir(), name),
        _ => storage::resolve(&c.dir, name),
    }
}

/// Only a couple of preview jobs at a time.
static THUMB_JOBS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(3);

fn fresh(cache: &std::path::Path, src: &std::path::Path) -> bool {
    let (Ok(c), Ok(s)) = (std::fs::metadata(cache), std::fs::metadata(src)) else {
        return false;
    };
    match (c.modified(), s.modified()) {
        // A file still being written changes; refresh its preview every minute.
        (Ok(ct), Ok(st)) => ct + Duration::from_secs(60) >= st,
        _ => false,
    }
}

async fn recording_thumb(
    State(m): App,
    Path((id, name)): Path<(String, String)>,
    req: Request,
) -> Response {
    let c = camera!(m, id);
    let Ok(src) = storage::resolve(&c.dir, &name) else {
        return err(StatusCode::NOT_FOUND, "not found");
    };
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

async fn ensure_sprite(
    m: &Manager,
    c: &crate::manager::CameraRuntime,
    kind: &str,
    name: &str,
) -> anyhow::Result<(PathBuf, ffmpeg::SpriteInfo)> {
    let src = media_path(c, kind, name)?;
    let dir = src.parent().unwrap().join(".thumbs");
    let prefix = dir.join(format!("{name}.sprite"));
    let first = dir.join(format!("{name}.sprite-000.jpg"));
    let meta = dir.join(format!("{name}.sprite.json"));
    let load = || -> Option<ffmpeg::SpriteInfo> {
        serde_json::from_slice(&std::fs::read(&meta).ok()?).ok()
    };
    if fresh(&first, &src)
        && let Some(i) = load()
    {
        return Ok((prefix, i));
    }
    let _p = THUMB_JOBS.acquire().await;
    if fresh(&first, &src)
        && let Some(i) = load()
    {
        return Ok((prefix, i));
    }
    std::fs::create_dir_all(&dir)?;
    // Remove pages of an older (shorter or longer) version.
    if let Ok(rd) = std::fs::read_dir(&dir) {
        let start = format!("{name}.sprite");
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with(&start) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let info = ffmpeg::sprite(&m.config().server.ffmpeg_path, &src, &prefix).await?;
    std::fs::write(&meta, serde_json::to_vec(&info)?)?;
    Ok((prefix, info))
}

/// Background job: keep the time-lapse previews of the last 24 hours ready
/// for every camera (newest first), one file at a time and only while
/// nobody is waiting for previews. Older days are still made on demand.
pub async fn prebuild_timelapse(m: Arc<Manager>) {
    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    let mut announced = false;
    loop {
        let mut built = 0usize;
        if m.config().storage.prebuild_timelapse {
            let since = Local::now() - chrono::Duration::hours(24);
            let mut todo: Vec<(
                Arc<crate::manager::CameraRuntime>,
                &'static str,
                String,
                DateTime<Local>,
            )> = Vec::new();
            for c in m.cameras() {
                let recs = library::recordings(&c);
                for r in recs.iter().filter(|r| r.end >= since) {
                    // Skip the file still being written (it keeps growing).
                    if (Local::now() - r.end).num_seconds() < 30 {
                        continue;
                    }
                    todo.push((c.clone(), "rec", r.name.clone(), r.start));
                }
                for e in c.events.between(since, Local::now()) {
                    if let Some(clip) = e.clip {
                        todo.push((c.clone(), "clip", clip, e.start));
                    }
                }
            }
            todo.sort_by(|a, b| b.3.cmp(&a.3));
            for (c, kind, name, _) in todo {
                if !m.config().storage.prebuild_timelapse {
                    break;
                }
                let Ok(src) = media_path(&c, kind, &name) else {
                    continue;
                };
                let first = src
                    .parent()
                    .unwrap()
                    .join(".thumbs")
                    .join(format!("{name}.sprite-000.jpg"));
                if fresh(&first, &src) {
                    continue;
                }
                // Only when the computer has room to spare.
                crate::load::wait_for_cpu(75, std::time::Duration::from_secs(600)).await;
                // Let people scrubbing the timeline go first.
                while THUMB_JOBS.available_permits() < 3 {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
                if !announced {
                    tracing::info!(
                        "preparing time-lapse previews for the last 24 hours in the background"
                    );
                    announced = true;
                }
                if ensure_sprite(&m, &c, kind, &name).await.is_ok() {
                    built += 1;
                }
            }
        }
        if built > 0 {
            tracing::info!("time-lapse previews ready ({built} file(s) prepared)");
        }
        tokio::time::sleep(std::time::Duration::from_secs(300)).await;
    }
}

async fn sprite_info(
    State(m): App,
    Path((id, kind, name)): Path<(String, String, String)>,
) -> Response {
    let c = camera!(m, id);
    match ensure_sprite(&m, &c, &kind, &name).await {
        Ok((_, info)) => Json(json!({
            "info": info,
            "url": format!("media/{id}/sprite/{kind}/{name}?v={}&page=", info.version),
        }))
        .into_response(),
        Err(e) => err(StatusCode::NOT_FOUND, format!("{e:#}")),
    }
}

#[derive(Deserialize)]
struct PageQuery {
    #[serde(default)]
    page: u32,
}

async fn sprite_image(
    State(m): App,
    Path((id, kind, name)): Path<(String, String, String)>,
    Query(q): Query<PageQuery>,
    req: Request,
) -> Response {
    let c = camera!(m, id);
    match ensure_sprite(&m, &c, &kind, &name).await {
        Ok((prefix, info)) if q.page < info.pages => {
            let file = PathBuf::from(format!("{}-{:03}.jpg", prefix.display(), q.page));
            let mut res = serve_file(file, req, None, false).await;
            // Pages are versioned in their URL, so they can be cached for long.
            res.headers_mut().insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("max-age=86400"),
            );
            res
        }
        Ok(_) => err(StatusCode::NOT_FOUND, "no such page"),
        Err(e) => err(StatusCode::NOT_FOUND, format!("{e:#}")),
    }
}

// ---------- live ----------

fn multipart_part(frame: &[u8]) -> Bytes {
    let head = format!(
        "--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
        frame.len()
    );
    let mut part = Vec::with_capacity(head.len() + frame.len() + 2);
    part.extend_from_slice(head.as_bytes());
    part.extend_from_slice(frame);
    part.extend_from_slice(b"\r\n");
    part.into()
}

fn mjpeg_response(
    stream: impl futures_util::Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static,
) -> Response {
    Response::builder()
        .header(
            header::CONTENT_TYPE,
            "multipart/x-mixed-replace; boundary=frame",
        )
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

async fn live_mjpeg(
    State(m): App,
    Path(id): Path<String>,
    Query(q): Query<QualityQuery>,
) -> Response {
    let c = camera!(m, id);
    let mut rx = c.live(q.q).subscribe();
    drop(c);
    let (tx, mut out) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(2);
    tokio::spawn(async move {
        while rx.changed().await.is_ok() {
            let Some((_, frame)) = rx.borrow_and_update().clone() else {
                continue;
            };
            if tx.send(Ok(multipart_part(&frame))).await.is_err() {
                break;
            }
        }
    });
    mjpeg_response(futures_util::stream::poll_fn(move |cx| out.poll_recv(cx)))
}

async fn live_frame(
    State(m): App,
    Path(id): Path<String>,
    Query(q): Query<QualityQuery>,
) -> Response {
    let c = camera!(m, id);
    match c.live(q.q).frame().await {
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
    match c.live_main.snapshot().await {
        Ok(jpg) => {
            let name = format!(
                "attachment; filename=\"{}_{}.jpg\"",
                c.cfg.id,
                Local::now().format("%Y-%m-%d_%H-%M-%S")
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
    /// Data saver: smaller, more compressed pictures.
    #[serde(default)]
    lite: bool,
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
        let Some(c) = m.camera(&sub.camera) else {
            return;
        };
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
                if frame
                    .as_ref()
                    .is_none_or(|f| f.0.elapsed() > Duration::from_secs(5))
                {
                    continue;
                }
            }
            let Some((_, jpg)) = frame else { continue };
            if last.elapsed() < min_gap {
                continue;
            }
            last = Instant::now();
            let jpg = if sub.lite {
                // Data saver: grid pictures 480 px, full view 960 px, more compressed.
                let (w, q) = if qb == 1 { (960, 50) } else { (480, 45) };
                match tokio::task::spawn_blocking(move || crate::imgutil::shrink_jpeg(&jpg, w, q))
                    .await
                {
                    Ok(Ok(small)) => Bytes::from(small),
                    _ => continue,
                }
            } else {
                jpg
            };
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

#[cfg(test)]
mod remote_tests {
    use super::is_remote;

    #[test]
    fn home_or_away() {
        let r = |s: &str| is_remote(s.parse().unwrap());
        assert!(!r("192.168.1.20"));
        assert!(!r("10.0.0.5"));
        assert!(!r("127.0.0.1"));
        assert!(!r("::1"));
        assert!(!r("fe80::1"));
        assert!(!r("::ffff:192.168.1.9"));
        assert!(r("100.101.102.103")); // Tailscale
        assert!(r("fd7a:115c:a1e0::1")); // Tailscale IPv6
        assert!(r("203.0.113.7"));
        assert!(r("2001:db8::1"));
    }
}
