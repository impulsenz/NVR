# Downloads and updates

GAZE is built by GitHub and published as a **release**. Users download the
ready-made program from the release page, and GAZE then keeps itself up to
date from the same place. Nobody needs Rust or a compiler.

This guide covers:

1. [For users: download and update](#1-for-users-download-and-update)
2. [Publishing a new version (impulsenz/NVR)](#2-publishing-a-new-version)
3. [Hosting your own copy on your own GitHub](#3-hosting-your-own-copy)
4. [How it works and what's checked](#4-how-it-works)
5. [Troubleshooting](#5-troubleshooting)

---

## 1. For users: download and update

1. Open the latest release: <https://github.com/impulsenz/NVR/releases/latest>
2. Download `nvr-windows-x64.exe` (rename it to `nvr.exe` if you like).
3. Put it in a folder of its own, e.g. `C:\GAZE`. Settings, logs and AI
   models are kept next to it; recordings go in a `recordings` folder there
   unless you choose another.
4. Install ffmpeg if you don't have it. On Windows, open Command Prompt and
   run `winget install ffmpeg`.
5. Start the program and open <http://localhost:8080>. The first visit asks
   you to create the admin account.

**Updating:** GAZE checks for a new version **once a day**. When there
is one, a bar slides down under the header for admins, showing the change
of version (e.g. `0.5.4 → 0.5.5`) and when it was released:
- **Update now** installs it straight away. The bar shows the download's
  progress; GAZE then restarts and the page reloads by itself. If it doesn't
  work, the bar says why and offers **Try again**.
- **Later** hides the bar until the next version.

The release notes are in Settings → About.

**⟳ Updates**, next to your name in the header, checks right away; a green
✓ on the button means you're up to date. The same
controls are in **Settings → About → Updates**. **Install updates
automatically** (off by default) installs new versions as soon as they're
found, when no alert is going on. This all works when GAZE runs as a
Windows service too.

> Moving from a version you built yourself (0.4 or older)? Those don't have
> the updater yet. Download the new program once as above and use it instead
> of `target\release\nvr.exe`: copy it into the folder where your
> `nvr.toml` is, or start it from that folder, so it keeps your settings.

---

## 2. Publishing a new version

GitHub builds and publishes everything **when the version number goes up**.
The workflow that does it is `.github/workflows/release.yml`.

1. Raise the version in `Cargo.toml`, e.g. `version = "0.6.0"`.
   Installed copies only update to a **higher** version.
2. Add a section to the top of `CHANGELOG.md` starting with that number:
   ```
   ## 0.6.0 — December 2026
   - What changed…
   ```
   This section becomes the release notes shown in GAZE before updating.
3. Commit with the title **`Version 0.6.0`** (exactly "Version" and the
   new number) and push to the repository's default branch:
   ```bash
   git commit -am "Version 0.6.0"
   git push
   ```
   That's all. There's no tag to make: GitHub sees the new version, builds
   it, and creates the tag `v0.6.0` and the release itself. The run is
   listed in **Actions** as "Version 0.6.0". Pushing a
   version tag by hand (`git tag v0.6.0`, `git push origin v0.6.0`) works
   too.
4. On GitHub, open **Actions** → *Release* to follow the build. It takes
   about 10–15 minutes. When it's done, **Releases** has *GAZE v0.6.0* with
   these files:
   - `nvr-windows-x64.exe` and `nvr-windows-x64.exe.sha256`
5. Installed copies see the new version within a minute.

Other pushes don't start a release (a change to `Cargo.toml` in a commit
with another title builds nothing).

To test a build without publishing it, start the *Release* workflow by hand
(**Actions → Release → Run workflow**). It builds the program and attaches
it to the run as a download, without making a release.

To take a bad release back, delete it (or mark it as a pre-release) on
GitHub. Copies that already installed it stay on it until a newer version
is published.

---

## 3. Hosting your own copy

You can build and publish GAZE from **your own** GitHub account, for example
with your own changes. Your installations then update from your releases
instead of the original ones.

1. **Fork** the repository on GitHub (the *Fork* button on
   <https://github.com/impulsenz/NVR>), or push the code to a new repository
   of yours.
2. In your repository, open **Settings → Actions → General**:
   - Under *Actions permissions*, allow actions.
   - Under *Workflow permissions*, choose **Read and write permissions**, so
     the workflow can create releases.
3. Make the repository **public**. GAZE downloads updates without signing
   in, which only works for public repositories.
4. Publish a version exactly as in [section 2](#2-publishing-a-new-version),
   in your repository.

Programs built by **your** repository's workflow automatically take their
updates from your repository: the workflow builds in its name, and Settings →
About → Updates shows "Updates from: github.com/you/NVR".

**Switching an existing installation** to another repository (for example a
copy you built from the original): Settings → About → Updates → **Update
from**, enter `you/NVR` (or paste the GitHub address), then **Save**. Leave
it empty to go back to the repository the program was built from. The same
setting is in `nvr.toml`:

```toml
[update]
auto = true
repo = "you/NVR"
```

Keep your version numbers above the original's if you switch an existing
installation over, because GAZE only updates to higher versions.

---

## 4. How it works

**Checking:** once a day (soon after GAZE starts, then every 24 hours), and when you press **⟳ Updates**, GAZE opens
`github.com/<owner>/<repo>/releases/latest` and reads which version it leads
to. That's GitHub's normal web page, not its API, which allows only 60
requests an hour per internet connection when not signed in (and answers
"403 rate limit exceeded" after that). The program and its checksum are
downloaded from the release's fixed download addresses; the API is asked only
once per new version, for the release notes. If GitHub still says to slow
down, GAZE waits until the time GitHub gives before checking again.

**Updating:**
1. GAZE downloads the program for this system from the release.
2. It checks the download against the published SHA-256 checksum, and stops
   without changing anything if they differ.
3. It renames the running program to `nvr.old.exe`, which Windows allows
   while it runs, and puts the new one in its place.
4. It restarts as a new process. If GAZE runs as a Windows service, it
   restarts the service.
5. The old program is deleted at the next start.

**What's not touched:** settings (`nvr.toml`), the data folder
(`nvr-data`), recordings and accounts. Only the program file is replaced.

**Who can update:** only admin accounts can see or use the update controls.
Updates only come from the configured GitHub repository's releases, over
HTTPS.

---

## 5. Troubleshooting

**"403 rate limit exceeded" for api.github.com**
Versions before 0.5.7 asked GitHub's API every minute, and GitHub allows only
60 such requests an hour per internet connection (fewer if other programs at
home use it too). It sorts itself out within the hour. From 0.5.7 GAZE checks
GitHub's normal release page instead, which doesn't have that limit.

**"no releases yet"**
No version has been published in that repository. See
[section 2](#2-publishing-a-new-version).

**"no ready-made updates for this kind of computer"**
Releases are built for 64-bit Windows. On other systems, build GAZE
yourself; see the README.

**The workflow fails with "Resource not accessible by integration"**
The workflow isn't allowed to create releases. Allow *Read and write
permissions* in the repository's Settings → Actions → General.

**"could not move the running program aside"**
GAZE can't write to its own folder. This happens, for example, under
`C:\Program Files` without admin rights. Keep GAZE in a folder of its own
that it can write to, such as `C:\GAZE`, or run it as a Windows service.

**The update installed but GAZE didn't come back**
Start it again. If it runs as a Windows service, start the *GAZE NVR*
service in Windows *Services*, or use the tray icon → *Restart GAZE*.
`nvr.old.exe` next to it is the previous version: rename it back to
`nvr.exe` to undo the update.
