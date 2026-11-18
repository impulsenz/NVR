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
| L-1 | L | No page tests | `static/` | Component/e2e tests (Playwright is already used in `tools/bench`) | — |
| L-2 | L | chrono unmaintained notice (RUSTSEC-2026-0543) | everywhere | Watch; plan `jiff`/`time` migration | — |
| L-3 | L | `fullscreen_corners` is misnamed (only the first is used); `fullscreen_tiles` unused | `config.rs`, `app.js` | Migration + removal in a version that raises the minimum upgrade | approval (changes saved files) |
| L-4 | L | Legacy migration fields `ZoneConfig.watch_items`, `DetectConfig.package_area` | `config.rs` | Remove with L-3 | L-3 |
| L-5 | L | Two NAS concepts (Backup = copy, Deep storage = move) with similar pages | `static/app.js` | Clearer names/intro text; maybe merge pages | — |
| A-1 | M | CUDA, Intel graphics and NPU paths are untested on real hardware (OpenVINO on a CPU is tested: `NVR_TEST_MODELS=… GAZE_TEST_ACCEL=openvino cargo test onnx_runtime_matches_tract`). The CUDA wheel is not unpacked-size checked; accelerator choice can't change without a restart | `infer.rs` | Run the test with `GAZE_TEST_ACCEL=cuda` / `GAZE_TEST_OV_DEVICE=GPU` on such machines | hardware |
| ST-1 | L | Learned states: `states.json` and its pictures aren't in Export / import setup; the Home Assistant sensor for a state made after MQTT connected appears at the next reconnect; no "from an alert picture" way to add examples | `states.rs`, `mqtt.rs`, `backup` | Add to export; republish discovery when states change | — |
| ST-2 | M | Learned states were checked with the real picture model on synthetic scenes only (door open/shut under 0.7–1.35× light: 12/12 right, worst margin 0.058); never with a real camera, night/infrared, or look-alike states. Thresholds `MIN_SCORE`/`MIN_MARGIN` are untuned guesses beyond that | `states.rs` | Try on real cameras; `NVR_TEST_MODELS=… cargo test real_model_tells -- --nocapture` | real cameras |
| SM-1 | M | Drive health: parser tested on fixtures in smartctl's documented JSON and on its real "cannot read" output, but never on a physical drive; drives aren't matched to the recordings folders (all drives are shown); warnings repeat once per GAZE restart; needs admin/root for smartctl | `smart.rs` | Try on a real machine; map recording folders to drives; remember what was told | real hardware |
| LX-1 | M | Linux: no Linux binary in the releases (the release workflow builds Windows only); Dockerfile never built; unit never run under the `gaze` user; no CI check that Linux still builds and tests | `.github/workflows/`, `packaging/` | Add a Linux build+test job and attach `GAZE-linux-x86_64` to releases; build the image | CI access |
| S5 | M | `GET /api/snooze`: confirm constant-time signature check | `notify::snooze_ok` | Read and test | — |
| S8 | M | Sign-in lockout: per user only? | `users.rs` | Check; add per-address limit | — |
| X-1 | M | Not audited: `onvif_device.rs` input handling, `remote.rs` trust model, checksum pinning of ffmpeg/go2rtc/voice downloads, power-cut test of recording repair | — | Targeted review | — |
| X-2 | L | 850 rustfmt differences | all | Not enforced; a mass reformat would bury blame | — |

## Looks (skins.css)

* The ten looks were checked on Live, Alerts, Settings (day and night, 1440 px) and the account menu; not yet every dialog, the Recordings timeline, System graphs and Evidence page in every look. Pages with hard-coded colours (tags, timeline segments, `.tag.person`) keep them in every look.
* `skins.css` needs CSS nesting (Chrome/Edge 120+, Safari 17.2+, Firefox 117+); on older browsers a look keeps its colours but loses its layout.
* Stored but unreviewed: the Windows-only code is untouched by this change.
