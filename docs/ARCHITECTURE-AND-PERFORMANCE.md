> **Status (November 2026):** this is the record of the 1.0 audit and its
> benchmark figures, which still stand. Some statements are out of date: camera
> folders are now named after the camera (not its id); the page is ~9,800
> lines; recordings can move to deep storage. The current map is
> `ARCHITECTURE.md`; the latest audit is `AUDIT-2026-11.md`.

# GAZE: architecture, performance audit and changes (1.0)

This is the record of the 1.0 audit: how GAZE moves video from the cameras to
the disk and to the browser, what was measured, what was changed, and what
was deliberately left alone. The figures come from the benchmark in
`tools/bench` (simulated cameras on one 4-core Linux machine). Anything that
couldn't be measured there is marked as such.

## 1. The architecture as found

GAZE is one Rust program (axum web server, Tokio), a plain-JavaScript web
page (no framework, ~9,000 lines), and ffmpeg processes it supervises.

**Camera to disk**

```
camera ──RTSP──▶ relay ffmpeg (one per stream: -c:v copy, sound → AAC, MPEG-TS on stdout)
                    │  (GAZE keeps the packets since the last keyframe; bounded queue per consumer)
                    ├──▶ recorder ffmpeg   (-c:v copy → MP4 segments, fragmented while written, finalized after)
                    ├──▶ pre-event buffer  (event modes: short HD segments)
                    ├──▶ timeline copy     (HD 24/7 cameras: the sub-stream copied, for quick scrubbing)
                    └──▶ motion decoder    (sub-stream only: a few frames/s, scaled, raw RGB → motion + AI)
```

* **One connection per camera stream** (`relay.rs`): every consumer is a local
  ffmpeg fed from GAZE's memory, not a second RTSP session.
* **No transcoding for recording**: video is copied; only rotation, privacy
  areas and dewarping need decoding.
* **Crash safety**: recordings are fragmented MP4 while written, repaired after
  a power cut (`mp4fix.rs`), and finalized (`finalize.rs`) before they count.
  `faststart` is not combined with fragmented MP4.

**Camera to browser (live)**

* **The grid**: JPEG pictures over one WebSocket, made from the frames motion
  detection has already decoded (no extra decode).
* **Smooth video**: fragmented MP4 over a WebSocket into Media Source
  Extensions. ffmpeg only re-packages it (`-c:v copy`).
* **Recordings**: played as HLS made from the MP4 files (`hls.rs`), read
  through a block reader built for slow disks and NAS (`vstore.rs`).

**Storage and indexing**

* Camera folders are named by **camera id** (renaming a camera changes nothing
  on disk).
* Recordings are found by **listing the folder** (cached 5 s).
* Alerts are JSON lines per camera, kept in memory.

## 2. Bottlenecks found

| # | Finding | Evidence |
|---|---|---|
| 1 | **One repackaging ffmpeg per viewer per camera** for live video | 4 cameras: 20 → 24 → 32 ffmpeg processes with 0 → 1 → 3 viewers |
| 2 | **~5 always-on ffmpeg processes per camera** (relay main, relay sub, recorder, timeline copy, motion decoder) | 20 processes for 4 cameras, 40 for 8 |
| 3 | **Live video kept streaming to hidden tabs** (the JPEG grid already stopped) | code review |
| 4 | **Time-lapse previews redone once a minute, all cameras at once** | code review: a burst of work every minute |
| 5 | **NAS / folder backup: no verification**, retried every minute with no backoff | code review |
| 6 | **Faces skipped** on cameras that keep no HD video of the moment (sub-stream-only recording) | code review (`identify.rs`) |
| 7 | **Folder listings for the timeline** | measured, see 9: **not a bottleneck on a local disk** |

No unbounded queues were found in the video path (each relay consumer has a
1,024-chunk queue and is dropped when it can't keep up), and no
shell-constructed commands (every process gets its arguments as a list; text
for Windows voices goes in an environment variable, never into the script).

## 3. Changes made

### Shared live repackaging (`livemux.rs`)
* **What was wrong**: each browser watching a camera started its own ffmpeg
  to turn the relay's MPEG-TS into fragmented MP4.
* **What changed**: one repackager per camera stream, shared by every
  viewer. GAZE splits ffmpeg's output into whole MP4 pieces (header,
  fragments) and keeps the fragments since the latest keyframe, so a viewer
  joining starts at once. Each viewer has a bounded queue (256 fragments,
  about 25 s): one that can't keep up is dropped and reconnects. It stops
  10 s after the last viewer leaves.
* **Measured**: 8 cameras with 3 viewers went from 66 to 48 ffmpeg processes
  and 1,344 to 1,126 MB, with no per-viewer growth in process count (table
  below).
* **Trade-off**: a viewer joining an existing stream starts at the latest
  keyframe of a stream already running, not a fresh one: the same picture,
  and the page already skips to the live edge.

### Live video pauses with the tab
Video connections are let go 10 s after the tab is hidden and made again when
it's shown (a quick switch of tabs keeps them). This frees the shared
repackager when nobody looks.

### Time-lapse previews: continuous and staggered
The files being recorded are brought up to date every 15 s instead of every
60 s, one file after another with a pause between them, instead of all
cameras in one burst. They're made from the small pictures motion detection
already keeps, so no extra decoding. The Tasks panel shows the pictures being
made going by, the files done (camera and time range), and how busy the CPU
and GPU are while it runs.

### HD pictures for face recognition
When a person is first seen (on the sub-stream) on a camera that keeps no HD
video of that moment, one HD picture is taken from the main stream right
then. It shares the decode with "double-check in HD" when that's on. Faces are
looked for in it. Cameras that record HD already used the HD recording.

### Verified backups to a folder / NAS (`cloud.rs`)
1. The copy is written as `<name>.partial`, flushed, and `fsync`ed.
2. Its size and **SHA-256** are checked against the original.
3. Only then is it renamed to its real name and marked as copied.
4. A mismatch removes the partial file and the copy is tried again later. The
   original is never touched.
5. Failures back off 1, 2, 4 … up to 30 minutes, instead of a retry every
   minute.

### go2rtc as an option (`go2rtc.rs`)
See 6.

### Evidence Locker (`cases.rs`)
Cases hold copies of alerts, clips and pictures. Protected items lock the
footage they came from against **every** delete path: Recordings, Alerts,
Delete footage and the automatic clean-up (retention, storage limits). The
refusal names the case.

### Other 1.0 changes
* The interface changes (live view, recordings, system page, hardware bar,
  settings clean-ups) are listed in the commit messages.
* A Security workflow runs `cargo audit` (section 11).

## 4. Deliberately not done

| Proposal | Why not (yet) |
|---|---|
| **SQLite recording index** | Measured: listing a month of 5-minute files for one camera (8,640) takes **16 ms** on a local disk, so about 270 ms for 16 cameras, and the result is cached. A database would add a second source of truth that must follow every write, rename, repair, move and delete. That's a real risk to recordings for no measured gain locally. **Revisit for recordings on a NAS**, where each listing is a network round trip (the test `storage::tests::list_speed` is ready to measure it there). |
| **Moving recordings into a `YYYY/MM/DD` layout** | Folders are already by stable camera id; moving existing recordings carries risk with no performance benefit. |
| **WebRTC for every camera** | The local grid already plays the camera's own H.264 with no transcoding (MSE). WebRTC is most useful for PTZ and remote viewing; go2rtc (now an option) can provide it in a next step, rather than a second WebRTC stack inside GAZE. |
| **Merging recorder, buffer and timeline copy into one ffmpeg** | Would save ~2 processes per camera, but a failure in one output would stop the others (today each restarts on its own). Better done behind a setting, with long tests on real cameras. |
| **Per-viewer JPEG changes** | The grid's JPEGs already come from frames decoded for motion detection: no extra decode. |
| **TensorRT / batch inference** | Can't be measured without an NVIDIA GPU; the AI already runs on the GPU through DirectML on Windows and backs off under load. |

## 5. Measurements

Simulated cameras: 2560×1440 20 fps H.264 (4 Mbit/s, AAC) main stream and
640×360 15 fps sub-stream, recorded HD 24/7 with motion detection, on a
4-core Xeon VM. Viewers are headless Chromium with the Live page open (all
cameras, H.264 video). Each run is 30 s after a 30 s warm-up. Memory is PSS:
shared libraries are counted once, not once per process.

| Build | Cameras | Viewers | ffmpeg processes | Memory (PSS) | CPU (% of 1 core) |
|---|---|---|---|---|---|
| before | 4 | 0 | 20 | 507 MB | 28.6 |
| after  | 4 | 0 | 20 | 534 MB | 30.3 |
| before | 4 | 1 | 24 | 618 MB | 30.2 |
| after  | 4 | 1 | 24 | 621 MB | 30.8 |
| before | 4 | 3 | **32** | **743 MB** | 32.6 |
| after  | 4 | 3 | **24** | **658 MB** | 32.2 |
| before | 8 | 0 | 40 | 976 MB | 60.1 |
| after  | 8 | 0 | 38 | 939 MB | 63.0 |
| before | 8 | 3 | **66** | **1,344 MB** | 38.8 |
| after  | 8 | 3 | **48** | **1,126 MB** | 48.0 |

**Reading the table**

* **Process count and memory**: these are the reliable figures, and they show
  the change. Before, every viewer added one ffmpeg per camera; now viewers
  share them.
* **CPU**: the CPU column is not a fair comparison. The browsers run on the
  same 4 cores, GAZE spreads work out when the CPU is busy (so a busier
  machine can show *less* GAZE CPU), and the "after" runs overlapped with a
  compile. Repackaging only copies data, so the CPU saving per removed
  process is small. The saving is processes, memory, and data copied between
  processes.
* **Not measured here**:
  * 12 and 16 cameras: 1440p test streams on 4 cores leave no headroom for the
    browsers.
  * GPU decoding: no GPU here.
  * Real cameras.
  * Latency.

  These need the user's hardware: `tools/bench` runs there too.

**Folder listing (for the SQLite decision)**: 8,640 files listed in 15.7 to
17.5 ms on a local disk (five runs).

## 6. Streaming architecture (recommended)

* **Recording**: camera → relay (one connection) → copy to disk. Unchanged:
  it's already transcoding-free and independent of viewers.
* **Local live grid**: JPEG from motion frames (lightest), smooth video as
  fragmented MP4 over WebSocket into MSE, one shared repackager per stream.
* **Recordings**: HLS from the MP4 files.
* **go2rtc (option, off by default)**: Settings → System → Streaming engine.
  * **How it runs**: go2rtc runs supervised as a child of GAZE, on this
    computer only (RTSP 127.0.0.1:18554, API 127.0.0.1:11984; WebRTC, RTMP and
    SRTP servers switched off). It holds the one connection to each camera, and
    GAZE's relays read from it.
  * **Where cameras connect**: every reconnect asks where to connect. While
    go2rtc runs, that's through go2rtc; when it stops or can't start, it's
    the camera itself. Switching moves cameras over within seconds, and a new
    recording file starts.
  * **Tested here**:
    * Switching over to a go2rtc stand-in (same ports) and back.
    * Losing it mid-stream: all streams reconnected directly within ~6 s.
    * A go2rtc that can't start: recording carried on, retries backed off.
  * **Not tested here**: the go2rtc program itself. It wasn't allowed to be
    downloaded and run in the build environment. GAZE fetches go2rtc 1.9.9 when
    it's switched on, or you can set the path to your own copy. **Try it on a
    test camera first.**
  * **Next step**: go2rtc's WebRTC for PTZ and remote viewing, served through
    GAZE's sign-in.

## 7. Recording and file formats

* **Continuous recording**: fragmented MP4 while written, finalized when
  complete, repaired after power loss. The camera's own codec; sound kept and
  made AAC.
* **Alert clips**: MP4.
* **Pictures**: JPEG.
* **Alert details**: JSON lines.
* **Segment length**: configurable, 5 min by default.
* **Not changed**: recording metadata (codec, size, frame rate) is read from
  the files when needed and not stored in an index (see 4).

## 8. Storage folders (unchanged)

```
<recordings folder>/<camera id>/                  continuous recordings (MP4 segments)
<recordings folder>/<camera id>/timeline/         light sub-stream copy (HD 24/7 cameras)
<recordings folder>/<camera id>/events/           alerts: events.jsonl, pictures, clips
<recordings folder>/<camera id>/.thumbs/          previews and time-lapse pictures (cache)
<recordings folder>/<camera id>/.buffer/          pre-event buffer (temporary)
nvr-data/                                         settings data, logs, models, sessions
nvr-data/cases/<case>/                            Evidence Locker copies + cases.json
```

Cleaning the caches (`.thumbs`, `.buffer`) can't remove recordings: they're
separate folders, and every delete goes through the protection checks.

## 9. Index design

No SQLite index in 1.0 (see 4).

## 10. NAS archiving

Backup to a folder or NAS share:
1. Finished recordings and alerts are queued (the list of what's copied is
   kept per camera).
2. Each one is copied as `.partial`, flushed, and checked for size and
   SHA-256.
3. The copy is renamed and marked done.
4. On failure it backs off 1 to 30 minutes. Recording carries on locally, and
   the original is never deleted by the backup.
5. FTP and rclone (cloud) targets work as before.
6. Windows shares are reached with UNC paths (`\\NAS\share`), signed in when a
   login is set.

**Not done**:
* Bandwidth limits.
* Per-target retention.
* A persistent list of failed jobs (the list of copied files is persistent).

## 11. Security

| Area | Change or check |
|---|---|
| **Dependencies** | A Security workflow runs `cargo audit` when dependencies change and weekly (`.github/workflows/security.yml`). Run here on 17 Nov 2026: 365 dependencies, **no known vulnerabilities** (1,509 advisories checked). |
| **Evidence Locker** | Unprotecting evidence and deleting cases or items needs an administrator. Operators may make cases and add to them. Case files are served only by name from the case's own list (no paths). |
| **go2rtc** | Listens on 127.0.0.1 only. Its settings file holds the camera passwords and is made private to GAZE's account. Its log lines have camera passwords removed. |
| **Links** | Shortcut, tray, notification and About links use https when the secure port is on. |
| **Reviewed, no change needed** | No shell-built commands. Camera addresses masked in logs. Path checks on recordings, events and case files. Sign-in and roles are checked for every API route by the existing middleware. |

## 12. Tests

* `cargo test --release`: 126 tests, all passing. New tests cover:
  * MP4 splitting and keyframe detection, including real ffmpeg output.
  * The Evidence Locker: locks, refusals, restart and unprotect.
  * Network history loading old files.
  * Alerts per hour.
  * Pets rule validation.
  * go2rtc settings and fallback.
  * Verified copies.
  * https links.
* **End to end against a running GAZE with simulated cameras**:
  * Evidence Locker: refusals with the case named, copies served, path
    traversal refused.
  * go2rtc: switching, fallback, a program that can't start.
* **Browser (headless Chromium)**: every page loads with no script errors;
  camera menu, digital zoom, full-screen drawer, opened-camera layout.
* **Hidden tab**: the shared repackagers went from 2 to 0 while hidden, and
  back to 2 (both videos playing) when shown again.

## 13. Limitations

* **Not measured here**: GPU behaviour, real cameras, 12 to 16 cameras, and
  live latency. They need the user's hardware.
* **go2rtc**: switching and fallback are tested, but the go2rtc program itself
  hasn't been run.
* **SQLite and the folder layout**: not done, by decision (see 4).
* **NAS backup**: no bandwidth limit.

## 14. Next phase

1. Run `tools/bench` on the real machine with 9 to 16 cameras, including GPU
   decoding, and add the numbers here.
2. Try go2rtc on one camera, then serve its WebRTC through GAZE for PTZ and
   remote viewing.
3. Measure folder listings on the NAS. If slow, keep listings between visits
   and add new files incrementally, before considering a database.
4. Merge recorder, buffer and timeline copy into one ffmpeg behind a setting,
   with a long soak test.
5. NAS bandwidth limit, and a persistent failed-copies list on the System page.
