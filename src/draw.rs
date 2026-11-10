//! Drawing detection boxes and labels on a picture (for notification
//! pictures that look like the alert cards: a box around each recognized
//! object with "PERSON 92%"). A small built-in 5×7 pixel font, scaled up.

use crate::imgutil::Frame;
use std::sync::Arc;

/// 5×7 glyphs, one byte per row (low 5 bits, leftmost pixel = bit 4).
fn glyph(c: char) -> [u8; 7] {
    match c.to_ascii_uppercase() {
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'D' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0A],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        '%' => [0x18, 0x19, 0x02, 0x04, 0x08, 0x13, 0x03],
        '-' => [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C],
        ':' => [0x00, 0x0C, 0x0C, 0x00, 0x0C, 0x0C, 0x00],
        _ => [0; 7],
    }
}

struct Canvas<'a> {
    w: i32,
    h: i32,
    px: &'a mut Vec<u8>,
}

impl Canvas<'_> {
    fn set(&mut self, x: i32, y: i32, c: [u8; 3]) {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            let i = ((y * self.w + x) * 3) as usize;
            self.px[i..i + 3].copy_from_slice(&c);
        }
    }
    fn fill(&mut self, x: i32, y: i32, w: i32, h: i32, c: [u8; 3]) {
        for yy in y.max(0)..(y + h).min(self.h) {
            for xx in x.max(0)..(x + w).min(self.w) {
                self.set(xx, yy, c);
            }
        }
    }
    fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, t: i32, c: [u8; 3]) {
        self.fill(x, y, w, t, c);
        self.fill(x, y + h - t, w, t, c);
        self.fill(x, y, t, h, c);
        self.fill(x + w - t, y, t, h, c);
    }
    fn text(&mut self, x: i32, y: i32, s: &str, scale: i32, c: [u8; 3]) {
        for (i, ch) in s.chars().enumerate() {
            let g = glyph(ch);
            let ox = x + i as i32 * 6 * scale;
            for (row, bits) in g.iter().enumerate() {
                for col in 0..5 {
                    if bits & (0x10 >> col) != 0 {
                        self.fill(ox + col * scale, y + row as i32 * scale, scale, scale, c);
                    }
                }
            }
        }
    }
}

pub fn text_width(s: &str, scale: i32) -> i32 {
    s.chars().count() as i32 * 6 * scale - scale
}

/// One box to draw: normalized position (0..1) and its label.
pub struct Mark<'a> {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub label: &'a str,
}

/// The picture with a green box and a "PERSON 92%" tag on each object.
pub fn boxes(f: &Frame, marks: &[Mark]) -> Frame {
    const GREEN: [u8; 3] = [46, 204, 113];
    const DARK: [u8; 3] = [10, 20, 14];
    let mut px = (*f.rgb).clone();
    let (w, h) = (f.width as i32, f.height as i32);
    let mut c = Canvas { w, h, px: &mut px };
    let t = (w / 320).clamp(2, 6);
    let scale = (w / 400).clamp(2, 5);
    for m in marks {
        let (x, y) = ((m.x * w as f32) as i32, (m.y * h as f32) as i32);
        let (bw, bh) = (
            ((m.w * w as f32) as i32).max(2 * t),
            ((m.h * h as f32) as i32).max(2 * t),
        );
        c.rect(x, y, bw, bh, t, GREEN);
        // The tag sits above the box, or inside it at the top edge.
        let tw = text_width(m.label, scale) + 4 * scale;
        let th = 7 * scale + 4 * scale;
        let ty = if y - th >= 0 { y - th } else { y };
        let tx = x.min(w - tw).max(0);
        c.fill(tx, ty, tw, th, GREEN);
        c.text(tx + 2 * scale, ty + 2 * scale, m.label, scale, DARK);
    }
    Frame {
        width: f.width,
        height: f.height,
        rgb: Arc::new(px),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_a_tagged_box() {
        let f = Frame {
            width: 640,
            height: 360,
            rgb: Arc::new(vec![0; 640 * 360 * 3]),
        };
        let out = boxes(
            &f,
            &[Mark {
                x: 0.25,
                y: 0.5,
                w: 0.25,
                h: 0.4,
                label: "PERSON 92%",
            }],
        );
        let at = |x: u32, y: u32| {
            let i = ((y * 640 + x) * 3) as usize;
            [out.rgb[i], out.rgb[i + 1], out.rgb[i + 2]]
        };
        assert_eq!(at(160, 250), [46, 204, 113], "box edge");
        assert_eq!(at(240, 300), [0, 0, 0], "inside untouched");
        // The tag above the box has green and dark (text) pixels.
        let tag: Vec<[u8; 3]> = (160..260)
            .flat_map(|x| (158..180).map(move |y| (x, y)))
            .map(|(x, y)| at(x, y))
            .collect();
        assert!(tag.contains(&[46, 204, 113]) && tag.contains(&[10, 20, 14]));
    }
}
