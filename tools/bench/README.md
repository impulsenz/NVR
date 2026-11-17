# Benchmark: simulated cameras

Measures GAZE (and every process it starts) while it records simulated
cameras, with or without browsers watching the Live page. Linux.

Needs: a built GAZE (`cargo build --release`), ffmpeg, Python 3, Node with
Playwright (for viewers), and [MediaMTX](https://github.com/bluenviron/mediamtx)
as the stand-in cameras' RTSP server.

```bash
export BENCH=/tmp/gaze-bench          # scratch folder
tools/bench/setup.sh                   # test videos + settings, starts MediaMTX
tools/bench/serve.sh                   # starts GAZE on port 18080 (admin / testpass123)
python3 tools/bench/bench.py 8 3 mylabel   # 8 cameras, 3 browsers on Live
```

Each run prints one JSON line (also appended to `$BENCH/bench-results.jsonl`):
CPU of GAZE and its child processes (% of one core), their memory (PSS:
shared libraries split between the processes, so ffmpeg's libraries aren't
counted 30 times), and how many ffmpeg processes run.

Simulated cameras: a 2560×1440 20 fps H.264 main stream (4 Mbit/s, AAC
sound) and a 640×360 15 fps sub-stream, looped. The CPU figure is noisy on a
small machine (the headless browsers share it, and GAZE moves work around
when the CPU is busy): compare process counts and memory first.
