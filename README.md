# NVR

A self-hosted network video recorder for home surveillance, written in Rust. It records your IP
cameras, spots people, vehicles and animals with AI, and gives you a fast web interface for live
viewing, finding footage, and managing everything, all from the browser.

## Features

**Live view**
- Dashboard of all cameras that you can **drag, drop and resize** (Live → Edit layout).
- **Camera groups** (Live → Groups…): switch between sets of cameras, or show all.
- **Full-screen video wall**: fills the whole screen, with its own layout per group, click a camera to
  enlarge it (HD), arrow keys to switch groups, and optional automatic cycling through groups.
- Always running: grid pictures come from each camera's sub-stream over one WebSocket and keep
  updating while you use other pages, so switching back is instant.
- Full-quality streams are **preloaded in the background**: opening a camera shows HD immediately.
- Click a camera for a big view with its **recent alerts** beside it (recordings are on the timeline), a **timeline** under it
  with symbols for what was detected (🚶 person, 🚗 car, 🐈 cat …), **quick quality / frame-rate options**,
  an **Edit camera** button, and **pan / tilt / zoom** controls with presets (ONVIF).
- Overlays you can configure globally or per camera (camera settings → On-screen display): camera name,
  **date & time** (format and corner), recording status, what is moving, **frames per second**, your own
  text, and **fullscreen buttons** in any corners.
- Active alerts are highlighted on the tile ("person", "car", …).
- **S1 / S2 button on every live picture** switches between stream 1 (main, full quality) and stream 2
  (sub-stream), separately for the grid and the opened camera; each browser remembers the choice.

**Recording**
- Per-camera modes, like Blue Iris:
  - **HD 24/7**
  - **Sub-stream 24/7** (saves space)
  - **Sub-stream 24/7 + HD clips on alerts**
  - **HD on alerts only**
  - **Off**
- **Pre-event buffer**: in alert modes, full-quality video is buffered continuously (30 s by default),
  so every alert clip includes what happened *before* it. Video that doesn't contain an alert is discarded.
- **File length** per camera: 5 min, 15 min, 30 min, 1 h … or one long file per day.
- Recording frame rate, audio (on by default), H.264 conversion, and schedules.
- **Storage folder per camera**: another drive, a NAS (`\\NAS\cameras`, `/mnt/nas`) or a synced cloud
  folder (Google Drive / OneDrive for desktop).
- Per-camera **retention days** and **GB limit**, plus global limits and a minimum free disk space.
- **Choose where recordings go** with a folder browser (any drive, NAS share or synced cloud folder),
  optionally moving existing recordings there.
- **Cloud backup** with rclone (Google Drive, OneDrive, Dropbox, S3, B2, …): only verified AI alerts,
  all alerts, or everything.
- **Crash and power-cut safe**: recordings are written in small self-contained pieces; after an
  unexpected stop the NVR cuts any incomplete piece off the end so every file plays. Alert clips are
  written under a temporary name and only appear when complete. Cameras reconnect automatically.
- **Recording profiles**: save a camera's recording, motion and AI settings as a named profile and apply
  it to other cameras in one click.

**Motion detection & AI**
- Blue Iris-style motion detection: **min. contrast**, **min. object size** in pixels, **make time**,
  **object must travel**, and **areas to ignore** painted on the picture. A live readout shows how big
  the moving objects are, and *Show motion* draws boxes around them. Lighting changes are ignored.
- AI checks run **only while there is motion**: no motion, nothing is sent to the AI.
- Object recognition, with a choice of AI:
  - **Built-in**: a YOLOX model runs on this computer's CPU. It's free and private, and the model downloads automatically.
  - **AI server on your network**: CodeProject.AI Server or DeepStack, which can use another computer's GPU.
  - **Ollama**: a local vision model such as qwen2.5vl or llava.
  - **Google Gemini**: has a free tier; needs an API key.
  - **Any OpenAI-compatible API**: OpenAI, OpenRouter, Groq, LM Studio, …
- Choose which objects alert (person, car, dog, …), the minimum confidence, and whether *any motion*
  or *only recognized objects* make an alert.
- Each alert gets a **thumbnail of the whole picture** that zooms in on the recognized object.
- **Detection boxes** on alert pictures (green for recognized objects with their label and confidence,
  dashed orange for motion) so you can check what triggered an alert; switch them off in the alert details.
- Damaged video frames (lost network packets, green, pink or smeared pictures) are skipped, so they don't
  cause false alerts or broken thumbnails.
- If the same object is recognized as two things (a cat also seen as a person), only the more
  confident answer is kept.
- Each recognized object gets a **colour**; language-model AIs also add a short description.
- "Test on this camera" shows the camera's picture with detection boxes.

**Recordings**
- **Calendar** to pick a single day or a range, optionally limited to a time of day.
- Filter by **camera(s)** and **alert type** (tick any of Person, Car, Truck, Dog, Motion, Recordings, …).
- **Search** alerts by what was seen: "red van", "white car", "black cat", "person", or words from the
  AI's description.
- Cards with **previews**: the alert thumbnail, or a frame from the recording.
- Player with a **scrub bar that previews frames as you move along it**, and fast seeking.
- **Clip export**: mark a start and end (buttons or drag the handles) and download just that part.
- **Full-screen mode** for browsing recordings.
- **Multi-camera timeline**: watch several cameras at the same moment (files play on without gaps), scrub across all of them at once
  (each shows a quick time-lapse preview), jump between alerts, and export the same time range from each camera.
  Click an alert symbol to watch that alert on that camera. Legends explain the colours and symbols.
- Time-lapse previews for the last 24 hours are prepared in the background, so they're ready when you
  open the timeline (older days are prepared when you open them).

**System & logs**
- **CPU, GPU and memory** usage with graphs (1 hour to 30 days, kept across restarts), so you can see if
  the server was overloaded while you were away. NVIDIA GPUs are read via nvidia-smi, AMD on Linux via
  sysfs, and any GPU on Windows via performance counters.
- **Disk space** for every storage location, **MB/GB recorded per hour and per day** for each camera, and
  an estimate of **how many days of recording fit** with your current settings.
- **Logs page**: errors, warnings, alerts and camera events, filterable by level, type, camera and text.
  Also saved to `nvr-data/nvr.log`.
- Click the graph's legend to show or hide lines; memory used by the NVR is shown next to the computer's.
- **Smart CPU/GPU use**: motion-detection decoding moves to the GPU when the CPU stays overloaded (and
  back), the built-in AI slows down under heavy load, and background jobs run at low priority.

**Setup**
- Add, edit, **clone** and delete cameras in the browser.
- **Change the camera's own settings over ONVIF** (camera editor → Camera settings): IP address / DHCP,
  subnet, gateway, DNS and host name (the NVR follows the camera to its new address), time server, time
  zone and clock sync, each stream's resolution, frame rate, bitrate, keyframe interval and H.264/H.265,
  brightness / contrast / colour / sharpness and day/night mode, and restart. What a camera doesn't
  support over ONVIF is shown as not available.
- **Camera brand quick setup**: pick the brand, enter the IP and login, and the main and sub-stream URLs
  are filled in for you. 15 common formats are included.
- **Test connection** plays both streams live and shows their codec, resolution and frame rate.
- **Export / import your whole setup** (cameras, settings, layout) to restore after a reinstall.
- GPU acceleration (NVIDIA, Intel, AMD, Apple) with automatic CPU fallback.
- Optional login.
- **Phone friendly**: bottom tab bar, large buttons, and the camera picture stays on screen while you scroll.
- **Data saver for remote viewing** (Tailscale, VPN, the internet): smaller live pictures at fewer frames
  per second, no background streams, and recordings streamed as light 480p video (about 0.7 Mbit/s).
  *Auto* turns it on whenever the browser connects from outside the home network; the 📶 button in the
  menu switches between Auto, On and Off for that browser.

## Install

You need [Rust](https://rustup.rs) and [ffmpeg](https://ffmpeg.org) (with ffprobe).

**Windows** (PowerShell; accept the Visual Studio Build Tools when the Rust installer asks):
```bash
winget install Git.Git Rustlang.Rustup Gyan.FFmpeg
```

**Linux (Ubuntu/Debian/Raspberry Pi OS)**:
```bash
sudo apt update && sudo apt install -y git ffmpeg build-essential curl
```
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

**macOS**:
```bash
brew install git ffmpeg rustup && rustup-init
```

Then, in a new terminal:
```bash
git clone https://github.com/impulsenz/NVR.git
```
```bash
cd NVR
```
```bash
cargo build --release
```

## Run

```bash
./target/release/nvr
```
(Windows: `.\target\release\nvr.exe`)

Open <http://localhost:8080> (or `http://<this-computer's-IP>:8080` from a phone) and go to
**Settings → Cameras → Add camera**.

Files are created next to where you start it:
- `nvr.toml`: all settings (normally changed from the web page)
- `nvr-data/`: logs, system history, AI models
- `recordings/<camera>/`: recordings; `events/` inside holds alert clips and thumbnails

### Getting started tips

1. Add each camera with both its **main stream and sub-stream**. The sub-stream is used for the grid,
   motion detection and AI, and saves a lot of CPU.
2. For storage-friendly recording, use **Sub-stream 24/7 + HD on alerts**.
3. Turn on AI: **Settings → Object recognition → Built-in**, then tick *Object recognition* on each camera
   (*Motion & AI* tab). Choose **Only recognized objects** to ignore trees, shadows and rain.
4. Check **System** after a day to see how much space each camera uses and how many days fit.

### Testing without a camera

A video file works as a camera URL (it plays in a loop):
```bash
ffmpeg -f lavfi -i testsrc2=size=1280x720:rate=15 -t 30 -c:v libx264 -pix_fmt yuv420p test.mp4
```

## Notes

- **Playing recordings on other devices:** if a browser can't play the camera's format (usually H.265),
  the NVR converts the recording to H.264 on the fly while it plays (using the GPU if set up). H.265 files
  are tagged so Apple devices (Safari) can play them directly.
- **GPU/CPU use:** each camera's sub-stream is decoded once (for motion detection, and reused for the
  live grid); a full-quality stream is decoded only while you watch that camera. Background HD preloading
  is off by default (Settings → Live view).
- **Gemini:** leave the model empty to use the newest Flash model automatically, or use *List models*.

- **One connection per camera stream:** the NVR connects to each stream once and shares it between
  recording, the alert buffer, motion detection, AI and the live view. Many cameras (TP-Link Tapo, many
  Hikvision/Dahua models) only allow 2–3 connections at a time and refuse more with errors like
  *453 Not Enough Bandwidth*. If you still see that, close other apps that view the camera (the
  manufacturer's app, another NVR, VLC) or restart the camera.

- **H.265 cameras:** recording works, but not every browser plays H.265. Turn on *Convert recordings to
  H.264* for the camera, or download the file and use VLC. Previews and thumbnails work either way.
- **Cloud AI** (Gemini, OpenAI…) sends camera pictures to that service. Set a requests-per-minute limit for
  free tiers. The built-in AI and local servers keep everything at home.
- **Security:** camera passwords are visible in camera settings. Turn on the login if others use your
  network, and use HTTPS (e.g. a reverse proxy) before exposing it to the internet.
- **Upgrading:** older `nvr.toml` files are converted automatically.

## How it works

```
camera ─ main stream ─► relay (1 connection) ─┬► 24/7 files
                                              ├► .buffer/ (rolling) ─► alert clips
                                              └► JPEG ─► WebSocket ─► browser (HD view)
       └ sub-stream ──► relay (1 connection) ─┬► RGB 5 fps ─► motion detector ─► AI ─► alerts + thumbnails
                                              └► JPEG ─► WebSocket ─► browser grid
```

| File | Purpose |
|---|---|
| `src/manager.rs` | Running cameras; applies and saves changes; usage estimates |
| `src/recorder.rs` | Recording processes: schedule, reconnect, stall detection, GPU fallback |
| `src/buffer.rs` | Pre-event buffer and alert clips |
| `src/detect.rs` | Motion detection and event lifecycle |
| `src/ai.rs` | AI providers (built-in YOLOX, CodeProject.AI/DeepStack, Ollama, Gemini, OpenAI-compatible) |
| `src/events.rs` | Alert storage |
| `src/relay.rs` | One shared connection per camera stream, fanned out to all users |
| `src/live.rs` | Live JPEG streams |
| `src/library.rs` | Recording search, timelines, export ranges |
| `src/storage.rs` | Files, usage, retention |
| `src/sysmon.rs` | CPU/GPU/RAM monitor and history |
| `src/logs.rs` | Log page and log file |
| `src/cloud.rs` | rclone backup |
| `src/ffmpeg.rs` | ffmpeg commands, GPU options, previews, export |
| `src/web.rs` | HTTP API and WebSocket |
| `static/` | Web interface (built into the program) |
