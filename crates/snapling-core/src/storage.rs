//! Where and how captures are saved: folder validation, file name templates, unique names.
//! Folder checks follow the idea of EiSen `domain/path_guard.rs` (MIT, © 2026 Tran Trong Thuc),
//! fixed for Windows: EiSen rejected every first-level folder such as `E:\Photos`.

use std::path::{Component, Path, PathBuf};

/// Local time parts, supplied by the platform layer (no date crate in core).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stamp {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

#[cfg(windows)]
const DENIED: &[&str] = &[
    "Windows",
    "Program Files",
    "Program Files (x86)",
    "ProgramData",
];
#[cfg(not(windows))]
const DENIED: &[&str] = &[
    "/System", "/Library", "/usr", "/bin", "/sbin", "/etc", "/dev", "/proc", "/sys", "/boot",
];

/// Shape-only check (no disk access): absolute, not a drive/filesystem root, not a system folder.
pub fn shape_error(dir: &Path) -> Option<String> {
    if dir.as_os_str().is_empty() {
        return Some("папка не задана".into());
    }
    if !dir.is_absolute() {
        return Some(format!("{}: нужен полный путь", dir.display()));
    }
    let normal: Vec<_> = dir
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().to_string()),
            _ => None,
        })
        .collect();
    if dir.components().any(|c| c == Component::ParentDir) {
        return Some(format!("{}: «..» в пути не допускается", dir.display()));
    }
    if normal.is_empty() {
        return Some(format!(
            "{}: корень диска не подходит, выбери папку",
            dir.display()
        ));
    }
    #[cfg(windows)]
    let denied = DENIED.iter().any(|d| normal[0].eq_ignore_ascii_case(d));
    #[cfg(not(windows))]
    let denied = DENIED.iter().any(|d| dir.starts_with(d));
    denied.then(|| format!("{}: системная папка, выбери другую", dir.display()))
}

/// `\\?\E:\x` → `E:\x`, `\\?\UNC\srv\x` → `\\srv\x`: canonicalize's verbatim form confuses users and some APIs.
pub fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match s.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => p,
    }
}

/// Validates, creates if missing and returns the canonical folder.
pub fn prepare_dir(dir: &Path) -> Result<PathBuf, String> {
    if let Some(e) = shape_error(dir) {
        return Err(e);
    }
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("{}: не удалось создать: {e}", dir.display()))?;
    let canon = std::fs::canonicalize(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let canon = strip_verbatim(canon);
    // A symlink/junction may point somewhere forbidden.
    match shape_error(&canon) {
        Some(e) => Err(e),
        None => Ok(canon),
    }
}

fn clean_segment(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .filter(|c| {
            !c.is_control() && !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
        })
        .collect();
    cleaned
        .trim()
        .trim_end_matches('.')
        .chars()
        .take(60)
        .collect()
}

/// Relative path from a template like `{yyyy}-{MM}/Snapling_{HH}-{mm}-{ss}_{app}` (no extension).
/// `/` and `\` make subfolders; empty, `.` and `..` segments are dropped.
pub fn render_name(template: &str, t: Stamp, app: &str) -> PathBuf {
    let app = match clean_segment(app) {
        a if a.is_empty() => "Screen".to_string(),
        a => a,
    };
    let filled = template
        .replace("{yyyy}", &format!("{:04}", t.year))
        .replace("{MM}", &format!("{:02}", t.month))
        .replace("{dd}", &format!("{:02}", t.day))
        .replace("{HH}", &format!("{:02}", t.hour))
        .replace("{mm}", &format!("{:02}", t.minute))
        .replace("{ss}", &format!("{:02}", t.second))
        .replace("{app}", &app);
    let mut out = PathBuf::new();
    for seg in filled.split(['/', '\\']) {
        let seg = clean_segment(seg);
        if !seg.is_empty() && seg != "." && seg != ".." {
            out.push(seg);
        }
    }
    if out.as_os_str().is_empty() {
        out.push("Snapling");
    }
    out
}

/// `name.png`, then `name (2).png`, … until free.
pub fn unique(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = path.extension().map(|s| s.to_string_lossy().to_string());
    for n in 2.. {
        let name = match &ext {
            Some(e) => format!("{stem} ({n}).{e}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = path.with_file_name(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Stamp = Stamp {
        year: 2026,
        month: 10,
        day: 9,
        hour: 7,
        minute: 5,
        second: 3,
    };

    #[test]
    fn template_fills_and_makes_subfolders() {
        let p = render_name(
            "{yyyy}-{MM}/Snapling_{yyyy}-{MM}-{dd}_{HH}-{mm}-{ss}_{app}",
            T,
            "Discord",
        );
        assert_eq!(
            p,
            PathBuf::from("2026-10").join("Snapling_2026-10-09_07-05-03_Discord")
        );
    }

    #[test]
    fn template_cannot_escape_the_folder() {
        let p = render_name("../../{app}/./x", T, "a:b*c?");
        assert_eq!(p, PathBuf::from("abc").join("x"));
        assert_eq!(render_name("", T, ""), PathBuf::from("Snapling"));
        assert_eq!(render_name("{app}", T, "  "), PathBuf::from("Screen"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_shapes() {
        assert_eq!(shape_error(Path::new(r"E:\Photos")), None, "the EiSen bug");
        assert_eq!(shape_error(Path::new(r"D:\Screens\2026")), None);
        assert!(shape_error(Path::new(r"E:\")).is_some());
        assert!(shape_error(Path::new(r"Photos")).is_some());
        assert!(shape_error(Path::new(r"C:\Windows\System32")).is_some());
        assert!(shape_error(Path::new(r"c:\program files\x")).is_some());
        assert!(shape_error(Path::new(r"E:\Photos\..\..")).is_some());
        assert!(shape_error(Path::new("")).is_some());
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_shapes() {
        assert_eq!(shape_error(Path::new("/home/u/Pictures")), None);
        assert!(shape_error(Path::new("/")).is_some());
        assert!(shape_error(Path::new("/etc/x")).is_some());
        assert!(shape_error(Path::new("pics")).is_some());
    }

    #[test]
    fn verbatim_prefix_is_stripped() {
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\E:\Photos")),
            PathBuf::from(r"E:\Photos")
        );
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\UNC\nas\shots")),
            PathBuf::from(r"\\nas\shots")
        );
        assert_eq!(
            strip_verbatim(PathBuf::from("/home/u")),
            PathBuf::from("/home/u")
        );
    }

    #[test]
    fn prepare_creates_and_unique_counts() {
        let base = std::env::temp_dir().join(format!("snapling-storage-{}", std::process::id()));
        let dir = prepare_dir(&base.join("a").join("b")).unwrap();
        assert!(dir.is_dir());
        assert!(!dir.to_string_lossy().starts_with(r"\\?\"));
        let f = dir.join("x.png");
        assert_eq!(unique(f.clone()), f);
        std::fs::write(&f, b"1").unwrap();
        assert_eq!(unique(f.clone()), dir.join("x (2).png"));
        std::fs::write(dir.join("x (2).png"), b"1").unwrap();
        assert_eq!(unique(f), dir.join("x (3).png"));
        let _ = std::fs::remove_dir_all(base);
    }
}

/// Encodes top-down BGRA into `dir/name.<ext>` (unique name, parent folders created).
/// Written to `*.part` first and renamed, so a crash never leaves a half-written image.
pub fn save_bgra(
    bgra: &[u8],
    width: u32,
    height: u32,
    dir: &Path,
    name: &Path,
    format: crate::config::Format,
    jpeg_quality: u8,
) -> Result<PathBuf, String> {
    use crate::config::Format;
    use image::ImageEncoder;

    let rgb: Vec<u8> = bgra
        .chunks_exact(4)
        .flat_map(|p| [p[2], p[1], p[0]])
        .collect();
    let path = unique(dir.join(name).with_extension(format.ext()));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let part = path.with_extension(format!("{}.part", format.ext()));
    let file = std::fs::File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?;
    let out = std::io::BufWriter::new(file);
    let color = image::ExtendedColorType::Rgb8;
    let encoded = match format {
        Format::Png => {
            image::codecs::png::PngEncoder::new(out).write_image(&rgb, width, height, color)
        }
        Format::Webp => image::codecs::webp::WebPEncoder::new_lossless(out)
            .write_image(&rgb, width, height, color),
        Format::Jpeg => {
            image::codecs::jpeg::JpegEncoder::new_with_quality(out, jpeg_quality.clamp(1, 100))
                .write_image(&rgb, width, height, color)
        }
    };
    if let Err(e) = encoded {
        let _ = std::fs::remove_file(&part);
        return Err(format!("{}: {e}", path.display()));
    }
    std::fs::rename(&part, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod save_tests {
    use super::*;
    use crate::config::Format;

    #[test]
    fn saves_every_format_and_decodes_back() {
        let base = std::env::temp_dir().join(format!("snapling-save-{}", std::process::id()));
        let (w, h) = (3u32, 2u32);
        let mut bgra = Vec::new();
        for i in 0..(w * h) {
            bgra.extend_from_slice(&[10 * i as u8, 0, 200, 255]);
        }
        for f in [Format::Png, Format::Webp, Format::Jpeg] {
            let p = save_bgra(&bgra, w, h, &base, Path::new("sub/shot"), f, 90).unwrap();
            assert_eq!(p.extension().unwrap(), f.ext());
            let img = image::open(&p).unwrap().to_rgb8();
            assert_eq!(img.dimensions(), (w, h));
            if f != Format::Jpeg {
                assert_eq!(img.get_pixel(1, 0).0, [200, 0, 10], "BGRA → RGB order");
            }
        }
        let again = save_bgra(&bgra, w, h, &base, Path::new("sub/shot"), Format::Png, 90).unwrap();
        assert!(again.ends_with("shot (2).png"));
        assert!(
            std::fs::read_dir(base.join("sub")).unwrap().all(|e| !e
                .unwrap()
                .path()
                .to_string_lossy()
                .ends_with(".part"))
        );
        let _ = std::fs::remove_dir_all(base);
    }
}
