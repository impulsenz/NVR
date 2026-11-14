# What's new in GAZE

## 0.6.94 — November 2026
- **Save buttons show that they worked**: every button gives a little when pressed. Save buttons (Save, Apply, Add, Import…) also pulse when clicked, show a spinner while saving, then flash green with a ✓ — or shake red if the save failed (the message says why). The tick finds the button again even when the page redraws after saving.
- **Profiles update their cameras straight away**: saving a profile now applies its live-view and display changes to every camera using it at once, as the camera's own window does; other changes restart those cameras with the new settings, as before.
- **Sound detection (and faces & plates, description search) moves to the GPU once it's ready**: a model loaded before the GPU was ready used to stay on the processor until GAZE restarted ("the GPU runtime wasn't ready yet…"). It now loads again on the GPU by itself — and back on the processor if that's chosen.
- **Default recordings folder removed** from Settings → Recording & storage: each camera's folder is set in its Recording tab (or by its profile). Cameras without their own folder carry on where they were.

## 0.6.93 — November 2026
- **Shorter recording files**: Default file length (Settings → Recording & storage) and each camera's File length now also offer **1 second, 5 seconds, 10 seconds and 15 seconds**. Very short files make thousands of files a day, so browsing and time-lapse previews get slower; a file also can't be shorter than the camera's keyframe interval (often 1–4 seconds).

## 0.6.92 — November 2026
- **Bring in another GAZE's cameras** (Settings → System → Other GAZE servers → Check its cameras): tick the cameras you want and press **Bring in the ticked cameras**. They become cameras of this GAZE — on the Live page, the timeline, time-lapse, Recordings and Alerts — while **their footage stays on that server** and is read from it as you watch, as if it were stored here. Nothing opens that server's own page or address.
  - Choose **Record here too** instead to record them on this GAZE as well (with alerts and AI here).
  - Alerts you view are marked as viewed on that server too; deleting one of their clips deletes it there (admins only).
  - Cameras already brought in are shown as such, and no longer appear a second time as view-only pictures.
- **Alerts switch to full quality by themselves**: an alert opened while it's still going (or just over) plays from the continuous recording — often the lower-quality stream 2 — until its full-quality clip is saved a few seconds after the activity stops. It now changes over to the full-quality clip at the same moment as soon as it's ready, instead of staying in standard quality. This was most noticeable with sound alerts, which last as long as the sound does.
- **CPU reading matches Task Manager more closely** (Windows): GAZE now reads Windows' own "% Processor Utility" figure directly (the one Task Manager shows), in any Windows language. Before, it went through a helper program, and if that failed GAZE quietly used a different measure that didn't line up with Task Manager.

## 0.6.91 — November 2026
- **Other GAZE servers, reworked** (Settings → System):
  - The address is entered in parts — http or https, the address, and the port — and shown put together.
  - **Check its cameras** lists every camera that server offers. Tick the ones to show on your Live page; if only some appear, the token was made by an account that can see only those (make it with an admin account there).
  - **View only**: clicking another server's camera on the Live page opens it bigger, moving, right here — no more being sent to that server's sign-in page, and its settings stay its own.
  - **Record here**: any of its cameras can become a camera of this GAZE — recorded here, with motion, AI and alerts here, like your own cameras (the video comes through that GAZE, so it works across networks the other GAZE can reach).
- **Layouts preview as you hover**: in the Layouts window (also in full screen), hovering a layout arranges the cameras behind it that way, so you can see it before choosing; Cancel puts yours back.
- **Room is always kept free on the disk**: at least 2 GB or 1% of the disk (or more, if set), even if the setting is lower or 0 — a full disk would stop recording. When room is needed, a little extra is cleared in one go, rather than file by file at the edge.
- **Deleting shown on the timeline**: while a camera's oldest footage is being deleted to keep room free, the start of its footage on the timeline shows a moving 🗑 mark.
- **Recordings folders are called like the cameras** (e.g. "Front Door" instead of "front"). Existing folders are renamed once when GAZE starts (instantly, on the same disk; kept as they are if that name is taken). Renaming a camera renames its folder too, in the background.
- Fixed: the other servers' camera pictures on the Live page could pile on top of each other.

## 0.6.90 — November 2026
- **CPU and GPU readings, accurate and every second**: the System page's CPU, GPU and memory now update every second (the history graph keeps its one-minute steps). The readings come from counters that keep running, the way Task Manager reads them: before, a counter program was started for every reading, and its own start-up work made the CPU look busier than it was (e.g. 73% where Task Manager said 50%).
- **Where the processor time goes**: under the CPU card, *Where GAZE's share goes* shows how much motion detection, live pictures, live video, recording, conversions, sound detection and GAZE itself are using.
- **GPU acceleration → Choose automatically now measures**: at start-up every graphics-card decoder that works (NVIDIA CUDA first, then DirectX, Intel) decodes a full-HD and a sub-stream-sized test clip, and the one that leaves the processor least busy is used — for the small sub-streams too when that's cheaper on the card. The result shows in Settings → GPU acceleration.
- **ffmpeg comes with GAZE on Windows**: if ffmpeg isn't installed, GAZE starts anyway, downloads it by itself (a build with NVIDIA, Intel, AMD and DirectX video support; progress under System → Tasks, with a note at the top of the page), and restarts using it. No separate install needed.

## 0.6.89 — November 2026
- **Scrubbing a camera's timeline** (after clicking a camera on the Live page) no longer makes the picture vanish and come back as a sheet of tiny thumbnails: the picture keeps its size and shows the time-lapse picture of where you are.
- **HD on the timeline, never black**: an alert's HD clip is only shown once it actually has a picture. If this browser can't show the camera's HD format (often H.265), only the HD clips are converted (started a few seconds ahead), while the sub-stream keeps playing directly. Before, one such clip could switch the whole camera to slow converted playback, sub-stream included — a likely cause of black screens when clicking alert pictures on the timeline, and of extra work for the computer.
- **Editing older profiles**: the Edit button works again on profiles made before 0.6.79 (it failed on settings those profiles didn't have yet). Saving brings them up to date.
- **Only administrators can delete** alerts, clips and recordings; other accounts don't see the delete buttons. An opened alert has a 🗑 Delete button (alerts can also still be deleted from their picture).
- **Export needs your password**, and lets you choose what goes in: cameras & settings, people & vehicles (with the faces GAZE has learned and the corrections), and users. Backups now carry a version note: every GAZE reads backups from older versions, and an older GAZE reads what it knows of a newer one.
- **Voices download by themselves**: every natural voice is listed under Settings → Voice & talking → Voice; choosing one (or importing settings that use one) downloads it automatically, and the computer's own voice speaks until it's ready.
- **Moving a camera's recordings to a new folder** no longer keeps the window open: the camera records into the new folder at once, and the older recordings move in the background (progress under System → Tasks).
- Alert cards show the HD AI badge above the camera name. The version next to GAZE sits on the same line as the rest of the top bar, a little bigger.

## 0.6.88 — November 2026
- **Alerts in full quality on the timeline, without a break**: watching the sub-stream recording, the picture now changes to the alert's HD clip right where the alert starts, and back to the sub-stream where it ends, with no blink, skip or wait. The HD clip is loaded a few seconds ahead, and the sub-stream keeps going underneath, ready to take over again. A clip the browser can't play (e.g. H.265 in some browsers) is skipped and the sub-stream carries on.
- **Alert cards**: the HD AI ✓ / ? badge lines up with the camera name and time. Sound alerts' "Heard: …" now goes across the middle of the picture, where the card's captions don't cover it (for new alerts).
- **Day and night colours, reviewed**: the REC badge on Settings → Cameras is readable in day colours, and the "Also search" box next to Scan network looks like the other boxes.

## 0.6.87 — November 2026
- **Drop-down lists over the video are readable**: the playback speed list in full screen (and the presets list on a pan / tilt / zoom camera) showed white text on white, visible only under the mouse. Every drop-down list now shows its choices clearly, in day and night colours.

## 0.6.86 — November 2026
- **Opening a camera shows only that camera**: switching from one camera straight to another could bring back the previous camera's picture a few seconds later (until a page refresh). The previous camera's video, kept a few seconds in case you come back, no longer touches the picture of the camera you opened.
- **No more frozen picture when a camera is both in the grid and opened on the same stream** (e.g. S2 in both): the opened picture now plays its own video instead of sitting on a still.

## 0.6.85 — November 2026
- **Pan / tilt / zoom, roaming and tracking**: the button under the presets now shows the mode the camera is in — **Off** (stays where it is), **Roam** (visits the ticked presets in turn), **Track** (follows a person the AI finds, then goes back) or **Roam + track** (roams; when the AI finds a person it stops roaming, follows them until they're gone, then carries on). It's green and turning while active. The stay at each preset can now be anything from **0 seconds** (keeps moving from one preset to the next as soon as it gets there) upwards.
- **Roaming works reliably now**: it used to stop after the first move, because the camera's own movement set off motion alerts and roaming paused during alerts. The camera's own movement no longer counts as motion (no more false alerts while it roams), and the AI keeps looking for people while it moves when tracking is on.
- **Delete a preset**: 🗑 next to ☆ deletes the chosen preset (it's taken out of the roaming list too).
- **Move the PTZ controls**: drag them anywhere on the picture by the grip at the top; the place is remembered per camera on this device (double-click the grip to put them back).
- **Download more voices**: Settings → Voice & talking → Download voices: natural-sounding free voices (British, American, German, French, Spanish …) from the open-source Piper project. They run on this computer; nothing is sent online.
- **Say… queues**: messages sent quickly one after another (or while someone talks or the siren sounds) are said in turn instead of giving a "busy" error.
- The **version number** now shows next to GAZE in the top bar.

## 0.6.84 — November 2026
- **🚨 Siren** to scare off intruders: on the live view (and full-screen toolbar) of a camera with two-way audio, Siren sounds a loud wailing alarm through the camera's speaker with spoken warnings in between, such as "You are being recorded. The homeowner has been notified" and "Leave the property now. The police have been notified". It runs for the set time or until you press **Stop siren**, and is kept in the recordings. Set it up in **Settings → Voice & talking → Siren**: how long it sounds, your own warnings (or pick from ready ones), whether to speak them, and **Hear the siren here**.
- **Tapo cameras**: choosing the Tapo brand in a camera's settings now picks **UDP** for the video (much smoother on Tapo's Wi-Fi cameras) and Tapo's pan / tilt / zoom control port **2020**. Nothing else is changed, and cameras already set up stay as they are.

## 0.6.83 — November 2026
- **Presets from the keyboard**: on a pan / tilt / zoom camera's live view, press **1–9** (and 0 for the tenth) to go to that preset. The preset list shows each one's number.
- **Roam between presets** right from the camera's picture: the new ⟲ button next to ☆ lets you turn roaming on or off, tick which presets to visit and set how long to stay at each. It lights up while roaming. The same settings are in the camera's Pan / tilt / zoom tab.
- The "Moving to the preset…" message is gone.
- **Day colours**: the camera name and clock on live pictures, and the coloured person / vehicle tags, are now really readable in day mode (0.6.79 missed a spot).

## 0.6.82 — November 2026
- **Settings → Voice & talking** for 💬 Say…: choose the **voice** (every voice on the computer, including the newer natural Windows voices you add in Windows Settings → Time & language → Speech), the **speed** and **loudness**, an optional **chime** before speaking, and your own **ready phrases** (the buttons under the text box). **Hear it here** plays the voice in your browser exactly as the camera gets it; **Say on camera** tries it on a camera.
- **Plain motion alerts no longer count as new**: the Alerts tab's number and "Dismiss all new" count only alerts where something was recognised (people, vehicles, sounds…). Motion-only alerts are still there to look up and filter, just without the NEW mark.
- **Desktop shortcut**: when GAZE is first installed, and after each update, it checks that there's a shortcut to GAZE.exe on the desktop and makes one if not.

## 0.6.81 — November 2026
- **Two-way audio on Tapo cameras**: Push to talk, Say… and the speaker Test now work with TP-Link Tapo (and VIGI) cameras, which don't offer the usual ONVIF speaker connection. GAZE talks to them the way the Tapo app does. To set it up: camera settings → Audio → tick *Talk through this camera's speaker*, enter the password of the **TP-Link / Tapo app account** the camera is in (not the camera account made for the video) and press **Use for talking**, then Test. Newer firmware with the stronger login is handled automatically. If the camera says it's busy, close the Tapo app's live view and try again.

## 0.6.80 — November 2026
- **Installs like a normal Windows program**: run a freshly downloaded GAZE.exe and it puts itself in its own program folder (your user's Programs\GAZE folder, no administrator needed), with a **GAZE** shortcut in the Start menu and one on the desktop. An existing set-up (settings next to the program) stays where it is and just gets the shortcuts.
- **The GAZE shortcut** starts GAZE when it isn't running, and then opens it: in the installed GAZE app if you have one, otherwise in your web browser. Opening GAZE again while it's already running no longer shows an error; it simply opens it.

## 0.6.79 — November 2026
- **Live view, more reliable**:
  - A live picture that freezes now starts again by itself within a few seconds (with the still pictures standing in meanwhile), instead of needing a page refresh.
  - "Loading full quality…" no longer stays up when the HD picture is already showing, which happened mostly when going full screen.
  - The S1 / S2 choice now belongs to the camera, in the grid, the camera view and full screen alike, so it doesn't flip back to the other stream when you go in or out of full screen.
- **Say… (text to speech)**: on the live view of a camera with two-way audio, 💬 Say… lets you type words (or pick a ready phrase), and the camera's speaker says them. It uses the voice built into Windows; what's said is kept in the recordings like Push to talk. Also in the full-screen toolbar when live.
- **Profiles cover sound too**: sound detection (what to listen for, sensitivity), the live speaker button, the doorbell chime, and using what the camera itself detects. A profile's window now has an Audio tab. Two-way audio itself, privacy areas, ignore areas and zones stay with each camera. Profiles made before keep each camera's own sound settings until you save them again.
- **Day colours**: the on-screen clock and name on live pictures, and the coloured person / vehicle tags, are readable again in day mode.

## 0.6.78 — November 2026
- **Search settings**: a search box at the top of Settings finds any setting, including the ones in each camera's window, and takes you straight to it (arrow keys and Enter work too).
- **Alert cards**: symbols for what was found at the top left, NEW at the top right until you've opened it, how long ago at the bottom left, and the date and time at the bottom right.
- **Full screen**: the toolbar now disappears as soon as the mouse leaves it. A **● Live** button in the full-screen toolbar jumps to the live picture in the same window, with Push to talk for cameras that have a speaker. The extra full-screen button at the bottom right of the live picture is gone.
- **"Recording now"** stripes at the end of the bar are 20% longer and fade in smoothly. While the HD main stream is shown, the legend's "Recording now" is purple to match.
- **ONVIF tab is back** in each camera's window: the camera's own network address, date & time, video streams (resolution, frame rate, bitrate) and picture settings, with Apply to the camera, Restart, and a button to **open the camera's own web page** (also next to the IP address on the General tab).
- **Adding cameras**: the Format drop-downs under Streams are gone (the brand above fills in the addresses). Test connection shows the picture turned as chosen under Rotate picture. Scan results stay when you add a camera, with "already added" updated, until you press Scan network again.
- **AI defaults for new cameras**: watch for people and vehicles, 70% sure, and alerts only for recognised objects (plain motion alerts only when object recognition is off). Bicycle, dog, cat and the rest are now under "More things to watch for", for alerts and for objects that stay in one place.
- **Updates**: the progress ring fills smoothly in 1% steps.
- **"GAZE can't be reached" page** tries again every 10 seconds by itself, without a counter.

## 0.6.77 — November 2026
- **Day and night colours**: 👤 menu → Colours on this device: ☀️ Day (light), 🌙 Night (dark), or Automatic, which follows the device's own light/dark setting (and changes with it). Remembered per device, for the browser and the app alike. Camera pictures, video and their on-screen display keep their dark surroundings in both.
- **Saving a camera after choosing a profile**: if the camera can't be saved (for example the profile's recording mode needs motion detection, which the camera has off), the window now stays open with the reason right next to Save, instead of closing and flashing back with the message out of sight on another tab.

## 0.6.76 — November 2026
- **"Start with Windows" works**: GAZE set to start with Windows didn't start — Windows starts programs in its own system folder, where GAZE couldn't find or write its files. GAZE now always works in its own folder, and the start-up entry says which folder (it's corrected by itself the next time GAZE starts).
- **Old copies of GAZE are tidied away**: the copy replaced by an update (GAZE.old.exe, nvr-windows-x64.old.exe …) is now deleted once it has closed, whatever its name — before, it could stay behind.
- **"Try again" shows it's trying**: on the "GAZE can't be reached" page, the button shows a spinning ring and "Connecting…" while it tries, and the page keeps trying by itself every few seconds — GAZE opens as soon as it answers.
- **Sign-in settings**: Settings → Users → Signing in. **Stay signed in for** sets how long "Keep me signed in" lasts (1 day to 1 year; was always 30 days), also while the app or browser is closed. **Sign out when not used** (off by default; 5 minutes to 8 hours) signs out after that long with no mouse, keyboard or touch on the page, and ends a sign-in nothing has used for that long (e.g. the app closed). The sign-in page then says why.

## 0.6.75 — November 2026
- **The program file is now GAZE.exe on your computer too**: GAZE installed from an older download (nvr-windows-x64.exe) makes itself GAZE.exe in the same folder the next time it starts, carries on from there, and removes the old file. "Start with Windows" is pointed at GAZE.exe. Settings, recordings and everything else stay where they are. (GAZE running as a Windows service keeps its file name, which the service is registered with.)

## 0.6.74 — November 2026
- **Test connection works with cameras that allow few connections**: testing a new camera could fail on one stream with "method SETUP failed: 500" even though both streams work once saved — the test opened four connections to the camera at once, more than many cameras (Tapo, Reolink …) allow. It now checks the streams one after another, then shows both pictures, and tries again by itself if the camera says it's busy.
- **New cameras start with the same login**: adding another camera fills in the username and password of the camera added last (most cameras in one home share them), with a note saying so — change them if this camera's are different.
- **Cameras are looked for straight away**: on a new GAZE with no cameras, the Live page starts scanning the network by itself as soon as it opens.
- **Info pop-ups show above windows**: the explanations behind the little ⓘ icons appeared behind the camera window and other pop-up windows; they now show on top.
- One eye per password box (the camera window showed two).

## 0.6.73 — November 2026
- **GAZE is called GAZE on the computer**: the program now carries its name and icon, so Task Manager, the taskbar and the file's properties show **GAZE** with the GAZE eye. Releases offer the program as **GAZE.exe** (new installs download that); the older nvr-windows-x64.exe stays alongside so installed copies keep finding their updates. An installed copy keeps its file name when it updates — rename it to GAZE.exe while GAZE is stopped if you like (then switch "Start with Windows" off and on again in the tray menu, so it points at the new name).
- **Open in GAZE app**: when GAZE is installed as an app on this computer, the 👤 menu in the browser offers **📲 Open in GAZE app** (where "Install GAZE as an app" was), which opens the same page in the app. The first time, the browser asks whether GAZE may open the app.
- **See the password you typed**: every password box (sign-in, users, camera logins, notifications …) has an 👁 to show or hide what you typed.

## 0.6.72 — November 2026
- **Scan for cameras from the Live page**: on a new GAZE with no cameras yet, the Live page now has a **🔍 Scan network for cameras** button (as in Settings → Cameras) next to adding one by hand. Cameras found are listed there; **+ Add** opens the camera window with its address, brand and a name filled in, ready for the camera's login. Cameras on another network can be found by adding it (e.g. 192.168.5.0/24) under "Cameras on another network?".

## 0.6.71 — November 2026
- **Updates install more reliably on Windows**: installing an update could fail with "could not move the running program aside: Access is denied". GAZE now tries a few times (antivirus programs often hold a new file for a moment), uses another name for the old copy when an earlier one is still in use, and — if Windows still says no, e.g. because the program file was put there by an administrator or by the GAZE service — asks once for permission with the usual Windows prompt on the computer's screen, then finishes the update. If that's refused too, it says why and how to update by hand.

## 0.6.70 — November 2026
- **Cars, trucks, buses and motorbikes are now all "vehicle"**: the AI often mixes these up (a car taken for a bus), which could stop an alert from going off. GAZE now treats them as one kind — vehicle — everywhere: what each camera watches for, alerts and the Alerts filter, notifications, smart-home and webhook choices, zones, PTZ following, number plates, the HD check, and parked (stationary) vehicles, so a parked car later seen as a truck is still the same parked vehicle. Two boxes on the same vehicle (one "car", one "truck") become one. Your settings are changed over by themselves, and older alerts show and filter as vehicles too. Bicycles stay separate.

## 0.6.69 — November 2026
- **Every picture's on-screen display works the same way**: the live grid, a camera's view, the Recordings tab, an alert's player and full screen now all use one on-screen display. Your display settings (camera name, extra text, date and time, status, in the places you chose) apply everywhere, including recordings. A recording shows its own time in the clock's place and a blue **▶ PLAYBACK** tag where live shows the camera's state (REC, armed, offline), so it's always clear which you're looking at.
- **The buttons are in the same place everywhere**: one row in the same corner (top right, or the corner chosen for the full-screen button) — stream 1 / 2 (live only), 🔊 sound with its volume, and ⛶ full screen. Players keep their playback bar (play, frames, speed, time, clip, download) along the bottom; the full-screen bars no longer repeat the sound and full-screen buttons. A camera's own view now has the full-screen button on its picture too.
- **Bigger, sharper previews when hovering a player's bar**: the picture shown while you point along a recording's bar (and along a camera's timeline) is the time-lapse picture at its full size — up to 480 pixels wide with "Sharpest" time-lapse pictures — instead of a small 192-pixel one.

## 0.6.68 — November 2026
- **Live button on the Recordings tab**: ● Live shows what's happening now on every camera, with the timeline's playhead kept at now. Press Play to watch the recording from a few seconds ago, or drag or click along the timeline to go back in time.
- **Sound on the Recordings tab**: the 🔇 Sound button turns sound on or off, and 🔈 on a camera's picture chooses which camera you hear (one at a time). It follows that camera's recording, or its live sound in Live; it goes quiet above 4× speed.

## 0.6.67 — November 2026
- **Ready-made layouts for the Live page**: the new ▦ Layouts button offers 15 layouts, each previewed with your own cameras: equal grids (best fit, 2, 3, 4 or 6 across), one big picture with smaller ones around it (1 + 5, 1 + 7, 1 big in the middle with 12 around, 1 + 2 beside, 1 big with a strip below, 1 big + 2 medium), two big (side by side, or stacked with a column beside), three big and four big. Choose which cameras get the big spots; cameras beyond a layout's spots carry on in rows of small pictures below. It's saved for the group you're looking at (and the full-screen view has its own), and Edit layout still lets you fine-tune.
- **Clear choice when changing time-lapse settings**: saving new time-lapse settings now asks whether they apply **from now on only** (what's already made stays, nothing is rebuilt — the default) or to **remake the older time-lapse** too.

## 0.6.66 — November 2026
- **Check faces page**: Settings → Check faces shows every face GAZE found in alerts, so you can go through them in one place. Choose what to see (to check, confirmed, not named, or all), one person or everyone, and how many days. **✓ Right** adds a face to that person so GAZE knows them better; **✎ Wrong?** says who it really is (or nobody you know); **✓ All are right** confirms every face shown at once. Click a face to open its alert. The ✓ Right button in each alert stays too.
- **The time-lapse "ready" line fills in faster**: on the Recordings tab the blue line under each camera now appears sooner (every camera is asked at once instead of one after another), fills in as soon as a time-lapse shows while you drag, and keeps up while the timeline slides past its edge.
- **Quick times for time-lapse videos**: the Time-lapse video window has Last 24, 12, 6, 3 and 1 hour buttons.
- **PTZ controls only on the live picture**: the pan / tilt / zoom pad is hidden while a recording plays in the camera view, or while you drag along its timeline.

## 0.6.65 — November 2026
- **A preview for each camera link**: Settings → Smart home → Each camera on its own now shows a small live picture beside each camera, from the stream chosen for its link, refreshed every 2 seconds while it's on screen. Click it to open the link.

## 0.6.64 — November 2026
- **Sound alerts have a picture saying what was heard**: when sound detection sets off an alert, its picture is now taken from the camera at that moment, with what was heard written across the bottom ("HEARD: SMOKE OR FIRE ALARM"). It shows on the alert card, in the alert and in the notification, and stays when the picture is remade in full quality from the recording. If the AI recognised someone or something in the same alert, that picture is used instead.
- **Sound detection works with motion detection off**: a camera with motion detection and AI switched off still raises sound alerts (and the camera's own detection alerts), and those alerts are no longer marked as motion.

## 0.6.63 — November 2026
- **Faster sliding at the timeline's edge**: dragging the time-lapse past either end of the Recordings timeline now slides it twice as fast as before (about two thirds of the view a second at the edge, up to four times that further out).

## 0.6.62 — November 2026
- **Scrub past the edge of the timeline**: on the Recordings tab, dragging the time-lapse to the left or right end of the timeline now slides the timeline along instead of stopping there. Hold at the edge to move slowly; go further past it to move faster. It moves back as far as the dates you've chosen and forward up to now, and stops as soon as you move back inside the timeline.

## 0.6.61 — November 2026
- **Previous / next alert plays in full quality**: on the Recordings tab, ⏮ Alert and Alert ⏭ now play the alert from the main stream (as clicking it on the timeline does), instead of the low-resolution sub-stream copy. Other cameras on the timeline carry on as they were.
- **No Refresh button on the Recordings tab**: the timeline keeps itself up to date. Alerts still has it.

## 0.6.60 — November 2026
- **Search alerts by description**: switch it on in Settings → Object recognition → Search alerts by description. Then type what you're looking for on the Alerts page — "a man in a white shirt", "someone carrying a box", "a van" — and GAZE shows the alerts whose picture looks like it, without anyone having labelled them. The camera, date and type filters still apply. It uses CLIP (by OpenAI, open source), which downloads once (420 MB) and then looks at each alert picture on this computer. New alerts are ready within a minute; older ones (the last 30 days) are done in the background when the computer is quiet. When nothing matches clearly, it says so and shows the closest few.
- **Sound filter on the Alerts page**: alerts from sound detection have their own 🔊 Sound chip.

## 0.6.59 — November 2026
- **Sound detection**: camera → Audio → Sound detection. GAZE listens to the camera's microphone and alerts on what you tick — glass breaking, a smoke or fire alarm, screaming or shouting, a gunshot or bang, a dog barking, a baby crying, a siren, a car alarm, knocking or the doorbell, or talking — even when nothing moves in the picture. A sensitivity slider sets how faint a sound counts, and the tab shows live what the camera is hearing. Sound alerts carry a 🔊 label, say what was heard, and send a notification. It uses YAMNet (by Google, open source), which knows hundreds of everyday sounds and downloads by itself the first time (16 MB). It runs on this computer every half second, and the sound never leaves it.

## 0.6.58 — November 2026
- **New recording mode: HD 24/7 plus HD alert clips**: camera → Recording. Full quality around the clock, and each alert also saved as its own full-quality clip (with the seconds before it). The clips are kept for as long as Storage → "Keep alerts for" says, even after the 24/7 footage around them is deleted to make room.
- **A private link for each camera**: Settings → Smart home → Overview → Each camera on its own. Choose the stream (low resolution or full quality), how many pictures a second, and a moving picture (MJPEG) or the newest still (JPEG), then copy or open the link. Each camera's link has its own key and opens only that camera's picture; "New link" replaces them all.
- **Backups now include everything you've set up**: Settings → Backup now downloads one .zip with every camera and setting (profiles, zones, masks, modes, notifications, smart-home links), users with their camera access and layouts, and People & vehicles with their faces, plates and photos. Without "Include passwords", every password, key and token is left out (live-streaming keys, MQTT and other GAZE servers' tokens too). Importing keeps the passwords this computer already has, so cameras don't lose theirs; older .toml backups still import.

## 0.6.57 — November 2026
- **Work goes back to the graphics card when it's quiet**: with smart load balancing on, video work that moved to the processor because the graphics card was busy (live view decoding, recording conversion) now moves back once the graphics card has been quiet for a minute, instead of staying on the processor until GAZE restarts. A recording being converted finishes its file and carries on in a new one.
- **Faces you confirm or correct teach GAZE**: an alert's recognised face now has a ✓ Right button. Confirming it adds that face to the person's faces, so they're recognised more surely, and it shows as confirmed. Choosing who it really is still adds the face to the right person and stops GAZE taking it for the wrong one.
- **Object recognition test shows every step**: Settings → Object recognition → Test on this camera now shows the picture from each step GAZE takes before an alert, with what it found and how long it took: the quick check (your chosen model), the double-check (the bigger model), and the HD check on a full-quality picture, where each find is shown confirmed, seen as something else, or not seen. Things the camera isn't watching for show faintly.

## 0.6.56 — November 2026
- **Time-lapse dragging is much faster**: scrubbing along the timeline (Recordings, and a camera's own timeline) now keeps up with your finger or mouse — working out which recording you're over was being redone for every file at every move, which slowed down as recordings built up (about 17× quicker now), and pictures update once per screen frame.
- **Live sound no longer stops and starts**: the sound keeps a little more in hand (cameras send sound in bursts) and catches up by playing a touch faster instead of jumping.
- **Smoother live video on Tapo and other Wi-Fi cameras**: the live player rides out bursts of video instead of skipping ahead, catching up gently when it falls behind.
- **Correct frame rate on live view**: the fps shown is now the camera's real video frame rate (it counted the still pictures and the video together before).

## 0.6.55 — November 2026
- **No more flicker opening the Live tab**: coming back to Live from another page or from a camera keeps each camera's video playing instead of starting it again, so it no longer drops to a still picture or goes blank for a moment.
- **Full-screen camera timeline fixed**: in a full-screen camera, moving or clicking across an alert on the timeline no longer brings back the old timeline. The alert plays in full screen, and the timeline returns to its place when you leave full screen.
- **Green tick when settings save**: saving any setting now shows a ✓ on the button you pressed and in the message, so it's clear it worked.
- **"Open alert" in notifications**: ntfy notifications now have an Open alert button next to the snooze buttons.
- **Alert pictures show the whole view**: the zoom-in on what the AI found is gone from alert thumbnails; the box around it still shows.

## 0.6.54 — November 2026
- **Stream live to YouTube, Facebook or Twitch**: camera → General → Stream live, with the service's RTMP address and stream key. The camera's own video is passed through (H.265 cameras are converted), with sound added when the camera has none; it restarts by itself if the connection drops.
- **Fisheye (360°) cameras**: camera → General → Fisheye. The live view and snapshots become a normal flat picture — choose where to look and how wide; recordings keep the whole circle.
- **Your own AI model**: Settings → Object recognition → AI model → Your own model. Any YOLOv8 / YOLO11 (Ultralytics) or YOLOX detection model in ONNX format, with its picture size and class names — e.g. one trained to tell a fox, your dog or a delivery van. Its classes appear in each camera's "More things to watch for".
- **Other GAZE servers**: Settings → System → Other GAZE servers. The cameras of another GAZE (a holiday house, the shop) show on this Live page; click one to open it there.
- **API tokens**: 👤 menu → API tokens. Keys for scripts, Home Assistant or another GAZE to use GAZE as you (Authorization: Bearer …); removing one stops it at once.
- **Safe mode**: if the settings file can't be read (a typo when editing it by hand, a damaged file), GAZE now starts anyway without touching it, shows exactly what's wrong, and lets you fix the file on the page or restore the settings from the last good start.

## 0.6.53 — November 2026
- **Two-step sign-in**: 👤 menu → Two-step sign-in. After the password, GAZE asks for the 6-digit code from an authenticator app (Google or Microsoft Authenticator, Authy, a password manager) — a stolen password alone isn't enough. Eight one-time recovery codes for a lost phone; an admin can turn it off for someone (Settings → Users).
- **Overview of all cameras**: every camera in one moving picture, or only the ones with something going on — for a wall tablet, a slow connection or a Home Assistant dashboard. A private link works without signing in (Settings → Smart home), and a new link stops the old one.
- **PTZ patrol**: a moving camera can visit chosen presets in turn, a set time at each; it pauses while something is going on and for 2 minutes after you steer it by hand.
- **Follow people (auto-tracking)**: a moving camera turns to keep the person (or car, dog…) the AI finds in view, then goes back when they've gone.
- **Speed**: give an area along a road its real length, and alerts note how fast things went through it ("Car going 42 km/h through Road").
- **Privacy areas**: black out parts of a camera's picture for good (a neighbour's window, a footpath) — in the live view, new recordings, snapshots and alert pictures, and nothing is detected there. Camera → General.
- **Time-lapse video**: Recordings → 🎞 Time-lapse video downloads a sped-up MP4 of any camera and time (a whole day in about 5 minutes), made in seconds from the time-lapse pictures.

## 0.6.52 — November 2026
- **Smooth live video**: live pictures are now the camera's own video, played by your browser at the camera's full frame rate with half a second of buffer to smooth out Wi-Fi hiccups — much smoother (Tapo cameras especially) and far less work for the computer. The old picture stream is still used as the first picture, for data saver, and for browsers that can't play the camera's format. Settings → Live view & display → Live pictures switches it per browser.
- **Secure address (https)**: GAZE also answers on https (port 8443). It makes its own certificate (browsers warn once), or gets a trusted one from Tailscale with one button — trusted everywhere, renewed by itself, and the easy way in from away from home. Settings → System.
- **Health warnings**: a phone notification when a camera is offline for more than 2 minutes (and when it's back), when a camera stops recording, and when the recordings drive is nearly full. On by default (Settings → Notifications).
- **Home, Away and Night**: a mode button at the top of every page, like an alarm system. Each mode chooses which cameras send phone notifications — e.g. none at Home, everything when Away. Night can come on by itself at night while you're Home. Recording and alerts carry on in every mode.
- **Keep alerts longer than recordings**: Settings → Recording & storage → Keep alerts for. An alert's clip, picture and the stretch of 24/7 recording it plays from are kept that long; when space runs out, ordinary footage goes first.
- **Install GAZE as an app**: on a secure address, GAZE can be installed on phones and computers (👤 menu → Install GAZE as an app, or Add to Home Screen on iPhone); it opens full screen like an app and shows a friendly message when GAZE can't be reached.
- **Home Assistant (MQTT)**: Settings → Smart home. Every camera appears in Home Assistant by itself: motion and each object as sensors, online status, the last alert picture, an alert event, and a Home / Away / Night switch.
- **Webhooks**: each new alert can be sent as JSON to IFTTT, Node-RED, Zapier, a smart plug or your own script — limited to some cameras and kinds of alert, with the picture if wanted.
- **The camera's own detection**: camera → AI → "Use what the camera itself detects". Person, vehicle, line-crossing, tampering and doorbell-press events from the camera (ONVIF) become alerts in GAZE; a doorbell press always notifies.
- **Snooze**: alert notifications have "Snooze 1 hour" and "Snooze until tomorrow" buttons, and the camera view has a 🔕 menu — no notifications from that camera for a while; it still records and keeps alerts.

## 0.6.51 — November 2026
- **Share alerts**: a **⤴ Share** button on every alert sends it to someone, ready to read on its own: the alert picture with its boxes and a caption (what, which camera, when), and a short message with what was seen.
  - **Share picture… / Share video…** opens your phone's or computer's own share menu — Messenger, WhatsApp, Facebook, Instagram, email, Teams or any other app you have — with the picture or the alert's video attached. (Browsers only offer this on a secure https address, or when GAZE is opened on the computer it runs on.)
  - **Email, WhatsApp, Messenger, Facebook, Telegram, Text message**: one click saves the picture and opens the app with the message filled in (or copied, ready to paste), so you just attach the picture.
  - **Copy picture** and **Copy message** to paste anywhere; **Copy GAZE link** opens the alert for others in your home who have a GAZE account.
  - **Save picture** and **Save video** (the alert's clip, or its part of the 24/7 recording), named after the camera and time.

## 0.6.50 — November 2026
- **ⓘ info cards on settings**: settings that aren't obvious have a small ⓘ next to them — hover over it (or tap it on a phone) for what the setting does and an example. Long explanations under settings moved into these cards, so the pages are shorter and easier to scan.
- **Plainer names**: technical labels are now everyday words — e.g. Sensitivity (was Min. contrast), Smallest movement, Must move for (was Make time), How sure the AI must be (was Minimum confidence), AI looks every, Connection type (was RTSP transport), Control port and login (was ONVIF), Smoothness in the grid / when opened (was frame rate), Video encoder, and "Sub-stream" everywhere instead of "stream 2" or "Substream".
- **One place for package delivery**: the old "Package delivery" tick box on zones is gone — the painted package spot (camera → AI) does this. Zones that had it ticked, and package boxes drawn by older versions, are turned into the painted spot automatically, so nothing stops working.
- **Fixes**: saving GPU acceleration settings no longer says cameras are restarting when nothing changed; the AI setting no longer shows the internal name "builtin"; removed outdated text (smart search, "the AI always runs on the CPU"); the zone loitering setting reads "Alert if something stays longer than … seconds".

## 0.6.49 — November 2026
Security fixes:
- **Other web pages on the same computer can't make changes any more**: GAZE checked that a change came from its own page by the address only, not the port, so a web page served by another program on the same computer (or the same router address) could make changes while you were signed in. The port is now checked too.
- **Accounts limited to some cameras** can no longer get the time-lapse details of other cameras, or name faces from other cameras' alerts.
- **Space use** only lists the cameras an account may see, and the recording folders are only shown to admins.
- **Video conversion is limited** to a few at a time, so one browser (or a page stuck asking again and again) can't tie up the computer.

Fixes:
- **Time-lapse scrubbing works for viewer and operator accounts** (it was refused for anyone but an admin).

## 0.6.48 — November 2026
- **Full-screen timeline uses the whole width**: the camera name column is gone, so the time line runs edge to edge.
- **Live cameras go straight to the slide-up timeline**: full screen on a Live grid camera (double-click or ⛶) now opens it in full screen with the same timeline as Recordings — move the mouse to the bottom of the screen and it slides up. The ⏪ Timeline button is gone. Leaving full screen goes back to the grid.
- **One look for both**: the live full-screen timeline has the same height, zoom and ◀ ▶ buttons in its toolbar as the Recordings one.

## 0.6.47 — November 2026
- **One toolbar in full screen, on every video**: the old controls across the bottom of the picture are gone. Play/pause, −1/+1 frame, speed, clip start and end, ⇩ File and Back to live are all in the toolbar under the full-screen timeline.
- **Timeline in live full screen**: a camera's live view in full screen has its timeline in the slide-up panel — drag back to watch what happened, then Back to live. A full-screen tile on the Live grid has a **⏪ Timeline** button that opens it.
- **Only that camera's timeline**: with one camera in full screen on Recordings, the panel shows just its row.
- **Clip start / Clip end everywhere**: the same names on every video, and the orange mark shows as soon as you set either one (it no longer waits for the second).
- **More room for the picture on Recordings**: the title, camera choice, dates and Refresh moved to a column on the left, and a single camera's picture is sized so the toolbar and timeline fit on the screen without scrolling.
- **Alerts: bigger pictures and a Columns choice** (Auto, or 2 to 10 side by side).
- **What these mean**: a key on the Alerts page explains every badge and symbol on an alert — NEW, the object and how sure the AI is, motion only, the green box, HD AI ✓, HD AI ?, play, still recording, download and delete.
- **Alert filters only show what your cameras look for**: no Dog or Cat chip unless a camera (or its profile) watches for them; Item left/taken, Faces and Number plates appear only when they're turned on. Turn one on and its chip appears.
- **Join alerts** (camera → Motion & AI, default 30 seconds): if a camera triggers again within this time, it's added to the same alert — one longer video instead of lots of short ones. The timer starts again with each trigger; an alert is at most 10 minutes.
- **PTZ panel**: tucked evenly into the corner, more see-through, and without the "drag on the picture" text (messages still show briefly, and errors stay).

## 0.6.46 — November 2026
- **Time-lapse settings only change new footage**: changing how often pictures are kept, or their size, no longer remakes the time-lapse that's already there. To remake it at the new settings, use the new **Remake time-lapse with these settings** button (it warns first: footage older than a few hours is read again from the recordings, which takes a while).
- **Time-lapse can be turned off** (Settings → Recording & storage, on by default — one picture every 10 seconds, standard size). Off, nothing is kept or made, and dragging along a timeline moves the video itself.
- **Alerts open on the last 24 hours** instead of today; Today is still in the date picker.
- **Quick setup**: the channel and port boxes are gone — the stream addresses say those.
- **New look for the PTZ controls**: one frosted panel with a round joystick pad, a zoom pill and the presets.

## 0.6.45 — November 2026
- **The whole timeline in full screen**: a Recordings camera in full screen now has the real Recordings timeline — every camera, alert pictures, the time-lapse line, scrubbing, zoom and ◀ ▶ — in a panel that slides up when the mouse comes to the bottom of the screen and away when it leaves, with the play, frame, alert, speed and clip controls under it.
- **Passwords hidden in stream addresses**: in a camera's settings, the password inside the main, sub-stream and speaker addresses shows as dots; an 👁 button next to each shows it. Editing the rest of the address keeps the password. The camera and ONVIF login passwords have the 👁 too.
- **Network scan finds cameras on other networks**: the scan now searches every network this computer is on (it only found the main one), sends the ONVIF search out of every network adapter, also searches the networks of the cameras already set up, and has a box for other networks to search (e.g. 192.168.5.0/24 behind another router).

## 0.6.44 — November 2026
- **Time-lapse quality settings** (Settings → Recording & storage): choose how often a picture is kept (every 2 s for the smoothest scrubbing, up to every 60 s) and how sharp they are — Standard (256 × 144), Sharp (384 × 216) or Sharpest (512 × 288). Since the pictures come from what motion detection already looks at, more and sharper pictures cost little but disk space (shown as you choose). Changes apply straight away, without restarting the cameras.
- The old "Prepare time-lapse previews automatically" option is gone: it's always done now (it's cheap).
- **Alerts open faster**: the alert list no longer reads the details of every recording file on every camera (or of each alert's files) each time it loads, and folder listings are reused for a few seconds — a big difference with weeks of recordings, on Windows especially.

## 0.6.43 — November 2026
- **Time-lapse made as it's recorded**: motion detection already looks at every camera's stream 2 several times a second; it now keeps one small picture every time-lapse interval, and a recording's time-lapse is put together from those — the recording isn't read and decoded a second time. Much less work for the computer, and time-lapse for new footage is ready straight away. (Footage from before this version, or from a camera without motion detection, is still done the old way.)

## 0.6.42 — November 2026
- **Tidy-up**: removed code for features that are no longer there (old notification AI settings, removing PTZ presets from the server, styles for the removed description feature) and the conversions for settings files from very old versions (0.1 to 0.5.7).
- **New README**: shorter and up to date — what GAZE does, how to install it, a recommended first setup, and how it's put together.

## 0.6.41 — November 2026
- **Controls on the picture in full screen**: a Recordings camera in full screen now has the same on-screen controls as the alert player — a scrub bar (with the time-lapse while dragging), play, ±1 frame, previous / next alert, speed, clip start / end with download, and sound. A live camera in full screen has sound, push to talk (two-way audio cameras), snapshot, and clip start / end with download (from the recording). They fade while the mouse is still.
- **Alerts**: the "HD" tag on every alert is gone; an alert whose AI find was confirmed in full quality shows **HD AI ✓** next to the camera's name (or **HD AI ?** when it wasn't).
- **Downloads are named after the camera and time**: clips, files and snapshots download as e.g. "Front Door 2026-11-12 09-34-36.mp4".

## 0.6.40 — November 2026
- **No more stale pages after an update**: the browser could keep running GAZE's old page code after an update (talking to the new server), so things misbehaved until the browser's cache was cleared. The page now always loads the version that's installed.
- **◀ ▶ on the Recordings timeline while playing**: pressing back kept snapping back to where it was playing; each press now moves further back (or forward) as before.
- **Less load with several GAZE windows open**: a window or tab you're not looking at stops loading Recordings and time-lapse pictures in the background, and carries on when you go back to it.
- **HD clips that play converted** (H.265 the browser can't play) no longer switch the camera's other footage to converted playback too — the sub-stream keeps streaming directly (converting is slow and heavy on the server).

## 0.6.39 — November 2026
- **Recordings stays loaded near "now"**: watching recent footage, each camera's stream now keeps growing with the recording instead of being loaded again every so often — which emptied the picture (black) while it reloaded, and is why coming back to Recordings showed black boxes. The picture and what's loaded now stay, and newer footage is added as it's recorded.
- **Packages must stay a minute**: when the package spot (or an area watching for items) looks different after activity, GAZE waits a minute and looks again. Only a change that's still there — and hasn't moved — counts as a parcel left or taken; something set down for a moment, a shadow or a passing reflection doesn't.

## 0.6.38 — November 2026
- **New server load graph** (System): CPU, GPU and memory over time as smooth, filled lines, with a faint band for the peaks. Cards above it show each one now, its average and its peak over the range; hover anywhere for that moment's readings, click a card to hide or show its line.
- **The time ranges match the data**: each range (1 h, 6 h, 24 h, 7 days, 30 days) now spans its full length with round time marks. After GAZE restarts, the 1 h and 6 h views use the saved per-minute history for the time before (they showed nothing but zero there). Times without readings (GAZE not running, computer off) are shown hatched instead of as 0 %.

## 0.6.37 — November 2026
- **AI finds are checked in HD**: the AI keeps watching stream 2 (light on the computer), and when it finds something new, GAZE takes one full-quality picture from the main stream and has the AI look closely at that spot. It's either confirmed (**HD✓** on the alert), corrected (say a cat, not a dog), or not confirmed. Settings → camera → AI → **Check in full quality (HD)**: *mark* what HD doesn't confirm (shown as **HD?**, and no notification for it), *leave it out* of the alert (an alert with nothing confirmed is dropped), or *off*. Notifications wait a few seconds for the check. Needs a separate stream 2.

## 0.6.36 — November 2026
- **Paint the package spot**: instead of dragging a box, colour in exactly where parcels are left (Settings → camera → AI), with the same paint / erase brush as the ignore areas. When an alert ends, only the painted part is compared with how it looked before, and the alert's before-and-after pictures show just that part (everything else blacked out). A box drawn before becomes painted the first time you open the camera's settings.

## 0.6.35 — November 2026
- **Recordings loads in the background**: open GAZE (or refresh the page) on any page and the Recordings timeline loads behind the scenes — footage, each camera's stream and the time-lapse pictures — so it's ready the moment you go there.
- **Time-lapse when reviewing an alert**: dragging along an alert's (or any recording's) bar now shows the time-lapse pictures already made for the timeline, and the HD video loads where you let go. Hovering over the bar shows them too. (Before, it looked for the alert clip's own time-lapse, which usually isn't made.)

## 0.6.34 — November 2026
- **Main stream (HD) time-lapse everywhere**: dragging through Recordings with Main stream chosen now shows the time-lapse at every moment with footage — not only where there's HD footage. (It stopped where there was none once a camera's HD footage played converted.)
- **Main stream is purple**: on the timelines, main-stream (HD) recordings are purple and sub-stream recordings green, whether 24/7 or alert clips.
- **Recordings stays loaded**: going to another page and back no longer reloads every camera — the players, pictures and time-lapse carry on where they were (only newer footage is fetched), with no black boxes while they pick up.
- **Space by kind of footage** (System): how much room the sub-stream recordings, main-stream recordings, alerts and time-lapse previews take, in total and per camera.

## 0.6.33 — November 2026
- **Smooth, continuous playback in Recordings**: each camera's recordings now play as one continuous stream instead of 5-minute files one after another. GAZE indexes the small pieces every recording is already made of, so the player fetches only the part it needs, buffers ahead, and goes from one file to the next with no reload and no black blink. Jumping to any moment shows video in a fraction of a second. Nothing is converted or re-encoded. (HD alert clips, and browsers that can't stream it, still play the files.)
- **Camera view playback opens instantly too**: opening a recording at a moment, or jumping within it, fetches just that part.
- **No more waiting for a recording to be "prepared"**: the first time a recording was opened it used to be copied into another format first (up to 20 seconds). That's gone; recordings an earlier version copied are turned back, once, in the background (System → Tasks: "Preparing recordings for smooth playback").
- **Time-lapse previews are kept for good**: a fault since 0.6.1 meant previews weren't recognised as made, so they were made again after a refresh (and in the background, over and over). They're now kept and come back straight away after a refresh. Previews made before this version are made once more, then kept.
- **Main stream (HD) time-lapse**: with Main stream chosen you can drag through the whole timeline with the time-lapse, even where only the sub-stream is recorded; letting go there says there's no HD footage at that time.

## 0.6.32 — November 2026
- **Time-lapse scrubbing on Main stream (HD)**: dragging through Recordings with Main stream chosen now shows the time-lapse pictures, as it does on Sub-stream. The HD alert clips have no time-lapse of their own, so their moments show the 24/7 recording's pictures (the blue line).

## 0.6.31 — November 2026
- **Ignore areas count for the AI too**: something the AI finds mostly inside the areas you painted to ignore (say a car parked behind the trees) no longer makes an alert — even when something else set off the check — and isn't boxed on the alert's HD picture either. A new tick box under the ignore areas, **"AI ignores these areas too"** (on by default), turns this off if you still want AI alerts there.
- **Solid timeline bars**: recordings that follow each other are one continuous green bar (no gaps or rounded ends every 5 minutes), and the blue time-lapse line is solid too.
- **Playhead time out of the way**: in Recordings the playhead's time now sits just under the time scale, clear of the red NOW mark.

## 0.6.30 — November 2026
- **Faster switch from time-lapse to video**: when you let go after dragging through Recordings, the cameras' videos now load first. Dragging asked for the time-lapse of every file you passed over, and those requests were holding up the videos; they're dropped and the time-lapse carries on loading afterwards. A player that's switched away from also stops downloading its old file.
- **Sub-stream / Main stream (HD) shows only that stream**: choose Main stream and the timeline shows only full-quality footage (the 24/7 HD recording, or the purple HD alert clips if you record the sub-stream 24/7); where there's none, nothing plays. A stream you haven't recorded shows as "not recorded" and can't be picked. Each file's stream is told by its picture size, so footage from before a change of recording mode is still shown correctly.
- **Click Recordings again to reload it**: with Recordings open, clicking the Recordings tab loads it afresh — newest footage, every camera's video reloaded — keeping the cameras you chose.
- **New update animation**: the spinning arrow is replaced by a download arrow dropping into a tray, with a ring that fills as the update downloads.

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
