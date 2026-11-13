# GAZE

GAZE is a network video recorder for home IP cameras. It records your cameras around the clock or on
alerts, uses on-device object detection to tell people, vehicles and animals apart from swaying trees
and passing headlights, and gives you one web interface for live viewing, reviewing footage and
managing the system.

It is a single program written in Rust. It runs on Windows (where it sits in the system tray and can run
as a service), and also builds on Linux and macOS. Everything runs locally: no account, no cloud service,
and no pictures leave the machine unless you turn on phone notifications or cloud backup.

## Highlights

**Live view.** A resizable grid of cameras with groups and a full-screen video wall. The grid uses each
camera's sub-stream, so it stays light; opening a camera switches to the full-quality stream. Each camera
can show a configurable on-screen display, pan/tilt/zoom controls and presets (ONVIF), and a push-to-talk
button for cameras with a speaker.

**Recording.** Per camera: main stream 24/7, sub-stream 24/7, sub-stream 24/7 with full-quality alert clips,
alert clips only, or off. Alert clips include a pre-event buffer so you see what happened before the
trigger. Recordings are written as fragmented MP4, so a power cut costs at most the last second or two.
Each camera can have its own storage folder (another drive, a NAS share, a synced cloud folder),
retention period and size limit.

**Detection.** Motion detection with minimum size, contrast, travel and painted ignore areas, followed by
object detection (YOLOX, run on the graphics card through DirectML or on the CPU). New finds can be
checked against a full-quality picture before they count. Other options:

- zones, line crossing and loitering;
- parked vehicles that stay quiet until they move;
- a painted package spot that reports parcels left or taken (the change has to stay for a minute);
- faces and number plates read from the HD footage, so alerts can say who or whose car it was.

**Reviewing footage.** A multi-camera timeline shows recordings, alerts (as thumbnails) and a time-lapse
strip:
- Drag across it to scrub every camera at once using the time-lapse pictures.
- Let go to play from that moment.
- Playback is streamed in small indexed pieces (HLS), so it starts quickly and plays across file
  boundaries without reloading.

An alert list with filters, a calendar and per-alert detail sits alongside. Clips can be marked and
downloaded from any player, including in full screen.

**Notifications.** Optional push notifications through the free ntfy app, with the alert picture
straight away and a summary when the alert ends. Suspicious activity (line crossings, lingering, items
taken, people at night) is sent with high priority.

**System.** CPU, GPU and memory history, disk usage broken down by kind of footage, an estimate of how many
days of recording fit, a log viewer, background task status, and self-updating from GitHub releases.

## Requirements

- [ffmpeg](https://ffmpeg.org) with ffprobe on the PATH (on Windows: `winget install Gyan.FFmpeg`).
- IP cameras with RTSP. A sub-stream is strongly recommended: it is used for the grid, motion detection
  and the AI, and keeps CPU use low.
- For GPU object detection on Windows, any reasonably recent NVIDIA, AMD or Intel graphics. The ONNX
  Runtime (about 25 MB) is downloaded the first time it's needed.

## Installing

### Windows

Download `GAZE.exe` from the [latest release](https://github.com/impulsenz/NVR/releases/latest),
put it in a folder of its own and run it. GAZE starts in the system tray; click the icon to open it in
your browser. After that it can update itself (Settings → About).

### Building from source

You need [Rust](https://rustup.rs) and ffmpeg.

```bash
git clone https://github.com/impulsenz/NVR.git
```
```bash
cd NVR
```
```bash
cargo build --release
```

The program is `target/release/nvr` (`nvr.exe` on Windows). Run it from the folder where you want its
settings and recordings to live.

## First run

Open <http://localhost:8080> (or `http://<computer-ip>:8080` from another device) and create the admin
account. Then go to **Settings → Cameras → Add camera**: pick the brand, enter the IP address and login,
and the stream addresses are filled in. **Test connection** shows both streams with their codec and
resolution.

A setup that works well for most homes:

1. Give every camera both its main stream and its sub-stream.
2. Use **Sub-stream 24/7 plus HD AI alerts** as the recording mode. It keeps continuous footage small and
   alert footage sharp.
3. Turn on object detection (**Settings → Object recognition**), then tick the objects you care about on
   each camera's AI tab.
4. Paint ignore areas over trees, roads and anything else that moves but doesn't matter.
5. After a day, check **System** to see how much space each camera uses and how many days fit.

## Files

GAZE keeps everything next to the program:

| Path | Contents |
|---|---|
| `nvr.toml` | Settings. Normally edited from the web interface; can be exported and imported. |
| `nvr-data/` | Accounts, logs, system history, downloaded AI models. |
| `recordings/<camera>/` | Recordings, with alert clips and pictures in `events/` and timeline copies in `timeline/`. |

## Notes

- **Camera connections.** GAZE opens one connection per camera stream and shares it between recording,
  detection and live view. Many cameras only accept two or three connections at a time. If one reports
  "Not Enough Bandwidth", close other apps that are viewing it.
- **H.265.** Not every browser can play H.265. Where a browser can't, GAZE converts on the fly for
  playback. Recording and detection are unaffected.
- **Security.** Use the login, and put GAZE behind HTTPS (a reverse proxy, or a VPN such as Tailscale)
  before exposing it outside your network. The microphone for push-to-talk only works over HTTPS or on
  localhost.
- **Updates** are described in [UPDATES.md](UPDATES.md), including how to build and publish your own
  releases.

## Testing without a camera

A video file works as a camera address and plays in a loop:

```bash
ffmpeg -f lavfi -i testsrc2=size=1280x720:rate=15 -t 30 -c:v libx264 -pix_fmt yuv420p test.mp4
```

## How it's put together

```
camera ─ main stream ─► relay ─┬► recorder (24/7 files)
                               ├► rolling buffer ─► alert clips
                               └► live view / HD checks
       └ sub-stream ──► relay ─┬► decoder (a few fps) ─► motion ─► object detection ─► alerts
                               └► live grid
```

| Source | Role |
|---|---|
| `src/manager.rs` | Starts and supervises everything per camera; applies setting changes. |
| `src/relay.rs` | One connection per camera stream, shared by every consumer. |
| `src/recorder.rs`, `src/buffer.rs` | Continuous recording, the pre-event buffer and alert clips. |
| `src/detect.rs`, `src/ai.rs`, `src/infer.rs` | Motion detection, object detection, the alert lifecycle. |
| `src/identify.rs`, `src/recog.rs`, `src/known.rs` | Faces and number plates, and the people and vehicles you've named. |
| `src/zones.rs`, `src/stationary.rs` | Zones, line crossing, loitering, items left or taken, parked vehicles. |
| `src/events.rs`, `src/notify.rs` | Alert storage and phone notifications. |
| `src/library.rs`, `src/hls.rs` | Timelines, and playback of recordings as indexed HLS streams. |
| `src/storage.rs`, `src/finalize.rs`, `src/cloud.rs` | Files, retention, repair and cloud backup. |
| `src/web.rs`, `static/` | The HTTP API and the web interface (built into the program). |
| `src/sysmon.rs`, `src/logs.rs`, `src/update.rs` | System monitoring, logging and self-updates. |

`static/vendor/hls.min.js` is [hls.js](https://github.com/video-dev/hls.js) 1.5.20, used under the Apache
License 2.0 (see `static/vendor/hls.js-LICENSE.txt`).
