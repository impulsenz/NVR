//! Crash and power-loss protection for recordings.
//!
//! Recordings are fragmented MP4: a header, then self-contained pieces
//! (`moof` + `mdat`) every keyframe. If the NVR or the computer stops
//! suddenly, only the piece being written can be incomplete. This module
//! finds such a file and cuts it back to the last complete piece, so it
//! always plays (losing at most a second or two at the very end).

use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use tracing::{info, warn};

#[derive(Debug, PartialEq)]
pub enum Check {
    /// The file ends on a complete piece.
    Ok,
    /// Incomplete data after `good_len` bytes.
    Truncated { good_len: u64 },
    /// A header and no video at all (the power failed, or ffmpeg died, before
    /// the first piece was written): nothing to play and nothing to lose.
    Empty,
    /// Not an MP4 we can repair (or nothing usable).
    Unrecognized,
}

/// Walk the top-level boxes and find where the last complete piece ends.
pub fn check(path: &Path) -> std::io::Result<Check> {
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    let mut pos = 0u64;
    // End of the last box after which the file is valid (not a moof
    // waiting for its mdat).
    let mut good = 0u64;
    let mut seen_moov = false;
    // Video data (an `mdat`) is among what is kept.
    let mut media = false;
    let mut header = [0u8; 16];
    while pos < len {
        if len - pos < 8 {
            break;
        }
        f.seek(SeekFrom::Start(pos))?;
        f.read_exact(&mut header[..8])?;
        let size32 = u32::from_be_bytes(header[..4].try_into().unwrap()) as u64;
        let kind = [header[4], header[5], header[6], header[7]];
        let size = match size32 {
            0 => len - pos, // extends to end of file
            1 => {
                if len - pos < 16 {
                    break;
                }
                f.read_exact(&mut header[8..16])?;
                u64::from_be_bytes(header[8..16].try_into().unwrap())
            }
            n => n,
        };
        if size < 8 || !kind.iter().all(|c| c.is_ascii_alphanumeric() || *c == b' ') {
            break; // garbage
        }
        if pos + size > len {
            break; // box cut off
        }
        pos += size;
        match &kind {
            b"moov" => {
                seen_moov = true;
                good = pos;
            }
            // A fragment header is only useful together with its data.
            b"moof" => {}
            b"mdat" => {
                media = true;
                good = pos;
            }
            _ => good = pos,
        }
    }
    if !seen_moov {
        return Ok(Check::Unrecognized);
    }
    if !media {
        return Ok(Check::Empty);
    }
    Ok(if good == len {
        Check::Ok
    } else {
        Check::Truncated { good_len: good }
    })
}

/// Repair a file if needed. Returns true if it was changed.
pub fn repair(path: &Path) -> bool {
    match check(path) {
        Ok(Check::Truncated { good_len }) => {
            let len = path.metadata().map(|m| m.len()).unwrap_or(0);
            match OpenOptions::new().write(true).open(path).and_then(|f| {
                f.set_len(good_len)?;
                f.sync_all()
            }) {
                Ok(()) => {
                    info!(
                        "repaired {} (removed {} incomplete bytes at the end)",
                        path.display(),
                        len - good_len
                    );
                    true
                }
                Err(e) => {
                    warn!("could not repair {}: {e}", path.display());
                    false
                }
            }
        }
        // Nothing was ever recorded in it: it would only show as a recording
        // that can't be played. (Called only for files whose writer has
        // stopped: at start-up, and after a recorder's ffmpeg has ended.)
        Ok(Check::Empty) => match std::fs::remove_file(path) {
            Ok(()) => {
                info!("removed {} (it held no video, only the file header)", path.display());
                true
            }
            Err(e) => {
                warn!("could not remove empty {}: {e}", path.display());
                false
            }
        },
        _ => false,
    }
}

/// Files being written get this ending until they are complete.
pub const PART_SUFFIX: &str = ".part.mp4";

/// Make a finished file permanent: flush it to disk, then give it its real
/// name in one step (a rename is atomic, so the file is either complete or
/// not there at all).
pub fn commit(part: &Path, dest: &Path) -> std::io::Result<()> {
    if let Ok(f) = std::fs::File::open(part) {
        let _ = f.sync_all();
    }
    std::fs::rename(part, dest)?;
    // Also flush the folder entry (where the platform allows it).
    #[cfg(unix)]
    if let Some(dir) = dest.parent()
        && let Ok(d) = std::fs::File::open(dir)
    {
        let _ = d.sync_all();
    }
    Ok(())
}

/// Check and repair the recordings in `dir` changed in the last `hours`.
pub fn repair_dir(dir: &Path, hours: u64) -> usize {
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(hours * 3600);
    let mut fixed = 0;
    let mut dirs = vec![
        dir.to_path_buf(),
        dir.join("events"),
        dir.join(".buffer"),
        dir.join("timeline"),
    ];
    while let Some(d) = dirs.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        // Only the files being written when GAZE stopped can be cut short:
        // the newest few in each folder (one per recorder writing there).
        // Checking every recent file read each one through, which on a
        // network drive (NAS) could hold up starting for minutes.
        let mut recent: Vec<(std::time::SystemTime, std::path::PathBuf)> = Vec::new();
        for e in rd.flatten() {
            let p = e.path();
            // Leftovers of files that were being written when it stopped.
            if p.to_string_lossy().ends_with(PART_SUFFIX) {
                info!("removing incomplete file {}", p.display());
                let _ = std::fs::remove_file(&p);
                continue;
            }
            if p.extension().is_none_or(|x| x != "mp4") {
                continue;
            }
            if let Ok(t) = e.metadata().and_then(|m| m.modified())
                && t >= cutoff
            {
                recent.push((t, p));
            }
        }
        recent.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, p) in recent.into_iter().take(4) {
            if repair(&p) {
                fixed += 1;
            }
        }
    }
    fixed
}

/// After a recorder process stopped unexpectedly: repair its own newest
/// files (only those starting with `prefix`, only in `dir`, so files other
/// recorders are still writing are never touched).
pub fn repair_recent(dir: &Path, prefix: &str, minutes: u64) -> usize {
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(minutes * 60);
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    rd.flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.starts_with(prefix)
                && name.ends_with(".mp4")
                && e.metadata()
                    .and_then(|m| m.modified())
                    .map(|t| t >= cutoff)
                    .unwrap_or(false)
        })
        .filter(|e| repair(&e.path()))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(kind: &[u8; 4], body: usize) -> Vec<u8> {
        let mut v = ((body + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend(std::iter::repeat_n(0u8, body));
        v
    }

    #[test]
    fn trims_incomplete_tail() {
        let dir = std::env::temp_dir().join(format!("nvr-mp4-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.mp4");
        let mut good = [
            bx(b"ftyp", 16),
            bx(b"moov", 100),
            bx(b"moof", 50),
            bx(b"mdat", 1000),
        ]
        .concat();
        let good_len = good.len() as u64;
        // A complete moof without its mdat, then a cut-off mdat.
        good.extend(bx(b"moof", 50));
        good.extend(&bx(b"mdat", 1000)[..300]);
        std::fs::write(&p, &good).unwrap();
        assert_eq!(check(&p).unwrap(), Check::Truncated { good_len });
        assert!(repair(&p));
        assert_eq!(std::fs::metadata(&p).unwrap().len(), good_len);
        assert_eq!(check(&p).unwrap(), Check::Ok);
        assert!(!repair(&p));
        // Not an MP4: left alone.
        std::fs::write(&p, b"hello world, not a video").unwrap();
        assert_eq!(check(&p).unwrap(), Check::Unrecognized);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_with_no_video_is_removed_but_one_with_any_video_is_kept() {
        let dir = std::env::temp_dir().join(format!("nvr-mp4-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let header = [bx(b"ftyp", 16), bx(b"moov", 100)].concat();
        // The header alone; the header and a piece's index with no data; the
        // header and junk: all hold no video.
        for (name, data) in [("a.mp4", header.clone()), ("b.mp4", [header.clone(), bx(b"moof", 50)].concat()), ("c.mp4", [header.clone(), vec![0u8; 40]].concat()),
                             ("d.mp4", [header.clone(), bx(b"moof", 50), bx(b"mdat", 1000)[..300].to_vec()].concat())] {
            let p = dir.join(name);
            std::fs::write(&p, data).unwrap();
            assert_eq!(check(&p).unwrap(), Check::Empty, "{name}");
            assert!(repair(&p), "{name}");
            assert!(!p.exists(), "{name} should have been removed");
        }
        // One whole piece of video: kept (and the cut-off piece after it trimmed).
        let p = dir.join("e.mp4");
        let kept = [header.clone(), bx(b"moof", 50), bx(b"mdat", 1000)].concat();
        std::fs::write(&p, [kept.clone(), bx(b"moof", 50), bx(b"mdat", 1000)[..10].to_vec()].concat()).unwrap();
        assert!(repair(&p));
        assert_eq!(std::fs::read(&p).unwrap(), kept);
        // A normal (not fragmented) MP4 with its data first and its index last: untouched.
        let q = dir.join("f.mp4");
        std::fs::write(&q, [bx(b"ftyp", 16), bx(b"mdat", 500), bx(b"moov", 100)].concat()).unwrap();
        assert_eq!(check(&q).unwrap(), Check::Ok);
        assert!(!repair(&q) && q.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A real power cut: a recording made by ffmpeg the way the recorder makes
    /// them (fragmented MP4, a piece every keyframe) is cut at many places
    /// across its length, as if the power failed while it was being written.
    /// After the repair every cut file must be a valid MP4 that decodes with
    /// no errors, still holds every complete piece before the cut, and the
    /// repair must be harmless to run again. Needs ffmpeg (skipped without).
    #[test]
    fn a_real_recording_cut_anywhere_is_repaired_to_a_clean_file() {
        use std::process::{Command, Stdio};
        let ff = |args: &[&str]| Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "error", "-y"]).args(args).stdin(Stdio::null()).output();
        if ff(&["-version"]).map(|o| !o.status.success()).unwrap_or(true) {
            eprintln!("skipped: ffmpeg isn't installed");
            return;
        }
        let dir = std::env::temp_dir().join(format!("nvr-powercut-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let whole = dir.join("whole.mp4");
        // 12 s of video, a keyframe every second (25 fps), so 12 pieces.
        let made = ff(&["-f", "lavfi", "-i", "testsrc2=size=320x180:rate=25", "-t", "12", "-c:v", "libx264", "-g", "25", "-keyint_min", "25", "-sc_threshold", "0", "-pix_fmt", "yuv420p",
            "-movflags", "+frag_keyframe+empty_moov+default_base_moof", whole.to_str().unwrap()]).unwrap();
        assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));
        assert_eq!(check(&whole).unwrap(), Check::Ok);
        let bytes = std::fs::read(&whole).unwrap();
        let frames = |p: &Path| -> Option<u64> {
            let o = Command::new("ffprobe").args(["-v", "error", "-count_frames", "-select_streams", "v:0", "-show_entries", "stream=nb_read_frames", "-of", "csv=p=0", p.to_str().unwrap()]).output().ok()?;
            String::from_utf8_lossy(&o.stdout).trim().parse().ok()
        };
        assert_eq!(frames(&whole), Some(300));
        // Where each complete piece ends (the top-level boxes: ftyp, moov, then moof+mdat pairs).
        let mut ends = Vec::new();
        let (mut pos, mut moofs) = (0usize, 0);
        while pos + 8 <= bytes.len() {
            let size = u32::from_be_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
            let kind = &bytes[pos + 4..pos + 8];
            pos += size;
            if kind == b"mdat" {
                ends.push(pos);
                moofs += 1;
            }
        }
        assert_eq!(moofs, 12, "expected one piece per second");
        // Cut at 60 places, from right after the header to inside the last piece.
        let header_end = {
            let mut p = 0usize;
            loop {
                let size = u32::from_be_bytes(bytes[p..p + 4].try_into().unwrap()) as usize;
                if &bytes[p + 4..p + 8] == b"moof" {
                    break p;
                }
                p += size;
            }
        };
        let (mut checked, mut empty) = (0, 0);
        for i in 0..60 {
            let cut = header_end + 1 + (bytes.len() - header_end - 1) * i / 59;
            let cut = cut.min(bytes.len());
            let f = dir.join(format!("cut{i}.mp4"));
            std::fs::write(&f, &bytes[..cut]).unwrap();
            let kept_pieces = ends.iter().filter(|e| **e <= cut).count();
            repair(&f);
            if kept_pieces == 0 {
                // Cut before the first whole piece: nothing was recorded yet, so
                // the file holds no video and is removed.
                assert!(!f.exists(), "cut at {cut}: a file with no video should have been removed");
                empty += 1;
                continue;
            }
            // Still a recording, and complete up to the last whole piece.
            assert_eq!(check(&f).unwrap(), Check::Ok, "cut at {cut} of {}", bytes.len());
            let len_after = std::fs::metadata(&f).unwrap().len() as usize;
            // (A file that wasn't cut at all is complete, with the index box ffmpeg
            // writes when it closes a file normally, and is left exactly as it is.)
            let expect_len = if cut == bytes.len() { cut } else { ends[kept_pieces - 1] };
            assert_eq!(len_after, expect_len, "cut at {cut}: kept the wrong amount");
            // Repairing again changes nothing.
            assert!(!repair(&f));
            // ffmpeg decodes all of it without a single error, with 25 frames per kept piece.
            let dec = ff(&["-i", f.to_str().unwrap(), "-f", "null", "-"]).unwrap();
            assert!(dec.status.success() && dec.stderr.is_empty(), "cut at {cut}: ffmpeg complained: {}", String::from_utf8_lossy(&dec.stderr));
            assert_eq!(frames(&f), Some(25 * kept_pieces as u64), "cut at {cut}: frames lost");
            checked += 1;
        }
        // The other way a power cut shows: the file was extended but the last data
        // never reached the disk, leaving a run of zero bytes at the end.
        for (i, zeros) in [1usize, 7, 8, 100, 4096, 65536].into_iter().enumerate() {
            for (j, piece) in [2usize, 6, 11].into_iter().enumerate() {
                let f = dir.join(format!("zero{i}-{j}.mp4"));
                let mut data = bytes[..ends[piece - 1]].to_vec();
                data.extend(std::iter::repeat_n(0u8, zeros));
                std::fs::write(&f, &data).unwrap();
                repair(&f);
                assert_eq!(check(&f).unwrap(), Check::Ok, "{zeros} zero bytes after piece {piece}");
                assert_eq!(std::fs::metadata(&f).unwrap().len() as usize, ends[piece - 1], "{zeros} zero bytes after piece {piece}: not cut back");
                let dec = ff(&["-i", f.to_str().unwrap(), "-f", "null", "-"]).unwrap();
                assert!(dec.status.success() && dec.stderr.is_empty(), "{zeros} zeros: {}", String::from_utf8_lossy(&dec.stderr));
                assert_eq!(frames(&f), Some(25 * piece as u64));
            }
        }
        // Most cuts fall after at least one whole piece (and keep every whole piece).
        assert!(checked >= 55 && checked + empty == 60, "{checked} decoded, {empty} header-only");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

