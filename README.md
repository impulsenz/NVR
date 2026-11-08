# NVR

A small, self-hosted network video recorder for home surveillance, written in Rust.
It records IP cameras around the clock (or on a schedule) and gives you a web interface
to watch them live, browse and play recordings, and manage everything, including adding
cameras, from the browser.

## Features

**Cameras**
- Add, edit and delete cameras from the web interface. Changes take effect immediately.
- **Test connection** button: shows a preview image plus the stream's codec, resolution and frame rate.
- Main stream for recording, optional sub-stream for the live view.
- RTSP over TCP or UDP.

**Recording**
- Per camera: **Always (24/7)**, **On a schedule** (days of the week + time window, overnight supported), or **Off**.
- Pause/resume recording from the live view.
- Video is copied as-is by default (very low CPU), or optionally **converted to H.264** so H.265 cameras play in any browser.
- Files survive power cuts (fragmented MP4), and recording auto-reconnects with backoff if a camera drops out or stalls.
- Optional audio.

**Storage**
- Configurable file length.
- Automatic clean-up by age, by total size, and when free disk space runs low. The oldest recordings go first.

**GPU acceleration**
- Hardware video decoding for the live view, and hardware H.264 encoding for cameras that convert recordings.
- NVIDIA (CUDA/NVENC), Intel Quick Sync, VAAPI (Intel/AMD on Linux), Apple VideoToolbox, and Direct3D/AMF/Media Foundation on Windows.
- Shows which GPU options your ffmpeg supports, and a **Test** button checks the hardware really works.
- If the GPU fails, that camera **falls back to the CPU automatically** and the UI says why.

**Viewing**
- Live grid of all cameras, and a full-size single-camera view with snapshot and fullscreen.
- Recordings browser by camera and day, with playback, seeking, download and delete.
- Works on phones.

**System**
- NVR name, web port, optional login (HTTP basic auth), ffmpeg location.
- System info page: version, ffmpeg build, GPU capabilities, disk usage.

## Requirements

- [Rust](https://rustup.rs) (to build)
- [ffmpeg](https://ffmpeg.org), including `ffprobe` (normally installed together)

See the install steps below for your system.

## Install

**Linux (Ubuntu/Debian/Raspberry Pi OS)**
```bash
sudo apt update && sudo apt install -y git ffmpeg build-essential curl
```
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

**macOS**
```bash
brew install git ffmpeg rustup && rustup-init
```

**Windows (PowerShell)**
```bash
winget install Git.Git Rustlang.Rustup Gyan.FFmpeg
```

Then open a new terminal and build:

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

Open <http://localhost:8080> (or `http://<this-computer's-IP>:8080` from another device), go to
**Settings → Cameras → Add camera**, paste your camera's RTSP URL, and click **Test connection**.

On first run, `nvr.toml` is created next to where you started it, and recordings go into
`recordings/<camera>/`. To use a different config file: `nvr /path/to/nvr.toml`.
Stop with Ctrl+C. The current file is closed cleanly.

### Camera stream URLs

| Brand | Main stream | Sub-stream |
|---|---|---|
| Hikvision | `rtsp://user:pass@IP:554/Streaming/Channels/101` | `.../102` |
| Dahua / Amcrest | `rtsp://user:pass@IP:554/cam/realmonitor?channel=1&subtype=0` | `subtype=1` |
| Reolink | `rtsp://user:pass@IP:554/h264Preview_01_main` | `..._sub` |
| TP-Link Tapo | `rtsp://user:pass@IP:554/stream1` | `stream2` |

Many cameras need RTSP turned on in their app first; Tapo also needs a separate "camera account".

### Testing without a camera

A local video file works as a camera URL (it loops in real time):

```bash
ffmpeg -f lavfi -i testsrc2=size=1280x720:rate=15 -t 30 -c:v libx264 -pix_fmt yuv420p test.mp4
```

Add a camera with the URL `test.mp4` (or its full path).

## GPU acceleration

Normal recording copies the camera's video without decoding it, so it needs no GPU.
The GPU is used for:

1. **Live view:** the camera stream is decoded to show it in the browser.
2. **Convert recordings to H.264** (per-camera option): the stream is decoded and re-encoded.

In **Settings → GPU acceleration**, pick the decoder and encoder for your hardware, then click **Test**:

| Hardware | Decoding | Encoder |
|---|---|---|
| NVIDIA | NVIDIA (CUDA) | NVIDIA NVENC |
| Intel (any OS) | Intel Quick Sync | Intel Quick Sync |
| Intel/AMD on Linux | VAAPI | VAAPI |
| Mac | Apple VideoToolbox | Apple VideoToolbox |
| Windows, AMD | Direct3D 11 | AMD AMF |

The GPU must be supported by your ffmpeg build and its drivers must be installed (for NVIDIA, the
proprietary driver; for VAAPI, e.g. `intel-media-va-driver` or `mesa-va-drivers`). Options your
ffmpeg doesn't support are greyed out.

## Notes

- **Security:** Settings show camera URLs including their passwords. Turn on the login
  (Settings → System & login) if others use your network. The login is HTTP basic auth, so put
  the NVR behind HTTPS (e.g. a reverse proxy) before exposing it to the internet.
- **Upgrading from 0.1:** the old single-camera config is converted automatically and existing
  recordings are moved into the camera's folder.
- Changing the web port needs a restart. Changing storage, live view or GPU settings briefly
  restarts the cameras.
- Disk-space reporting (and the "keep free disk space" rule) currently works on Linux and macOS only.

## How it works

```
camera ─RTSP─► ffmpeg (copy, or GPU decode→H.264 encode) ─► recordings/<camera>/<camera>_<time>.mp4
       ─RTSP─► ffmpeg (GPU decode → JPEG) ─► browsers (only while someone is watching)
```

| File | Purpose |
|---|---|
| `src/main.rs` | Startup and shutdown |
| `src/config.rs` | `nvr.toml` format, defaults, validation, schedules |
| `src/manager.rs` | Running cameras; applies and saves changes made in the UI |
| `src/recorder.rs` | Per-camera recording process: schedule, reconnect, stall detection, GPU fallback |
| `src/live.rs` | Per-camera on-demand live stream |
| `src/ffmpeg.rs` | ffmpeg command lines, GPU options, capability detection, connection test |
| `src/storage.rs` | Recording files and retention |
| `src/web.rs` | HTTP API and login |
| `static/index.html` | The web interface (built into the program) |

### HTTP API

| Method | Path | Description |
|---|---|---|
| GET | `/api/status` | All cameras' state and storage |
| GET / PUT | `/api/settings` | General settings |
| GET / POST | `/api/cameras` | List / add cameras |
| PUT / DELETE | `/api/cameras/{id}` | Edit / delete (`?delete_recordings=true`) |
| POST | `/api/cameras/{id}/pause`, `/resume` | Pause/resume recording |
| GET | `/api/cameras/{id}/recordings` | Recordings, newest first |
| DELETE | `/api/cameras/{id}/recordings/{name}` | Delete a recording |
| POST | `/api/probe` | Test a stream URL |
| GET | `/api/system` | Version and GPU capabilities |
| POST | `/api/hw-test` | Test GPU options |
| GET | `/recordings/{id}/{name}` | Play a file (`?download=1` to download) |
| GET | `/live/{id}/stream.mjpeg`, `/live/{id}/frame.jpg` | Live view |
| GET | `/snapshot/{id}` | Full-resolution JPEG |

## Ideas for next steps

Motion detection with events on a timeline, notifications, PTZ control via ONVIF,
camera auto-discovery, user accounts, and running as a system service.
