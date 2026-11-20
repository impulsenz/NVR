# Configuration inventory

**Where a setting lives.** Settings are one TOML file (`nvr.toml`) parsed into
`src/config.rs` (`Config` and 31 other structs, **260 settings**). There are
no environment-variable settings for the program itself; a few variables
exist for tests and the build (`GAZE_UPDATE_REPO`, `GAZE_UPDATE_WEB`,
`GAZE_UPDATE_API`, `RUST_LOG`). The watchdog has its own file
(`nvr-data/watchdog/settings.toml`, 19 settings, edited from Settings →
Watchdog). Per-browser preferences (theme, layout, picture size, timeline
orientation …) are in the browser's `localStorage` through `store.get/set` in
`app.js` and are never sent to the server.

**How a setting flows.** page form → `PUT /api/settings` (or
`/api/cameras/{id}`) → `manager.rs` (`update_settings`, `update_camera`;
`validate()`, `check_urls()`) → `Config::to_toml` → file → running parts read
`mgr.config()` (a cloned snapshot) → anything that needs a restart is in
`CameraConfig::needs_restart`; the rest applies live.

**Compatibility rules.** Unknown keys in a settings file are ignored
(`#[serde(default)]`), so removing a setting never stops an old file from
loading; but removing one *silently drops the user's value* — do it only with a
migration in `Config::migrate` and a note in the changelog. Validation runs at
load (so keep it permissive) and, for stream addresses, additionally when an
address is entered (`stream_url_ok`).

## Audit result (1.1.9) — `python3 tools/audit/config_usage.py`

* **Not mentioned outside `config.rs`:** `ZoneConfig.watch_items` (read only by
  the migration that moved watched items into the package spot).
* **Mentioned only by the page (30):** the on-screen display layout
  (`OsdConfig.*`), `DisplayConfig.clock_format`/`fullscreen_corners`,
  `ViewConfig.grid_quality`, `LiveConfig.preload_hd_streams`, dewarp view
  (`yaw/pitch/lens`), `ModesConfig.auto_night`, schedule, deep-storage and
  privacy-mode fields. These are stored by the server for the page to apply;
  not dead.
* **Obsolete but kept for old files:** `DashboardConfig/CameraGroup.fullscreen_tiles`
  (the page writes `[]`), `DetectConfig.package_area`, `ZoneConfig.watch_items`.
* **Misnamed:** `DisplayConfig.fullscreen_corners` (only the first entry, the
  position of the button cluster, is used).
* **Removed earlier:** `cloud.offload`, `cloud.local_hours` (1.1.5).

No setting was found that is exposed but never applied, or that conflicts with
another. Defaults were not re-evaluated one by one (not done).

## Where to look

| Area | Struct | Applied in |
|---|---|---|
| Server (listen, https, ffmpeg path, update repo) | `ServerConfig`, `UpdateConfig` | `main.rs`, `tls.rs`, `update.rs` |
| Storage (folders, retention, limits, time-lapse) | `StorageConfig` | `storage.rs`, `picturelog.rs` |
| Deep storage | `StorageConfig.archive` (`ArchiveConfig`), `CameraConfig.archive_*` | `archive.rs`, `Config::archive_for` |
| Backup | `CloudConfig` | `cloud.rs` |
| Cameras (stream, recording, detection, PTZ, privacy, audio) | `CameraConfig` and children | `manager.rs` → `recorder.rs`, `relay.rs`, `detect.rs` … |
| AI | `AiConfig` (`device`, `accelerator`, `openvino_device`), per camera `DetectConfig` (incl. `hd_check` and `hd_frames`, the full-quality double-check and how many pictures it may take; `TtsConfig.phrases_version` marks which ready phrases were added to the list) | `ai.rs`, `infer.rs`, `detect.rs` |
| Live view and display | `LiveConfig`, `DisplayConfig`, `OsdConfig`, `ViewConfig`, `DashboardConfig` | page (`app.js`) and `live.rs` |
| Modes | `ModesConfig` | `notify.rs`, `manager.rs::privacy_watch` |
| Notifications (ntfy + email), MQTT, webhooks, voice, siren | `NotifyConfig` (`.email`: `EmailConfig`, `.digest`: `DigestConfig`), `MqttConfig`, `Webhook`, `TtsConfig` | `notify.rs` (`send`: both channels), `mail.rs`, `digest.rs`, `mqtt.rs`, `hooks.rs`, `tts.rs`, `siren.rs` |
| Learned states (not in `nvr.toml`: their own data file `states.json`) | `states::State` | `states.rs`, `web.rs` (`/api/states`), `mqtt.rs` |
| Daily summary (`[summary]`: schedule on/off, time, hours covered, cameras, kinds of object, video length, speed, days kept; videos in `nvr-data/synopsis/`) | `config::SummaryConfig` (defaults: off, 06:30, 24 h, all cameras, person/vehicle/dog/cat/bird, 60 s, 2× speed, 30 days; absent in old files = defaults) | `synopsis.rs`, `web.rs` (`/api/summaries*`, `/api/cameras/{id}/synopsis*`), Settings → Daily summary |
| Heat map (no setting: always logs; `nvr-data/heat/`, 30 days) | `heatmap::KEEP_DAYS` | `heatmap.rs`, `detect.rs`, `web.rs` (`/api/heatmaps`) |
| Floor plan and following people (not in `nvr.toml`: `plan.json` (incl. walls, icon sizes), `plan.jpg`, `tracks/`) | `plan::Plan`, `plan::Tuning` (14 settings + labels, off by default) | `plan.rs`, `tracks.rs`, `geo.rs`, `web.rs` (`/api/plan*`) |
| Watchdog | `settings.toml` | `watchdog/main.rs`; written by `watchdog_ctl.rs` |
