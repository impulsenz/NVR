# Prompt: rebuild GAZE from scratch

Copy everything below the line into a new session.

---

You are rebuilding **GAZE**, a network video recorder (NVR) for home IP cameras, from scratch. The old
code (a Rust program with one `app.js` page of ~11,000 lines) grew by accretion and has quirks, duplicated
logic and overlapping features. Keep every feature listed here; do not copy the old code's mistakes.
Use the old repository only as a reference for behaviour, never as a template to port line by line.

## Order of priority (never break this)

1. **Recording reliability and the user's saved data come before every feature.**
2. Recording must never depend on AI, the web page, deep storage, or any watchdog. If those fail, recording
   carries on.
3. Never delete or overwrite footage without a verified copy. Never touch footage held in an Evidence Locker
   case.
4. A restricted account must never see or change another camera's data.
5. Speed of the Recordings page: it must start playing quickly, with many cameras at once.

## Shape of the product

* A single program (Rust preferred, one executable, no installer needed) with a built-in web server and a
  web page with **no build step**. Windows is the main target (system tray, runs as a service, self-update
  from GitHub releases, a separate **watchdog** service that reopens GAZE however it stopped); Linux, macOS
  and Docker also supported.
* Everything runs locally. No account, no cloud service. Nothing leaves the machine unless the user turns on
  phone/email notifications or backup.
* One settings file (`nvr.toml`) that old versions' files still load into: unknown keys ignored, a default
  for every new setting, a migration (never a silent drop) when a setting is renamed or removed.
* ffmpeg and ffprobe do the video work (one process per job). One connection per camera stream, shared by
  recording, detection and live view (many cameras only allow two or three connections).

## Features to rebuild

### Cameras and setup
* Add camera wizard: pick brand, IP address and login, stream addresses filled in; **Test connection**
  shows both streams with codec and resolution. Discovery on the network (ONVIF). TP-Link Tapo and similar
  cameras through go2rtc, falling back to a direct connection if go2rtc gets no picture.
* Main stream + sub-stream per camera (sub-stream strongly recommended). Only streaming addresses accepted
  (rtsp, rtmp, http streams…); never build shell strings.
* Per-camera: name (folders are named as the user calls the camera), groups, storage folder (another drive,
  a NAS share, a synced folder), retention days, size limit, schedule, privacy areas (boxes drawn over the
  picture, can follow a mode), audio on/off, display (dewarp view for fisheye cameras), on-screen display.
* H.265 cameras: tag copied H.265 as `hvc1`; convert on the fly (H.264) only for browsers that can't play
  it; convert non-AAC sound (G.711, G.726…) to AAC so recordings stream in the browser.
* Pan/tilt/zoom with presets (ONVIF), automatic PTZ, push-to-talk to cameras with a speaker, siren.
* RTMP and ONVIF event inputs; FTP inbox for cameras that push pictures.

### Recording
* Modes per camera: main stream 24/7; sub-stream 24/7; sub-stream 24/7 **plus** full-quality (HD) alert
  clips; alert clips only; off. Scheduled recording.
* Copy the camera's video as it is (`-c:v copy`); re-encode only when a camera is set to lower its frame
  rate or draw privacy boxes (GPU encoder when it works, CPU fallback).
* Files: fixed length (default **5 minutes**), cut on the clock, named `<camera>_YYYY-MM-DD_HH-MM-SS.mp4`,
  written as **fragmented MP4** so a power cut costs at most the last second or two. **A finished file must
  never be much longer than the set segment length** — warn in the log if it is (an hour-long file was seen
  in the old app and its cause was never found).
* Crash repair: on start, cut any file back to its last complete piece.
* Pre-event buffer: continuously record full quality into short files, keep only the pre-event time, and
  join the buffer plus the event plus post-event time into one clip when an alert happens.
* Optional "timeline copy": the sub-stream recorded alongside HD so the timeline opens and scrubs instantly.
* Supervisor: restart ffmpeg when it dies or stalls, back off, fall back from GPU to CPU.
* Recording folder unreachable (NAS down): show a clear warning, keep trying, keep recording elsewhere if
  configured. Never lose footage already written.

### Storage
* Retention by days and by size per camera and overall, estimated "how many days fit", disk usage by kind of
  footage.
* **Deep storage** (a NAS or second drive for older footage): after N hours (default 24) move each file:
  copy, verify (size and checksum), only then delete the local copy. Own keep time and size limit. Playback,
  thumbnails, exports and the timeline find files wherever they are, and **footage in deep storage must play
  when scrubbing, as fast as local footage** (read in pieces, with read-ahead and caching, tolerant of a
  slow share).
* **Backup** (copies, keeps originals) of alerts or recordings to a folder, FTP or rclone; verified.
* **Evidence Locker**: cases that lock footage so nothing (retention, deep-storage retention, deletes) can
  remove it; export, picture-to-case, notes.
* Data retention for logs, tracks, heat data (30 days), summaries.
* Drive health (SMART) notice, once per new problem.

### Live view
* Resizable grid with groups and a full-screen video wall. Grid uses sub-streams; opening a camera switches
  to the full-quality stream. On-screen display (name, clock, layout configurable), PTZ controls, push to
  talk, sound, snapshot, data-saver (smaller pictures, fewer frames), "lighter live video".

### Recordings page (the most important page — design it first)
* **Multi-camera vertical timeline**: one column per camera, time down the side, recordings as bars, alerts
  as thumbnails with counts, a time-lapse strip, a "now" line, zoom (e.g. 1 h), merge cameras, horizontal
  or vertical, layout choice (e.g. 3 across).
* Drag across the timeline to **scrub every camera at once** using preview pictures; let go to play from that
  moment. Play, ±1 frame, previous/next alert, jump to live, sound, speed (1×…), sub-stream/HD choice, clip
  start/end handles, download clip, add picture to a case.
* Playback is **one continuous stream per camera (HLS)** built from byte ranges of the recorded files
  (index each file's pieces from the box headers; nothing converted or copied), buffered ahead (about
  10 minutes), loaded evenly across cameras, stepping over small gaps, going file to file without a blink.
  Fallbacks: file-by-file player for HD alert clips, converted playback for formats the browser refuses.
* An alert list with filters, a calendar, per-alert detail; delete footage (admin) with confirmation.
* Time-lapse video export.
* Loading rules learned the hard way — **video always comes first**:
  * The browser allows only 6 connections per host over plain HTTP/1.1. Do not let background work (preview
    pictures, warm-ups, other cameras' playlists) starve the video requests. Prefer HTTP/2 (HTTPS) and say so
    in the page.
  * Preview-picture pages are ~3 MB each: never fetch every page of every file. Fetch only what is near the
    playhead, only on the Recordings page, only after the video has started.
  * A playlist request needs a generous timeout (30 s) and a retry; a failed one must not switch a whole
    camera to a worse mode for good.
  * Never seek far past the end of a file (a `t=12676` seek into a 5-minute file hung the page).
  * Opening the page should fetch only what is visible, then fill in the rest.

### Detection and AI
* Motion detection (minimum size, contrast, travel, painted ignore areas) followed by object detection
  (YOLOX-class model on GPU via DirectML / CUDA / OpenVINO, or CPU). New finds can be verified on a
  full-quality picture. Zones, line crossing, loitering; parked vehicles stay quiet until they move; a
  painted package spot that reports parcels left or taken; ignore pets; privacy-aware.
* Faces and number plates read from HD footage so alerts say who or whose car; learned **states** ("garage
  door open/closed") by labelled examples; sound classification; tracking of people across cameras.
* **Heat map page**: logs every motion look and every AI look (small files, 30 days), shows motion, people,
  vehicles, animals, any one kind, and "motion nothing recognised" (where to paint ignore areas), per camera,
  24 h / 7 / 30 days, smoothed, with ignore areas drawn on top.
* **Floor plan**: image or satellite map of the property, camera icons (resizable), view cones, walls, zoom
  and pan, where alerts happened.
* **Daily summary** video per camera (everything recognised played together, choose objects, speed, length,
  schedule, make-all-at-once, "open footage" links when paused); only the latest per camera is kept.
* Recording, detection and alerts never read any of the above: a broken AI never delays recording.

### Notifications and integrations
* ntfy push (picture straight away, summary when the alert ends, high priority for suspicious activity),
  email, daily digest, MQTT (with Home Assistant discovery), webhooks, text-to-speech announcements, siren.
* Modes (home / away / night, privacy) that change what notifies and records.

### System, accounts, security
* CPU/GPU/memory history, task status (archive moves, summaries, conversions), log viewer with a playback
  report log, updates from GitHub releases (Windows self-update; every open page reloads after an update or
  restart), only administrators can restart.
* Accounts: admin, operator, viewer; restricted accounts limited to chosen cameras (check by camera id on
  every route and every list that returns camera data); sessions, remember-me, idle logout; https with the
  user's own certificate (Tailscale) or a self-made one.
* Security rules: escape all text put into the page, no shell strings, stream-address allow-list, CSP and
  security headers, rate-limit sign-in, review every route that takes a camera id (a test must fail for a
  new unreviewed one).
* Settings export/import. Day and night themes using colour tokens only (no hard-coded colours, no second
  rule for a selector). Per-browser preferences in `localStorage`, never sent to the server.

## Quirks and bugs of the old app to avoid

* Bulk preview-picture downloads starved video playlists ("continuous playback off … manifestLoadTimeOut")
  so the Recordings page stopped loading. (Fixed in 1.7.3 by limiting pages fetched ahead; design it right
  from the start.)
* Files far longer than the segment setting (cause unknown) made previews of 37 pages and seeks hours into a
  file.
* A single 11,000-line page script and a 7,000-line `web.rs`; 84 duplicated CSS selectors; settings that
  were exposed but obsolete (`fullscreen_corners`, `fullscreen_tiles`, `package_area`); two overlapping
  "NAS" concepts (Backup copies, Deep storage moves) with similar pages; ten abandoned alternative
  "looks"; 72 global mutable statics; 38 timers, five polling the server.
* Pages and recordings only tested by a no-camera smoke test. Build tests with simulated cameras
  (MediaMTX + looped test video) and real behaviour tests for playback, deep storage and restricted accounts.

## How to work

1. **Plan before code.** Produce a short architecture document (modules, data formats, API, how a file moves
   from camera → disk → deep storage → browser) and wait for approval.
2. **Build in vertical slices**, each releasable and tested: (1) record to disk reliably (crash repair,
   retention); (2) Recordings page with HLS playback of local files; (3) deep storage and its playback;
   (4) Live view; (5) detection, alerts, notifications; (6) accounts and security; (7) everything else above.
3. **Measure before optimising** (simulated cameras, `curl`, browser network panel). State what was tested
   and what was not.
4. Keep the documentation in the same change: architecture, changelog, a technical-debt list (add what you
   leave undone, delete what you fix), a configuration inventory.
5. Never silently drop a user's saved value; provide a migration from the old `nvr.toml`, and **keep the
   existing recordings on disk playable** (same folder layout, file names and fragmented MP4 files) so
   nothing the user already recorded is stranded.
6. Check pages in a browser in both day and night themes.
