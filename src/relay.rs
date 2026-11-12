//! Stream relay: exactly ONE connection to each camera stream, shared by
//! everything that needs it (recording, pre-event buffer, detection, live
//! view, snapshots).
//!
//! Many cameras (TP-Link Tapo, cheaper Hikvision/Dahua models, …) only allow
//! a couple of simultaneous RTSP sessions and answer extra ones with errors
//! such as "453 Not Enough Bandwidth" or "500 Internal Server Error". So one
//! ffmpeg process pulls the stream and re-packages it (video copied, audio
//! converted to AAC) as MPEG-TS; the NVR fans that out to local ffmpeg
//! processes through their stdin.
//!
//! The relay keeps the packets since the last keyframe, so a new consumer
//! starts with a complete picture immediately.

use crate::config::{mask_url, redact};
use crate::ffmpeg::{self, StreamInfo};
use bytes::{Bytes, BytesMut};
use serde::Serialize;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio::sync::{Notify, mpsc, watch};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

const TS: usize = 188;
/// Data queued for a slow consumer before it is dropped (it then restarts).
const QUEUE_CHUNKS: usize = 1024;
/// Largest keyframe group kept for new consumers.
const MAX_GOP_BYTES: usize = 24 * 1024 * 1024;
/// Disconnect from the camera after nobody has needed the stream this long.
const IDLE_DISCONNECT: Duration = Duration::from_secs(20);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RelayState {
    Idle,
    Connecting,
    Connected,
    Retrying,
}

#[derive(Debug, Clone, Serialize)]
pub struct RelayStatus {
    pub state: RelayState,
    pub consumers: usize,
    pub error: Option<String>,
    pub connects: u32,
    /// Seconds until the next connection attempt while retrying.
    pub retry_in: Option<u64>,
}

struct Shared {
    subs: Vec<mpsc::Sender<Bytes>>,
    /// PAT + PMT packets (needed first by any MPEG-TS reader).
    headers: Vec<u8>,
    /// Packets since the last video keyframe.
    gop: Vec<u8>,
    gop_ok: bool,
    connected: bool,
    last_used: Instant,
    status: RelayStatus,
}

pub struct Relay {
    pub name: String,
    pub label: &'static str,
    url: String,
    transport: String,
    ffmpeg: String,
    shared: Mutex<Shared>,
    wake: Notify,
    info: watch::Sender<Option<StreamInfo>>,
    /// The camera's sound track as ffmpeg described it at the last connect:
    /// `None` before the first connect, `Some("")` when it sends no sound.
    audio: Mutex<Option<String>>,
    cancel: CancellationToken,
    /// Picture rotation for this camera, degrees clockwise (0, 90, 180, 270).
    pub rotate: u16,
}

impl Relay {
    pub fn new(
        name: &str,
        label: &'static str,
        url: &str,
        transport: &str,
        ffmpeg: &str,
        cancel: CancellationToken,
        rotate: u16,
    ) -> Arc<Self> {
        let r = Arc::new(Self {
            rotate,
            name: name.into(),
            label,
            url: url.into(),
            transport: transport.into(),
            ffmpeg: ffmpeg.into(),
            shared: Mutex::new(Shared {
                subs: Vec::new(),
                headers: Vec::new(),
                gop: Vec::new(),
                gop_ok: false,
                connected: false,
                last_used: Instant::now(),
                status: RelayStatus {
                    state: RelayState::Idle,
                    consumers: 0,
                    error: None,
                    connects: 0,
                    retry_in: None,
                },
            }),
            wake: Notify::new(),
            info: watch::Sender::new(None),
            audio: Mutex::new(None),
            cancel,
        });
        tokio::spawn(r.clone().run());
        r
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn status(&self) -> RelayStatus {
        let s = self.shared.lock().unwrap();
        let mut st = s.status.clone();
        st.consumers = s.subs.iter().filter(|x| !x.is_closed()).count();
        st
    }

    /// Codec, size and frame rate, once connected.
    /// The sound the camera sends on this stream ("pcm_mulaw, 8000 Hz"),
    /// `Some("")` when it sends none, `None` when not connected yet.
    pub fn audio(&self) -> Option<String> {
        self.audio.lock().unwrap().clone()
    }

    pub fn info(&self) -> Option<StreamInfo> {
        self.info.borrow().clone()
    }

    /// Wait (up to `timeout`) for the stream details.
    pub async fn wait_info(&self, timeout: Duration) -> Option<StreamInfo> {
        let mut rx = self.info.subscribe();
        let r = tokio::time::timeout(timeout, rx.wait_for(|i| i.is_some())).await;
        r.ok().and_then(|x| x.ok()).and_then(|x| x.clone())
    }

    /// Receive the stream as MPEG-TS. Starts the camera connection if needed.
    /// The receiver ends when the camera connection drops.
    pub fn subscribe(&self) -> mpsc::Receiver<Bytes> {
        self.subscribe_with(true)
    }

    /// Like `subscribe`, but from this moment on: without the last few
    /// seconds kept for a quick picture (live sound would start that far
    /// behind and stay there).
    pub fn subscribe_now(&self) -> mpsc::Receiver<Bytes> {
        self.subscribe_with(false)
    }

    fn subscribe_with(&self, gop: bool) -> mpsc::Receiver<Bytes> {
        let (tx, rx) = mpsc::channel(QUEUE_CHUNKS);
        let mut s = self.shared.lock().unwrap();
        s.last_used = Instant::now();
        if s.connected && !s.headers.is_empty() {
            let mut start = s.headers.clone();
            if gop && s.gop_ok {
                start.extend_from_slice(&s.gop);
            }
            let _ = tx.try_send(Bytes::from(start));
        }
        s.subs.push(tx);
        drop(s);
        self.wake.notify_one();
        rx
    }

    async fn run(self: Arc<Self>) {
        let mut backoff = Duration::from_secs(2);
        let mut last_fail: Option<Instant> = None;
        let tag = format!("[{}] {}", self.name, self.label);
        loop {
            // Wait for someone to need the stream.
            loop {
                let wanted = {
                    let mut s = self.shared.lock().unwrap();
                    s.subs.retain(|x| !x.is_closed());
                    !s.subs.is_empty()
                };
                if wanted {
                    break;
                }
                self.set_state(RelayState::Idle, None);
                tokio::select! {
                    _ = self.wake.notified() => {}
                    _ = self.cancel.cancelled() => return,
                }
            }
            // Respect the back-off after a failure, so a camera that refuses
            // connections is not hammered.
            if let Some(t) = last_fail {
                let wait = backoff.saturating_sub(t.elapsed());
                if !wait.is_zero() {
                    self.shared.lock().unwrap().status.retry_in = Some(wait.as_secs() + 1);
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => {}
                        _ = self.cancel.cancelled() => return,
                    }
                }
            }
            self.shared.lock().unwrap().status.retry_in = None;
            self.set_state(RelayState::Connecting, None);
            info!("{tag}: connecting to {}", mask_url(&self.url));
            let started = Instant::now();
            let (got_data, err) = self.clone().session().await;
            // Ending the connection ends every consumer; they restart and
            // re-subscribe (keeps their timestamps consistent).
            {
                let mut s = self.shared.lock().unwrap();
                s.subs.clear();
                s.connected = false;
                s.headers.clear();
                s.gop.clear();
                s.gop_ok = false;
            }
            if self.cancel.is_cancelled() {
                return;
            }
            match err {
                None => {
                    info!("{tag}: disconnected (not needed)");
                    last_fail = None;
                    backoff = Duration::from_secs(2);
                }
                Some(e) => {
                    if got_data && started.elapsed() > Duration::from_secs(60) {
                        backoff = Duration::from_secs(2);
                    } else if last_fail.is_some() {
                        backoff = (backoff * 2).min(MAX_BACKOFF);
                    }
                    warn!("{tag}: {e}; retrying in {}s", backoff.as_secs());
                    let mut s = self.shared.lock().unwrap();
                    s.status.state = RelayState::Retrying;
                    s.status.error = Some(e);
                    last_fail = Some(Instant::now());
                }
            }
        }
    }

    fn set_state(&self, state: RelayState, error: Option<String>) {
        let mut s = self.shared.lock().unwrap();
        s.status.state = state;
        if error.is_some() || state == RelayState::Connected {
            s.status.error = error;
        }
    }

    /// One camera connection. Returns (received data, error if it failed).
    async fn session(self: Arc<Self>) -> (bool, Option<String>) {
        let mut cmd = crate::load::cmd(&self.ffmpeg);
        cmd.args([
            "-hide_banner",
            "-nostdin",
            "-nostats",
            "-loglevel",
            "level+info",
            "-fflags",
            "+genpts",
        ])
        .args(ffmpeg::input_args(&self.url, &self.transport))
        .args([
            "-map", "0:v:0", "-map", "0:a:0?", "-c:v", "copy", "-c:a", "aac", "-b:a", "64k", "-ar",
            "16000",
        ])
        .args([
            "-f",
            "mpegts",
            "-mpegts_flags",
            "+resend_headers",
            "-muxdelay",
            "0",
            "-muxpreload",
            "0",
            "pipe:1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return (false, Some(format!("cannot start ffmpeg: {e}"))),
        };
        let mut stdout = child.stdout.take().unwrap();
        let first_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(self.clone().read_stderr(stderr, first_error.clone()));
        }

        let mut parser = TsParser::default();
        let mut buf = BytesMut::with_capacity(256 * 1024);
        let mut chunk = vec![0u8; 64 * 1024];
        let mut got = false;
        let mut idle_check = tokio::time::interval(Duration::from_secs(2));
        let result: Option<String> = loop {
            tokio::select! {
                r = tokio::time::timeout(Duration::from_secs(20), stdout.read(&mut chunk)) => {
                    let n = match r {
                        Err(_) => break Some("no data from camera for 20 s".into()),
                        Ok(Err(e)) => break Some(e.to_string()),
                        Ok(Ok(0)) => break Some(first_error.lock().unwrap().clone().unwrap_or_else(|| "camera closed the stream".into())),
                        Ok(Ok(n)) => n,
                    };
                    buf.extend_from_slice(&chunk[..n]);
                    // Resynchronise if needed, then forward whole packets.
                    while !buf.is_empty() && buf[0] != 0x47 {
                        let _ = buf.split_to(1);
                    }
                    let whole = buf.len() / TS * TS;
                    if whole == 0 {
                        continue;
                    }
                    let data = buf.split_to(whole).freeze();
                    if !got {
                        got = true;
                        let mut s = self.shared.lock().unwrap();
                        s.connected = true;
                        s.status.connects += 1;
                        drop(s);
                        self.set_state(RelayState::Connected, None);
                        info!("[{}] {}: connected", self.name, self.label);
                    }
                    self.forward(&mut parser, data);
                }
                _ = idle_check.tick() => {
                    let mut s = self.shared.lock().unwrap();
                    s.subs.retain(|x| !x.is_closed());
                    if !s.subs.is_empty() {
                        s.last_used = Instant::now();
                    } else if s.last_used.elapsed() > IDLE_DISCONNECT {
                        break None;
                    }
                }
                _ = self.cancel.cancelled() => break None,
            }
        };
        let _ = child.kill().await;
        (got, result)
    }

    fn forward(&self, parser: &mut TsParser, data: Bytes) {
        let mut s = self.shared.lock().unwrap();
        for pkt in data.chunks_exact(TS) {
            match parser.packet(pkt) {
                Kind::Header => {
                    // Keep the latest PAT/PMT.
                    if parser.pat_and_pmt_known() {
                        s.headers = parser.headers();
                    }
                }
                Kind::Keyframe => {
                    s.gop.clear();
                    s.gop_ok = true;
                }
                Kind::Other => {}
            }
            if s.gop_ok {
                if s.gop.len() + TS > MAX_GOP_BYTES {
                    s.gop.clear();
                    s.gop_ok = false;
                } else {
                    s.gop.extend_from_slice(pkt);
                }
            }
        }
        // Slow consumers are dropped (they restart) rather than delaying everyone.
        s.subs.retain(|tx| tx.try_send(data.clone()).is_ok());
    }

    async fn read_stderr(
        self: Arc<Self>,
        stderr: tokio::process::ChildStderr,
        first_error: Arc<Mutex<Option<String>>>,
    ) {
        let mut lines = BufReader::new(stderr).lines();
        let mut filter = ffmpeg::StderrFilter::default();
        let mut in_input = true;
        let mut audio = String::new();
        while let Ok(Some(line)) = lines.next_line().await {
            // "-loglevel level+info" prefixes each line with its level.
            let (level, msg) = match line
                .trim_start()
                .strip_prefix('[')
                .and_then(|r| r.split_once("] "))
            {
                Some((l, m))
                    if ["info", "warning", "error", "fatal", "panic", "verbose"].contains(&l) =>
                {
                    (l, m)
                }
                _ => ("info", line.as_str()),
            };
            if level == "info" || level == "verbose" {
                if msg.contains("Output #0") {
                    if in_input {
                        if audio.is_empty() {
                            info!("[{}] {}: the camera sends no sound", self.name, self.label);
                        }
                        *self.audio.lock().unwrap() = Some(audio.clone());
                    }
                    in_input = false;
                } else if in_input
                    && audio.is_empty()
                    && let Some(a) = parse_audio_line(msg)
                {
                    info!("[{}] {}: sound {a}", self.name, self.label);
                    audio = a;
                } else if in_input
                    && self.info.borrow().is_none()
                    && let Some(i) = parse_stream_line(msg)
                {
                    info!(
                        "[{}] {}: {} {}×{} {} fps",
                        self.name,
                        self.label,
                        i.codec,
                        i.width.unwrap_or(0),
                        i.height.unwrap_or(0),
                        i.fps.unwrap_or(0.0)
                    );
                    self.info.send_replace(Some(i));
                }
                continue;
            }
            if let Some(m) = filter.filter(msg) {
                let m = redact(&m, &self.url);
                warn!("[{}] {} ffmpeg: {m}", self.name, self.label);
                first_error.lock().unwrap().get_or_insert(m);
            }
            if let Some(sm) = filter.summary(false) {
                info!("[{}] {}: {sm}", self.name, self.label);
            }
        }
    }
}

/// Parse ffmpeg's "Stream #0:0: Video: hevc (Main), yuvj420p(pc), 2560x1440, 15 fps, ..." line.
/// "Stream #0:1: Audio: pcm_mulaw, 8000 Hz, mono, s16, 64 kb/s" →
/// "pcm_mulaw, 8000 Hz".
pub fn parse_audio_line(line: &str) -> Option<String> {
    let rest = line.trim().strip_prefix("Stream #")?;
    let a = rest.split_once("Audio: ")?.1;
    let mut parts = a.split(',').map(str::trim);
    let codec = parts.next()?.split(' ').next()?.to_string();
    Some(match parts.next() {
        Some(hz) if hz.ends_with("Hz") => format!("{codec}, {hz}"),
        _ => codec,
    })
}

pub fn parse_stream_line(line: &str) -> Option<StreamInfo> {
    let rest = line.trim().strip_prefix("Stream #")?;
    let v = rest.split_once("Video: ")?.1;
    let codec = v.split([' ', ',']).next()?.to_string();
    let size = v.split([',', ' ']).find_map(|t| {
        let (w, h) = t.split_once('x')?;
        Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?))
    });
    let fps = v
        .split(',')
        .find_map(|t| t.trim().strip_suffix(" fps")?.trim().parse::<f64>().ok());
    Some(StreamInfo {
        kind: "video".into(),
        codec,
        width: size.map(|s| s.0),
        height: size.map(|s| s.1),
        fps,
    })
}

// ---------- minimal MPEG-TS parsing ----------

#[derive(Default)]
struct TsParser {
    pmt_pid: Option<u16>,
    video_pid: Option<u16>,
    pat: Vec<u8>,
    pmt: Vec<u8>,
}

enum Kind {
    Header,
    Keyframe,
    Other,
}

impl TsParser {
    fn pat_and_pmt_known(&self) -> bool {
        !self.pat.is_empty() && !self.pmt.is_empty()
    }

    fn headers(&self) -> Vec<u8> {
        [self.pat.as_slice(), self.pmt.as_slice()].concat()
    }

    fn packet(&mut self, p: &[u8]) -> Kind {
        let pid = (((p[1] & 0x1f) as u16) << 8) | p[2] as u16;
        let pusi = p[1] & 0x40 != 0;
        let afc = (p[3] >> 4) & 3;
        let mut payload = 4;
        let mut random_access = false;
        if afc & 2 != 0 {
            let af_len = p[4] as usize;
            if af_len > 0 && 5 < TS {
                random_access = p[5] & 0x40 != 0;
            }
            payload = 5 + af_len;
        }
        if afc & 1 == 0 || payload >= TS {
            return Kind::Other;
        }
        let body = &p[payload..];
        if pid == 0 && pusi {
            self.pat = p.to_vec();
            if let Some(pmt) = parse_pat(body) {
                self.pmt_pid = Some(pmt);
            }
            return Kind::Header;
        }
        if Some(pid) == self.pmt_pid && pusi {
            self.pmt = p.to_vec();
            if let Some(v) = parse_pmt(body) {
                self.video_pid = Some(v);
            }
            return Kind::Header;
        }
        if Some(pid) == self.video_pid && pusi && random_access {
            return Kind::Keyframe;
        }
        Kind::Other
    }
}

/// PMT PID of the first program.
fn parse_pat(body: &[u8]) -> Option<u16> {
    let ptr = *body.first()? as usize;
    let s = body.get(1 + ptr..)?;
    let len = (((*s.get(1)? as usize) & 0x0f) << 8) | *s.get(2)? as usize;
    let end = (3 + len).saturating_sub(4).min(s.len());
    let mut i = 8;
    while i + 4 <= end {
        let program = ((s[i] as u16) << 8) | s[i + 1] as u16;
        let pid = (((s[i + 2] & 0x1f) as u16) << 8) | s[i + 3] as u16;
        if program != 0 {
            return Some(pid);
        }
        i += 4;
    }
    None
}

/// PID of the first video stream (H.264, H.265, MPEG-4, MJPEG …).
fn parse_pmt(body: &[u8]) -> Option<u16> {
    let ptr = *body.first()? as usize;
    let s = body.get(1 + ptr..)?;
    let len = (((*s.get(1)? as usize) & 0x0f) << 8) | *s.get(2)? as usize;
    let end = (3 + len).saturating_sub(4).min(s.len());
    let info_len = (((*s.get(10)? as usize) & 0x0f) << 8) | *s.get(11)? as usize;
    let mut i = 12 + info_len;
    while i + 5 <= end {
        let stream_type = s[i];
        let pid = (((s[i + 1] & 0x1f) as u16) << 8) | s[i + 2] as u16;
        let es_len = (((s[i + 3] as usize) & 0x0f) << 8) | s[i + 4] as usize;
        if matches!(stream_type, 0x01 | 0x02 | 0x10 | 0x1b | 0x24 | 0x42 | 0xd1) {
            return Some(pid);
        }
        i += 5 + es_len;
    }
    None
}

// ---------- consumers ----------

impl Relay {
    /// Input arguments for a consumer of this relay: the stream from stdin,
    /// with the camera's rotation. Decoding consumers get upright pictures
    /// (ffmpeg rotates them); copying recorders store a rotation flag that
    /// players and browsers follow, without re-encoding.
    pub fn input_args(&self) -> Vec<String> {
        let mut a = Vec::new();
        if !self.rotate.is_multiple_of(360) {
            // ffmpeg counts anticlockwise.
            a.extend([
                "-display_rotation".to_string(),
                ((360 - self.rotate % 360) % 360).to_string(),
            ]);
        }
        a.extend(pipe_input_args());
        a
    }

    /// Picture size after rotation.
    pub fn rotated(&self, w: u32, h: u32) -> (u32, u32) {
        if self.rotate % 180 == 90 {
            (h, w)
        } else {
            (w, h)
        }
    }
}

/// Input arguments for an ffmpeg process that reads a relay from stdin.
/// The relay starts each consumer with the stream headers and a keyframe,
/// so a short analysis is enough (ffmpeg's default waits 5 s).
pub fn pipe_input_args() -> Vec<String> {
    [
        "-analyzeduration",
        "1500000",
        "-probesize",
        "4000000",
        "-f",
        "mpegts",
        "-i",
        "pipe:0",
    ]
    .map(String::from)
    .to_vec()
}

/// ffmpeg input from a relay pipe for live sound: as little buffering and
/// probing as possible.
pub fn pipe_input_args_low_delay() -> Vec<String> {
    [
        "-fflags",
        "nobuffer",
        "-flags",
        "low_delay",
        "-analyzeduration",
        "300000",
        "-probesize",
        "200000",
        "-f",
        "mpegts",
        "-i",
        "pipe:0",
    ]
    .map(String::from)
    .to_vec()
}

/// Feed a relay into a child ffmpeg's stdin until either side stops.
/// Closing stdin (when the relay ends or `stop` fires) makes ffmpeg finish
/// its output files cleanly.
pub fn feed(
    mut rx: mpsc::Receiver<Bytes>,
    mut stdin: ChildStdin,
    stop: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                d = rx.recv() => {
                    let Some(d) = d else { break };
                    if stdin.write_all(&d).await.is_err() {
                        break;
                    }
                }
                _ = stop.cancelled() => break,
            }
        }
        let _ = stdin.shutdown().await;
    })
}

/// Grab one RGB frame (scaled to `w`×`h`) from a relay.
pub async fn grab_rgb(relay: &Relay, ffmpeg: &str, w: u32, h: u32) -> anyhow::Result<Vec<u8>> {
    let mut child: Child = crate::load::cmd(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error"])
        .args(relay.input_args())
        .args([
            "-frames:v",
            "1",
            "-vf",
            &format!("scale={w}:{h}"),
            "-pix_fmt",
            "rgb24",
            "-f",
            "rawvideo",
            "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let stop = CancellationToken::new();
    let _feed = feed(relay.subscribe(), child.stdin.take().unwrap(), stop.clone());
    let mut out = Vec::new();
    let mut stdout = child.stdout.take().unwrap();
    let r = tokio::time::timeout(Duration::from_secs(25), stdout.read_to_end(&mut out)).await;
    stop.cancel();
    let _ = child.kill().await;
    r.map_err(|_| anyhow::anyhow!("no picture from the camera (timed out)"))??;
    if out.len() < (w * h * 3) as usize {
        anyhow::bail!("no picture from the camera");
    }
    out.truncate((w * h * 3) as usize);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stream_lines() {
        assert_eq!(
            parse_audio_line("  Stream #0:1: Audio: pcm_mulaw, 8000 Hz, mono, s16, 64 kb/s")
                .as_deref(),
            Some("pcm_mulaw, 8000 Hz")
        );
        assert_eq!(
            parse_audio_line(
                "Stream #0:1(und): Audio: aac (LC) (mp4a / 0x6134706D), 16000 Hz, mono"
            )
            .as_deref(),
            Some("aac, 16000 Hz")
        );
        assert!(parse_audio_line("Stream #0:0: Video: h264").is_none());
        let i = parse_stream_line("  Stream #0:0: Video: hevc (Main), yuvj420p(pc, bt709), 2560x1440, 15 fps, 15 tbr, 90k tbn").unwrap();
        assert_eq!(i.codec, "hevc");
        assert_eq!((i.width, i.height), (Some(2560), Some(1440)));
        assert_eq!(i.fps, Some(15.0));
        let i = parse_stream_line("Stream #0:0[0x1](und): Video: h264 (High) (avc1 / 0x31637661), yuv420p(progressive), 1280x720 [SAR 1:1 DAR 16:9], 1323 kb/s, 15 fps, 15 tbr").unwrap();
        assert_eq!(i.codec, "h264");
        assert_eq!(i.width, Some(1280));
        assert!(parse_stream_line("Stream #0:1: Audio: aac").is_none());
    }
}
