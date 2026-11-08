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
- Click a camera for a big view with its **recent alerts and recordings** beside it. Play them in place.
- Overlays you can configure: camera name, **date & time** (choose the format and corner),
  **frames per second** and stream details, and **fullscreen buttons** in any corners.
- Active alerts are highlighted on the tile ("person", "car", …).

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
- Recordings survive power cuts, and cameras reconnect automatically.

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
- Each alert gets a **thumbnail cropped around the detected object**.
- "Test on this camera" shows the camera's picture with detection boxes.

**Recordings**
- **Calendar** to pick a single day or a range, optionally limited to a time of day.
- Filter by **camera(s)** and **alert type** (tick any of Person, Car, Truck, Dog, Motion, Recordings, …).
- Cards with **previews**: the alert thumbnail, or a frame from the recording.
- Player with a **scrub bar that previews frames as you move along it**, and fast seeking.
- **Clip export**: mark a start and end (buttons or drag the handles) and download just that part.
- **Multi-camera timeline**: watch several cameras at the same moment, scrub across all of them at once
  (each shows a quick time-lapse preview), jump between alerts, and export the same time range from each camera.

**System & logs**
- **CPU, GPU and memory** usage with graphs (1 hour to 30 days, kept across restarts), so you can see if
  the server was overloaded while you were away. NVIDIA GPUs are read via nvidia-smi, AMD on Linux via
  sysfs, and any GPU on Windows via performance counters.
- **Disk space** for every storage location, **MB/GB recorded per hour and per day** for each camera, and
  an estimate of **how many days of recording fit** with your current settings.
- **Logs page**: errors, warnings, alerts and camera events, filterable by level, type, camera and text.
  Also saved to `nvr-data/nvr.log`.

**Setup**
- Add, edit, **clone** and delete cameras in the browser.
- **Camera brand quick setup**: pick the brand, enter the IP and login, and the main and sub-stream URLs
  are filled in for you. 15 common formats are included.
- **Test connection** plays both streams live and shows their codec, resolution and frame rate.
- **Export / import your whole setup** (cameras, settings, layout) to restore after a reinstall.
- GPU acceleration (NVIDIA, Intel, AMD, Apple) with automatic CPU fallback.
- Optional login.

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
