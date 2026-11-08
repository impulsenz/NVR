//! Live view: one shared ffmpeg process transcodes the camera stream to JPEG
//! frames, which are fanned out to every connected browser as MJPEG.
//! The process only runs while somebody is watching.

use crate::config::{Config, ffmpeg_input_args};
use anyhow::{Context, Result, bail};
use bytes::{Bytes, BytesMut};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::broadcast;
use tracing::{info, warn};

pub struct Live {
    cfg: Arc<Config>,
    tx: broadcast::Sender<Bytes>,
    running: Mutex<bool>,
    latest: Mutex<Option<(Instant, Bytes)>>,
}

impl Live {
    pub fn new(cfg: Arc<Config>) -> Arc<Self> {
        let (tx, _) = broadcast::channel(4);
        Arc::new(Self {
            cfg,
            tx,
            running: Mutex::new(false),
            latest: Mutex::new(None),
        })
    }

    pub fn viewers(&self) -> usize {
        self.tx.receiver_count()
    }

    /// Subscribe to live JPEG frames, starting the transcoder if needed.
    pub fn subscribe(self: &Arc<Self>) -> broadcast::Receiver<Bytes> {
        let mut running = self.running.lock().unwrap();
        let rx = self.tx.subscribe();
        if !*running {
            *running = true;
            tokio::spawn(self.clone().produce());
        }
        rx
    }

    /// A single JPEG snapshot. Uses the live frame if fresh, otherwise grabs one.
    pub async fn snapshot(&self) -> Result<Bytes> {
        if let Some((at, frame)) = self.latest.lock().unwrap().clone()
            && at.elapsed() < Duration::from_secs(2)
        {
            return Ok(frame);
        }
        let out = tokio::time::timeout(
            Duration::from_secs(20),
            Command::new(&self.cfg.server.ffmpeg_path)
                .args(["-hide_banner", "-nostdin", "-loglevel", "error"])
                .args(ffmpeg_input_args(&self.cfg.camera.url))
                .args([
                    "-frames:v",
                    "1",
                    "-q:v",
                    "2",
                    "-f",
                    "image2pipe",
                    "-c:v",
                    "mjpeg",
                    "pipe:1",
                ])
                .stdin(Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .context("timed out grabbing snapshot")?
        .context("running ffmpeg")?;
        if out.stdout.is_empty() {
            bail!(
                "no image from camera: {}",
                crate::config::redact(
                    String::from_utf8_lossy(&out.stderr).trim(),
                    &self.cfg.camera.url
                )
            );
        }
        Ok(Bytes::from(out.stdout))
    }

    async fn produce(self: Arc<Self>) {
        info!("live: starting stream");
        loop {
            if let Err(e) = self.run_once().await {
                warn!("live: {e:#}");
            }
            // Stop if nobody is watching any more; otherwise retry.
            {
                let mut running = self.running.lock().unwrap();
                if self.tx.receiver_count() == 0 {
                    *running = false;
                    info!("live: no viewers, stopping stream");
                    return;
                }
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }

    async fn run_once(&self) -> Result<()> {
        let live = &self.cfg.live;
        let mut vf = format!("fps={}", live.fps);
        if live.width > 0 {
            vf.push_str(&format!(",scale='min({},iw)':-2", live.width));
        }
        let mut child = Command::new(&self.cfg.server.ffmpeg_path)
            .args(["-hide_banner", "-nostdin", "-loglevel", "error"])
            .args(["-fflags", "nobuffer", "-flags", "low_delay"])
            .args(ffmpeg_input_args(self.cfg.live_url()))
            .args(["-an", "-vf", &vf, "-q:v", &live.quality.to_string()])
            .args(["-f", "image2pipe", "-c:v", "mjpeg", "pipe:1"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("starting ffmpeg for live view")?;
        let mut stdout = child.stdout.take().context("no stdout")?;
        if let Some(stderr) = child.stderr.take() {
            let url = self.cfg.live_url().to_string();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    warn!("live ffmpeg: {}", crate::config::redact(line.trim(), &url));
                }
            });
        }

        let mut buf = BytesMut::with_capacity(512 * 1024);
        let mut chunk = vec![0u8; 64 * 1024];
        let mut scan_from = 0usize;
        loop {
            let n = tokio::time::timeout(Duration::from_secs(20), stdout.read(&mut chunk))
                .await
                .context("live stream stalled")??;
            if n == 0 {
                bail!("live ffmpeg exited");
            }
            buf.extend_from_slice(&chunk[..n]);

            // Split the byte stream into JPEG images (SOI 0xFFD8 .. EOI 0xFFD9).
            while let Some(frame) = next_jpeg(&mut buf, &mut scan_from) {
                *self.latest.lock().unwrap() = Some((Instant::now(), frame.clone()));
                let _ = self.tx.send(frame);
            }
            if buf.len() > 8 * 1024 * 1024 {
                buf.clear();
                scan_from = 0;
            }
            if self.tx.receiver_count() == 0 {
                let _ = child.kill().await;
                return Ok(());
            }
        }
    }
}

/// Extract the next complete JPEG from `buf`, if any.
fn next_jpeg(buf: &mut BytesMut, scan_from: &mut usize) -> Option<Bytes> {
    let start = buf.windows(2).position(|w| w == [0xFF, 0xD8])?;
    if start > 0 {
        let _ = buf.split_to(start);
        *scan_from = 0;
    }
    let from = (*scan_from).max(2);
    if buf.len() < from + 2 {
        return None;
    }
    match buf[from..].windows(2).position(|w| w == [0xFF, 0xD9]) {
        Some(pos) => {
            let end = from + pos + 2;
            *scan_from = 0;
            Some(buf.split_to(end).freeze())
        }
        None => {
            *scan_from = buf.len() - 1;
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_jpegs() {
        let mut buf = BytesMut::new();
        let mut scan = 0;
        buf.extend_from_slice(&[0x00, 0xFF, 0xD8, 1, 2, 0xFF]);
        assert!(next_jpeg(&mut buf, &mut scan).is_none());
        buf.extend_from_slice(&[0xD9, 0xFF, 0xD8, 3, 0xFF, 0xD9]);
        assert_eq!(
            &next_jpeg(&mut buf, &mut scan).unwrap()[..],
            &[0xFF, 0xD8, 1, 2, 0xFF, 0xD9]
        );
        assert_eq!(
            &next_jpeg(&mut buf, &mut scan).unwrap()[..],
            &[0xFF, 0xD8, 3, 0xFF, 0xD9]
        );
        assert!(next_jpeg(&mut buf, &mut scan).is_none());
    }
}
