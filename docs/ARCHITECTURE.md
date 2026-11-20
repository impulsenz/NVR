# GAZE architecture (current: 1.1.x)

Verified against the code in the November 2026 audit. The 1.0 performance
record (`ARCHITECTURE-AND-PERFORMANCE.md`) has the benchmarks; this file is the
map. **Update it when the structure changes** (see `CLAUDE.md`).

## Programs

| Program | Source | Role |
|---|---|---|
| `GAZE.exe` (`nvr`) | `src/main.rs` + modules | The NVR: web server, supervision of ffmpeg, recording, detection, storage |
| `GAZE-Watchdog.exe` (`gaze-watchdog`) | `src/watchdog/` | Optional separate program (and Windows service, with its own tray icon) that reopens GAZE if it stops or hangs; talks to GAZE only through files in `nvr-data/watchdog/` |
| ffmpeg | external | One process per job (relay, recorder, buffer, timeline copy, live decode, live repackage, motion decode) |
| go2rtc | external, optional | Camera connection layer instead of direct connections |
| Browser | `static/` (served from the binary) | Plain JavaScript page, no framework, no build step |

## Camera → disk and browser

```
camera ─RTSP─▶ relay (src/relay.rs): ONE connection per stream, bounded queue per consumer
                 ├─▶ recorder.rs      -c:v copy → MP4 segments  ─▶ storage.rs / finalize.rs / mp4fix.rs
                 ├─▶ buffer.rs        pre-event HD segments (event modes)
                 ├─▶ timeline copy    sub-stream copy for fast scrubbing
                 ├─▶ detect.rs        sub-stream decode → motion → ai.rs (YOLOX via infer.rs) → events.rs
                 │        │                                   └▶ identify.rs (faces/plates after an alert), clip.rs (CLIP index)
                 │        └▶ live.rs   JPEG frames → web page grid (one WebSocket)
                 └─▶ livemux.rs       fragmented MP4 over WebSocket → MSE (one shared repackager per stream)
recordings ─▶ hls.rs (playlists from MP4 files) / vstore.rs (block reader for slow disks and NAS) ─▶ browser
```

Recording never reads `ai`, `detect` or `infer` (checked). AI failure degrades
alerts only.

## AI placement (`infer.rs`)

Models run on an accelerator through ONNX Runtime (`Accel`: DirectML on Windows; CUDA or OpenVINO on Linux x86-64; setting `ai.accelerator`, `ai.openvino_device`) or on the processor with tract. One runtime library per process, so changing accelerator needs a restart. Linux runtimes come from PyPI wheels pinned by URL and SHA-256 (`OPENVINO_WHEEL`, `CUDA_WHEEL`), unpacked to `nvr-data/runtime/`. A model the accelerator can't run falls back to tract; recording never depends on any of this.

## Learned states (`states.rs`)

A state ("garage door": open / closed) is a camera, an optional area, and labelled examples, each stored as CLIP numbers (`Clip::region`, the picture squeezed to a square so nothing is cut off). A background loop (idle until a state has two labels with examples) takes the detector's latest frame (or a fresh grab), crops the area and scores it against each label (mean of the closest three examples, `classify`); a change needs two agreeing looks (`Debounce`). The result goes to the page, MQTT (`<prefix>/<camera>/state/<name>` plus a Home Assistant sensor) and, if set, `notify::send`. Recording, detection and alerts never read it.

## Drive health (`smart.rs`)

Runs `smartctl --scan -j`, then `smartctl -a -j` per drive (the program is optional and external; nothing is linked). `parse` turns one drive's JSON into `Drive` (health `ok` / `warning` / `failing` / `unknown`); `run` rescans every 6 h and tells `notify::send` once per new problem. Shown in Settings → System; `/api/smart` is admin only. Read-only: recording and storage never consult it.

Backup (Settings → Backup, `export_config` / `import_config` in `web.rs`): a .zip of settings, users, people, learned data and, optionally, the **Evidence Locker** (`locker/cases.json` + `locker/<case>/<file>`, stored not compressed). The export is written to `nvr-data/tmp/` and streamed (a locker can be many gigabytes); the import streams the upload to a file first. Importing adds cases that aren't here and never replaces or changes one that is (`cases::import_case`).

Recordings timeline (`drawTimeline`, `drawVTimeline`, `tlMerged`): merged by default (one lane of every camera's footage and alerts, along the bottom or down the side), or one lane per camera; the choice is `localStorage` `tl-merge`. The per-camera blue time-lapse strip exists only in the by-camera lanes.

Camera logs (`camlog.rs`): per camera, what the camera itself reported over ONVIF (`onvif_events.rs` writes each activation, notes unused topics at most every 10 minutes per topic, and connection changes) in `nvr-data/camlog/<camera id>.jsonl` (last 1,000). `GET /api/cameras/{id}/log` (admin only) merges them with that camera's lines of the system log (`logs.rs`, by camera name). Shown on Edit camera → Log. Package delivery (`detect.packages`, `detect.package_mask`) is set up under Settings → Learned states (`packageSection`).

Drive health (`smart.rs`): `POST /api/smart/install` (Windows) downloads smartmontools' own installer, runs it only if its SHA-256 matches `WIN_INSTALLER_SHA256`, installs it silently and rescans. The daily summary (`synopsis.rs`) decodes with the Hardware settings' decoder (`Decoder::open`, processor if the card can't read the file) and encodes with its GPU encoder (`encoder_works` tries it once), libx264 otherwise.

## Heat map page and daily digest (`heatmap.rs`, `digest.rs`)

**Heat map** (the *Heat map* tab; a function, not a setting): `detect.rs` hands `heatmap::motion` every look of the motion detector (`MotionResult.raw`: the moving areas **before** ignore areas apply) and `heatmap::objects` every look of the AI (every recognised object with score ≥ 0.4, whether or not it alerts or is wanted; vehicle kinds as "vehicle"). They are added into a per-camera, per-hour 64×36 grid per kind (`ACC`) and written once a minute by `heatmap::run` to `nvr-data/heat/<camera>/<day>.json` (only the cells that counted; the last cell of a grid counts boxes), kept 30 days (`tidy`). `GET /api/heatmaps?days=&kind=&camera=&include_ignored=` (`heatmap::read` + `select`) returns each visible camera's grid for a kind (plus boxes by hour of day, how many of each object, and `ignored_share`). **The areas painted as ignore in the camera's Motion settings are never applied when logging**; when reading, the cells they cover (`ignore_cells`, ≥ half painted) are left out unless `include_ignored=true` (page default: off), using the camera's *current* mask, so nothing is lost by ignoring and the box counts are estimates (scaled to the cells kept) whenever something is left out. Kinds: `motion`, `objects`, `people`, `vehicles`, `animals`, `other`, `label:<name>`, or `unexplained` (motion minus what recognised objects account for: where to paint ignore areas; every recognised object counts as an explanation). The page offers only `heatmap::SHOWN` (person, vehicle, cat, dog): `objects` and the list of objects seen are limited to them, with the camera's ignore mask. Hours before the log existed are filled from the boxes stored with alerts (`Event.boxes`). The page (`showHeat`, `renderHeat`, `drawHeat`; a controls card, a card per camera with a 24-hour strip, and a one-camera view with the object breakdown) smooths: a small blur of the numbers, bilinear scaling and a CSS blur, a square-root curve and a colour ramp from the `--heat-*` tokens scaled to the 95th-percentile level so one hot spot doesn't wash out the rest. Counts are looks (about one per second of something being there), so dwell time weighs. Recording, detection and alerts never read it.

**Daily digest**: `digest::compose` (pure) turns the last 24 hours of alerts plus current problems (offline cameras, `health::drive_low`, `smart::overview`) into a message; `digest::run` sends it once a day through `notify::send` (a marker file `nvr-data/digest-last` prevents a second send the same day; a missed time is caught up for 6 hours).

## Floor plan and following people (`plan.rs`, `tracks.rs`, `geo.rs`)

```
detect.rs handle_ai ──▶ tracks::observe (no-op unless switched on)
      │                    └▶ Tracker per camera: looks → walks (nearest match, radius, gap)
      │                         clothing-colour Sig per walk (imgutil crop; weak clue)
      ▼                              │ closed walks
(recording and alerts unchanged)     ▼ DONE queue ──▶ tracks::run ──▶ Store: nvr-data/tracks/<day>.jsonl
                                                                        │ (picture coordinates only)
 GET /api/plan/journeys ◀── tracks::link(walks, plan) ◀── read ◀────────┘
      │      homography per camera (geo.rs, 4 calibration spots) → plan coordinates, metres (px_per_m)
      ▼      join score = weighted timing + distance/speed + colour; one-to-one, best first
 page: map (SVG over plan.jpg), journeys, playback, live dots (GET /api/plan/live)
```

* Walks are stored in **picture** coordinates; journeys and plan paths are
  computed when read, so re-calibrating a camera fixes old walks too.
* `plan.json` holds the picture's size, scale, cameras (position, facing, field
  of view, calibration), neighbour links and the `Tuning` settings (validated
  on save; off by default). `plan.jpg` is the picture, re-encoded to JPEG by the
  page. Restricted accounts get only their cameras and the links between them
  (`restrict_plan`), and only walks on their cameras.
* `plan.json` also holds **walls** (segments in plan fractions, `plan::Wall`, at most 1500). The page's `planSight` ray-casts each camera's view against them for drawing only; tracking and linking ignore walls.
* The **map picker** (`planMapPicker` in `app.js`) draws slippy-map tiles from a provider onto a canvas in the browser (Web-Mercator maths, rotation), snapshots it with the credit line, and uploads it like any picture; `px_per_m` comes from the latitude and zoom. GAZE's server never fetches imagery.
* The detector's only involvement is one call after each AI look. A busy or
  broken tracker drops data; it never delays recording, detection or alerts.

## Daily summary (`synopsis.rs`)

A camera's day or last N hours as a short video of **AI-recognised** objects. Asked for from the Alerts page (`POST /api/summaries/make`, one or all cameras, or `POST /api/cameras/{id}/synopsis?when=`) or by `synopsis::run` at the time set in `[summary]` (Settings → Daily summary; marker `nvr-data/summary-last`, up to 6 h catch-up like the digest). A summary is named by an **id**: a day (`2026-11-18`) or a window (`2026-11-19_0630_24h`), checked by `parse_id` so ids are always safe file names. Requests join `QUEUE`; one worker thread makes them one after another at low priority (`nice` / below-normal class; waits while `load::cpu()` is high).

Footage is the light timeline copy where it covers a recording, else the recordings and deep storage (`pick_sources`); ffmpeg decodes to raw 640×360 pictures.

1. `backgrounds`: keyframes (one per ~90 s, at most 15 per hour kept) → per-pixel median per hour = the empty scene; the hour of middle brightness is the one shown.
2. `Detector` (6 pictures/s): 320×180 sample, a slowly adapting copy of the scene (stopped things fade in about a minute), brightness drift removed, changed cells joined into boxes. `Tracker` joins boxes over pictures into **tubes** (RGBA cut-outs, memory capped).
3. Recognition: while a tube is unnamed, about every 1.5 s the picture goes to `Ai::detect` (the same model the detector uses, called from the worker thread via the runtime handle, at most 20,000 looks). A tube takes the name of the wanted kind (`[summary].objects`, vehicles as "vehicle") found where it is; after 3 looks with none its pictures are dropped. Unnamed tubes never reach the video. So it does **not** depend on what alerts or clips exist.
4. `plan` / `schedule`: tubes on a 16×9 footprint grid, each **as early as it fits anywhere on the new timeline** (so objects in different parts of the picture play together whatever time they really were); from the asked speed (`[summary].speed`, default 2×) with no overlap, then with more overlap, then faster (up to 8×) only while longer than `[summary].seconds`.
5. `render` (24 pictures/s): between two seen moments an object is drawn as both, each shifted towards where it is now and cross-faded (`blit_between`), cut-outs have a feathered edge and are lightened to the shown scene (`relight`). → ffmpeg (libx264) → `nvr-data/synopsis/<camera>/<id>.mp4` + `.json` (objects: name, real times, boxes twice a second for the click-through) + `.jpg` (busiest moment, the card picture). **One summary per camera**: once a new one is made, `keep_only` deletes that camera's other summaries (never footage, never a `.part.mp4` being written), and `list` shows only the newest per camera (older folders may hold more until the next one). `tidy` removes files past `keep_days`.

`GET /api/summaries` lists cards and what is queued or running (restricted accounts see only their cameras). Camera routes are under `camera_in_path`; operators may make summaries (CPU cost), viewers only watch. Recording, detection and alerts never read any of it.

## Storage tiers

```
local disk  <storage>/<camera folder>/*.mp4          recorder → storage::cleanup (limits, retention)
    │  after N hours (archive.rs; verified copy, then remove)
    ▼
deep storage <archive dir>/<camera folder>/*.mp4     archive::retention (days, GB); read in place
```

* `storage::resolve(dir, name)` falls back to the camera's deep-storage folder
  (registry kept by `archive::sync_registry` and `library::timeline`), so
  playback, thumbnails, HLS, exports find moved files; `library::recordings_all`
  merges both lists for the Alerts/Recordings list.
* Backup (`cloud.rs`) *copies* alerts or recordings to a folder/FTP/rclone and
  keeps the originals; deep storage *moves*.
* Evidence Locker (`cases.rs`): copies of footage in `nvr-data/cases/`;
  protected items block every delete path (`locks_time`, `locks_event`).

## State (no database)

| What | Where |
|---|---|
| Settings | `nvr.toml` (`config.rs`; migrations in `Config::migrate`; safe mode if unreadable) |
| Alerts | JSON lines per camera in `events/`, all held in memory (`events.rs`) |
| Recordings | files; listing cached 5 s (`storage::list`) |
| Time-lapse previews | `.thumbs/` beside each camera folder |
| Users, sessions, tokens | `users.rs` (Argon2), files in `nvr-data/` |
| Cases | `nvr-data/cases/cases.json` + files |
| Learned states | `nvr-data/states.json` (CLIP numbers per example) + `nvr-data/states/<id>/` example pictures (`states.rs`) |
| Watchdog | `nvr-data/watchdog/{settings.toml,status.json,commands/,watchdog.log}` |
| Global runtime state | 72 `static` Mutex/LazyLock/OnceLock (jobs, snooze, cloud/archive status, registries …); locked through `sync.rs` (`.locked()`, `.reader()`, `.writer()`), which ignores poisoning |

## Web layer (`src/web.rs`, 6,500 lines)

axum router → `auth` middleware (origin check, security headers) →
`check_access` (open paths, private-link keys, bearer token or session cookie,
`allowed()` role check, `camera_in_path()` camera check) → handler. Handlers
that take a camera **also** check `visible(&user)` (35 do). Admin-only
prefixes are listed in `allowed()`.

Static assets: `app.js`, `app.css`, `hls.min.js`, index page — gzipped once,
ETag, 304 (`pack`, `static_text`).

## Background workers (`tokio::spawn` in `main.rs`)

notifications (ntfy push and email, `notify.rs` + `mail.rs`), housekeeping (retention), system monitor, cloud backup, **archive mover**,
watchdog sync, prebuilt time-lapse, finalize, discovery, identify, talk mixing,
notifications, health, MQTT, hooks, ONVIF events, voices, sound detection,
CLIP indexing, PTZ auto, RTMP restream, updates. Each has its own backoff;
work that can wait pauses for CPU (`load.rs`) and can be paused by the user
(System → Tasks).

## Page (`static/`)

`app.js` 9,800 lines (pages: Live, Alerts/Recordings, Evidence, System,
Settings), `app.css` 1,700 lines, `index.html`. 38 timers; one WebSocket for
live JPEG, one per watched video stream. Theme tokens in `:root` and the
light override (including the `--tl-*` timeline tokens).

Only one design exists (the original); the 1.3.x "looks" (`skins.css`) were removed. Day / night is the only choice (`data-theme`, `um-theme`).

## Updates and installation (Windows)

Releases on GitHub (`release.yml`, Windows only): `GAZE.exe`, `GAZE-Watchdog.exe` (+ `.sha256` each) and `GAZE-Watchdog.version`. The watchdog has its own version (`VERSION` in `src/watchdog/main.rs`, raised only when the watchdog is changed on purpose) and updates only when that file differs from its own.
GAZE updates itself (`update.rs`, swap-and-restart); the watchdog updates
itself (`watchdog/main.rs`) or on request from GAZE's page. Every open page
reloads when the server's boot id changes.
