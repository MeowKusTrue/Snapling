use super::*;

/// Button of the notification after a shot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastAction {
    Editor,
    Text,
    Folder,
}

pub struct Toast<'a> {
    pub scale: f32,
    pub title: &'a str,
    pub sub: &'a str,
    /// BGRA, width, height.
    pub shot: (&'a [u8], u32, u32),
    pub hover: Option<ToastAction>,
}

pub(crate) const TOAST: (f32, f32) = (392.0, 96.0);
pub(crate) const TOAST_THUMB: (f32, f32) = (116.0, 76.0);
pub(crate) const TOAST_BUTTONS: [(ToastAction, &str); 3] = [
    (ToastAction::Editor, n("Редактор")),
    (ToastAction::Text, n("Текст")),
    (ToastAction::Folder, n("Папка")),
];

pub fn toast_size(s: f32) -> (u32, u32) {
    ((TOAST.0 * s).ceil() as u32, (TOAST.1 * s).ceil() as u32)
}

pub(crate) fn toast_buttons(s: f32) -> Vec<(Rect, ToastAction, &'static str)> {
    let mut x = 138.0 * s;
    let y = 56.0 * s;
    TOAST_BUTTONS
        .map(|(a, label)| {
            let label = t(label);
            let w = text_w(&fonts().semibold, label, 12.5 * s) + 22.0 * s;
            let b = r(x, y, w, 30.0 * s);
            x += w + 4.0 * s;
            (b, a, label)
        })
        .to_vec()
}

/// The button under a point of the notification.
pub fn toast_at(s: f32, p: Point) -> Option<ToastAction> {
    toast_buttons(s)
        .into_iter()
        .find(|(b, ..)| b.contains(p))
        .map(|(_, a, _)| a)
}

/// `s` cut with an ellipsis to fit `max` pixels.
pub(crate) fn fit_text(font: &FontRef<'_>, s: &str, px: f32, max: f32) -> String {
    if text_w(font, s, px) <= max {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    (0..chars.len())
        .rev()
        .map(|n| format!("{}…", chars[..n].iter().collect::<String>()))
        .find(|c| text_w(font, c, px) <= max)
        .unwrap_or_default()
}

/// The whole notification into a transparent BGRA buffer, premultiplied.
pub fn draw_toast(pm: &mut PixmapMut<'_>, tt: &Toast<'_>) {
    let s = tt.scale;
    let f = fonts();
    let id = Transform::identity();
    let (w, h) = (pm.width() as f32, pm.height() as f32);
    if let Some(p) = rrect(0.5, 0.5, w - 1.0, h - 1.0, 18.0 * s) {
        pm.fill_path(&p, &paint([22, 27, 39, 245]), FillRule::Winding, id, None);
        pm.stroke_path(&p, &paint([255, 255, 255, 26]), &stroke(1.0), id, None);
    }
    let (px, py) = (10.0 * s, 10.0 * s);
    let (tw, th) = (TOAST_THUMB.0 * s, TOAST_THUMB.1 * s);
    if let (Some(path), Some(src)) = (
        rrect(px, py, tw, th, 10.0 * s),
        PixmapRef::from_bytes(tt.shot.0, tt.shot.1, tt.shot.2),
    ) {
        // Cover the frame, keep the middle of the shot.
        let (iw, ih) = (tt.shot.1 as f32, tt.shot.2 as f32);
        let k = (tw / iw).max(th / ih);
        let dx = px + (tw - iw * k) / 2.0;
        let dy = py + (th - ih * k) / 2.0;
        pm.fill_path(
            &path,
            &paint([49, 51, 56, 255]),
            FillRule::Winding,
            id,
            None,
        );
        let shader = tiny_skia::Pattern::new(
            src,
            tiny_skia::SpreadMode::Pad,
            tiny_skia::FilterQuality::Bilinear,
            1.0,
            Transform::from_row(k, 0.0, 0.0, k, dx, dy),
        );
        let p = Paint {
            shader,
            anti_alias: true,
            ..Paint::default()
        };
        pm.fill_path(&path, &p, FillRule::Winding, id, None);
        pm.stroke_path(&path, &paint([255, 255, 255, 20]), &stroke(1.0), id, None);
    }
    let x = 138.0 * s;
    let right = w - 12.0 * s;
    icon(
        pm,
        &["M 5 12 L 10 17 L 20 7"],
        x + 8.0 * s,
        22.0 * s,
        16.0 * s,
        [0x5B, 0xD3, 0xA8, 255],
    );
    let title = fit_text(&f.semibold, tt.title, 14.0 * s, right - x - 22.0 * s);
    text(
        pm,
        &f.semibold,
        &title,
        x + 22.0 * s,
        22.0 * s,
        14.0 * s,
        TEXT,
    );
    let sub = fit_text(&f.regular, tt.sub, 12.0 * s, right - x);
    text(pm, &f.regular, &sub, x, 42.0 * s, 12.0 * s, ICON_MUTED);
    for (i, (b, a, label)) in toast_buttons(s).into_iter().enumerate() {
        let on = tt.hover == Some(a);
        let fill = match (i, on) {
            (0, true) => Some([59, 108, 245, 90]),
            (0, false) => Some([59, 108, 245, 51]),
            (_, true) => Some([255, 255, 255, 26]),
            _ => None,
        };
        if let (Some(c), Some(p)) = (fill, rr(b, 9.0 * s)) {
            pm.fill_path(&p, &paint(c), FillRule::Winding, id, None);
        }
        let cy = b.top as f32 + b.height as f32 / 2.0;
        let c = if i == 0 {
            [0xC9, 0xD8, 0xFF, 255]
        } else {
            ICON
        };
        text(
            pm,
            &f.semibold,
            label,
            b.left as f32 + 11.0 * s,
            cy,
            12.5 * s,
            c,
        );
    }
}
