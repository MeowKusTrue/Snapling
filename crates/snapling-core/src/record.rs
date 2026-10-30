//! Screen recording that needs no system: a GIF written frame by frame, and long shots stitched
//! from what scrolled past.

use crate::ocr::resize;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

/// Longer side of a GIF: bigger ones grow far faster than they gain.
const GIF_SIDE: u32 = 960;
/// Quality of the color reduction, 1 best and slowest, 30 fast enough for live capture.
const GIF_SPEED: i32 = 30;

pub struct GifWriter {
    enc: gif::Encoder<BufWriter<File>>,
    src: (u32, u32),
    size: (u32, u32),
}

/// Size of a GIF for a `width`×`height` capture.
pub fn gif_size(width: u32, height: u32) -> (u32, u32) {
    let k = (GIF_SIDE as f32 / width.max(height) as f32).min(1.0);
    let side = |v: u32| ((v as f32 * k) as u32).max(1);
    (side(width), side(height))
}

impl GifWriter {
    pub fn new(path: &Path, width: u32, height: u32) -> Result<Self, String> {
        let file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let size = gif_size(width, height);
        let mut enc = gif::Encoder::new(BufWriter::new(file), size.0 as u16, size.1 as u16, &[])
            .map_err(|e| e.to_string())?;
        enc.set_repeat(gif::Repeat::Infinite)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            enc,
            src: (width, height),
            size,
        })
    }

    /// One BGRA frame shown for `ms`.
    pub fn push(&mut self, bgra: &[u8], ms: u32) -> Result<(), String> {
        let (w, h) = self.size;
        let mut rgba = if self.size == self.src {
            bgra.to_vec()
        } else {
            resize(bgra, self.src.0, self.src.1, w, h)
        };
        for px in rgba.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
        }
        let mut frame = gif::Frame::from_rgba_speed(w as u16, h as u16, &mut rgba, GIF_SPEED);
        frame.delay = ((ms + 5) / 10).clamp(2, u32::from(u16::MAX)) as u16;
        self.enc.write_frame(&frame).map_err(|e| e.to_string())
    }
}

/// Rows at least this many must line up, or two frames are not the same page.
const MIN_OVERLAP: usize = 16;
/// Rows that may differ inside an overlap: a blinking caret, a playing animation.
const MISS_RATE: usize = 20;

fn row_hash(row: &[u8]) -> u64 {
    row.iter().fold(0xcbf29ce484222325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
    })
}

/// A shot of equal width that grows down as the page scrolls. Rows that stay put, sticky headers
/// and footers, are not repeated.
pub struct Stitcher {
    width: usize,
    height: usize,
    out: Vec<u8>,
    last: Vec<u64>,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Added {
    /// New rows went in.
    Grew(usize),
    /// The picture did not move: the end of the page.
    Same,
    /// No overlap: it scrolled too far or the page changed.
    Lost,
}

impl Stitcher {
    fn hashes(&self, bgra: &[u8]) -> Vec<u64> {
        // The right edge is left out: a scrollbar moves on every step.
        let row = self.width * 4;
        let used = row - (row / 20).min(row.saturating_sub(4));
        bgra.chunks_exact(row)
            .map(|r| row_hash(&r[..used]))
            .collect()
    }

    pub fn new(bgra: &[u8], width: u32, height: u32) -> Self {
        let mut s = Self {
            width: width as usize,
            height: height as usize,
            out: bgra.to_vec(),
            last: Vec::new(),
        };
        s.last = s.hashes(bgra);
        s
    }

    pub fn rows(&self) -> usize {
        self.out.len() / (self.width * 4)
    }

    pub fn add(&mut self, bgra: &[u8]) -> Added {
        let h = self.height;
        let next = self.hashes(bgra);
        let top = self
            .last
            .iter()
            .zip(&next)
            .take_while(|(a, b)| a == b)
            .count();
        if top == h {
            return Added::Same;
        }
        let bottom = self
            .last
            .iter()
            .rev()
            .zip(next.iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let end = h - bottom;
        let Some(shift) = self.find_shift(&next, top, end) else {
            return Added::Lost;
        };
        let row = self.width * 4;
        self.out.truncate(self.out.len() - bottom * row);
        self.out.extend_from_slice(&bgra[(end - shift) * row..]);
        self.last = next;
        Added::Grew(shift)
    }

    /// Smallest move down that lines the scrolled part of two frames up.
    fn find_shift(&self, next: &[u64], top: usize, end: usize) -> Option<usize> {
        let band = end.checked_sub(top)?;
        (1..band.saturating_sub(MIN_OVERLAP) + 1).find(|&shift| {
            let overlap = band - shift;
            let misses = (0..overlap)
                .filter(|i| self.last[top + shift + i] != next[top + i])
                .count();
            misses * MISS_RATE <= overlap
        })
    }

    pub fn finish(self) -> (Vec<u8>, u32, u32) {
        let rows = self.rows() as u32;
        (self.out, self.width as u32, rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page where every row differs; `at` is the top of the view into it.
    fn view(at: usize, w: usize, h: usize, header: usize, footer: usize) -> Vec<u8> {
        let mut out = Vec::new();
        for y in 0..h {
            let line = if y < header {
                1_000_000 + y
            } else if y >= h - footer {
                2_000_000 + y
            } else {
                at + y
            };
            for x in 0..w {
                let v = (line * 37 + line / 251 + x) as u32;
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        out
    }

    #[test]
    fn scrolled_frames_join_without_repeats() {
        let (w, h) = (40, 200);
        let mut s = Stitcher::new(&view(0, w, h, 0, 0), w as u32, h as u32);
        assert_eq!(s.add(&view(60, w, h, 0, 0)), Added::Grew(60));
        assert_eq!(s.add(&view(130, w, h, 0, 0)), Added::Grew(70));
        assert_eq!(s.add(&view(130, w, h, 0, 0)), Added::Same);
        let (px, _, rows) = s.finish();
        assert_eq!(rows, 330);
        assert_eq!(px, view(0, w, 330, 0, 0));
    }

    #[test]
    fn sticky_header_and_footer_stay_single() {
        let (w, h) = (40, 200);
        let (head, foot) = (30, 20);
        let mut s = Stitcher::new(&view(0, w, h, head, foot), w as u32, h as u32);
        assert_eq!(s.add(&view(50, w, h, head, foot)), Added::Grew(50));
        let (px, _, rows) = s.finish();
        assert_eq!(rows, 250);
        let row = w * 4;
        assert_eq!(px[..row], view(0, w, h, head, foot)[..row], "the header");
        let last = (rows as usize - 1) * row;
        assert_eq!(
            px[last..],
            view(0, w, h, head, foot)[(h - 1) * row..],
            "the footer"
        );
    }

    #[test]
    fn a_jump_past_the_view_is_lost_not_glued() {
        let (w, h) = (40, 100);
        let mut s = Stitcher::new(&view(0, w, h, 0, 0), w as u32, h as u32);
        assert_eq!(s.add(&view(5000, w, h, 0, 0)), Added::Lost);
        assert_eq!(s.rows(), 100);
    }

    #[test]
    fn gif_frames_are_written_and_big_ones_shrink() {
        assert_eq!(gif_size(1920, 1080), (960, 540));
        assert_eq!(gif_size(300, 200), (300, 200));
        let dir = std::env::temp_dir().join(format!("snapling-gif-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.gif");
        let mut g = GifWriter::new(&path, 64, 32).unwrap();
        for i in 0..3u8 {
            g.push(&[i * 40, 10, 200, 255].repeat(64 * 32), 66).unwrap();
        }
        drop(g);
        let img = image::open(&path).unwrap();
        assert_eq!((img.width(), img.height()), (64, 32));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
