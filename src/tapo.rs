//! Two-way audio on TP-Link Tapo (and VIGI) cameras, which have no ONVIF
//! audio backchannel. The Tapo app's own way in: an HTTP connection to port
//! 8800 (`POST /stream`, digest login as "admin" with a hash of the TP-Link
//! account password), then a never-ending multipart exchange. A JSON
//! request opens a "talk" session; the audio is then sent as parts of
//! MPEG-TS holding G.711 A-law (stream type 0x90), 20 ms at a time.
//!
//! Speaker address: `tapo://admin:TP-LINK-PASSWORD@192.168.1.50` (the
//! password of the TP-Link / Tapo app account the camera is in, not the
//! camera account made for RTSP). A password that is already the 32- or
//! 64-character hash is used as it is. `tapo://PASSWORD@ip` works too.

use anyhow::{Context, Result, anyhow, bail};
use md5::{Digest, Md5};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

const CLIENT_BOUNDARY: &str = "--client-stream-boundary--";
const DEVICE_BOUNDARY: &str = "--device-stream-boundary--";
const AUDIO_PID: u16 = 0x100;
const PMT_PID: u16 = 0x1000;
/// Tapo's own MPEG-TS stream type for G.711 A-law.
const STREAM_TYPE_PCMA_TAPO: u8 = 0x90;

pub fn is_tapo(url: &str) -> bool {
    url.trim().to_ascii_lowercase().starts_with("tapo://")
}

struct Addr {
    host: String,
    port: u16,
    /// Login tries, in order: (user, password).
    logins: Vec<(String, String)>,
    /// The account password, for the newer (SHA-256) login.
    cloud: Option<String>,
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("x"), 16)
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

fn hex_upper(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect()
}

fn md5_hex(s: &str) -> String {
    Md5::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn is_hash(s: &str) -> bool {
    (s.len() == 32 || s.len() == 64) && s.chars().all(|c| c.is_ascii_hexdigit())
}

fn parse(url: &str) -> Result<Addr> {
    let rest = url.trim();
    let rest = rest
        .get(7..)
        .filter(|_| is_tapo(rest))
        .ok_or_else(|| anyhow!("a Tapo speaker address starts with tapo://"))?;
    let authority = &rest[..rest.find(['/', '?']).unwrap_or(rest.len())];
    let (cred, hostport) = match authority.rfind('@') {
        Some(i) => (&authority[..i], &authority[i + 1..]),
        None => bail!("add the TP-Link account password: tapo://admin:PASSWORD@{authority}"),
    };
    let (user, pass) = cred.split_once(':').unwrap_or((cred, ""));
    let (user, pass) = (pct_decode(user), pct_decode(pass));
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() => (h.to_string(), p.parse().unwrap_or(8800)),
        _ => (hostport.to_string(), 8800),
    };
    // tapo://PASSWORD@ip: the account password in the user's place.
    let (user, pass) = if pass.is_empty() { ("admin".to_string(), user) } else { (user, pass) };
    if pass.is_empty() {
        bail!("add the TP-Link account password: tapo://admin:PASSWORD@{host}");
    }
    let (logins, cloud) = if is_hash(&pass) {
        (vec![(user, pass.to_ascii_uppercase())], None)
    } else {
        let md5 = hex_upper(&Md5::digest(pass.as_bytes()));
        (vec![(user, md5)], Some(pass))
    };
    Ok(Addr { host, port, logins, cloud })
}

/// An open Tapo speaker.
pub struct Speaker {
    rd: BufReader<OwnedReadHalf>,
    wr: OwnedWriteHalf,
    session: String,
    cc_pat: u8,
    cc_pmt: u8,
    cc_audio: u8,
    pts: u64,
    frames: u64,
}

struct Head {
    status: u16,
    headers: Vec<(String, String)>,
}

impl Head {
    fn get(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

async fn read_head(rd: &mut BufReader<OwnedReadHalf>) -> Result<Head> {
    let mut line = String::new();
    let mut status = 0;
    let mut headers = Vec::new();
    loop {
        line.clear();
        if rd.read_line(&mut line).await? == 0 {
            bail!("the camera closed the connection");
        }
        let l = line.trim_end();
        if status == 0 {
            if l.is_empty() {
                continue;
            }
            status = l.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            continue;
        }
        if l.is_empty() {
            break;
        }
        if let Some((k, v)) = l.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    Ok(Head { status, headers })
}

fn between<'a>(s: &'a str, a: &str, b: &str) -> &'a str {
    s.find(a)
        .map(|i| &s[i + a.len()..])
        .map(|r| &r[..r.find(b).unwrap_or(r.len())])
        .unwrap_or("")
}

impl Speaker {
    pub async fn open(url: &str) -> Result<Speaker> {
        let a = parse(url)?;
        let mut logins = a.logins.clone();
        let mut last = anyhow!("the camera refused the login");
        let mut i = 0;
        while i < logins.len() {
            let (user, pass) = logins[i].clone();
            i += 1;
            match Self::login(&a, &user, &pass).await {
                Ok(Ok(s)) => return Self::start(s).await,
                Ok(Err(newer)) => {
                    // Newer firmware: the account password hashed with SHA-256.
                    if newer && let Some(c) = &a.cloud {
                        use sha2::Sha256;
                        let h = hex_upper(&Sha256::digest(c.as_bytes()));
                        if !logins.iter().any(|(_, p)| *p == h) {
                            logins.push((user, h));
                        }
                    }
                    last = anyhow!(
                        "the Tapo camera refused the login — use the password of the TP-Link (Tapo app) account the camera is in"
                    );
                }
                Err(e) => return Err(e),
            }
        }
        Err(last)
    }

    /// Ok(Ok(connection)) logged in; Ok(Err(newer_login)) refused.
    async fn login(a: &Addr, user: &str, pass: &str) -> Result<std::result::Result<(BufReader<OwnedReadHalf>, OwnedWriteHalf), bool>> {
        let s = tokio::time::timeout(Duration::from_secs(5), TcpStream::connect((a.host.as_str(), a.port)))
            .await
            .map_err(|_| anyhow!("no answer from {}:{} (is it a Tapo camera?)", a.host, a.port))?
            .with_context(|| format!("can't connect to {}:{}", a.host, a.port))?;
        let _ = s.set_nodelay(true);
        let (rd, mut wr) = s.into_split();
        let mut rd = BufReader::new(rd);
        let hostport = format!("{}:{}", a.host, a.port);
        let req = |auth: &str| {
            format!(
                "POST /stream HTTP/1.1\r\nHost: {hostport}\r\nUser-Agent: Go-http-client/1.1\r\nContent-Length: 0\r\n{auth}Content-Type: multipart/mixed; boundary={CLIENT_BOUNDARY}\r\n\r\n"
            )
        };
        wr.write_all(req("").as_bytes()).await?;
        let h = tokio::time::timeout(Duration::from_secs(5), read_head(&mut rd)).await.map_err(|_| anyhow!("the camera didn't answer"))??;
        if h.status != 401 {
            bail!("the camera answered {} (expected a login request)", h.status);
        }
        // What came with the refusal.
        let n: usize = h.get("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(0);
        let mut skip = vec![0u8; n.min(65536)];
        rd.read_exact(&mut skip).await?;
        let www = h.get("WWW-Authenticate").unwrap_or("").to_string();
        let newer = www.contains("encrypt_type=\"3\"");
        let realm = between(&www, "realm=\"", "\"");
        let nonce = between(&www, "nonce=\"", "\"");
        let qop = between(&www, "qop=\"", "\"");
        let qop = if qop.is_empty() { "auth" } else { qop.split(',').next().unwrap_or("auth").trim() };
        let uri = "/stream";
        let ha1 = md5_hex(&format!("{user}:{realm}:{pass}"));
        let ha2 = md5_hex(&format!("POST:{uri}"));
        let nc = "00000001";
        let cnonce: String = {
            let mut b = [0u8; 16];
            rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut b);
            b.iter().map(|x| format!("{x:02x}")).collect()
        };
        let response = md5_hex(&format!("{ha1}:{nonce}:{nc}:{cnonce}:{qop}:{ha2}"));
        let mut auth = format!(
            "Authorization: Digest username=\"{user}\", realm=\"{realm}\", nonce=\"{nonce}\", uri=\"{uri}\", qop={qop}, nc={nc}, cnonce=\"{cnonce}\", response=\"{response}\""
        );
        let opaque = between(&www, "opaque=\"", "\"");
        if !opaque.is_empty() {
            auth.push_str(&format!(", opaque=\"{opaque}\", algorithm=MD5"));
        }
        auth.push_str("\r\n");
        wr.write_all(req(&auth).as_bytes()).await?;
        let h = tokio::time::timeout(Duration::from_secs(5), read_head(&mut rd)).await.map_err(|_| anyhow!("the camera didn't answer"))??;
        match h.status {
            200 => Ok(Ok((rd, wr))),
            401 | 403 => Ok(Err(newer)),
            s => bail!("the camera answered {s} to the login"),
        }
    }

    async fn start((mut rd, mut wr): (BufReader<OwnedReadHalf>, OwnedWriteHalf)) -> Result<Speaker> {
        let body = r#"{"params":{"talk":{"mode":"aec"},"method":"get"},"seq":3,"type":"request"}"#;
        let part = format!(
            "--{CLIENT_BOUNDARY}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}\r\n",
            body.len()
        );
        wr.write_all(part.as_bytes()).await?;
        let json = tokio::time::timeout(Duration::from_secs(5), read_part(&mut rd))
            .await
            .map_err(|_| anyhow!("the camera didn't answer the talk request"))??;
        let v: serde_json::Value = serde_json::from_slice(&json).unwrap_or_default();
        let session = v["params"]["session_id"]
            .as_str()
            .map(str::to_string)
            .or_else(|| v["params"]["session_id"].as_i64().map(|n| n.to_string()))
            .unwrap_or_default();
        if session.is_empty() {
            let code = v["params"]["error_code"].as_i64().or(v["error_code"].as_i64());
            bail!(
                "the Tapo camera didn't open its speaker{}",
                code.map(|c| format!(" (error {c}) — it may be in use by the Tapo app")).unwrap_or_default()
            );
        }
        let mut s = Speaker { rd, wr, session, cc_pat: 0, cc_pmt: 0, cc_audio: 0, pts: 90_000, frames: 0 };
        let head = s.tables();
        s.write_part(&head).await?;
        Ok(s)
    }

    fn tables(&mut self) -> Vec<u8> {
        tables(&mut self.cc_pat, &mut self.cc_pmt)
    }

    async fn write_part(&mut self, ts: &[u8]) -> Result<()> {
        let mut part = format!(
            "--{CLIENT_BOUNDARY}\r\nContent-Type: audio/mp2t\r\nX-If-Encrypt: 0\r\nX-Session-Id: {}\r\nContent-Length: {}\r\n\r\n",
            self.session,
            ts.len()
        )
        .into_bytes();
        part.extend_from_slice(ts);
        part.extend_from_slice(b"\r\n");
        self.wr.write_all(&part).await.context("the camera closed the connection")
    }

    /// One frame of A-law audio (normally 160 bytes, 20 ms).
    pub async fn send(&mut self, alaw: &[u8]) -> Result<()> {
        let mut ts = Vec::with_capacity(400);
        // The tables again now and then, for a camera that missed them.
        if self.frames > 0 && self.frames.is_multiple_of(50) {
            ts.extend(self.tables());
        }
        ts.extend(pes(alaw, self.pts, &mut self.cc_audio));
        self.pts += alaw.len() as u64 * 90_000 / 8000;
        self.frames += 1;
        self.write_part(&ts).await
    }

    /// Read whatever the camera sent, without waiting.
    pub async fn tick(&mut self) -> Result<()> {
        while let Ok(r) = tokio::time::timeout(Duration::from_millis(1), self.rd.fill_buf()).await {
            let n = r?.len();
            if n == 0 {
                bail!("the camera closed the connection");
            }
            self.rd.consume(n);
        }
        Ok(())
    }

    pub async fn stop(mut self) {
        let _ = self.wr.shutdown().await;
    }
}

/// The next part the camera sends: its body.
async fn read_part(rd: &mut BufReader<OwnedReadHalf>) -> Result<Vec<u8>> {
    let mut line = String::new();
    // Up to the boundary.
    loop {
        line.clear();
        if rd.read_line(&mut line).await? == 0 {
            bail!("the camera closed the connection");
        }
        if line.contains(DEVICE_BOUNDARY) {
            break;
        }
    }
    let mut len = 0usize;
    loop {
        line.clear();
        if rd.read_line(&mut line).await? == 0 {
            bail!("the camera closed the connection");
        }
        let l = line.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some((k, v)) = l.split_once(':')
            && k.trim().eq_ignore_ascii_case("content-length")
        {
            len = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; len.min(1 << 20)];
    rd.read_exact(&mut body).await?;
    Ok(body)
}

/// PAT and PMT: one program with the A-law audio.
fn tables(cc_pat: &mut u8, cc_pmt: &mut u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(376);
    let mut pat = vec![0x00, 0xB0, 0, 0x00, 0x01, 0xC1, 0x00, 0x00, 0x00, 0x01];
    pat.extend_from_slice(&(0xE000 | PMT_PID).to_be_bytes());
    out.extend(psi(0, &mut pat, cc_pat));
    let mut pmt = vec![0x02, 0xB0, 0, 0x00, 0x01, 0xC1, 0x00, 0x00];
    pmt.extend_from_slice(&(0xE000 | AUDIO_PID).to_be_bytes());
    pmt.extend_from_slice(&[0xF0, 0x00, STREAM_TYPE_PCMA_TAPO]);
    pmt.extend_from_slice(&(0xE000 | AUDIO_PID).to_be_bytes());
    pmt.extend_from_slice(&[0xF0, 0x00]);
    out.extend(psi(PMT_PID, &mut pmt, cc_pmt));
    out
}

fn crc32_mpeg(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= (b as u32) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 { (crc << 1) ^ 0x04C1_1DB7 } else { crc << 1 };
        }
    }
    crc
}

/// A table section in one TS packet (section length and CRC filled in).
fn psi(pid: u16, section: &mut Vec<u8>, cc: &mut u8) -> Vec<u8> {
    let len = section.len() - 3 + 4;
    section[1] = 0xB0 | ((len >> 8) as u8 & 0x0F);
    section[2] = len as u8;
    let crc = crc32_mpeg(section);
    section.extend_from_slice(&crc.to_be_bytes());
    let mut p = vec![0x47, 0x40 | ((pid >> 8) as u8 & 0x1F), pid as u8, 0x10 | (*cc & 0x0F), 0x00];
    *cc = cc.wrapping_add(1);
    p.extend_from_slice(section);
    p.resize(188, 0xFF);
    p
}

fn pts_bytes(pts: u64, marker: u8) -> [u8; 5] {
    let p = pts & 0x1_FFFF_FFFF;
    [
        (marker << 4) | (((p >> 30) as u8 & 0x07) << 1) | 1,
        (p >> 22) as u8,
        (((p >> 15) as u8 & 0x7F) << 1) | 1,
        (p >> 7) as u8,
        ((p as u8 & 0x7F) << 1) | 1,
    ]
}

/// One PES packet of audio, cut into TS packets.
fn pes(payload: &[u8], pts: u64, cc: &mut u8) -> Vec<u8> {
    let mut pes = vec![0x00, 0x00, 0x01, 0xC0];
    let len = 3 + 5 + payload.len();
    pes.extend_from_slice(&(len as u16).to_be_bytes());
    pes.extend_from_slice(&[0x80, 0x80, 0x05]);
    pes.extend_from_slice(&pts_bytes(pts, 0x2));
    pes.extend_from_slice(payload);
    let mut out = Vec::new();
    let mut rest = &pes[..];
    let mut first = true;
    while !rest.is_empty() {
        let mut p = Vec::with_capacity(188);
        p.push(0x47);
        p.push(if first { 0x40 } else { 0x00 } | ((AUDIO_PID >> 8) as u8 & 0x1F));
        p.push(AUDIO_PID as u8);
        let take = rest.len().min(184);
        if take < 184 {
            // Stuffing in an adaptation field to fill the packet.
            p.push(0x30 | (*cc & 0x0F));
            let af = 183 - take;
            p.push(af as u8);
            if af > 0 {
                p.push(0x00);
                p.extend(std::iter::repeat_n(0xFF, af - 1));
            }
        } else {
            p.push(0x10 | (*cc & 0x0F));
        }
        *cc = cc.wrapping_add(1);
        p.extend_from_slice(&rest[..take]);
        rest = &rest[take..];
        first = false;
        debug_assert_eq!(p.len(), 188);
        out.extend(p);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        let a = parse("tapo://admin:secret@192.168.1.5").unwrap();
        assert_eq!((a.host.as_str(), a.port), ("192.168.1.5", 8800));
        assert_eq!(a.logins[0].0, "admin");
        assert_eq!(a.logins[0].1, hex_upper(&Md5::digest(b"secret")));
        let b = parse("tapo://p%40ss@10.0.0.2:8801").unwrap();
        assert_eq!((b.port, b.cloud.as_deref()), (8801, Some("p@ss")));
        let h = "0123456789ABCDEF0123456789abcdef";
        assert_eq!(parse(&format!("tapo://admin:{h}@x")).unwrap().logins[0].1, h.to_ascii_uppercase());
        assert!(parse("tapo://10.0.0.2").is_err());
        assert!(parse("rtsp://a:b@c").is_err());
    }

    #[test]
    fn transport_stream() {
        let mut cc = 0;
        let ts = pes(&[0xD5; 160], 90_000, &mut cc);
        assert_eq!(ts.len(), 188);
        assert_eq!(&ts[..3], &[0x47, 0x41, 0x00]);
        assert_eq!(ts[3] & 0x30, 0x30);
        assert_eq!(&ts[188 - 160..], &[0xD5; 160][..]);
        let big = pes(&[1; 400], 0, &mut cc);
        assert_eq!(big.len(), 188 * 3);
        assert_eq!(big[3] & 0x0F, 1);
        // PAT: the well-known CRC for program 1 → PMT 0x1000.
        let mut s = tables(&mut 0, &mut 0);
        assert_eq!(s.len(), 376);
        assert_eq!(&s[..5], &[0x47, 0x40, 0x00, 0x10, 0x00]);
        assert_eq!(&s[5..17], &[0x00, 0xB0, 0x0D, 0x00, 0x01, 0xC1, 0x00, 0x00, 0x00, 0x01, 0xF0, 0x00]);
        assert_eq!(&s[17..21], &[0x2A, 0xB1, 0x04, 0xB2]);
        s.clear();
    }

    #[test]
    fn pts_encoding() {
        assert_eq!(pts_bytes(0, 2), [0x21, 0x00, 0x01, 0x00, 0x01]);
        assert_eq!(pts_bytes(90_000, 2), [0x21, 0x00, 0x05, 0xBF, 0x21]);
    }
}
