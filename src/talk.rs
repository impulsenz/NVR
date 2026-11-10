//! Two-way audio: talk through a camera's speaker.
//!
//! Uses the ONVIF audio backchannel (ONVIF Streaming spec, also supported by
//! many Dahua, Hikvision, Amcrest, Axis… cameras): an RTSP DESCRIBE with
//! `Require: www.onvif.org/ver20/backchannel` lists an extra audio track
//! marked `sendonly`; after SETUP and PLAY the client sends RTP audio on it.
//!
//! The browser sends 16-bit mono PCM at 8 kHz over a WebSocket; it is
//! encoded to G.711 (μ-law or A-law, whichever the camera asks for) and sent
//! in 20 ms RTP packets, interleaved on the RTSP connection.

use anyhow::{Context, Result, anyhow, bail};
use md5::{Digest, Md5};
use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

const BACKCHANNEL: &str = "www.onvif.org/ver20/backchannel";
/// Samples per RTP packet (20 ms at 8 kHz).
const PACKET: usize = 160;

/// Cameras someone is talking through right now (one at a time each).
static BUSY: Mutex<Option<HashSet<String>>> = Mutex::new(None);

struct Target {
    host: String,
    port: u16,
    user: String,
    pass: String,
    /// The address without user name and password.
    url: String,
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) =
                u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("x"), 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_url(url: &str) -> Result<Target> {
    let rest = url
        .strip_prefix("rtsp://")
        .ok_or_else(|| anyhow!("two-way audio needs the camera's rtsp:// address"))?;
    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let (cred, hostport) = match authority.rfind('@') {
        Some(i) => (&authority[..i], &authority[i + 1..]),
        None => ("", authority),
    };
    let (user, pass) = cred.split_once(':').unwrap_or((cred, ""));
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(']') || h.starts_with('[') => {
            (h.to_string(), p.parse().unwrap_or(554))
        }
        _ => (hostport.to_string(), 554),
    };
    Ok(Target {
        host: host.clone(),
        port,
        user: pct_decode(user),
        pass: pct_decode(pass),
        url: format!(
            "rtsp://{hostport}{}",
            if path.is_empty() { "/" } else { path }
        ),
    })
}

struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Response {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    fn headers<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.headers
            .iter()
            .filter(move |(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

fn md5_hex(s: &str) -> String {
    Md5::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn param(h: &str, name: &str) -> Option<String> {
    let i = h.to_ascii_lowercase().find(&format!("{name}="))?;
    let v = &h[i + name.len() + 1..];
    Some(match v.strip_prefix('"') {
        Some(q) => q[..q.find('"').unwrap_or(q.len())].to_string(),
        None => v[..v.find(',').unwrap_or(v.len())].trim().to_string(),
    })
}

enum Auth {
    None,
    Basic,
    Digest {
        realm: String,
        nonce: String,
        qop: bool,
        opaque: Option<String>,
    },
}

struct Rtsp {
    rd: BufReader<OwnedReadHalf>,
    wr: OwnedWriteHalf,
    t: Target,
    cseq: u32,
    nc: u32,
    auth: Auth,
    session: Option<String>,
}

impl Rtsp {
    async fn connect(t: Target) -> Result<Self> {
        let s = tokio::time::timeout(
            Duration::from_secs(6),
            TcpStream::connect((t.host.as_str(), t.port)),
        )
        .await
        .map_err(|_| anyhow!("the camera did not answer"))?
        .with_context(|| format!("can't reach the camera at {}:{}", t.host, t.port))?;
        s.set_nodelay(true).ok();
        let (rd, wr) = s.into_split();
        Ok(Self {
            rd: BufReader::new(rd),
            wr,
            t,
            cseq: 0,
            nc: 0,
            auth: Auth::None,
            session: None,
        })
    }

    fn authorization(&mut self, method: &str, uri: &str) -> Option<String> {
        use base64::Engine;
        match &self.auth {
            Auth::None => None,
            Auth::Basic => Some(format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD
                    .encode(format!("{}:{}", self.t.user, self.t.pass))
            )),
            Auth::Digest {
                realm,
                nonce,
                qop,
                opaque,
            } => {
                let ha1 = md5_hex(&format!("{}:{realm}:{}", self.t.user, self.t.pass));
                let ha2 = md5_hex(&format!("{method}:{uri}"));
                let mut h = format!(
                    r#"Digest username="{}", realm="{realm}", nonce="{nonce}", uri="{uri}""#,
                    self.t.user
                );
                if *qop {
                    self.nc += 1;
                    let cnonce = format!(
                        "{:016x}",
                        rand_core::RngCore::next_u64(&mut rand_core::OsRng)
                    );
                    let nc = format!("{:08x}", self.nc);
                    let r = md5_hex(&format!("{ha1}:{nonce}:{nc}:{cnonce}:auth:{ha2}"));
                    h += &format!(r#", response="{r}", qop=auth, nc={nc}, cnonce="{cnonce}""#);
                } else {
                    h += &format!(
                        r#", response="{}""#,
                        md5_hex(&format!("{ha1}:{nonce}:{ha2}"))
                    );
                }
                if let Some(o) = opaque {
                    h += &format!(r#", opaque="{o}""#);
                }
                Some(h)
            }
        }
    }

    async fn send(&mut self, method: &str, uri: &str, extra: &[(&str, &str)]) -> Result<()> {
        self.cseq += 1;
        let mut req = format!(
            "{method} {uri} RTSP/1.0\r\nCSeq: {}\r\nUser-Agent: GAZE\r\nRequire: {BACKCHANNEL}\r\n",
            self.cseq
        );
        if let Some(a) = self.authorization(method, uri) {
            req += &format!("Authorization: {a}\r\n");
        }
        if let Some(s) = &self.session {
            req += &format!("Session: {s}\r\n");
        }
        for (k, v) in extra {
            req += &format!("{k}: {v}\r\n");
        }
        req += "\r\n";
        self.wr.write_all(req.as_bytes()).await?;
        Ok(())
    }

    /// The next RTSP response (skipping any interleaved data).
    async fn read_response(&mut self) -> Result<Response> {
        loop {
            let first = self.rd.fill_buf().await?;
            if first.is_empty() {
                bail!("the camera closed the connection");
            }
            if first[0] == b'$' {
                let mut h = [0u8; 4];
                self.rd.read_exact(&mut h).await?;
                let mut skip = vec![0u8; u16::from_be_bytes([h[2], h[3]]) as usize];
                self.rd.read_exact(&mut skip).await?;
                continue;
            }
            let mut line = String::new();
            self.rd.read_line(&mut line).await?;
            if !line.starts_with("RTSP/") {
                continue; // a request from the camera, or noise
            }
            let status = line
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let mut headers = Vec::new();
            loop {
                let mut l = String::new();
                if self.rd.read_line(&mut l).await? == 0 {
                    bail!("the camera closed the connection");
                }
                let l = l.trim_end();
                if l.is_empty() {
                    break;
                }
                if let Some((k, v)) = l.split_once(':') {
                    headers.push((k.trim().to_string(), v.trim().to_string()));
                }
            }
            let len: usize = headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, v)| v.parse().ok())
                .unwrap_or(0);
            let mut body = vec![0u8; len];
            self.rd.read_exact(&mut body).await?;
            return Ok(Response {
                status,
                headers,
                body: String::from_utf8_lossy(&body).into_owned(),
            });
        }
    }

    /// Send a request and wait for its answer, signing in if asked to.
    async fn request(
        &mut self,
        method: &str,
        uri: &str,
        extra: &[(&str, &str)],
    ) -> Result<Response> {
        for attempt in 0..2 {
            self.send(method, uri, extra).await?;
            let r = tokio::time::timeout(Duration::from_secs(8), self.read_response())
                .await
                .map_err(|_| anyhow!("the camera did not answer {method}"))??;
            if r.status == 401 && attempt == 0 {
                if self.t.user.is_empty() {
                    bail!("the camera wants a user name and password (add them to its address)");
                }
                let chal: Vec<&str> = r.headers("www-authenticate").collect();
                self.auth = if let Some(d) = chal
                    .iter()
                    .find(|h| h.to_ascii_lowercase().starts_with("digest"))
                {
                    Auth::Digest {
                        realm: param(d, "realm").unwrap_or_default(),
                        nonce: param(d, "nonce").unwrap_or_default(),
                        qop: param(d, "qop")
                            .is_some_and(|q| q.split(',').any(|x| x.trim() == "auth")),
                        opaque: param(d, "opaque"),
                    }
                } else {
                    Auth::Basic
                };
                continue;
            }
            if r.status == 401 {
                bail!("the camera refused the user name or password");
            }
            return Ok(r);
        }
        unreachable!()
    }
}

/// The camera's speaker track, from the DESCRIBE answer.
#[derive(Debug, PartialEq)]
struct Speaker {
    control: String,
    payload: u8,
    /// "PCMU" or "PCMA".
    codec: String,
}

fn find_speaker(sdp: &str, base: &str) -> Result<Speaker> {
    let mut other = None;
    for media in sdp.split("\nm=").skip(1) {
        let lines: Vec<&str> = media.lines().map(str::trim).collect();
        if !lines[0].starts_with("audio") || !lines.iter().any(|l| *l == "a=sendonly") {
            continue;
        }
        let pts: Vec<u8> = lines[0]
            .split_whitespace()
            .skip(3)
            .filter_map(|p| p.parse().ok())
            .collect();
        let name_of = |pt: u8| {
            lines
                .iter()
                .find_map(|l| l.strip_prefix(&format!("a=rtpmap:{pt} ")))
                .map(|r| r.split('/').next().unwrap_or("").to_ascii_uppercase())
                .unwrap_or_else(|| match pt {
                    0 => "PCMU".into(),
                    8 => "PCMA".into(),
                    _ => format!("type {pt}"),
                })
        };
        let control = lines
            .iter()
            .find_map(|l| l.strip_prefix("a=control:"))
            .unwrap_or("*");
        let control = if control.starts_with("rtsp://") {
            control.to_string()
        } else if control == "*" {
            base.to_string()
        } else {
            format!(
                "{}/{}",
                base.trim_end_matches('/'),
                control.trim_start_matches('/')
            )
        };
        for &pt in &pts {
            let codec = name_of(pt);
            if codec == "PCMU" || codec == "PCMA" {
                return Ok(Speaker {
                    control,
                    payload: pt,
                    codec,
                });
            }
        }
        other = pts.first().map(|&pt| name_of(pt));
    }
    match other {
        Some(c) => {
            bail!("the camera's speaker wants {c} audio, which GAZE can't send yet (G.711 only)")
        }
        None => bail!("this camera doesn't offer two-way audio (no ONVIF audio backchannel)"),
    }
}

/// G.711 μ-law.
fn ulaw(s: i16) -> u8 {
    const BIAS: i32 = 0x84;
    let mut v = s as i32;
    let sign = if v < 0 {
        v = -v;
        0x80
    } else {
        0
    };
    v = (v.min(32635)) + BIAS;
    let exp = (7 - (v as u32).leading_zeros().saturating_sub(17).min(7)) as i32;
    let mant = (v >> (exp + 3)) & 0x0f;
    !(sign | (exp << 4) as u8 | mant as u8)
}

/// G.711 A-law.
fn alaw(s: i16) -> u8 {
    let mut v = s as i32;
    let sign = if v >= 0 { 0xd5u8 } else { 0x55 };
    if v < 0 {
        v = -v - 1;
    }
    v = v.min(32767);
    let out = if v < 256 {
        (v >> 4) as u8
    } else {
        let exp = 31 - (v as u32).leading_zeros() as i32 - 7; // 1..7
        ((exp << 4) | ((v >> (exp + 3)) & 0x0f)) as u8
    };
    out ^ sign
}

/// An open speaker: send audio with `send`, close with `stop`.
pub struct Talk {
    rtsp: Rtsp,
    channel: u8,
    speaker: Speaker,
    seq: u16,
    ts: u32,
    ssrc: u32,
    pending: Vec<i16>,
    camera: String,
    last_keepalive: std::time::Instant,
    uri: String,
}

impl Talk {
    pub fn codec(&self) -> &str {
        &self.speaker.codec
    }

    pub async fn open(camera: &str, url: &str) -> Result<Talk> {
        {
            let mut busy = BUSY.lock().unwrap();
            let busy = busy.get_or_insert_with(HashSet::new);
            if !busy.insert(camera.to_string()) {
                bail!("someone else is already talking through this camera");
            }
        }
        match Self::open_inner(camera, url).await {
            Ok(t) => Ok(t),
            Err(e) => {
                release(camera);
                Err(e)
            }
        }
    }

    async fn open_inner(camera: &str, url: &str) -> Result<Talk> {
        let t = parse_url(url)?;
        let uri = t.url.clone();
        let mut rtsp = Rtsp::connect(t).await?;
        let r = rtsp
            .request("DESCRIBE", &uri, &[("Accept", "application/sdp")])
            .await?;
        if r.status == 551 {
            bail!("this camera doesn't offer two-way audio (no ONVIF audio backchannel)");
        }
        if r.status != 200 {
            bail!("the camera answered {} to DESCRIBE", r.status);
        }
        let base = r
            .header("content-base")
            .or(r.header("content-location"))
            .unwrap_or(&uri)
            .to_string();
        let speaker = find_speaker(&r.body, &base)?;
        let r = rtsp
            .request(
                "SETUP",
                &speaker.control,
                &[("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1")],
            )
            .await?;
        if r.status != 200 {
            bail!(
                "the camera answered {} when setting up its speaker",
                r.status
            );
        }
        let channel = r
            .header("transport")
            .and_then(|t| param(t, "interleaved"))
            .and_then(|i| i.split('-').next().and_then(|c| c.parse().ok()))
            .unwrap_or(0);
        rtsp.session = r
            .header("session")
            .map(|s| s.split(';').next().unwrap_or("").trim().to_string());
        let r = rtsp
            .request("PLAY", &base, &[("Range", "npt=0.000-")])
            .await?;
        if r.status != 200 {
            bail!("the camera answered {} to PLAY", r.status);
        }
        Ok(Talk {
            rtsp,
            channel,
            speaker,
            seq: rand_core::RngCore::next_u32(&mut rand_core::OsRng) as u16,
            ts: rand_core::RngCore::next_u32(&mut rand_core::OsRng),
            ssrc: rand_core::RngCore::next_u32(&mut rand_core::OsRng),
            pending: Vec::new(),
            camera: camera.to_string(),
            last_keepalive: std::time::Instant::now(),
            uri: base,
        })
    }

    /// Send 8 kHz mono samples to the camera's speaker.
    pub async fn send(&mut self, samples: &[i16]) -> Result<()> {
        self.pending.extend_from_slice(samples);
        let n = self.pending.len() / PACKET * PACKET;
        let mut out = Vec::with_capacity(n / PACKET * (4 + 12 + PACKET));
        for chunk in self.pending[..n].chunks(PACKET) {
            let len = (12 + PACKET) as u16;
            out.extend_from_slice(&[b'$', self.channel]);
            out.extend_from_slice(&len.to_be_bytes());
            out.extend_from_slice(&[0x80, self.speaker.payload & 0x7f]);
            out.extend_from_slice(&self.seq.to_be_bytes());
            out.extend_from_slice(&self.ts.to_be_bytes());
            out.extend_from_slice(&self.ssrc.to_be_bytes());
            let pcmu = self.speaker.codec == "PCMU";
            out.extend(chunk.iter().map(|&s| if pcmu { ulaw(s) } else { alaw(s) }));
            self.seq = self.seq.wrapping_add(1);
            self.ts = self.ts.wrapping_add(PACKET as u32);
        }
        self.pending.drain(..n);
        if !out.is_empty() {
            self.rtsp
                .wr
                .write_all(&out)
                .await
                .context("the camera closed the connection")?;
        }
        Ok(())
    }

    /// Call now and then: keeps the camera from closing the session, and
    /// reads anything it sent so its buffers don't fill up.
    pub async fn tick(&mut self) -> Result<()> {
        // Drain whatever arrived (answers, RTCP) without waiting.
        while let Ok(r) =
            tokio::time::timeout(Duration::from_millis(1), self.rtsp.rd.fill_buf()).await
        {
            let n = r?.len();
            if n == 0 {
                bail!("the camera closed the connection");
            }
            self.rtsp.rd.consume(n);
        }
        if self.last_keepalive.elapsed() > Duration::from_secs(25) {
            self.last_keepalive = std::time::Instant::now();
            let uri = self.uri.clone();
            self.rtsp.send("GET_PARAMETER", &uri, &[]).await?;
        }
        Ok(())
    }

    pub async fn stop(mut self) {
        let uri = self.uri.clone();
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            self.rtsp.send("TEARDOWN", &uri, &[]),
        )
        .await;
    }
}

fn release(camera: &str) {
    if let Some(b) = BUSY.lock().unwrap().as_mut() {
        b.remove(camera);
    }
}

impl Drop for Talk {
    fn drop(&mut self) {
        release(&self.camera);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn g711() {
        // Reference values from the ITU G.711 tables.
        assert_eq!(ulaw(0), 0xff);
        assert_eq!(ulaw(-1), 0x7f);
        assert_eq!(ulaw(32767), 0x80);
        assert_eq!(ulaw(-32768), 0x00);
        assert_eq!(alaw(0), 0xd5);
        assert_eq!(alaw(-1), 0x55);
        assert_eq!(alaw(32767), 0xaa);
        assert_eq!(alaw(-32768), 0x2a);
        assert_eq!(ulaw(1000), 0xce);
        assert_eq!(alaw(1000), 0xfa);
    }

    #[test]
    fn speaker_track() {
        let sdp = "v=0\r\no=- 0 0 IN IP4 0.0.0.0\r\ns=x\r\nm=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\na=control:trackID=0\r\na=recvonly\r\n\
                   m=audio 0 RTP/AVP 8\r\na=rtpmap:8 PCMA/8000\r\na=control:trackID=1\r\na=recvonly\r\n\
                   m=audio 0 RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\na=control:trackID=5\r\na=sendonly\r\n";
        let s = find_speaker(sdp, "rtsp://cam/stream/").unwrap();
        assert_eq!(
            s,
            Speaker {
                control: "rtsp://cam/stream/trackID=5".into(),
                payload: 0,
                codec: "PCMU".into()
            }
        );
        assert!(find_speaker(&sdp.replace("a=sendonly", "a=recvonly"), "rtsp://cam").is_err());
        let t = parse_url("rtsp://ad%40min:p%3Ass@192.168.1.5:8554/cam/main").unwrap();
        assert_eq!(
            (t.user.as_str(), t.pass.as_str(), t.host.as_str(), t.port),
            ("ad@min", "p:ss", "192.168.1.5", 8554)
        );
        assert_eq!(t.url, "rtsp://192.168.1.5:8554/cam/main");
        let d = r#"Digest realm="Login to 123", nonce="abc", qop="auth", opaque="o""#;
        assert_eq!(param(d, "nonce").as_deref(), Some("abc"));
        assert_eq!(param(d, "qop").as_deref(), Some("auth"));
    }
}
