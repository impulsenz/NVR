# Technical debt register

Keep this current. **Add an entry when you leave something unresolved; delete
the entry when it is fixed.** Source of the first entries: `AUDIT-2026-11.md`
(IDs match). Rank: C critical · H high · M medium · L low.

| ID | Rank | Debt | Where | Next step | Blocked by |
|---|---|---|---|---|---|
| H-1 | M | Camera authorisation: the by-id check now lives in one function (`camera_denied`, unit-tested) and a test (`parameterised_routes_are_covered_or_reviewed`) fails for a new unreviewed route with a parameter. Still by hand: about 20 handlers that *filter lists* by `visible(&user)` (alerts, recordings, status …) and the 3 "no restricted account at all" checks | `web.rs` `camera_denied`, `visible` | A list-filter helper; a test that calls each camera route as a restricted account | — |
| H-4 | H | No CSP/HSTS/Permissions-Policy; inline `onclick` and heavy `innerHTML` | `web.rs` `harden`, `static/index.html`, `app.js` | Report-Only CSP; move inline handlers to listeners | approval (can break the page) |
| H-5 | M | 38 timers; five poll the server | `app.js` | One status channel over the existing WebSocket; JSON compression limited by content type (never streams) | — |
| M-1 | M | `app.css`: 84 selectors defined more than once (`tools/audit/css_duplicates.py`) | `app.css` | One block per component with tokens; screenshots of both themes | — |
| M-2 | M | `web.rs` 6,500 lines, `app.js` 9,800 lines | — | Split `web.rs` by feature; page modules | H-1 first |
| M-3 | L | `EventStore::between` scans all events. **Measured (100 k events, release): 1.6 ms per day query, `all()` 47 ms, open 270 ms** — fast enough; an SQLite index is not justified. Watch callers of `all()` on hot paths | `events.rs` | `cargo test --release bench_between -- --ignored --nocapture` after any change | — |
| L-1 | L | Page tests: only a smoke test exists (`tools/e2e/smoke.js`: every page and settings section, both themes, no errors; not in CI, runs with no cameras). No behaviour tests, no screenshot comparison | `static/`, `tools/e2e/` | Add it to `quality.yml`; screenshot baselines per look; tests with simulated cameras (`tools/bench`) | — |
| L-2 | L | chrono unmaintained notice (RUSTSEC-2026-0543) | everywhere | Watch; plan `jiff`/`time` migration | — |
| L-3 | L | `fullscreen_corners` is misnamed (only the first is used); `fullscreen_tiles` unused | `config.rs`, `app.js` | Migration + removal in a version that raises the minimum upgrade | approval (changes saved files) |
| L-4 | L | Legacy migration fields `ZoneConfig.watch_items`, `DetectConfig.package_area` | `config.rs` | Remove with L-3 | L-3 |
| L-5 | L | Two NAS concepts (Backup = copy, Deep storage = move) with similar pages | `static/app.js` | Clearer names/intro text; maybe merge pages | — |
| A-1 | M | CUDA, Intel graphics and NPU paths are untested on real hardware (OpenVINO on a CPU is tested: `NVR_TEST_MODELS=… GAZE_TEST_ACCEL=openvino cargo test onnx_runtime_matches_tract`). The CUDA wheel is not unpacked-size checked; accelerator choice can't change without a restart | `infer.rs` | Run the test with `GAZE_TEST_ACCEL=cuda` / `GAZE_TEST_OV_DEVICE=GPU` on such machines | hardware |
| ST-1 | L | Learned states: `states.json` and its pictures aren't in Export / import setup; the Home Assistant sensor for a state made after MQTT connected appears at the next reconnect; no "from an alert picture" way to add examples | `states.rs`, `mqtt.rs`, `backup` | Add to export; republish discovery when states change | — |
| ST-2 | M | Learned states were checked with the real picture model on synthetic scenes only (door open/shut under 0.7–1.35× light: 12/12 right, worst margin 0.058); never with a real camera, night/infrared, or look-alike states. Thresholds `MIN_SCORE`/`MIN_MARGIN` are untuned guesses beyond that | `states.rs` | Try on real cameras; `NVR_TEST_MODELS=… cargo test real_model_tells -- --nocapture` | real cameras |
| SM-1 | M | Drive health: parser tested on fixtures in smartctl's documented JSON and on its real "cannot read" output, but never on a physical drive; drives aren't matched to the recordings folders (all drives are shown); warnings repeat once per GAZE restart; needs admin/root for smartctl | `smart.rs` | Try on a real machine; map recording folders to drives; remember what was told | real hardware |
| LX-1 | L | Linux: unit never run under its own `gaze` user; no self-update on Linux (by design: systemd/Docker users update with their tools); only x86-64 (no ARM build); the Linux release job has run only as a dry run on a branch until the next release | `packaging/`, `release.yml` | ARM build; verify the unit on a real server | real server |
| S8 | L | Sign-in lock-out is per user name and per address (IPv6 by /64, last proxy address). A guesser can still lock a real user out for up to 15 min by failing for their name (the usual trade-off) | `users.rs` | If it matters: an exception for addresses that recently signed in as that user | — |
| X-1 | M | Reviewed (see CHANGELOG 1.2.2): `onvif_device.rs`, `remote.rs`, snooze signature, power-cut repair. **Still open:** ffmpeg for Windows comes from a floating `latest` URL (BtbN), so it is only checked against the checksum list published next to it (catches damaged or swapped files, not a replaced build with its list). AI model files (YOLOX, faces, plates, Piper voices: data, not programs) are downloaded unpinned | `getffmpeg.rs`, `ai.rs`, `recog.rs`, `piper.rs` | Move ffmpeg to a frozen build with a pinned checksum once one can be tested on Windows hardware; pin the models | Windows test |
| X-2 | L | 850 rustfmt differences | all | Not enforced; a mass reformat would bury blame | — |

## Looks (skins.css)

* The ten looks were checked on Live, Alerts, Settings (day and night, 1440 px) and the account menu; not yet every dialog, the Recordings timeline, System graphs and Evidence page in every look. Pages with hard-coded colours (tags, timeline segments, `.tag.person`) keep them in every look.
* `skins.css` needs CSS nesting (Chrome/Edge 120+, Safari 17.2+, Firefox 117+); on older browsers a look keeps its colours but loses its layout.
* Stored but unreviewed: the Windows-only code is untouched by this change.
