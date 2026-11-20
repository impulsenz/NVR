//! Evidence Locker: cases that keep important footage safe.
//!
//! A **case** ("Break-in 12 March", "Parcel theft") holds clips and pictures:
//! an alert (its picture and video), a stretch of recording, or a single
//! picture from a recording. Each one is **copied into the case**
//! (`nvr-data/cases/<case>/`), so it outlives the recordings it came from.
//!
//! An item is **protected** when added. While it is, the footage it came
//! from (the alert, and the recording files covering its time) can't be
//! deleted anywhere else in GAZE — not from Recordings or Alerts, not by
//! "Delete footage", and not by the automatic clean-up (retention, storage
//! limits). Deleting it there says which case holds it; it is unprotected
//! in the case first. A protected item can't be removed from its case, and
//! a case with protected items can't be deleted.

use crate::sync::MutexExt;
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Case {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub notes: String,
    pub created: DateTime<Local>,
    #[serde(default)]
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// An alert: its picture, and its video when there is one.
    Alert,
    /// A stretch of recording.
    Clip,
    /// One picture.
    Image,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Item {
    pub id: String,
    pub kind: Kind,
    pub title: String,
    pub camera: String,
    #[serde(default)]
    pub camera_name: String,
    /// The alert it came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    /// The time it covers.
    pub from: DateTime<Local>,
    pub to: DateTime<Local>,
    /// Files in the case's folder: the video and/or the picture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picture: Option<String>,
    #[serde(default)]
    pub notes: String,
    pub added: DateTime<Local>,
    #[serde(default)]
    pub added_by: String,
    /// Locks the footage it came from against deletion.
    pub protected: bool,
}

struct Locker {
    dir: PathBuf,
    cases: Vec<Case>,
}

static LOCKER: LazyLock<Mutex<Option<Locker>>> = LazyLock::new(|| Mutex::new(None));

/// Load the cases (at start-up). `data_dir` is GAZE's data folder.
pub fn init(data_dir: &Path) {
    let dir = data_dir.join("cases");
    let file = dir.join("cases.json");
    let cases = match std::fs::read_to_string(&file) {
        Err(_) => Vec::new(),
        Ok(t) => match serde_json::from_str::<Vec<Case>>(&t) {
            Ok(c) => c,
            Err(e) => {
                // Kept aside (never overwritten), so nothing is lost.
                let aside = dir.join(format!("cases.json.unreadable-{}", Local::now().format("%Y%m%d-%H%M%S")));
                let _ = std::fs::rename(&file, &aside);
                tracing::error!("Evidence Locker: cases.json can't be read ({e}); it was kept as {}", aside.display());
                Vec::new()
            }
        },
    };
    if !cases.is_empty() {
        let n: usize = cases.iter().map(|c| c.items.iter().filter(|i| i.protected).count()).sum();
        tracing::info!("Evidence Locker: {} case(s), {n} protected item(s)", cases.len());
    }
    *LOCKER.locked() = Some(Locker { dir, cases });
}

fn with<R>(f: impl FnOnce(&mut Locker) -> Result<R>) -> Result<R> {
    let mut g = LOCKER.locked();
    let l = g.as_mut().context("the Evidence Locker isn't ready")?;
    f(l)
}

/// Write the list safely (a new file, then renamed over the old one).
fn save(l: &Locker) -> Result<()> {
    std::fs::create_dir_all(&l.dir)?;
    let tmp = l.dir.join("cases.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&l.cases)?)?;
    std::fs::rename(&tmp, l.dir.join("cases.json"))?;
    Ok(())
}

pub fn list() -> Vec<Case> {
    LOCKER.locked().as_ref().map(|l| l.cases.clone()).unwrap_or_default()
}

pub fn get(id: &str) -> Option<Case> {
    list().into_iter().find(|c| c.id == id)
}

/// The folder a case's files are kept in.
pub fn case_dir(id: &str) -> Option<PathBuf> {
    let g = LOCKER.locked();
    let l = g.as_ref()?;
    l.cases.iter().any(|c| c.id == id).then(|| l.dir.join(id))
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}{}", Local::now().format("%Y%m%d%H%M%S%3f"))
}

pub fn create(name: &str, notes: &str) -> Result<Case> {
    let name = name.trim();
    if name.is_empty() {
        bail!("give the case a name");
    }
    with(|l| {
        let c = Case { id: new_id("case-"), name: name.to_string(), notes: notes.trim().to_string(), created: Local::now(), items: Vec::new() };
        std::fs::create_dir_all(l.dir.join(&c.id))?;
        l.cases.push(c.clone());
        save(l)?;
        Ok(c)
    })
}

pub fn rename(id: &str, name: &str, notes: &str) -> Result<Case> {
    let name = name.trim();
    if name.is_empty() {
        bail!("give the case a name");
    }
    with(|l| {
        let c = l.cases.iter_mut().find(|c| c.id == id).context("no such case")?;
        c.name = name.to_string();
        c.notes = notes.trim().to_string();
        let out = c.clone();
        save(l)?;
        Ok(out)
    })
}

/// Delete a case and its files: only once nothing in it is protected.
pub fn delete(id: &str) -> Result<()> {
    with(|l| {
        let i = l.cases.iter().position(|c| c.id == id).context("no such case")?;
        let n = l.cases[i].items.iter().filter(|x| x.protected).count();
        if n > 0 {
            bail!("“{}” has {n} protected item(s): unprotect them first", l.cases[i].name);
        }
        let dir = l.dir.join(id);
        l.cases.remove(i);
        save(l)?;
        let _ = std::fs::remove_dir_all(dir);
        Ok(())
    })
}

/// Add an item whose files are already in the case's folder.
pub fn add(case: &str, mut item: Item) -> Result<Item> {
    with(|l| {
        let c = l.cases.iter_mut().find(|c| c.id == case).context("no such case")?;
        item.id = new_id("item-");
        item.added = Local::now();
        c.items.push(item.clone());
        save(l)?;
        Ok(item)
    })
}

/// Protect or unprotect an item; change its title or notes.
pub fn update_item(case: &str, item: &str, protected: Option<bool>, title: Option<&str>, notes: Option<&str>) -> Result<Item> {
    with(|l| {
        let c = l.cases.iter_mut().find(|c| c.id == case).context("no such case")?;
        let it = c.items.iter_mut().find(|i| i.id == item).context("no such item in this case")?;
        if let Some(p) = protected {
            it.protected = p;
        }
        if let Some(t) = title.map(str::trim).filter(|t| !t.is_empty()) {
            it.title = t.to_string();
        }
        if let Some(n) = notes {
            it.notes = n.trim().to_string();
        }
        let out = it.clone();
        save(l)?;
        Ok(out)
    })
}

/// Remember the thumbnail made for an item that had none.
pub fn set_picture(case: &str, item: &str, name: &str) -> Result<()> {
    with(|l| {
        let c = l.cases.iter_mut().find(|c| c.id == case).context("no such case")?;
        let it = c.items.iter_mut().find(|i| i.id == item).context("no such item in this case")?;
        if it.picture.is_none() {
            it.picture = Some(name.to_string());
            save(l)?;
        }
        Ok(())
    })
}

/// Remove an item (and its copied files) from a case: only when unprotected.
pub fn remove_item(case: &str, item: &str) -> Result<()> {
    with(|l| {
        let dir = l.dir.join(case);
        let c = l.cases.iter_mut().find(|c| c.id == case).context("no such case")?;
        let i = c.items.iter().position(|x| x.id == item).context("no such item in this case")?;
        if c.items[i].protected {
            bail!("“{}” is protected: unprotect it first", c.items[i].title);
        }
        let it = c.items.remove(i);
        save(l)?;
        for f in [it.video, it.picture].into_iter().flatten() {
            let _ = std::fs::remove_file(dir.join(f));
        }
        Ok(())
    })
}

/// The case (by name) whose protected items lock this alert.
pub fn locks_event(camera: &str, event: &str) -> Option<String> {
    let g = LOCKER.locked();
    let l = g.as_ref()?;
    l.cases.iter().find(|c| c.items.iter().any(|i| i.protected && i.camera == camera && i.event.as_deref() == Some(event))).map(|c| c.name.clone())
}

/// The case whose protected items lock footage of this camera between these
/// times (a recording file, an alert clip).
pub fn locks_time(camera: &str, from: DateTime<Local>, to: DateTime<Local>) -> Option<String> {
    let g = LOCKER.locked();
    let l = g.as_ref()?;
    l.cases.iter().find(|c| c.items.iter().any(|i| i.protected && i.camera == camera && i.from <= to && i.to >= from)).map(|c| c.name.clone())
}

/// What a blocked delete says.
pub fn locked_message(case: &str) -> String {
    format!("This is locked in the Evidence Locker case “{case}”. To delete it, open that case (Evidence) and unprotect it first.")
}

pub fn safe_part(s: &str) -> String {
    let t: String = s.chars().map(|c| if c.is_alphanumeric() || " -_.".contains(c) { c } else { '_' }).collect();
    let t = t.trim().trim_matches('.').to_string();
    let t: String = t.chars().take(80).collect();
    if t.is_empty() { "item".into() } else { t }
}

/// What a backup of the locker holds: every case, and each item's files that
/// exist, as (name in the backup, path). Also the files' total size.
pub fn backup_files() -> (Vec<Case>, Vec<(String, PathBuf)>, u64) {
    let g = LOCKER.locked();
    let Some(l) = g.as_ref() else { return (Vec::new(), Vec::new(), 0) };
    let (mut files, mut bytes) = (Vec::new(), 0u64);
    for c in &l.cases {
        for f in c.items.iter().flat_map(|i| [i.video.as_ref(), i.picture.as_ref()]).flatten() {
            let path = l.dir.join(&c.id).join(f);
            if let Ok(md) = std::fs::metadata(&path)
                && md.is_file()
            {
                bytes += md.len();
                files.push((format!("locker/{}/{}", c.id, f), path));
            }
        }
    }
    (l.cases.clone(), files, bytes)
}

/// (cases, items, bytes) the locker holds, for Settings → Backup.
pub fn stats() -> (usize, usize, u64) {
    let (cases, _, bytes) = backup_files();
    (cases.len(), cases.iter().map(|c| c.items.len()).sum(), bytes)
}

/// Add a case from a backup. A case that is already here is left exactly as
/// it is (evidence is never overwritten): `None`. Otherwise the folder for
/// its files, which the caller fills.
pub fn import_case(c: Case) -> Result<Option<PathBuf>> {
    if c.id != safe_part(&c.id) || c.id.is_empty() {
        bail!("a case in the backup has an unusable id");
    }
    with(|l| {
        if l.cases.iter().any(|x| x.id == c.id) {
            return Ok(None);
        }
        let dir = l.dir.join(&c.id);
        std::fs::create_dir_all(&dir)?;
        l.cases.push(c);
        save(l)?;
        Ok(Some(dir))
    })
}

/// One file of an item, as the review page lists it.
#[derive(serde::Serialize)]
struct ViewFile {
    /// From the top of the ZIP.
    path: String,
    #[serde(rename = "type")]
    kind: &'static str,
    sha256: String,
    size: u64,
    /// The video's format as recorded ("h264", "hevc"…), when known.
    #[serde(skip_serializing_if = "String::is_empty")]
    codec: String,
    /// A copy any web browser can play (H.264), made at export when the original's format can't be: the page plays this one.
    #[serde(skip_serializing_if = "Option::is_none")]
    web: Option<String>,
}

#[derive(serde::Serialize)]
struct ViewItem<'a> {
    #[serde(flatten)]
    item: &'a Item,
    /// The times as recorded, "2026-11-20 14:03:22" (the page filters on these as text: no time zone surprises).
    from_text: String,
    to_text: String,
    added_text: String,
    files: Vec<ViewFile>,
}

#[derive(serde::Serialize)]
struct ViewCase<'a> {
    name: &'a str,
    notes: &'a str,
    items: Vec<ViewItem<'a>>,
}

/// The review page (`case_viewer.html`) with the cases' data in it. Nothing is loaded from anywhere: it works from a folder on a disk.
fn viewer_html(cases: &[ViewCase]) -> Result<String> {
    let data = serde_json::json!({
        "exported": Local::now().format("%d %b %Y %H:%M").to_string(),
        "tz": format!("UTC{}", Local::now().format("%:z")),
        "cases": cases,
    });
    // (No "</script>" can end the data early.)
    let json = serde_json::to_string(&data)?.replace('<', "\\u003c");
    Ok(include_str!("case_viewer.html").replacen("/*DATA*/", &json, 1))
}

/// What the export prepared beforehand (see `web::cases_zip`): each video's format and the browser-playable
/// copy made of it, both by (case id, file name).
#[derive(Default)]
pub struct Prepared {
    pub codecs: std::collections::HashMap<(String, String), String>,
    pub web: std::collections::HashMap<(String, String), PathBuf>,
}

/// Everything in `cases` as one ZIP, for handing to somebody else.
///
/// ```text
/// index.html            open it: filter by day, time, camera and type; play each video with its notes
/// <Case>/               (only with several cases; with one, its files are at the top)
///   case-notes.txt      the notes about the case as a whole
///   manifest.txt        SHA-256 of every file
///   01 <item title>/
///     <video>.mp4  <picture>.jpg
///     notes.txt         the notes for this video, with when / where
/// ```
/// `allow` says which cameras the person exporting may see. With a
/// `password`, every file in the ZIP (the videos, the pictures and the review
/// page) is encrypted with AES-256: nothing in it can be read without the
/// password, and a program that supports AES ZIPs (7-Zip, WinRAR, Keka) is
/// needed to open it. Once extracted, the files are of course plain.
pub fn export_zip(cases: &[Case], out: &Path, allow: &dyn Fn(&str) -> bool, password: Option<&str>, prep: &Prepared) -> Result<()> {
    use sha2::Digest;
    use std::io::{Read, Write};
    use zip::write::SimpleFileOptions;
    let password = password.filter(|p| !p.is_empty());
    let l_dir = with(|l| Ok(l.dir.clone()))?;
    let mut z = zip::ZipWriter::new(std::io::BufWriter::new(std::fs::File::create(out)?));
    let lock = |o: SimpleFileOptions| match password {
        Some(p) => o.with_aes_encryption(zip::AesMode::Aes256, p),
        None => o,
    };
    let text = lock(SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).large_file(true));
    let stored = lock(SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored).large_file(true));
    let many = cases.len() > 1;
    let mut used = std::collections::HashSet::new();
    let mut view: Vec<ViewCase> = Vec::new();
    let when = |t: DateTime<Local>| t.format("%Y-%m-%d %H:%M:%S").to_string();
    for c in cases {
        let mut root = safe_part(&c.name);
        while !used.insert(root.clone()) {
            root = format!("{root}-2");
        }
        let prefix = if many { format!("{root}/") } else { String::new() };
        let items: Vec<&Item> = c.items.iter().filter(|i| allow(&i.camera)).collect();
        let mut manifest = String::new();
        let mut vitems = Vec::new();
        z.start_file(format!("{prefix}case-notes.txt"), text)?;
        write!(z, "{}\r\n\r\nCreated: {}\r\n\r\nNotes\r\n-----\r\n{}\r\n", c.name, c.created.format("%d %b %Y %H:%M:%S"), c.notes)?;
        for (n, i) in items.iter().enumerate() {
            let rel_folder = format!("{:02} {}", n + 1, safe_part(&i.title));
            let folder = format!("{prefix}{rel_folder}");
            z.start_file(format!("{folder}/notes.txt"), text)?;
            write!(
                z,
                "{}\r\n\r\nCamera: {}\r\nFrom: {}\r\nTo: {}\r\nType: {:?}\r\nAdded: {}{}\r\nProtected: {}\r\n\r\nNotes\r\n-----\r\n{}\r\n",
                i.title,
                if i.camera_name.is_empty() { &i.camera } else { &i.camera_name },
                i.from.format("%d %b %Y %H:%M:%S"),
                i.to.format("%d %b %Y %H:%M:%S"),
                i.kind,
                i.added.format("%d %b %Y %H:%M:%S"),
                if i.added_by.is_empty() { String::new() } else { format!(" by {}", i.added_by) },
                if i.protected { "yes" } else { "no" },
                i.notes
            )?;
            let mut files = Vec::new();
            for (file, kind) in i.video.iter().map(|f| (f, "video")).chain(i.picture.iter().map(|f| (f, "picture"))) {
                if file.contains('/') || file.contains('\\') || file.contains("..") {
                    continue;
                }
                let src = l_dir.join(&c.id).join(file);
                let Ok(mut f) = std::fs::File::open(&src) else { continue };
                z.start_file(format!("{folder}/{file}"), stored)?;
                let mut h = sha2::Sha256::new();
                let mut size = 0u64;
                let mut buf = vec![0u8; 1 << 20];
                loop {
                    let k = f.read(&mut buf)?;
                    if k == 0 {
                        break;
                    }
                    size += k as u64;
                    h.update(&buf[..k]);
                    z.write_all(&buf[..k])?;
                }
                let sum = h.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>();
                manifest.push_str(&format!("{sum}  {rel_folder}/{file}\r\n"));
                // The browser-playable copy, beside the original (which stays as it was recorded).
                let mut web = None;
                if kind == "video"
                    && let Some(wp) = prep.web.get(&(c.id.clone(), file.clone()))
                    && let Ok(mut wf) = std::fs::File::open(wp)
                {
                    let wname = format!("{}.web.mp4", file.trim_end_matches(".mp4"));
                    z.start_file(format!("{folder}/{wname}"), stored)?;
                    let mut wh = sha2::Sha256::new();
                    loop {
                        let k = wf.read(&mut buf)?;
                        if k == 0 {
                            break;
                        }
                        wh.update(&buf[..k]);
                        z.write_all(&buf[..k])?;
                    }
                    manifest.push_str(&format!("{}  {rel_folder}/{wname}  (copy for web browsers)\r\n", wh.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>()));
                    web = Some(format!("{folder}/{wname}"));
                }
                files.push(ViewFile {
                    path: format!("{folder}/{file}"),
                    kind,
                    sha256: sum,
                    size,
                    codec: prep.codecs.get(&(c.id.clone(), file.clone())).cloned().unwrap_or_default(),
                    web,
                });
            }
            // (The page's camera name is what the exporter sees, never an empty one.)
            vitems.push(ViewItem { item: i, from_text: when(i.from), to_text: when(i.to), added_text: when(i.added), files });
        }
        z.start_file(format!("{prefix}manifest.txt"), text)?;
        z.write_all(manifest.as_bytes())?;
        view.push(ViewCase { name: &c.name, notes: &c.notes, items: vitems });
    }
    z.start_file("index.html", text)?;
    z.write_all(viewer_html(&view)?.as_bytes())?;
    z.finish()?.flush()?;
    Ok(())
}

/// A safe file name from a title.
pub fn file_name(title: &str, ext: &str) -> String {
    let base: String = title.chars().map(|c| if c.is_alphanumeric() || " -_".contains(c) { c } else { '_' }).collect();
    let base = base.trim();
    format!("{}-{}.{ext}", if base.is_empty() { "item" } else { base }, Local::now().format("%H%M%S%3f"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    // One locker for the whole program: these steps run as one test.
    #[test]
    fn protection_blocks_deletes_until_unprotected() {
        let dir = std::env::temp_dir().join(format!("gaze-cases-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        init(&dir);
        let c = create("Break-in", "back door").unwrap();
        assert!(create("  ", "").is_err());
        let t = Local::now();
        let it = add(
            &c.id,
            Item {
                id: String::new(),
                kind: Kind::Alert,
                title: "Person at the back door".into(),
                camera: "back".into(),
                camera_name: "Back".into(),
                event: Some("e1".into()),
                from: t - Duration::seconds(10),
                to: t,
                video: None,
                picture: None,
                notes: String::new(),
                added: t,
                added_by: "admin".into(),
                protected: true,
            },
        )
        .unwrap();
        // The alert and the footage of its time are locked; other footage isn't.
        assert_eq!(locks_event("back", "e1").as_deref(), Some("Break-in"));
        assert!(locks_event("front", "e1").is_none());
        assert!(locks_time("back", t - Duration::seconds(5), t + Duration::seconds(60)).is_some());
        assert!(locks_time("back", t + Duration::seconds(1), t + Duration::seconds(60)).is_none());
        // Protected: can't be removed, nor the case deleted.
        assert!(remove_item(&c.id, &it.id).is_err());
        assert!(delete(&c.id).is_err());
        // Kept across a restart.
        init(&dir);
        assert_eq!(get(&c.id).unwrap().items.len(), 1);
        // Export: a folder per item with its video and notes, and a manifest.
        {
            let cd = case_dir(&c.id).unwrap();
            std::fs::write(cd.join("v.mp4"), b"video").unwrap();
            update_item(&c.id, &it.id, None, None, Some("saw a man")).unwrap();
            with(|l| {
                l.cases[0].items[0].video = Some("v.mp4".into());
                Ok(())
            })
            .unwrap();
            let out = dir.join("x.zip");
            export_zip(&list(), &out, &|_| true, None, &Prepared::default()).unwrap();
            let mut z = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
            let names: Vec<String> = (0..z.len()).map(|i| z.by_index(i).unwrap().name().unwrap().to_string()).collect();
            assert!(names.iter().any(|n| n == "index.html"), "{names:?}");
            assert!(names.iter().any(|n| n.starts_with("01 ") && n.ends_with("/notes.txt")), "{names:?}");
            assert!(names.iter().any(|n| n.ends_with("/v.mp4")), "{names:?}");
            // The review page carries every item's times, files and hash.
            let mut z = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
            let mut html = String::new();
            std::io::Read::read_to_string(&mut z.by_name("index.html").unwrap(), &mut html).unwrap();
            assert!(
                html.contains("\"from_text\":") && html.contains("v.mp4") && html.contains("saw a man") && !html.contains("/*DATA*/"),
                "{}",
                &html[html.len().saturating_sub(600)..]
            );
            // With a password nothing opens without it, and everything does with it.
            let sealed = dir.join("sealed.zip");
            export_zip(&list(), &sealed, &|_| true, Some("hunter2-pass"), &Prepared::default()).unwrap();
            let mut z = zip::ZipArchive::new(std::fs::File::open(&sealed).unwrap()).unwrap();
            assert!(z.by_name("index.html").is_err(), "a locked file opened without the password");
            assert!(z.by_name_decrypt("index.html", b"wrong-password").is_err());
            let mut got = Vec::new();
            std::io::Read::read_to_end(&mut z.by_name_decrypt("index.html", b"hunter2-pass").unwrap(), &mut got).unwrap();
            assert!(String::from_utf8_lossy(&got).contains("saw a man"));
            let bytes = std::fs::read(&sealed).unwrap();
            assert!(!bytes.windows(5).any(|w| w == b"video"), "the footage is readable in the locked file");
            // Someone who can't see the camera gets none of its items.
            export_zip(&list(), &out, &|_| false, None, &Prepared::default()).unwrap();
            let z = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
            assert!(z.file_names().all(|n| !n.unwrap().contains("v.mp4")));
        }
        // Backup: every case and each item's existing files; a case that is here
        // already is never replaced, a new one gets a folder, a bad id is refused.
        {
            let (cases, files, bytes) = backup_files();
            assert_eq!(cases.len(), 1);
            assert_eq!(files.len(), 1);
            assert_eq!(files[0].0, format!("locker/{}/v.mp4", c.id));
            assert_eq!(bytes, 5);
            assert_eq!(stats(), (1, 1, 5));
            assert!(import_case(cases[0].clone()).unwrap().is_none());
            let mut other = Case { id: "case-restored".into(), items: Vec::new(), ..cases[0].clone() };
            let d = import_case(other.clone()).unwrap().expect("a new case is added");
            assert!(d.is_dir());
            assert_eq!(list().len(), 2);
            other.id = "../evil".into();
            assert!(import_case(other).is_err());
            delete("case-restored").unwrap();
        }
        // Unprotected: the footage is free again, and the item can go.
        update_item(&c.id, &it.id, Some(false), None, None).unwrap();
        assert!(locks_event("back", "e1").is_none());
        remove_item(&c.id, &it.id).unwrap();
        delete(&c.id).unwrap();
        assert!(list().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
