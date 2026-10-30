//! A color as text in the formats people paste into code and design tools.

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorFormat {
    #[default]
    Hex,
    Rgb,
    Hsl,
    Hsv,
    Cmyk,
}

impl ColorFormat {
    pub const ALL: [ColorFormat; 5] = [
        ColorFormat::Hex,
        ColorFormat::Rgb,
        ColorFormat::Hsl,
        ColorFormat::Hsv,
        ColorFormat::Cmyk,
    ];

    /// Name on the buttons.
    pub fn label(self) -> &'static str {
        match self {
            ColorFormat::Hex => "HEX",
            ColorFormat::Rgb => "RGB",
            ColorFormat::Hsl => "HSL",
            ColorFormat::Hsv => "HSV",
            ColorFormat::Cmyk => "CMYK",
        }
    }

    /// Value in the `[capture] color_format` key.
    pub fn key(self) -> &'static str {
        match self {
            ColorFormat::Hex => "hex",
            ColorFormat::Rgb => "rgb",
            ColorFormat::Hsl => "hsl",
            ColorFormat::Hsv => "hsv",
            ColorFormat::Cmyk => "cmyk",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|f| *f == self).unwrap_or(0)
    }

    /// Text of a color given as red, green, blue.
    pub fn text(self, [r, g, b]: [u8; 3]) -> String {
        let (fr, fg, fb) = (
            f32::from(r) / 255.0,
            f32::from(g) / 255.0,
            f32::from(b) / 255.0,
        );
        let (max, min) = (fr.max(fg).max(fb), fr.min(fg).min(fb));
        let delta = max - min;
        let hue = if delta == 0.0 {
            0.0
        } else if max == fr {
            60.0 * ((fg - fb) / delta).rem_euclid(6.0)
        } else if max == fg {
            60.0 * ((fb - fr) / delta + 2.0)
        } else {
            60.0 * ((fr - fg) / delta + 4.0)
        };
        let pct = |v: f32| (v * 100.0).round() as u32;
        match self {
            ColorFormat::Hex => format!("#{r:02X}{g:02X}{b:02X}"),
            ColorFormat::Rgb => format!("rgb({r}, {g}, {b})"),
            ColorFormat::Hsl => {
                let l = (max + min) / 2.0;
                let s = if delta == 0.0 {
                    0.0
                } else {
                    delta / (1.0 - (2.0 * l - 1.0).abs())
                };
                format!("hsl({}, {}%, {}%)", hue.round() as u32, pct(s), pct(l))
            }
            ColorFormat::Hsv => {
                let s = if max == 0.0 { 0.0 } else { delta / max };
                format!("hsv({}, {}%, {}%)", hue.round() as u32, pct(s), pct(max))
            }
            ColorFormat::Cmyk => {
                let k = 1.0 - max;
                let part = |v: f32| {
                    if k >= 1.0 {
                        0
                    } else {
                        pct((1.0 - v - k) / (1.0 - k))
                    }
                };
                format!("cmyk({}, {}, {}, {})", part(fr), part(fg), part(fb), pct(k))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_colors_read_the_same_in_every_format() {
        let red = [255, 0, 0];
        assert_eq!(ColorFormat::Hex.text([0x29, 0x5B, 0xE8]), "#295BE8");
        assert_eq!(ColorFormat::Rgb.text(red), "rgb(255, 0, 0)");
        assert_eq!(ColorFormat::Hsl.text(red), "hsl(0, 100%, 50%)");
        assert_eq!(ColorFormat::Hsv.text(red), "hsv(0, 100%, 100%)");
        assert_eq!(ColorFormat::Cmyk.text(red), "cmyk(0, 100, 100, 0)");
        assert_eq!(ColorFormat::Hsl.text([0, 0, 255]), "hsl(240, 100%, 50%)");
        assert_eq!(ColorFormat::Hsl.text([128, 128, 128]), "hsl(0, 0%, 50%)");
        assert_eq!(ColorFormat::Cmyk.text([0, 0, 0]), "cmyk(0, 0, 0, 100)");
    }
}
