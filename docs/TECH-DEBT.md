# Technical debt register

Keep this current. **Add an entry when you leave something unresolved; delete
the entry when it is fixed.** Source of the first entries: `AUDIT-2026-11.md`
(IDs match). Rank: C critical · H high · M medium · L low.

| ID | Rank | Debt | Where | Next step | Blocked by |
|---|---|---|---|---|---|
| H-1 | H | Camera authorisation is a convention: middleware + 35 handler checks | `web.rs` `check_access`, `camera_in_path`, `visible` | One extractor `camera_for(user, id)`; a test that walks the route table and fails for uncovered camera routes | — |
| H-2 | H | 292 `lock().unwrap()`: a poisoned `std::sync::Mutex` cascades panics | all modules | `parking_lot::Mutex` or a recovering `lock()` helper | — |
| H-4 | H | No CSP/HSTS/Permissions-Policy; inline `onclick` and heavy `innerHTML` | `web.rs` `harden`, `static/index.html`, `app.js` | Report-Only CSP; move inline handlers to listeners | approval (can break the page) |
| H-5 | M | 38 timers; five poll the server | `app.js` | One status channel over the existing WebSocket; JSON compression limited by content type (never streams) | — |
| M-1 | M | `app.css`: 84 selectors defined more than once (`tools/audit/css_duplicates.py`) | `app.css` | One block per component with tokens; screenshots of both themes | — |
| M-2 | M | `web.rs` 6,500 lines, `app.js` 9,800 lines | — | Split `web.rs` by feature; page modules | H-1 first |
| M-3 | M | `EventStore::between` scans and clones all events per call | `events.rs` | Benchmark with 100 k events; time index if needed | — |
| L-1 | L | No page tests | `static/` | Component/e2e tests (Playwright is already used in `tools/bench`) | — |
| L-2 | L | chrono unmaintained notice (RUSTSEC-2026-0543) | everywhere | Watch; plan `jiff`/`time` migration | — |
| L-3 | L | `fullscreen_corners` is misnamed (only the first is used); `fullscreen_tiles` unused | `config.rs`, `app.js` | Migration + removal in a version that raises the minimum upgrade | approval (changes saved files) |
| L-4 | L | Legacy migration fields `ZoneConfig.watch_items`, `DetectConfig.package_area` | `config.rs` | Remove with L-3 | L-3 |
| L-5 | L | Two NAS concepts (Backup = copy, Deep storage = move) with similar pages | `static/app.js` | Clearer names/intro text; maybe merge pages | — |
| S5 | M | `GET /api/snooze`: confirm constant-time signature check | `notify::snooze_ok` | Read and test | — |
| S8 | M | Sign-in lockout: per user only? | `users.rs` | Check; add per-address limit | — |
| X-1 | M | Not audited: `onvif_device.rs` input handling, `remote.rs` trust model, checksum pinning of ffmpeg/go2rtc/voice downloads, power-cut test of recording repair | — | Targeted review | — |
| X-2 | L | 850 rustfmt differences | all | Not enforced; a mass reformat would bury blame | — |
