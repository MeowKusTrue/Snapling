use super::*;

/// Row icon of the tray menu; also says what the row does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuIcon {
    Region,
    Screen,
    Repeat,
    Delay,
    Record,
    Scroll,
    Text,
    History,
    Folder,
    Settings,
    Quit,
}

/// Tray menu row, `assets/design/TrayWindows.dc.html`: icon, title, hotkey.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuRow {
    Item(MenuIcon, String, String),
    Line,
}

pub(crate) const MENU_W: f32 = 300.0;
pub(crate) const MENU_PAD: f32 = 4.0;
pub(crate) const MENU_ITEM: f32 = 36.0;
/// Room around the panel for its shadow.
pub const MENU_SHADOW: f32 = 32.0;

pub(crate) const CIRCLE: &str =
    "M 12 3 C 17 3 21 7 21 12 C 21 17 17 21 12 21 C 7 21 3 17 3 12 C 3 7 7 3 12 3 Z";

pub(crate) fn menu_icon(i: MenuIcon) -> &'static [&'static str] {
    match i {
        MenuIcon::Region => &SELECT,
        MenuIcon::Screen => &[
            "M 5 4 H 19 Q 21 4 21 6 V 15 Q 21 17 19 17 H 5 Q 3 17 3 15 V 6 Q 3 4 5 4 Z",
            "M 8 21 H 16",
            "M 12 17 V 21",
        ],
        MenuIcon::Repeat => &[
            "M 3 12 C 3 17 7 21 12 21 C 17 21 21 17 21 12 C 21 7 17 3 12 3 C 9.7 3 7.6 3.9 6 5.3 L 3 8",
            "M 3 3 V 8 H 8",
        ],
        MenuIcon::Delay => &[
            "M 7 3 H 17",
            "M 7 21 H 17",
            "M 8 3 C 8 9 16 9 16 12 C 16 15 8 15 8 21",
            "M 16 3 C 16 9 8 9 8 12 C 8 15 16 15 16 21",
        ],
        MenuIcon::Record => icon_data(Button::Record),
        MenuIcon::Scroll => icon_data(Button::Scroll),
        MenuIcon::Text => icon_data(Button::Text),
        MenuIcon::History => &[CIRCLE, "M 12 7 V 12 L 15 14"],
        MenuIcon::Folder => &[
            "M 5 5 H 9 L 11 7 H 19 Q 21 7 21 9 V 17 Q 21 19 19 19 H 5 Q 3 19 3 17 V 7 Q 3 5 5 5 Z",
        ],
        MenuIcon::Settings => &[
            "M 9.8 5.0 L 10.0 2.2 L 14.0 2.2 L 14.2 5.0 L 15.4 5.5 L 17.5 3.7 L 20.3 6.5 L 18.5 8.6 L 19.0 9.8 L 21.8 10.0 L 21.8 14.0 L 19.0 14.2 L 18.5 15.4 L 20.3 17.5 L 17.5 20.3 L 15.4 18.5 L 14.2 19.0 L 14.0 21.8 L 10.0 21.8 L 9.8 19.0 L 8.6 18.5 L 6.5 20.3 L 3.7 17.5 L 5.5 15.4 L 5.0 14.2 L 2.2 14.0 L 2.2 10.0 L 5.0 9.8 L 5.5 8.6 L 3.7 6.5 L 6.5 3.7 L 8.6 5.5 Z",
            "M 12 9 C 13.7 9 15 10.3 15 12 C 15 13.7 13.7 15 12 15 C 10.3 15 9 13.7 9 12 C 9 10.3 10.3 9 12 9 Z",
        ],
        MenuIcon::Quit => &[
            "M 12 3 V 12",
            "M 6.3 7.3 C 3.2 10.4 3.2 15.5 6.3 18.6 C 9.4 21.8 14.6 21.8 17.7 18.6 C 20.8 15.5 20.8 10.4 17.7 7.3",
        ],
    }
}

/// Top of each row inside the panel, then the panel height.
pub(crate) fn menu_tops(rows: &[MenuRow]) -> (Vec<f32>, f32) {
    let mut y = MENU_PAD;
    let tops = rows
        .iter()
        .map(|row| {
            let top = y;
            y += match row {
                MenuRow::Item(..) => MENU_ITEM + 1.0,
                MenuRow::Line => 8.0,
            };
            top
        })
        .collect();
    (tops, y - 1.0 + MENU_PAD)
}

pub fn menu_size(rows: &[MenuRow], s: f32) -> (u32, u32) {
    let full = |v: f32| ((v + MENU_SHADOW * 2.0) * s).ceil() as u32;
    (full(MENU_W), full(menu_tops(rows).1))
}

/// Index of the row under a point of the window.
pub fn menu_at(rows: &[MenuRow], s: f32, p: Point) -> Option<usize> {
    let m = MENU_SHADOW * s;
    menu_tops(rows).0.iter().zip(rows).position(|(top, row)| {
        matches!(row, MenuRow::Item(..))
            && r(
                m + MENU_PAD * s,
                m + top * s,
                (MENU_W - MENU_PAD * 2.0) * s,
                MENU_ITEM * s,
            )
            .contains(p)
    })
}

/// The whole menu into a transparent BGRA buffer, premultiplied.
pub fn draw_menu(
    pm: &mut PixmapMut<'_>,
    rows: &[MenuRow],
    s: f32,
    hover: Option<usize>,
    light: bool,
) {
    let f = fonts();
    let id = Transform::identity();
    let (fill, edge, fg, muted, over) = if light {
        (
            [249, 249, 249, 245],
            [0, 0, 0, 26],
            [27, 27, 27, 255],
            [95, 95, 95, 255],
            [0, 0, 0, 14],
        )
    } else {
        (
            [44, 44, 48, 245],
            [255, 255, 255, 20],
            [255, 255, 255, 255],
            [0xA8, 0xA8, 0xA8, 255],
            [255, 255, 255, 18],
        )
    };
    let m = MENU_SHADOW * s;
    let (w, h) = (MENU_W * s, menu_tops(rows).1 * s);
    if let Some(mut layer) = tiny_skia::Pixmap::new(pm.width(), pm.height()) {
        if let Some(p) = rrect(m, m + 8.0 * s, w, h, 8.0 * s) {
            let shade = paint([0, 0, 0, 115]);
            layer.fill_path(&p, &shade, FillRule::Winding, id, None);
        }
        annotate::box_blur(&mut layer, (8.0 * s) as usize);
        pm.draw_pixmap(
            0,
            0,
            layer.as_ref(),
            &tiny_skia::PixmapPaint::default(),
            id,
            None,
        );
    }
    if let Some(p) = rrect(m + 0.5, m + 0.5, w - 1.0, h - 1.0, 8.0 * s) {
        pm.fill_path(&p, &paint(fill), FillRule::Winding, id, None);
        pm.stroke_path(&p, &paint(edge), &stroke(1.0), id, None);
    }
    let (left, right) = (m + MENU_PAD * s, m + w - MENU_PAD * s);
    for (i, (row, top)) in rows.iter().zip(menu_tops(rows).0).enumerate() {
        let y = m + top * s;
        let MenuRow::Item(glyph, title, keys) = row else {
            if let Some(line) = tiny_skia::Rect::from_xywh(
                left + 4.0 * s,
                y + 3.0 * s,
                right - left - 8.0 * s,
                s.max(1.0),
            ) {
                pm.fill_rect(line, &paint(edge), id, None);
            }
            continue;
        };
        if hover == Some(i)
            && let Some(p) = rrect(left, y, right - left, MENU_ITEM * s, 5.0 * s)
        {
            pm.fill_path(&p, &paint(over), FillRule::Winding, id, None);
        }
        let cy = y + MENU_ITEM * s / 2.0;
        icon(pm, menu_icon(*glyph), left + 20.0 * s, cy, 16.0 * s, fg);
        let x = left + 40.0 * s;
        let mut end = right - 12.0 * s;
        if !keys.is_empty() {
            let kw = text_w(&f.regular, keys, 12.0 * s);
            text(pm, &f.regular, keys, end - kw, cy, 12.0 * s, muted);
            end -= kw + 12.0 * s;
        }
        let title = fit_text(&f.regular, title, 14.0 * s, end - x);
        text(pm, &f.regular, &title, x, cy, 14.0 * s, fg);
    }
}
