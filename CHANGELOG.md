# What's new in GAZE

## 0.6.29 — November 2026
- **Alert pictures on the timelines**: above the recording bar, each AI alert now shows its actual picture, framed in the colour of its kind. Where several alerts are too close to fit, they stack into one picture with a count — click it to see them all and pick one to watch.
- **No more red NOW lines down every row**: "now" is a single small mark on the time scale, and the part of the recording still being made has a moving striped end (like the time-lapse line while it loads).
- **Time-lapse in the camera view fills the box**: scrubbing no longer shows bits of the next and previous pictures down the sides.
- **Settings opens instantly**: leaving Live or Recordings now stops their background time-lapse downloads and half-loaded videos, which were holding up the browser's connections; Settings also shows straight away instead of waiting for the server.

## 0.6.28 — November 2026
- **New look for the timelines**: the Recordings timeline and the camera view's timeline now look the same, with much more detail:
  - alerts coloured by kind — **people blue, vehicles amber, animals green, packages pink, motion orange** — each with a round symbol badge;
  - shaded recording bars (purple for full-quality alert clips) and a glowing time-lapse line that shimmers while it's being made;
  - alternating hour bands, bold hours, a line and date at midnight, and a red **NOW** line;
  - the playhead has a knob and shows its time, and a guide line follows the mouse with the time under it;
  - each camera shows whether it's recording (red dot) and how many alerts are in view.

## 0.6.27 — November 2026
- **No black blink after scrubbing**: when you let go of the timeline, the time-lapse picture stays until each camera's video has really loaded its picture — however long that takes — and then fades smoothly into the video.
- **What's new after an update**: when GAZE has been updated, a window shows which version you're now running and what changed in it (once).
- The Updates button starts at **0%** when an update begins (it showed 100% for a moment).

## 0.6.26 — November 2026
- The detection label on the picture (e.g. "🚶 person") is always one solid bar — it no longer splits the icon and the word onto two lines near the edge of the picture. Other freely placed on-screen display items stay on one line too.

## 0.6.25 — November 2026
- The **Scrub preview** choice (Time-lapse / Sub-stream / Main stream) and its "ms a picture" readout are gone from Recordings: dragging along the timeline always shows the time-lapse.

## 0.6.24 — November 2026
- **Time-lapse stays after a reload**: previews already made show as ready straight away when you open or reload the timeline (GAZE asks for all of them in one go) — only missing ones are made.
- **No more little gaps in the blue time-lapse bar** between recording files.
- **Choose how often time-lapse takes a picture**: Settings → Recording & storage → "Time-lapse pictures: one every N seconds" (1–120, default 10). Fewer seconds give smoother scrubbing but more work for the computer; changing it makes the previews again in the background.
- **The "GAZE is ready to install" bar stays at the top** under the toolbar while you scroll.

## 0.6.23 — November 2026
- **Time-lapse task shows its progress**: while you're looking at the timeline, previews are made for the page first; the task in Settings → Tasks now counts those too, instead of staying at "0 of 40".
- **Time-lapse strip animation fixed**: the striped "being made" piece is now the same thin bar as the blue one and sits exactly under the footage it's for (it was taller and shifted to the left).
- **Previews made on the graphics card**: with GPU acceleration set up (Settings → GPU acceleration), time-lapse pictures are decoded on the graphics card, and on the processor if that doesn't work for a file.

## 0.6.22 — November 2026
- **New alerts appear straight away**: the Alerts page updates as soon as a new alert comes in, without refreshing — also when you've had the page open a long time (and "Today" moves on to the new day at midnight).
- **Easier to pick alerts on the timeline**: the alert nearest the mouse (even a few pixels off) lights up with a white outline, and that's the one a click opens — no more jiggling to hit thin or close-together markers.
- **Time-lapse is back**: the blue bar under each camera's timeline shows where a time-lapse preview is ready. Previews now take one picture every 10 seconds (from keyframes only), which is far lighter on the computer.

## 0.6.21 — November 2026
- **Bigger alert window that stays put**: on a computer the alert window now fills most of the screen and keeps exactly the same size and place as you step through alerts with ◀ ▶, so the arrows never move under your mouse (longer details scroll inside). Clicking outside the window closes it.
- **Updating**: the Updates button just shows the ring filling up (no spinning arrows).
- **Alerts filters start afresh**: coming back to Alerts from another page resets the filters (today, every camera, the usual kinds of alert, anyone).

## 0.6.20 — November 2026
- **Deleted alerts really stay gone**: on Windows the list of alerts sometimes couldn't be replaced at the moment of deleting (another program holding the file for a moment), so the old alerts came back — without pictures — when GAZE next started (e.g. after an update). The list is now saved reliably, and when GAZE starts it drops any alert whose picture and clip are both gone, which also clears the black ones already showing.
- **Delete footage** also clears the alerts of cameras that are switched off.

## 0.6.19 — November 2026
- **No more black gaps between recordings**: on the Recordings timeline the next file is loaded in the background and switched to only once it has a picture, so playback runs straight from one 5-minute file into the next.
- **Updates button shows what's happening**: it spins while checking, shows a dot when a new version is ready, and while updating a ring fills up with "Updating 45%", then "Installing…" and "Restarting…".
- The update bar is shorter (no "released … · takes a few seconds" line).

## 0.6.18 — November 2026
- **Your voice in the recordings**: what you say with 🎙️ Push to talk (and the chimes) is now added to the camera's recordings — the recording, the timeline copy and the alert clip of that time — so playing them back you hear both sides of the conversation. It's added a minute or two after each recording file is finished; the video itself isn't touched.
- **Hear the camera while you talk**: pressing Push to talk turns on the camera's sound automatically (and leaves it on to hear the answer; the 🔊 button turns it off).

## 0.6.17 — November 2026
- **"Goodbye" tune when you stop talking**: letting go of 🎙️ Push to talk (or tapping it off) plays three quick, soft falling notes through the camera's speaker, so whoever is there knows you've finished. It follows the camera's chime setting (Audio tab).

## 0.6.16 — November 2026
- **Much less delay when talking**: what you say no longer builds up while the camera's speaker connects (it stayed as a delay for the whole conversation); your voice goes straight after the doorbell chime.
- **Live sound closer to real time**: the camera's sound starts from this moment (not a few seconds back), and the player keeps itself within about a third of a second of live instead of falling behind.
- **Instant snapshots**: 📷 Snapshot saves the full-quality picture you're looking at straight away (if the picture hasn't loaded yet, it asks the camera as before).
- "Hold to talk" is now **Push to talk**.

## 0.6.15 — November 2026
- **Deleted alerts stay deleted**: after **Delete footage** (or deleting an alert), alerts no longer come back a while later with a black picture. Background work that was still busy with them (faces and plates, full-quality clips, notifications, sharper pictures) now lets them go instead of saving them again.

## 0.6.14 — November 2026
- **Parked cars stay quiet**: "Stop alerting for objects that stay in one place" now works for cars that are only partly in the picture or recognised with a varying score. GAZE follows every sighting of a parked vehicle (not just those strong enough to alert), allows for the box around it jumping about, and only treats it as moving off once it's seen somewhere else twice in a row.

## 0.6.13 — November 2026
- **Profiles save in the background**: the window closes as soon as you press **Save profile**; the profile is saved and the cameras following it updated in the background ("Profile saved" appears when done). If it's refused, the window comes back with the reason.
- The hint next to "AI check interval" (camera → AI) no longer mentions cloud AI.

## 0.6.12 — November 2026
- **Settings → Profiles**: profiles now have their own place in Settings. Create one (from scratch or from a camera's settings), edit its Recording, Motion and AI settings, rename or delete it, and see which cameras follow it. In a camera's settings you just choose which profile it follows. Each camera keeps its own ignore areas, package spot and zones.
- **Doorbell chime when you talk**: pressing 🎙️ Hold to talk rings a "ding-dong" through the camera's speaker straight away, to get the attention of whoever is there; your voice follows right after. It can be turned off per camera (Audio tab).
- **No alert for a cat**: "Check motion with AI first" (camera → Motion) now has the AI look at the motion's frames at least twice before it becomes an alert, and if it sees a cat — or another animal you don't watch for — at any point (and nothing you do watch for), the motion isn't kept on the Alerts page.
- **Record before alert** is 3 seconds for new cameras (was 30).
- Camera pictures always stretch to fill their box (the setting is gone).
- Notifications: the "Only suspicious activity" option is gone.

## 0.6.11 — November 2026
- **Camera settings close straight away**: the window closes as soon as you press **Save camera**, and the camera is saved (and its video restarted) in the background — "Camera updated" appears when done. If the settings are refused, the window comes back with what you typed and the reason. Moving recordings to a new folder still waits until it's done.

## 0.6.10 — November 2026
- **See how far "Object must travel" is**: camera → Motion now draws a green ring on the camera picture showing how far something's centre has to move before it counts, and it changes as you type the number (like the blue box for the minimum object size).

## 0.6.9 — November 2026
- **S1 / S2 in full screen**: switching a camera between stream 1 and stream 2 on the Live page no longer takes it out of full screen — it stays in full screen (or stays out of it) and just changes the picture.

## 0.6.8 — November 2026
- **Correct faces from People & vehicles**: each person now lists the faces recognised as them in the last 30 days of alerts ("Recognised as … — check for mistakes"). Any of them has **✎ Wrong?**: it's someone else, a new person, or not anyone you know. GAZE stops taking that face for them.
- **Alerts → New faces & plates to name**: a new choice in the person filter (next to "Anyone") shows the faces nobody is named for yet and number plates of no known vehicle, from the days chosen in the calendar. The same person seen in several alerts is shown once: choose who it is (or New person / New vehicle) and all those alerts are named and the face learned, or hide passers-by.

## 0.6.7 — November 2026
- **Time-lapse previews switched off for now**: making them in the background couldn't keep up and kept the computer busy. Nothing is made any more; dragging along a timeline moves the video itself (sub-stream or main stream, as chosen on Recordings), and alert pop-ups jump straight to the full-quality video.
- **Delete all footage** (header, admins): removes every recording, timeline copy, alert, clip and alert picture on all cameras after you type DELETE — handy for testing. Cameras, settings and known people stay, and recording carries on.
- **Step one frame**: −10s / +10s are now **−1 frame / +1 frame** in the video players and on the Recordings timeline (pauses on that frame).

## 0.6.6 — November 2026
- **Package spot picture fixed**: the camera's picture under "Notice parcels delivered or picked up" now shows the live sub-stream (like the motion mask), so it appears straight away and keeps updating. Before, it asked the camera for a full-size snapshot, which could fail or time out (blank box) on big or H.265 streams.
- The camera editor's pictures (motion mask, package spot, zones) now always get the sub-stream while the editor is open, even if the Live grid is set to the main stream.

## 0.6.5 — November 2026
- **Fix a wrongly named face**: on an alert, a recognised face now has **✎ Wrong?**. Choose who it really is, make a **new person** from it, or say it's **not anyone you know**. GAZE learns from the correction: the person it was wrongly taken for stops matching that face (and faces very like it), so the same mistake isn't made again. The alert's "Recognized …" notes are updated too.

## 0.6.4 — November 2026
- **Package detection stays in the package spot**: the spot you drew (and zones watching for items) is only checked when something actually reached it during the alert: movement there, or a person or object the AI found there. Activity elsewhere in the picture, with shadows or light changing over the spot, no longer makes "item left or taken" / package alerts.
- The "before" picture of the spot must be recent (from the last two minutes before the alert); an old one is not compared.

## 0.6.3 — November 2026
- **Phones: one camera across the whole screen**: on a phone's Live page each camera now fills the full width of the screen, edge to edge, with the others stacked below it. Computers are unchanged. (Two side by side is still possible: Edit layout on the phone → 2 columns.)
- The bar above the cameras wraps on phones instead of being cut off.

## 0.6.2 — November 2026
- **Missing time-lapse is filled in**: besides the last 24 hours, GAZE now goes back over all older footage every ten minutes and makes any time-lapse preview that's missing (a batch at a time, while the computer is quiet). A file that fails is tried again later. Settings → Tasks shows how many are left.
- **Choose what scrubbing shows** (Recordings, next to the clip buttons): the **time-lapse**, the **sub-stream** or the **main stream** video, with how long each picture takes to appear, so you can compare.
- **Time-lapse in alert pop-ups**: drag along the bar and the time-lapse picture fills the video; let go and the full-quality footage loads at that moment.
- Time-lapse pictures that came out as one flat colour (often green, from a frame that couldn't be decoded) are replaced with the picture next to them.
- Calendar: **Whole day** is now **All time**, showing every alert there is.
- The data rate at the top right counts only what's streaming from GAZE to the device you're on.

## 0.6.1 — November 2026
- **Fewer missed objects**: when the AI sees nothing in the whole picture, it takes a second, zoomed-in look where the motion is, so small or far-away people, cars and animals are found.
- **Alert pictures show the right moment**: the picture taken from the recording is matched against what detection saw live (the recording and the live view run a second or two apart), so the box no longer lands on an empty spot.
- **Ignore motion set off by animals** (camera → Motion, on by default): with "Any motion", the AI looks first; if all it sees is a cat, dog or bird you don't watch for, no alert. Tick "cat" in the AI tab to get alerts for cats.
- **Alerts grouped by day, time of day or hour**, each group folded away with − and opened with + (or all at once).
- **Ignore areas painted in fine detail**: the grid is 5× finer, with Paint and Erase tools and brush sizes (right-click or Shift erases too). Masks you already have are kept.
- **Recordings**: click a camera's name on the timeline to see just that camera; choose whether to play the **sub-stream** (default, opens instantly) or the **main stream**.
- **Time-lapse previews load reliably**: a preview that failed is tried again by itself, a broken page download is fetched again, and remaking a preview no longer breaks the one being shown.
- Updates are checked **once a day** (⟳ Updates still checks straight away).
- The clip-cutting hint under alert videos is gone.

## 0.6.0 — November 2026
- **Back up to a NAS**: Settings → Backup now offers **a NAS over FTP** (Synology, QNAP, a router's USB drive … — built in, nothing to install) and a login for a NAS's **shared folder** (`\\NAS\cameras`).
- **Time-lapse for all your footage**: previews are now made for every recording, also the times without a light timeline copy (e.g. after GAZE was off or the copy reconnected), and the background task keeps the newest footage ready. Settings → Tasks shows which camera, file and time it's on, and how many are left.
- **Alerts calendar fixed**: one click shows that day (and it stays), a second click makes a range, the quick choices apply at once, and there's a new **All time**.
- **Full screen** from the header, on any page (phones too, where the browser allows it).
- **Logs** are now under Settings.
- **Live pictures fill their box** (stretched by default, as set in Settings → Live view & display), so 4:3 sub-streams show no black bars.
- Camera view: the stream and frame-rate choices moved to the camera's settings (General → Live view). Timeline legends only list what's on them and the objects that camera looks for.
- On-screen display: **frame rate** and **resolution** are separate items.
- Camera quick setup: **Amcrest**, Lorex, Imou, Milesight, Bosch, Vivotek, Panasonic / i-PRO, INSTAR, D-Link and older Hikvision formats added (25 in all).

## 0.5.9 — November 2026
- **Made for phones**: a tab bar at the bottom (Live, Alerts, Recordings, Settings and your account menu, which now holds System, Logs and the update check), bigger buttons, toolbars and filter chips in one row that scrolls sideways, alerts one per row, one camera filling the width, a compact video player in the alert view, the timeline right under the pictures, and the settings sections as tabs.
- **Alert pictures from the recording**: a few seconds after the moment is saved, each alert's picture is taken from the recorded file (full quality when there is some) instead of the live stream — also while the alert is still going on.
- **Object recognition is Off or Built-in**: the AI server, Ollama, Gemini and OpenAI-compatible options are gone; a setup using one switches to Built-in.

## 0.5.8 — November 2026
- **On-screen display**: items you drag on the preview now stay exactly where you drop them (no snapping to a grid). The nine position buttons are still there for quick placement.
- **Camera view timeline works like the Recordings timeline**: detailed time marks with a "now" line, zoom from 5 minutes to 7 days, **drag along it to run through the time-lapse on the camera's picture** (with the time shown), let go to play from there, a line that follows playback, and the strip showing which parts of the time-lapse are loaded — it keeps loading up to now while you watch.

## 0.5.7 — November 2026
- **Update checks fixed**: "403 rate limit exceeded" from GitHub is gone. GAZE now reads the newest version from GitHub's release page instead of its API (which allows only 60 requests an hour per internet connection), and waits as long as GitHub asks if it's ever told to slow down.
- **Video player controls on the video**: play, the timeline, ±10 s, speed, clip start/end, download, volume and full screen sit on the picture and fade while it plays. **Full screen keeps them**, together with the camera's on-screen display (name and the recording's date and time).
- **Taller, more detailed timeline**: minute marks, grid lines and a "now" line. A strip under each camera shows which parts of the time-lapse are loaded (blue), loading (striped) or not yet; it replaces the progress bar, and new footage keeps loading while the page stays open.
- **Alerts**: Person, Car, Truck, Motorcycle, Item left/taken, Faces and Number plates are selected by default (remembered per browser), and a new drop-down shows only the alerts of a saved person or vehicle.
- **On-screen display**: Settings → Live view & display has the same layout editor as each camera — every item, the nine positions and a live preview — and items can be dragged on the preview, snapping to the nine positions or staying exactly where you drop them.
- **Removed**: alert descriptions and the description search (smart search, vision-model descriptions, the Describe button, descriptions in notifications) — to be rebuilt later. The old models and indexes are deleted, freeing 0.6–1.7 GB. Vehicles are recognised by number plate only.
- **Removed**: the camera's ONVIF settings tab. **Restart camera** is under General, for cameras that answer over ONVIF.

## 0.5.6 — November 2026
- **Much better alert descriptions**: choose who writes them in Settings → Object recognition → **Descriptions written by**. A vision-language model looks at several moments of each alert together and writes what happened ("A courier in a hi-vis vest leaves a parcel at the door and walks back to a white van"), instead of set phrases. Use **Ollama or LM Studio on your own computer** (free, pictures stay at home), **Claude** or **Gemini** (cloud). A daily limit keeps cloud costs known, and **Describe the latest alert** tries it before saving.
- **One AI for notifications too**: the same model writes the notification text and judges what looks suspicious. Notifications no longer have separate Gemini settings — an existing Gemini key moves over by itself.
- **Time-lapse scrubbing fixed**: no more black blinks, it now goes right up to the current time (ending on the live picture), and the picture no longer jumps or zooms when you let go (previews keep the camera's own shape).
- **Less disk work**: recordings are made quick to open only when they hold an alert, or the first time you open them, and are written once instead of twice.
- Security: an account limited to some cameras could export clips of the others — fixed. Exporting the setup without passwords now leaves out every key and camera login.
- Clean-up of unused code and settings.

## 0.5.5 — November 2026
- **Live sound explains itself**: when a camera's sound can't be played, GAZE now says why instead of "no supported source" — most often the camera stream is set to video only (on Hikvision: Video/Audio → Video → Stream Type "Video&Audio"). G.711 µ-law/A-law, AAC and other camera audio is converted for the browser, and the sub-stream's sound is used when only it has sound. The log says which sound each camera stream sends.
- **A nicer update bar**: the version change (0.5.4 → 0.5.5), when it was released, a progress bar while downloading, and **Try again** if an update fails. **Later** hides it until the next version.
- **⟳ Updates** shows a green ✓ after checking when GAZE is up to date.
- **Alerts filter**: new **Faces** and **Number plates** chips show alerts where a face was found or a plate read. Bicycle, bus and "Other objects" are gone from the chips (those alerts still show under "All alerts").
- **Dismiss all new** on the Alerts page clears every new alert at once and shows how many there are.
- **Pause background work** (Settings → Tasks): pause AI on alerts, previews, cloud backup or maintenance — or all of them — to lighten the load, and resume when you like. Recording, live view and new alerts carry on as normal.

## 0.5.4 — November 2026
- **Video on the graphics card**: GAZE now tests your GPU's video decoder and encoder at start-up and uses them by itself (Settings → GPU acceleration → Choose automatically), for every stream — so live view, motion detection and previews no longer load the processor.
- **What runs where** (Settings → Tasks): every AI model and video job, with GPU or CPU — and why, when it's the CPU.
- **Sound on every live picture**: a 🔇 speaker on each camera (grid, camera view, full screen) to hear it, with a volume slider when you hover it.
- Camera settings have an **Audio** tab: record audio, live sound and two-way audio together.
- Release builds use current GitHub Actions versions.

## 0.5.3 — November 2026
- **AI on the graphics card**: object recognition, smart search and faces & plates now run on the GPU (any recent NVIDIA, AMD or Intel graphics on Windows, through DirectML). The GPU runtime (about 25 MB) downloads by itself the first time. Choose the processor instead in Settings → Object recognition → Run AI on; if the graphics card can't be used, the processor takes over and the settings say why.
- Fixed: Windows could report the GPU as fully busy when it wasn't (engines of the same kind were added together), which made load balancing move video work to the processor.

## 0.5.2 — November 2026
- **Live sound**: the camera view has a 🔊 Sound button (volume on hover) to hear the camera — together with Hold to talk, a two-way conversation.
- **Alert pictures in HD**: every alert's picture is remade from the full-quality footage once it's over. Faces and number plates are only read from full-quality video.
- Alerts show just the object (“person 88%”) without colour words or dots.
- Camera view: the status card is titled with the camera's name, with ✎ Edit next to it.
- **Updates**: GAZE checks every minute; a bar under the header says when a new version is out (Update now / What's new / hide). ⟳ Updates next to your name checks right away. Automatic installing is now off by default and, when on, installs as soon as an update is found.
- Faces and number plates are on by default for new installations.
- Camera AI tab: the object list is the usual few (person, car, truck, motorbike, bicycle, dog, cat); “More things to watch for…” adds others (bus, bird, horse, suitcase…), which then also appear in the Alerts filter.
- On-screen display: items can sit in 9 places — the corners, the middle of each edge, or the centre (pickers and drag-and-drop).
- Camera settings: ◀ ▶ buttons switch to the previous/next camera without closing the editor (it stays on the same tab and asks before dropping unsaved changes).
- Time-lapse previews of the last 24 hours are prepared every minute (they were every 5) and the file still being recorded shows its latest preview at once instead of making you wait; previews are only removed together with their recordings (retention or low disk space).
- The red recording dot is back on camera tiles and in Settings → Cameras.
- The recording mode is now called “Substream 24/7 plus HD AI alerts”.
- Smart search downloads no longer leave a cut-off model that crashes GAZE; a model that does crash it is downloaded again, or smart search pauses instead of restarting over and over.

## 0.5.1 — November 2026
- First release published on GitHub: GAZE can now update itself from Settings → About.
- Model and update downloads show as a small “⬇ 51%” in the header instead of a banner across the page.
- The web port is simply typed in (no list of common ports).

## 0.5.0 — November 2026
**Major**
- **Updates from GitHub** (Settings → About): GAZE checks for new versions, downloads and checks them, and restarts itself — with one click, or automatically at night. No more building it yourself.
- **Two-way audio with push to talk**: a per-camera setting (with a beep test); hold the 🎙️ button in the live view to talk through the camera's speaker, or tap to keep talking.
- **System tray (Windows)**: no more console window; GAZE has an icon by the clock with open, recordings, log, start when you sign in, restart and quit.
- **Run as a Windows service** from the tray icon: starts with the computer (before sign-in), keeps recording after sign-out, restarts after a crash; the icon can turn it back into a normal program.
- **Tasks** (Settings → Tasks, also on the System page): see what GAZE is doing in the background — AI descriptions, faces & plates, downloads with progress, previews, cloud backup — plus *Rebuild previews*, *Refresh alert pictures* (sharper pictures from the HD recordings), *Describe alerts again* and *Look for faces & plates again*, and how many alerts are waiting.
- **Parked vehicles are ignored**: a car that stays in the same place stops making alerts and alerts again when it moves (camera settings → Detection).
- **Package delivery**: a doorstep area set to watch for parcels says “Package delivered” or “Package picked up”, with before/after pictures.
- **Looks at more of the video**: alert descriptions use several moments of the full-quality recording (not just one picture), and when no face (or no known face) is found GAZE searches more of the HD video for one.
- **Phone notifications** (ntfy): the alert picture right away, then a summary of what happened (with names, plates, parcels). Optional Google Gemini summaries that also judge what's suspicious; suspicious activity is sent with high priority. One per camera every few minutes (shorter at night); tapping opens the alert.
- **Smart load balancing**: when the GPU is busy, new live views, conversions and exports run on the CPU, and motion detection moves its decoding to whichever has room (Settings → GPU acceleration).
- **Camera AI tab**: object recognition, stationary objects (choose which kinds) and package delivery with its own drawing window now have their own tab in camera settings.
- **Notifications wait for the AI's description** and send it with the picture, which now shows a box and “PERSON 92%” on each object like the alert cards; tapping opens the alert (this computer's home-network address is used when none is set).
- **New faces & number plates inbox** (Settings → People & vehicles): unknown faces (grouped by person) and plates from recent alerts — assign them to someone, or hide them.

**Security**
- Sign-in: repeated wrong passwords lock out the user name *and* the address they come from, for longer each time (30 s up to 15 min); a wrong user name takes as long as a wrong password; password checks no longer slow down the rest of GAZE.
- Sessions: only a fingerprint of each sign-in is stored, so the sessions file can't be used to sign in (everyone signs in once after updating). The cookie is marked Secure over HTTPS.
- Requests from other websites (including live video and talk connections) are refused; pages can't be framed by other sites.
- Passwords need at least 8 characters. The old single login in `nvr.toml` (versions before 0.4) becomes an account and its password is removed from the file.
- Settings, accounts and sessions files can only be read by their owner (Linux). Logs and system details are for admins only; viewers only see the cameras they're allowed in "New faces & plates".
- Number plates the reader isn't sure of are ignored.

**Smaller changes**
- Alerts are only described and identified once they're over; notifications, descriptions and faces added while an alert is still going are no longer lost.

## 0.4.0 — November 2026
**Major**
- **GAZE**: new name and logo.
- **User accounts**: the first visit sets up an admin account. The admin can add users as *Admin*, *Operator* (also moves PTZ cameras, pauses recording, removes alerts) or *Viewer* (watch only), optionally limited to some cameras. A proper sign-in page replaces the browser's login pop-up.
- **Personal layouts**: each user has their own camera layout, kept separately for phones and computers. Layouts are easier to edit on a phone.
- **Alerts tab**: its own tab with a count of new alerts, pop-up review (the list stays put), new/viewed marks and a description of what happened.
- **Fast timeline**: a light stream 2 copy is recorded alongside HD for instant browsing; clicking an alert plays it in HD and carries on in stream 2. Finished recordings open and seek much faster.
- **Linked profiles**: cameras follow a profile; changing the profile changes every camera that follows it.
- **Pan / tilt / zoom on the picture**: drag on the video, scroll to zoom, presets on screen.
- **Rotate** a camera's picture in 90° steps.
- **Zones**: areas and lines drawn on the picture, with line crossing, lingering and items left or taken (packages).
- **Faces & number plates**: GAZE recognises the people and vehicles you teach it (Settings → People & vehicles, or "Who is this?" on an alert).
- **Find cameras**: scans the network for cameras, and once a day tells you about new ones.

**Smaller changes**
- Camera offline notice, error-only log count, one Save button for camera settings, HD clip downloads, volume slider, data saver in Settings, bandwidth shown, CPU use that matches Windows, GPU name and usage, on-screen display fixes and corner pickers, many small fixes.

## 0.3.0 — November 2026
**Major**
- **Smart search**: find alerts by description ("kid with a red backpack", "white van") with a CLIP model on this computer.
- **Bigger AI models**, double-checking of alerts by a second model, and night boost for dark pictures.
- **Camera settings over ONVIF**: IP address, time, video streams and picture settings, changed from GAZE.
- **Data saver** for viewing away from home; **Stream 1 / Stream 2** on every picture.
- **Crash and power-cut protection** for recordings.

**Smaller changes**
- Detection boxes on alert pictures, colour search, recording profiles, phone-friendly layout, double-click for full screen, smarter CPU/GPU use, many fixes.

## 0.2.0 — November 2026
**Major**
- **AI object detection** (built-in, local server or cloud), alerts with clips and a pre-alert buffer.
- **Video wall and camera groups**, drag-and-drop dashboard, multi-camera timeline with time-lapse previews.
- **PTZ control**, per-camera on-screen display, finer motion detection settings.

**Smaller changes**
- One shared connection per camera stream, system monitor and logs, playback conversion for H.265, fixes.

## 0.1.0 — November 2026
- First version: record IP cameras, live view and recordings in the browser, multiple cameras, GPU acceleration.
