# Tooling inventory

Only tools that give a measurable benefit to GAZE. Reassess every few months
(delete what isn't used). Third-party code is treated as untrusted: read it,
prefer tools that run locally and offline, never send source, settings,
camera addresses or recordings to an outside service without the owner's say.

## Installed / used in the audit (local, not part of the repository)

| Tool | Version | Source | Purpose | Permissions / network |
|---|---|---|---|---|
| `cargo clippy` | 0.1.97 | Rust toolchain | Lints (29 style warnings, no correctness lints) | local |
| `cargo audit` | 0.23.0 | RustSec (`cargo install cargo-audit --locked`) | Known vulnerabilities / unmaintained crates in `Cargo.lock` | downloads the advisory database from github.com/RustSec; sends nothing |
| `rustup target add x86_64-pc-windows-gnu` + `gcc-mingw-w64-x86-64` | — | rustup / Ubuntu | **Compile-check the Windows-only code from Linux** (`CARGO_TARGET_DIR=/tmp/wintarget cargo check --target x86_64-pc-windows-gnu`) — catches service/tray/FFI errors before a release | local |
| Playwright (Chromium) | preinstalled | — | Screenshots of both themes, page checks with stubbed APIs | local |
| `curl`, `ps` | — | system | Running-server baseline (page weight, memory) | local |
| `tools/audit/config_usage.py` | — | this repository | Dead-setting scan | local |
| `tools/audit/css_duplicates.py` | — | this repository | CSS drift scan | local |
| `tools/bench/` | — | this repository (1.0 audit) | Simulated cameras, CPU/memory/process counts (needs MediaMTX, Node, Playwright) | local |

## Page smoke test (`tools/e2e/smoke.js`)

Starts the built program in a scratch folder, signs in, opens every page and
every Settings section in the day and night theme and fails on a script
error, a console error, an unexpected failed request or an empty page
(50 page views, about 40 s). Needs Node + Playwright and `cargo build`:

```bash
node tools/e2e/smoke.js            # SHOTS=/some/folder keeps a screenshot of each view
```

It reads `SECTIONS` from `static/app.js`, so a new settings page is covered
automatically. It does **not** check how pages look or what buttons do, and
it runs with no cameras (see the expected-failure list in the script). Not in
CI yet.

## In CI (`.github/workflows/`)

| Workflow | Does |
|---|---|
| `release.yml` | Builds `GAZE.exe` and `GAZE-Watchdog.exe` on Windows and `GAZE-linux-x86_64` on Linux (separate job), publishes the release |
| `linux.yml` | On changes: tests, page smoke test, Docker image build + start, release-mode Linux build (publishes nothing) |
| `security.yml` | `cargo audit` on dependency changes and weekly |
| `quality.yml` | On pull requests / by hand: tests, clippy correctness lints, **Windows compile check** |

## Considered, not installed

| Tool | Why not now |
|---|---|
| Claude Code skills / MCP servers from the directory | None relevant to Rust/ffmpeg/NVR exist in the directory searched; the one security connector found is a hosted service that would receive source code |
| `cargo-deny` | Useful for licence/duplicate-crate policy; add when a policy is decided |
| `cargo-machete` / `cargo-udeps` | Checked unused dependencies by script instead (all 35 used); `udeps` needs nightly |
| `cargo-bloat`, `cargo flamegraph`, `heaptrack` | Add when a specific CPU/memory question needs profiling on real cameras |
| `cargo fmt --check` | 850 differences: not enforced (see `TECH-DEBT.md` X-2) |
| Bundle analysers | There is no bundler; the page is one served file (221 KB gzipped) |

## Rule

Before adding a tool, workflow, dependency or setting: look for an existing
one (see `CLAUDE.md`). Add it to this file with its purpose and source, or
don't add it.
