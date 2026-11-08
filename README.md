# NVR

A small, self-hosted network video recorder for home surveillance, written in Rust.
It records one IP camera around the clock and gives you a web page to watch it live,
browse recordings, and play them back.

This is a deliberately simple first version for **one camera**. It's meant to grow over time.

## Features

- **Continuous recording** of the camera's RTSP stream into MP4 files (5 minutes each by default).
  The video is copied as-is, not re-encoded, so CPU use stays very low.
- **Survives power cuts:** files are fragmented MP4, so a file that was being written when
  the power went out can still be played.
- **Auto-reconnect:** if the camera drops off the network or the stream stalls, the NVR
  retries with backoff and shows the error in the UI.
- **Live view** in the browser (MJPEG). It only runs while someone is watching, and you
  can point it at the camera's low-resolution sub-stream.
- **Playback, download and delete** of recordings, grouped by day.
- **Snapshot** button that saves a JPEG.
- **Retention:** recordings are deleted automatically after N days and/or once they pass
  a storage limit.
- **Optional login** (HTTP basic auth).
- Shows disk usage and camera status.

## Requirements

- [Rust](https://rustup.rs) (to build)
- [ffmpeg](https://ffmpeg.org) on the `PATH`, e.g. `sudo apt install ffmpeg`

## Getting started

```bash
cargo build --release
```

```bash
./target/release/nvr
```

On first run it writes an example `nvr.toml` and exits. Edit it and set your camera's
RTSP URL:

```toml
[camera]
name = "Front Door"
url = "rtsp://admin:password@192.168.1.50:554/stream1"
live_url = "rtsp://admin:password@192.168.1.50:554/stream2"   # optional sub-stream
```

Run it again and open <http://localhost:8080>. To use a different config file, pass its
path: `nvr /etc/nvr.toml`.

All options are documented in [`nvr.example.toml`](nvr.example.toml).

### Finding your camera's RTSP URL

It depends on the brand. Some common ones:

| Brand | Main stream | Sub-stream |
|---|---|---|
| Hikvision | `rtsp://user:pass@IP:554/Streaming/Channels/101` | `.../102` |
| Dahua / Amcrest | `rtsp://user:pass@IP:554/cam/realmonitor?channel=1&subtype=0` | `subtype=1` |
| Reolink | `rtsp://user:pass@IP:554/h264Preview_01_main` | `..._sub` |
| TP-Link Tapo | `rtsp://user:pass@IP:554/stream1` | `stream2` |

You can check a URL with `ffplay "rtsp://..."` before putting it in the config.

### Testing without a camera

`camera.url` can also be a local video file. It is looped in real time, as if it were a camera:

```bash
ffmpeg -f lavfi -i testsrc2=size=1280x720:rate=15 -t 30 -c:v libx264 -pix_fmt yuv420p test.mp4
```

Then set `url = "test.mp4"`.

## Notes

- **H.265/HEVC cameras:** recording works, but many browsers can't play H.265, so in-browser
  playback may fail. Download the file and play it in VLC, or switch the camera to H.264.
- Recording uses the main stream, so recordings are full quality even if the live view uses the sub-stream.
- Stop the NVR with Ctrl+C (or SIGTERM). It closes the current file cleanly before exiting.
- Basic auth sends the password in a reversible form. Keep the NVR on your home network,
  or put it behind HTTPS (e.g. a reverse proxy) before exposing it to the internet.

## How it works

```
camera ──RTSP──► ffmpeg (copy) ──► recordings/front-door_2026-11-08_14-05-00.mp4 ...
       ──RTSP──► ffmpeg (MJPEG) ──► broadcast ──► browsers (only while viewed)
```

| File | Purpose |
|---|---|
| `src/main.rs` | Startup, shutdown |
| `src/config.rs` | `nvr.toml` loading and defaults |
| `src/recorder.rs` | Supervises the recording ffmpeg process: start/stop, reconnect, stall detection |
| `src/live.rs` | Shared on-demand live MJPEG stream and snapshots |
| `src/storage.rs` | Listing recordings, retention clean-up, disk space |
| `src/web.rs` | HTTP routes and auth |
| `static/index.html` | The web interface (embedded in the binary) |

### HTTP API

| Method | Path | Description |
|---|---|---|
| GET | `/api/status` | Recorder state, storage, camera info |
| POST | `/api/record/start` / `/api/record/stop` | Turn recording on/off |
| GET | `/api/recordings` | List of recordings, newest first |
| DELETE | `/api/recordings/{name}` | Delete a recording |
| GET | `/recordings/{name}` | Play a file (supports seeking); add `?download=1` to download |
| GET | `/live.mjpeg` | Live view |
| GET | `/snapshot.jpg` | Current frame as JPEG |

## Ideas for next steps

Multiple cameras, motion detection with event markers on a timeline, settings page in the UI,
PTZ control via ONVIF, notifications, and running as a systemd service.
