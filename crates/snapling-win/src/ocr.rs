//! Text recognition with the built-in Windows OCR engine (Windows.Media.Ocr): offline,
//! nothing leaves the machine. Approach from EiSen windows_adapter.rs, but the bitmap is
//! built from memory instead of a temp file.

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
        return Some("OCR: в Windows не установлен ни один язык распознавания текста.".into());
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
    Some(format!(
        "OCR: язык «{lang}» не установлен (есть: {}). Добавьте его: Параметры → Время и язык → Язык и регион.",
        have.join(", ")
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
        return Err(format!("область больше {max} px — выделите поменьше"));
    }
    let engine = engine(lang)
        .map_err(|e| language_problem(lang).unwrap_or_else(|| format!("OCR недоступен: {e}")))?;
    let run = || -> windows::core::Result<String> {
        let w = DataWriter::new()?;
        w.WriteBytes(bgra)?;
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

    #[test]
    fn blank_image_has_no_text() {
        if languages().is_empty() {
            return;
        }
        let white = vec![255u8; 200 * 100 * 4];
        assert_eq!(recognize(&white, 200, 100, "auto").unwrap(), "");
    }
}
