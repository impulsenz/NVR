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

    /// Crop around a normalized box with a fixed picture shape (`aspect` = w/h),
    /// so thumbnails are uniform. The box is padded, made at least
    /// `min_frac` of the frame width, and kept inside the frame.
    pub fn crop_aspect(
        &self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        pad: f32,
        min_frac: f32,
        aspect: f32,
    ) -> Frame {
        let (fw, fh) = (self.width as f32, self.height as f32);
        let mut cw = (w * fw * (1.0 + 2.0 * pad)).max(fw * min_frac);
        let mut ch = h * fh * (1.0 + 2.0 * pad);
        if cw / ch.max(1.0) < aspect {
            cw = ch * aspect;
        } else {
            ch = cw / aspect;
        }
        // Too big for the frame: shrink while keeping the shape.
        let k = (fw / cw).min(fh / ch).min(1.0);
        let (cw, ch) = ((cw * k).max(16.0), (ch * k).max(16.0));
        let cx = (x + w / 2.0) * fw;
        let cy = (y + h / 2.0) * fh;
        let x0 = (cx - cw / 2.0).clamp(0.0, (fw - cw).max(0.0)) as u32;
        let y0 = (cy - ch / 2.0).clamp(0.0, (fh - ch).max(0.0)) as u32;
        let (cw, ch) = (
            (cw as u32).min(self.width - x0),
            (ch as u32).min(self.height - y0),
        );
        let mut out = Vec::with_capacity((cw * ch * 3) as usize);
        for row in y0..y0 + ch {
            let start = ((row * self.width + x0) * 3) as usize;
            out.extend_from_slice(&self.rgb[start..start + (cw * 3) as usize]);
        }
        Frame {
            width: cw,
            height: ch,
            rgb: Arc::new(out),
        }
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
        let c = f.crop_aspect(0.9, 0.9, 0.2, 0.2, 0.1, 0.2, 16.0 / 9.0);
        assert!(c.width <= w && c.height <= h && c.width >= 10);
        assert_eq!(c.rgb.len() as u32, c.width * c.height * 3);
        let r = f.resize(32, 24);
        assert_eq!(r.rgb.len(), 32 * 24 * 3);
        let j = f.jpeg(80).unwrap();
        assert_eq!(&j[..2], &[0xFF, 0xD8]);
        assert_eq!(f.fit(32).width, 32);
        // A tall box still gives a 16:9 picture.
        let t = f.crop_aspect(0.4, 0.1, 0.05, 0.8, 0.1, 0.3, 16.0 / 9.0);
        assert!(
            (t.width as f32 / t.height as f32 - 16.0 / 9.0).abs() < 0.15,
            "{}x{}",
            t.width,
            t.height
        );
    }
}
