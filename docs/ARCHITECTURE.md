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

**Looks (skins)**: `static/skins.css` (served at `/skins.css`, versioned like
`app.css`) holds ten alternative designs; with no `data-skin` on `<html>` it
does nothing (that is Classic). The choice is per device (`localStorage`
`nvr-skin`, 👤 menu → Look; `SKINS`/`applySkin` in `app.js`, early inline script
in `index.html`). `data-mode="light|dark"` is the resolved day/night, set next
to `data-theme`. Part 1 of the file defines each look's palette with the same
token names as `app.css`; part 2 (CSS nesting inside `:where()`, specificity 0)
does layout and decoration, so any class rule in `app.css` still wins. Layout
changes sit in `(min-width: 761px)`: phones keep the tab bar. Timeline tokens
are derived from the palette (`color-mix`) in one place. To add a look: palette
for both modes, a nested block, an entry in `SKINS`.

## Updates and installation (Windows)

Releases on GitHub (`release.yml`): `GAZE.exe`, `GAZE-Watchdog.exe` + `.sha256`.
GAZE updates itself (`update.rs`, swap-and-restart); the watchdog updates
itself (`watchdog/main.rs`) or on request from GAZE's page. Every open page
reloads when the server's boot id changes.
