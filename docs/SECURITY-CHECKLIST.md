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
