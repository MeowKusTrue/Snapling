//! Editable project next to a saved shot: `shot.png` → `shot.snapling.json`. Holds the
//! original pixels and the annotations, so the editor can move or change them later.

use crate::annotate::{Doc, Pixmap};
use crate::config::Format;
use crate::i18n::tf;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const EXT: &str = ".snapling.json";
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct File {
    snapling_project: u32,
    app: String,
    /// The finished image this project belongs to, a file name in the same folder.
    image: String,
    width: u32,
    height: u32,
    #[serde(flatten)]
    doc: Doc,
    /// Original pixels as PNG, base64.
    original: String,
}

pub struct Project {
    pub base: Pixmap,
    pub app: String,
    pub doc: Doc,
    /// The finished image, saved over on the next save.
    pub image: PathBuf,
}

pub fn path_for(image: &Path) -> PathBuf {
    let stem = image.file_stem().unwrap_or_default().to_string_lossy();
    image.with_file_name(format!("{stem}{EXT}"))
}

pub fn is_project(path: &Path) -> bool {
    path.to_string_lossy().to_lowercase().ends_with(EXT)
}

/// The format a saved image was written in, by its extension.
pub fn format_of(image: &Path) -> Format {
    let ext = image.extension().unwrap_or_default().to_string_lossy();
    match ext.to_lowercase().as_str() {
        "jpg" | "jpeg" => Format::Jpeg,
        "webp" => Format::Webp,
        _ => Format::Png,
    }
}

/// Writes the project for `image`; returns its path.
pub fn save(image: &Path, base: &Pixmap, app: &str, doc: &Doc) -> Result<PathBuf, String> {
    use image::ImageEncoder;
    let (w, h) = (base.width(), base.height());
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(base.data(), w, h, image::ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())?;
    let file = File {
        snapling_project: VERSION,
        app: app.into(),
        image: image
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into(),
        width: w,
        height: h,
        doc: doc.clone(),
        original: b64(&png),
    };
    let json = serde_json::to_vec(&file).map_err(|e| e.to_string())?;
    let path = path_for(image);
    let part = path.with_extension("json.part");
    std::fs::write(&part, json)
        .and_then(|()| std::fs::rename(&part, &path))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

pub fn load(path: &Path) -> Result<Project, String> {
    let name = path.display();
    let bad = || tf("{0}: проект повреждён", &[("0", &name)]);
    let data = std::fs::read(path).map_err(|e| format!("{name}: {e}"))?;
    let file: File = serde_json::from_slice(&data).map_err(|_| bad())?;
    if file.snapling_project > VERSION {
        return Err(tf(
            "{0}: проект из более новой версии Snapling, обновите программу",
            &[("0", &name)],
        ));
    }
    let png = unb64(&file.original).ok_or_else(bad)?;
    let img = image::load_from_memory(&png)
        .map_err(|_| bad())?
        .into_rgba8();
    if img.dimensions() != (file.width, file.height) {
        return Err(bad());
    }
    let size = tiny_skia::IntSize::from_wh(file.width, file.height).ok_or_else(bad)?;
    let base = Pixmap::from_vec(img.into_raw(), size).ok_or_else(bad)?;
    Ok(Project {
        base,
        app: file.app,
        doc: file.doc,
        image: path.with_file_name(&file.image),
    })
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = u32::from(c[0]) << 16
            | u32::from(*c.get(1).unwrap_or(&0)) << 8
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn unb64(text: &str) -> Option<Vec<u8>> {
    let text = text.trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for &ch in text {
        acc = acc << 6 | B64.iter().position(|&b| b == ch)? as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotate::{ArrowStyle, Element, Kind, Pt};

    #[test]
    fn base64_round_trips() {
        for n in 0..10 {
            let data: Vec<u8> = (0..n).map(|i: u8| i.wrapping_mul(37)).collect();
            assert_eq!(unb64(&b64(&data)).unwrap(), data);
        }
        assert_eq!(b64(b"Man"), "TWFu");
        assert_eq!(b64(b"Ma"), "TWE=");
    }

    #[test]
    fn project_round_trips_and_rejects_damage() {
        let dir = std::env::temp_dir().join(format!("snapling-proj-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let image = dir.join("shot.png");
        let mut base = Pixmap::new(3, 2).unwrap();
        base.fill(tiny_skia::Color::from_rgba8(10, 20, 30, 255));
        let pt = |x, y| Pt { x, y };
        let doc = Doc {
            elements: vec![
                Element {
                    kind: Kind::Arrow(pt(0.0, 0.0), pt(2.0, 1.0), ArrowStyle::default()),
                    color: [255, 0, 0],
                    size: 5.0,
                },
                Element {
                    kind: Kind::Text(pt(1.0, 1.0), "привет".into(), "Arial", Some((40.0, 20.0))),
                    color: [0, 0, 0],
                    size: 3.0,
                },
            ],
            crop: Some((pt(0.0, 0.0), pt(2.0, 2.0))),
        };
        let path = save(&image, &base, "app", &doc).unwrap();
        assert_eq!(path, dir.join("shot.snapling.json"));
        assert!(is_project(&path));
        let p = load(&path).unwrap();
        assert_eq!(p.doc, doc);
        assert_eq!(p.base.data(), base.data());
        assert_eq!(p.image, image);
        std::fs::write(&path, b"{\"snapling_project\":1").unwrap();
        assert!(load(&path).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[test]
fn old_text_without_frame_loads() {
    let k: crate::annotate::Kind =
        serde_json::from_str(r#"{"text":[{"x":1.0,"y":2.0},"a",""]}"#).unwrap();
    assert!(matches!(k, crate::annotate::Kind::Text(_, _, _, None)));
}
