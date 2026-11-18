# GAZE on Linux

GAZE builds and runs on 64-bit Linux. The Windows extras (tray icon, Windows
service, watchdog, self-update) don't apply; use systemd (or Docker) to keep
it running and your package manager for ffmpeg.

**Status:** CI (`.github/workflows/linux.yml`) runs the tests, the page smoke
test, builds the Docker image and starts it, on every change. The systemd
unit was checked with `systemd-analyze verify` but has not been run under its
own `gaze` user: report anything that needs fixing.

## What you need

* ffmpeg (and ffprobe) on the PATH: `sudo apt install ffmpeg`.
* Optional: `smartmontools` for Settings → System → Drive health (smartctl
  needs root, or run GAZE as root / give the container the drives).

## Build

```bash
cargo build --release          # target/release/nvr
```

Or, from the next release on, download `GAZE-linux-x86_64` (and its
`.sha256`) from the GitHub releases page. GAZE does not update itself on
Linux: replace the file, or pull a new image.

## Run as a service (systemd)

See the commands at the top of `packaging/gaze.service`. Settings, accounts
and recordings live in the folder given with `--dir` (`/var/lib/gaze`);
choose other drives for recordings in Settings → Recording & storage and make
those folders writable by the `gaze` user. Web interface: `http://<computer>:8080`.

## Run in Docker

```bash
docker build -f packaging/Dockerfile -t gaze .
docker run -d --name gaze --restart unless-stopped -p 8080:8080 -p 8443:8443 -v gaze-data:/data gaze
```

For recordings on a big drive, mount it (`-v /mnt/cams:/data/recordings`).
Cameras must be reachable from the container (host networking,
`--network host`, is simplest for ONVIF discovery).

## AI acceleration

Settings → Object recognition → *Graphics API*. *Automatic* uses
**OpenVINO**, which runs on Intel graphics (iGPU, Arc), an Intel NPU, or
simply the processor, faster than the built-in engine. It downloads once
(about 85 MB) and is checked against a pinned checksum.

* **Docker + Intel graphics:** add `--device /dev/dri` (and the host's
  `render` group, e.g. `--group-add $(stat -c %g /dev/dri/renderD128)`).
* **NVIDIA (CUDA):** needs the NVIDIA driver plus the CUDA 12 and cuDNN 9
  libraries on the computer. *Automatic* picks CUDA only when it finds them,
  otherwise OpenVINO. The Debian-based image above does not contain CUDA/cuDNN;
  use a CUDA runtime base image with `--gpus all` if you want it.
* CUDA, Intel graphics and the NPU paths have not been run on real hardware
  (see `docs/TECH-DEBT.md`, A-1). OpenVINO on a processor has.
