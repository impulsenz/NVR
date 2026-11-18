//! Live video for browsers (fragmented MP4 over a WebSocket, played with
//! Media Source Extensions): ONE repackaging ffmpeg per camera stream,
//! shared by every viewer.
//!
//! Before, each browser tab watching a camera started its own ffmpeg to
//! repackage the relay's MPEG-TS as fragmented MP4 — three people watching
//! 16 cameras meant 48 extra processes, each reading the same data. Now the
//! first viewer starts the repackager, later viewers join it, and it stops
//! a little after the last one leaves.
//!
//! The video is never decoded or re-encoded here (`-c:v copy`). A viewer
//! joining gets the MP4 header and the fragments since the latest keyframe,
//! so its picture starts at once. Each viewer has a bounded queue: one that
//! can't keep up (a slow phone connection) is dropped and reconnects, rather
//! than holding video in memory or slowing the others down.

use crate::sync::MutexExt;
use crate::relay::Relay;
use bytes::{Bytes, BytesMut};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Fragments queued for one viewer before it's dropped (~100 ms each: about
/// 25 s of video, far more than a viewer that's keeping up ever holds).
const VIEWER_QUEUE: usize = 256;
/// Largest keyframe group kept for viewers joining (a camera with a very
/// long keyframe interval): beyond this, joiners wait for the next keyframe.
const MAX_GOP_BYTES: usize = 32 * 1024 * 1024;
/// Keep the repackager running this long after the last viewer leaves, so
/// switching layouts or reopening a camera doesn't restart it.
const LINGER: Duration = Duration::from_secs(10);

struct Shared {
    viewers: Vec<mpsc::Sender<Bytes>>,
    /// `ftyp` + `moov`: what a player needs first.
    init: Option<Bytes>,
    /// Fragments (`moof` + `mdat`) since the latest keyframe.
    gop: Vec<Bytes>,
    gop_bytes: usize,
    running: bool,
}

pub struct LiveMux {
    relay: Arc<Relay>,
    ffmpeg: String,
    shared: Mutex<Shared>,
    wake: Notify,
}

impl LiveMux {
    pub fn new(relay: Arc<Relay>, ffmpeg: &str) -> Arc<Self> {
        Arc::new(Self {
            relay,
            ffmpeg: ffmpeg.to_string(),
            shared: Mutex::new(Shared { viewers: Vec::new(), init: None, gop: Vec::new(), gop_bytes: 0, running: false }),
            wake: Notify::new(),
        })
    }

    /// Viewers watching now.
    pub fn viewers(&self) -> usize {
        self.shared.locked().viewers.iter().filter(|v| !v.is_closed()).count()
    }

    /// Watch: MP4 pieces to pass on to the browser as they are. Ends when
    /// the stream stops (the browser then reconnects).
    pub fn subscribe(self: &Arc<Self>) -> mpsc::Receiver<Bytes> {
        let (tx, rx) = mpsc::channel(VIEWER_QUEUE);
        let mut s = self.shared.locked();
        if let Some(init) = &s.init {
            // The header and the latest keyframe group in one piece.
            let mut first = BytesMut::with_capacity(init.len() + s.gop_bytes);
            first.extend_from_slice(init);
            for f in &s.gop {
                first.extend_from_slice(f);
            }
            let _ = tx.try_send(first.freeze());
        }
        s.viewers.push(tx);
        if !s.running {
            s.running = true;
            tokio::spawn(self.clone().run());
        }
        drop(s);
        self.wake.notify_one();
        rx
    }

    fn tag(&self) -> &'static str {
        if self.relay.info().is_some_and(|i| i.codec.contains("265") || i.codec.contains("hevc")) { "hvc1" } else { "avc1" }
    }

    async fn run(self: Arc<Self>) {
        let name = format!("[{}] {}", self.relay.name, self.relay.label);
        let res = self.serve().await;
        if let Err(e) = &res {
            warn!("{name}: live video (browser) stopped: {e:#}");
        }
        let mut s = self.shared.locked();
        s.init = None;
        s.gop.clear();
        s.gop_bytes = 0;
        // The viewers' streams end (they reconnect, starting another).
        s.viewers.clear();
        s.running = false;
    }

    async fn serve(self: &Arc<Self>) -> anyhow::Result<()> {
        // The codec decides the MP4 tag: wait (briefly) for the camera.
        let _ = self.relay.wait_info(Duration::from_secs(8)).await;
        let mut child = crate::load::cmd(&self.ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-fflags", "+genpts"])
            .args(self.relay.input_args())
            .args(["-map", "0:v:0", "-c:v", "copy", "-tag:v", self.tag(), "-an"])
            .args(["-f", "mp4", "-movflags", "empty_moov+default_base_moof+frag_keyframe", "-frag_duration", "100000", "pipe:1"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let stop = CancellationToken::new();
        let _feed = crate::relay::feed(self.relay.subscribe(), child.stdin.take().expect("stdin"), stop.clone());
        let _stop = stop.drop_guard();
        let mut out = child.stdout.take().expect("stdout");
        let mut boxes = BoxReader::default();
        let mut buf = vec![0u8; 256 * 1024];
        let mut alone_since: Option<tokio::time::Instant> = None;
        let mut started = false;
        loop {
            let linger = async {
                match alone_since {
                    Some(t) => tokio::time::sleep_until(t + LINGER).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                n = out.read(&mut buf) => {
                    let n = n?;
                    if n == 0 {
                        anyhow::bail!("the camera stream ended");
                    }
                    for piece in boxes.push(&buf[..n]) {
                        if !started {
                            started = true;
                            info!("[{}] {}: live video for browsers started (shared)", self.relay.name, self.relay.label);
                        }
                        self.publish(piece);
                    }
                }
                _ = self.wake.notified() => {}
                _ = linger => {
                    info!("[{}] {}: live video for browsers stopped (nobody watching)", self.relay.name, self.relay.label);
                    return Ok(());
                }
            }
            alone_since = match (self.viewers(), alone_since) {
                (0, None) => Some(tokio::time::Instant::now()),
                (0, t) => t,
                _ => None,
            };
        }
    }

    /// Keep and send one piece of the MP4 stream.
    fn publish(&self, piece: Piece) {
        let mut s = self.shared.locked();
        let bytes = match piece {
            Piece::Init(b) => {
                s.init = Some(b.clone());
                s.gop.clear();
                s.gop_bytes = 0;
                b
            }
            Piece::Fragment { data, keyframe } => {
                if keyframe {
                    s.gop.clear();
                    s.gop_bytes = 0;
                }
                if s.gop_bytes + data.len() <= MAX_GOP_BYTES && (keyframe || !s.gop.is_empty()) {
                    s.gop_bytes += data.len();
                    s.gop.push(data.clone());
                }
                data
            }
        };
        // A viewer that's full (can't keep up) or gone is dropped.
        s.viewers.retain(|v| v.try_send(bytes.clone()).is_ok());
    }
}

/// One complete piece of a fragmented MP4 stream.
#[derive(Debug)]
enum Piece {
    /// `ftyp` + `moov`.
    Init(Bytes),
    /// `moof` + `mdat` (with any boxes between).
    Fragment { data: Bytes, keyframe: bool },
}

/// Splits ffmpeg's fragmented MP4 output into whole pieces.
#[derive(Default)]
struct BoxReader {
    buf: BytesMut,
    /// Boxes of the piece being collected.
    pending: BytesMut,
    keyframe: bool,
    have_init: bool,
}

impl BoxReader {
    fn push(&mut self, data: &[u8]) -> Vec<Piece> {
        self.buf.extend_from_slice(data);
        let mut out = Vec::new();
        loop {
            if self.buf.len() < 8 {
                break;
            }
            let size32 = u32::from_be_bytes(self.buf[0..4].try_into().unwrap()) as u64;
            let kind: [u8; 4] = self.buf[4..8].try_into().unwrap();
            let size = match size32 {
                1 => {
                    if self.buf.len() < 16 {
                        break;
                    }
                    u64::from_be_bytes(self.buf[8..16].try_into().unwrap())
                }
                // "To the end of the file": not used for live fragments.
                0 => break,
                n => n,
            } as usize;
            if size < 8 {
                // Not an MP4 box: drop what we have and wait for a header.
                self.buf.clear();
                break;
            }
            if self.buf.len() < size {
                break;
            }
            let b = self.buf.split_to(size).freeze();
            match &kind {
                b"ftyp" => {
                    self.pending.clear();
                    self.pending.extend_from_slice(&b);
                }
                b"moov" => {
                    self.pending.extend_from_slice(&b);
                    self.have_init = true;
                    out.push(Piece::Init(self.pending.split().freeze()));
                }
                b"moof" => {
                    self.pending.clear();
                    self.keyframe = moof_starts_with_keyframe(&b[8..]);
                    self.pending.extend_from_slice(&b);
                }
                b"mdat" => {
                    if self.have_init && !self.pending.is_empty() {
                        self.pending.extend_from_slice(&b);
                        out.push(Piece::Fragment { data: self.pending.split().freeze(), keyframe: self.keyframe });
                    }
                }
                _ => {
                    // (styp, sidx, prft …) belong to the next piece.
                    if self.have_init {
                        self.pending.extend_from_slice(&b);
                    }
                }
            }
        }
        out
    }
}

/// The child boxes of a box's body: (type, body).
fn children(mut d: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    std::iter::from_fn(move || {
        if d.len() < 8 {
            return None;
        }
        let size = u32::from_be_bytes(d[0..4].try_into().ok()?) as usize;
        let kind: [u8; 4] = d[4..8].try_into().ok()?;
        if size < 8 || size > d.len() {
            return None;
        }
        let body = &d[8..size];
        d = &d[size..];
        Some((kind, body))
    })
}

/// Does this fragment start with a keyframe (a sync sample)? From the
/// track fragment's first sample flags (or its default sample flags). When
/// it can't tell, it says yes: a joining viewer then starts there, which at
/// worst shows a moment of grey instead of waiting.
fn moof_starts_with_keyframe(moof: &[u8]) -> bool {
    const NON_SYNC: u32 = 0x0001_0000;
    for (k, traf) in children(moof) {
        if &k != b"traf" {
            continue;
        }
        let mut default_flags = None;
        for (k, body) in children(traf) {
            let be = |o: usize| body.get(o..o + 4).map(|x| u32::from_be_bytes(x.try_into().unwrap()));
            match &k {
                b"tfhd" => {
                    let Some(vf) = be(0) else { continue };
                    let flags = vf & 0xFF_FFFF;
                    // track_ID, then the optional fields in order.
                    let mut o = 8;
                    for (bit, len) in [(0x01, 8), (0x02, 4), (0x08, 4), (0x10, 4)] {
                        if flags & bit != 0 {
                            o += len;
                        }
                    }
                    if flags & 0x20 != 0 {
                        default_flags = be(o);
                    }
                }
                b"trun" => {
                    let Some(vf) = be(0) else { continue };
                    let flags = vf & 0xFF_FFFF;
                    // sample_count, then data_offset if present.
                    let mut o = 8;
                    if flags & 0x01 != 0 {
                        o += 4;
                    }
                    let first = if flags & 0x04 != 0 {
                        be(o)
                    } else if flags & 0x400 != 0 {
                        // Per-sample flags: the first sample's.
                        let skip = if flags & 0x100 != 0 { 4 } else { 0 } + if flags & 0x200 != 0 { 4 } else { 0 };
                        be(o + skip)
                    } else {
                        default_flags
                    };
                    return first.is_none_or(|f| f & NON_SYNC == 0);
                }
                _ => {}
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }

    /// A moof whose trun gives the first sample's flags.
    fn moof(first_flags: u32) -> Vec<u8> {
        let mut trun = (0x0000_0005u32).to_be_bytes().to_vec(); // data offset + first sample flags
        trun.extend_from_slice(&1u32.to_be_bytes()); // sample count
        trun.extend_from_slice(&0u32.to_be_bytes()); // data offset
        trun.extend_from_slice(&first_flags.to_be_bytes());
        let mut tfhd = (0x0002_0000u32).to_be_bytes().to_vec(); // default-base-is-moof
        tfhd.extend_from_slice(&1u32.to_be_bytes());
        let traf = [bx(b"tfhd", &tfhd), bx(b"trun", &trun)].concat();
        bx(b"moof", &[bx(b"mfhd", &[0; 8]), bx(b"traf", &traf)].concat())
    }

    #[test]
    fn splits_the_stream_into_header_and_fragments() {
        let stream = [
            bx(b"ftyp", b"isom"),
            bx(b"moov", &[1, 2, 3]),
            moof(0x0200_0000), // keyframe ("depends on no other")
            bx(b"mdat", &[9; 50]),
            moof(0x0101_0000), // not a keyframe
            bx(b"mdat", &[8; 20]),
        ]
        .concat();
        // Arriving in awkward pieces.
        let mut r = BoxReader::default();
        let mut got = Vec::new();
        for chunk in stream.chunks(7) {
            got.extend(r.push(chunk));
        }
        assert_eq!(got.len(), 3);
        assert!(matches!(&got[0], Piece::Init(b) if b.len() == 12 + 11));
        assert!(matches!(&got[1], Piece::Fragment { keyframe: true, .. }));
        assert!(matches!(&got[2], Piece::Fragment { keyframe: false, .. }));
        let total: usize = got.iter().map(|p| match p { Piece::Init(b) | Piece::Fragment { data: b, .. } => b.len() }).sum();
        assert_eq!(total, stream.len());
    }

    #[test]
    fn default_sample_flags_decide_when_the_run_has_none() {
        let mut tfhd = (0x0002_0020u32).to_be_bytes().to_vec();
        tfhd.extend_from_slice(&1u32.to_be_bytes());
        tfhd.extend_from_slice(&0x0101_0000u32.to_be_bytes());
        let mut trun = 0u32.to_be_bytes().to_vec();
        trun.extend_from_slice(&1u32.to_be_bytes());
        let traf = [bx(b"tfhd", &tfhd), bx(b"trun", &trun)].concat();
        let m = bx(b"moof", &bx(b"traf", &traf));
        assert!(!moof_starts_with_keyframe(&m[8..]));
    }

    /// ffmpeg's real output (when ffmpeg is installed): keyframes found
    /// where they are, every fragment kept.
    #[test]
    fn finds_keyframes_in_ffmpeg_output() {
        let Ok(out) = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "testsrc2=size=320x240:rate=25", "-t", "4"])
            .args(["-c:v", "libx264", "-g", "25", "-f", "mp4", "-movflags", "empty_moov+default_base_moof+frag_keyframe"])
            .args(["-frag_duration", "100000", "pipe:1"])
            .output()
        else {
            return;
        };
        if !out.status.success() || out.stdout.is_empty() {
            return;
        }
        let mut r = BoxReader::default();
        let got = r.push(&out.stdout);
        let keys = got.iter().filter(|p| matches!(p, Piece::Fragment { keyframe: true, .. })).count();
        let frags = got.iter().filter(|p| matches!(p, Piece::Fragment { .. })).count();
        assert!(matches!(got[0], Piece::Init(_)));
        assert_eq!(keys, 4, "one keyframe a second for 4 seconds");
        assert!(frags > 20, "fragments of about 100 ms: {frags}");
    }
}
