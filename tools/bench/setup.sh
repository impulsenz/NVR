#!/bin/bash
# Test videos, GAZE settings and the stand-in cameras' RTSP server.
# Needs $BENCH (a scratch folder) and mediamtx on the PATH (or $MEDIAMTX).
set -e
: "${BENCH:?set BENCH to a scratch folder}"
mkdir -p "$BENCH/run"
cd "$BENCH"
[ -f main.mp4 ] || ffmpeg -loglevel error -y -f lavfi -i "testsrc2=size=2560x1440:rate=20" \
  -f lavfi -i "sine=frequency=440:sample_rate=16000" -t 30 -c:v libx264 -preset veryfast \
  -tune zerolatency -g 40 -b:v 4M -pix_fmt yuv420p -c:a aac -b:a 32k -shortest main.mp4
[ -f sub.mp4 ] || ffmpeg -loglevel error -y -f lavfi -i "testsrc2=size=640x360:rate=15" -t 30 \
  -c:v libx264 -preset veryfast -g 30 -b:v 400k -pix_fmt yuv420p sub.mp4
[ -f run/nvr.toml ] || printf '[server]\nlisten = "127.0.0.1:18080"\nhttps_port = 0\n[storage]\ndir = "rec"\nmax_storage_gb = 3.0\n' > run/nvr.toml
pgrep -f "${MEDIAMTX:-mediamtx}" >/dev/null || (nohup "${MEDIAMTX:-mediamtx}" >/dev/null 2>&1 &)
echo "ready: $BENCH"
