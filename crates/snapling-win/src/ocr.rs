//! Text recognition with the built-in Windows OCR engine (Windows.Media.Ocr): offline,
//! nothing leaves the machine. Approach from EiSen windows_adapter.rs, but the bitmap is
//! built from memory instead of a temp file.

use snapling_core::i18n::{t, tf};
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::core::HSTRING;

/// Language tags the OCR engine can use here, e.g. `["en-US", "ru-RU"]`.
pub fn languages() -> Vec<String> {
    init();
    OcrEngine::AvailableRecognizerLanguages()
        .map(|v| {
            v.into_iter()
                .filter_map(|l| l.LanguageTag().ok())
                .map(|t| t.to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// `None` when `lang` ("auto" or a tag like "ru") can be used, else a message for the user.
pub fn language_problem(lang: &str) -> Option<String> {
    let have = languages();
    if have.is_empty() {
        return Some(t("OCR: в Windows не установлен ни один язык распознавания текста.").into());
    }
    if lang == "auto"
        || have.iter().any(|t| {
            t.eq_ignore_ascii_case(lang)
                || t.split('-')
                    .next()
                    .is_some_and(|p| p.eq_ignore_ascii_case(lang))
        })
    {
        return None;
    }
    Some(tf(
        "OCR: язык «{lang}» не установлен (есть: {0}). Добавьте его: Параметры → Время и язык → Язык и регион.",
        &[("lang", &lang), ("0", &have.join(", "))],
    ))
}

fn init() {
    // S_FALSE / RPC_E_CHANGED_MODE are fine: WinRT just needs some apartment.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
}

fn engine(lang: &str) -> windows::core::Result<OcrEngine> {
    if lang == "auto" {
        return OcrEngine::TryCreateFromUserProfileLanguages();
    }
    let tag = languages()
        .into_iter()
        .find(|t| {
            t.eq_ignore_ascii_case(lang)
                || t.split('-')
                    .next()
                    .is_some_and(|p| p.eq_ignore_ascii_case(lang))
        })
        .unwrap_or_else(|| lang.to_string());
    OcrEngine::TryCreateFromLanguage(&Language::CreateLanguage(&HSTRING::from(tag))?)
}

/// Recognized lines joined with `\n`; empty when there is no text. Blocks: call off the UI thread.
pub fn recognize(bgra: &[u8], width: u32, height: u32, lang: &str) -> Result<String, String> {
    init();
    let max = OcrEngine::MaxImageDimension().map_err(|e| e.to_string())?;
    if width > max || height > max {
        return Err(tf(
            "область больше {max} px — выделите поменьше",
            &[("max", &max)],
        ));
    }
    let engine = engine(lang).map_err(|e| {
        language_problem(lang).unwrap_or_else(|| tf("OCR недоступен: {e}", &[("e", &e)]))
    })?;
    let k = snapling_core::ocr::scale(width, height, max);
    let (bgra, width, height) = snapling_core::ocr::enlarge(bgra, width, height, k);
    let run = || -> windows::core::Result<String> {
        let w = DataWriter::new()?;
        w.WriteBytes(&bgra)?;
        let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
            &w.DetachBuffer()?,
            BitmapPixelFormat::Bgra8,
            width as i32,
            height as i32,
            BitmapAlphaMode::Premultiplied,
        )?;
        let result = engine.RecognizeAsync(&bitmap)?.join()?;
        let lines: Vec<String> = result
            .Lines()?
            .into_iter()
            .filter_map(|l| l.Text().ok())
            .map(|t| t.to_string())
            .collect();
        Ok(lines.join("\n"))
    };
    run().map_err(|e| format!("OCR: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use snapling_core::annotate::{self, Doc, Element, Kind, Pt};

    fn shot(text: &str) -> (Vec<u8>, u32, u32) {
        let (w, h) = (900, 140);
        let white = vec![255u8; (w * h * 4) as usize];
        let base = annotate::pixmap_from_bgra(&white, w, h).unwrap();
        let doc = Doc {
            elements: vec![Element {
                kind: Kind::Text(Pt { x: 20.0, y: 30.0 }, text.into()),
                color: [17, 17, 17],
                size: 6.0,
            }],
            crop: None,
        };
        annotate::export(&base, &doc)
    }

    #[test]
    fn reads_rendered_text() {
        for (lang, text, want) in [
            ("en", "Hello Snapling 2026", "Snapling"),
            ("ru", "Привет мир", "Привет"),
        ] {
            if language_problem(lang).is_some() {
                eprintln!("skip {lang}: not installed ({:?})", languages());
                continue;
            }
            let (bgra, w, h) = shot(text);
            let got = recognize(&bgra, w, h, lang).unwrap();
            assert!(got.contains(want), "{lang}: {got:?}");
        }
    }

    fn distance(a: &str, b: &str) -> usize {
        let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
        let mut row: Vec<usize> = (0..=b.len()).collect();
        for (i, ca) in a.iter().enumerate() {
            let mut prev = row[0];
            row[0] = i + 1;
            for (j, cb) in b.iter().enumerate() {
                let cur = row[j + 1];
                row[j + 1] = (prev + usize::from(ca != cb)).min(cur + 1).min(row[j] + 1);
                prev = cur;
            }
        }
        row[b.len()]
    }

    /// `cargo test --release -p snapling-win ocr_quality -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn ocr_quality() {
        use snapling_core::annotate::{Pixmap, text_path_in};
        use tiny_skia::{Color, FillRule, Paint, Transform};
        let font =
            snapling_core::annotate::FontRef::try_from_slice(snapling_scene::FONT_REGULAR).unwrap();
        let texts = [
            ("ru", "Настройки сохранены в папке пользователя"),
            ("ru", "Счёт на 2 400 рублей оплачен вчера в 14:30"),
            ("ru", "Съешь же ещё этих мягких французских булок"),
            ("en", "The quick brown fox jumps over the lazy dog"),
            ("en", "Invoice #2400 was paid on 12/03 at 14:30"),
            ("en", "Settings are stored in the user folder"),
        ];
        let colors = [
            ("light", [255u8, 255, 255], [30u8, 30, 30]),
            ("dark", [32, 34, 37], [220, 221, 222]),
            ("gray", [240, 240, 240], [120, 120, 120]),
        ];
        let modes = ["auto"];
        println!("px  theme  {}", modes.join(" "));
        let mut total = [0f32; 1];
        let mut n = 0.0;
        for px in [10.0f32, 11.0, 12.0, 13.0, 14.0, 16.0, 20.0] {
            for (name, bg, fg) in colors {
                let mut line = [0f32; 1];
                for (lang, text) in texts {
                    let (w, h) = ((px * 34.0) as u32, (px * 2.4) as u32);
                    let mut pm = Pixmap::new(w, h).unwrap();
                    pm.fill(Color::from_rgba8(bg[0], bg[1], bg[2], 255));
                    let path = text_path_in(&font, text, Pt { x: px, y: px * 0.6 }, px).unwrap();
                    let mut paint = Paint::default();
                    paint.set_color_rgba8(fg[0], fg[1], fg[2], 255);
                    pm.fill_path(
                        &path,
                        &paint,
                        FillRule::Winding,
                        Transform::identity(),
                        None,
                    );
                    let got = recognize(pm.data(), w, h, lang).unwrap_or_default();
                    let d = distance(got.trim(), text) as f32;
                    line[0] += 1.0 - (d / text.chars().count() as f32).min(1.0);
                }
                let row: Vec<String> = line
                    .iter()
                    .map(|v| format!("{:>6.1}%", v / 6.0 * 100.0))
                    .collect();
                println!("{px:>2}  {name:<5}  {}", row.join(" "));
                for (t, v) in total.iter_mut().zip(line) {
                    *t += v;
                }
                n += 6.0;
            }
        }
        println!(
            "all         {}",
            total
                .iter()
                .map(|v| format!("{:>6.1}%", v / n * 100.0))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }

    #[test]
    fn blank_image_has_no_text() {
        if languages().is_empty() {
            return;
        }
        let white = vec![255u8; 200 * 100 * 4];
        assert_eq!(recognize(&white, 200, 100, "auto").unwrap(), "");
    }
}
