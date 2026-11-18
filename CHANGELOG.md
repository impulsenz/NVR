# What's new in GAZE

## 1.2.2 — November 2026
- **Recordings cut short by a power cut or crash**: a file that holds no video at all (only its header, because the power failed within the first second of a new file) is now removed during the repair instead of staying in the list as a recording that can't be played. Files with any video are repaired as before. (Checked with real recordings cut at 60 places and with zero-filled endings.)
- **ffmpeg download (Windows)** is checked against the checksum list its builder publishes with it; a damaged or cut-off download is deleted and not used. If the list can't be read, ffmpeg is used anyway so recording is never held up.
- **Security**: downloaded programs and libraries are now checked against a pinned checksum before they are used — go2rtc, the Piper voice engine and the DirectML runtime (a file that doesn't match is deleted and not used). The sign-in lock-out can no longer be dodged by sending made-up `X-Forwarded-For` addresses through a reverse proxy on the same computer, or by changing address inside an IPv6 /64.
- **Tests** for the recording repair after a power cut (real ffmpeg recordings cut at 60 places, and with zero-filled endings) and for the snooze-link signature (the hash is checked against the RFC 4231 answer).
- **Linux download**: releases now also carry `GAZE-linux-x86_64` (with its checksum). GAZE doesn't update itself on Linux. A Linux check (tests, page smoke test, Docker image build and start) now runs on every change. The Windows release is unaffected (the Linux build is a separate job).

## 1.2.1 — November 2026
- **Page smoke test** for developers (`tools/e2e/smoke.js`): opens every page and settings section in both themes and fails on script errors. No change to GAZE itself.
- **Linux packaging**: a systemd unit (`packaging/gaze.service`), a Dockerfile (`packaging/Dockerfile`) and a guide (`docs/LINUX.md`). The Docker image has not been built yet and there's no Linux download in the releases yet.
- **Sturdier camera access checks** (no visible change): the rule that keeps a camera-limited account off other cameras now lives in one tested function instead of twelve copies.
- **Drive health** (Settings → System): reads every drive's own health report (SMART) through `smartctl` (smartmontools, installed separately) and shows model, size, temperature, age and health. It checks every few hours, and with *Warn me when something's wrong* on, notifies you (phone and/or email) when a drive starts to fail — reallocated or unreadable sectors, a failed SMART status, an NVMe critical warning or nearly-used-up life, overheating — so you can copy the footage off in time. If smartctl isn't installed, or isn't allowed to read the drives (it usually needs administrator / root rights), the page says so. Recording never depends on it.

## 1.2.0 — November 2026
- **Learned states** (Settings → Learned states): teach GAZE what a camera sees from a few examples — garage door open or closed, bin out or in, gate, a light. Make one, then press *Add example* while the scene is in each state; GAZE compares the camera's picture with the examples every few seconds (using the picture model description search already uses; 420 MB downloaded once, nothing leaves the computer, no training) and shows the answer. Optionally choose the piece of the picture that matters, and be notified (phone and/or email) when it changes. The answer is also sent to MQTT, and appears in Home Assistant as a sensor. A change must be seen twice in a row, so a passing shadow doesn't flip it. It never affects recording or alerts.
- **AI on Linux: OpenVINO and CUDA** (Settings → Object recognition → Graphics API). *Automatic* uses CUDA when an NVIDIA card with CUDA 12 and cuDNN 9 is installed, otherwise OpenVINO, which runs on Intel graphics, an Intel NPU, or just the processor (faster than GAZE's built-in engine). The add-on downloads by itself the first time (OpenVINO about 85 MB, CUDA about 250 MB), is checked against a pinned SHA-256 and kept in `nvr-data/runtime/`. Changing it takes a restart. Windows keeps DirectML. Tested: OpenVINO on a processor (same answers as the built-in engine for detection, faces and plates). **Not tested on real hardware: CUDA, Intel graphics, NPU.** If an accelerator can't run a model, that model uses the processor and Settings → Tasks says why.
- **Email notifications** (Settings → Notifications → Email): every notification (alert picture, summary, camera and drive warnings) can also go by email through any SMTP server (STARTTLS, TLS or a plain relay on your network), to one or more addresses, with the picture attached. Works with or without ntfy. The password is kept in the settings file and never sent back to the page. Existing settings files are unchanged (email is off by default).
- **Sturdier internals** (no visible change): a fault in one part of GAZE can no longer lock up other parts through a poisoned lock (`src/sync.rs`); a test now fails if a new camera-specific web address is added without an access decision.

## 1.1.11 — November 2026
- **Eleven looks** (👤 menu → *Look on this device*): Classic (as before), Rail (icon sidebar), Dock (floating bottom dock), Terminal (green phosphor), HUD (mission control), Paper (newspaper masthead), Brutal (loud blocks), Neon (synthwave, menu on the right), Soft (clay sidebar), Aurora (frosted glass) and Swiss (numbered grid). Each changes the layout, shapes, type and colours, and each has a day and a night version that follow *Colours on this device*. Chosen per device; no GAZE setting changes, recording is untouched, and phones keep the bottom tab bar in every look.

## 1.1.10 — November 2026
- **Faster page loads**: the page, its script and styles are compressed and remembered by the browser: the first load is about 70 % smaller (900 KB → 260 KB) and a reload downloads almost nothing.
- **Security**: the Evidence Locker no longer lets an account that can't see a camera change or delete cases and items holding that camera's footage; the streaming (HLS) addresses are covered by the central camera check; a camera's address must be a streaming address (rtsp://, rtsps://, http(s)://, rtmp(s)://, srt://, rtp://, udp://, tcp://) — addresses that read files or run programs are refused when entered (cameras already saved are not affected).
- Audit report and project documentation (`docs/AUDIT-2026-11.md`, architecture map, technical-debt register, configuration inventory, security checklist, tooling), rules for future development (`CLAUDE.md`), and a pull-request quality workflow (tests, lints, Windows compile check).

## 1.1.9 — November 2026
- **Watchdog reopens GAZE however it stopped**: crashed, hung, ended in Task Manager or closed with Quit. (Before, it left GAZE closed after Quit. That's now an opt-in setting: “Leave GAZE closed when I quit it myself”.)
- **The watchdog is its own Windows service** (Settings → Watchdog → “As its own Windows service”): starts with the computer, before anyone signs in, and Windows restarts the watchdog itself if it ever stops. It updates itself as a service too.
- **The watchdog has its own icon** next to the clock (the GAZE eye with a green tick): status at a glance; menu to open GAZE, restart it, pause watching, open the log.
- **Day theme**: the timelines (Recordings and the camera view, horizontal and vertical), the server-load graph and the sign-in card are now light on the day theme (they were fixed dark); night theme unchanged.

## 1.1.8 — November 2026
- **Footage in deep storage stays on the timeline**, drawn darker so you can see it's no longer on the local disk (the key says "In deep storage (darker)"). It plays straight from deep storage (read there, not copied back), shows in the Alerts / Recordings list, plays from alerts, can be exported, and deleting it deletes it from there.
- **Vertical timeline**: the recording-now bar looks like the bottom timeline's (its colour with a moving striped end); camera names slant up to the right so they read the right way up, with the recording dot at the top.
- **Folders are named as you call the camera**: older cameras that record into a folder named after their id are renamed to their name when GAZE starts (instant, on the same drive), and a camera's folder in deep storage follows when you rename the camera.

## 1.1.7 — November 2026
- **Recordings, vertical timeline**: the controls (Zoom, ◀ ▶, Merge cameras, Layout, Horizontal, Clip start / end …) are no longer hidden behind the timeline: they sit under the camera pictures, and the timeline runs down the whole right side.

## 1.1.6 — November 2026
- **GAZE Watchdog** (optional, its own program `GAZE-Watchdog.exe`): watches GAZE and opens it again if it crashes or stops answering. It starts with Windows — when someone signs in, or when the computer starts — so after a restart it opens GAZE too. Install, set up, monitor and update it from the browser: **Settings → Watchdog** (status, last check, restarts, log, check interval, restarts allowed before it waits, how GAZE is opened — program or Windows service). It has its own updates (it can install them by itself, or you update it from the same page). If you quit GAZE yourself, the watchdog leaves it closed.

## 1.1.5 — November 2026
- **Deep storage** (Settings → Recording & storage, redesigned): footage stays on the fast local disk for as long as you say (24 hours by default), then moves to a NAS or big drive — copied, checked (size and checksum) and only then taken off the local disk. Deep storage has its own keep time (days) and size limit; footage in an Evidence Locker case is never deleted from it.
- Set it for all cameras, or give a camera its own times, or keep it on the local disk. Each camera shows what's on the local disk, what's in deep storage and what's waiting to move; Browse plays or downloads footage from deep storage.
- Moves show under System → Tasks (files left, size, speed) and can be paused; **Move now** and **Test the folder** buttons; a picture across the top shows the path footage takes.
- With deep storage on, the local disk's age limit is replaced by the move; the disk-space limits (maximum storage, keep free) still apply.
- The timeline shows what is on the local disk; older footage is in deep storage. (This replaces the "data retention" option of Backup added in 1.1.2.)

## 1.1.4 — November 2026
- **Recordings, vertical timeline in detail**: a line for each camera (or Merge cameras into one), day bands and gridlines with minor ticks, recording bars (sub-stream, HD, recording now), time-lapse and loaded strips, alert bars, **alert pictures** down the side (stacked with a count where they touch, click to watch or choose), clip start / end handles, the time and camera under the pointer, camera names to click for just that camera.
- **Recordings layout**: Auto (best fit) picks the number of columns that makes the camera pictures as big as the screen allows, or choose 1–6 across.
- **Settings → Cameras** is now a grid of cards with big pictures (Medium / Large / Extra large), a search box, a summary (how many are recording or have problems), and Edit / Live / Clone / Delete on each.

## 1.1.3 — November 2026
- **Recordings**: a timeline down the side (button above the timeline: ⇅ Vertical / ⇄ Horizontal). All cameras' footage and alerts are merged into one line with the pictures bigger beside it; drag, click, scroll the wheel to move through time. Full screen keeps the bottom timeline.
- **Privacy areas can follow the mode**: black them out only in Home, Away and/or Night (camera → Privacy areas). The camera reconnects for a moment when the mode changes (also when Home turns into Night by itself).
- **Updates and restarts reload every open GAZE page**, not only the one that started the update. A page that loses GAZE during a restart reloads when it's back.
- **Evidence Locker**: thumbnails are made once and kept (older items get theirs the first time they're shown), and the browser keeps them.

## 1.1.2 — November 2026
- **Live view**: the camera timeline is now a panel over the bottom of the picture that slides up when the pointer is at (or below) the bottom of the picture, as in full screen. A mini pan / tilt / zoom pad shows in full screen. The grid shrinks to fit the browser window without scrolling. Switching back to Live no longer flashes blank pictures.
- **Live view & display**: the overlay settings run down the left, the camera preview sits on the right.
- **Siren** sounds for 60 seconds by default.
- **Data retention** (Settings → Backup): keep recordings on this computer for a set time (24 hours by default), then move them to the NAS — copied and checked first, then removed locally (never footage held in an Evidence Locker case). Shows under System → Tasks with files left, size and speed.
- **Evidence Locker**: thumbnails for clips; playing one video pauses the others; notes are typed in place (for each video and for the whole case); **Export all** makes a ZIP with a folder per video (video, picture, notes), an `index.html` to review it, and a SHA-256 manifest. Export one case or every case.
- **Privacy areas** are drawn with a brush (draw, erase, brush size).
- The "GAZE CPU" line is gone from the server load graph.

## 1.1.1 — November 2026
- **go2rtc with TP-Link Tapo (and similar) cameras**: go2rtc no longer asks cameras for their two-way audio channel, which left some cameras (Tapo among them) with no picture ("could not find codec parameters"). GAZE talks through cameras itself as before.
- **A camera that gets no picture through go2rtc connects straight to the camera** after two tries (for 10 minutes, then tries go2rtc again), so it keeps recording. What go2rtc said about it is written to Settings → Logs.

## 1.1.0 — November 2026
- **Evidence Locker** (new Evidence page): keep alerts, clips and pictures in cases. Everything added is copied into the case and protected — it can't be deleted from Recordings, Alerts, Delete footage or by the automatic clean-up until it's unprotected in its case.
- **Live view**: the opened camera fills its box edge to edge with its controls underneath; choose all cameras, a group or any cameras from one menu; full screen shows the same layout as the Live page; zoom into any picture with the mouse wheel; no more full-screen button on each picture.
- **Pan / tilt / zoom**: ＋ adds a preset, a bin deletes one, and the controls stay on screen when Track or Roam opens.
- **Recordings and alerts**: opens on the stream that's recorded all the time; LIVE shows instead of PLAYBACK while watching live; Back after opening an alert returns to exactly where you were; Download clip next to Full alert; recognized people are named on alert pictures.
- **Ignore pets** (camera → AI): no alerts for dogs and cats on their own; a person seen with them still alerts.
- **System**: a hardware bar along the bottom (network, CPU, GPU, memory, free space) and a drawer with the same in full screen; green / yellow / red load colours; alerts, motion, cars and people per hour for each camera; the page uses the whole width of wide screens.
- **Lighter live video**: one stream per camera shared by everyone watching, paused when the tab is hidden.
- **go2rtc** can be chosen as the streaming engine (Settings → System, to try out); FFmpeg stays the default.
- **Backups to a folder or NAS are checked** (size and checksum) before they count.
- Profiles are gone (cameras keep their settings); Save and Save and close when editing a camera; GAZE opens at its https address.

## 1.0.0 — November 2026
- First release.
