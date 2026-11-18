//! Local QR decoding. Payloads are data, never commands to execute.
use crate::i18n::t;

pub fn decode(bgra: &[u8], width: u32, height: u32) -> Result<Vec<String>, String> {
    let bytes = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4));
    if width == 0 || height == 0 || bytes != Some(bgra.len()) {
        return Err(t("Неверный размер изображения QR-кода").into());
    }
    let mut image =
        rqrr::PreparedImage::prepare_from_greyscale(width as usize, height as usize, |x, y| {
            let i = (y * width as usize + x) * 4;
            ((u32::from(bgra[i + 2]) * 77 + u32::from(bgra[i + 1]) * 150 + u32::from(bgra[i]) * 29)
                >> 8) as u8
        });
    let mut texts = Vec::new();
    for grid in image.detect_grids() {
        if let Ok((_, text)) = grid.decode()
            && !text.is_empty()
            && !texts.contains(&text)
        {
            texts.push(text);
        }
    }
    if texts.is_empty() {
        Err(t(
            "QR-код не найден или не прочитан. Выдели код целиком, оставив немного места вокруг.",
        )
        .into())
    } else {
        Ok(texts)
    }
}

/// Only explicit HTTP(S) links get an Open action; other payloads stay copyable text.
pub fn web_link(text: &str) -> bool {
    let rest = text
        .strip_prefix("https://")
        .or_else(|| text.strip_prefix("http://"));
    rest.is_some_and(|rest| {
        !rest.is_empty()
            && !rest.starts_with('/')
            && !text.chars().any(|c| c.is_whitespace() || c.is_control())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_generated_qr_at_multiple_scales_and_rotations() {
        // Generated locally with python-qrcode 8.2; this fixture contains no user data.
        let matrix: Vec<_> = include_str!("../tests/data/qr-example.txt")
            .lines()
            .map(str::as_bytes)
            .collect();
        let n = matrix.len();
        for scale in [2, 4] {
            for rotated in [false, true] {
                let side = n * scale;
                let mut pixels = vec![255; side * side * 4];
                for y in 0..side {
                    for x in 0..side {
                        let (mx, my) = if rotated {
                            (y / scale, n - 1 - x / scale)
                        } else {
                            (x / scale, y / scale)
                        };
                        if matrix[my][mx] == b'1' {
                            let at = (y * side + x) * 4;
                            pixels[at..at + 3].fill(0);
                        }
                    }
                }
                assert_eq!(
                    decode(&pixels, side as u32, side as u32).unwrap(),
                    ["https://example.com/qr"]
                );
            }
        }
    }
    #[test]
    fn rejects_bad_images_and_never_opens_payload_schemes() {
        assert!(decode(&[], 1, 1).is_err());
        assert!(decode(&[255; 32 * 32 * 4], 32, 32).is_err());
        assert!(web_link("https://example.com/path"));
        for s in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "https://",
            "https:///path",
            "https://a\nb",
            "WIFI:S:home;;",
        ] {
            assert!(!web_link(s), "{s}");
        }
    }
}
