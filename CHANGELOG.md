# What's new in GAZE

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
