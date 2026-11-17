# What's new in GAZE

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
