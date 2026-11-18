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
    l.cases
        .iter()
        .find(|c| c.items.iter().any(|i| i.protected && i.camera == camera && i.from <= to && i.to >= from))
        .map(|c| c.name.clone())
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

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Everything in `cases` as one ZIP, for handing to somebody else.
///
/// ```text
/// <Case>/
///   index.html          open it: every video with its notes beside it
///   case-notes.txt      the notes about the case as a whole
///   manifest.txt        SHA-256 of every file
///   01 <item title>/
///     <video>.mp4  <picture>.jpg
///     notes.txt         the notes for this video, with when / where
/// ```
/// With several cases, each is a folder of its own. `allow` says which
/// cameras the person exporting may see.
pub fn export_zip(cases: &[Case], out: &Path, allow: &dyn Fn(&str) -> bool) -> Result<()> {
    use sha2::Digest;
    use std::io::{Read, Write};
    use zip::write::SimpleFileOptions;
    let l_dir = with(|l| Ok(l.dir.clone()))?;
    let mut z = zip::ZipWriter::new(std::io::BufWriter::new(std::fs::File::create(out)?));
    let text = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).large_file(true);
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored).large_file(true);
    let many = cases.len() > 1;
    let mut used = std::collections::HashSet::new();
    for c in cases {
        let mut root = safe_part(&c.name);
        while !used.insert(root.clone()) {
            root = format!("{root}-2");
        }
        let prefix = if many { format!("{root}/") } else { String::new() };
        let items: Vec<&Item> = c.items.iter().filter(|i| allow(&i.camera)).collect();
        let mut manifest = String::new();
        let mut html = format!(
            "<!doctype html><meta charset=utf-8><title>{n}</title><style>body{{font:15px system-ui,sans-serif;max-width:960px;margin:24px auto;padding:0 16px;color:#222}}video,img{{max-width:100%;border-radius:6px;background:#000}}.i{{border-top:1px solid #ccc;padding:16px 0}}pre{{white-space:pre-wrap;font:inherit;background:#f4f4f4;padding:10px;border-radius:6px}}.m{{color:#666;font-size:13px}}</style><h1>{n}</h1><p class=m>Exported {now} · {k} item(s)</p><h2>Notes</h2><pre>{cn}</pre>",
            n = esc(&c.name),
            now = Local::now().format("%d %b %Y %H:%M"),
            k = items.len(),
            cn = if c.notes.trim().is_empty() { "(none)".into() } else { esc(&c.notes) },
        );
        z.start_file(format!("{prefix}case-notes.txt"), text)?;
        write!(z, "{}\r\n\r\nCreated: {}\r\n\r\nNotes\r\n-----\r\n{}\r\n", c.name, c.created.format("%d %b %Y %H:%M:%S"), c.notes)?;
        for (n, i) in items.iter().enumerate() {
            let folder = format!("{prefix}{:02} {}", n + 1, safe_part(&i.title));
            let rel_folder = format!("{:02} {}", n + 1, safe_part(&i.title));
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
            html.push_str(&format!(
                "<div class=i><h2>{}</h2><p class=m>{} · {} – {}</p>",
                esc(&i.title),
                esc(if i.camera_name.is_empty() { &i.camera } else { &i.camera_name }),
                i.from.format("%d %b %Y %H:%M:%S"),
                i.to.format("%H:%M:%S")
            ));
            let enc_path = |f: &str| format!("{}/{}", rel_folder.replace(' ', "%20"), f.replace(' ', "%20"));
            for (file, is_video) in i.video.iter().map(|f| (f, true)).chain(i.picture.iter().map(|f| (f, false))) {
                if file.contains('/') || file.contains('\\') || file.contains("..") {
                    continue;
                }
                let src = l_dir.join(&c.id).join(file);
                let Ok(mut f) = std::fs::File::open(&src) else { continue };
                z.start_file(format!("{folder}/{file}"), stored)?;
                let mut h = sha2::Sha256::new();
                let mut buf = vec![0u8; 1 << 20];
                loop {
                    let k = f.read(&mut buf)?;
                    if k == 0 {
                        break;
                    }
                    h.update(&buf[..k]);
                    z.write_all(&buf[..k])?;
                }
                manifest.push_str(&format!("{}  {rel_folder}/{file}\r\n", h.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>()));
                if is_video {
                    html.push_str(&format!("<video controls preload=metadata src=\"{}\"></video>", enc_path(file)));
                } else if i.video.is_none() {
                    html.push_str(&format!("<img src=\"{}\" alt=\"\">", enc_path(file)));
                }
            }
            html.push_str(&format!("<h3>Notes for this video</h3><pre>{}</pre></div>", if i.notes.trim().is_empty() { "(none)".into() } else { esc(&i.notes) }));
        }
        z.start_file(format!("{prefix}index.html"), text)?;
        z.write_all(html.as_bytes())?;
        z.start_file(format!("{prefix}manifest.txt"), text)?;
        z.write_all(manifest.as_bytes())?;
    }
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
        let it = add(&c.id, Item {
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
        })
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
            export_zip(&list(), &out, &|_| true).unwrap();
            let mut z = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
            let names: Vec<String> = (0..z.len()).map(|i| z.by_index(i).unwrap().name().unwrap().to_string()).collect();
            assert!(names.iter().any(|n| n == "index.html"), "{names:?}");
            assert!(names.iter().any(|n| n.starts_with("01 ") && n.ends_with("/notes.txt")), "{names:?}");
            assert!(names.iter().any(|n| n.ends_with("/v.mp4")), "{names:?}");
            // Someone who can't see the camera gets none of its items.
            export_zip(&list(), &out, &|_| false).unwrap();
            let z = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
            assert!(z.file_names().all(|n| !n.unwrap().contains("v.mp4")));
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
