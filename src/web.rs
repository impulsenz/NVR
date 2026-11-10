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
        .route(
            "/logo.png",
            get(|| async {
                (
                    [
                        (header::CONTENT_TYPE, "image/png"),
                        (header::CACHE_CONTROL, "max-age=86400"),
                    ],
                    include_bytes!("../static/logo.png").as_slice(),
                )
            }),
        )
        .route(
            "/icon.png",
            get(|| async {
                (
                    [
                        (header::CONTENT_TYPE, "image/png"),
                        (header::CACHE_CONTROL, "max-age=86400"),
                    ],
                    include_bytes!("../static/icon.png").as_slice(),
                )
            }),
        )
        .route(
            "/favicon.png",
            get(|| async {
                (
                    [
                        (header::CONTENT_TYPE, "image/png"),
                        (header::CACHE_CONTROL, "max-age=86400"),
                    ],
                    include_bytes!("../static/favicon.png").as_slice(),
                )
            }),
        )
        .route("/api/auth/state", get(auth_state))
        .route("/api/auth/setup", post(auth_setup))
        .route("/api/auth/login", post(auth_login))
        .route("/api/auth/logout", post(auth_logout))
        .route("/api/auth/password", post(auth_password))
        .route("/api/users", get(users_list).post(users_create))
        .route("/api/users/{name}", put(users_update).delete(users_delete))
        .route(
            "/api/changelog",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/markdown; charset=utf-8")],
                    include_str!("../CHANGELOG.md"),
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
        .route("/api/discover", post(discover_now))
        .route("/api/known", get(known_list).post(known_create))
        .route("/api/known/{id}", put(known_update).delete(known_delete))
        .route(
            "/api/known/{id}/photo",
            post(known_photo).layer(axum::extract::DefaultBodyLimit::max(25 * 1024 * 1024)),
        )
        .route("/api/known/{id}/photo/{name}", get(known_photo_get))
        .route("/api/known/{id}/learn", post(known_learn))
        .route("/api/cloud/status", get(cloud_status))
        .route("/api/cloud/remotes", post(cloud_remotes))
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
        .route("/api/cameras/{id}/talk", get(talk_ws))
        .route("/api/jobs", get(jobs_list))
        .route("/api/notify/test", post(notify_test))
        .route("/api/notify/status", get(notify_status))
        .route("/api/maintenance/previews", post(rebuild_previews))
        .route(
            "/api/maintenance/alert-pictures",
            post(refresh_alert_pictures),
        )
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
        .route("/media/{id}/tl/{name}", get(serve_timeline_file))
        .route("/media/{id}/thumb/{name}", get(recording_thumb))
        .route("/media/{id}/play/{kind}/{name}", get(play_converted))
        .route("/media/{id}/sprite/{kind}/{name}", get(sprite_image))
        .route("/live/{id}/stream.mjpeg", get(live_mjpeg))
        .route("/live/{id}/frame.jpg", get(live_frame))
        .route("/snapshot/{id}", get(snapshot))
        .layer(DefaultBodyLimit::max(10 * 1024 * 1024))
        .layer(middleware::from_fn_with_state(mgr.clone(), auth))
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

#[derive(Deserialize, Default)]
struct DeviceQuery {
    /// "mobile" or "desktop": each has its own layout.
    #[serde(default)]
    device: String,
}

fn device_kind(d: &str) -> &'static str {
    if d == "mobile" { "mobile" } else { "desktop" }
}

async fn status(
    State(m): App,
    user: Option<axum::Extension<CurrentUser>>,
    Query(dq): Query<DeviceQuery>,
) -> Response {
    let cfg = m.config();
    let vis = visible(&user);
    let cameras: Vec<_> = m
        .cameras()
        .iter()
        .filter(|c| vis.as_ref().is_none_or(|v| v.contains(&c.cfg.id)))
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
        // The user's own layout for this kind of device (else the shared one).
        "dashboard": user.as_ref().and_then(|u| m.users.layout(&u.0.0.username, device_kind(&dq.device))).unwrap_or(cfg.dashboard.clone()),
        "me": user.as_ref().map(|u| crate::users::Me { username: u.0.0.username.clone(), role: u.0.0.role, cameras: u.0.0.cameras.clone() }),
        "cameras": cameras,
        "ai": m.ai.get().status(),
        "smart_search": m.clip.status(),
        "alerts_unseen": library::unseen_count(&m),
        "smart_search_on": cfg.ai.smart_search,
        "new_cameras": if user.as_ref().is_some_and(|u| u.0.0.role == crate::users::Role::Admin) { m.new_cameras.lock().unwrap().len() } else { 0 },
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
    let (has_token, has_gemini) = (
        !cfg.notify.token.is_empty(),
        !cfg.notify.gemini_key.is_empty(),
    );
    cfg.server.password.clear();
    cfg.ai.api_key.clear();
    cfg.notify.token.clear();
    cfg.notify.gemini_key.clear();
    let mut v = serde_json::to_value(&cfg).unwrap();
    v["server"]["has_password"] = json!(has_password);
    v["ai"]["has_key"] = json!(has_key);
    v["notify"]["has_token"] = json!(has_token);
    v["notify"]["has_gemini_key"] = json!(has_gemini);
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

async fn discover_now(State(m): App) -> Response {
    let found = crate::discover::scan(&m).await;
    *m.new_cameras.lock().unwrap() = found.iter().filter(|f| !f.added).cloned().collect();
    Json(found).into_response()
}

async fn known_list(State(m): App) -> Response {
    Json(m.known.list()).into_response()
}

#[derive(Deserialize)]
struct KnownReq {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    name: String,
    plates: Option<Vec<String>>,
}

async fn known_create(State(m): App, Json(r): Json<KnownReq>) -> Response {
    match m
        .known
        .create(&r.kind, &r.name, r.plates.unwrap_or_default())
    {
        Ok(id) => Json(json!({ "ok": true, "id": id })).into_response(),
        Err(e) => bad(e),
    }
}

async fn known_update(State(m): App, Path(id): Path<String>, Json(r): Json<KnownReq>) -> Response {
    match m.known.update(&id, Some(&r.name), r.plates) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => bad(e),
    }
}

async fn known_delete(State(m): App, Path(id): Path<String>) -> Response {
    match m.known.delete(&id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => bad(e),
    }
}

/// Teach with a photo (JPEG in the request body): a person's face, or a
/// vehicle's plate and look.
async fn known_photo(State(m): App, Path(id): Path<String>, body: Bytes) -> Response {
    let res = async {
        let k = m.known.get(&id).ok_or_else(|| anyhow::anyhow!("not found"))?;
        let img = crate::imgutil::decode_jpeg(&body).map_err(|_| anyhow::anyhow!("send the photo as a JPEG picture"))?;
        if k.kind == "person" {
            let mut faces: Vec<_> = m.recog.faces(&img).await?.into_iter().filter(|f| f.score >= 0.7).collect();
            faces.sort_by(|a, b| (b.w * b.h).total_cmp(&(a.w * a.h)));
            let face = faces.first().ok_or_else(|| anyhow::anyhow!("no face found in this photo — use a clear photo of the face"))?;
            // Several people of similar size: we can't tell which one is meant.
            if let Some(second) = faces.get(1)
                && second.w * second.h > face.w * face.h * 0.5
            {
                anyhow::bail!("this photo has {} faces — use a photo of {} alone (or crop it)", faces.len(), k.name);
            }
            let print = m.recog.faceprint(&img, face).await?;
            m.known.add(&id, Some(&print), None, None, Some(&body))?;
            anyhow::Ok(json!({ "ok": true, "message": format!("Learned {}'s face", k.name) }))
        } else {
            let plates = m.recog.plates(&img).await.unwrap_or_default();
            let look = if m.config().ai.smart_search { m.clip.embed_image(&img).await.ok() } else { None };
            for p in &plates {
                m.known.add(&id, None, None, Some(&p.text), None)?;
            }
            m.known.add(&id, None, look.as_deref(), None, Some(&body))?;
            let what = match (plates.first(), look.is_some()) {
                (Some(p), _) => format!("Read the plate {} and saved the photo", p.text),
                (None, true) => "Saved how it looks (no plate could be read — type it in)".to_string(),
                (None, false) => "Photo saved (no plate could be read — type it in; turn on smart search to also match how it looks)".to_string(),
            };
            anyhow::Ok(json!({ "ok": true, "message": what }))
        }
    }
    .await;
    match res {
        Ok(v) => Json(v).into_response(),
        Err(e) => bad(e),
    }
}

async fn known_photo_get(
    State(m): App,
    Path((id, name)): Path<(String, String)>,
    req: Request,
) -> Response {
    match m.known.photo_path(&id, &name) {
        Some(p) => serve_file(p, req, None, false).await,
        None => err(StatusCode::NOT_FOUND, "not found"),
    }
}

#[derive(Deserialize)]
struct LearnReq {
    camera: String,
    event: String,
    face: usize,
}

/// "This is …": name a face seen in an alert (and learn it).
async fn known_learn(State(m): App, Path(id): Path<String>, Json(r): Json<LearnReq>) -> Response {
    let res = (|| {
        let k = m
            .known
            .get(&id)
            .ok_or_else(|| anyhow::anyhow!("not found"))?;
        let c = m
            .camera(&r.camera)
            .ok_or_else(|| anyhow::anyhow!("camera not found"))?;
        let mut e = c
            .events
            .get(&r.event)
            .ok_or_else(|| anyhow::anyhow!("alert not found"))?;
        let f = e
            .faces
            .get_mut(r.face)
            .ok_or_else(|| anyhow::anyhow!("face not found"))?;
        let print = crate::known::unpack(&f.print);
        let photo = std::fs::read(c.events.dir().join(&f.file)).ok();
        m.known
            .add(&id, Some(&print), None, None, photo.as_deref())?;
        f.name = Some(k.name.clone());
        e.notes.retain(|n| n != "Unknown face");
        let note = format!("Recognized {}", k.name);
        if !e.notes.contains(&note) {
            e.notes.push(note);
        }
        e.summary.clear(); // described again, with the name
        c.events.save(e);
        anyhow::Ok(k.name)
    })();
    match res {
        Ok(name) => Json(json!({ "ok": true, "message": format!("Learned — this is {name}") }))
            .into_response(),
        Err(e) => bad(e),
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

async fn put_dashboard(
    State(m): App,
    user: Option<axum::Extension<CurrentUser>>,
    Query(dq): Query<DeviceQuery>,
    Json(d): Json<DashboardConfig>,
) -> Response {
    // Saved as this user's own layout for this kind of device.
    let res = match &user {
        Some(u) => m
            .users
            .set_layout(&u.0.0.username, device_kind(&dq.device), d),
        None => m.update_dashboard(d).await,
    };
    match res {
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
    let child = crate::load::cmd(&ffmpeg)
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

async fn cloud_status() -> Response {
    Json(crate::cloud::STATUS.lock().unwrap().clone()).into_response()
}

async fn cloud_remotes(Json(c): Json<crate::config::CloudConfig>) -> Response {
    match crate::cloud::remotes(&c).await {
        Ok(r) => Json(json!({ "ok": true, "remotes": r })).into_response(),
        Err(e) => Json(json!({ "ok": false, "message": format!("{e:#}") })).into_response(),
    }
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

/// The cameras asked for, limited to those the user may see.
fn allowed_cameras(asked: Vec<String>, vis: Option<Vec<String>>) -> Vec<String> {
    match vis {
        None => asked,
        Some(v) if asked.is_empty() => v,
        Some(v) => {
            let a: Vec<String> = asked.into_iter().filter(|c| v.contains(c)).collect();
            if a.is_empty() {
                vec!["\u{0}none".into()]
            } else {
                a
            }
        }
    }
}

async fn search(
    State(m): App,
    user: Option<axum::Extension<CurrentUser>>,
    Query(q): Query<SearchQuery>,
) -> Response {
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
        m.clip.set_model(&m.config().ai.smart_model);
        m.smart.set_file(&m.clip.index_file());
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
        cameras: allowed_cameras(split_list(&q.cameras), visible(&user)),
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

async fn timeline(
    State(m): App,
    user: Option<axum::Extension<CurrentUser>>,
    Query(q): Query<SearchQuery>,
) -> Response {
    let (from, to) = range(q.from, q.to);
    let cams = allowed_cameras(split_list(&q.cameras), visible(&user));
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

async fn serve_timeline_file(
    State(m): App,
    Path((id, name)): Path<(String, String)>,
    req: Request,
) -> Response {
    let c = camera!(m, id);
    match storage::resolve(&storage::timeline_dir(&c.dir), &name) {
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
    let gpu = hw.uses_gpu() && !crate::load::gpu_too_busy("video conversion");
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
    let mut cmd = crate::load::cmd(&cfg.server.ffmpeg_path);
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
        "tl" => storage::resolve(&storage::timeline_dir(&c.dir), name),
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
    // The light timeline copies get one picture per second.
    let info = ffmpeg::sprite(&m.config().server.ffmpeg_path, &src, &prefix, kind == "tl").await?;
    std::fs::write(&meta, serde_json::to_vec(&info)?)?;
    Ok((prefix, info))
}

/// Background job: keep the time-lapse previews of the last 24 hours ready
/// for every camera (newest first), one file at a time and only while
/// nobody is waiting for previews. Older days are still made on demand.
/// "Rebuild previews" (System → Tasks): run the job now, even if
/// preparing previews in advance is off.
static PREBUILD_NOW: tokio::sync::Notify = tokio::sync::Notify::const_new();
static PREBUILD_FORCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub async fn prebuild_timelapse(m: Arc<Manager>) {
    tokio::select! {
        _ = tokio::time::sleep(std::time::Duration::from_secs(60)) => {}
        _ = PREBUILD_NOW.notified() => {}
    }
    let mut announced = false;
    loop {
        let mut built = 0usize;
        let force = PREBUILD_FORCE.swap(false, std::sync::atomic::Ordering::SeqCst);
        let mut job: Option<crate::jobs::Job> = None;
        if force || m.config().storage.prebuild_timelapse {
            let since = Local::now() - chrono::Duration::hours(24);
            let mut todo: Vec<(
                Arc<crate::manager::CameraRuntime>,
                &'static str,
                String,
                DateTime<Local>,
            )> = Vec::new();
            for c in m.cameras() {
                // Cameras with a light timeline copy: previews from that
                // (cheap, also for the file still being recorded, refreshed
                // each round). Otherwise from the recordings themselves.
                let copies = library::timeline_recordings(&c);
                if !copies.is_empty() {
                    for r in copies.iter().filter(|r| r.end >= since) {
                        todo.push((c.clone(), "tl", r.name.clone(), r.start));
                    }
                } else {
                    for r in library::recordings(&c).iter().filter(|r| r.end >= since) {
                        // Skip the file still being written (it keeps growing).
                        if (Local::now() - r.end).num_seconds() < 30 {
                            continue;
                        }
                        todo.push((c.clone(), "rec", r.name.clone(), r.start));
                    }
                }
                for e in c.events.between(since, Local::now()) {
                    if let Some(clip) = e.clip {
                        todo.push((c.clone(), "clip", clip, e.start));
                    }
                }
            }
            todo.sort_by(|a, b| b.3.cmp(&a.3));
            for (c, kind, name, start) in todo {
                if !force && !m.config().storage.prebuild_timelapse {
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
                let j = job.get_or_insert_with(|| {
                    crate::jobs::start("previews", "Time-lapse previews (last 24 hours)")
                });
                j.waiting();
                // Only when the computer has room to spare.
                crate::load::wait_for_cpu(75, std::time::Duration::from_secs(600)).await;
                j.step(format!("{} — {}", c.cfg.name, start.format("%H:%M")));
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
                    j.done_one();
                }
            }
        }
        drop(job);
        if built > 0 {
            tracing::info!("time-lapse previews ready ({built} file(s) prepared)");
        }
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(300)) => {}
            _ = PREBUILD_NOW.notified() => {}
        }
    }
}

// ---------- notifications ----------

/// Send a test notification (with the newest alert picture, if any), using
/// the settings in the request (so they can be tried before saving).
async fn notify_test(State(m): App, Json(mut n): Json<crate::config::NotifyConfig>) -> Response {
    let saved = m.config().notify;
    if n.token.is_empty() {
        n.token = saved.token;
    }
    let newest = m
        .cameras()
        .into_iter()
        .filter_map(|c| {
            c.events
                .all()
                .into_iter()
                .filter(|e| e.thumb.is_some())
                .max_by_key(|e| e.start)
                .map(|e| (c, e))
        })
        .max_by_key(|(_, e)| e.start);
    let picture = newest
        .as_ref()
        .and_then(|(c, e)| std::fs::read(c.events.dir().join(e.thumb.as_ref()?)).ok());
    let msg = crate::notify::Message {
        title: format!("{}: test notification", m.config().server.name),
        body: if picture.is_some() {
            "Notifications work. This is the newest alert picture.".into()
        } else {
            "Notifications work.".into()
        },
        tags: "white_check_mark",
        priority: 3,
        click: (!n.link.trim().is_empty()).then(|| n.link.trim().to_string()),
        picture,
    };
    match crate::notify::send(&n, msg).await {
        Ok(()) => Json(json!({"ok": true, "message": "Sent — check your phone"})).into_response(),
        Err(e) => bad(e),
    }
}

async fn notify_status() -> Response {
    Json(crate::notify::STATUS.lock().unwrap().clone()).into_response()
}

// ---------- System → Tasks ----------

async fn jobs_list() -> Response {
    Json(json!({ "jobs": crate::jobs::list() })).into_response()
}

/// Throw away all scrub / time-lapse previews and make those of the last
/// 24 hours again (older ones are made when looked at).
async fn rebuild_previews(State(m): App) -> Response {
    let mut removed = 0;
    for c in m.cameras() {
        for dir in [
            c.dir.clone(),
            crate::storage::timeline_dir(&c.dir),
            c.events.dir().to_path_buf(),
        ] {
            let t = dir.join(".thumbs");
            if t.is_dir() && std::fs::remove_dir_all(&t).is_ok() {
                removed += 1;
            }
        }
    }
    tracing::info!("previews cleared ({removed} folder(s)); making them again");
    PREBUILD_FORCE.store(true, std::sync::atomic::Ordering::SeqCst);
    PREBUILD_NOW.notify_one();
    Json(json!({"ok": true, "message": "Previews cleared — the last 24 hours are being made again (see Tasks)"})).into_response()
}

#[derive(Deserialize)]
struct RefreshPictures {
    #[serde(default = "seven")]
    days: i64,
}
fn seven() -> i64 {
    7
}

/// Alert pictures again, from the full-quality recordings.
async fn refresh_alert_pictures(State(m): App, Json(r): Json<RefreshPictures>) -> Response {
    tokio::spawn(crate::identify::refresh_pictures(
        m.clone(),
        r.days.clamp(1, 90),
    ));
    Json(json!({"ok": true, "message": "Making sharper alert pictures from the recordings (see Tasks)"})).into_response()
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
async fn live_ws(
    State(m): App,
    user: Option<axum::Extension<CurrentUser>>,
    ws: WebSocketUpgrade,
) -> Response {
    let vis = visible(&user);
    ws.on_upgrade(move |socket| live_socket(socket, m, vis))
}

/// Two-way audio: the browser sends 16-bit mono PCM at 8 kHz (binary
/// messages); GAZE plays it on the camera's speaker. The first text message
/// from the server says whether it worked: {"ok":true,"codec":".."} or
/// {"error":".."}.
async fn talk_ws(
    State(m): App,
    Path(id): Path<String>,
    user: Option<axum::Extension<CurrentUser>>,
    ws: WebSocketUpgrade,
) -> Response {
    let vis = visible(&user);
    let cam = m.config().cameras.iter().find(|c| c.id == id).cloned();
    let Some(cam) = cam.filter(|_| vis.as_ref().is_none_or(|v| v.contains(&id))) else {
        return err(StatusCode::NOT_FOUND, "no such camera");
    };
    let who = user.map(|u| u.0.0.username.clone()).unwrap_or_default();
    ws.on_upgrade(move |mut socket| async move {
        let mut talk = match crate::talk::Talk::open(&cam.id, &cam.url).await {
            Ok(t) => t,
            Err(e) => {
                let _ = socket.send(Message::Text(json!({"error": format!("{e:#}")}).to_string().into())).await;
                return;
            }
        };
        tracing::info!("[{}] {} is talking through the camera's speaker ({})", cam.name, if who.is_empty() { "someone" } else { &who }, talk.codec());
        let _ = socket.send(Message::Text(json!({"ok": true, "codec": talk.codec()}).to_string().into())).await;
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let res: anyhow::Result<()> = async {
            loop {
                tokio::select! {
                    msg = socket.recv() => {
                        let Some(Ok(msg)) = msg else { break };
                        match msg {
                            Message::Binary(b) => {
                                let s: Vec<i16> = b.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
                                talk.send(&s).await?;
                            }
                            Message::Close(_) => break,
                            _ => {}
                        }
                    }
                    _ = tick.tick() => talk.tick().await?,
                }
            }
            Ok(())
        }
        .await;
        if let Err(e) = res {
            let _ = socket.send(Message::Text(json!({"error": format!("{e:#}")}).to_string().into())).await;
        }
        talk.stop().await;
        tracing::info!("[{}] talking stopped", cam.name);
    })
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

async fn live_socket(mut socket: WebSocket, m: Arc<Manager>, vis: Option<Vec<String>>) {
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
                    .filter(|s| vis.as_ref().is_none_or(|v| v.contains(&s.camera)))
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

// ---------- users & sign-in ----------

const COOKIE: &str = "gaze_session";

fn session_token(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v.to_string())
}

fn session_cookie(token: &str, remember: bool) -> HeaderValue {
    let age = if remember { "; Max-Age=2592000" } else { "" };
    HeaderValue::from_str(&format!(
        "{COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax{age}"
    ))
    .unwrap()
}

/// The signed-in user, put on each request by the sign-in check.
#[derive(Clone)]
pub struct CurrentUser(pub Arc<crate::users::User>);

/// Camera id in a path like /api/cameras/{id}/…, /media/{id}/…, /live/{id}/….
fn camera_in_path(path: &str) -> Option<&str> {
    let mut parts = path.trim_start_matches('/').split('/');
    match (parts.next()?, parts.next()?) {
        ("api", "cameras") | ("api", "sprite") => parts.next(),
        ("media", id) | ("live", id) | ("snapshot", id) => Some(id),
        _ => None,
    }
}

/// What a role may change (everything else that isn't a read needs admin).
fn allowed(role: crate::users::Role, method: &axum::http::Method, path: &str) -> bool {
    use crate::users::Role::*;
    if role == Admin {
        return true;
    }
    let read = method == axum::http::Method::GET || method == axum::http::Method::HEAD;
    // Settings, camera connection details and accounts: admin only.
    let admin_only = [
        "/api/settings",
        "/api/config/",
        "/api/users",
        "/api/fs",
        "/api/probe",
        "/api/preview",
        "/api/hw-test",
        "/api/ai/",
        "/api/cloud/",
        "/api/ptz/test",
        "/api/profiles",
        "/api/maintenance",
        "/api/notify",
    ];
    if admin_only.iter().any(|p| path.starts_with(p))
        || path == "/api/cameras"
        || path.ends_with("/device")
        || path.contains("/device/")
    {
        return false;
    }
    // Talking through a camera's speaker: not for viewers.
    if path.ends_with("/talk") {
        return role != Viewer;
    }
    if read {
        return true;
    }
    // Operators may teach who someone is from an alert.
    if role == Operator && path.starts_with("/api/known/") && path.ends_with("/learn") {
        return true;
    }
    // Changes anyone signed in may make.
    if path.starts_with("/api/alerts/")
        || path == "/api/dashboard"
        || path.starts_with("/api/auth/")
    {
        return true;
    }
    // Operators: move cameras, pause recording, remove alerts/recordings.
    role == Operator
        && (path.contains("/ptz/")
            || path.ends_with("/pause")
            || path.ends_with("/resume")
            || (method == axum::http::Method::DELETE
                && (path.contains("/events/") || path.contains("/recordings/"))))
}

async fn auth(State(m): App, mut req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    // The page itself, its pictures and the sign-in API are open: the page
    // shows the sign-in (or first-time setup) screen.
    let open = matches!(
        path.as_str(),
        "/" | "/app.js"
            | "/app.css"
            | "/logo.png"
            | "/icon.png"
            | "/favicon.png"
            | "/api/auth/state"
            | "/api/auth/login"
            | "/api/auth/setup"
            | "/api/changelog"
    );
    if open {
        return next.run(req).await;
    }
    let user = session_token(req.headers()).and_then(|t| m.users.session_user(&t));
    let Some(user) = user else {
        return err(StatusCode::UNAUTHORIZED, "please sign in");
    };
    if !allowed(user.role, req.method(), &path) {
        return err(
            StatusCode::FORBIDDEN,
            "your account isn't allowed to do this — ask the admin",
        );
    }
    if let Some(cam) = camera_in_path(&path)
        && !user.can_see(cam)
    {
        return err(StatusCode::FORBIDDEN, "your account can't see this camera");
    }
    req.extensions_mut().insert(CurrentUser(Arc::new(user)));
    next.run(req).await
}

/// Cameras the signed-in user may see (None = all).
fn visible(user: &Option<axum::Extension<CurrentUser>>) -> Option<Vec<String>> {
    user.as_ref()
        .map(|u| u.0.0.cameras.clone())
        .filter(|c| !c.is_empty())
}

async fn auth_state(State(m): App, headers: axum::http::HeaderMap) -> Response {
    let me = session_token(&headers)
        .and_then(|t| m.users.session_user(&t))
        .map(|u| crate::users::Me {
            username: u.username,
            role: u.role,
            cameras: u.cameras,
        });
    Json(json!({ "setup_needed": m.users.setup_needed(), "user": me, "name": m.config().server.name })).into_response()
}

#[derive(Deserialize)]
struct LoginReq {
    username: String,
    password: String,
    #[serde(default)]
    remember: bool,
}

async fn auth_setup(State(m): App, Json(r): Json<LoginReq>) -> Response {
    match m.users.setup(&r.username, &r.password) {
        Ok(t) => {
            tracing::info!("admin account \"{}\" created", r.username.trim());
            (
                [(header::SET_COOKIE, session_cookie(&t, true))],
                Json(json!({ "ok": true })),
            )
                .into_response()
        }
        Err(e) => bad(e),
    }
}

async fn auth_login(State(m): App, Json(r): Json<LoginReq>) -> Response {
    // A little delay makes guessing passwords slow.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    match m.users.login(&r.username, &r.password, r.remember) {
        Ok(t) => (
            [(header::SET_COOKIE, session_cookie(&t, r.remember))],
            Json(json!({ "ok": true })),
        )
            .into_response(),
        Err(e) => {
            tracing::warn!("failed sign-in for \"{}\"", r.username.trim());
            err(StatusCode::UNAUTHORIZED, format!("{e:#}"))
        }
    }
}

async fn auth_logout(State(m): App, headers: axum::http::HeaderMap) -> Response {
    if let Some(t) = session_token(&headers) {
        m.users.logout(&t);
    }
    (
        [(
            header::SET_COOKIE,
            HeaderValue::from_static("gaze_session=; Path=/; Max-Age=0"),
        )],
        Json(json!({ "ok": true })),
    )
        .into_response()
}

#[derive(Deserialize)]
struct PasswordReq {
    current: String,
    new: String,
}

async fn auth_password(
    State(m): App,
    user: axum::Extension<CurrentUser>,
    Json(r): Json<PasswordReq>,
) -> Response {
    let name = user.0.0.username.clone();
    if m.users.login(&name, &r.current, false).is_err() {
        return err(StatusCode::BAD_REQUEST, "the current password is wrong");
    }
    match m.users.update(&name, None, None, Some(&r.new)) {
        Ok(()) => {
            // Sign in again with the new password on this browser.
            let t = m.users.login(&name, &r.new, true).unwrap_or_default();
            (
                [(header::SET_COOKIE, session_cookie(&t, true))],
                Json(json!({ "ok": true })),
            )
                .into_response()
        }
        Err(e) => bad(e),
    }
}

async fn users_list(State(m): App) -> Response {
    Json(m.users.list()).into_response()
}

#[derive(Deserialize)]
struct UserReq {
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
    role: Option<crate::users::Role>,
    cameras: Option<Vec<String>>,
}

async fn users_create(State(m): App, Json(r): Json<UserReq>) -> Response {
    match m.users.create(
        &r.username,
        &r.password,
        r.role.unwrap_or(crate::users::Role::Viewer),
        r.cameras.unwrap_or_default(),
    ) {
        Ok(()) => {
            tracing::info!("user \"{}\" created", r.username.trim());
            Json(json!({ "ok": true })).into_response()
        }
        Err(e) => bad(e),
    }
}

async fn users_update(State(m): App, Path(name): Path<String>, Json(r): Json<UserReq>) -> Response {
    let pw = (!r.password.is_empty()).then_some(r.password.as_str());
    match m.users.update(&name, r.role, r.cameras, pw) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => bad(e),
    }
}

async fn users_delete(
    State(m): App,
    Path(name): Path<String>,
    user: axum::Extension<CurrentUser>,
) -> Response {
    if user.0.0.username == name {
        return err(
            StatusCode::BAD_REQUEST,
            "you can't delete your own account while signed in with it",
        );
    }
    match m.users.delete(&name) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => bad(e),
    }
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
