# GAZE on Linux

GAZE builds and runs on 64-bit Linux. The Windows extras (tray icon, Windows
service, watchdog, self-update) don't apply; use systemd (or Docker) to keep
it running and your package manager for ffmpeg.

**Status:** the program was run on Linux for this guide (started with
`--dir`, web interface up, files created in that folder). The systemd unit
was checked with `systemd-analyze verify`. The **Dockerfile has not been
built** (no Docker daemon was available) and the unit has not been run under
its own `gaze` user: report anything that needs fixing.

## What you need

* ffmpeg (and ffprobe) on the PATH: `sudo apt install ffmpeg`.
* Optional: `smartmontools` for Settings → System → Drive health (smartctl
  needs root, or run GAZE as root / give the container the drives).

## Build

```bash
cargo build --release          # target/release/nvr
```

There is no Linux download in the GitHub releases yet (the release workflow
builds Windows only).

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
