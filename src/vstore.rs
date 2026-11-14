//! Reading recordings for playback, built for slow storage (a NAS, a busy
//! hard drive).
//!
//! The browser asks for a recording piece by piece (a few hundred KB at a
//! time, many requests per minute per camera). Opening the file and reading
//! a small piece for each request is cheap on a local SSD, but on a network
//! drive every open and every read is a round trip to the NAS (and often an
//! antivirus check), which made playback crawl.
//!
//! So instead:
//! - files stay open between requests (a small pool);
//! - they're read in large blocks (2 MB) — a network drive is fast at large
//!   reads and slow at many small ones — kept in memory for a while;
//! - while one block is being watched, the next ones are already being read
//!   (read-ahead), so the video doesn't wait for the NAS.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime};

pub const BLOCK: u64 = 2 << 20;
/// Memory kept for blocks (least recently used go first).
const BUDGET: u64 = 384 << 20;
/// Blocks read ahead of the one asked for.
const AHEAD: u64 = 3;
const OPEN_FILES: usize = 64;

#[derive(Clone, Copy)]
struct Meta {
    len: u64,
    modified: SystemTime,
    at: Instant,
}

#[derive(Default)]
struct State {
    files: HashMap<PathBuf, (Arc<File>, Instant)>,
    meta: HashMap<PathBuf, Meta>,
    blocks: HashMap<(PathBuf, u64), (Arc<Vec<u8>>, Instant)>,
    bytes: u64,
    reading: HashSet<(PathBuf, u64)>,
}

static ST: LazyLock<Mutex<State>> = LazyLock::new(Default::default);
/// Read-ahead is done by a few threads for the whole server (it never
/// competes much with what's being asked for right now).
static AHEAD_SLOTS: Mutex<usize> = Mutex::new(0);

/// Testing: pretend every open and read of the storage takes this long (a
/// network drive), to measure how playback copes.
fn test_delay() {
    static D: LazyLock<Option<Duration>> =
        LazyLock::new(|| std::env::var("GAZE_TEST_IO_DELAY_MS").ok().and_then(|v| v.parse().ok()).map(Duration::from_millis));
    if let Some(d) = *D {
        std::thread::sleep(d);
    }
    IO_OPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}
/// Opens and reads made on the storage (for tests and the log).
pub static IO_OPS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn open(path: &Path) -> std::io::Result<Arc<File>> {
    if let Some((f, at)) = ST.lock().unwrap().files.get_mut(path) {
        *at = Instant::now();
        return Ok(f.clone());
    }
    test_delay();
    let f = Arc::new(File::open(path)?);
    let mut st = ST.lock().unwrap();
    close_idle(&mut st);
    if st.files.len() >= OPEN_FILES
        && let Some(old) = st.files.iter().min_by_key(|(_, (_, t))| *t).map(|(p, _)| p.clone())
    {
        st.files.remove(&old);
    }
    st.files.insert(path.to_path_buf(), (f.clone(), Instant::now()));
    Ok(f)
}

/// Size and time of a file: asked of the disk at most every 1½ s for a file
/// being recorded (it grows), every 10 minutes for a finished one (on a
/// network drive each look is a trip to the NAS).
pub fn meta(path: &Path) -> std::io::Result<(u64, SystemTime)> {
    if let Some(m) = ST.lock().unwrap().meta.get(path)
        && m.at.elapsed() < if growing(m.modified) { Duration::from_millis(1500) } else { Duration::from_secs(600) }
    {
        return Ok((m.len, m.modified));
    }
    test_delay();
    let md = std::fs::metadata(path)?;
    if !md.is_file() {
        return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "not a file"));
    }
    let m = Meta { len: md.len(), modified: md.modified().unwrap_or(SystemTime::UNIX_EPOCH), at: Instant::now() };
    let mut st = ST.lock().unwrap();
    if st.meta.len() > 50_000 {
        st.meta.clear();
    }
    st.meta.insert(path.to_path_buf(), m);
    Ok((m.len, m.modified))
}

/// Size and time of a file, as a folder listing told them.
pub fn seed(path: &Path, len: u64, modified: SystemTime) {
    let mut st = ST.lock().unwrap();
    if st.meta.len() > 50_000 {
        st.meta.clear();
    }
    st.meta.insert(path.to_path_buf(), Meta { len, modified, at: Instant::now() });
}

/// Close every file kept open (before files or folders are deleted, renamed
/// or moved: Windows doesn't allow some of that while a file is open). What
/// was read stays in memory.
pub fn close_all() {
    ST.lock().unwrap().files.clear();
}

/// Files not asked for in a while are closed.
fn close_idle(st: &mut State) {
    st.files.retain(|_, (_, at)| at.elapsed() < Duration::from_secs(15));
}

/// The file was replaced or deleted: forget what's kept of it.
#[allow(dead_code)]
pub fn forget(path: &Path) {
    let mut st = ST.lock().unwrap();
    st.files.remove(path);
    st.meta.remove(path);
    let gone: Vec<_> = st.blocks.keys().filter(|(p, _)| p == path).cloned().collect();
    for k in gone {
        if let Some((b, _)) = st.blocks.remove(&k) {
            st.bytes -= b.len() as u64;
        }
    }
}

fn read_at(f: &File, buf: &mut [u8], off: u64) -> std::io::Result<usize> {
    #[cfg(unix)]
    {
        std::os::unix::fs::FileExt::read_at(f, buf, off)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::FileExt::seek_read(f, buf, off)
    }
}

/// A file that may still be growing (being recorded): its last, partial
/// block isn't kept.
fn growing(modified: SystemTime) -> bool {
    modified.elapsed().map(|e| e < Duration::from_secs(90)).unwrap_or(true)
}

/// Block `n` of a file (read now, or kept from before).
fn block(path: &Path, n: u64, len: u64, modified: SystemTime) -> std::io::Result<Arc<Vec<u8>>> {
    let key = (path.to_path_buf(), n);
    // Being read by the read-ahead right now: wait for it rather than read
    // the same part twice.
    for _ in 0..400 {
        let mut st = ST.lock().unwrap();
        if let Some((b, at)) = st.blocks.get_mut(&key) {
            *at = Instant::now();
            return Ok(b.clone());
        }
        if !st.reading.contains(&key) {
            st.reading.insert(key.clone());
            break;
        }
        drop(st);
        std::thread::sleep(Duration::from_millis(5));
    }
    let res = (|| -> std::io::Result<Arc<Vec<u8>>> {
        let f = open(path)?;
        let start = n * BLOCK;
        let size = BLOCK.min(len.saturating_sub(start)) as usize;
        let mut buf = vec![0u8; size];
        let mut got = 0;
        while got < size {
            test_delay();
            let k = read_at(&f, &mut buf[got..], start + got as u64)?;
            if k == 0 {
                break;
            }
            got += k;
        }
        buf.truncate(got);
        Ok(Arc::new(buf))
    })();
    let mut st = ST.lock().unwrap();
    st.reading.remove(&key);
    let b = res?;
    // Kept unless it's the unfinished end of a file still being recorded.
    let full = b.len() as u64 == BLOCK || !growing(modified) || (n + 1) * BLOCK <= len;
    if full && !b.is_empty() {
        st.bytes += b.len() as u64;
        st.blocks.insert(key, (b.clone(), Instant::now()));
        if st.bytes > BUDGET {
            let mut by_age: Vec<_> = st.blocks.iter().map(|(k, (b, t))| (*t, k.clone(), b.len() as u64)).collect();
            by_age.sort_by_key(|x| x.0);
            for (_, k, sz) in by_age {
                if st.bytes <= BUDGET * 9 / 10 {
                    break;
                }
                st.blocks.remove(&k);
                st.bytes -= sz;
            }
        }
    }
    Ok(b)
}

/// The recording after each one (from the playlists): when playback nears
/// the end of a file, the start of the next is read too, so crossing into
/// it doesn't wait for the NAS.
static NEXT: LazyLock<Mutex<HashMap<PathBuf, PathBuf>>> = LazyLock::new(Default::default);

pub fn set_next(files: &[PathBuf]) {
    let mut n = NEXT.lock().unwrap();
    if n.len() > 50_000 {
        n.clear();
    }
    for w in files.windows(2) {
        n.insert(w[0].clone(), w[1].clone());
    }
}

/// Read the next few blocks in the background (into the next recording
/// when this one ends).
fn read_ahead(path: &Path, from: u64, len: u64, modified: SystemTime) {
    let last = len.saturating_sub(1) / BLOCK;
    if from + AHEAD > last + 1 && !growing(modified) {
        let next = NEXT.lock().unwrap().get(path).cloned();
        if let Some(next) = next {
            let left = (from + AHEAD).saturating_sub(last + 1).min(2);
            if left > 0 {
                std::thread::spawn(move || {
                    if let Ok((l2, m2)) = meta(&next) {
                        for n in 0..left {
                            if n * BLOCK < l2 {
                                let _ = block(&next, n, l2, m2);
                            }
                        }
                    }
                });
            }
        }
    }
    for n in from..=(from + AHEAD - 1).min(last) {
        let key = (path.to_path_buf(), n);
        {
            let st = ST.lock().unwrap();
            if st.blocks.contains_key(&key) || st.reading.contains(&key) {
                continue;
            }
        }
        {
            let mut busy = AHEAD_SLOTS.lock().unwrap();
            if *busy >= 4 {
                return;
            }
            *busy += 1;
        }
        let p = path.to_path_buf();
        std::thread::spawn(move || {
            let _ = block(&p, n, len, modified);
            *AHEAD_SLOTS.lock().unwrap() -= 1;
        });
    }
}

static LAST_READ: Mutex<Option<Instant>> = Mutex::new(None);

/// Someone has been watching recordings in the last 20 seconds (background
/// work that reads recordings waits).
pub fn watching() -> bool {
    LAST_READ.lock().unwrap().is_some_and(|t| t.elapsed() < Duration::from_secs(20))
}

/// Bytes `[a, b]` (inclusive) of a file, through the blocks.
pub fn read(path: &Path, a: u64, b: u64) -> std::io::Result<Vec<u8>> {
    *LAST_READ.lock().unwrap() = Some(Instant::now());
    let (len, modified) = meta(path)?;
    if len == 0 || a >= len {
        return Ok(Vec::new());
    }
    let b = b.min(len - 1);
    let mut out = Vec::with_capacity((b - a + 1) as usize);
    let (n0, n1) = (a / BLOCK, b / BLOCK);
    for n in n0..=n1 {
        let blk = block(path, n, len, modified)?;
        let s = if n == n0 { (a - n * BLOCK) as usize } else { 0 };
        let e = if n == n1 { (b - n * BLOCK + 1) as usize } else { BLOCK as usize };
        let e = e.min(blk.len());
        if s < e {
            out.extend_from_slice(&blk[s..e]);
        }
        if e < (if n == n1 { (b - n * BLOCK + 1) as usize } else { BLOCK as usize }) {
            break; // the file ended early (being written)
        }
    }
    read_ahead(path, n1 + 1, len, modified);
    Ok(out)
}

/// Byte range asked for by a `Range` header: (first, last) of `len`.
pub fn parse_range(h: &str, len: u64) -> Option<(u64, u64)> {
    let r = h.trim().strip_prefix("bytes=")?;
    if r.contains(',') || len == 0 {
        return None;
    }
    let (a, b) = r.split_once('-')?;
    let (a, b) = (a.trim(), b.trim());
    if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        if n == 0 {
            return None;
        }
        return Some((len.saturating_sub(n), len - 1));
    }
    let a: u64 = a.parse().ok()?;
    let b: u64 = if b.is_empty() { len - 1 } else { b.parse::<u64>().ok()?.min(len - 1) };
    (a <= b && a < len).then_some((a, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-", 100), Some((0, 99)));
        assert_eq!(parse_range("bytes=10-19", 100), Some((10, 19)));
        assert_eq!(parse_range("bytes=90-200", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=-10", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=100-", 100), None);
        assert_eq!(parse_range("bytes=5-1", 100), None);
    }

    #[test]
    fn reads_match_the_file() {
        let d = std::env::temp_dir().join(format!("gaze-vstore-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("a.mp4");
        let data: Vec<u8> = (0..(BLOCK * 2 + 12345)).map(|i| (i * 7 % 251) as u8).collect();
        std::fs::write(&p, &data).unwrap();
        for (a, b) in [(0, 10), (BLOCK - 5, BLOCK + 5), (1, BLOCK * 2 + 12344), (BLOCK * 2, BLOCK * 2 + 100000)] {
            let got = read(&p, a, b).unwrap();
            let b2 = b.min(data.len() as u64 - 1);
            assert_eq!(got, data[a as usize..=b2 as usize], "{a}-{b}");
        }
        forget(&p);
        let _ = std::fs::remove_dir_all(&d);
    }
}
