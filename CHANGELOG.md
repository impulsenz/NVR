# What's new in GAZE

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
- **Two-way audio**: talk through a camera's speaker from the camera view.
- **System tray (Windows)**: no more console window; GAZE has an icon by the clock with open, recordings, log, start with Windows, restart and quit.
- **Tasks** (System page): see what GAZE is doing in the background — AI descriptions, faces & plates, downloads with progress, previews, cloud backup — plus *Rebuild previews* and *Refresh alert pictures* (sharper pictures from the HD recordings).
- **Parked vehicles are ignored**: a car that stays in the same place stops making alerts and alerts again when it moves (camera settings → Detection).
- **Package delivery**: a doorstep area set to watch for parcels says “Package delivered” or “Package picked up”, with before/after pictures.
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
- **PTZ control**, per-camera on-screen display, Blue Iris-style motion detection.

**Smaller changes**
- One shared connection per camera stream, system monitor and logs, playback conversion for H.265, fixes.

## 0.1.0 — November 2026
- First version: record IP cameras, live view and recordings in the browser, multiple cameras, GPU acceleration.
