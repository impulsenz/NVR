//! A small FTP client for backing up to a NAS (Synology, QNAP, a router's
//! USB drive …): log in, make the folders, upload in passive mode under a
//! temporary name and rename it when complete. Plain FTP, as NAS boxes on a
//! home network offer it; the login is sent the way FTP sends it.

use anyhow::{Context, Result, anyhow, bail};
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

/// Where to upload: `ftp://host[:port]/folder`.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub host: String,
    pub port: u16,
    pub folder: String,
}

/// Parse `ftp://host[:port][/folder]`.
pub fn parse(url: &str) -> Option<Target> {
    let rest = url.trim().strip_prefix("ftp://")?;
    let (hostport, folder) = rest.split_once('/').unwrap_or((rest, ""));
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) if !h.contains(']') || h.ends_with(']') => (h, p.parse().ok()?),
        _ => (hostport, 21),
    };
    (!host.is_empty()).then(|| Target {
        host: host.trim_matches(['[', ']']).to_string(),
        port,
        folder: folder.trim_matches('/').to_string(),
    })
}

pub fn is_ftp(remote: &str) -> bool {
    remote.trim().starts_with("ftp://")
}

struct Session {
    ctl: BufReader<TcpStream>,
}

const TIMEOUT: Duration = Duration::from_secs(30);

impl Session {
    async fn connect(t: &Target, user: &str, pass: &str) -> Result<Self> {
        let s = tokio::time::timeout(TIMEOUT, TcpStream::connect((t.host.as_str(), t.port)))
            .await
            .map_err(|_| anyhow!("{}:{} didn't answer", t.host, t.port))?
            .with_context(|| format!("cannot reach {}:{}", t.host, t.port))?;
        let mut me = Self {
            ctl: BufReader::new(s),
        };
        me.expect(&[220]).await.context("not an FTP server")?;
        let user = if user.trim().is_empty() {
            "anonymous"
        } else {
            user.trim()
        };
        let (code, msg) = me.cmd(&format!("USER {user}")).await?;
        if code == 331 {
            let (code, msg) = me.cmd(&format!("PASS {pass}")).await?;
            if code != 230 && code != 202 {
                bail!("the NAS refused the login: {msg}");
            }
        } else if code != 230 {
            bail!("the NAS refused the login: {msg}");
        }
        me.cmd("TYPE I").await?;
        Ok(me)
    }

    /// One reply: (code, text), multi-line replies joined.
    async fn reply(&mut self) -> Result<(u32, String)> {
        let mut line = String::new();
        let mut text = String::new();
        let mut first: Option<String> = None;
        loop {
            line.clear();
            let n = tokio::time::timeout(TIMEOUT, self.ctl.read_line(&mut line))
                .await
                .map_err(|_| anyhow!("the NAS stopped answering"))??;
            if n == 0 {
                bail!("the NAS closed the connection");
            }
            let l = line.trim_end();
            text.push_str(l);
            text.push(' ');
            let code = l.get(..3).filter(|c| c.chars().all(|x| x.is_ascii_digit()));
            match (&first, code) {
                (None, Some(c)) => {
                    if l.as_bytes().get(3) != Some(&b'-') {
                        return Ok((c.parse()?, text.trim().to_string()));
                    }
                    first = Some(c.to_string());
                }
                (Some(f), Some(c)) if c == f && l.as_bytes().get(3) == Some(&b' ') => {
                    return Ok((c.parse()?, text.trim().to_string()));
                }
                _ => {}
            }
        }
    }

    async fn expect(&mut self, ok: &[u32]) -> Result<String> {
        let (code, msg) = self.reply().await?;
        if !ok.contains(&code) {
            bail!("{msg}");
        }
        Ok(msg)
    }

    async fn cmd(&mut self, c: &str) -> Result<(u32, String)> {
        self.ctl
            .get_mut()
            .write_all(format!("{c}\r\n").as_bytes())
            .await?;
        self.reply().await
    }

    async fn ok(&mut self, c: &str, codes: &[u32]) -> Result<String> {
        let (code, msg) = self.cmd(c).await?;
        if !codes.contains(&code) {
            let what = c.split(' ').next().unwrap_or(c);
            bail!("{what}: {msg}");
        }
        Ok(msg)
    }

    /// Go into `folder`, making each part that doesn't exist yet.
    async fn enter(&mut self, folder: &str) -> Result<()> {
        for part in folder.split('/').filter(|p| !p.is_empty()) {
            if self.cmd(&format!("CWD {part}")).await?.0 != 250 {
                self.cmd(&format!("MKD {part}")).await?;
                self.ok(&format!("CWD {part}"), &[250])
                    .await
                    .with_context(|| format!("can't make the folder {part}"))?;
            }
        }
        Ok(())
    }

    /// A data connection (passive mode), to the same address as the control
    /// connection (NAS boxes behind routers often announce a wrong one).
    async fn data(&mut self) -> Result<TcpStream> {
        let peer = self.ctl.get_ref().peer_addr()?;
        let (code, msg) = self.cmd("EPSV").await?;
        let port = if code == 229 {
            msg.split('|').filter_map(|p| p.parse::<u16>().ok()).next()
        } else {
            None
        };
        let port = match port {
            Some(p) => p,
            None => {
                let msg = self.ok("PASV", &[227]).await?;
                let nums: Vec<u16> = msg
                    .split(['(', ')'])
                    .nth(1)
                    .unwrap_or("")
                    .split(',')
                    .filter_map(|n| n.trim().parse().ok())
                    .collect();
                if nums.len() != 6 {
                    bail!("unexpected passive-mode reply: {msg}");
                }
                nums[4] * 256 + nums[5]
            }
        };
        tokio::time::timeout(TIMEOUT, TcpStream::connect((peer.ip(), port)))
            .await
            .map_err(|_| anyhow!("the NAS's data connection didn't open"))?
            .context("the NAS's data connection didn't open")
    }

    /// Upload `src` as `name` in the current folder.
    async fn put(&mut self, src: &Path, name: &str) -> Result<()> {
        let tmp = format!("{name}.part");
        let mut data = self.data().await?;
        let (code, msg) = self.cmd(&format!("STOR {tmp}")).await?;
        if code != 150 && code != 125 {
            bail!("the NAS won't take the file: {msg}");
        }
        let mut f = tokio::fs::File::open(src).await?;
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let n = f.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            data.write_all(&buf[..n]).await?;
        }
        data.shutdown().await?;
        drop(data);
        self.expect(&[226, 250])
            .await
            .context("upload not finished")?;
        self.cmd(&format!("DELE {name}")).await?; // an older copy, if any
        self.ok(&format!("RNFR {tmp}"), &[350]).await?;
        self.ok(&format!("RNTO {name}"), &[250]).await?;
        Ok(())
    }

    async fn quit(mut self) {
        let _ = self.cmd("QUIT").await;
    }
}

/// Upload a file to `<folder>/<rel>` on the NAS.
pub async fn upload(t: &Target, user: &str, pass: &str, src: &Path, rel: &str) -> Result<()> {
    let mut s = Session::connect(t, user, pass).await?;
    let path = format!("{}/{}", t.folder, rel);
    let (dir, name) = path.rsplit_once('/').unwrap_or(("", &path));
    s.enter(dir).await?;
    s.put(src, name).await?;
    s.quit().await;
    Ok(())
}

/// Log in, make the folder and check it can be written to.
pub async fn test(t: &Target, user: &str, pass: &str) -> Result<()> {
    let mut s = Session::connect(t, user, pass).await?;
    s.enter(&t.folder).await?;
    let probe = std::env::temp_dir().join(format!("gaze-ftp-test-{}", std::process::id()));
    std::fs::write(&probe, b"GAZE can write here")?;
    let r = s.put(&probe, ".gaze-test").await;
    let _ = std::fs::remove_file(&probe);
    r?;
    s.cmd("DELE .gaze-test").await?;
    s.quit().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_addresses() {
        let t = super::parse("ftp://nas.local/Cameras/GAZE/").unwrap();
        assert_eq!(
            (t.host.as_str(), t.port, t.folder.as_str()),
            ("nas.local", 21, "Cameras/GAZE")
        );
        let t = super::parse("ftp://192.168.1.5:2121").unwrap();
        assert_eq!(
            (t.host.as_str(), t.port, t.folder.as_str()),
            ("192.168.1.5", 2121, "")
        );
        assert!(super::parse("gdrive:NVR").is_none());
        assert!(super::parse("ftp://").is_none());
    }
}
