# GAZE — rules for AI-assisted development sessions

Read this first. GAZE is a recording product: **recording reliability and the
user's saved data come before every feature.** Earlier sessions left drift
(duplicated styles, stale docs, overlapping features); these rules exist to
stop that. Background: `docs/AUDIT-2026-11.md`, `docs/ARCHITECTURE.md`,
`docs/TECH-DEBT.md`.

## Before you add anything

1. **Search for an existing implementation** (`grep` the Rust modules and
   `static/app.js`; read the module's `//!` header). Extend it; don't write a
   second one. The code is well documented: modules start with a description.
2. **Before a new dependency, setting, service, endpoint or abstraction**:
   prove the existing code can't do it. New setting → add it to
   `docs/CONFIG-INVENTORY.md`'s areas and give it a default that keeps old
   settings files working.
3. **Before removing or replacing** something: find its real use (dynamic
   references, settings files in the wild, migrations). Never silently drop a
   user's saved value — use `Config::migrate`.
4. **Update the documentation in the same change** when structure changes:
   `docs/ARCHITECTURE.md`, `CHANGELOG.md`, and `docs/TECH-DEBT.md` (add what
   you left undone; delete what you fixed).

## Never

* Make recording depend on AI, the page, deep storage, or the watchdog.
* Delete or overwrite footage without a verified copy; never touch footage
  held in an Evidence Locker case (`cases::locks_*`).
* Let a restricted account see or change another camera's data — see
  `docs/SECURITY-CHECKLIST.md`.
* Put raw text into `innerHTML` (use `esc()`), build shell strings, or accept
  stream addresses that aren't streaming schemes (`stream_url_ok`).
* Hard-code colours in CSS: use the theme tokens in `:root` and
  `:root[data-theme="light"]` (timelines: `--tl-*`). Check both themes.
* Add a second rule for a selector that already has one — edit the original
  (`python3 tools/audit/css_duplicates.py`).

## Build, test, check

```bash
cargo test                      # GAZE (130 tests) and the watchdog (11)
cargo clippy --all-targets      # currently 29 style warnings, no correctness lints
cargo audit                     # dependency advisories (0 vulnerabilities)
node --check static/app.js      # the page has no build step
python3 tools/audit/config_usage.py     # dead-setting scan
```

**Windows-only code** (service, tray, watchdog, updater) only compiles on
Windows, and the release workflow is the only place it was built. Check it
from Linux before pushing:

```bash
rustup target add x86_64-pc-windows-gnu && sudo apt-get install -y gcc-mingw-w64-x86-64
CARGO_TARGET_DIR=/tmp/wintarget cargo check --target x86_64-pc-windows-gnu
```

Look at pages in a browser when you change them (Playwright is available;
stub `/api/*` and serve `static/`). Test the **day and night** themes.

## Releasing

The default branch is **not** `main`: it is whatever GitHub says (`git remote
show origin`; currently `claude/beautiful-galileo-j4o73a`). The release workflow
builds when a push to it raises `version` in `Cargo.toml` and the head commit
is titled `Version x.y.z`. Each release: bump `Cargo.toml` **and** the `nvr`
entry in `Cargo.lock`, add a `CHANGELOG.md` section, run the tests, commit
`Version x.y.z`, push to the default branch (and to your session's working
branch if the session requires it). Releases also publish `GAZE-Watchdog.exe`.

## Style of work

Small, verifiable steps. Measure before optimising (`tools/bench`, `curl`).
Say what was tested and what wasn't. Prefer the simplest implementation that
meets the need.
