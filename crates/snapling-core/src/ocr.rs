//! Screenshot preparation for text recognition.

use std::borrow::Cow;

/// How many times to enlarge a shot: engines lose small screen text, but big shots get slow.
pub fn scale(width: u32, height: u32, max_side: u32) -> u32 {
    let pixels = u64::from(width) * u64::from(height);
    let k: u32 = if pixels <= 1_000_000 {
        3
    } else if pixels <= 4_000_000 {
        2
    } else {
        1
    };
    k.min(max_side / width.max(height).max(1)).max(1)
}

/// Four-byte pixels at a new size.
pub fn resize(px: &[u8], width: u32, height: u32, w: u32, h: u32) -> Vec<u8> {
    use image::imageops::{FilterType, resize};
    match image::RgbaImage::from_raw(width, height, px.to_vec()) {
        Some(src) => resize(&src, w, h, FilterType::CatmullRom).into_raw(),
        None => vec![0; (w * h * 4) as usize],
    }
}

/// BGRA `k` times larger; the same pixels when `k` is 1.
pub fn enlarge(bgra: &[u8], width: u32, height: u32, k: u32) -> (Cow<'_, [u8]>, u32, u32) {
    if k < 2 {
        return (Cow::Borrowed(bgra), width, height);
    }
    let (w, h) = (width * k, height * k);
    (Cow::Owned(resize(bgra, width, height, w, h)), w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_shots_grow_more_and_stay_within_the_engine_limit() {
        assert_eq!(scale(600, 40, 10_000), 3);
        assert_eq!(scale(1920, 1080, 10_000), 2);
        assert_eq!(scale(3840, 2160, 10_000), 1);
        assert_eq!(scale(4000, 20, 10_000), 2);
        assert_eq!(scale(20_000, 20, 10_000), 1);
        let px = [10, 20, 30, 255].repeat(6);
        assert_eq!(enlarge(&px, 3, 2, 2).0.len(), 6 * 4 * 4);
        assert_eq!(*enlarge(&px, 3, 2, 1).0, *px);
    }
}
