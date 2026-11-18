# Security checklist

Run through this when a change touches routes, accounts, files, processes or
the page. Findings and history: `AUDIT-2026-11.md` (section E).

## Every new or changed route

- [ ] Which roles may call it? Add it to `allowed()` in `web.rs` (admin-only
      is the default for anything that isn't a read).
- [ ] Does it take a **camera id** (path, query, body, or inside a case/alert)?
      The central `camera_in_path()` covers path shapes only. The handler must
      also call `visible(&user)` — and a test in `camera_ids_are_found_in_every_camera_path`
      style must name the shape. A restricted account must never read, change
      or delete anything of a camera it can't see (evidence cases and alerts
      included).
- [ ] File names from the client go through `valid_component` /
      `storage::resolve` (never `dir.join(name)` on raw input).
- [ ] No shell strings: processes get an argument list.
- [ ] Anything shown back to the page goes through `esc()` in the page, or
      `textContent`; never raw into `innerHTML`.
- [ ] Responses don't contain passwords/tokens (`mask_url`, `has_password`).

## Anything that reaches ffmpeg, go2rtc or a URL

- [ ] Stream addresses: `config::stream_url_ok` (streaming schemes only).
- [ ] Addresses from discovery or from another device are *untrusted input*.
- [ ] Outbound requests to user-set URLs (webhooks, ntfy, remote GAZE) are
      admin-set; don't add one an operator can set.

## Accounts and sessions

- [ ] Passwords hashed with Argon2 only; sessions are random tokens; cookies
      stay `HttpOnly; SameSite=Lax` (+ `Secure` on https).
- [ ] State-changing requests must pass the origin check (including WebSocket
      upgrades). Don't add routes before the `auth` layer.
- [ ] Open (no sign-in) paths are listed in `check_access`: keep that list
      minimal; each needs a reason (page assets, sign-in API, signed links).

## Build and supply chain

- [ ] `cargo audit` clean (the Security workflow, weekly and on dependency
      changes).
- [ ] New dependency? First check that nothing existing does the job
      (`CLAUDE.md`), then its maintenance status.
- [ ] Downloaded programs/models: pin a SHA-256 (as `clip.rs` does).
- [ ] No secrets in the repository, logs or error messages.

## Open items

See the S-entries in `TECH-DEBT.md`.

## Added in 1.2.2

- [ ] A program or library GAZE downloads and then runs or loads (go2rtc, Piper,
      DirectML, the OpenVINO/CUDA runtimes) is checked against a SHA-256 pinned
      in the code **before** it is unpacked (`jobs::verify_sha256`; a failing file
      is deleted). Pins are for a fixed version; a floating "latest" URL can't be
      pinned (ffmpeg: checked against the list BtbN publishes next to it, see TECH-DEBT X-1).
- [ ] The client address used for the sign-in lock-out comes from `client_ip`: the
      connection's own address, or, from a proxy on this computer, the **last**
      `X-Forwarded-For` address. IPv6 is counted per /64 (`users::ip_bucket`).

## Day summary

- [ ] `/api/cameras/{id}/synopsis*` are under `camera_in_path` (restricted accounts can't reach another camera's); the output is `nvr-data/synopsis/<camera id>/<day>.mp4` with the camera id checked by `valid_folder` and the day parsed as a date (no user text in paths). Operators may start one (CPU only); nothing is deleted except summaries older than 30 days.

## Floor plan and following people

- [ ] GAZE's server fetches no map imagery: the *browser* loads tiles from the chosen provider (the address you type in the search box goes to Nominatim; custom tile URLs must be http(s) and are used only by the page).

- [ ] It is **off by default** and says what it keeps: positions and times of
      walks (never pictures) for `keep_days`; *Delete all walks* removes them.
- [ ] Restricted accounts: `/api/plan` returns only their cameras and the links
      between them (`restrict_plan`), `/api/plan/journeys` only walks on their
      cameras, `/api/plan/live` only their cameras. Writing (plan, picture, delete
      walks) is admin only (non-GET falls to `allowed()`).
- [ ] The picture upload accepts only a JPEG, at most 12 MB and 8000 px on a side,
      checked by decoding its header, and is stored under a fixed name (never a
      name from the request). The page re-encodes pictures, which drops hidden
      data such as location.
- [ ] No map imagery is fetched by GAZE (no outbound request to a map service).

