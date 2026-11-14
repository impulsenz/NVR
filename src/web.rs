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
                    [
                        (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
                        (header::CACHE_CONTROL, "no-cache"),
                    ],
                    include_str!("../static/app.js"),
                )
            }),
        )
        .route(
            "/hls.min.js",
            get(|| async {
                (
                    [
                        (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
                        (header::CACHE_CONTROL, "public, max-age=86400"),
                    ],
                    // hls.js 1.5.20 (Apache-2.0, see static/vendor/hls.js-LICENSE.txt):
                    // plays the recordings as one continuous stream.
                    include_str!("../static/vendor/hls.min.js"),
                )
            }),
        )
        .route(
            "/app.css",
            get(|| async {
                (
                    [
                        (header::CONTENT_TYPE, "text/css; charset=utf-8"),
                        (header::CACHE_CONTROL, "no-cache"),
                    ],
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
        // Installing GAZE as an app (phones, Windows, Mac).
        .route("/manifest.webmanifest", get(|| async { ([(header::CONTENT_TYPE, "application/manifest+json"), (header::CACHE_CONTROL, "no-cache")], include_str!("../static/manifest.webmanifest")) }))
        .route("/sw.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")], include_str!("../static/sw.js")) }))
        .route("/icon-192.png", get(|| async { ([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "max-age=86400")], include_bytes!("../static/icon-192.png").as_slice()) }))
        .route("/icon-512.png", get(|| async { ([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "max-age=86400")], include_bytes!("../static/icon-512.png").as_slice()) }))
        .route("/icon-maskable.png", get(|| async { ([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "max-age=86400")], include_bytes!("../static/icon-maskable.png").as_slice()) }))
        .route("/api/auth/state", get(auth_state))
        .route("/api/auth/setup", post(auth_setup))
        .route("/api/auth/login", post(auth_login))
        .route("/api/auth/logout", post(auth_logout))
        .route("/api/auth/password", post(auth_password))
        .route("/api/auth/two-step/start", post(two_step_start))
        .route("/api/auth/tokens", get(tokens_list).post(tokens_new))
        .route("/api/auth/tokens/{id}", axum::routing::delete(tokens_remove))
        .route("/api/auth/two-step/finish", post(two_step_finish))
        .route("/api/auth/two-step/off", post(two_step_off))
        .route("/api/users/{name}/two-step-off", post(two_step_off_admin))
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
        .route("/api/usage/kinds", get(usage_kinds))
        .route("/api/logs", get(get_logs))
        .route("/api/logs/download", get(download_logs))
        .route("/api/settings", get(get_settings).put(put_settings))
        .route("/api/dashboard", put(put_dashboard))
        .route("/api/profiles", get(get_profiles).put(put_profiles))
        .route("/api/discover", post(discover_now))
        .route("/api/known", get(known_list).post(known_create))
        .route("/api/known/{id}", put(known_update).delete(known_delete))
        .route(
            "/api/known/{id}/photo",
            post(known_photo).layer(axum::extract::DefaultBodyLimit::max(25 * 1024 * 1024)),
        )
        .route("/api/known/{id}/photo/{name}", get(known_photo_get))
        .route("/api/known/{id}/learn", post(known_learn))
        .route("/api/known-inbox", get(inbox_list))
        .route("/api/known-seen", get(known_seen))
        .route("/api/faces", get(all_faces))
        .route("/api/known-inbox/assign", post(inbox_assign))
        .route("/api/known-inbox/dismiss", post(inbox_dismiss))
        .route("/api/cloud/status", get(cloud_status))
        .route("/api/cloud/remotes", post(cloud_remotes))
        .route("/api/alerts/seen", post(alerts_seen))
        .route("/api/alerts/seen-all", post(alerts_seen_all))
        .route("/api/alerts/unseen", post(alerts_unseen))
        .route("/api/config/export", get(export_config))
        .route("/api/config/import", post(import_config).layer(DefaultBodyLimit::max(200 * 1024 * 1024)))
        .route("/api/cameras", get(list_cameras).post(add_camera))
        .route(
            "/api/cameras/{id}",
            put(update_camera).delete(delete_camera),
        )
        .route("/api/cameras/{id}/pause", post(pause))
        .route("/api/cameras/{id}/ptz/move", post(ptz_move))
        .route("/api/cameras/{id}/device", get(device_get))
        .route("/api/cameras/{id}/device/reboot", post(device_reboot))
        .route("/api/cameras/{id}/device/report", get(device_report))
        .route("/api/cameras/{id}/device/network", put(device_network))
        .route("/api/cameras/{id}/device/time", put(device_time))
        .route("/api/cameras/{id}/device/video", put(device_video))
        .route("/api/cameras/{id}/device/imaging", put(device_imaging))
        .route("/api/cameras/{id}/ptz/stop", post(ptz_stop))
        .route("/api/cameras/{id}/talk", get(talk_ws))
        .route("/api/talk-test", post(talk_test))
        .route("/api/cameras/{id}/say", post(say))
        .route("/api/tts/voices", get(tts_voices))
        .route("/api/tts/preview", post(tts_preview))
        .route("/api/jobs", get(jobs_list))
        .route("/api/jobs/pause", post(jobs_pause))
        .route("/api/where", get(where_work_runs))
        .route("/api/https", get(https_status))
        .route("/api/mode", post(set_mode))
        .route("/api/sound", get(sound_status))
        .route("/api/search", get(describe_search))
        .route("/api/search/status", get(describe_status))
        .route("/api/remotes", get(remotes_list))
        .route("/remote/{i}/{cam}/frame.jpg", get(remote_frame))
        .route("/api/safe-mode/config", get(safe_config_get).post(safe_config_save))
        .route("/api/safe-mode/restore", post(safe_config_restore))
        .route("/api/snooze", get(snooze_link).post(snooze_link))
        .route("/api/cameras/{id}/snooze", post(snooze_camera))
        .route("/api/webhooks/test", post(webhook_test))
        .route("/api/modes", put(put_modes))
        .route("/api/https/tailscale", post(https_tailscale))
        .route("/api/update", get(update_status))
        .route("/api/update/check", post(update_check))
        .route("/api/update/install", post(update_install))
        .route("/api/notify/test", post(notify_test))
        .route("/api/notify/status", get(notify_status))
        .route("/api/maintenance/previews", post(rebuild_previews))
        .route("/api/maintenance/reidentify", post(reidentify))
        .route("/api/maintenance/delete-footage", post(delete_all_footage))
        .route(
            "/api/maintenance/alert-pictures",
            post(refresh_alert_pictures),
        )
        .route(
            "/api/cameras/{id}/ptz/presets",
            get(ptz_presets).post(ptz_save_preset),
        )
        .route("/api/cameras/{id}/ptz/goto", post(ptz_goto))
        .route("/api/cameras/{id}/ptz/patrol", get(ptz_patrol_get).put(ptz_patrol_put))
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
        .route("/api/timelapse-export", get(timelapse_export))
        .route("/api/sprite/{id}/{kind}/{name}", get(sprite_info))
        .route("/api/sprites-ready", post(sprites_ready))
        .route("/api/whatsnew", get(whats_new))
        .route("/api/probe", post(probe))
        .route("/api/fs", get(fs_list))
        .route("/api/fs/mkdir", post(fs_mkdir))
        .route("/api/preview", post(start_preview))
        .route("/api/preview/{token}", get(preview_stream))
        .route("/api/hw-test", post(hw_test))
        .route("/api/ai/test", post(ai_test))
        .route("/api/cameras/{id}/motion", get(camera_motion))
        .route("/api/cloud/test", post(cloud_test))
        .route("/api/live/ws", get(live_ws))
        .route("/api/hls/{id}/{stream}", get(hls_playlist))
        .route("/api/hls-file/{id}/{kind}/{name}", get(hls_file_playlist))
        .route("/media/{id}/rec/{name}", get(serve_recording))
        .route("/media/{id}/events/{name}", get(serve_event_file))
        .route("/media/{id}/tl/{name}", get(serve_timeline_file))
        .route("/media/{id}/thumb/{name}", get(recording_thumb))
        .route("/media/{id}/play/{kind}/{name}", get(play_converted))
        .route("/media/{id}/sprite/{kind}/{name}", get(sprite_image))
        .route("/live/{id}/stream.mjpeg", get(live_mjpeg))
        .route("/live/{id}/audio.mp3", get(live_audio))
        .route("/live/{id}/frame.jpg", get(live_frame))
        .route("/live/{id}/video", get(live_video_ws))
        .route("/overview.jpg", get(overview_jpg))
        .route("/overview.mjpeg", get(overview_mjpeg))
        .route("/api/overview/key", get(overview_key).post(overview_key_renew))
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

/// The page. Its script and styles are asked for with this version in
/// their address, and none of the three is kept by the browser without
/// asking: after an update the browser always runs the new page (an old
/// script kept in its cache, talking to the new server, broke things until
/// the cache was cleared).
async fn index() -> Response {
    static PAGE: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        let v = env!("CARGO_PKG_VERSION");
        include_str!("../static/index.html")
            .replace("href=\"app.css\"", &format!("href=\"app.css?v={v}\""))
            .replace("src=\"app.js\"", &format!("src=\"app.js?v={v}\""))
    });
    ([(header::CACHE_CONTROL, "no-cache")], Html(PAGE.as_str())).into_response()
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
                // What it alerts on (the Alerts filter offers these).
                "watch": cur.detect.objects,
                "snoozed_until": crate::notify::snoozed_until(&c.cfg.id),
                "camera_events": crate::onvif_events::status(&c.cfg.id),
                "streaming": crate::rtmp::status(&c.cfg.id),
                "alerts_on": alert_kinds(cur, &cfg.ai),
                "talk": c.cfg.talk,
                "talk_chime": c.cfg.talk_chime,
                "sound": c.cfg.live_sound,
                "url": mask_url(&c.cfg.url),
                "transcode": c.cfg.reencode(),
                "detection": c.cfg.detection_enabled(),
                "ai": cur.detect.ai,
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
    let upd = crate::update::status();
    Json(json!({
        "name": cfg.server.name,
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_seconds": m.started.elapsed().as_secs(),
        "display": cfg.display,
        "live": cfg.live,
        // The user's own layout for this kind of device (else the shared one).
        "mode": { "current": cfg.modes.current, "effective": cfg.modes.effective(crate::notify::is_night(&cfg.notify, Local::now())), "modes": cfg.modes },
        "dashboard": user.as_ref().and_then(|u| m.users.layout(&u.0.0.username, device_kind(&dq.device))).unwrap_or(cfg.dashboard.clone()),
        "me": user.as_ref().map(|u| crate::users::Me { username: u.0.0.username.clone(), role: u.0.0.role, two_step: u.0.0.two_step(), cameras: u.0.0.cameras.clone() }),
        "cameras": cameras,
        "ai": m.ai.get().status(),
        // Where the AI models run (GPU or processor, and why).
        "ai_device": crate::infer::status(),
        // A newer GAZE (for the notice at the top; admins only use it).
        "update": {"available": upd.available, "latest": upd.latest, "state": upd.state,
            "done": upd.done, "total": upd.total, "error": upd.error, "published": upd.published},
        // Downloads in progress (AI models, updates), for the header.
        "downloads": crate::jobs::list().into_iter()
            .filter(|j| j.kind == "download" && j.finished.is_none())
            .map(|j| json!({"title": j.title, "done": j.done, "total": j.total}))
            .collect::<Vec<_>>(),
        "alerts_unseen": library::unseen_count(&m),
        "say_phrases": m.config().tts.phrases,
        "timelapse": crate::picturelog::enabled(),
        "timelapse_seconds": crate::picturelog::seconds(),
        "safe_mode": Config::safe_mode(),
        "has_remotes": !cfg.remotes.is_empty(),
        "safe_mode_good_copy": Config::safe_mode().is_some() && m.cfg_path.with_extension("toml.good").is_file(),
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
        "ai_objects": crate::ai::MAIN_OBJECTS,
        "ai_more": ai_more(&m),
    }))
    .into_response()
}

/// This computer's network (IPv4) addresses, main one first.
pub fn local_addresses() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    // Every network adapter's address (a computer can be on several
    // networks, e.g. 192.168.1.x and 192.168.5.x).
    for i in if_addrs::get_if_addrs().unwrap_or_default() {
        let ip = i.ip();
        if ip.is_ipv4() && !ip.is_loopback() && !ip.is_unspecified() && !out.contains(&ip.to_string()) {
            out.push(ip.to_string());
        }
    }
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

async fn usage(State(m): App, user: Option<axum::Extension<CurrentUser>>) -> Response {
    let mut u = m.usage();
    let admin = user.as_ref().is_none_or(|x| x.0.0.role == crate::users::Role::Admin);
    if let Some(v) = visible(&user) {
        u.cameras.retain(|c| v.contains(&c.usage.camera));
        for l in &mut u.locations {
            l.cameras.retain(|c| v.contains(c));
        }
        u.locations.retain(|l| !l.cameras.is_empty());
    }
    if !admin {
        for c in &mut u.cameras {
            c.usage.dir = PathBuf::new();
        }
        for l in &mut u.locations {
            l.path = PathBuf::new();
        }
    }
    Json(u).into_response()
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
    let has_token = !cfg.notify.token.is_empty();
    let has_cloud_password = !cfg.cloud.password.is_empty();
    let has_mqtt_password = !cfg.mqtt.password.is_empty();
    for r in &mut cfg.remotes {
        r.token.clear();
    }
    cfg.mqtt.password.clear();
    cfg.server.password.clear();
    cfg.notify.token.clear();
    cfg.cloud.password.clear();
    let mut v = serde_json::to_value(&cfg).unwrap();
    v["notify"]["has_token"] = json!(has_token);
    v["cloud"]["has_password"] = json!(has_cloud_password);
    v["mqtt"]["has_password"] = json!(has_mqtt_password);
    v["mqtt"]["status"] = json!(crate::mqtt::status());
    v.as_object_mut().unwrap().remove("cameras");
    Json(v).into_response()
}

async fn put_settings(State(m): App, Json(up): Json<SettingsUpdate>) -> Response {
    match m.update_settings(up).await {
        Ok(restart) => Json(json!({ "ok": true, "restart_required": restart })).into_response(),
        Err(e) => bad(e),
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

async fn alerts_unseen(
    State(m): App,
    user: Option<axum::Extension<CurrentUser>>,
    Json(r): Json<SeenReq>,
) -> Response {
    if visible(&user).is_some_and(|v| !v.contains(&r.camera)) {
        return err(StatusCode::FORBIDDEN, "your account can't see this camera");
    }
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

#[derive(Deserialize, Default)]
struct DiscoverReq {
    /// Other networks to search too, e.g. "192.168.5.0/24, 10.0.2.x".
    #[serde(default)]
    networks: String,
}

async fn discover_now(State(m): App, body: Option<Json<DiscoverReq>>) -> Response {
    let extra = crate::discover::parse_networks(&body.map(|b| b.0).unwrap_or_default().networks);
    let found = crate::discover::scan(&m, &extra).await;
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
        let k = m
            .known
            .get(&id)
            .ok_or_else(|| anyhow::anyhow!("not found"))?;
        let img = crate::imgutil::decode_jpeg(&body)
            .map_err(|_| anyhow::anyhow!("send the photo as a JPEG picture"))?;
        if k.kind == "person" {
            let mut faces: Vec<_> = m
                .recog
                .faces(&img)
                .await?
                .into_iter()
                .filter(|f| f.score >= 0.7)
                .collect();
            faces.sort_by(|a, b| (b.w * b.h).total_cmp(&(a.w * a.h)));
            let face = faces.first().ok_or_else(|| {
                anyhow::anyhow!("no face found in this photo — use a clear photo of the face")
            })?;
            // Several people of similar size: we can't tell which one is meant.
            if let Some(second) = faces.get(1)
                && second.w * second.h > face.w * face.h * 0.5
            {
                anyhow::bail!(
                    "this photo has {} faces — use a photo of {} alone (or crop it)",
                    faces.len(),
                    k.name
                );
            }
            let print = m.recog.faceprint(&img, face).await?;
            m.known.add(&id, Some(&print), None, Some(&body))?;
            anyhow::Ok(json!({ "ok": true, "message": format!("Learned {}'s face", k.name) }))
        } else {
            let plates = m.recog.plates(&img).await.unwrap_or_default();
            for p in &plates {
                m.known.add(&id, None, Some(&p.text), None)?;
            }
            m.known.add(&id, None, None, Some(&body))?;
            let what = match plates.first() {
                Some(p) => format!("Read the plate {} and saved the photo", p.text),
                None => "Photo saved (no plate could be read — type it in)".to_string(),
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

/// "This is …": name a face seen in an alert (and learn it), or correct a
/// wrong name: the person it was wrongly taken for stops matching it. The
/// id "none" means "not anyone I know".
async fn known_learn(
    State(m): App,
    user: Option<axum::Extension<CurrentUser>>,
    Path(id): Path<String>,
    Json(r): Json<LearnReq>,
) -> Response {
    if visible(&user).is_some_and(|v| !v.contains(&r.camera)) {
        return err(StatusCode::FORBIDDEN, "your account can't see this camera");
    }
    let res = (|| {
        let k = if id == "none" {
            None
        } else {
            Some(
                m.known
                    .get(&id)
                    .ok_or_else(|| anyhow::anyhow!("not found"))?,
            )
        };
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
        let new_name = k.as_ref().map(|k| k.name.clone());
        // Wrongly named before: that person no longer matches this face.
        let old = f.name.clone().filter(|o| Some(o) != new_name.as_ref());
        if let Some(o) = &old {
            m.known.not_this(o, &print)?;
        }
        // Confirming a name GAZE gave ("✓ Right"): the face is added to
        // that person's profile too, so they're recognised more surely.
        let again = f.confirmed && f.name == new_name;
        let confirming = !f.confirmed && f.name.is_some() && f.name == new_name;
        if let Some(k) = &k
            && !again
        {
            let photo = std::fs::read(c.events.dir().join(&f.file)).ok();
            m.known.add(&k.id, Some(&print), None, photo.as_deref())?;
        }
        f.confirmed = true;
        f.name = new_name.clone();
        // The alert's notes: who was recognised, and whether a face is unknown.
        let names: Vec<String> = e.faces.iter().filter_map(|f| f.name.clone()).collect();
        e.notes.retain(|n| {
            n.strip_prefix("Recognized ")
                .is_none_or(|who| names.iter().any(|x| x == who))
        });
        for n in &names {
            let note = format!("Recognized {n}");
            if !e.notes.contains(&note) {
                e.notes.push(note);
            }
        }
        let unknown = e.faces.iter().any(|f| f.name.is_none());
        e.notes.retain(|n| n != "Unknown face");
        if unknown {
            e.notes.push("Unknown face".into());
        }
        let notes = e.notes.clone();
        c.events.save(e);
        anyhow::Ok((
            notes,
            match (new_name, old) {
                (Some(n), Some(o)) => format!(
                    "Corrected — this is {n}, not {o}. GAZE won't take this face for {o} again."
                ),
                (Some(n), None) if again => format!("Already confirmed — this is {n}"),
                (Some(n), None) if confirming => format!("Confirmed — this is {n}. Added to {n}'s faces, so GAZE knows them better."),
                (Some(n), None) => format!("Learned — this is {n}. Added to {n}'s faces."),
                (None, Some(o)) => {
                    format!("Corrected — not {o}. GAZE won't take this face for {o} again.")
                }
                (None, None) => "Marked as someone you don't know".to_string(),
            },
        ))
    })();
    match res {
        Ok((notes, message)) => {
            Json(json!({ "ok": true, "message": message, "notes": notes })).into_response()
        }
        Err(e) => bad(e),
    }
}

/// The kinds of alert a camera can raise, for the Alerts page's filter.
fn alert_kinds(c: &crate::config::CameraConfig, ai: &crate::config::AiConfig) -> Vec<&'static str> {
    let d = &c.detect;
    let mut k = Vec::new();
    if !c.enabled {
        return k;
    }
    if d.motion && d.alert_on_motion() {
        k.push("motion");
    }
    if d.ai {
        if d.packages {
            k.push("package");
        }
        if ai.faces && d.objects.iter().any(|o| o == "person") {
            k.push("face");
        }
        let vehicles = ["vehicle", "car", "truck", "bus", "motorcycle"];
        if ai.plates && d.objects.iter().any(|o| vehicles.contains(&o.as_str())) {
            k.push("plate");
        }
    }
    if c.sound.enabled && !c.sound.kinds.is_empty() {
        k.push("sound");
    }
    k
}

// ---------- new faces & plates ----------

fn recent_events(m: &Manager, days: i64) -> Vec<crate::events::Event> {
    let now = Local::now();
    m.cameras()
        .iter()
        .flat_map(|c| c.events.between(now - chrono::Duration::days(days), now))
        .collect()
}

#[derive(Deserialize)]
struct FacesQuery {
    #[serde(default)]
    days: Option<i64>,
}

/// Every face found in alerts (Settings → Check faces), newest first: who
/// GAZE took it for, and whether someone has confirmed it.
async fn all_faces(State(m): App, user: Option<axum::Extension<CurrentUser>>, Query(q): Query<FacesQuery>) -> Response {
    let vis = visible(&user);
    let names: std::collections::HashMap<String, String> = m.cameras().iter().map(|c| (c.cfg.id.clone(), c.cfg.name.clone())).collect();
    let mut evs = recent_events(&m, q.days.unwrap_or(30).clamp(1, 365));
    evs.retain(|e| !e.faces.is_empty() && vis.as_ref().is_none_or(|v| v.contains(&e.camera)));
    evs.sort_by(|a, b| b.start.cmp(&a.start));
    let mut out = Vec::new();
    for e in &evs {
        for (i, f) in e.faces.iter().enumerate() {
            out.push(json!({
                "camera": e.camera,
                "camera_name": names.get(&e.camera).cloned().unwrap_or_else(|| e.camera.clone()),
                "event": e.id,
                "face": i,
                "time": e.start,
                "name": f.name,
                "confirmed": f.confirmed,
                "picture": format!("media/{}/events/{}", e.camera, f.file),
            }));
            if out.len() >= 3000 {
                break;
            }
        }
    }
    Json(out).into_response()
}

/// For each known person: the faces in the last 30 days of alerts that were
/// taken for them (newest first, up to 30), to check and correct.
async fn known_seen(State(m): App, user: Option<axum::Extension<CurrentUser>>) -> Response {
    let vis = visible(&user);
    let mut evs = recent_events(&m, 30);
    evs.retain(|e| vis.as_ref().is_none_or(|v| v.contains(&e.camera)));
    evs.sort_by(|a, b| b.start.cmp(&a.start));
    let mut out: std::collections::BTreeMap<String, Vec<serde_json::Value>> = Default::default();
    for e in &evs {
        for (i, f) in e.faces.iter().enumerate() {
            let Some(name) = &f.name else { continue };
            let list = out.entry(name.clone()).or_default();
            if list.len() < 30 {
                list.push(json!({
                    "camera": e.camera,
                    "event": e.id,
                    "face": i,
                    "time": e.start,
                    "picture": format!("media/{}/events/{}", e.camera, f.file),
                }));
            }
        }
    }
    Json(out).into_response()
}

#[derive(Deserialize)]
struct InboxRange {
    from: Option<DateTime<Local>>,
    to: Option<DateTime<Local>>,
}

/// New faces and plates (not named yet), from the last 30 days, or from the
/// days asked for (the Alerts page's "New faces & plates" filter).
async fn inbox_list(
    State(m): App,
    user: Option<axum::Extension<CurrentUser>>,
    Query(r): Query<InboxRange>,
) -> Response {
    let vis = visible(&user);
    let mut evs = match (r.from, r.to) {
        (Some(from), Some(to)) => m
            .cameras()
            .iter()
            .flat_map(|c| c.events.between(from, to))
            .collect(),
        _ => recent_events(&m, 30),
    };
    evs.retain(|e| vis.as_ref().is_none_or(|v| v.contains(&e.camera)));
    let d = crate::inbox::dismissed(&m.data_dir);
    let faces = crate::inbox::group_faces(&evs, &d);
    let plates = crate::inbox::group_plates(&evs, &d, |p| m.known.match_plate(p).is_some());
    Json(json!({ "faces": faces, "plates": plates })).into_response()
}

#[derive(Deserialize)]
struct InboxReq {
    /// Known person / vehicle id (assign).
    #[serde(default)]
    known: String,
    #[serde(default)]
    members: Vec<crate::inbox::FaceRef>,
    #[serde(default)]
    plate: String,
}

/// Name every alert a face group is in, and learn the face.
async fn inbox_assign(State(m): App, Json(r): Json<InboxReq>) -> Response {
    let res = (|| {
        let k = m
            .known
            .get(&r.known)
            .ok_or_else(|| anyhow::anyhow!("choose a person or vehicle"))?;
        if !r.plate.is_empty() {
            if k.kind != "vehicle" {
                anyhow::bail!("a number plate belongs to a vehicle");
            }
            m.known.add(&k.id, None, Some(&r.plate), None)?;
            // Alerts with this plate now say whose it is.
            let mut n = 0;
            for c in m.cameras() {
                for mut e in c
                    .events
                    .between(Local::now() - chrono::Duration::days(30), Local::now())
                {
                    if !e
                        .plates
                        .iter()
                        .any(|p| crate::recog::plate_matches(p, &r.plate))
                    {
                        continue;
                    }
                    let note = format!("Plate {} — {}", r.plate, k.name);
                    e.notes
                        .retain(|x| !x.ends_with(&format!(" plate {}", r.plate)));
                    if !e.notes.contains(&note) {
                        e.notes.push(note);
                    }
                    c.events.save(e);
                    n += 1;
                }
            }
            return anyhow::Ok(format!(
                "Plate {} is {} — {n} alert(s) updated",
                r.plate, k.name
            ));
        }
        if k.kind != "person" {
            anyhow::bail!("a face belongs to a person");
        }
        let mut prints = Vec::new();
        let mut photo = None;
        for f in &r.members {
            let Some(c) = m.camera(&f.camera) else {
                continue;
            };
            let Some(mut e) = c.events.get(&f.event) else {
                continue;
            };
            let Some(face) = e.faces.get_mut(f.face) else {
                continue;
            };
            prints.push(crate::known::unpack(&face.print));
            if photo.is_none() {
                photo = std::fs::read(c.events.dir().join(&face.file)).ok();
            }
            face.name = Some(k.name.clone());
            if !e.faces.iter().any(|x| x.name.is_none()) {
                e.notes.retain(|n| n != "Unknown face");
            }
            let note = format!("Recognized {}", k.name);
            if !e.notes.contains(&note) {
                e.notes.push(note);
            }
            c.events.save(e);
        }
        anyhow::ensure!(!prints.is_empty(), "those alerts are gone");
        for (i, p) in crate::inbox::learn_set(&prints).iter().enumerate() {
            m.known.add(
                &k.id,
                Some(p),
                None,
                if i == 0 { photo.as_deref() } else { None },
            )?;
        }
        anyhow::Ok(format!(
            "This is {} — {} alert(s) named and the face learned",
            k.name,
            r.members.len()
        ))
    })();
    match res {
        Ok(msg) => Json(json!({ "ok": true, "message": msg })).into_response(),
        Err(e) => bad(e),
    }
}

/// "Not interesting": hide a face (a passer-by) or a plate from the list.
async fn inbox_dismiss(State(m): App, Json(r): Json<InboxReq>) -> Response {
    let mut d = crate::inbox::dismissed(&m.data_dir);
    if !r.plate.is_empty() {
        d.plates.push(r.plate.clone());
    } else if let Some(f) = r.members.first()
        && let Some(e) = m.camera(&f.camera).and_then(|c| c.events.get(&f.event))
        && let Some(p) = crate::inbox::print_of(&e, f.face)
    {
        d.faces.push(p);
    } else {
        return err(StatusCode::BAD_REQUEST, "nothing to hide");
    }
    match crate::inbox::save_dismissed(&m.data_dir, &d) {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => bad(e),
    }
}

async fn get_profiles(State(m): App) -> Response {
    Json(m.config().profiles).into_response()
}

#[derive(Deserialize)]
struct ProfileRename {
    rename_from: Option<String>,
    rename_to: Option<String>,
}

async fn put_profiles(
    State(m): App,
    Query(r): Query<ProfileRename>,
    Json(p): Json<Vec<crate::config::Profile>>,
) -> Response {
    let rename = r.rename_from.zip(r.rename_to);
    match m.update_profiles(p, rename).await {
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

/// Download the whole setup as one backup file (.zip): settings, cameras
/// and profiles (settings.toml), users with their layouts (users.json), and
/// people & vehicles with their photos (known.json, known/). Without
/// `passwords`, every password, key and token is left out.
async fn export_config(State(m): App, Query(q): Query<ExportCfgQuery>) -> Response {
    let mut cfg = m.config();
    if !q.passwords {
        cfg.remove_secrets();
    }
    let res = (|| {
        use std::io::Write;
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opt = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        z.start_file("settings.toml", opt)?;
        z.write_all(cfg.to_toml()?.as_bytes())?;
        z.start_file("users.json", opt)?;
        z.write_all(&m.users.export(q.passwords))?;
        let (known, photos) = m.known.export();
        z.start_file("known.json", opt)?;
        z.write_all(&known)?;
        // Photos are JPEGs already: stored as they are.
        let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, b) in photos {
            z.start_file(name, stored)?;
            z.write_all(&b)?;
        }
        anyhow::Ok(z.finish()?.into_inner())
    })();
    match res {
        Ok(bytes) => {
            let name = format!("gaze-backup-{}.zip", Local::now().format("%Y-%m-%d"));
            (
                [
                    (header::CONTENT_TYPE, "application/zip".to_string()),
                    (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\"")),
                ],
                bytes,
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

/// Restore a backup: the .zip from Export, or an older settings-only .toml.
/// Passwords the backup left out are kept from the settings in use.
async fn import_config(State(m): App, Query(q): Query<ImportQuery>, body: Bytes) -> Response {
    let res = async {
        // Every file in the backup (at most 500 MB unpacked).
        let is_zip = body.starts_with(b"PK");
        let mut files: std::collections::HashMap<String, Vec<u8>> = std::collections::HashMap::new();
        if is_zip {
            use std::io::Read;
            let mut z = zip::ZipArchive::new(std::io::Cursor::new(body.to_vec()))?;
            let mut total = 0u64;
            for i in 0..z.len() {
                let f = z.by_index(i)?;
                if !f.is_file() {
                    continue;
                }
                let name = f.name()?.to_string();
                let mut b = Vec::new();
                f.take(500 * 1024 * 1024 - total).read_to_end(&mut b)?;
                total += b.len() as u64;
                anyhow::ensure!(total < 500 * 1024 * 1024, "the backup is too big");
                files.insert(name, b);
            }
        }
        let read = |name: &str| files.get(name).cloned();
        let text = match read("settings.toml") {
            Some(b) => String::from_utf8(b)?,
            None if is_zip => anyhow::bail!("this .zip isn't a GAZE backup (no settings.toml in it)"),
            None => String::from_utf8(body.to_vec()).map_err(|_| anyhow::anyhow!("choose the backup file made by Export (.zip or .toml)"))?,
        };
        let (mut cfg, _) = Config::from_toml(&text)?;
        cfg.keep_secrets_from(&m.config());
        let users = read("users.json");
        let known = read("known.json");
        let n = cfg.cameras.len();
        let restart = m.import(cfg, q.keep_login).await?;
        let mut parts = vec![format!("{n} camera(s) and all settings")];
        if let Some(u) = users {
            let (got, skipped) = m.users.import(&u)?;
            parts.push(format!("{got} user(s) with their layouts"));
            if skipped > 0 {
                parts.push(format!("{skipped} new user(s) skipped — the backup has no passwords; add them in Settings → Users"));
            }
        }
        if let Some(k) = known {
            let got = m.known.import(&k, |id, p| read(&format!("known/{id}/{p}")))?;
            parts.push(format!("{got} people & vehicles"));
        }
        anyhow::Ok((n, restart, parts.join(", ")))
    }
    .await;
    match res {
        Ok((n, restart, what)) => {
            Json(json!({ "ok": true, "cameras": n, "restart_required": restart, "message": format!("Imported {what}.") })).into_response()
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

/// Whether the camera answers over ONVIF (for its Restart button).
async fn device_get(State(m): App, Path(id): Path<String>) -> Response {
    let t = match device_target(&m, &id) {
        Ok(t) => t,
        Err(e) => return bad(e),
    };
    match crate::onvif::Onvif::default().device_info(&t).await {
        Ok(i) => {
            Json(json!({ "ok": true, "info": i, "address": format!("{}:{}", t.host, t.port) }))
                .into_response()
        }
        Err(e) => Json(json!({ "ok": false, "error": format!("{e:#}") })).into_response(),
    }
}

/// Everything the camera tells over ONVIF, for its ONVIF tab.
async fn device_report(State(m): App, Path(id): Path<String>) -> Response {
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
    crate::ptz_auto::manual(&id);
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
    crate::ptz_auto::manual(&id);
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

#[derive(Deserialize, serde::Serialize)]
struct PatrolReq {
    /// Preset tokens visited in turn (empty = not roaming).
    patrol: Vec<String>,
    patrol_seconds: u32,
}

async fn ptz_patrol_get(State(m): App, Path(id): Path<String>) -> Response {
    match m.config().cameras.iter().find(|c| c.id == id) {
        Some(c) => Json(PatrolReq { patrol: c.ptz.patrol.clone(), patrol_seconds: c.ptz.patrol_seconds }).into_response(),
        None => err(StatusCode::NOT_FOUND, "no such camera"),
    }
}

/// The preset overlay's "Roam between presets": which, and how long at each.
async fn ptz_patrol_put(State(m): App, Path(id): Path<String>, Json(r): Json<PatrolReq>) -> Response {
    let Some(mut cam) = m.config().cameras.iter().find(|c| c.id == id).cloned() else {
        return err(StatusCode::NOT_FOUND, "no such camera");
    };
    cam.ptz.patrol = r.patrol.into_iter().take(64).collect();
    cam.ptz.patrol_seconds = r.patrol_seconds.clamp(5, 3600);
    match m.update_camera(&id, cam, false).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
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
        Some(cfg) => {
            let h = crate::ai::AiHandle::new(cfg, &m.data_dir);
            h.get()
        }
        None => m.ai.get(),
    };
    // A freshly created built-in engine needs a moment to load its model.
    let t = Instant::now();
    let mut res = ai.detect_stages(&frame).await;
    while matches!(res, Err(crate::ai::AiError::NotReady(_)))
        && t.elapsed() < Duration::from_secs(120)
    {
        tokio::time::sleep(Duration::from_millis(500)).await;
        res = ai.detect_stages(&frame).await;
    }
    let data_url = |f: &crate::imgutil::Frame| {
        f.jpeg(85).ok().map(|j| format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(j)))
    };
    let image = data_url(&frame);
    let stages = match res {
        Ok(s) => s,
        Err(e) => return Json(json!({ "ok": false, "error": e.to_string(), "image": image })).into_response(),
    };
    // As during an alert: the double-check model's answer counts when it
    // ran, and what it found is checked in a full-quality picture (when
    // the camera has the HD check on, or always here when it has a main
    // stream to check with).
    let finds = stages.last().map(|s| s.1.clone()).unwrap_or_default();
    let mut out: Vec<serde_json::Value> = stages
        .iter()
        .enumerate()
        .map(|(i, (model, d, ms))| {
            json!({"kind": if i == 0 { "fast" } else { "verify" }, "model": model, "detections": d, "ms": ms,
                   "image": if i == 0 { image.clone() } else { None }})
        })
        .collect();
    let watched: Vec<crate::ai::Detection> = finds
        .iter()
        .filter(|d| c.cfg.detect.objects.contains(&d.label))
        .cloned()
        .collect();
    if c.cfg.has_sub() {
        let t = Instant::now();
        let ffmpeg = m.config().server.ffmpeg_path;
        match crate::detect::hd_test(&ai, &c.cfg, &c.relay_main, &ffmpeg, &watched).await {
            Ok((f, checks)) => out.push(json!({
                "kind": "hd", "image": data_url(&f), "ms": t.elapsed().as_millis() as u64, "setting": c.cfg.detect.hd_check,
                "detections": checks.iter().map(|(d, v)| json!({"x": d.x, "y": d.y, "w": d.w, "h": d.h, "label": v, "score": d.score, "ok": v.starts_with('✓')})).collect::<Vec<_>>(),
            })),
            Err(e) => out.push(json!({"kind": "hd", "error": format!("{e:#}"), "setting": c.cfg.detect.hd_check, "detections": []})),
        }
    }
    Json(json!({ "ok": true, "detections": finds, "image": image, "ms": t.elapsed().as_millis() as u64, "stages": out,
                 "watched": c.cfg.detect.objects }))
    .into_response()
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

async fn cloud_test(State(m): App, Json(mut c): Json<crate::config::CloudConfig>) -> Response {
    // The saved password only goes to the saved destination.
    let saved = m.config().cloud;
    if c.password.is_empty() && c.remote.trim() == saved.remote.trim() {
        c.password = saved.password;
    }
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
    /// Only alerts with this known person or vehicle (its id).
    #[serde(default)]
    who: String,
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
    // A known person or vehicle (People & vehicles).
    let who = (!q.who.is_empty())
        .then(|| m.known.get(&q.who))
        .flatten()
        .map(|k| library::Who {
            name: k.name,
            plates: k.plates,
        });
    let lq = LibQuery {
        cameras: allowed_cameras(split_list(&q.cameras), visible(&user)),
        from,
        to,
        tod,
        types: split_list(&q.types).into_iter().collect(),
        who,
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
    Json(items).into_response()
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

/// Space taken by each kind of footage, per camera (counted every few
/// minutes: walking the folders takes a moment).
async fn usage_kinds(State(m): App, user: Option<axum::Extension<CurrentUser>>) -> Response {
    static CACHE: std::sync::Mutex<Option<(std::time::Instant, Vec<library::SpaceUse>)>> = std::sync::Mutex::new(None);
    let cached = CACHE.lock().unwrap().as_ref().filter(|(t, _)| t.elapsed() < Duration::from_secs(180)).map(|(_, v)| v.clone());
    let list = match cached {
        Some(v) => v,
        None => {
            let v = tokio::task::spawn_blocking(move || m.cameras().iter().map(|c| library::space_use(c)).collect::<Vec<_>>())
                .await
                .unwrap_or_default();
            *CACHE.lock().unwrap() = Some((std::time::Instant::now(), v.clone()));
            v
        }
    };
    let vis = visible(&user);
    let list: Vec<_> = list.into_iter().filter(|u| vis.as_ref().is_none_or(|v| v.contains(&u.camera))).collect();
    Json(list).into_response()
}

#[derive(Deserialize)]
struct HlsQuery {
    from: DateTime<chrono::FixedOffset>,
    to: DateTime<chrono::FixedOffset>,
    /// Reaches now: a growing playlist (see hls::playlist_as).
    #[serde(default)]
    live: bool,
}

/// A camera's recordings of one stream ("sub" or "main", path ends in
/// `.m3u8`) between two times, as one continuous HLS playlist (see hls.rs).
/// HD alert clips aren't in it (they're played as files).
async fn hls_playlist(
    State(m): App,
    user: Option<axum::Extension<CurrentUser>>,
    Path((id, stream)): Path<(String, String)>,
    Query(q): Query<HlsQuery>,
) -> Response {
    if visible(&user).is_some_and(|v| !v.contains(&id)) {
        return err(StatusCode::FORBIDDEN, "your account can't see this camera");
    }
    let c = camera!(m, id);
    let stream = stream.trim_end_matches(".m3u8").to_string();
    let (from, to, live) = (q.from.with_timezone(&Local), q.to.with_timezone(&Local), q.live);
    if to <= from || (to - from).num_hours() > 26 {
        return err(StatusCode::BAD_REQUEST, "bad time range");
    }
    let text = tokio::task::spawn_blocking(move || {
        let t = library::timeline(&m, std::slice::from_ref(&c.cfg.id), from, to);
        let mut segs: Vec<_> = t
            .into_iter()
            .flat_map(|x| x.segments)
            .filter(|s| s.kind != "clip" && s.stream == stream)
            .collect();
        segs.sort_by_key(|s| s.start);
        let mut files = Vec::new();
        for s in segs {
            let path = if s.kind == "sub" {
                storage::timeline_dir(&c.dir).join(&s.name)
            } else {
                c.dir.join(&s.name)
            };
            files.push(crate::hls::PlaylistFile { path, url: s.url, start: s.start });
        }
        crate::hls::playlist_as(&files, 4.0, "../../../", live && to > Local::now())
    })
    .await
    .unwrap_or_default();
    (
        [
            (header::CONTENT_TYPE, "application/vnd.apple.mpegurl"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        text,
    )
        .into_response()
}

/// One recording as an HLS playlist of its pieces (see hls.rs): the camera
/// view's player opens and jumps within a file without reading it through.
async fn hls_file_playlist(
    State(m): App,
    user: Option<axum::Extension<CurrentUser>>,
    Path((id, kind, name)): Path<(String, String, String)>,
) -> Response {
    if visible(&user).is_some_and(|v| !v.contains(&id)) {
        return err(StatusCode::FORBIDDEN, "your account can't see this camera");
    }
    let c = camera!(m, id);
    let name = name.trim_end_matches(".m3u8").to_string();
    let (path, url) = match kind.as_str() {
        "rec" => (storage::resolve(&c.dir, &name), library::rec_url(&c.cfg.id, &name)),
        "tl" => (
            storage::resolve(&storage::timeline_dir(&c.dir), &name),
            format!("media/{}/tl/{}", library::enc(&c.cfg.id), library::enc(&name)),
        ),
        _ => return err(StatusCode::NOT_FOUND, "not found"),
    };
    let Ok(path) = path else { return err(StatusCode::NOT_FOUND, "not found") };
    let start = Local::now();
    let text = tokio::task::spawn_blocking(move || {
        crate::hls::index_cached(&path)?;
        Some(crate::hls::playlist(&[crate::hls::PlaylistFile { path, url, start }], 4.0, "../../../../"))
    })
    .await
    .ok()
    .flatten();
    match text {
        Some(t) => ([(header::CONTENT_TYPE, "application/vnd.apple.mpegurl"), (header::CACHE_CONTROL, "no-store")], t).into_response(),
        None => err(StatusCode::NOT_FOUND, "this recording can't be streamed in pieces"),
    }
}

#[derive(Deserialize)]
struct ExportQuery {
    camera: String,
    from: DateTime<chrono::FixedOffset>,
    to: DateTime<chrono::FixedOffset>,
}

/// Cut a time range out of the recordings and download it as one MP4.
async fn export_clip(
    State(m): App,
    user: Option<axum::Extension<CurrentUser>>,
    Query(q): Query<ExportQuery>,
    req: Request,
) -> Response {
    // The camera is in the query here, not the path: check it's one this
    // account may see.
    if visible(&user).is_some_and(|v| !v.contains(&q.camera)) {
        return err(StatusCode::FORBIDDEN, "your account can't see this camera");
    }
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
        "{} {} to {}.mp4",
        file_safe(&c.cfg.name),
        from.format("%Y-%m-%d %H-%M-%S"),
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

/// Content-Disposition for a download: a plain-ASCII name for old browsers
/// and the exact (UTF-8) name for the rest.
fn attachment(name: &str) -> String {
    let ascii: String = name.chars().map(|c| if c.is_ascii_graphic() || c == ' ' { if c == '"' { '\'' } else { c } } else { '_' }).collect();
    let enc: String = name
        .bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect();
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{enc}")
}

/// A name without characters Windows doesn't allow in file names.
fn file_safe(s: &str) -> String {
    let t: String = s.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) || c.is_control() { '-' } else { c }).collect();
    let t = t.trim().trim_end_matches('.').to_string();
    if t.is_empty() { "camera".into() } else { t }
}

/// What a downloaded file is called: the camera's name and the time of the
/// footage, e.g. "Front Door 2026-11-12 09-01-40.mp4" (the file's own name
/// when the time isn't known).
fn download_name(camera: &str, when: Option<DateTime<Local>>, file: &str) -> String {
    let ext = std::path::Path::new(file).extension().and_then(|e| e.to_str()).unwrap_or("mp4");
    match when {
        Some(t) => format!("{} {}.{ext}", file_safe(camera), t.format("%Y-%m-%d %H-%M-%S")),
        None => file.to_string(),
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
                && let Ok(v) = HeaderValue::from_str(&attachment(&name))
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
        Ok(p) => {
            let dl = download_name(&c.cfg.name, storage::parse_start(&name), &name);
            serve_file(p, req, Some(dl), false).await
        }
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
        Ok(p) => {
            let dl = download_name(&c.cfg.name, storage::parse_start(&name), &name);
            serve_file(p, req, Some(dl), false).await
        }
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
        Ok(p) => {
            // The alert's file: named after the camera and the alert's time.
            let when = c.events.all().into_iter().find(|e| name.starts_with(&e.id)).map(|e| e.clip_start.unwrap_or(e.start));
            let dl = download_name(&c.cfg.name, when, &name);
            serve_file(p, req, Some(dl), false).await
        }
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
    // Each one is a video encoder running: a few at a time, so no one
    // (or a stuck page asking again and again) can tie up the computer.
    let Ok(permit) = CONVERSIONS.clone().try_acquire_owned() else {
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "too many videos are being converted right now — try again in a moment",
        );
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
        (tokio_util::io::ReaderStream::new(stdout), child, permit),
        |(mut r, c, p)| async move {
            use futures_util::StreamExt;
            r.next().await.map(|x| (x, (r, c, p)))
        },
    );
    Response::builder()
        .header(header::CONTENT_TYPE, "video/mp4")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from_stream(stream))
        .unwrap()
}

/// Videos being converted for the browser at once.
static CONVERSIONS: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(6)));

#[derive(Deserialize)]
struct TimelapseQuery {
    camera: String,
    from: DateTime<chrono::FixedOffset>,
    to: DateTime<chrono::FixedOffset>,
    /// Pictures per second in the video (how fast it goes).
    #[serde(default)]
    fps: Option<u32>,
}

/// A time-lapse video (MP4) of any stretch of time, made from the
/// time-lapse pictures already kept: a whole day takes seconds to make.
async fn timelapse_export(
    State(m): App,
    user: Option<axum::Extension<CurrentUser>>,
    Query(q): Query<TimelapseQuery>,
) -> Response {
    if visible(&user).is_some_and(|v| !v.contains(&q.camera)) {
        return err(StatusCode::FORBIDDEN, "your account can't see this camera");
    }
    let c = camera!(m, q.camera);
    let (from, to) = (q.from.with_timezone(&Local), q.to.with_timezone(&Local));
    if to <= from || (to - from).num_days() > 31 {
        return err(StatusCode::BAD_REQUEST, "choose up to 31 days");
    }
    let pics = crate::picturelog::between(&c.dir, from.timestamp_millis(), to.timestamp_millis());
    if pics.len() < 2 {
        return err(StatusCode::NOT_FOUND, "no time-lapse pictures for that time (time-lapse previews are kept as the camera records)");
    }
    let fps = q.fps.unwrap_or(30).clamp(1, 120);
    let Ok(_permit) = CONVERSIONS.clone().try_acquire_owned() else {
        return err(StatusCode::SERVICE_UNAVAILABLE, "too many videos are being made right now — try again in a moment");
    };
    let ffmpeg_path = m.config().server.ffmpeg_path.clone();
    let child = crate::load::cmd(&ffmpeg_path)
        .args(["-hide_banner", "-loglevel", "error", "-f", "image2pipe", "-framerate", &fps.to_string(), "-c:v", "mjpeg", "-i", "pipe:0"])
        // Even sizes for H.264; the pictures may differ in size over time.
        .args(["-vf", "scale=trunc(iw/2)*2:trunc(ih/2)*2,format=yuv420p", "-c:v", "libx264", "-preset", "veryfast", "-crf", "23"])
        .args(["-f", "mp4", "-movflags", "frag_keyframe+empty_moov+default_base_moof", "pipe:1"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let mut stdin = child.stdin.take().unwrap();
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        for p in pics {
            if let Ok(b) = tokio::fs::read(&p).await
                && stdin.write_all(&b).await.is_err()
            {
                break;
            }
        }
        let _ = stdin.shutdown().await;
    });
    let stdout = child.stdout.take().unwrap();
    let stream = futures_util::stream::unfold(
        (tokio_util::io::ReaderStream::new(stdout), child, _permit),
        |(mut r, c, p)| async move {
            use futures_util::StreamExt;
            r.next().await.map(|x| (x, (r, c, p)))
        },
    );
    let name = format!("{} time-lapse {} to {}.mp4", file_safe(&c.cfg.name), from.format("%Y-%m-%d %H-%M"), to.format("%Y-%m-%d %H-%M"));
    let mut res = Response::builder()
        .header(header::CONTENT_TYPE, "video/mp4")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from_stream(stream))
        .unwrap();
    if let Ok(v) = HeaderValue::from_str(&attachment(&name)) {
        res.headers_mut().insert(header::CONTENT_DISPOSITION, v);
    }
    res
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

/// The preview's description is of the current kind (pictures stored in
/// their own shape, one every `interval` seconds as set): otherwise it's
/// made again.
fn sprite_meta_current(meta: &std::path::Path, interval: u32) -> bool {
    std::fs::read(meta)
        .ok()
        .and_then(|b| serde_json::from_slice::<ffmpeg::SpriteInfo>(&b).ok())
        // Made at an earlier picture interval: still used (only new footage
        // follows a changed setting, unless it's remade on purpose).
        .is_some_and(|i| i.aspect > 0.0 && i.interval > 0.0 && interval > 0)
}

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
    // Previews from before pictures were stored in their own shape are made again.
    let every = m.config().storage.timelapse_seconds;
    let load = || -> Option<ffmpeg::SpriteInfo> {
        sprite_meta_current(&meta, every)
            .then(|| serde_json::from_slice(&std::fs::read(&meta).ok()?).ok())
            .flatten()
    };
    if fresh(&first, &src)
        && let Some(i) = load()
    {
        return Ok((prefix, i));
    }
    // The file still being recorded: show the preview made a minute or
    // two ago straight away; the background job keeps it up to date.
    let growing = std::fs::metadata(&src)
        .and_then(|m| m.modified())
        .is_ok_and(|t| t.elapsed().unwrap_or_default() < Duration::from_secs(120));
    if growing
        && let Some(i) = load()
    {
        return Ok((prefix, i));
    }
    let _p = THUMB_JOBS.acquire().await;
    // Made while waiting (another request for the same file), or a growing
    // file's preview refreshed in the last few seconds: use that.
    let just_made = std::fs::metadata(&meta)
        .and_then(|m| m.modified())
        .is_ok_and(|t| t.elapsed().unwrap_or_default() < Duration::from_secs(20));
    if (fresh(&first, &src) || (growing && just_made))
        && let Some(i) = load()
    {
        return Ok((prefix, i));
    }
    std::fs::create_dir_all(&dir)?;
    // Made under a temporary name and then moved into place page by page,
    // so the older preview keeps working while this one is made (a page
    // holds the same moments in the same places, only more of them).
    let before = load();
    let staging = dir.join(format!("{name}.sprite-new"));
    let page = |p: &std::path::Path, n: u32| PathBuf::from(format!("{}-{n:03}.jpg", p.display()));
    let cfg = m.config();
    let hw = ffmpeg::decode_args(&cfg.hardware, false);
    // Put together from the pictures kept as it was recorded (no decoding;
    // see picturelog); otherwise from the file.
    let from_log = match (kind, storage::parse_start(name)) {
        ("rec" | "tl", Some(start)) => {
            let end = std::fs::metadata(&src).and_then(|x| x.modified()).ok().map(DateTime::<Local>::from);
            let aspect = match before.as_ref().filter(|b| b.aspect > 0.0) {
                Some(b) => b.aspect,
                None => ffmpeg::display_aspect(&cfg.server.ffmpeg_path, &src).await.unwrap_or(16.0 / 9.0),
            };
            let (cam_dir, staging2, every) = (c.dir.clone(), staging.clone(), cfg.storage.timelapse_seconds as f64);
            match end {
                Some(end) if end > start => tokio::task::spawn_blocking(move || crate::picturelog::sprite(&cam_dir, start, end, every, aspect, &staging2))
                    .await
                    .ok()
                    .flatten(),
                _ => None,
            }
        }
        _ => None,
    };
    let made = match from_log {
        Some(i) => Ok(i),
        None => ffmpeg::sprite(&cfg.server.ffmpeg_path, &src, &staging, cfg.storage.timelapse_seconds as f64, &hw).await,
    };
    let info = match (made, before) {
        // Fewer pictures than before (the end of a file being written
        // couldn't be read this time), or none at all: keep the older one.
        (Ok(i), Some(b)) if b.count > i.count => {
            for n in 0..i.pages {
                let _ = std::fs::remove_file(page(&staging, n));
            }
            return Ok((prefix, b));
        }
        (Err(_), Some(b)) => return Ok((prefix, b)),
        (Err(e), None) => return Err(e),
        (Ok(i), _) => i,
    };
    for n in 0..info.pages {
        std::fs::rename(page(&staging, n), page(&prefix, n))?;
    }
    // What the pages hold (how many pictures, where): without it the
    // preview isn't known to be there, and would be made again each time.
    std::fs::write(&meta, serde_json::to_vec(&info)?)?;
    // Pages beyond the new end (from an older, longer version).
    if let Ok(rd) = std::fs::read_dir(&dir) {
        let start = format!("{name}.sprite-");
        for e in rd.flatten() {
            let f = e.file_name().to_string_lossy().to_string();
            if let Some(n) = f
                .strip_prefix(&start)
                .and_then(|r| r.strip_suffix(".jpg"))
                .and_then(|n| n.parse::<u32>().ok())
                && n >= info.pages
            {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    Ok((prefix, info))
}

/// "Rebuild previews" (System → Tasks): run the job now, even if
/// preparing previews in advance is off.
static PREBUILD_NOW: tokio::sync::Notify = tokio::sync::Notify::const_new();
static PREBUILD_FORCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A file's time-lapse preview is made and up to date.
fn preview_ready(c: &crate::manager::CameraRuntime, kind: &str, name: &str, every: u32) -> bool {
    let Ok(src) = media_path(c, kind, name) else {
        return false;
    };
    let first = src
        .parent()
        .unwrap()
        .join(".thumbs")
        .join(format!("{name}.sprite-000.jpg"));
    let meta = first.with_file_name(format!("{name}.sprite.json"));
    fresh(&first, &src) && sprite_meta_current(&meta, every)
}

type PreviewTodo = (
    Arc<crate::manager::CameraRuntime>,
    &'static str,
    String,
    DateTime<Local>,
    DateTime<Local>,
);

/// Files between `from` and `to` whose time-lapse preview is missing or out
/// of date, newest first: the light timeline copies, the recordings of any
/// time without a copy (e.g. while the copy was reconnecting) and alert clips.
fn missing_previews(m: &Manager, from: DateTime<Local>, to: DateTime<Local>) -> Vec<PreviewTodo> {
    let mut todo: Vec<PreviewTodo> = Vec::new();
    let now = Local::now();
    for c in m.cameras() {
        let copies: Vec<_> = library::timeline_recordings(&c)
            .into_iter()
            .filter(|r| r.end >= from && r.start < to)
            .collect();
        for r in &copies {
            todo.push((c.clone(), "tl", r.name.clone(), r.start, r.end));
        }
        for r in library::recordings(&c)
            .iter()
            .filter(|r| r.end >= from && r.start < to)
        {
            // Skip the file still being written (it keeps growing).
            if (now - r.end).num_seconds() < 30 {
                continue;
            }
            let covered = copies.iter().any(|t| {
                t.start <= r.start + chrono::Duration::seconds(5)
                    && t.end >= r.end - chrono::Duration::seconds(5)
            });
            if !covered {
                todo.push((c.clone(), "rec", r.name.clone(), r.start, r.end));
            }
        }
        for e in c.events.between(from, to) {
            if let Some(clip) = e.clip {
                todo.push((c.clone(), "clip", clip, e.start, e.end));
            }
        }
    }
    let every = m.config().storage.timelapse_seconds;
    todo.retain(|(c, kind, name, ..)| {
        media_path(c, kind, name).is_ok() && !preview_ready(c, kind, name, every)
    });
    todo.sort_by(|a, b| b.3.cmp(&a.3));
    todo
}

/// Background job: keep the time-lapse previews of the last 24 hours ready
/// (checked every minute), and go back over all older footage every ten
/// minutes to make any preview that's missing (GAZE was busy or off, a file
/// couldn't be read at the time …), a batch at a time while the computer
/// is quiet. A file that fails is tried again later (a few times).
pub async fn prebuild_timelapse(m: Arc<Manager>) {
    // Let the cameras connect first.
    tokio::select! {
        _ = tokio::time::sleep(std::time::Duration::from_secs(15)) => {}
        _ = PREBUILD_NOW.notified() => {}
    }
    let mut announced = false;
    // Files that failed: (tries, when).
    let mut failed: std::collections::HashMap<String, (u32, std::time::Instant)> =
        std::collections::HashMap::new();
    let mut last_sweep: Option<std::time::Instant> = None;
    loop {
        let mut built = 0usize;
        // Switched off (Settings → Recording & storage): nothing is made.
        if !crate::picturelog::enabled() {
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(60)) => {}
                _ = PREBUILD_NOW.notified() => {}
            }
            continue;
        }
        let force = PREBUILD_FORCE.swap(false, std::sync::atomic::Ordering::SeqCst);
        {
            let now = Local::now();
            let day_ago = now - chrono::Duration::hours(24);
            // Not again too soon after failing: 5 minutes, then longer each
            // time (up to 6 hours); older footage at most 3 tries.
            let retry_ok =
                |failed: &std::collections::HashMap<String, (u32, std::time::Instant)>,
                 t: &PreviewTodo,
                 old: bool| {
                    failed
                        .get(&format!("{}/{}", t.0.cfg.id, t.2))
                        .is_none_or(|(n, at)| {
                            (!old || *n < 3)
                                && at.elapsed()
                                    >= std::time::Duration::from_secs(
                                        (300u64 << (*n - 1).min(6)).min(6 * 3600),
                                    )
                        })
                };
            let recent: Vec<PreviewTodo> =
                missing_previews(&m, day_ago, now + chrono::Duration::minutes(5))
                    .into_iter()
                    .filter(|t| force || retry_ok(&failed, t, false))
                    .collect();
            let sweep = force
                || recent.is_empty()
                    && last_sweep
                        .is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(600));
            let mut older: Vec<PreviewTodo> = if sweep {
                last_sweep = Some(std::time::Instant::now());
                missing_previews(&m, now - chrono::Duration::days(3650), day_ago)
                    .into_iter()
                    .filter(|t| force || retry_ok(&failed, t, true))
                    .collect()
            } else {
                Vec::new()
            };
            // A batch at a time: new footage shouldn't wait behind a long backlog.
            let backlog = older.len();
            if !force {
                older.truncate(30);
            }
            for (part, list, title) in [
                (0, recent, "Time-lapse previews (last 24 hours)".to_string()),
                (
                    1,
                    older,
                    format!(
                        "Time-lapse previews — catching up on older footage ({backlog} missing)"
                    ),
                ),
            ] {
                if list.is_empty() {
                    continue;
                }
                let j = crate::jobs::start("previews", &title);
                j.total(list.len() as u64);
                for t in list {
                    let (c, kind, name, start, end) = &t;
                    j.waiting();
                    if part == 0 {
                        // Room to spare first, but never held up for long: the
                        // newest footage should always be ready to scrub.
                        crate::load::wait_for_cpu(90, std::time::Duration::from_secs(60)).await;
                    } else if !force {
                        // Older footage only while the computer is quiet.
                        crate::load::wait_for_cpu(70, std::time::Duration::from_secs(600)).await;
                        if crate::load::cpu() > 85 {
                            break;
                        }
                    }
                    j.pause_point().await;
                    j.step(format!(
                        "{} — {} {}–{}",
                        c.cfg.name,
                        match *kind {
                            "tl" => "timeline copy",
                            "clip" => "alert clip",
                            _ => "recording",
                        },
                        if part == 0 {
                            start.format("%H:%M:%S").to_string()
                        } else {
                            start.format("%d/%m %H:%M:%S").to_string()
                        },
                        end.format("%H:%M:%S")
                    ));
                    // Let people scrubbing the timeline go first — the preview
                    // may be made for them meanwhile: then it counts as done.
                    let every = m.config().storage.timelapse_seconds;
                    let ready = || preview_ready(c, kind, name, every);
                    while THUMB_JOBS.available_permits() < 3 && !ready() {
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    }
                    if ready() {
                        built += 1;
                        j.done_one();
                        continue;
                    }
                    if !announced {
                        tracing::info!("preparing time-lapse previews in the background");
                        announced = true;
                    }
                    let key = format!("{}/{}", c.cfg.id, name);
                    match ensure_sprite(&m, c, kind, name).await {
                        Ok(_) => {
                            failed.remove(&key);
                            built += 1;
                        }
                        Err(e) => {
                            let n = failed.get(&key).map(|x| x.0).unwrap_or(0) + 1;
                            if n == 1 {
                                tracing::debug!("time-lapse preview of {name} failed: {e:#}");
                            }
                            failed.insert(key, (n, std::time::Instant::now()));
                        }
                    }
                    j.done_one();
                }
            }
            if failed.len() > 20_000 {
                failed.clear();
            }
        }
        if built > 0 {
            tracing::info!("time-lapse previews ready ({built} file(s) prepared)");
        }
        // Every minute, so new recordings get their previews right away.
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(60)) => {}
            _ = PREBUILD_NOW.notified() => {}
        }
    }
}

// ---------- updates ----------

async fn update_status() -> Response {
    Json(crate::update::status()).into_response()
}

async fn update_check(State(m): App) -> Response {
    match crate::update::check(&crate::update::repo(&m.config().update)).await {
        Ok(s) => Json(s).into_response(),
        Err(e) => bad(e),
    }
}

/// Download and install the newest version, then restart (in the
/// background; the page follows along with /api/update).
async fn update_install() -> Response {
    if !crate::update::status().available {
        return err(StatusCode::BAD_REQUEST, "no update available");
    }
    tokio::spawn(async {
        if let Err(e) = crate::update::install().await {
            tracing::warn!("update failed: {e:#}");
        }
    });
    Json(json!({"ok": true})).into_response()
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
        actions: None,
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

/// Settings → Tasks → "What runs where": every part that can use the
/// graphics card, and where it runs now (and why, when it's the processor).
async fn where_work_runs(State(m): App) -> Response {
    let cfg = m.config();
    let hw = &cfg.hardware;
    let gpu_dec = hw.hwaccel != "none";
    let no_dec = if hw.auto {
        "no GPU video decoder worked on this computer (checked at start-up)"
    } else {
        "GPU video decoding is off in Settings → GPU acceleration"
    };
    let cams: Vec<_> = m
        .cameras()
        .iter()
        .map(|c| {
            let live_main = c.live_main.on_gpu();
            let live_sub = c.cfg.has_sub() && c.live_sub.on_gpu();
            let det = c.detector().map(|d| d.status().gpu_decoding);
            json!({
                "name": c.cfg.name,
                "live_hd": live_main,
                "live_small": if c.cfg.has_sub() { Some(live_sub) } else { None },
                "detection": det,
            })
        })
        .collect();
    Json(json!({
        "ai_device": crate::infer::status(),
        "ai_wanted": crate::infer::use_gpu(),
        "models": crate::infer::placed(),
        "video": {
            "auto": hw.auto,
            "decoder": hw.hwaccel,
            "encoder": hw.resolved_encoder(),
            "small_on_gpu": hw.gpu_substreams,
            "why_not": if gpu_dec { None } else { Some(no_dec) },
            "balance": hw.balance,
            "moved_to_cpu": crate::load::moved_to_cpu(),
        },
        "cameras": cams,
    }))
    .into_response()
}

async fn jobs_list(State(m): App) -> Response {
    // Alerts still waiting for the background AI.
    let cfg = m.config();
    let now = Local::now();
    let mut identify = 0;
    for c in m.cameras() {
        for e in c.events.between(now - chrono::Duration::days(7), now) {
            if (cfg.ai.faces || cfg.ai.plates)
                && !e.identified
                && e.start > now - chrono::Duration::days(7)
                && e.boxes.iter().any(|b| {
                    ["person", "vehicle", "car", "truck", "bus", "motorcycle"].contains(&b.label.as_str())
                })
            {
                identify += 1;
            }
        }
    }
    Json(json!({
        "jobs": crate::jobs::list(),
        "waiting": {"identify": identify},
        "paused": crate::jobs::paused(),
    }))
    .into_response()
}

#[derive(Deserialize)]
struct PauseReq {
    /// "ai", "previews", "cloud", "maintenance", or "all".
    kind: String,
    paused: bool,
}

/// Pause a kind of background work to lighten the load, until resumed.
async fn jobs_pause(Json(r): Json<PauseReq>) -> Response {
    if r.kind == "all" {
        for k in crate::jobs::PAUSABLE {
            crate::jobs::set_paused(k, r.paused);
        }
    } else if !crate::jobs::set_paused(&r.kind, r.paused) {
        return err(StatusCode::BAD_REQUEST, "this kind of task can't be paused");
    }
    tracing::info!(
        "background work {:?} {}",
        r.kind,
        if r.paused { "paused" } else { "resumed" }
    );
    Json(json!({ "paused": crate::jobs::paused() })).into_response()
}

/// Look for faces and number plates in the alerts of the last `days` again,
/// e.g. after teaching GAZE new people.
async fn reidentify(State(m): App, Json(r): Json<RefreshPictures>) -> Response {
    let cfg = m.config();
    if !(cfg.ai.faces || cfg.ai.plates) {
        return err(
            StatusCode::BAD_REQUEST,
            "Faces & plates are off (Settings → People & vehicles)",
        );
    }
    let since = Local::now() - chrono::Duration::days(r.days.clamp(1, 7));
    let mut n = 0;
    for c in m.cameras() {
        for mut e in c.events.between(since, Local::now()) {
            if !e.identified {
                continue;
            }
            for f in e.faces.drain(..) {
                let _ = std::fs::remove_file(c.events.dir().join(&f.file));
            }
            e.notes.retain(|x| {
                !(x.starts_with("Recognized ")
                    || x == "Unknown face"
                    || x.contains(" plate ")
                    || x.starts_with("Plate ")
                    || x.starts_with("Looks like "))
            });
            e.plates.clear();
            e.identified = false;
            c.events.save(e);
            n += 1;
        }
    }
    Json(json!({"ok": true, "message": format!("{n} alert(s) will be checked for faces and plates again (see the list below)")})).into_response()
}

/// Throw away all scrub / time-lapse previews and make those of the last
/// 24 hours again (older ones are made when looked at).
async fn rebuild_previews(State(m): App) -> Response {
    if !crate::picturelog::enabled() {
        return err(
            StatusCode::BAD_REQUEST,
            "time-lapse previews are switched off",
        );
    }
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

/// Delete all saved footage (for testing): every recording, timeline copy,
/// alert with its clip and pictures, and the cached previews, for every
/// camera. Settings, cameras, known people and vehicles stay. The file each
/// camera is writing right now is left alone (it's still open).
async fn delete_all_footage(State(m): App) -> Response {
    let res = tokio::task::spawn_blocking(move || {
        let (mut files, mut bytes, mut alerts) = (0u64, 0u64, 0usize);
        let busy = |p: &std::path::Path| {
            std::fs::metadata(p)
                .and_then(|m| m.modified())
                .is_ok_and(|t| t.elapsed().unwrap_or_default() < Duration::from_secs(20))
        };
        // Every camera's alerts, also cameras that aren't running.
        for store in m.event_stores().values() {
            alerts += store.remove_where(|_| true);
        }
        for c in m.cameras() {
            let _ = std::fs::remove_dir_all(crate::talkmix::talk_dir(&c.dir));
            for dir in [
                c.dir.clone(),
                crate::storage::timeline_dir(&c.dir),
                c.events.dir().to_path_buf(),
            ] {
                let _ = std::fs::remove_dir_all(dir.join(".thumbs"));
                let Ok(rd) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for e in rd.flatten() {
                    let p = e.path();
                    let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("");
                    // Pictures always; video files unless being written right now.
                    if !p.is_file()
                        || !["mp4", "jpg", "ts", "part"].contains(&ext)
                        || (ext != "jpg" && busy(&p))
                    {
                        continue;
                    }
                    let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                    if std::fs::remove_file(&p).is_ok() {
                        files += 1;
                        bytes += size;
                    }
                }
            }
        }
        (files, bytes, alerts)
    })
    .await;
    match res {
        Ok((files, bytes, alerts)) => {
            tracing::warn!(
                "all saved footage deleted from the toolbar: {files} file(s), {:.1} GB, {alerts} alert(s)",
                bytes as f64 / 1e9
            );
            Json(json!({"ok": true, "message": format!(
                "Deleted {files} file(s) ({:.1} GB) and {alerts} alert(s). Recording carries on.",
                bytes as f64 / 1e9
            )}))
            .into_response()
        }
        Err(e) => bad(anyhow::anyhow!(e)),
    }
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

/// What's new in the running version: its section of the changelog
/// (built into GAZE), shown once after an update.
async fn whats_new() -> Response {
    const LOG: &str = include_str!("../CHANGELOG.md");
    let version = env!("CARGO_PKG_VERSION");
    let head = format!("## {version} ");
    let text = LOG
        .split("\n## ")
        .map(|s| format!("## {s}"))
        .find(|s| s.starts_with(&head) || s.starts_with(&format!("#{head}")))
        .map(|s| {
            s.lines()
                .skip(1)
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_string()
        })
        .unwrap_or_default();
    Json(json!({ "version": version, "text": text })).into_response()
}

#[derive(Deserialize)]
struct ReadyReq {
    camera: String,
    /// (kind, file name) of each file.
    files: Vec<(String, String)>,
}

/// Which of these files already have their time-lapse preview made (and
/// its details), in one go — nothing is made here. The timeline asks this
/// when it opens, so previews made before show at once after a reload.
async fn sprites_ready(
    State(m): App,
    user: Option<axum::Extension<CurrentUser>>,
    Json(r): Json<ReadyReq>,
) -> Response {
    if visible(&user).is_some_and(|v| !v.contains(&r.camera)) {
        return err(StatusCode::FORBIDDEN, "your account can't see this camera");
    }
    if !crate::picturelog::enabled() {
        return Json(json!({})).into_response();
    }
    let c = camera!(m, r.camera);
    let every = m.config().storage.timelapse_seconds;
    let mut out = serde_json::Map::new();
    for (kind, name) in r.files.into_iter().take(2000) {
        if !preview_ready(&c, &kind, &name, every) {
            continue;
        }
        let Ok(src) = media_path(&c, &kind, &name) else {
            continue;
        };
        let meta = src
            .parent()
            .unwrap()
            .join(".thumbs")
            .join(format!("{name}.sprite.json"));
        let Some(info) = std::fs::read(&meta)
            .ok()
            .and_then(|b| serde_json::from_slice::<ffmpeg::SpriteInfo>(&b).ok())
        else {
            continue;
        };
        out.insert(
            format!("{kind}/{name}"),
            json!({"info": info, "url": format!("media/{}/sprite/{kind}/{name}?v={}&page=", c.cfg.id, info.version)}),
        );
    }
    Json(serde_json::Value::Object(out)).into_response()
}

async fn sprite_info(
    State(m): App,
    Path((id, kind, name)): Path<(String, String, String)>,
) -> Response {
    if !crate::picturelog::enabled() {
        return err(
            StatusCode::NOT_FOUND,
            "time-lapse previews are switched off",
        );
    }
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
    if !crate::picturelog::enabled() {
        return err(
            StatusCode::NOT_FOUND,
            "time-lapse previews are switched off",
        );
    }
    let c = camera!(m, id);
    // A page already made is served as it is (pages are only ever written
    // over with more pictures); making previews is for the info request.
    if let Ok(src) = media_path(&c, &kind, &name) {
        let file = src
            .parent()
            .unwrap()
            .join(".thumbs")
            .join(format!("{name}.sprite-{:03}.jpg", q.page));
        if file.is_file() {
            let mut res = serve_file(file, req, None, false).await;
            res.headers_mut().insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("max-age=86400"),
            );
            return res;
        }
    }
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
    /// At most this many pictures a second (a camera's private link).
    #[serde(default)]
    fps: Option<f32>,
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
    let every = q.fps.filter(|f| *f > 0.0).map(|f| Duration::from_secs_f32(1.0 / f.clamp(0.2, 30.0)));
    let (tx, mut out) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(2);
    tokio::spawn(async move {
        let mut last: Option<Instant> = None;
        while rx.changed().await.is_ok() {
            let Some((_, frame)) = rx.borrow_and_update().clone() else {
                continue;
            };
            if let (Some(e), Some(l)) = (every, last)
                && l.elapsed() < e
            {
                continue;
            }
            last = Some(Instant::now());
            if tx.send(Ok(multipart_part(&frame))).await.is_err() {
                break;
            }
        }
    });
    mjpeg_response(futures_util::stream::poll_fn(move |cx| out.poll_recv(cx)))
}

/// The camera's sound, live, as an MP3 stream for an <audio> element (the
/// live picture comes as JPEGs, which carry no sound). Taken from the shared
/// camera connection, which already carries the audio; ends when the page
/// stops listening.
/// What to tell the user when a camera sends no sound.
const NO_SOUND: &str = "The camera isn't sending sound. In the camera's own settings, turn on audio for this stream (on Hikvision: Video/Audio → Video → Stream Type \"Video&Audio\", for both the main and sub-stream), then try again.";

async fn live_audio(State(m): App, Path(id): Path<String>) -> Response {
    let c = camera!(m, id);
    let ffmpeg = m.config().server.ffmpeg_path;
    // The main stream, unless only the sub-stream is known to carry sound.
    let has = |a: Option<String>| a.is_some_and(|a| !a.is_empty());
    let relay = if c.relay_main.audio().as_deref() == Some("") && has(c.relay_sub.audio()) {
        c.relay_sub.clone()
    } else {
        c.relay_main.clone()
    };
    if relay.audio().as_deref() == Some("")
        && !has(c.relay_sub.audio())
        && c.relay_sub.audio().is_some()
    {
        return err(StatusCode::NOT_FOUND, NO_SOUND);
    }
    let rx = relay.subscribe_now();
    let child = crate::load::cmd(&ffmpeg)
        .args(["-hide_banner", "-loglevel", "error"])
        .args(crate::relay::pipe_input_args_low_delay())
        .args([
            "-vn",
            "-map",
            "0:a:0",
            "-c:a",
            "libmp3lame",
            "-b:a",
            "64k",
            "-ac",
            "1",
            "-ar",
            "22050",
            "-reservoir",
            "0",
        ])
        .args(["-flush_packets", "1", "-f", "mp3", "pipe:1"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("can't start ffmpeg: {e}"),
            );
        }
    };
    let stop = tokio_util::sync::CancellationToken::new();
    crate::relay::feed(rx, child.stdin.take().unwrap(), stop.clone());
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    use tokio::io::AsyncReadExt;
    // Wait for the first sound before answering, so a camera without sound
    // gets a message the page can show instead of an empty file.
    let mut buf = vec![0u8; 4096];
    let first =
        tokio::time::timeout(std::time::Duration::from_secs(15), stdout.read(&mut buf)).await;
    let n = match first {
        Ok(Ok(n)) if n > 0 => n,
        other => {
            stop.cancel();
            let _ = child.kill().await;
            let mut e = String::new();
            let _ = tokio::time::timeout(
                std::time::Duration::from_millis(500),
                stderr.read_to_string(&mut e),
            )
            .await;
            let audio = relay.audio();
            let msg = if audio.as_deref() == Some("") || e.contains("matches no streams") {
                NO_SOUND.to_string()
            } else if other.is_err() {
                "The camera didn't send any sound within 15 seconds.".to_string()
            } else if let Some(l) = e.lines().rev().find(|l| !l.trim().is_empty()) {
                format!(
                    "ffmpeg could not convert the camera's sound ({}): {}",
                    audio.unwrap_or_default(),
                    l.trim()
                )
            } else {
                "The camera's stream isn't available right now.".to_string()
            };
            tracing::warn!("[{}] live sound: {msg}", c.cfg.name);
            return err(StatusCode::NOT_FOUND, msg);
        }
    };
    let (tx, mut out) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(8);
    let _ = tx.try_send(Ok(Bytes::copy_from_slice(&buf[..n])));
    tokio::spawn(async move {
        // Keep stderr drained so ffmpeg never blocks on it.
        tokio::spawn(async move {
            let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
        });
        loop {
            tokio::select! {
                n = stdout.read(&mut buf) => match n {
                    Ok(0) | Err(_) => break,
                    Ok(n) => if tx.send(Ok(Bytes::copy_from_slice(&buf[..n]))).await.is_err() { break },
                },
                _ = tx.closed() => break,
            }
        }
        stop.cancel();
        let _ = child.kill().await;
    });
    (
        [
            (header::CONTENT_TYPE, "audio/mpeg"),
            (header::CACHE_CONTROL, "no-cache, no-store"),
        ],
        axum::body::Body::from_stream(futures_util::stream::poll_fn(move |cx| out.poll_recv(cx))),
    )
        .into_response()
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
            let name = attachment(&download_name(&c.cfg.name, Some(Local::now()), "snapshot.jpg"));
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

/// Send a made-up alert to a webhook, to try it.
async fn webhook_test(State(m): App, Json(h): Json<crate::config::Webhook>) -> Response {
    let cam = m.config().cameras.first().map(|c| (c.id.clone(), c.name.clone())).unwrap_or_default();
    let body = json!({"event": "test", "id": "test", "camera": cam.0, "camera_name": cam.1, "start": Local::now(), "end": Local::now(),
        "labels": [{"label": "person", "score": 0.9}], "what": "person", "notes": ["A test from GAZE"], "plates": [], "faces": [], "mode": m.config().modes.current});
    match crate::hooks::send(&h, &body).await {
        Ok(()) => Json(json!({"ok": true, "message": "Sent — the address accepted it"})).into_response(),
        Err(e) => bad(e),
    }
}

#[derive(Deserialize)]
struct SnoozeLink {
    c: String,
    m: i64,
    e: i64,
    s: String,
}

/// A snooze button in a notification: signed, so it works without signing in.
async fn snooze_link(State(m): App, Query(q): Query<SnoozeLink>) -> Response {
    if !crate::notify::snooze_ok(&m.data_dir, &q.c, q.m, q.e, &q.s) || m.camera(&q.c).is_none() {
        return err(StatusCode::FORBIDDEN, "this snooze link isn't valid (or is more than 2 days old)");
    }
    crate::notify::snooze(&q.c, q.m);
    let name = m.camera(&q.c).map(|c| c.cfg.name.clone()).unwrap_or_default();
    tracing::info!("[{name}] notifications snoozed for {} min (from a notification)", q.m);
    Html(format!("<!doctype html><meta name=viewport content=\"width=device-width\"><body style=\"font:16px system-ui;background:#0f1218;color:#e6e9f0;text-align:center;padding:40px\"><h2>🔕 {} snoozed</h2><p>No notifications from this camera until {}. It still records and keeps alerts.</p>",
        html_escape(&name), crate::notify::snoozed_until(&q.c).map(|t| t.format("%H:%M on %a %-d %b").to_string()).unwrap_or_default())).into_response()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[derive(Deserialize)]
struct SnoozeReq {
    /// 0 = notify again now; -1 = until tomorrow morning.
    minutes: i64,
}

async fn snooze_camera(State(m): App, Path(id): Path<String>, Json(r): Json<SnoozeReq>) -> Response {
    let c = camera!(m, id);
    let minutes = if r.minutes < 0 { crate::notify::until_morning() } else { r.minutes.min(7 * 24 * 60) };
    crate::notify::snooze(&c.cfg.id, minutes);
    Json(json!({"ok": true, "until": crate::notify::snoozed_until(&c.cfg.id)})).into_response()
}

/// Safe mode: the settings file as it is, to fix it on the page.
async fn safe_config_get(State(m): App) -> Response {
    let text = std::fs::read_to_string(&m.cfg_path).unwrap_or_default();
    Json(json!({ "text": text, "problem": Config::safe_mode(), "path": m.cfg_path.display().to_string() })).into_response()
}

#[derive(Deserialize)]
struct SafeText {
    text: String,
}

/// Save a corrected settings file (only if GAZE can read it) and restart.
async fn safe_config_save(State(m): App, Json(r): Json<SafeText>) -> Response {
    if let Err(e) = Config::from_toml(&r.text) {
        return err(StatusCode::BAD_REQUEST, format!("still not right: {e:#}"));
    }
    let _ = std::fs::copy(&m.cfg_path, m.cfg_path.with_extension("toml.broken"));
    if let Err(e) = std::fs::write(&m.cfg_path, &r.text) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    tracing::info!("settings file fixed from the web page; restarting");
    restart_soon();
    Json(json!({ "ok": true, "message": "Saved — GAZE is restarting with the fixed settings" })).into_response()
}

/// Put back the settings from the last time GAZE started fine, and restart.
async fn safe_config_restore(State(m): App) -> Response {
    let good = m.cfg_path.with_extension("toml.good");
    let Ok(text) = std::fs::read_to_string(&good) else {
        return err(StatusCode::NOT_FOUND, "there's no last good copy");
    };
    if let Err(e) = Config::from_toml(&text) {
        return err(StatusCode::BAD_REQUEST, format!("the last good copy can't be read either: {e:#}"));
    }
    let _ = std::fs::copy(&m.cfg_path, m.cfg_path.with_extension("toml.broken"));
    if let Err(e) = std::fs::write(&m.cfg_path, text) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    tracing::info!("settings restored from the last good copy; restarting");
    restart_soon();
    Json(json!({ "ok": true, "message": "Restored — GAZE is restarting" })).into_response()
}

fn restart_soon() {
    tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(800)).await;
        if !crate::update::restart_now() {
            tracing::warn!("restart GAZE to use the fixed settings");
        }
    });
}

#[derive(Deserialize)]
struct ModeReq {
    mode: String,
}

/// Switch Home / Away / Night.
async fn set_mode(State(m): App, user: Option<axum::Extension<CurrentUser>>, Json(r): Json<ModeReq>) -> Response {
    match m.set_mode(&r.mode).await {
        Ok(()) => {
            let who = user.map(|u| u.0.0.username.clone()).unwrap_or_default();
            tracing::info!("mode: {} ({})", r.mode, if who.is_empty() { "API" } else { &who });
            Json(json!({ "ok": true, "mode": r.mode })).into_response()
        }
        Err(e) => bad(e),
    }
}

async fn put_modes(State(m): App, Json(c): Json<crate::config::ModesConfig>) -> Response {
    match m.update_modes(c).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => bad(e),
    }
}

/// "More things to watch for": your own model's classes first, if you use one.
fn ai_more(m: &Manager) -> Vec<String> {
    let mut v: Vec<String> = crate::ai::more_objects().into_iter().map(String::from).collect();
    if m.config().ai.builtin_model == "custom" {
        for l in m.ai.get().labels().into_iter().rev() {
            if !v.contains(&l) {
                v.insert(0, l);
            }
        }
    }
    v
}

/// The other GAZE servers and their cameras. Only for accounts that see
/// every camera here (the others' cameras aren't sorted by account).
async fn remotes_list(State(m): App, user: Option<axum::Extension<CurrentUser>>) -> Response {
    if visible(&user).is_some() {
        return Json(json!([])).into_response();
    }
    let mut out = Vec::new();
    for (i, r) in m.config().remotes.iter().enumerate() {
        let mut v = crate::remote::cameras(r).await;
        v["i"] = json!(i);
        v["name"] = json!(r.name);
        v["url"] = json!(r.url);
        out.push(v);
    }
    Json(out).into_response()
}

async fn remote_frame(State(m): App, user: Option<axum::Extension<CurrentUser>>, Path((i, cam)): Path<(usize, String)>) -> Response {
    if visible(&user).is_some() {
        return err(StatusCode::FORBIDDEN, "your account can't see other servers' cameras");
    }
    let Some(r) = m.config().remotes.get(i).cloned() else {
        return err(StatusCode::NOT_FOUND, "no such server");
    };
    match crate::remote::frame(&r, &cam).await {
        Ok(b) => ([(header::CONTENT_TYPE, "image/jpeg"), (header::CACHE_CONTROL, "no-store")], b).into_response(),
        Err(e) => err(StatusCode::BAD_GATEWAY, format!("{e:#}")),
    }
}

/// The secure address: on or not, and with which certificate.
async fn https_status(State(m): App) -> Response {
    let s = crate::tls::status();
    Json(json!({ "status": s, "port": m.config().server.https_port, "cert": m.config().server.https_cert })).into_response()
}

/// Get a trusted certificate from Tailscale and use it (after a restart).
async fn https_tailscale(State(m): App) -> Response {
    match crate::tls::from_tailscale(&m.data_dir).await {
        Ok((cert, key, name)) => {
            let sv = m.config().server;
            let up = SettingsUpdate {
                server: Some(crate::manager::ServerUpdate {
                    name: sv.name,
                    listen: sv.listen,
                    ffmpeg_path: sv.ffmpeg_path,
                    https_port: None,
                    https_cert: Some(cert),
                    https_key: Some(key),
                }),
                ..Default::default()
            };
            match m.update_settings(up).await {
                Ok(_) => Json(json!({ "ok": true, "name": name, "message": format!("Certificate for {name} saved. Restart GAZE to use it, then open https://{name}:{}/", m.config().server.https_port) })).into_response(),
                Err(e) => bad(e),
            }
        }
        Err(e) => bad(e),
    }
}

#[derive(Deserialize)]
struct LiveVideoQuery {
    #[serde(default)]
    q: Option<Quality>,
}

/// Live view as real video: the camera's own stream, repackaged (not
/// re-encoded) into small MP4 fragments the browser plays with Media Source
/// Extensions. Smooth at the camera's full frame rate, decoded by the
/// viewer's own graphics hardware, and almost no work for this computer.
/// The first message is the MP4 header; the browser reads the codec from
/// it. A text message {"error": ..} means: use the picture stream instead.
async fn live_video_ws(
    State(m): App,
    Path(id): Path<String>,
    Query(lq): Query<LiveVideoQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    let c = camera!(m, id);
    let main = !matches!(lq.q, Some(Quality::Sub)) || !c.cfg.has_sub();
    let relay = if main { c.relay_main.clone() } else { c.relay_sub.clone() };
    let ffmpeg_path = m.config().server.ffmpeg_path.clone();
    let rotated = c.cfg.rotate % 360 != 0 || !c.cfg.privacy.is_empty() || c.cfg.dewarp.enabled;
    ws.on_upgrade(move |mut socket| async move {
        // Rotation and privacy areas are applied while decoding: those
        // cameras keep the picture stream.
        if rotated {
            let _ = socket.send(Message::Text(json!({"error": "rotated"}).to_string().into())).await;
            return;
        }
        let child = crate::load::cmd(&ffmpeg_path)
            .args(["-hide_banner", "-loglevel", "error", "-fflags", "+genpts"])
            .args(relay.input_args())
            .args(["-map", "0:v:0", "-c:v", "copy", "-tag:v", if relay.info().is_some_and(|i| i.codec.contains("265") || i.codec.contains("hevc")) { "hvc1" } else { "avc1" }, "-an"])
            .args([
                "-f", "mp4", "-movflags", "empty_moov+default_base_moof+frag_keyframe",
                "-frag_duration", "100000", "pipe:1",
            ])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn();
        let Ok(mut child) = child else {
            let _ = socket.send(Message::Text(json!({"error": "ffmpeg"}).to_string().into())).await;
            return;
        };
        let stop = tokio_util::sync::CancellationToken::new();
        let _feed = crate::relay::feed(relay.subscribe(), child.stdin.take().unwrap(), stop.clone());
        let _stop = stop.drop_guard();
        let mut out = child.stdout.take().unwrap();
        let mut buf = vec![0u8; 256 * 1024];
        use tokio::io::AsyncReadExt;
        loop {
            tokio::select! {
                n = out.read(&mut buf) => {
                    let Ok(n) = n else { break };
                    if n == 0 { break; }
                    if socket.send(Message::Binary(bytes::Bytes::copy_from_slice(&buf[..n]))).await.is_err() {
                        break;
                    }
                }
                msg = socket.recv() => {
                    match msg {
                        Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                        _ => {}
                    }
                }
            }
        }
    })
}

#[derive(Deserialize)]
struct OverviewQuery {
    /// Only cameras with something going on.
    #[serde(default)]
    active: bool,
    #[serde(default)]
    w: Option<u32>,
    #[serde(default)]
    fps: Option<f32>,
}

fn overview_size(q: &OverviewQuery) -> (u32, u32) {
    let w = q.w.unwrap_or(1280).clamp(320, 3840) & !1;
    (w, (w * 9 / 16) & !1)
}

/// Every camera in one picture.
async fn overview_jpg(State(m): App, user: Option<axum::Extension<CurrentUser>>, Query(q): Query<OverviewQuery>) -> Response {
    let vis = visible(&user);
    let (w, h) = overview_size(&q);
    let m2 = m.clone();
    match tokio::task::spawn_blocking(move || crate::overview::picture(&m2, vis.as_deref(), q.active, w, h)).await.ok().flatten() {
        Some(jpg) => ([(header::CONTENT_TYPE, "image/jpeg"), (header::CACHE_CONTROL, "no-store")], jpg).into_response(),
        None => err(StatusCode::SERVICE_UNAVAILABLE, "no camera pictures yet"),
    }
}

/// Every camera in one moving picture (MJPEG: plays in any browser, in
/// Home Assistant's generic camera, in VLC).
async fn overview_mjpeg(State(m): App, user: Option<axum::Extension<CurrentUser>>, Query(q): Query<OverviewQuery>) -> Response {
    let vis = visible(&user);
    let (w, h) = overview_size(&q);
    let every = Duration::from_secs_f32(1.0 / q.fps.unwrap_or(2.0).clamp(0.2, 10.0));
    let (tx, mut out) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(2);
    tokio::spawn(async move {
        loop {
            let (m2, v2) = (m.clone(), vis.clone());
            let pic = tokio::task::spawn_blocking(move || crate::overview::picture(&m2, v2.as_deref(), q.active, w, h)).await.ok().flatten();
            if let Some(jpg) = pic
                && tx.send(Ok(multipart_part(&Bytes::from(jpg)))).await.is_err()
            {
                break;
            }
            if tx.is_closed() {
                break;
            }
            tokio::time::sleep(every).await;
        }
    });
    mjpeg_response(futures_util::stream::poll_fn(move |cx| out.poll_recv(cx)))
}

#[derive(Deserialize)]
struct DescribeQuery {
    q: String,
    #[serde(default)]
    days: Option<i64>,
}

/// Alerts found by description ("a red car", "someone with a ladder").
async fn describe_search(State(m): App, user: Option<axum::Extension<CurrentUser>>, Query(q): Query<DescribeQuery>) -> Response {
    let vis = visible(&user);
    match crate::clip::search(&m, vis.as_deref(), &q.q, q.days.unwrap_or(30)).await {
        Ok((hits, looked, closest)) => Json(json!({
            "closest": closest,
            "hits": hits.iter().map(|(c, id, s)| json!({"camera": c, "id": id, "score": s})).collect::<Vec<_>>(),
            "looked": looked,
            "waiting": crate::clip::STATUS.lock().unwrap().waiting,
        }))
        .into_response(),
        Err(e) => bad(e),
    }
}

async fn describe_status(State(m): App) -> Response {
    let mut s = serde_json::to_value(crate::clip::STATUS.lock().unwrap().clone()).unwrap_or_default();
    s["on"] = json!(m.config().ai.search);
    Json(s).into_response()
}

/// What can be listened for, and what each camera is hearing now.
async fn sound_status() -> Response {
    let kinds: Vec<_> = crate::sound::KINDS.iter().map(|k| json!({"id": k.0, "name": k.1})).collect();
    let status = crate::sound::STATUS.lock().unwrap().clone();
    Json(json!({ "kinds": kinds, "cameras": status })).into_response()
}

async fn overview_key(State(m): App) -> Response {
    let cams: serde_json::Map<String, serde_json::Value> = m
        .cameras()
        .iter()
        .map(|c| (c.cfg.id.clone(), json!(crate::overview::camera_key(&m.data_dir, &c.cfg.id))))
        .collect();
    Json(json!({ "key": crate::overview::key(&m.data_dir, false), "cameras": cams })).into_response()
}

async fn overview_key_renew(State(m): App) -> Response {
    crate::overview::key(&m.data_dir, true);
    overview_key(State(m)).await
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
    if !cam.talk {
        return err(
            StatusCode::BAD_REQUEST,
            "two-way audio is off for this camera (camera settings → General)",
        );
    }
    let talk_url = if cam.talk_url.trim().is_empty() {
        cam.url.clone()
    } else {
        cam.talk_url.trim().to_string()
    };
    let who = user.map(|u| u.0.0.username.clone()).unwrap_or_default();
    ws.on_upgrade(move |mut socket| async move {
        let mut talk = match crate::talk::Talk::open(&cam.id, &talk_url).await {
            Ok(t) => t,
            Err(e) => {
                let _ = socket.send(Message::Text(json!({"error": format!("{e:#}")}).to_string().into())).await;
                return;
            }
        };
        tracing::info!("[{}] {} is talking through the camera's speaker ({})", cam.name, if who.is_empty() { "someone" } else { &who }, talk.codec());
        let _ = socket.send(Message::Text(json!({"ok": true, "codec": talk.codec()}).to_string().into())).await;
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        // What's said is kept, to be mixed into the recordings afterwards.
        let mut kept: Vec<i16> = Vec::new();
        let mut kept_from: Option<DateTime<Local>> = None;
        let res: anyhow::Result<()> = async {
            loop {
                tokio::select! {
                    msg = socket.recv() => {
                        let Some(Ok(msg)) = msg else { break };
                        match msg {
                            Message::Binary(b) => {
                                let s: Vec<i16> = b.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
                                kept_from.get_or_insert_with(Local::now);
                                if kept.len() < 8000 * 60 * 30 {
                                    kept.extend_from_slice(&s);
                                }
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
        if let (Some(from), Some(c)) = (kept_from, m.camera(&cam.id))
            && let Err(e) = crate::talkmix::save(&c.dir, from, &kept)
        {
            tracing::warn!("[{}] couldn't keep what was said: {e:#}", cam.name);
        }
    })
}

/// Settings → voice: the voices this computer has.
async fn tts_voices() -> Response {
    Json(crate::tts::voices().await).into_response()
}

#[derive(Deserialize)]
struct PreviewReq {
    text: String,
    #[serde(flatten)]
    tts: crate::config::TtsConfig,
}

/// Settings → voice: hear it here (a WAV file, as the camera would get it).
async fn tts_preview(State(m): App, Json(r): Json<PreviewReq>) -> Response {
    let ffmpeg = m.config().server.ffmpeg_path.clone();
    match crate::tts::speak(&ffmpeg, &r.text, &r.tts).await {
        Ok(s) => ([(axum::http::header::CONTENT_TYPE, "audio/wav")], crate::tts::wav(&s)).into_response(),
        Err(e) => bad(e),
    }
}

#[derive(Deserialize)]
struct SayReq {
    text: String,
}

/// Live view → "Say": the camera's speaker says the typed words.
async fn say(
    State(m): App,
    Path(id): Path<String>,
    user: Option<axum::Extension<CurrentUser>>,
    Json(r): Json<SayReq>,
) -> Response {
    let vis = visible(&user);
    let cfg = m.config();
    let Some(cam) = cfg.cameras.iter().find(|c| c.id == id && vis.as_ref().is_none_or(|v| v.contains(&id))).cloned() else {
        return err(StatusCode::NOT_FOUND, "no such camera");
    };
    if !cam.talk {
        return err(StatusCode::BAD_REQUEST, "two-way audio is off for this camera (camera settings → Audio)");
    }
    let samples = match crate::tts::speak(&cfg.server.ffmpeg_path, &r.text, &cfg.tts).await {
        Ok(s) => s,
        Err(e) => return bad(e),
    };
    let url = if cam.talk_url.trim().is_empty() { cam.url.clone() } else { cam.talk_url.trim().to_string() };
    let mut talk = match crate::talk::Talk::open(&cam.id, &url).await {
        Ok(t) => t,
        Err(e) => return bad(e),
    };
    let who = user.map(|u| u.0.0.username.clone()).unwrap_or_default();
    tracing::info!("[{}] {} had the camera say: {}", cam.name, if who.is_empty() { "someone" } else { &who }, r.text.trim());
    let from = Local::now();
    // Played in real time, a tenth of a second at a time.
    let start = tokio::time::Instant::now();
    for (i, chunk) in samples.chunks(800).enumerate() {
        if let Err(e) = talk.send(chunk).await {
            return bad(e);
        }
        tokio::time::sleep_until(start + Duration::from_millis(100 * (i as u64 + 1)) - Duration::from_millis(40)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    talk.stop().await;
    if let Some(c) = m.camera(&cam.id)
        && let Err(e) = crate::talkmix::save(&c.dir, from, &samples)
    {
        tracing::warn!("[{}] couldn't keep what was said: {e:#}", cam.name);
    }
    Json(json!({"ok": true, "seconds": samples.len() as f64 / 8000.0})).into_response()
}

#[derive(Deserialize)]
struct TalkTest {
    /// Existing camera (for its saved address when `url` is empty).
    #[serde(default)]
    camera: String,
    #[serde(default)]
    url: String,
}

/// Camera settings → "Test two-way audio": connect to the speaker and play
/// a short beep.
async fn talk_test(State(m): App, Json(t): Json<TalkTest>) -> Response {
    let cfg = m.config();
    let saved = cfg.cameras.iter().find(|c| c.id == t.camera);
    let url = if !t.url.trim().is_empty() {
        t.url.trim().to_string()
    } else if let Some(c) = saved {
        if c.talk_url.trim().is_empty() {
            c.url.clone()
        } else {
            c.talk_url.clone()
        }
    } else {
        return err(StatusCode::BAD_REQUEST, "enter the camera's address first");
    };
    let key = if t.camera.is_empty() {
        "test".to_string()
    } else {
        t.camera.clone()
    };
    let mut talk = match crate::talk::Talk::open(&key, &url).await {
        Ok(t) => t,
        Err(e) => return bad(e),
    };
    // Two short beeps (8 kHz).
    let beep: Vec<i16> = (0..8000)
        .map(|i| {
            let on = (i % 4000) < 2400;
            if on {
                ((i as f32 * 2.0 * std::f32::consts::PI * 880.0 / 8000.0).sin() * 9000.0) as i16
            } else {
                0
            }
        })
        .collect();
    let codec = talk.codec().to_string();
    for chunk in beep.chunks(800) {
        if let Err(e) = talk.send(chunk).await {
            return bad(e);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    talk.stop().await;
    Json(json!({"ok": true, "message": format!("Connected to the camera's speaker ({codec} audio) and played two beeps")})).into_response()
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
/// The sign-in cookie on the secure (https) address. Browsers share
/// cookies between ports, and a secure-only cookie would stop signing in on
/// the plain address: the two keep their own.
const COOKIE_SECURE: &str = "gaze_session_s";

fn session_token(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .filter(|(k, _)| *k == if secure_request(headers) { COOKIE_SECURE } else { COOKIE })
        .map(|(_, v)| v.to_string())
        .next()
}

/// Reached over HTTPS (e.g. through `tailscale serve` or another proxy)?
/// Came in on GAZE's own https port.
fn secure_request(headers: &axum::http::HeaderMap) -> bool {
    headers.contains_key("x-gaze-https")
}
fn https(headers: &axum::http::HeaderMap) -> bool {
    secure_request(headers)
        || headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("https"))
}

fn session_cookie(token: &str, remember: bool, secure: bool) -> HeaderValue {
    let age = if remember { format!("; Max-Age={}", crate::users::remember_days() as u64 * 86400) } else { String::new() };
    let sec = if secure { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{}={token}; Path=/; HttpOnly; SameSite=Lax{age}{sec}",
        if secure { COOKIE_SECURE } else { COOKIE }
    ))
    .unwrap()
}

/// The address a request comes from (the real one behind a proxy on this
/// computer, such as `tailscale serve`).
fn client_ip(addr: std::net::SocketAddr, headers: &axum::http::HeaderMap) -> std::net::IpAddr {
    let ip = addr.ip();
    if ip.is_loopback()
        && let Some(f) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .and_then(|v| v.trim().parse().ok())
    {
        return f;
    }
    ip
}

/// Password checks are slow on purpose (Argon2); a few at a time, off the
/// web server's threads, so a flood of sign-in attempts can't stall GAZE.
static PASSWORD_CHECKS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

async fn check_password_blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let _slot = PASSWORD_CHECKS.acquire().await.ok()?;
    tokio::task::spawn_blocking(f).await.ok()
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
        "/api/talk-test",
        "/api/tts/",
        "/api/update",
        "/api/logs",
        "/api/system",
        "/api/where",
        "/api/jobs/pause",
        "/api/https",
        "/api/modes",
        "/api/safe-mode",
        "/api/webhooks",
        "/api/overview",
    ];
    if admin_only.iter().any(|p| path.starts_with(p))
        || path == "/api/cameras"
        || path.ends_with("/device")
        || path.contains("/device/")
    {
        return false;
    }
    // Talking through a camera's speaker: not for viewers.
    if path.ends_with("/talk") || path.ends_with("/say") {
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
        || path == "/api/sprites-ready"
        || path.starts_with("/api/auth/")
    {
        return true;
    }
    // Operators: move cameras, pause recording, remove alerts/recordings,
    // switch Home / Away / Night.
    role == Operator
        && (path.contains("/ptz/")
            || path == "/api/mode"
            || path.ends_with("/snooze")
            || path.ends_with("/pause")
            || path.ends_with("/resume")
            || (method == axum::http::Method::DELETE
                && (path.contains("/events/") || path.contains("/recordings/"))))
}

/// Same-origin check: a change (or a live connection) must come from GAZE's
/// own page, not from another website open in the same browser.
fn foreign_origin(req: &Request) -> bool {
    let h = req.headers();
    let upgrade = h.get(header::UPGRADE).is_some();
    let read = matches!(
        *req.method(),
        axum::http::Method::GET | axum::http::Method::HEAD
    );
    if read && !upgrade {
        return false;
    }
    let Some(origin) = h.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        return false; // not a browser (curl, scripts with a session)
    };
    let Some((scheme, origin_host)) = origin.split_once("://") else {
        return true;
    };
    // HTTP/2 (https) gives the address in the request line, not a Host header.
    let host = h
        .get("x-forwarded-host")
        .or_else(|| h.get(header::HOST))
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or_else(|| req.uri().authority().map(|a| a.to_string()))
        .unwrap_or_default();
    origin == "null" || authority(origin_host, scheme) != authority(&host, scheme)
}

/// "host:port" in lower case, without the scheme's default port. The port
/// counts: another program's page on another port of the same computer is
/// the same "site" to the browser (its SameSite cookies don't stop it), but
/// it isn't GAZE.
fn authority(a: &str, scheme: &str) -> String {
    let a = a.trim().trim_end_matches('/').to_ascii_lowercase();
    let default = if scheme.eq_ignore_ascii_case("https") { ":443" } else { ":80" };
    a.strip_suffix(default).map(str::to_string).unwrap_or(a)
}

/// Security headers on every answer.
fn harden(mut r: Response) -> Response {
    let h = r.headers_mut();
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
    h.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    h.insert("referrer-policy", HeaderValue::from_static("same-origin"));
    r
}

async fn auth(State(m): App, req: Request, next: Next) -> Response {
    if foreign_origin(&req) {
        return harden(err(
            StatusCode::FORBIDDEN,
            "requests from other websites are not allowed",
        ));
    }
    harden(check_access(m, req, next).await)
}

async fn check_access(m: Arc<Manager>, mut req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    // The page itself, its pictures and the sign-in API are open: the page
    // shows the sign-in (or first-time setup) screen.
    let open = matches!(
        path.as_str(),
        "/" | "/app.js"
            | "/hls.min.js"
            | "/app.css"
            | "/logo.png"
            | "/icon.png"
            | "/favicon.png"
            | "/manifest.webmanifest"
            | "/api/snooze"
            | "/sw.js"
            | "/icon-192.png"
            | "/icon-512.png"
            | "/icon-maskable.png"
            | "/api/auth/state"
            | "/api/auth/login"
            | "/api/auth/setup"
            | "/api/changelog"
    );
    if open {
        return next.run(req).await;
    }
    // The private overview link: its key opens the overview, nothing else.
    let link_key = req.uri().query().and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("k=")));
    if (path == "/overview.jpg" || path == "/overview.mjpeg")
        && let Some(k) = link_key
        && crate::overview::key_ok(&m.data_dir, k)
    {
        return next.run(req).await;
    }
    // A camera's private link: its own key opens its picture and nothing else.
    if let Some(k) = link_key
        && let Some(rest) = path.strip_prefix("/live/")
        && let Some((cam, what)) = rest.split_once('/')
        && (what == "stream.mjpeg" || what == "frame.jpg")
        && crate::overview::camera_key_ok(&m.data_dir, cam, k)
    {
        return next.run(req).await;
    }
    // A signed-in browser, or an API token (scripts, Home Assistant,
    // another GAZE): "Authorization: Bearer gaze_…".
    let bearer = req.headers().get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).map(str::to_string);
    let user = match bearer {
        Some(t) => m.users.token_user(&t),
        None => session_token(req.headers()).and_then(|t| m.users.session_user(&t)),
    };
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
            two_step: u.two_step(),
            username: u.username,
            role: u.role,
            cameras: u.cameras,
        });
    Json(json!({ "setup_needed": m.users.setup_needed(), "user": me, "name": m.config().server.name, "idle_minutes": crate::users::idle_minutes() })).into_response()
}

#[derive(Deserialize)]
struct LoginReq {
    username: String,
    password: String,
    /// The code from the authenticator app (two-step sign-in).
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    remember: bool,
}

async fn auth_setup(
    State(m): App,
    headers: axum::http::HeaderMap,
    Json(r): Json<LoginReq>,
) -> Response {
    let m2 = m.clone();
    let (u, p) = (r.username.clone(), r.password.clone());
    let res = check_password_blocking(move || m2.users.setup(&u, &p)).await;
    match res.unwrap_or_else(|| Err(anyhow::anyhow!("busy, try again"))) {
        Ok(t) => {
            tracing::info!("admin account {:?} created", r.username.trim());
            (
                [(
                    header::SET_COOKIE,
                    session_cookie(&t, true, https(&headers)),
                )],
                Json(json!({ "ok": true })),
            )
                .into_response()
        }
        Err(e) => bad(e),
    }
}

async fn auth_login(
    State(m): App,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: axum::http::HeaderMap,
    Json(r): Json<LoginReq>,
) -> Response {
    let ip = client_ip(addr, &headers).to_string();
    let m2 = m.clone();
    let (u, p, remember, from, code) = (
        r.username.clone(),
        r.password.clone(),
        r.remember,
        ip.clone(),
        r.code.clone(),
    );
    let res = check_password_blocking(move || m2.users.login(&u, &p, code.as_deref(), remember, &from)).await;
    match res.unwrap_or_else(|| Err(anyhow::anyhow!("busy, try again"))) {
        Ok(t) => (
            [(
                header::SET_COOKIE,
                session_cookie(&t, r.remember, https(&headers)),
            )],
            Json(json!({ "ok": true })),
        )
            .into_response(),
        // Right password: now the code from the authenticator app.
        Err(e) if e.to_string() == crate::users::NEED_CODE => {
            (StatusCode::UNAUTHORIZED, Json(json!({ "error": "Enter the 6-digit code from your authenticator app", "need_code": true }))).into_response()
        }
        Err(e) => {
            // {:?}: a user name can't add lines to the log.
            tracing::warn!("failed sign-in for {:?} from {ip}", r.username.trim());
            err(StatusCode::UNAUTHORIZED, format!("{e:#}"))
        }
    }
}

/// API tokens of the signed-in user.
async fn tokens_list(State(m): App, user: axum::Extension<CurrentUser>) -> Response {
    Json(m.users.tokens(&user.0.0.username)).into_response()
}

#[derive(Deserialize)]
struct TokenReq {
    name: String,
}

async fn tokens_new(State(m): App, user: axum::Extension<CurrentUser>, Json(r): Json<TokenReq>) -> Response {
    match m.users.new_token(&user.0.0.username, &r.name) {
        Ok((id, token)) => {
            tracing::info!("{:?} made an API token \"{}\"", user.0.0.username, r.name.trim());
            Json(json!({ "id": id, "token": token })).into_response()
        }
        Err(e) => bad(e),
    }
}

async fn tokens_remove(State(m): App, user: axum::Extension<CurrentUser>, Path(id): Path<String>) -> Response {
    match m.users.remove_token(&user.0.0.username, &id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => bad(e),
    }
}

/// Turning on two-step sign-in: a new secret and its QR code to scan.
async fn two_step_start(State(m): App, user: axum::Extension<CurrentUser>) -> Response {
    let name = user.0.0.username.clone();
    let (secret, uri) = m.users.start_two_step(&name);
    let qr = qrcode::QrCode::new(uri.as_bytes())
        .map(|q| q.render::<qrcode::render::svg::Color>().min_dimensions(200, 200).quiet_zone(true).build())
        .unwrap_or_default();
    // Spaced in fours, for typing it in instead.
    let spaced = secret.as_bytes().chunks(4).map(|c| String::from_utf8_lossy(c).to_string()).collect::<Vec<_>>().join(" ");
    Json(json!({ "qr_svg": qr, "secret": spaced })).into_response()
}

#[derive(Deserialize)]
struct CodeReq {
    #[serde(default)]
    code: String,
    #[serde(default)]
    password: String,
}

async fn two_step_finish(State(m): App, user: axum::Extension<CurrentUser>, Json(r): Json<CodeReq>) -> Response {
    match m.users.finish_two_step(&user.0.0.username, &r.code) {
        Ok(codes) => {
            tracing::info!("{:?} turned on two-step sign-in", user.0.0.username);
            Json(json!({ "ok": true, "recovery": codes })).into_response()
        }
        Err(e) => bad(e),
    }
}

/// Turn it off for yourself: needs your password.
async fn two_step_off(
    State(m): App,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: axum::http::HeaderMap,
    user: axum::Extension<CurrentUser>,
    Json(r): Json<CodeReq>,
) -> Response {
    let name = user.0.0.username.clone();
    let ip = client_ip(addr, &headers).to_string();
    let (m2, n2, pw) = (m.clone(), name.clone(), r.password.clone());
    match check_password_blocking(move || m2.users.check(&n2, &pw, &ip)).await {
        Some(Ok(_)) => {}
        _ => return err(StatusCode::BAD_REQUEST, "the password is wrong"),
    }
    match m.users.stop_two_step(&name) {
        Ok(()) => {
            tracing::info!("{name:?} turned off two-step sign-in");
            Json(json!({ "ok": true })).into_response()
        }
        Err(e) => bad(e),
    }
}

/// An admin turns it off for someone who lost their phone and codes.
async fn two_step_off_admin(State(m): App, Path(name): Path<String>) -> Response {
    match m.users.stop_two_step(&name) {
        Ok(()) => {
            tracing::info!("two-step sign-in turned off for {name:?} by an admin");
            Json(json!({ "ok": true })).into_response()
        }
        Err(e) => bad(e),
    }
}

async fn auth_logout(State(m): App, headers: axum::http::HeaderMap) -> Response {
    if let Some(t) = session_token(&headers) {
        m.users.logout(&t);
    }
    (
        [(
            header::SET_COOKIE,
            if secure_request(&headers) {
                HeaderValue::from_static("gaze_session_s=; Path=/; Max-Age=0; Secure")
            } else {
                HeaderValue::from_static("gaze_session=; Path=/; Max-Age=0")
            },
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
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: axum::http::HeaderMap,
    user: axum::Extension<CurrentUser>,
    Json(r): Json<PasswordReq>,
) -> Response {
    let name = user.0.0.username.clone();
    let ip = client_ip(addr, &headers).to_string();
    let (m2, n2, cur) = (m.clone(), name.clone(), r.current.clone());
    let ok = check_password_blocking(move || m2.users.check(&n2, &cur, &ip)).await;
    match ok {
        Some(Ok(_)) => {}
        Some(Err(e)) if e.to_string().contains("wait") => return err(StatusCode::BAD_REQUEST, e),
        _ => return err(StatusCode::BAD_REQUEST, "the current password is wrong"),
    }
    let (m3, n3, new) = (m.clone(), name.clone(), r.new.clone());
    let res = check_password_blocking(move || m3.users.update(&n3, None, None, Some(&new))).await;
    match res.unwrap_or_else(|| Err(anyhow::anyhow!("busy, try again"))) {
        Ok(()) => {
            // Signed out everywhere; stay signed in on this browser.
            let t = m.users.new_session(&name, true);
            (
                [(
                    header::SET_COOKIE,
                    session_cookie(&t, true, https(&headers)),
                )],
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
mod tests {
    use super::*;

    fn req(method: &str, origin: Option<&str>, host: &str) -> Request {
        let mut b = axum::http::Request::builder().method(method).uri("/api/settings").header(header::HOST, host);
        if let Some(o) = origin {
            b = b.header(header::ORIGIN, o);
        }
        b.body(Body::empty()).unwrap()
    }

    #[test]
    fn other_sites_cannot_make_changes() {
        // GAZE's own page.
        assert!(!foreign_origin(&req("POST", Some("http://192.168.1.5:8080"), "192.168.1.5:8080")));
        assert!(!foreign_origin(&req("POST", Some("https://nvr.home"), "nvr.home:443")));
        assert!(!foreign_origin(&req("POST", Some("http://NVR.home"), "nvr.home")));
        // Another program's page on another port of the same computer.
        assert!(foreign_origin(&req("POST", Some("http://192.168.1.5:3000"), "192.168.1.5:8080")));
        assert!(foreign_origin(&req("POST", Some("http://evil.example"), "192.168.1.5:8080")));
        // HTTP/2: the address is in the request line.
        let h2 = axum::http::Request::builder().method("POST").uri("https://192.168.1.5:8443/api/x").header(header::ORIGIN, "https://192.168.1.5:8443").body(Body::empty()).unwrap();
        assert!(!foreign_origin(&h2));
        assert!(foreign_origin(&req("POST", Some("null"), "192.168.1.5:8080")));
        // Reading is fine; tools without a browser send no Origin.
        assert!(!foreign_origin(&req("GET", Some("http://evil.example"), "192.168.1.5:8080")));
        assert!(!foreign_origin(&req("POST", None, "192.168.1.5:8080")));
    }

    #[test]
    fn viewers_can_load_previews_but_not_settings() {
        use crate::users::Role::*;
        let post = axum::http::Method::POST;
        assert!(allowed(Viewer, &post, "/api/sprites-ready"));
        assert!(!allowed(Viewer, &post, "/api/settings"));
        assert!(!allowed(Operator, &axum::http::Method::GET, "/api/cameras"));
    }
}
