#!/bin/bash
# Usage: cams.sh N — publish N simulated cameras (main + sub) to the local RTSP server.
: "${BENCH:?set BENCH to a scratch folder}"
N=${1:-1}
pkill -f "^ffmpeg -loglevel error -re -stream_loop" 2>/dev/null; sleep 0.5
for i in $(seq 1 "$N"); do
  nohup ffmpeg -loglevel error -re -stream_loop -1 -i "$BENCH/main.mp4" -c copy -f rtsp "rtsp://127.0.0.1:8554/cam$i" >/dev/null 2>&1 &
  nohup ffmpeg -loglevel error -re -stream_loop -1 -i "$BENCH/sub.mp4" -c copy -f rtsp "rtsp://127.0.0.1:8554/cam${i}sub" >/dev/null 2>&1 &
done
