//! Recordings played as one continuous stream (HLS).
//!
//! Recordings are fragmented MP4 files of a few minutes each: a header,
//! then a small piece (`moof` + `mdat`) for every key frame. Played as files,
//! the browser has no index of those pieces: jumping into a file meant
//! downloading it up to that point, and every file change reloaded the
//! player (a black blink). Here each file is indexed (where each piece is
//! and how long it lasts, read from the box headers only) and a time range
//! of one camera is described as a single HLS playlist of byte ranges of
//! the files. The player fetches only the pieces it needs, buffers ahead,
//! and plays across files without a break. Nothing is converted or copied.

use chrono::{DateTime, Local};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// A piece of a file: bytes `off..off+len`, `dur` seconds long.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Piece {
    pub off: u64,
    pub len: u64,
    pub dur: f64,
}

#[derive(Debug, Default)]
pub struct FileIndex {
    /// Bytes of the header (`ftyp` + `moov`): the init segment.
    pub init_len: u64,
    /// Seconds before the first piece (from its decode time).
    pub start: f64,
    pub pieces: Vec<Piece>,
}

fn be32(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?))
}
fn be64(b: &[u8], i: usize) -> Option<u64> {
    Some(u64::from_be_bytes(b.get(i..i + 8)?.try_into().ok()?))
}

/// Child boxes of a box body: (type, body).
fn boxes(b: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
    let mut i = 0;
    std::iter::from_fn(move || {
        let size = be32(b, i)? as usize;
        let (hdr, size) = if size == 1 { (16, be64(b, i + 8)? as usize) } else { (8, size) };
        if size < hdr || i + size > b.len() {
            return None;
        }
        let item = (&b[i + 4..i + 8], &b[i + hdr..i + size]);
        i += size;
        Some(item)
    })
}

/// The video track of a `moov`: (track id, timescale, default sample duration).
fn video_track(moov: &[u8]) -> Option<(u32, u32, u32)> {
    let mut trex = HashMap::new();
    let mut found = None;
    for (t, body) in boxes(moov) {
        match t {
            b"trak" => {
                let (mut id, mut scale, mut video) = (0, 0, false);
                for (t2, b2) in boxes(body) {
                    if t2 == b"tkhd" {
                        id = if b2.first() == Some(&1) { be32(b2, 20)? } else { be32(b2, 12)? };
                    } else if t2 == b"mdia" {
                        for (t3, b3) in boxes(b2) {
                            if t3 == b"mdhd" {
                                scale = if b3.first() == Some(&1) { be32(b3, 20)? } else { be32(b3, 12)? };
                            } else if t3 == b"hdlr" {
                                video = b3.get(8..12) == Some(b"vide");
                            }
                        }
                    }
                }
                if video && found.is_none() && scale > 0 {
                    found = Some((id, scale));
                }
            }
            b"mvex" => {
                for (t2, b2) in boxes(body) {
                    if t2 == b"trex" {
                        trex.insert(be32(b2, 4)?, be32(b2, 12)?);
                    }
                }
            }
            _ => {}
        }
    }
    let (id, scale) = found?;
    Some((id, scale, trex.get(&id).copied().unwrap_or(0)))
}

/// Decode time and length (in timescale units) of the video in a `moof`.
fn moof_video(moof: &[u8], track: u32, default_dur: u32) -> Option<(u64, u64)> {
    for (t, traf) in boxes(moof) {
        if t != b"traf" {
            continue;
        }
        let (mut id, mut def, mut base, mut total) = (0, default_dur, 0u64, 0u64);
        for (t2, b) in boxes(traf) {
            match t2 {
                b"tfhd" => {
                    let flags = be32(b, 0)? & 0xff_ffff;
                    id = be32(b, 4)?;
                    let mut i = 8;
                    if flags & 0x1 != 0 {
                        i += 8;
                    }
                    if flags & 0x2 != 0 {
                        i += 4;
                    }
                    if flags & 0x8 != 0 {
                        def = be32(b, i)?;
                    }
                }
                b"tfdt" => base = if b.first() == Some(&1) { be64(b, 4)? } else { be32(b, 4)? as u64 },
                b"trun" => {
                    let flags = be32(b, 0)? & 0xff_ffff;
                    let n = be32(b, 4)? as usize;
                    let mut i = 8;
                    if flags & 0x1 != 0 {
                        i += 4;
                    }
                    if flags & 0x4 != 0 {
                        i += 4;
                    }
                    let per = [0x100, 0x200, 0x400, 0x800].iter().filter(|f| flags & **f != 0).count() * 4;
                    if flags & 0x100 != 0 {
                        for s in 0..n {
                            total += be32(b, i + s * per)? as u64;
                        }
                    } else {
                        total += def as u64 * n as u64;
                    }
                }
                _ => {}
            }
        }
        if id == track {
            return Some((base, total));
        }
    }
    None
}

/// Index a fragmented MP4 file (only complete pieces: a file still being
/// recorded is indexed as far as it's written).
pub fn index_file(path: &Path) -> Option<FileIndex> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let mut pos = 0u64;
    let mut track = None;
    let mut out = FileIndex::default();
    let mut pending: Option<(u64, f64, f64)> = None; // moof start, start, duration
    while pos + 8 <= len {
        f.seek(SeekFrom::Start(pos)).ok()?;
        let mut h = [0u8; 16];
        f.read_exact(&mut h[..8]).ok()?;
        let mut size = u32::from_be_bytes(h[0..4].try_into().ok()?) as u64;
        let mut hdr = 8u64;
        if size == 1 {
            f.read_exact(&mut h[8..16]).ok()?;
            size = u64::from_be_bytes(h[8..16].try_into().ok()?);
            hdr = 16;
        } else if size == 0 {
            break; // runs to the end: still being written
        }
        if size < hdr || pos + size > len {
            break; // not complete yet
        }
        let kind = [h[4], h[5], h[6], h[7]];
        match &kind {
            b"moov" | b"moof" => {
                if size > 64 << 20 {
                    return None;
                }
                let mut body = vec![0; (size - hdr) as usize];
                f.read_exact(&mut body).ok()?;
                if &kind == b"moov" {
                    track = video_track(&body);
                    out.init_len = pos + size;
                } else if let Some((id, scale, def)) = track
                    && let Some((base, dur)) = moof_video(&body, id, def)
                {
                    let (s, d) = (base as f64 / scale as f64, dur as f64 / scale as f64);
                    if out.pieces.is_empty() && pending.is_none() {
                        out.start = s;
                    }
                    pending = Some((pos, s, d));
                }
            }
            b"mdat" => {
                if let Some((p0, _, d)) = pending.take() {
                    out.pieces.push(Piece { off: p0, len: pos + size - p0, dur: d });
                }
            }
            _ => {}
        }
        pos += size;
    }
    (track.is_some() && out.init_len > 0 && !out.pieces.is_empty()).then_some(out)
}

/// Indexes of files, kept while the file doesn't change.
pub fn index_cached(path: &Path) -> Option<Arc<FileIndex>> {
    type Cache = HashMap<PathBuf, (u64, Arc<FileIndex>)>;
    static CACHE: Mutex<Option<Cache>> = Mutex::new(None);
    let len = std::fs::metadata(path).ok()?.len();
    if let Some((l, ix)) = CACHE.lock().ok()?.get_or_insert_with(HashMap::new).get(path)
        && *l == len
    {
        return Some(ix.clone());
    }
    let ix = Arc::new(index_file(path)?);
    let mut c = CACHE.lock().ok()?;
    let map = c.get_or_insert_with(HashMap::new);
    if map.len() > 20_000 {
        map.clear();
    }
    map.insert(path.to_path_buf(), (len, ix.clone()));
    Some(ix)
}

/// One file in a playlist: where it is, its URL (relative to the page) and
/// when it starts.
pub struct PlaylistFile {
    pub path: PathBuf,
    pub url: String,
    pub start: DateTime<Local>,
}

/// An HLS playlist of these files, in order. Pieces are grouped into parts
/// of about `part` seconds. URLs are made relative to the playlist: `up`
/// leads from where it's served back to the page (e.g. "../../../").
pub fn playlist(files: &[PlaylistFile], part: f64, up: &str) -> String {
    playlist_as(files, part, up, false)
}

/// The same; `live`: a playlist that keeps growing (up to now, more is
/// added as it's recorded): the player asks for it again every few seconds
/// and carries on without starting over.
pub fn playlist_as(files: &[PlaylistFile], part: f64, up: &str, live: bool) -> String {
    let mut body = String::new();
    let mut longest: f64 = 1.0;
    let mut first = true;
    let mut end: Option<DateTime<Local>> = None;
    for file in files {
        let Some(ix) = index_cached(&file.path) else { continue };
        // Overlapping files (rare): the earlier one is kept. Ends are
        // measured from the pieces (a file's modified time can be later).
        let len: f64 = ix.pieces.iter().map(|p| p.dur).sum();
        let start = file.start + chrono::Duration::milliseconds((ix.start * 1000.0) as i64);
        if end.is_some_and(|e| start < e - chrono::Duration::seconds(2)) {
            continue;
        }
        end = Some(start + chrono::Duration::milliseconds((len * 1000.0) as i64));
        let url = format!("{up}{}", file.url);
        if !first {
            body.push_str("#EXT-X-DISCONTINUITY\n");
        }
        first = false;
        let _ = writeln!(body, "#EXT-X-MAP:URI=\"{url}\",BYTERANGE=\"{}@0\"", ix.init_len);
        // Group consecutive pieces (they're next to each other in the file).
        let mut t = ix.start;
        let mut i = 0;
        while i < ix.pieces.len() {
            let (off, mut len, mut dur) = (ix.pieces[i].off, ix.pieces[i].len, ix.pieces[i].dur);
            i += 1;
            while i < ix.pieces.len() && dur < part {
                len += ix.pieces[i].len;
                dur += ix.pieces[i].dur;
                i += 1;
            }
            let when = file.start + chrono::Duration::milliseconds((t * 1000.0) as i64);
            let _ = writeln!(body, "#EXT-X-PROGRAM-DATE-TIME:{}", when.format("%Y-%m-%dT%H:%M:%S%.3f%:z"));
            let _ = writeln!(body, "#EXTINF:{dur:.3},\n#EXT-X-BYTERANGE:{len}@{off}\n{url}");
            longest = longest.max(dur);
            t += dur;
        }
    }
    format!(
        "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:{}\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-PLAYLIST-TYPE:{}\n#EXT-X-INDEPENDENT-SEGMENTS\n{body}{}",
        longest.ceil() as u64,
        if live { "EVENT" } else { "VOD" },
        if live { "" } else { "#EXT-X-ENDLIST\n" }
    )
}

#[cfg(test)]
mod tests {
    fn bx(t: &[u8], body: &[u8]) -> Vec<u8> {
        [&((body.len() + 8) as u32).to_be_bytes()[..], t, body].concat()
    }
    fn full(v: u8, flags: u32, rest: &[u8]) -> Vec<u8> {
        [&(((v as u32) << 24) | flags).to_be_bytes()[..], rest].concat()
    }

    /// A small fragmented MP4: video track 1 at 90 kHz, `n` pieces of 2 s.
    fn sample(n: usize, cut_last: bool) -> Vec<u8> {
        let mut tkhd = vec![0u8; 80];
        tkhd[12..16].copy_from_slice(&1u32.to_be_bytes());
        let mut mdhd = vec![0u8; 20];
        mdhd[12..16].copy_from_slice(&90000u32.to_be_bytes());
        let mut hdlr = vec![0u8; 20];
        hdlr[8..12].copy_from_slice(b"vide");
        let trak = bx(b"trak", &[bx(b"tkhd", &tkhd), bx(b"mdia", &[bx(b"mdhd", &mdhd), bx(b"hdlr", &hdlr)].concat())].concat());
        let trex = bx(b"trex", &full(0, 0, &[1u32.to_be_bytes(), 1u32.to_be_bytes(), 3000u32.to_be_bytes(), 0u32.to_be_bytes(), 0u32.to_be_bytes()].concat()));
        let mut out = [bx(b"ftyp", b"isom\0\0\0\0"), bx(b"moov", &[trak, bx(b"mvex", &trex)].concat())].concat();
        for k in 0..n {
            // 60 samples of the default 3000 units = 2 s.
            let tfhd = bx(b"tfhd", &full(0, 0x20000, &1u32.to_be_bytes()));
            let tfdt = bx(b"tfdt", &full(1, 0, &((k as u64) * 180000).to_be_bytes()));
            let trun = bx(b"trun", &full(0, 0x200, &[&60u32.to_be_bytes()[..], &[0u8; 240][..]].concat()));
            out.extend(bx(b"moof", &bx(b"traf", &[tfhd, tfdt, trun].concat())));
            out.extend(bx(b"mdat", &vec![7u8; 500]));
        }
        if cut_last {
            out.truncate(out.len() - 100);
        }
        out
    }

    #[test]
    fn indexes_pieces_and_skips_an_unfinished_one() {
        let dir = std::env::temp_dir().join(format!("gaze-hls-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.mp4");
        std::fs::write(&p, sample(5, true)).unwrap();
        let ix = super::index_file(&p).unwrap();
        assert_eq!(ix.pieces.len(), 4);
        assert!(ix.pieces.iter().all(|x| (x.dur - 2.0).abs() < 1e-9));
        assert_eq!(ix.pieces[1].off, ix.pieces[0].off + ix.pieces[0].len);
        let pl = super::playlist(
            &[super::PlaylistFile { path: p.clone(), url: "media/c/rec/a.mp4".into(), start: chrono::Local::now() }],
            4.0,
            "../../../",
        );
        assert_eq!(pl.matches("#EXTINF:4.000").count(), 2);
        assert!(pl.contains(&format!("BYTERANGE=\"{}@0\"", ix.init_len)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
