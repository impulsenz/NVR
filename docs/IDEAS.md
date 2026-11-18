# Ideas that fit GAZE

Ranked by value for the effort, with what each needs from a home computer
(reference: an Intel N100 mini PC, 8 cameras at 1080p/4 MP, no discrete GPU).
Everything builds on data GAZE already has or on the floor plan (`plan.rs`,
`tracks.rs`). "Cheap" = a few hundred lines on top of what exists. Nothing here
needs a cloud service.

## Built on the floor plan (walks and journeys)

| Idea | What it gives | Hardware / cost |
|---|---|---|
| **Follow this person** | Click a journey and the Recordings page plays each camera's footage in turn along the path, with a map beside it. Segments already carry exact times per camera, and HLS playback exists. | Cheap. No extra CPU. **Best next step.** |
| **Suggest neighbours and walking times from data** | After a few days GAZE sees "people leave Gate and appear on Front 3–5 s later" and offers to draw that link with its usual time. Removes the manual setup step. | Cheap: a histogram of gaps between walk ends and starts. |
| **People on the property now** (and who's home) | Count entries and exits at the door and gate cameras; send "people on property" to MQTT / Home Assistant as a sensor. | Cheap. |
| **Walked-area heat on the plan** | Where people actually walk, drawn on the map, far more accurate than the alert heatmap because every look counts, not one picture per alert. Reveals desire paths, blind spots and dead zones. | Cheap. |
| **Route and time rules** | Alert when someone comes in by the side gate and goes round to the back, or when a path nobody has walked in 30 days appears at 03:00. Rules over camera sequences, plus a baseline of usual routes. | Moderate. |
| **Dwell and loiter areas on the map** | Draw a region on the plan; count time spent in it ("two minutes at the front window"). Works across cameras. | Cheap. |
| **Vehicles and pets** | The same tracker with other labels: driveway speed in km/h, "where does the dog go". The setting already takes any label the AI knows. | Cheap (a setting). Speed needs calibration. |
| **Better identity across cameras** | Use a CLIP picture of each person (the model description search already downloads, ~50–100 ms on a CPU, once or twice per walk) instead of the crude clothing colour; and, when a face is recognised, name the whole journey ("Sam came in by the gate at 17:42"). | Moderate; fits an N100 at one look a second. |
| **Coverage check** | From the cones and walked paths, show which parts of the property no camera sees and where people walk through them. | Cheap. |
| **Replay a window as a movie** | Several people moving on the map over the last ten minutes. | Cheap (page only). |
| **Export** | A journey (map picture, times, cameras) into an Evidence Locker case; CSV of walks. | Cheap. |
| **Several floors** | One plan picture per floor, cameras assigned to a floor. | Moderate. |

## Not on the floor plan

| Idea | Notes |
|---|---|
| **Per-camera "quiet hours" learned from history** | Suggest night times when alerts are nearly always false, from the data already stored. Cheap. |
| **Auto-suggest ignore areas** | The heatmap already finds patches that alert constantly; offer to paint them. Cheap. |
| **Weekly report** | The daily digest's content over a week, with trends. Cheap. |
| **Camera health history** | Uptime per camera over weeks (offline minutes, reconnects), to find the flaky cable. Cheap. |
| **Optional local language model for alert text** | Only for 8 GB+ of graphics memory; off by default. Not for an N100. |

## What the plan picture can be

GAZE does not download maps. You bring the picture:

* a floor plan or sketch (a photo of a drawing works);
* an aerial or satellite view of your property. **Check the terms of wherever it
  comes from**: some map services do not allow screenshots to be saved,
  published or shared, and GAZE will not fetch imagery for you. County or
  council aerial maps, OpenStreetMap exports (credit "© OpenStreetMap
  contributors") and your own drone or phone pictures are usually simpler;
* a top-down scan from a phone's depth sensor.

The page converts any image to a JPEG before uploading, which also removes
hidden data in the file such as a location.

## What this costs a home computer

The tracker only runs on pictures the AI already looks at (when there is
motion, about once a second per camera). It adds a few microseconds to each
look and a few hundred bytes to each walk on disk (about 2 KB for a typical
walk). One day of a busy property is well under 1 MB. The most it can do to
recording is nothing: the detector hands over what it found, and a problem in
following people is dropped without touching recording or alerts.
