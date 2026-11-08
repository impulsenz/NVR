//! Minimal RGB image helpers (no heavy image library needed).

use anyhow::Result;
use std::sync::Arc;

/// A decoded RGB24 frame.
#[derive(Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgb: Arc<Vec<u8>>,
}

impl Frame {
    pub fn jpeg(&self, quality: u8) -> Result<Vec<u8>> {
        encode_jpeg(&self.rgb, self.width, self.height, quality)
    }

    /// Bilinear resize.
    pub fn resize(&self, w: u32, h: u32) -> Frame {
        Frame {
            width: w,
            height: h,
            rgb: Arc::new(resize_rgb(&self.rgb, self.width, self.height, w, h)),
        }
    }

    /// Scale down so the longest side is at most `max` pixels.
    pub fn fit(&self, max: u32) -> Frame {
        let long = self.width.max(self.height);
        if long <= max {
            return self.clone();
        }
        let r = max as f32 / long as f32;
        let w = ((self.width as f32 * r) as u32).max(2) & !1;
        let h = ((self.height as f32 * r) as u32).max(2) & !1;
        self.resize(w, h)
    }
}

/// Night boost: for a dark picture (night, infrared), stretch the
/// brightness range to the full scale and lift the shadows, so object
/// recognition sees more. Returns None when the picture isn't dark.
pub fn low_light_boost(f: &Frame) -> Option<Frame> {
    let mut hist = [0u32; 256];
    let mut n = 0u32;
    for p in f.rgb.chunks_exact(3).step_by(5) {
        let y = (0.3 * p[0] as f32 + 0.59 * p[1] as f32 + 0.11 * p[2] as f32) as usize;
        hist[y.min(255)] += 1;
        n += 1;
    }
    if n == 0 {
        return None;
    }
    let mean = hist
        .iter()
        .enumerate()
        .map(|(v, c)| v as f64 * *c as f64)
        .sum::<f64>()
        / n as f64;
    if mean >= 70.0 {
        return None;
    }
    let pct = |p: f64| {
        let target = (n as f64 * p) as u32;
        let mut acc = 0;
        for (v, c) in hist.iter().enumerate() {
            acc += c;
            if acc >= target {
                return v as f32;
            }
        }
        255.0
    };
    let (lo, hi) = (pct(0.01), pct(0.995).max(pct(0.01) + 24.0));
    // Dark but already using the full range (e.g. dark clothes in daylight): leave it.
    if mean >= 45.0 && hi >= 235.0 {
        return None;
    }
    // Brightness curve: stretch, then a gamma that lifts shadows more when darker.
    let gamma = if mean < 35.0 { 0.6 } else { 0.75 };
    let mut lut = [0u8; 256];
    for (v, o) in lut.iter_mut().enumerate() {
        let x = ((v as f32 - lo) / (hi - lo)).clamp(0.0, 1.0);
        *o = (x.powf(gamma) * 255.0).round() as u8;
    }
    let rgb: Vec<u8> = f.rgb.iter().map(|&v| lut[v as usize]).collect();
    Some(Frame {
        width: f.width,
        height: f.height,
        rgb: Arc::new(rgb),
    })
}

/// Decode a JPEG picture into a frame.
pub fn decode_jpeg(jpg: &[u8]) -> Result<Frame> {
    let mut d = jpeg_decoder::Decoder::new(jpg);
    let pixels = d.decode()?;
    let info = d.info().ok_or_else(|| anyhow::anyhow!("no picture info"))?;
    let rgb = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => pixels,
        jpeg_decoder::PixelFormat::L8 => pixels.iter().flat_map(|&v| [v, v, v]).collect(),
        f => anyhow::bail!("unsupported picture format {f:?}"),
    };
    Ok(Frame {
        width: info.width as u32,
        height: info.height as u32,
        rgb: Arc::new(rgb),
    })
}

/// Make a picture smaller for slow connections: decode, scale down to at
/// most `max_w` pixels wide and encode again at `quality`.
pub fn shrink_jpeg(jpg: &[u8], max_w: u32, quality: u8) -> Result<Vec<u8>> {
    decode_jpeg(jpg)?.fit(max_w.max(160)).jpeg(quality)
}

pub fn encode_jpeg(rgb: &[u8], w: u32, h: u32, quality: u8) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let enc = jpeg_encoder::Encoder::new(&mut out, quality);
    enc.encode(rgb, w as u16, h as u16, jpeg_encoder::ColorType::Rgb)?;
    Ok(out)
}

pub fn resize_rgb(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let mut out = vec![0u8; (dw * dh * 3) as usize];
    let xr = sw as f32 / dw as f32;
    let yr = sh as f32 / dh as f32;
    for y in 0..dh {
        let fy = ((y as f32 + 0.5) * yr - 0.5).clamp(0.0, (sh - 1) as f32);
        let y0 = fy as u32;
        let y1 = (y0 + 1).min(sh - 1);
        let wy = fy - y0 as f32;
        for x in 0..dw {
            let fx = ((x as f32 + 0.5) * xr - 0.5).clamp(0.0, (sw - 1) as f32);
            let x0 = fx as u32;
            let x1 = (x0 + 1).min(sw - 1);
            let wx = fx - x0 as f32;
            for c in 0..3 {
                let p = |xx: u32, yy: u32| src[((yy * sw + xx) * 3 + c) as usize] as f32;
                let top = p(x0, y0) * (1.0 - wx) + p(x1, y0) * wx;
                let bot = p(x0, y1) * (1.0 - wx) + p(x1, y1) * wx;
                out[((y * dw + x) * 3 + c) as usize] = (top * (1.0 - wy) + bot * wy).round() as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_resize_encode() {
        let (w, h) = (64u32, 48u32);
        let rgb: Vec<u8> = (0..w * h * 3).map(|i| (i % 251) as u8).collect();
        let f = Frame {
            width: w,
            height: h,
            rgb: Arc::new(rgb),
        };
        let r = f.resize(32, 24);
        assert_eq!(r.rgb.len(), 32 * 24 * 3);
        let j = f.jpeg(80).unwrap();
        assert_eq!(&j[..2], &[0xFF, 0xD8]);
        let small = shrink_jpeg(&j, 32, 50).unwrap();
        assert!(small.len() < j.len());
        assert_eq!(f.fit(32).width, 32);
    }
}

const COLOR_WORDS: &[&str] = &[
    "black", "white", "grey", "gray", "silver", "red", "orange", "yellow", "green", "blue",
    "purple", "pink", "brown", "beige", "gold", "tan", "maroon", "navy",
];

/// The first colour word in a description, normalized ("gray" → "grey").
pub fn color_word(text: &str) -> Option<&'static str> {
    text.split(|c: char| !c.is_alphabetic())
        .map(|w| w.to_lowercase())
        .find_map(|w| {
            COLOR_WORDS
                .iter()
                .find(|c| **c == w)
                .map(|c| normalize_color(c))
        })
}

pub fn normalize_color(c: &str) -> &'static str {
    match c {
        "gray" | "grey" => "grey",
        "silver" => "silver",
        "tan" | "beige" => "beige",
        "maroon" => "red",
        "navy" => "blue",
        "gold" => "yellow",
        other => COLOR_WORDS
            .iter()
            .find(|w| **w == other)
            .copied()
            .unwrap_or("grey"),
    }
}

/// Name a pixel's colour (HSV buckets).
fn pixel_color(r: u8, g: u8, b: u8) -> &'static str {
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let v = max;
    let d = max - min;
    let s = if max > 0.0 { d / max } else { 0.0 };
    if v < 0.22 {
        return "black";
    }
    if s < 0.18 {
        return if v > 0.72 {
            "white"
        } else if v > 0.45 {
            "silver"
        } else {
            "grey"
        };
    }
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let h = if h < 0.0 { h + 360.0 } else { h };
    match h {
        h if !(15.0..330.0).contains(&h) => {
            if s < 0.45 && v > 0.6 {
                "pink"
            } else {
                "red"
            }
        }
        h if h < 40.0 => {
            if v < 0.6 {
                "brown"
            } else {
                "orange"
            }
        }
        h if h < 70.0 => {
            if v < 0.5 {
                "brown"
            } else {
                "yellow"
            }
        }
        h if h < 165.0 => "green",
        h if h < 255.0 => "blue",
        h if h < 290.0 => "purple",
        _ => "pink",
    }
}

impl Frame {
    /// The main colour of an object: the most common colour name in the
    /// middle of its box (the edges are usually background).
    pub fn color_name(&self, x: f32, y: f32, w: f32, h: f32) -> Option<&'static str> {
        let (fw, fh) = (self.width as f32, self.height as f32);
        let x0 = ((x + w * 0.2) * fw).clamp(0.0, fw - 1.0) as u32;
        let x1 = ((x + w * 0.8) * fw).clamp(0.0, fw) as u32;
        let y0 = ((y + h * 0.2) * fh).clamp(0.0, fh - 1.0) as u32;
        let y1 = ((y + h * 0.8) * fh).clamp(0.0, fh) as u32;
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let step = (((x1 - x0) * (y1 - y0)) as f32 / 4000.0).sqrt().max(1.0) as u32;
        let mut counts: std::collections::HashMap<&'static str, u32> = Default::default();
        for yy in (y0..y1).step_by(step as usize) {
            for xx in (x0..x1).step_by(step as usize) {
                let i = ((yy * self.width + xx) * 3) as usize;
                let c = pixel_color(self.rgb[i], self.rgb[i + 1], self.rgb[i + 2]);
                *counts.entry(c).or_default() += 1;
            }
        }
        // Colourful pixels count a bit more: a red car has grey windows
        // and black tyres, but it is still "red".
        counts
            .into_iter()
            .map(|(c, n)| {
                let w = if matches!(c, "black" | "white" | "grey" | "silver") {
                    1.0
                } else {
                    1.6
                };
                (c, n as f32 * w)
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(c, _)| c)
    }
}

#[cfg(test)]
mod color_tests {
    use super::*;

    #[test]
    fn names_colours() {
        assert_eq!(pixel_color(200, 30, 30), "red");
        assert_eq!(pixel_color(240, 240, 240), "white");
        assert_eq!(pixel_color(20, 20, 25), "black");
        assert_eq!(pixel_color(30, 60, 200), "blue");
        assert_eq!(pixel_color(140, 140, 140), "silver");
        assert_eq!(color_word("A White delivery van"), Some("white"));
        assert_eq!(color_word("gray cat"), Some("grey"));
        let f = Frame {
            width: 4,
            height: 4,
            rgb: Arc::new([200u8, 20, 20].repeat(16)),
        };
        assert_eq!(f.color_name(0.0, 0.0, 1.0, 1.0), Some("red"));
    }
}
