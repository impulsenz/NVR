//! Building ffmpeg command lines, GPU acceleration, and capability detection.

use crate::config::{HardwareConfig, redact};
use anyhow::{Context, Result, bail};
use base64::Engine;
use serde::Serialize;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::process::Command;

/// Input options for a camera URL.
pub fn input_args(url: &str, transport: &str) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    if url.starts_with("rtsp://") || url.starts_with("rtsps://") {
        let t = if transport == "udp" { "udp" } else { "tcp" };
        // Socket timeout in microseconds.
        args.extend(["-rtsp_transport", t, "-timeout", "10000000"].map(String::from));
    } else if !url.contains("://") {
        // Local file: play it in real time on a loop (test mode).
        args.extend(["-re", "-stream_loop", "-1"].map(String::from));
    }
    args.push("-i".into());
    args.push(url.into());
    args
}

/// Hardware decoding options (placed before `-i`).
/// `keep_on_gpu` keeps decoded frames in GPU memory for a matching GPU encoder.
pub fn decode_args(hw: &HardwareConfig, keep_on_gpu: bool) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    match hw.hwaccel.as_str() {
        "none" | "" => return a,
        "vaapi" => {
            a.extend(["-hwaccel", "vaapi", "-hwaccel_device"].map(String::from));
            a.push(hw.vaapi_device().into());
        }
        other => {
            a.push("-hwaccel".into());
            a.push(other.into());
        }
    }
    if keep_on_gpu {
        a.push("-hwaccel_output_format".into());
        a.push(hw.hwaccel.clone());
    }
    a
}

/// True when decoder and encoder are on the same GPU API so frames never leave the GPU.
pub fn full_gpu_pipeline(hw: &HardwareConfig) -> bool {
    matches!(
        (hw.hwaccel.as_str(), hw.resolved_encoder()),
        ("cuda", "h264_nvenc") | ("qsv", "h264_qsv") | ("vaapi", "h264_vaapi")
    )
}

/// H.264 encoder options: (before input, after input).
pub fn encoder_args(
    encoder: &str,
    hw: &HardwareConfig,
    frames_on_gpu: bool,
) -> (Vec<String>, Vec<String>) {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let mut pre = Vec::new();
    let mut out = match encoder {
        "h264_nvenc" => s(&["-c:v", "h264_nvenc", "-preset", "p4", "-cq", "23"]),
        "h264_qsv" => {
            let mut v = s(&["-c:v", "h264_qsv", "-global_quality", "23"]);
            if !frames_on_gpu {
                v.splice(0..0, s(&["-pix_fmt", "nv12"]));
            }
            v
        }
        "h264_vaapi" => {
            let mut v = Vec::new();
            if !frames_on_gpu {
                pre = vec!["-vaapi_device".into(), hw.vaapi_device().into()];
                v = s(&["-vf", "format=nv12,hwupload"]);
            }
            v.extend(s(&["-c:v", "h264_vaapi", "-qp", "23"]));
            v
        }
        "h264_videotoolbox" => s(&["-c:v", "h264_videotoolbox", "-b:v", "4M"]),
        "h264_amf" => s(&[
            "-c:v", "h264_amf", "-quality", "balanced", "-rc", "cqp", "-qp_i", "23", "-qp_p", "23",
        ]),
        "h264_mf" => s(&["-c:v", "h264_mf", "-b:v", "4M"]),
        _ => s(&[
            "-c:v", "libx264", "-preset", "veryfast", "-crf", "23", "-pix_fmt", "yuv420p",
        ]),
    };
    // Regular keyframes so files split cleanly and seeking is fast.
    out.extend(s(&["-force_key_frames", "expr:gte(t,n_forced*2)"]));
    (pre, out)
}

/// ffprobe sits next to ffmpeg.
fn ffprobe_path(ffmpeg: &str) -> String {
    match ffmpeg.rfind("ffmpeg") {
        Some(i) => format!("{}ffprobe{}", &ffmpeg[..i], &ffmpeg[i + 6..]),
        None => "ffprobe".into(),
    }
}

async fn run(cmd: &mut Command, secs: u64) -> Result<std::process::Output> {
    cmd.stdin(Stdio::null()).kill_on_drop(true);
    tokio::time::timeout(Duration::from_secs(secs), cmd.output())
        .await
        .context("timed out")?
        .context("could not run ffmpeg")
}

/// The most useful error from ffmpeg's stderr: the first line usually names the
/// root cause ("Cannot load libcuda.so.1"); later lines are generic fallout.
fn first_error(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr)
        .lines()
        .map(clean_line)
        .find(|l| !l.is_empty())
        .unwrap_or_default()
}

/// Split ffmpeg's `[hevc @ 0x55d1…] message` (Linux/macOS) or
/// `[hevc @ 000002138f33…] message` (Windows) into (context name, message).
fn context(line: &str) -> Option<(&str, &str)> {
    let l = line.trim();
    let end = l.strip_prefix('[')?.find("] ")? + 1;
    let (name, addr) = l[1..end].rsplit_once(" @ ")?;
    let hex = addr.trim_start_matches("0x");
    (!hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| (name, l[end + 2..].trim()))
}

/// Strip ffmpeg's `[h264_nvenc @ 0x55...] ` context prefix.
pub fn clean_line(line: &str) -> String {
    match context(line) {
        Some((_, rest)) => rest.to_string(),
        None => line.trim().to_string(),
    }
}

/// Decoders whose complaints are about individual damaged/missing frames.
const DECODERS: &[&str] = &[
    "h264",
    "hevc",
    "h265",
    "mpeg4",
    "mjpeg",
    "h263",
    "aac",
    "pcm_alaw",
    "pcm_mulaw",
    "adpcm_g726",
    "adpcm_g726le",
    "opus",
];

/// Separates useful ffmpeg messages from per-frame noise.
///
/// When a decoder joins a live stream mid-way, or a network packet is lost,
/// ffmpeg prints a warning for every affected frame ("Could not find ref with
/// POC 23", "First slice in a frame missing", "RTP: missed 3 packets"). The
/// picture recovers at the next keyframe, so these are counted and summarized
/// instead of flooding the log.
pub struct StderrFilter {
    pub decode_warnings: u64,
    lost_packets: u64,
    window: Instant,
}

impl Default for StderrFilter {
    fn default() -> Self {
        Self {
            decode_warnings: 0,
            lost_packets: 0,
            window: Instant::now(),
        }
    }
}

impl StderrFilter {
    /// Returns the message if it is worth logging, or `None` for noise (counted).
    pub fn filter(&mut self, raw: &str) -> Option<String> {
        let line = raw.trim();
        if line.is_empty() {
            return None;
        }
        // ffmpeg's own de-duplication of the noise below.
        if line.starts_with("Last message repeated") {
            return None;
        }
        let Some((name, msg)) = context(line) else {
            return Some(line.to_string());
        };
        if let Some(rest) = msg.strip_prefix("RTP: missed ") {
            let n: u64 = rest
                .split_whitespace()
                .next()
                .and_then(|n| n.parse().ok())
                .unwrap_or(1);
            self.lost_packets += n;
            return None;
        }
        if DECODERS.contains(&name) {
            self.decode_warnings += 1;
            return None;
        }
        Some(msg.to_string())
    }

    /// A one-line summary of suppressed noise: at most once a minute, or
    /// whenever `force` is set (e.g. when the process exits).
    pub fn summary(&mut self, force: bool) -> Option<String> {
        if self.decode_warnings + self.lost_packets == 0 {
            self.window = Instant::now();
            return None;
        }
        if !force && self.window.elapsed() < Duration::from_secs(60) {
            return None;
        }
        let mut parts = Vec::new();
        if self.decode_warnings > 0 {
            parts.push(format!(
                "{} damaged/incomplete video frames",
                self.decode_warnings
            ));
        }
        if self.lost_packets > 0 {
            parts.push(format!("{} lost network packets", self.lost_packets));
        }
        let msg = format!(
            "{} in the last {}s (normal when a stream starts; if it keeps happening, check the network/Wi-Fi)",
            parts.join(" and "),
            self.window.elapsed().as_secs().max(1)
        );
        *self = Self::default();
        Some(msg)
    }
}

// ---------- capability detection ----------

#[derive(Debug, Clone, Serialize, Default)]
pub struct Capabilities {
    pub ffmpeg_version: String,
    /// GPU decoders compiled into ffmpeg (not proof the hardware exists).
    pub hwaccels: Vec<String>,
    /// H.264 encoders compiled into ffmpeg.
    pub encoders: Vec<String>,
}

pub async fn capabilities(ffmpeg: &str) -> Capabilities {
    let mut caps = Capabilities::default();
    if let Ok(o) = run(crate::load::cmd(ffmpeg).arg("-version"), 10).await {
        caps.ffmpeg_version = String::from_utf8_lossy(&o.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .to_string();
    }
    if let Ok(o) = run(
        crate::load::cmd(ffmpeg).args(["-hide_banner", "-hwaccels"]),
        10,
    )
    .await
    {
        caps.hwaccels = String::from_utf8_lossy(&o.stdout)
            .lines()
            .skip(1)
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
    }
    if let Ok(o) = run(
        crate::load::cmd(ffmpeg).args(["-hide_banner", "-encoders"]),
        10,
    )
    .await
    {
        let text = String::from_utf8_lossy(&o.stdout);
        caps.encoders = crate::config::ENCODERS
            .iter()
            .filter(|e| **e != "auto")
            .filter(|e| {
                text.lines()
                    .any(|l| l.split_whitespace().nth(1) == Some(**e))
            })
            .map(|e| e.to_string())
            .collect();
    }
    caps
}

#[derive(Debug, Clone, Serialize)]
pub struct TestResult {
    pub ok: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HwTest {
    pub decoder: TestResult,
    pub encoder: TestResult,
}

/// Actually try the selected GPU decoder and encoder on a generated test clip.
pub async fn test_hardware(ffmpeg: &str, hw: &HardwareConfig) -> HwTest {
    let clip = std::env::temp_dir().join(format!("nvr-hwtest-{}.mp4", std::process::id()));
    let lavfi = [
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=1280x720:rate=25",
        "-t",
        "2",
    ];

    // Encoder.
    let encoder = hw.resolved_encoder().to_string();
    let (pre, out) = encoder_args(&encoder, hw, false);
    let enc = run(
        crate::load::cmd(ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(&pre)
            .args(lavfi)
            .args(&out)
            .arg(&clip),
        30,
    )
    .await;
    let encoder_res = match enc {
        Ok(o) if o.status.success() => TestResult {
            ok: true,
            message: format!("{encoder} works"),
        },
        Ok(o) => TestResult {
            ok: false,
            message: format!("{encoder} failed: {}", first_error(&o.stderr)),
        },
        Err(e) => TestResult {
            ok: false,
            message: format!("{encoder} failed: {e:#}"),
        },
    };

    // Decoder: needs an H.264 clip; make one on the CPU if the GPU encoder failed.
    let decoder_res = if hw.hwaccel == "none" {
        TestResult {
            ok: true,
            message: "GPU decoding is off (CPU decoding)".into(),
        }
    } else {
        let have_clip = encoder_res.ok
            || run(
                crate::load::cmd(ffmpeg)
                    .args(["-hide_banner", "-loglevel", "error", "-y"])
                    .args(lavfi)
                    .args(["-c:v", "libx264", "-preset", "ultrafast"])
                    .arg(&clip),
                30,
            )
            .await
            .is_ok_and(|o| o.status.success());
        if !have_clip {
            TestResult {
                ok: false,
                message: "could not create a test clip (is libx264 available?)".into(),
            }
        } else {
            let dec = run(
                crate::load::cmd(ffmpeg)
                    .args(["-hide_banner", "-loglevel", "error"])
                    .args(decode_args(hw, false))
                    .arg("-i")
                    .arg(&clip)
                    .args(["-f", "null", "-"]),
                30,
            )
            .await;
            match dec {
                Ok(o) if o.status.success() => TestResult {
                    ok: true,
                    message: if hw.hwaccel == "auto" {
                        "auto: ffmpeg will use a GPU if it finds one, otherwise the CPU".into()
                    } else {
                        format!("{} decoding works", hw.hwaccel)
                    },
                },
                Ok(o) => TestResult {
                    ok: false,
                    message: format!("{} failed: {}", hw.hwaccel, first_error(&o.stderr)),
                },
                Err(e) => TestResult {
                    ok: false,
                    message: format!("{} failed: {e:#}", hw.hwaccel),
                },
            }
        }
    };
    let _ = std::fs::remove_file(&clip);
    HwTest {
        decoder: decoder_res,
        encoder: encoder_res,
    }
}

// ---------- camera probe ----------

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct StreamInfo {
    pub kind: String,
    pub codec: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ProbeResult {
    pub ok: bool,
    pub error: Option<String>,
    pub streams: Vec<StreamInfo>,
    /// JPEG preview as a data: URL.
    pub snapshot: Option<String>,
}

/// Connect to a stream, report its codecs and grab a preview frame.
/// Codec, size and frame rate of each stream.
pub async fn probe_streams(ffmpeg: &str, url: &str, transport: &str) -> Result<Vec<StreamInfo>> {
    let mut cmd = crate::load::cmd(ffprobe_path(ffmpeg));
    cmd.args([
        "-v",
        "error",
        "-show_entries",
        "stream=codec_type,codec_name,width,height,avg_frame_rate",
        "-of",
        "json",
    ]);
    let mut input = input_args(url, transport);
    input.retain(|a| a != "-re" && a != "-stream_loop" && a != "-1");
    let url_arg = input.pop().unwrap();
    input.pop(); // "-i"
    cmd.args(input).arg(url_arg);
    let o = run(&mut cmd, 20).await?;
    if !o.status.success() {
        bail!("{}", first_error(&o.stderr));
    }
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).context("bad ffprobe output")?;
    let streams = v["streams"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|s| StreamInfo {
                    kind: s["codec_type"].as_str().unwrap_or("").into(),
                    codec: s["codec_name"].as_str().unwrap_or("").into(),
                    width: s["width"].as_u64().map(|x| x as u32),
                    height: s["height"].as_u64().map(|x| x as u32),
                    fps: s["avg_frame_rate"].as_str().and_then(parse_rate),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(streams)
}

pub async fn probe(ffmpeg: &str, url: &str, transport: &str) -> ProbeResult {
    let info = probe_streams(ffmpeg, url, transport);
    let snap = async {
        let o = run(
            crate::load::cmd(ffmpeg)
                .args(["-hide_banner", "-loglevel", "error"])
                .args(input_args(url, transport))
                .args([
                    "-frames:v",
                    "1",
                    "-vf",
                    "scale='min(640,iw)':-2",
                    "-q:v",
                    "4",
                    "-f",
                    "image2pipe",
                    "-c:v",
                    "mjpeg",
                    "pipe:1",
                ]),
            20,
        )
        .await?;
        if o.stdout.is_empty() {
            bail!("{}", first_error(&o.stderr));
        }
        Ok::<_, anyhow::Error>(format!(
            "data:image/jpeg;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&o.stdout)
        ))
    };
    let (info, snap) = tokio::join!(info, snap);
    let mut res = ProbeResult::default();
    match snap {
        Ok(s) => res.snapshot = Some(s),
        Err(e) => res.error = Some(redact(&format!("{e:#}"), url)),
    }
    match info {
        Ok(s) => res.streams = s,
        Err(e) if res.error.is_none() && res.snapshot.is_none() => {
            res.error = Some(redact(&format!("{e:#}"), url))
        }
        Err(_) => {}
    }
    res.ok = res.snapshot.is_some();
    res
}

// ---------- recorded files ----------

/// Video codec of a media file (e.g. "h264", "hevc").
pub async fn video_codec(ffmpeg: &str, file: &std::path::Path) -> Option<String> {
    let o = run(
        crate::load::cmd(ffprobe_path(ffmpeg))
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=codec_name",
                "-of",
                "default=nw=1:nk=1",
            ])
            .arg(file),
        20,
    )
    .await
    .ok()?;
    let c = String::from_utf8_lossy(&o.stdout).trim().to_string();
    (!c.is_empty()).then_some(c)
}

/// Extra output options so copied H.265 plays on Apple devices.
pub async fn hevc_tag(ffmpeg: &str, file: &std::path::Path) -> Vec<String> {
    if video_codec(ffmpeg, file).await.as_deref() == Some("hevc") {
        vec!["-tag:v".into(), "hvc1".into()]
    } else {
        vec![]
    }
}

/// Duration of a media file in seconds.
pub async fn duration(ffmpeg: &str, file: &std::path::Path) -> Option<f64> {
    let o = run(
        crate::load::cmd(ffprobe_path(ffmpeg))
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "default=nw=1:nk=1",
            ])
            .arg(file),
        30,
    )
    .await
    .ok()?;
    String::from_utf8_lossy(&o.stdout).trim().parse().ok()
}

/// A small preview image of a recording.
pub async fn thumbnail(ffmpeg: &str, file: &std::path::Path, out: &std::path::Path) -> Result<()> {
    let o = run(
        crate::load::background(&mut crate::load::cmd(ffmpeg))
            .args(["-hide_banner", "-loglevel", "error", "-y", "-ss", "1", "-i"])
            .arg(file)
            .args([
                "-frames:v",
                "1",
                "-vf",
                "scale=320:-2",
                "-q:v",
                "5",
                "-update",
                "1",
            ])
            .arg(out),
        30,
    )
    .await?;
    if !o.status.success() || !out.exists() {
        bail!("{}", first_error(&o.stderr));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
pub struct SpriteInfo {
    /// Seconds between pictures.
    pub interval: f64,
    pub count: u32,
    /// Pictures per row on each page.
    pub columns: u32,
    /// Pictures per page (pages are separate images).
    pub per_page: u32,
    pub pages: u32,
    pub tile_width: u32,
    pub tile_height: u32,
    pub duration: f64,
    /// Changes whenever the pictures are regenerated (cache busting).
    pub version: u64,
}

/// Preview pictures ("sprites") for scrubbing and time-lapse: pages of
/// 10×10 small pictures, saved as `<prefix>-000.jpg`, `<prefix>-001.jpg` …
///
/// `dense`: one picture per second, every frame decoded (for the light
/// sub-stream copies). Otherwise keyframes only (fast for big HD files),
/// one picture per 2 seconds. Pictures are put together here rather than
/// by ffmpeg, so the count is exact (no black filler pictures) and a
/// picture that decoded black is replaced by the previous one.
pub async fn sprite(
    ffmpeg: &str,
    file: &std::path::Path,
    prefix: &std::path::Path,
    dense: bool,
) -> Result<SpriteInfo> {
    use tokio::io::AsyncReadExt;
    const COLS: u32 = 10;
    const PER_PAGE: u32 = 100;
    let (tw, th) = (256u32, 144u32);
    let duration = duration(ffmpeg, file).await.unwrap_or(0.0);
    let interval = if dense { 1.0 } else { 2.0 };
    let vf = format!(
        "fps=1/{interval},scale={tw}:{th}:force_original_aspect_ratio=decrease,pad={tw}:{th}:-1:-1"
    );
    let mut command = crate::load::cmd(ffmpeg);
    let cmd = crate::load::background(&mut command);
    cmd.args(["-hide_banner", "-loglevel", "error"]);
    if !dense {
        cmd.args(["-skip_frame", "nokey"]);
    }
    let mut child = cmd
        .arg("-i")
        .arg(file)
        .args([
            "-an", "-vf", &vf, "-f", "rawvideo", "-pix_fmt", "rgb24", "pipe:1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("starting ffmpeg for previews")?;
    let mut out = child.stdout.take().context("no output")?;
    let frame_len = (tw * th * 3) as usize;
    let mut page: Vec<Vec<u8>> = Vec::new();
    let mut last: Option<Vec<u8>> = None;
    let (mut count, mut pages) = (0u32, 0u32);
    let save = |tiles: Vec<Vec<u8>>, n: u32| {
        let p = std::path::PathBuf::from(format!("{}-{n:03}.jpg", prefix.display()));
        async move {
            tokio::task::spawn_blocking(move || -> Result<()> {
                let rows = (tiles.len() as u32).div_ceil(COLS);
                let (w, h) = (COLS * tw, rows * th);
                let mut img = vec![0u8; (w * h * 3) as usize];
                for (i, t) in tiles.iter().enumerate() {
                    let (cx, cy) = ((i as u32 % COLS) * tw, (i as u32 / COLS) * th);
                    for y in 0..th {
                        let dst = (((cy + y) * w + cx) * 3) as usize;
                        let src = (y * tw * 3) as usize;
                        img[dst..dst + (tw * 3) as usize]
                            .copy_from_slice(&t[src..src + (tw * 3) as usize]);
                    }
                }
                let jpg = crate::imgutil::encode_jpeg(&img, w, h, 70)?;
                let tmp = p.with_extension("jpg.tmp");
                std::fs::write(&tmp, jpg)?;
                std::fs::rename(&tmp, &p)?;
                Ok(())
            })
            .await?
        }
    };
    let timeout = Duration::from_secs(600);
    let started = Instant::now();
    loop {
        let mut buf = vec![0u8; frame_len];
        match tokio::time::timeout(Duration::from_secs(60), out.read_exact(&mut buf)).await {
            Ok(Ok(_)) => {}
            _ => break,
        }
        // A picture that decoded as (nearly) solid black: keep the previous one.
        let sample = buf.iter().step_by(37);
        let (sum, n) = sample.fold((0u64, 0u64), |(s, n), v| (s + *v as u64, n + 1));
        if sum / n.max(1) < 3
            && let Some(prev) = &last
        {
            buf = prev.clone();
        }
        last = Some(buf.clone());
        page.push(buf);
        count += 1;
        if page.len() as u32 == PER_PAGE {
            save(std::mem::take(&mut page), pages).await?;
            pages += 1;
        }
        if started.elapsed() > timeout {
            break;
        }
    }
    let _ = child.kill().await;
    if !page.is_empty() {
        save(page, pages).await?;
        pages += 1;
    }
    if count == 0 {
        bail!("no pictures could be read from the recording");
    }
    let version = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Ok(SpriteInfo {
        interval,
        count,
        columns: COLS,
        per_page: PER_PAGE,
        pages,
        tile_width: tw,
        tile_height: th,
        duration: if duration > 0.0 {
            duration
        } else {
            count as f64 * interval
        },
        version,
    })
}

/// A path for an ffmpeg concat list. ffmpeg resolves relative paths in the
/// list against the list file's own folder, so always write the full path
/// (with forward slashes, and quotes escaped).
pub fn concat_path(path: &std::path::Path) -> String {
    let full = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    full.display()
        .to_string()
        .replace('\\', "/")
        .replace('\'', "'\\''")
}

/// Cut `[from, to)` seconds out of a sequence of files into one MP4
/// (without re-encoding, so cuts snap to the nearest keyframe).
pub async fn export(
    ffmpeg: &str,
    parts: &[(std::path::PathBuf, f64, f64)],
    out: &std::path::Path,
) -> Result<()> {
    let list = out.with_extension("txt");
    let mut text = String::new();
    for (path, inpoint, outpoint) in parts {
        text.push_str(&format!(
            "file '{}'\ninpoint {inpoint:.3}\noutpoint {outpoint:.3}\n",
            concat_path(path)
        ));
    }
    std::fs::write(&list, text)?;
    let tag = match parts.first() {
        Some(p) => hevc_tag(ffmpeg, &p.0).await,
        None => vec![],
    };
    let o = run(
        crate::load::cmd(ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "concat",
                "-safe",
                "0",
                "-i",
            ])
            .arg(&list)
            .args([
                "-c",
                "copy",
                "-movflags",
                "+faststart",
                "-avoid_negative_ts",
                "make_zero",
            ])
            .args(&tag)
            .arg(out),
        600,
    )
    .await;
    let _ = std::fs::remove_file(&list);
    let o = o?;
    if !o.status.success() {
        bail!("{}", first_error(&o.stderr));
    }
    Ok(())
}

fn parse_rate(r: &str) -> Option<f64> {
    let (n, d) = r.split_once('/')?;
    let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
    (d > 0.0 && n > 0.0).then(|| (n / d * 100.0).round() / 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffprobe_next_to_ffmpeg() {
        assert_eq!(ffprobe_path("ffmpeg"), "ffprobe");
        assert_eq!(
            ffprobe_path("/opt/ffmpeg/bin/ffmpeg.exe"),
            "/opt/ffmpeg/bin/ffprobe.exe"
        );
    }

    #[test]
    fn gpu_args() {
        let hw = HardwareConfig {
            hwaccel: "cuda".into(),
            encoder: "auto".into(),
            device: String::new(),
            ..Default::default()
        };
        assert!(full_gpu_pipeline(&hw));
        assert_eq!(
            decode_args(&hw, true),
            ["-hwaccel", "cuda", "-hwaccel_output_format", "cuda"]
        );
        let hw = HardwareConfig {
            hwaccel: "vaapi".into(),
            encoder: "h264_vaapi".into(),
            device: String::new(),
            ..Default::default()
        };
        let (pre, out) = encoder_args("h264_vaapi", &hw, false);
        assert_eq!(pre, ["-vaapi_device", "/dev/dri/renderD128"]);
        assert!(out.contains(&"format=nv12,hwupload".to_string()));
        assert_eq!(parse_rate("15/1"), Some(15.0));
        assert_eq!(parse_rate("0/0"), None);
        assert_eq!(
            clean_line("[h264_nvenc @ 0x5560a47] Cannot load libcuda.so.1"),
            "Cannot load libcuda.so.1"
        );
        assert_eq!(clean_line("[tcp] x"), "[tcp] x");
        assert_eq!(
            clean_line("[hevc @ 000002138f332440] First slice in a frame missing."),
            "First slice in a frame missing."
        );
    }

    #[test]
    fn filters_decoder_noise() {
        let mut f = StderrFilter::default();
        assert_eq!(
            f.filter("[hevc @ 000001ae7039be00] Could not find ref with POC 23"),
            None
        );
        assert_eq!(
            f.filter("[h264 @ 0x55d1c2a0] error while decoding MB 3 4"),
            None
        );
        assert_eq!(f.filter("Last message repeated 2 times"), None);
        assert_eq!(f.filter("[rtsp @ 0x5512] RTP: missed 7 packets"), None);
        assert_eq!(
            f.filter("[tcp @ 0x5512] Connection to tcp://10.0.0.5:554 failed: Connection refused")
                .as_deref(),
            Some("Connection to tcp://10.0.0.5:554 failed: Connection refused")
        );
        assert_eq!(
            f.filter("Error opening input file x.").as_deref(),
            Some("Error opening input file x.")
        );
        assert!(f.summary(false).is_none()); // not a minute yet
        let s = f.summary(true).unwrap();
        assert!(
            s.starts_with("2 damaged/incomplete video frames and 7 lost network packets"),
            "{s}"
        );
        assert!(f.summary(true).is_none());
    }
}
