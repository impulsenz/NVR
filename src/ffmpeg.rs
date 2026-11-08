//! Building ffmpeg command lines, GPU acceleration, and capability detection.

use crate::config::{HardwareConfig, redact};
use anyhow::{Context, Result, bail};
use base64::Engine;
use serde::Serialize;
use std::process::Stdio;
use std::time::Duration;
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

/// Strip ffmpeg's `[h264_nvenc @ 0x55...] ` context prefix.
pub fn clean_line(line: &str) -> String {
    let l = line.trim();
    if l.starts_with('[')
        && let Some(end) = l.find("] ")
        && l[..end].contains(" @ 0x")
    {
        return l[end + 2..].trim().to_string();
    }
    l.to_string()
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
    if let Ok(o) = run(Command::new(ffmpeg).arg("-version"), 10).await {
        caps.ffmpeg_version = String::from_utf8_lossy(&o.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .to_string();
    }
    if let Ok(o) = run(Command::new(ffmpeg).args(["-hide_banner", "-hwaccels"]), 10).await {
        caps.hwaccels = String::from_utf8_lossy(&o.stdout)
            .lines()
            .skip(1)
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
    }
    if let Ok(o) = run(Command::new(ffmpeg).args(["-hide_banner", "-encoders"]), 10).await {
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
        Command::new(ffmpeg)
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
                Command::new(ffmpeg)
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
                Command::new(ffmpeg)
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

#[derive(Debug, Clone, Serialize, Default)]
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
pub async fn probe(ffmpeg: &str, url: &str, transport: &str) -> ProbeResult {
    let info = async {
        let mut cmd = Command::new(ffprobe_path(ffmpeg));
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
        let v: serde_json::Value =
            serde_json::from_slice(&o.stdout).context("bad ffprobe output")?;
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
        Ok::<_, anyhow::Error>(streams)
    };
    let snap = async {
        let o = run(
            Command::new(ffmpeg)
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
    }
}
