//! PP-OCR on ONNX Runtime (PaddleOCR, Apache-2.0, ONNX export by monkt): a small text detector
//! and the East Slavic recognizer, which reads Russian and English on one line.
//! Sessions live for one call, so the tray process does not keep the models in memory.

use crate::{err, fetch_file, session};
use ort::session::Session;
use ort::value::Tensor;
use snapling_core::i18n::t;
use snapling_core::ocr::{enlarge, resize, scale};
use std::path::Path;

pub const FOLDER: &str = "paddleocr";
pub const REPO: &str = "monkt/paddleocr-onnx";
pub const DOWNLOAD_MB: u32 = 11;

const FILES: [(&str, &str, &str); 3] = [
    (
        "det.onnx",
        "detection/v3/det.onnx",
        "ee40e80071ba3a320d4efda75f3e22047a7d049e9bf7bcaaf9daea23fc21b935",
    ),
    (
        "rec.onnx",
        "languages/eslav/rec.onnx",
        "dc6bf0e855247decce214ba6dae5bc135fa0ad725a5918a7fcfb59fad6c9cdee",
    ),
    (
        "dict.txt",
        "languages/eslav/dict.txt",
        "3e95f1581557162870cacdba5af91a4c6be2890710d395b0c3c7578e7ee5e6eb",
    ),
];

/// Longest side the detector sees; a full 4K screen is scaled down to it.
const DET_LIMIT: u32 = 2560;
const REC_HEIGHT: u32 = 48;
const DET_THRESHOLD: f32 = 0.3;
const BOX_THRESHOLD: f32 = 0.6;
const LINE_THRESHOLD: f32 = 0.5;

pub fn installed(models: &Path) -> bool {
    FILES
        .iter()
        .all(|(f, ..)| models.join(FOLDER).join(f).is_file())
}

/// Called only after the user agreed.
pub fn download(models: &Path, proxy: Option<&str>) -> Result<(), String> {
    let root = models.join(FOLDER);
    std::fs::create_dir_all(&root).map_err(|e| format!("{}: {e}", root.display()))?;
    for (file, remote, sha) in FILES {
        let url = format!("https://huggingface.co/{REPO}/resolve/main/{remote}");
        fetch_file(&url, &root.join(file), sha, proxy)?;
    }
    Ok(())
}

/// Text box in shot pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Area {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

/// Connected regions of the probability map as boxes, grown back by the amount the model
/// shrinks text. Axis-aligned: screen text is horizontal.
// ponytail: no rotated boxes, add min-area rectangles if photos of text ever matter.
fn boxes(map: &[f32], w: usize, h: usize) -> Vec<Area> {
    let mut seen = vec![false; w * h];
    let mut out = Vec::new();
    let mut stack = Vec::new();
    for start in 0..w * h {
        if seen[start] || map[start] < DET_THRESHOLD {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
        let (mut sum, mut n) = (0.0, 0.0);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            sum += map[i];
            n += 1.0;
            let near = [
                (x > 0).then(|| i - 1),
                (x + 1 < w).then(|| i + 1),
                (y > 0).then(|| i - w),
                (y + 1 < h).then(|| i + w),
            ];
            for j in near.into_iter().flatten() {
                if !seen[j] && map[j] >= DET_THRESHOLD {
                    seen[j] = true;
                    stack.push(j);
                }
            }
        }
        let (bw, bh) = ((x1 - x0 + 1) as f32, (y1 - y0 + 1) as f32);
        if sum / n < BOX_THRESHOLD || bh < 3.0 || bw < 3.0 {
            continue;
        }
        let grow = bw * bh * 1.5 / (2.0 * (bw + bh));
        let left = (x0 as f32 - grow).max(0.0);
        let top = (y0 as f32 - grow).max(0.0);
        let right = (x1 as f32 + 1.0 + grow).min(w as f32);
        let bottom = (y1 as f32 + 1.0 + grow).min(h as f32);
        out.push(Area {
            x: left as u32,
            y: top as u32,
            w: (right - left) as u32,
            h: (bottom - top) as u32,
        });
    }
    out
}

fn detect(det: &mut Session, bgra: &[u8], w: u32, h: u32) -> Result<Vec<Area>, String> {
    let k = (DET_LIMIT as f32 / w.max(h) as f32).min(1.0);
    let side = |v: u32| (((v as f32 * k) / 32.0).round() as u32).max(1) * 32;
    let (dw, dh) = (side(w), side(h));
    let small = resize(bgra, w, h, dw, dh);
    let n = (dw * dh) as usize;
    let mut input = vec![0f32; n * 3];
    const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
    const STD: [f32; 3] = [0.229, 0.224, 0.225];
    for (i, px) in small.as_chunks::<4>().0.iter().enumerate() {
        for c in 0..3 {
            input[c * n + i] = (f32::from(px[c]) / 255.0 - MEAN[c]) / STD[c];
        }
    }
    let shape = [1, 3, dh as usize, dw as usize];
    let out = det
        .run(ort::inputs![
            Tensor::from_array((shape, input)).map_err(err)?
        ])
        .map_err(err)?;
    let (_, map) = out[0].try_extract_tensor::<f32>().map_err(err)?;
    let (kx, ky) = (w as f32 / dw as f32, h as f32 / dh as f32);
    Ok(boxes(map, dw as usize, dh as usize)
        .into_iter()
        .map(|b| Area {
            x: (b.x as f32 * kx) as u32,
            y: (b.y as f32 * ky) as u32,
            w: ((b.w as f32 * kx) as u32).min(w - (b.x as f32 * kx) as u32),
            h: ((b.h as f32 * ky) as u32).min(h - (b.y as f32 * ky) as u32),
        })
        .filter(|b| b.w > 1 && b.h > 1)
        .collect())
}

/// CTC: repeats collapse, class 0 is the blank, the last class is a space.
fn decode(logits: &[f32], classes: usize, dict: &[String]) -> (String, f32) {
    let (mut text, mut last) = (String::new(), 0);
    let (mut sum, mut n) = (0.0, 0.0);
    for step in logits.chunks_exact(classes) {
        let (best, score) = step
            .iter()
            .copied()
            .enumerate()
            .fold((0, f32::MIN), |a, b| if b.1 > a.1 { b } else { a });
        if best != 0 && best != last {
            text.push_str(dict.get(best - 1).map_or(" ", String::as_str));
            sum += score;
            n += 1.0;
        }
        last = best;
    }
    (text, if n > 0.0 { sum / n } else { 0.0 })
}

fn read_line(
    rec: &mut Session,
    dict: &[String],
    bgra: &[u8],
    width: u32,
    b: Area,
) -> Result<(String, f32), String> {
    let stride = width as usize * 4;
    let mut crop = Vec::with_capacity((b.w * b.h * 4) as usize);
    for y in b.y..b.y + b.h {
        let at = y as usize * stride + b.x as usize * 4;
        crop.extend_from_slice(&bgra[at..at + b.w as usize * 4]);
    }
    let w = (b.w * REC_HEIGHT / b.h).max(16);
    let px = resize(&crop, b.w, b.h, w, REC_HEIGHT);
    let n = (w * REC_HEIGHT) as usize;
    let mut input = vec![0f32; n * 3];
    for (i, p) in px.as_chunks::<4>().0.iter().enumerate() {
        for c in 0..3 {
            input[c * n + i] = f32::from(p[c]) / 127.5 - 1.0;
        }
    }
    let shape = [1, 3, REC_HEIGHT as usize, w as usize];
    let out = rec
        .run(ort::inputs![
            Tensor::from_array((shape, input)).map_err(err)?
        ])
        .map_err(err)?;
    let (dims, logits) = out[0].try_extract_tensor::<f32>().map_err(err)?;
    let classes = *dims.last().ok_or("rec shape")? as usize;
    Ok(decode(logits, classes, dict))
}

/// Boxes on one level become one line, left to right; lines go top to bottom.
fn lines(mut found: Vec<(Area, String)>) -> String {
    found.sort_by_key(|(b, _)| (b.y + b.h / 2, b.x));
    let mut rows: Vec<Vec<(Area, String)>> = Vec::new();
    for item in found {
        let mid = item.0.y + item.0.h / 2;
        match rows.last_mut() {
            Some(row) if mid < row[0].0.y + row[0].0.h => row.push(item),
            _ => rows.push(vec![item]),
        }
    }
    rows.iter_mut()
        .map(|row| {
            row.sort_by_key(|(b, _)| b.x);
            let parts: Vec<&str> = row.iter().map(|(_, s)| s.trim()).collect();
            parts.join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Recognized lines joined with `\n`; empty when there is no text. Blocks: call off the UI thread.
pub fn recognize(models: &Path, bgra: &[u8], width: u32, height: u32) -> Result<String, String> {
    if !installed(models) {
        return Err(t("Модель распознавания не установлена").into());
    }
    let root = models.join(FOLDER);
    let dict = std::fs::read_to_string(root.join(FILES[2].0)).map_err(err)?;
    let dict: Vec<String> = dict.lines().map(str::to_string).collect();
    let mut det = session(&root.join(FILES[0].0))?;
    let mut rec = session(&root.join(FILES[1].0))?;
    let k = scale(width, height, DET_LIMIT);
    let (big, w, h) = enlarge(bgra, width, height, k);
    let mut found = Vec::new();
    for b in detect(&mut det, &big, w, h)? {
        let (text, score) = read_line(&mut rec, &dict, &big, w, b)?;
        if score >= LINE_THRESHOLD && !text.trim().is_empty() {
            found.push((b, text));
        }
    }
    Ok(lines(found))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boxes_split_regions_and_drop_weak_ones() {
        let (w, h) = (40, 20);
        let mut map = vec![0.0; w * h];
        for y in 4..8 {
            for x in 4..16 {
                map[y * w + x] = 0.9;
            }
            for x in 24..36 {
                map[y * w + x] = 0.4;
            }
        }
        let found = boxes(&map, w, h);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].x < 4 && found[0].w > 12);
    }

    #[test]
    fn ctc_collapses_repeats_and_reads_the_space_class() {
        let dict = ["a".to_string(), "b".to_string()];
        let step = |c: usize| {
            let mut v = [0.0; 4];
            v[c] = 1.0;
            v
        };
        let logits: Vec<f32> = [1, 1, 0, 1, 3, 2].iter().flat_map(|&c| step(c)).collect();
        assert_eq!(decode(&logits, 4, &dict).0, "aa b");
    }

    #[test]
    fn lines_keep_reading_order() {
        let area = |x, y| Area { x, y, w: 50, h: 20 };
        let found = vec![
            (area(100, 2), "мир".to_string()),
            (area(0, 0), "привет".to_string()),
            (area(0, 40), "снизу".to_string()),
        ];
        assert_eq!(lines(found), "привет мир\nснизу");
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

    /// `SNAP_MODELS=<dir> cargo test --release -p snapling-translate paddle_quality -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn paddle_quality() {
        use snapling_core::annotate::{FontRef, Pixmap, Pt, text_path_in};
        use tiny_skia::{Color, FillRule, Paint, Transform};
        let models = std::path::PathBuf::from(std::env::var("SNAP_MODELS").expect("SNAP_MODELS"));
        let font = FontRef::try_from_slice(snapling_scene::FONT_REGULAR).unwrap();
        let texts = [
            "Настройки сохранены в папке пользователя",
            "Счёт на 2 400 рублей оплачен вчера в 14:30",
            "Съешь же ещё этих мягких французских булок",
            "The quick brown fox jumps over the lazy dog",
            "Invoice #2400 was paid on 12/03 at 14:30",
            "Settings are stored in the user folder",
            "Открой файл config.toml в папке Snapling",
        ];
        let themes = [
            ("light", [255u8, 255, 255], [30u8, 30, 30]),
            ("dark", [32, 34, 37], [220, 221, 222]),
            ("gray", [240, 240, 240], [120, 120, 120]),
        ];
        let (mut total, mut count) = (0.0, 0.0);
        for px in [10.0f32, 11.0, 12.0, 13.0, 14.0, 16.0, 20.0] {
            for (name, bg, fg) in themes {
                let mut line = 0.0;
                for text in texts {
                    let (w, h) = ((px * 34.0) as u32, (px * 2.4) as u32);
                    let mut pm = Pixmap::new(w, h).unwrap();
                    pm.fill(Color::from_rgba8(bg[0], bg[1], bg[2], 255));
                    let at = Pt { x: px, y: px * 0.6 };
                    let path = text_path_in(&font, text, at, px).unwrap();
                    let mut paint = Paint::default();
                    paint.set_color_rgba8(fg[0], fg[1], fg[2], 255);
                    let id = Transform::identity();
                    pm.fill_path(&path, &paint, FillRule::Winding, id, None);
                    let got = recognize(&models, pm.data(), w, h).unwrap();
                    if px == 14.0 && name == "light" {
                        println!("{got:?}");
                    }
                    let d = distance(got.trim(), text) as f32;
                    line += 1.0 - (d / text.chars().count() as f32).min(1.0);
                }
                println!("{px:>2} {name:<5} {:>6.1}%", line / 7.0 * 100.0);
                total += line;
                count += 7.0;
            }
        }
        println!("all {:>6.1}%", total / count * 100.0);
        let (w, h) = (1920u32, 1080u32);
        let mut pm = Pixmap::new(w, h).unwrap();
        pm.fill(Color::WHITE);
        let mut paint = Paint::default();
        paint.set_color_rgba8(20, 20, 20, 255);
        for row in 0..40 {
            let at = Pt {
                x: 20.0,
                y: row as f32 * 26.0,
            };
            let text = "Настройки сохранены в папке пользователя и ещё немного текста для строки";
            let path = text_path_in(&font, text, at, 14.0).unwrap();
            pm.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
        let t0 = std::time::Instant::now();
        let got = recognize(&models, pm.data(), w, h).unwrap();
        println!("1080p: {:?}, {} chars", t0.elapsed(), got.chars().count());
    }
}
