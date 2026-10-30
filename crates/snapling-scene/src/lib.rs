//! The selection overlay drawn 1:1 from the «Выделение области» mockup, on any OS.
//! The platform window blits the frozen frame (dimmed outside, original inside the
//! selection) into a BGRA buffer, then calls [`draw`]. Colors are written as BGRA.

use snapling_core::annotate::{self, FontRef, Pt};
use snapling_core::i18n::{n, t};
use snapling_core::selection::{Point, Rect};
use std::cell::RefCell;
use std::sync::OnceLock;
use tiny_skia::{
    BlendMode, FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, PixmapMut, PixmapRef,
    Stroke, Transform,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Copy,
    Save,
    Edit,
    Text,
    Translate,
    Cancel,
}

/// Drawing tools of the in-overlay editor, keys 1–9 in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Arrow,
    Line,
    Rect,
    Ellipse,
    Pen,
    Marker,
    Text,
    Step,
    Blur,
}

pub const TOOLS: [Tool; 9] = [
    Tool::Arrow,
    Tool::Line,
    Tool::Rect,
    Tool::Ellipse,
    Tool::Pen,
    Tool::Marker,
    Tool::Text,
    Tool::Step,
    Tool::Blur,
];

pub const FONT_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
pub const FONT_SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf");
pub const FONT_MONO: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-SemiBold.ttf");

pub const COLORS: [[u8; 3]; 8] = [
    [255, 59, 48],
    [255, 138, 31],
    [255, 204, 0],
    [52, 199, 89],
    [10, 132, 255],
    [175, 82, 222],
    [17, 17, 17],
    [255, 255, 255],
];
pub const SIZES: [f32; 3] = [2.0, 4.0, 8.0];

/// Anything clickable in the overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Action(Button),
    /// Rectangular selection (V).
    Select,
    /// Freeform selection (L).
    Lasso,
    Tool(Tool),
    Undo,
    Redo,
    Color(usize),
    Size(usize),
    /// Blur kind: `true` = Gaussian, `false` = cubes.
    Gauss(bool),
    Strength(u8),
}

/// Tool rail and annotations, on screen from the first frame.
pub struct Draw<'a> {
    /// Monitor the rail stays on: the one under the cursor at open.
    pub monitor: Rect,
    /// Freeform selection is the active mode.
    pub lasso_mode: bool,
    /// Outline of the freeform selection, finished or being drawn.
    pub lasso: Option<&'a [Pt]>,
    pub tool: Option<Tool>,
    pub color: [u8; 3],
    pub size: f32,
    pub blur: annotate::BlurStyle,
    pub can_undo: bool,
    pub can_redo: bool,
    /// Shape being dragged, in image coordinates and RGB.
    pub live: Option<&'a annotate::Element>,
    /// Text being typed and where.
    pub caret: Option<(Pt, &'a str)>,
}

/// What the cursor highlights before anything is selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Window,
    Element,
    Screen,
}

impl Target {
    fn label(self) -> &'static str {
        match self {
            Target::Window => n("Окно"),
            Target::Element => n("Элемент"),
            Target::Screen => n("Экран"),
        }
    }
}

pub struct Hover<'a> {
    pub rect: Rect,
    pub target: Target,
    pub app: &'a str,
}

pub struct Scene<'a> {
    /// DPI scale of the monitor the selection (or hover) is on; mockup sizes are at 1.0.
    pub scale: f32,
    /// That monitor, for keeping pills and the toolbar on screen.
    pub monitor: Rect,
    /// Monitor under the cursor, for the hint bar.
    pub cursor_monitor: Rect,
    pub selection: Option<Rect>,
    /// Selection finished: handles and toolbar.
    pub confirmed: bool,
    pub dragging: bool,
    pub hover: Option<Hover<'a>>,
    pub hover_button: Option<Control>,
    pub loupe: Option<Loupe<'a>>,
    pub tools: Option<Draw<'a>>,
}

/// Magnified pixels around the cursor.
pub struct Loupe<'a> {
    pub at: Point,
    /// The whole frame, BGRA.
    pub frame: &'a [u8],
    pub width: u32,
    pub height: u32,
}

/// `#RRGGBB` of a BGRA pixel.
pub fn color_hex(px: &[u8]) -> String {
    format!("#{:02X}{:02X}{:02X}", px[2], px[1], px[0])
}

fn loupe_px<'a>(l: &Loupe<'a>, x: i32, y: i32) -> Option<&'a [u8]> {
    let inside = x >= 0 && y >= 0 && x < l.width as i32 && y < l.height as i32;
    inside.then(|| {
        let i = (y as usize * l.width as usize + x as usize) * 4;
        &l.frame[i..i + 4]
    })
}

#[derive(Debug, Default, Clone)]
pub struct Layout {
    pub buttons: Vec<(Rect, Control)>,
    /// Everything drawn outside the selection itself, inflated for blur and shadows.
    pub bounds: Option<Rect>,
    toolbar: Option<Rect>,
    strip: Option<Rect>,
    options: Option<Rect>,
    dividers: Vec<Rect>,
    size_pill: Option<(Rect, String)>,
    hover_pill: Option<Rect>,
    hint_bar: Option<Rect>,
    loupe: Option<Rect>,
}

// Mockup tokens.
const GLASS: [u8; 4] = [22, 27, 39, 184];
const GLASS_HINT: [u8; 4] = [22, 27, 39, 153];
const HAIRLINE: [u8; 4] = [255, 255, 255, 20];
const BLUE: [u8; 4] = [0x29, 0x5B, 0xE8, 255];
const ORANGE: [u8; 4] = [0xFF, 0x8A, 0x1F, 255];
const ICON: [u8; 4] = [0xE6, 0xEB, 0xF5, 255];
const ICON_MUTED: [u8; 4] = [0xA3, 0xAE, 0xC2, 255];
const TEXT: [u8; 4] = [0xF3, 0xF6, 0xFC, 255];
const BTN: f32 = 36.0;
const BAR_PAD: f32 = 5.0;
const GAP: f32 = 4.0;
const RAIL_W: f32 = 56.0;
const RAIL_BTN: (f32, f32) = (44.0, 40.0);
const OPTS_W: f32 = 160.0;

/// Toolbar buttons in mockup order; `None` is the divider.
const BAR: [Option<Button>; 7] = [
    Some(Button::Copy),
    Some(Button::Save),
    Some(Button::Edit),
    Some(Button::Text),
    Some(Button::Translate),
    None,
    Some(Button::Cancel),
];

const HINTS_SELECTED: [(&str, &str); 8] = [
    ("Enter", n("копировать")),
    ("S", n("сохранить")),
    ("E", n("редактор")),
    ("O", n("текст")),
    ("V", n("область")),
    ("L", n("произвольная")),
    ("1–9", n("рисовать")),
    ("Esc", n("отмена")),
];

const HINTS: [(&str, &str); 7] = [
    (n("Мышь"), n("выделить")),
    (n("Клик"), n("окно")),
    ("Shift", n("квадрат")),
    ("L", n("произвольная")),
    ("I", n("цвет")),
    ("1–9", n("рисовать")),
    ("Esc", n("отмена")),
];

const HINTS_PAINT: [(&str, &str); 5] = [
    ("", n("Рисуй прямо по экрану")),
    ("V", n("выделить область")),
    ("L", n("произвольная")),
    ("Ctrl+Z", n("отменить")),
    ("Esc", n("закрыть")),
];

const HINTS_LASSO: [(&str, &str); 2] = [
    ("", n("Обведи мышью, контур замкнётся сам")),
    ("Esc", n("отмена")),
];

const HINTS_LASSO_DONE: [(&str, &str); 4] = [
    ("", n("За контуром снимок прозрачный")),
    ("Enter", n("копировать")),
    ("L", n("обвести заново")),
    ("Esc", n("отмена")),
];

struct Fonts {
    regular: FontRef<'static>,
    semibold: FontRef<'static>,
    mono: FontRef<'static>,
}

fn fonts() -> &'static Fonts {
    static F: OnceLock<Fonts> = OnceLock::new();
    F.get_or_init(|| {
        let f = |b: &'static [u8]| FontRef::try_from_slice(b).expect("bundled font");
        Fonts {
            regular: f(FONT_REGULAR),
            semibold: f(FONT_SEMIBOLD),
            mono: f(FONT_MONO),
        }
    })
}

fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect {
        left: x.round() as i32,
        top: y.round() as i32,
        width: w.round().max(0.0) as i32,
        height: h.round().max(0.0) as i32,
    }
}

fn union(a: Option<Rect>, b: Rect) -> Option<Rect> {
    let Some(a) = a else { return Some(b) };
    let (l, t) = (a.left.min(b.left), a.top.min(b.top));
    Some(Rect {
        left: l,
        top: t,
        width: a.right().max(b.right()) - l,
        height: a.bottom().max(b.bottom()) - t,
    })
}

fn inflate(a: Rect, d: i32) -> Rect {
    Rect {
        left: a.left - d,
        top: a.top - d,
        width: a.width + 2 * d,
        height: a.height + 2 * d,
    }
}

fn text_w(font: &FontRef<'_>, s: &str, px: f32) -> f32 {
    annotate::text_size_in(font, s, px).0
}

fn primary_w(s: f32) -> f32 {
    (10.0 + 17.0 + 8.0 + 14.0) * s + text_w(&fonts().semibold, t("Копировать"), 13.5 * s)
}

/// Pill above the selection's top-left corner, or inside it when there is no room.
fn above_or_inside(sel: Rect, mon: Rect, w: f32, h: f32, gap: f32, s: f32) -> Rect {
    let x = (sel.left as f32)
        .min(mon.right() as f32 - w)
        .max(mon.left as f32);
    let above = sel.top as f32 - gap - h;
    if above >= mon.top as f32 {
        r(x, above, w, h)
    } else {
        r(x + 8.0 * s, sel.top as f32 + 8.0 * s, w, h)
    }
}

pub fn layout(sc: &Scene<'_>) -> Layout {
    let s = sc.scale;
    let f = fonts();
    let mut out = Layout::default();
    let blur = (24.0 * s).ceil() as i32;
    if let Some(sel) = sc.selection {
        let label = format!("{} × {}", sel.width, sel.height);
        let (w, h) = (text_w(&f.mono, &label, 11.0 * s) + 16.0 * s, 20.0 * s);
        let pill = above_or_inside(sel, sc.monitor, w, h, 8.0 * s, s);
        out.bounds = union(out.bounds, inflate(pill, blur));
        out.size_pill = Some((pill, label));
        out.bounds = union(out.bounds, inflate(sel, (10.0 * s) as i32));
        if sc.confirmed && !sc.dragging {
            let lasso = sc.tools.as_ref().is_some_and(|t| t.lasso.is_some());
            let items: Vec<Option<Button>> = BAR
                .into_iter()
                .filter(|b| !lasso || !matches!(b, Some(Button::Text | Button::Translate)))
                .collect();
            let width = |b: Option<Button>| match b {
                Some(Button::Copy) => primary_w(s),
                Some(_) => BTN * s,
                None => 9.0 * s,
            };
            let tw = 2.0 * BAR_PAD * s
                + items.iter().map(|b| width(*b)).sum::<f32>()
                + (items.len() - 1) as f32 * GAP * s;
            let th = (BTN + 2.0 * BAR_PAD) * s;
            let gap = 14.0 * s;
            let mon = sc.monitor;
            let x = (sel.left as f32)
                .min(mon.right() as f32 - tw - gap)
                .max(mon.left as f32 + gap);
            let below = sel.bottom() as f32 + gap;
            let above = sel.top as f32 - gap - th - 38.0 * s;
            let y = if below + th + gap <= mon.bottom() as f32 {
                below
            } else if above >= mon.top as f32 + gap {
                sel.top as f32 - gap - th
            } else {
                sel.bottom() as f32 - th - gap
            };
            let bar = r(x, y, tw, th);
            let mut bx = x + BAR_PAD * s;
            let by = y + BAR_PAD * s;
            for b in items {
                let w = width(b);
                match b {
                    Some(b) => out
                        .buttons
                        .push((r(bx, by, w, BTN * s), Control::Action(b))),
                    None => out
                        .dividers
                        .push(r(bx + 4.0 * s, by + 8.0 * s, s, 20.0 * s)),
                }
                bx += w + GAP * s;
            }
            out.bounds = union(out.bounds, inflate(bar, (40.0 * s) as i32));
            out.toolbar = Some(bar);
        }
    } else if let Some(hv) = &sc.hover {
        let tag = text_w(&f.semibold, hv.target.label(), 13.0 * s) + 18.0 * s;
        let rest = text_w(&f.regular, &hover_text(hv), 13.0 * s);
        let (w, h) = (6.0 * s + tag + 8.0 * s + rest + 12.0 * s, 32.0 * s);
        let mon = sc.monitor;
        let x = (hv.rect.left as f32)
            .min(mon.right() as f32 - w)
            .max(mon.left as f32);
        let below = hv.rect.bottom() as f32 + 14.0 * s;
        let y = if below + h <= mon.bottom() as f32 {
            below
        } else {
            (hv.rect.bottom() as f32 - h - 14.0 * s).max(mon.top as f32)
        };
        let pill = r(x, y, w, h);
        out.bounds = union(out.bounds, inflate(pill, blur));
        out.bounds = union(out.bounds, inflate(hv.rect, (8.0 * s) as i32));
        out.hover_pill = Some(pill);
    }
    if let Some(pts) = sc.tools.as_ref().and_then(|t| t.lasso) {
        for p in pts {
            let dot = r(p.x - 4.0, p.y - 4.0, 8.0, 8.0);
            out.bounds = union(out.bounds, dot);
        }
    }
    if let Some(t) = sc.tools.as_ref().filter(|_| !sc.dragging) {
        rail_layout(&mut out, sc, t);
    }
    if !sc.dragging {
        let fs = 12.5 * s;
        let hints = hints(sc);
        let mut w = 32.0 * s + 18.0 * s * (hints.len() - 1) as f32;
        for &(k, v) in hints {
            w += text_w(&f.semibold, t(k), fs) + text_w(&f.regular, &format!(" {}", t(v)), fs);
        }
        let h = 34.0 * s;
        let m = sc.cursor_monitor;
        // Hints are a help, not a control: on a narrow monitor they stay out of the way.
        if w + 48.0 * s <= m.width as f32 {
            let bar = r(
                m.left as f32 + (m.width as f32 - w) / 2.0,
                m.bottom() as f32 - 24.0 * s - h,
                w,
                h,
            );
            out.bounds = union(out.bounds, inflate(bar, blur));
            out.hint_bar = Some(bar);
        }
    }
    if let Some(l) = &sc.loupe {
        let (side, mon) = (loupe_side(s), sc.cursor_monitor);
        let (w, h) = (side + 12.0 * s, side + 38.0 * s);
        let off = 22.0 * s;
        let (x, y) = (l.at.x as f32, l.at.y as f32);
        let px = if x + off + w > mon.right() as f32 {
            x - off - w
        } else {
            x + off
        };
        let py = if y + off + h > mon.bottom() as f32 {
            y - off - h
        } else {
            y + off
        };
        let panel = r(px, py, w, h);
        out.bounds = union(out.bounds, inflate(panel, 2));
        out.loupe = Some(panel);
    }
    out
}

const LOUPE_CELLS: i32 = 11;

fn loupe_cell(s: f32) -> f32 {
    (13.0 * s).round()
}

fn loupe_side(s: f32) -> f32 {
    loupe_cell(s) * LOUPE_CELLS as f32
}

fn hints(sc: &Scene<'_>) -> &'static [(&'static str, &'static str)] {
    let t = sc.tools.as_ref();
    let painting = t.is_some_and(|t| t.tool.is_some());
    match (t.is_some_and(|t| t.lasso_mode), sc.confirmed) {
        (_, false) if painting => &HINTS_PAINT,
        (true, true) if !painting => &HINTS_LASSO_DONE,
        (true, false) => &HINTS_LASSO,
        (_, true) => &HINTS_SELECTED,
        _ => &HINTS,
    }
}

/// Tool rail at the monitor's edge and the options of the active tool beside it.
/// The rail moves to the other edge when a selection would sit under it.
fn rail_layout(out: &mut Layout, sc: &Scene<'_>, t: &Draw<'_>) {
    let s = sc.scale;
    let mon = t.monitor;
    let (bw, bh) = (RAIL_BTN.0 * s, RAIL_BTN.1 * s);
    let (pad, gap, sep) = (6.0 * s, 2.0 * s, 13.0 * s);
    let items: Vec<Option<Control>> = [Some(Control::Select), Some(Control::Lasso), None]
        .into_iter()
        .chain(TOOLS.iter().map(|&t| Some(Control::Tool(t))))
        .chain([None, Some(Control::Undo), Some(Control::Redo)])
        .collect();
    let sw = RAIL_W * s;
    let sh = 2.0 * pad
        + items
            .iter()
            .map(|c| if c.is_some() { bh } else { sep })
            .sum::<f32>()
        + (items.len() - 1) as f32 * gap;
    let edge = 24.0 * s;
    let left = mon.left as f32 + edge;
    let reach = left + sw + 12.0 * s + OPTS_W * s;
    let covered = sc.selection.is_some_and(|sel| (sel.left as f32) < reach);
    let x = if covered {
        mon.right() as f32 - edge - sw
    } else {
        left
    };
    let y = mon.top as f32 + (mon.height as f32 - sh) / 2.0;
    let rail = r(x, y, sw, sh);
    let bx = x + pad;
    let mut by = y + pad;
    let mut first_tool = y;
    for c in items {
        match c {
            Some(c) => {
                if c == Control::Tool(TOOLS[0]) {
                    first_tool = by;
                }
                out.buttons.push((r(bx, by, bw, bh), c));
                by += bh + gap;
            }
            None => {
                out.dividers
                    .push(r(bx + 8.0 * s, by + 6.0 * s, 28.0 * s, s));
                by += sep + gap;
            }
        }
    }
    out.bounds = union(out.bounds, inflate(rail, (40.0 * s) as i32));
    out.strip = Some(rail);

    let Some(tool) = t.tool else { return };
    let rows: Vec<(Vec<Control>, f32)> = if tool == Tool::Blur {
        vec![
            (vec![Control::Gauss(false), Control::Gauss(true)], 36.0),
            ((0..3).map(Control::Strength).collect(), 32.0),
            ((0..3).map(Control::Size).collect(), 28.0),
        ]
    } else {
        vec![
            ((0..4).map(Control::Color).collect(), 26.0),
            ((4..8).map(Control::Color).collect(), 26.0),
            ((0..3).map(Control::Size).collect(), 28.0),
        ]
    };
    let ow = OPTS_W * s;
    let ox = if covered {
        x - 12.0 * s - ow
    } else {
        x + sw + 12.0 * s
    };
    let inner = ow - 24.0 * s;
    let mut cy = first_tool + 6.0 * s;
    let top = first_tool - 6.0 * s;
    for (i, (row, cell)) in rows.iter().enumerate() {
        let cell = cell * s;
        let step = (inner - cell) / (row.len() - 1) as f32;
        for (k, c) in row.iter().enumerate() {
            let cx = ox + 12.0 * s + k as f32 * step;
            out.buttons.push((r(cx, cy, cell, cell), *c));
        }
        cy += cell;
        let next = rows.get(i + 1);
        if next.is_some_and(|(n, _)| !matches!(n[0], Control::Color(_))) {
            out.dividers.push(r(ox + 12.0 * s, cy + 9.0 * s, inner, s));
            cy += 19.0 * s;
        } else if next.is_some() {
            cy += 8.0 * s;
        }
    }
    let opts = r(ox, top, ow, cy + 12.0 * s - top);
    out.bounds = union(out.bounds, inflate(opts, (40.0 * s) as i32));
    out.options = Some(opts);
}

fn hover_text(hv: &Hover<'_>) -> String {
    let tail = t("колесо — уровень · клик — выбрать");
    if hv.app.is_empty() {
        tail.into()
    } else {
        format!("{} · {tail}", hv.app)
    }
}

/// Mockup color into the BGRA buffer.
fn paint(c: [u8; 4]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c[2], c[1], c[0], c[3]);
    p.anti_alias = true;
    p
}

fn stroke(w: f32) -> Stroke {
    Stroke {
        width: w,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    }
}

fn rrect(x: f32, y: f32, w: f32, h: f32, rad: f32) -> Option<Path> {
    let rad = rad.min(w / 2.0).min(h / 2.0);
    let k = 0.552_284_8 * rad;
    let mut pb = PathBuilder::new();
    pb.move_to(x + rad, y);
    pb.line_to(x + w - rad, y);
    pb.cubic_to(x + w - rad + k, y, x + w, y + rad - k, x + w, y + rad);
    pb.line_to(x + w, y + h - rad);
    pb.cubic_to(
        x + w,
        y + h - rad + k,
        x + w - rad + k,
        y + h,
        x + w - rad,
        y + h,
    );
    pb.line_to(x + rad, y + h);
    pb.cubic_to(x + rad - k, y + h, x, y + h - rad + k, x, y + h - rad);
    pb.line_to(x, y + rad);
    pb.cubic_to(x, y + rad - k, x + rad - k, y, x + rad, y);
    pb.close();
    pb.finish()
}

/// Keeps a frame on screen: a full-screen window's outline is drawn just inside the monitor.
fn fit(a: Rect, mon: Rect, half_stroke: f32) -> Rect {
    let m = inflate(mon, -(half_stroke.ceil() as i32));
    let (l, t) = (a.left.max(m.left), a.top.max(m.top));
    Rect {
        left: l,
        top: t,
        width: (a.right().min(m.right()) - l).max(0),
        height: (a.bottom().min(m.bottom()) - t).max(0),
    }
}

fn rr(a: Rect, rad: f32) -> Option<Path> {
    rrect(
        a.left as f32,
        a.top as f32,
        a.width as f32,
        a.height as f32,
        rad,
    )
}

/// Three box passes ≈ a Gaussian (CSS `blur(sigma)`), on premultiplied BGRA.
/// Copy of `area` (clamped) from the buffer.
fn grab(pm: &PixmapMut<'_>, area: Rect) -> Option<(Pixmap, i32, i32)> {
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let (x0, y0) = (area.left.max(0), area.top.max(0));
    let (x1, y1) = (area.right().min(w), area.bottom().min(h));
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let mut out = Pixmap::new((x1 - x0) as u32, (y1 - y0) as u32)?;
    let src = pm.as_ref().data();
    let row = ((x1 - x0) * 4) as usize;
    for y in y0..y1 {
        let s = ((y * w + x0) * 4) as usize;
        let d = ((y - y0) as usize) * row;
        out.data_mut()[d..d + row].copy_from_slice(&src[s..s + row]);
    }
    Some((out, x0, y0))
}

/// Writes a [`grab`] copy back.
fn put(pm: &mut PixmapMut<'_>, part: &Pixmap, x0: i32, y0: i32) {
    let w = pm.width() as usize;
    let row = part.width() as usize * 4;
    let dst = pm.data_mut();
    for (y, line) in part.data().chunks_exact(row).enumerate() {
        let at = ((y0 as usize + y) * w + x0 as usize) * 4;
        dst[at..at + row].copy_from_slice(line);
    }
}

/// Painted glass, reusable while the pixels under it stay the same.
struct Pane {
    key: (Rect, u32, [u8; 4], u32, bool),
    under: Pixmap,
    done: Pixmap,
}

/// Panels on screen at once are a handful; moved ones age out.
const PANES: usize = 8;

thread_local! {
    static CACHE: RefCell<Vec<Pane>> = const { RefCell::new(Vec::new()) };
}

/// Forgets the cached glass: it holds pieces of the screenshot.
pub fn drop_cache() {
    CACHE.with(|c| *c.borrow_mut() = Vec::new());
}

/// Frosted panel, cached: blurring is most of a frame, and most frames change nothing under it.
fn glass(pm: &mut PixmapMut<'_>, at: Rect, rad: f32, tint: [u8; 4], blur: f32, shadow: bool) {
    let reach = if shadow { 2.4 * blur } else { 1.5 * blur };
    let area = inflate(at, reach.ceil() as i32);
    let Some((under, x0, y0)) = grab(pm, area) else {
        return glass_now(pm, at, rad, tint, blur, shadow);
    };
    let key = (at, rad.to_bits(), tint, blur.to_bits(), shadow);
    let hit = CACHE.with(|c| {
        let c = c.borrow();
        let same = c.iter().find(|p| p.key == key && p.under == under);
        same.map(|p| put(pm, &p.done, x0, y0)).is_some()
    });
    if hit {
        return;
    }
    glass_now(pm, at, rad, tint, blur, shadow);
    if let Some((done, ..)) = grab(pm, area) {
        CACHE.with(|c| {
            let mut c = c.borrow_mut();
            c.retain(|p| p.key != key);
            if c.len() == PANES {
                c.remove(0);
            }
            c.push(Pane { key, under, done });
        });
    }
}

/// Blurred backdrop, tint, hairline, optional drop shadow.
fn glass_now(pm: &mut PixmapMut<'_>, at: Rect, rad: f32, tint: [u8; 4], blur: f32, shadow: bool) {
    let id = Transform::identity();
    if shadow {
        let sh = Rect {
            top: at.top + (16.0 * blur / 24.0) as i32,
            ..inflate(at, -(12.0 * blur / 24.0) as i32)
        };
        let pad = (40.0 * blur / 24.0) as i32;
        if let Some(mut layer) =
            Pixmap::new((sh.width + 2 * pad) as u32, (sh.height + 2 * pad) as u32)
        {
            if let Some(p) = rrect(
                pad as f32,
                pad as f32,
                sh.width as f32,
                sh.height as f32,
                rad,
            ) {
                layer.fill_path(&p, &paint([0, 0, 0, 153]), FillRule::Winding, id, None);
            }
            annotate::box_blur(&mut layer, (pad / 3).max(1) as usize);
            pm.draw_pixmap(
                sh.left - pad,
                sh.top - pad,
                layer.as_ref(),
                &tiny_skia::PixmapPaint::default(),
                id,
                None,
            );
        }
    }
    let Some(path) = rr(at, rad) else { return };
    let b = (blur / 2.0).round() as i32;
    if let Some((mut back, x0, y0)) = grab(pm, inflate(at, b * 3)) {
        annotate::box_blur(&mut back, b.max(1) as usize);
        let shader = tiny_skia::Pattern::new(
            back.as_ref(),
            tiny_skia::SpreadMode::Pad,
            tiny_skia::FilterQuality::Nearest,
            1.0,
            Transform::from_translate(x0 as f32, y0 as f32),
        );
        let p = Paint {
            shader,
            anti_alias: true,
            ..Paint::default()
        };
        pm.fill_path(&path, &p, FillRule::Winding, id, None);
    }
    pm.fill_path(&path, &paint(tint), FillRule::Winding, id, None);
    if let Some(inner) = rr(inflate(at, -1), (rad - 1.0).max(0.0)) {
        pm.stroke_path(&inner, &paint(HAIRLINE), &stroke(1.0), id, None);
    }
}

fn text(
    pm: &mut PixmapMut<'_>,
    font: &FontRef<'_>,
    s: &str,
    x: f32,
    cy: f32,
    px: f32,
    c: [u8; 4],
) -> f32 {
    let (w, h) = annotate::text_size_in(font, s, px);
    if let Some(p) = annotate::text_path_in(font, s, Pt { x, y: cy - h / 2.0 }, px) {
        pm.fill_path(
            &p,
            &paint(c),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    w
}

/// Mockup icons (24-unit grid, 2-unit round strokes) as SVG path data.
const UNDO: [&str; 2] = [
    "M 9 14 L 4 9 L 9 4",
    "M 4 9 H 15 C 18 9 20 11 20 14 C 20 17 18 19 15 19 H 12",
];
const REDO: [&str; 2] = [
    "M 15 14 L 20 9 L 15 4",
    "M 20 9 H 9 C 6 9 4 11 4 14 C 4 17 6 19 9 19 H 12",
];
const CUBES: [&str; 5] = [
    "M 5 5 H 9 V 9 H 5 Z",
    "M 15 5 H 19 V 9 H 15 Z",
    "M 10 10 H 14 V 14 H 10 Z",
    "M 5 15 H 9 V 19 H 5 Z",
    "M 15 15 H 19 V 19 H 15 Z",
];
const SELECT: [&str; 4] = [
    "M 4 8 V 5 H 7",
    "M 17 5 H 20 V 8",
    "M 20 16 V 19 H 17",
    "M 7 19 H 4 V 16",
];
const LASSO: [&str; 2] = [
    "M 12 4 C 17 4 20 6.2 20 9.5 C 20 12.8 16 15 12 15 C 8 15 4 12.8 4 9.5 C 4 6.2 7 4 12 4 Z",
    "M 7 14 C 6 16 6 18 8 19 C 9.5 19.7 11 19 11 17.5",
];
const DROP: [&str; 1] =
    ["M 12 4 C 12 4 6 11 6 15 C 6 18.3 8.7 21 12 21 C 15.3 21 18 18.3 18 15 C 18 11 12 4 12 4 Z"];

fn tool_icon(t: Tool) -> &'static [&'static str] {
    match t {
        Tool::Arrow => &["M 5 19 L 19 5", "M 10 5 H 19 V 14"],
        Tool::Line => &["M 5 19 L 19 5"],
        Tool::Rect => {
            &["M 7 5 H 17 Q 19 5 19 7 V 17 Q 19 19 17 19 H 7 Q 5 19 5 17 V 7 Q 5 5 7 5 Z"]
        }
        Tool::Ellipse => &[
            "M 12 5 C 15.9 5 19 8.1 19 12 C 19 15.9 15.9 19 12 19 C 8.1 19 5 15.9 5 12 C 5 8.1 8.1 5 12 5 Z",
        ],
        Tool::Pen => &["M 4 20 L 8 19 L 19 8 L 16 5 L 5 16 L 4 20 Z"],
        Tool::Marker => &["M 4 20 H 9", "M 7 17 L 16 8 L 19 11 L 10 20 Z"],
        Tool::Text => &["M 6 6 H 18", "M 12 6 V 19", "M 9 19 H 15"],
        Tool::Step => &[
            "M 12 4 C 16.4 4 20 7.6 20 12 C 20 16.4 16.4 20 12 20 C 7.6 20 4 16.4 4 12 C 4 7.6 7.6 4 12 4 Z",
            "M 10.5 9.5 L 12.5 8 V 16",
        ],
        Tool::Blur => &CUBES,
    }
}

fn icon_data(b: Button) -> &'static [&'static str] {
    match b {
        Button::Copy => &[
            "M 10.5 8 H 17.5 Q 20 8 20 10.5 V 17.5 Q 20 20 17.5 20 H 10.5 Q 8 20 8 17.5 V 10.5 Q 8 8 10.5 8 Z",
            "M 16 8 V 6 Q 16 4 14 4 H 6 Q 4 4 4 6 V 14 Q 4 16 6 16 H 8",
        ],
        Button::Save => &["M 12 4 V 15", "M 7 10 L 12 15 L 17 10", "M 5 20 H 19"],
        Button::Edit => &["M 4 20 L 8 19 L 19 8 L 16 5 L 5 16 L 4 20 Z"],
        Button::Text => &[
            "M 4 8 V 5 H 7",
            "M 17 5 H 20 V 8",
            "M 20 16 V 19 H 17",
            "M 7 19 H 4 V 16",
            "M 9 9 H 15",
            "M 12 9 V 15",
        ],
        Button::Translate => &[
            "M 4 6 H 13",
            "M 8.5 4 V 6",
            "M 6 6 C 6 10 9 13 12 14",
            "M 11 6 C 11 9 9 12 5 14",
            "M 13 20 L 17 11 L 21 20",
            "M 14.5 17 H 19.5",
        ],
        Button::Cancel => &["M 6 6 L 18 18", "M 18 6 L 6 18"],
    }
}

/// Parses the absolute M/L/H/V/C/Q/Z subset used by [`icon_data`].
fn svg_path(d: &str, x: f32, y: f32, k: f32) -> Option<Path> {
    let mut pb = PathBuilder::new();
    let mut toks = d.split_whitespace().peekable();
    let (mut cx, mut cy) = (0.0f32, 0.0f32);
    let mut cmd = 'M';
    let num = |t: &mut std::iter::Peekable<std::str::SplitWhitespace<'_>>| -> f32 {
        t.next().and_then(|v| v.parse().ok()).unwrap_or(0.0)
    };
    while let Some(&t) = toks.peek() {
        if let Some(c) = t.chars().next().filter(char::is_ascii_alphabetic) {
            cmd = c;
            toks.next();
            if cmd == 'Z' {
                pb.close();
                continue;
            }
        }
        match cmd {
            'M' => {
                cx = num(&mut toks);
                cy = num(&mut toks);
                pb.move_to(x + cx * k, y + cy * k);
                cmd = 'L';
            }
            'L' => {
                cx = num(&mut toks);
                cy = num(&mut toks);
                pb.line_to(x + cx * k, y + cy * k);
            }
            'H' => {
                cx = num(&mut toks);
                pb.line_to(x + cx * k, y + cy * k);
            }
            'V' => {
                cy = num(&mut toks);
                pb.line_to(x + cx * k, y + cy * k);
            }
            'Q' => {
                let (x1, y1) = (num(&mut toks), num(&mut toks));
                cx = num(&mut toks);
                cy = num(&mut toks);
                pb.quad_to(x + x1 * k, y + y1 * k, x + cx * k, y + cy * k);
            }
            'C' => {
                let (x1, y1, x2, y2) = (
                    num(&mut toks),
                    num(&mut toks),
                    num(&mut toks),
                    num(&mut toks),
                );
                cx = num(&mut toks);
                cy = num(&mut toks);
                pb.cubic_to(
                    x + x1 * k,
                    y + y1 * k,
                    x + x2 * k,
                    y + y2 * k,
                    x + cx * k,
                    y + cy * k,
                );
            }
            _ => {
                toks.next();
            }
        }
    }
    pb.finish()
}

fn icon(pm: &mut PixmapMut<'_>, paths: &[&str], cx: f32, cy: f32, size: f32, c: [u8; 4]) {
    let k = size / 24.0;
    let (x, y) = (cx - size / 2.0, cy - size / 2.0);
    for d in paths {
        if let Some(p) = svg_path(d, x, y, k) {
            pm.stroke_path(&p, &paint(c), &stroke(2.0 * k), Transform::identity(), None);
        }
    }
}

/// Decorations over the already blitted frame. `lay` must come from [`layout`] of `sc`.
pub fn draw(pm: &mut PixmapMut<'_>, sc: &Scene<'_>, lay: &Layout) {
    let s = sc.scale;
    let f = fonts();
    let id = Transform::identity();
    if let Some(t) = &sc.tools {
        if let Some(el) = t.live {
            draw_element(pm, el);
        }
        if let Some((at, typed)) = t.caret {
            let el = annotate::Element {
                kind: annotate::Kind::Text(at, format!("{typed}|")),
                color: t.color,
                size: t.size,
            };
            draw_element(pm, &el);
        }
    }
    if let Some(hv) = &sc.hover
        && sc.selection.is_none()
    {
        let rad = if hv.target == Target::Element {
            6.0
        } else {
            12.0
        } * s;
        let mon = sc.monitor;
        if let Some(p) = rr(
            fit(inflate(hv.rect, (4.0 * s) as i32), mon, 2.0 * s),
            rad + 4.0 * s,
        ) {
            pm.stroke_path(
                &p,
                &paint([0xFF, 0x8A, 0x1F, 46]),
                &stroke(4.0 * s),
                id,
                None,
            );
        }
        if let Some(p) = rr(fit(inflate(hv.rect, (1.0 * s) as i32), mon, s), rad + s) {
            pm.stroke_path(&p, &paint(ORANGE), &stroke(2.0 * s), id, None);
        }
    }
    let lasso = sc.tools.as_ref().and_then(|t| t.lasso);
    if let Some(p) = lasso.and_then(annotate::polygon) {
        let c = [255, 255, 255, 178];
        pm.stroke_path(&p, &paint(c), &stroke(1.5 * s), id, None);
    }
    if let Some(sel) = sc.selection.filter(|_| lasso.is_none()) {
        // Design «А»: a hairline, with corners that show where to pull.
        let a = fit(sel, sc.monitor, 1.0 * s);
        if let Some(p) = rr(a, 3.0 * s) {
            pm.stroke_path(&p, &paint([255, 255, 255, 128]), &stroke(s), id, None);
        }
        let (x0, y0) = (a.left as f32, a.top as f32);
        let (x1, y1) = (a.right() as f32, a.bottom() as f32);
        let len = (14.0 * s)
            .min(a.width as f32 / 2.0)
            .min(a.height as f32 / 2.0);
        let rad = (4.0 * s).min(len);
        for (cx, cy, dx, dy) in [
            (x0, y0, 1.0, 1.0),
            (x1, y0, -1.0, 1.0),
            (x0, y1, 1.0, -1.0),
            (x1, y1, -1.0, -1.0),
        ] {
            let mut pb = PathBuilder::new();
            pb.move_to(cx, cy + dy * len);
            pb.line_to(cx, cy + dy * rad);
            pb.quad_to(cx, cy, cx + dx * rad, cy);
            pb.line_to(cx + dx * len, cy);
            if let Some(p) = pb.finish() {
                pm.stroke_path(&p, &paint([255, 255, 255, 255]), &stroke(2.0 * s), id, None);
            }
        }
    }
    if let Some((pill, label)) = &lay.size_pill {
        glass(pm, *pill, 7.0 * s, GLASS_HINT, 20.0 * s, false);
        let cy = pill.top as f32 + pill.height as f32 / 2.0;
        text(
            pm,
            &f.mono,
            label,
            pill.left as f32 + 8.0 * s,
            cy,
            11.0 * s,
            [0xC9, 0xD2, 0xE3, 255],
        );
    }
    if let (Some(pill), Some(hv)) = (lay.hover_pill, &sc.hover) {
        glass(pm, pill, pill.height as f32 / 2.0, GLASS, 20.0 * s, false);
        let cy = pill.top as f32 + pill.height as f32 / 2.0;
        let tag_w = text_w(&f.semibold, hv.target.label(), 13.0 * s) + 18.0 * s;
        let tag = r(pill.left as f32 + 6.0 * s, cy - 12.0 * s, tag_w, 24.0 * s);
        if let Some(p) = rr(tag, 12.0 * s) {
            pm.fill_path(
                &p,
                &paint([0xFF, 0x8A, 0x1F, 41]),
                FillRule::Winding,
                id,
                None,
            );
        }
        text(
            pm,
            &f.semibold,
            hv.target.label(),
            tag.left as f32 + 9.0 * s,
            cy,
            13.0 * s,
            [0xFF, 0xB7, 0x74, 255],
        );
        text(
            pm,
            &f.regular,
            &hover_text(hv),
            tag.right() as f32 + 8.0 * s,
            cy,
            13.0 * s,
            [0xC9, 0xD2, 0xE3, 255],
        );
    }
    for (panel, rad) in [(lay.toolbar, 14.0), (lay.strip, 16.0), (lay.options, 14.0)] {
        let Some(bar) = panel else { continue };
        glass(pm, bar, rad * s, GLASS, 24.0 * s, true);
        // Top highlight of the mockup's inset shadow.
        let mut pb = PathBuilder::new();
        pb.move_to(bar.left as f32 + 16.0 * s, bar.top as f32 + 1.0);
        pb.line_to(bar.right() as f32 - 16.0 * s, bar.top as f32 + 1.0);
        if let Some(top) = pb.finish() {
            pm.stroke_path(&top, &paint([255, 255, 255, 20]), &stroke(1.0), id, None);
        }
    }
    for d in &lay.dividers {
        if let Some(p) = rr(*d, 0.0) {
            pm.fill_path(&p, &paint([255, 255, 255, 31]), FillRule::Winding, id, None);
        }
    }
    let tools = sc.tools.as_ref();
    for &(b_rect, c) in &lay.buttons {
        let hovered = sc.hover_button == Some(c);
        let (bx, by, bw, bh) = (
            b_rect.left as f32,
            b_rect.top as f32,
            b_rect.width as f32,
            b_rect.height as f32,
        );
        let (cx, cy) = (bx + bw / 2.0, by + bh / 2.0);
        let selected = tools.is_some_and(|t| match c {
            Control::Tool(x) => t.tool == Some(x),
            Control::Select => t.tool.is_none() && !t.lasso_mode,
            Control::Lasso => t.tool.is_none() && t.lasso_mode,
            Control::Color(i) => t.color == COLORS[i],
            Control::Size(i) => t.size == SIZES[i],
            Control::Gauss(g) => t.blur.gauss == g,
            Control::Strength(i) => t.blur.strength == i,
            _ => false,
        });
        let disabled = tools.is_some_and(|t| {
            (c == Control::Undo && !t.can_undo) || (c == Control::Redo && !t.can_redo)
        });
        if c == Control::Action(Button::Copy) {
            let fill = if hovered {
                [0x37, 0x67, 0xEE, 255]
            } else {
                BLUE
            };
            if let Some(p) = rr(b_rect, 10.0 * s) {
                pm.fill_path(&p, &paint(fill), FillRule::Winding, id, None);
            }
            let mut pb = PathBuilder::new();
            pb.move_to(bx + 10.0 * s, by + 1.0);
            pb.line_to(bx + bw - 10.0 * s, by + 1.0);
            if let Some(top) = pb.finish() {
                pm.stroke_path(&top, &paint([255, 255, 255, 46]), &stroke(1.0), id, None);
            }
            icon(
                pm,
                icon_data(Button::Copy),
                bx + 18.5 * s,
                cy,
                17.0 * s,
                [255; 4],
            );
            text(
                pm,
                &f.semibold,
                t("Копировать"),
                bx + 35.0 * s,
                cy,
                13.5 * s,
                [255; 4],
            );
            continue;
        }
        let filled = matches!(
            c,
            Control::Tool(_) | Control::Gauss(_) | Control::Select | Control::Lasso
        );
        let fill = if selected && filled {
            Some(BLUE)
        } else if hovered && !disabled {
            Some([255, 255, 255, 20])
        } else {
            None
        };
        if let Some(fill) = fill
            && let Some(p) = rr(b_rect, 11.0 * s)
        {
            pm.fill_path(&p, &paint(fill), FillRule::Winding, id, None);
        }
        let on = if selected { [255; 4] } else { ICON_MUTED };
        match c {
            Control::Action(b) => {
                let (size, col) = if b == Button::Cancel {
                    (17.0, ICON_MUTED)
                } else {
                    (18.0, ICON)
                };
                icon(pm, icon_data(b), cx, cy, size * s, col);
            }
            Control::Tool(t) => {
                let col = if selected { [255; 4] } else { ICON };
                icon(pm, tool_icon(t), cx, cy, 19.0 * s, col);
            }
            Control::Select | Control::Lasso => {
                let col = if selected { [255; 4] } else { ICON };
                let d: &[&str] = if c == Control::Select {
                    &SELECT
                } else {
                    &LASSO
                };
                icon(pm, d, cx, cy, 19.0 * s, col);
            }
            Control::Undo | Control::Redo => {
                let col = if disabled {
                    [0xE6, 0xEB, 0xF5, 70]
                } else {
                    ICON
                };
                let d: &[&str] = if c == Control::Undo { &UNDO } else { &REDO };
                icon(pm, d, cx, cy, 19.0 * s, col);
            }
            Control::Color(i) => {
                let [r0, g0, b0] = COLORS[i];
                if let Some(p) = PathBuilder::from_circle(cx, cy, 12.0 * s) {
                    pm.fill_path(&p, &paint([r0, g0, b0, 255]), FillRule::Winding, id, None);
                    pm.stroke_path(&p, &paint([255, 255, 255, 40]), &stroke(1.0), id, None);
                }
                if (selected || hovered)
                    && let Some(p) = PathBuilder::from_circle(cx, cy, 15.0 * s)
                {
                    let a = if selected { 255 } else { 110 };
                    pm.stroke_path(&p, &paint([255, 255, 255, a]), &stroke(1.5 * s), id, None);
                }
            }
            Control::Size(i) => {
                if let Some(p) = PathBuilder::from_circle(cx, cy, (2.5 + i as f32 * 2.0) * s) {
                    pm.fill_path(&p, &paint(on), FillRule::Winding, id, None);
                }
            }
            Control::Strength(i) => {
                for k in 0..=i {
                    let h = (6.0 + 4.0 * f32::from(k)) * s;
                    let x = cx - f32::from(i) * 3.0 * s + f32::from(k) * 6.0 * s;
                    if let Some(p) = rrect(x - 1.5 * s, cy + 7.0 * s - h, 3.0 * s, h, 1.5 * s) {
                        pm.fill_path(&p, &paint(on), FillRule::Winding, id, None);
                    }
                }
            }
            Control::Gauss(g) => {
                let col = if selected { [255; 4] } else { ICON };
                let d: &[&str] = if g { &DROP } else { &CUBES };
                icon(pm, d, cx, cy, 19.0 * s, col);
            }
        }
    }
    if let Some(bar) = lay.hint_bar {
        glass(
            pm,
            bar,
            bar.height as f32 / 2.0,
            GLASS_HINT,
            20.0 * s,
            false,
        );
        let fs = 12.5 * s;
        let cy = bar.top as f32 + bar.height as f32 / 2.0;
        let mut x = bar.left as f32 + 16.0 * s;
        for &(k, v) in hints(sc) {
            x += text(pm, &f.semibold, t(k), x, cy, fs, TEXT);
            x += text(pm, &f.regular, &format!(" {}", t(v)), x, cy, fs, ICON_MUTED);
            x += 18.0 * s;
        }
    }
    if let (Some(panel), Some(l)) = (lay.loupe, &sc.loupe) {
        draw_loupe(pm, panel, l, s);
    }
}

fn draw_loupe(pm: &mut PixmapMut<'_>, panel: Rect, l: &Loupe<'_>, s: f32) {
    let f = fonts();
    let id = Transform::identity();
    if let Some(p) = rr(panel, 12.0 * s) {
        pm.fill_path(&p, &paint([22, 27, 39, 245]), FillRule::Winding, id, None);
        pm.stroke_path(&p, &paint([255, 255, 255, 40]), &stroke(1.0), id, None);
    }
    let c = loupe_cell(s);
    let (x0, y0) = (panel.left as f32 + 6.0 * s, panel.top as f32 + 6.0 * s);
    let half = LOUPE_CELLS / 2;
    for dy in -half..=half {
        for dx in -half..=half {
            let Some(px) = loupe_px(l, l.at.x + dx, l.at.y + dy) else {
                continue;
            };
            let cell = tiny_skia::Rect::from_xywh(
                x0 + (dx + half) as f32 * c,
                y0 + (dy + half) as f32 * c,
                c,
                c,
            );
            if let Some(cell) = cell {
                pm.fill_rect(cell, &paint([px[2], px[1], px[0], 255]), id, None);
            }
        }
    }
    let mid = half as f32 * c;
    if let Some(p) = rrect(x0 + mid, y0 + mid, c, c, 0.0) {
        pm.stroke_path(&p, &paint([0, 0, 0, 200]), &stroke(3.0), id, None);
        pm.stroke_path(&p, &paint([255, 255, 255, 255]), &stroke(1.0), id, None);
    }
    if let Some(px) = loupe_px(l, l.at.x, l.at.y) {
        let cy = panel.bottom() as f32 - 16.0 * s;
        if let Some(p) = PathBuilder::from_circle(x0 + 8.0 * s, cy, 7.0 * s) {
            pm.fill_path(
                &p,
                &paint([px[2], px[1], px[0], 255]),
                FillRule::Winding,
                id,
                None,
            );
            pm.stroke_path(&p, &paint([255, 255, 255, 90]), &stroke(1.0), id, None);
        }
        let hex = color_hex(px);
        text(pm, &f.mono, &hex, x0 + 22.0 * s, cy, 13.0 * s, TEXT);
        let key = text_w(&f.semibold, "I", 12.5 * s);
        let right = panel.right() as f32 - 8.0 * s - key;
        text(pm, &f.semibold, "I", right, cy, 12.5 * s, ICON_MUTED);
    }
}

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

const TOAST: (f32, f32) = (392.0, 96.0);
const TOAST_THUMB: (f32, f32) = (116.0, 76.0);
const TOAST_BUTTONS: [(ToastAction, &str); 3] = [
    (ToastAction::Editor, n("Редактор")),
    (ToastAction::Text, n("Текст")),
    (ToastAction::Folder, n("Папка")),
];

pub fn toast_size(s: f32) -> (u32, u32) {
    ((TOAST.0 * s).ceil() as u32, (TOAST.1 * s).ceil() as u32)
}

fn toast_buttons(s: f32) -> Vec<(Rect, ToastAction, &'static str)> {
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
fn fit_text(font: &FontRef<'_>, s: &str, px: f32, max: f32) -> String {
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

/// Copies `src` into the buffer inside the outline: the freeform selection stays undimmed.
/// A hard edge, the same one the exported image gets.
pub fn paint_region(pm: &mut PixmapMut<'_>, src: PixmapRef<'_>, pts: &[Pt]) {
    let Some(path) = annotate::polygon(pts) else {
        return;
    };
    let id = Transform::identity();
    let shader = tiny_skia::Pattern::new(
        src,
        tiny_skia::SpreadMode::Pad,
        tiny_skia::FilterQuality::Nearest,
        1.0,
        id,
    );
    let p = Paint {
        shader,
        blend_mode: BlendMode::Source,
        anti_alias: false,
        ..Paint::default()
    };
    pm.fill_path(&path, &p, FillRule::Winding, id, None);
}

pub fn button_at(lay: &Layout, p: Point) -> Option<Control> {
    lay.buttons
        .iter()
        .find(|(r, _)| r.contains(p))
        .map(|(_, c)| *c)
}

/// An annotation in the BGRA buffer: same element with its color channels swapped.
pub fn draw_element(pm: &mut PixmapMut<'_>, e: &annotate::Element) {
    let [r0, g0, b0] = e.color;
    let swapped = annotate::Element {
        color: [b0, g0, r0],
        ..e.clone()
    };
    annotate::draw(pm, &swapped);
}

/// Row icon of the tray menu; also says what the row does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuIcon {
    Region,
    Screen,
    Repeat,
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

const MENU_W: f32 = 300.0;
const MENU_PAD: f32 = 4.0;
const MENU_ITEM: f32 = 36.0;
/// Room around the panel for its shadow.
pub const MENU_SHADOW: f32 = 32.0;

const CIRCLE: &str =
    "M 12 3 C 17 3 21 7 21 12 C 21 17 17 21 12 21 C 7 21 3 17 3 12 C 3 7 7 3 12 3 Z";

fn menu_icon(i: MenuIcon) -> &'static [&'static str] {
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
fn menu_tops(rows: &[MenuRow]) -> (Vec<f32>, f32) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn scene(sel: Option<Rect>, confirmed: bool) -> Scene<'static> {
        let mon = Rect {
            left: 0,
            top: 0,
            width: 1440,
            height: 900,
        };
        Scene {
            scale: 1.0,
            monitor: mon,
            cursor_monitor: mon,
            selection: sel,
            confirmed,
            dragging: false,
            hover: None,
            hover_button: None,
            loupe: None,
            tools: None,
        }
    }

    #[test]
    fn toolbar_matches_mockup_geometry() {
        let sel = Rect {
            left: 368,
            top: 210,
            width: 488,
            height: 214,
        };
        let lay = layout(&scene(Some(sel), true));
        let bar = lay.toolbar.unwrap();
        assert_eq!((bar.left, bar.top, bar.height), (368, 438, 46));
        assert_eq!(lay.buttons.len(), 6);
        assert_eq!(lay.buttons[0].1, Control::Action(Button::Copy));
        let pill = lay.size_pill.as_ref().unwrap();
        assert_eq!((pill.0.left, pill.0.top), (368, 182));
        assert_eq!(pill.1, "488 × 214");
        let p = Point {
            x: lay.buttons[1].0.left + 5,
            y: lay.buttons[1].0.top + 5,
        };
        assert_eq!(button_at(&lay, p), Some(Control::Action(Button::Save)));
    }

    #[test]
    fn toolbar_flips_above_at_screen_bottom() {
        let sel = Rect {
            left: 100,
            top: 500,
            width: 300,
            height: 390,
        };
        let bar = layout(&scene(Some(sel), true)).toolbar.unwrap();
        assert!(bar.bottom() <= sel.top, "{bar:?}");
    }

    #[test]
    fn draws_into_bgra_buffer() {
        let (w, h) = (1440u32, 900u32);
        let mut buf = vec![40u8; (w * h * 4) as usize];
        let sel = Rect {
            left: 368,
            top: 210,
            width: 488,
            height: 214,
        };
        let sc = scene(Some(sel), true);
        let lay = layout(&sc);
        let mut pm = PixmapMut::from_bytes(&mut buf, w, h).unwrap();
        draw(&mut pm, &sc, &lay);
        let px = |x: u32, y: u32| {
            let i = ((y * w + x) * 4) as usize;
            [buf[i], buf[i + 1], buf[i + 2]]
        };
        let copy = lay.buttons[0].0;
        assert_eq!(
            px(copy.left as u32 + 4, copy.top as u32 + 20),
            [0xE8, 0x5B, 0x29],
            "blue as BGRA"
        );
        assert_eq!(
            px(sel.left as u32, sel.top as u32 + 8),
            [255, 255, 255],
            "white corner"
        );
        assert_ne!(
            px(sel.left as u32, sel.top as u32 + 80),
            [255, 255, 255],
            "only a hairline between corners"
        );
    }

    #[test]
    fn loupe_stays_on_the_monitor_and_shows_the_pixel() {
        let (w, h) = (400u32, 300u32);
        let mon = Rect {
            left: 0,
            top: 0,
            width: 400,
            height: 300,
        };
        let mut frame = [10u8, 20, 30, 255].repeat((w * h) as usize);
        let i = ((295 * w + 395) * 4) as usize;
        frame[i..i + 4].copy_from_slice(&[0x33, 0x22, 0x11, 255]);
        let at = Point { x: 395, y: 295 };
        let sc = Scene {
            scale: 1.0,
            monitor: mon,
            cursor_monitor: mon,
            selection: None,
            confirmed: false,
            dragging: false,
            hover: None,
            hover_button: None,
            loupe: Some(Loupe {
                at,
                frame: &frame,
                width: w,
                height: h,
            }),
            tools: None,
        };
        let lay = layout(&sc);
        let panel = lay.loupe.unwrap();
        assert!(panel.right() < at.x && panel.bottom() < at.y, "{panel:?}");
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let mut pm = PixmapMut::from_bytes(&mut buf, w, h).unwrap();
        draw(&mut pm, &sc, &lay);
        let c = loupe_cell(1.0) as i32;
        let (x, y) = (
            panel.left + 6 + 5 * c + c / 2,
            panel.top + 6 + 5 * c + c / 2,
        );
        let i = ((y as u32 * w + x as u32) * 4) as usize;
        assert_eq!(
            buf[i..i + 3],
            [0x33, 0x22, 0x11],
            "the middle cell is the pixel"
        );
        assert_eq!(color_hex(&[0x33, 0x22, 0x11, 255]), "#112233");
    }

    #[test]
    fn cached_glass_follows_what_is_under_it() {
        let (w, h) = (300u32, 200u32);
        let at = Rect {
            left: 100,
            top: 80,
            width: 100,
            height: 40,
        };
        let run = |buf: &mut [u8]| {
            let mut pm = PixmapMut::from_bytes(buf, w, h).unwrap();
            glass(&mut pm, at, 8.0, GLASS, 24.0, true);
        };
        drop_cache();
        let plain = [90u8, 90, 90, 255].repeat((w * h) as usize);
        let mut first = plain.clone();
        run(&mut first);
        let mut again = plain.clone();
        run(&mut again);
        assert_eq!(first, again, "a hit paints the same pixels");
        let mut other = plain.clone();
        let i = ((100 * w + 150) * 4) as usize;
        other[i..i + 40].fill(255);
        run(&mut other);
        assert_ne!(first, other, "a change under the panel is seen");
        drop_cache();
        assert_eq!(CACHE.with(|c| c.borrow().len()), 0);
    }

    #[test]
    fn menu_finds_rows_and_skips_lines() {
        let rows = vec![
            MenuRow::Item(MenuIcon::Region, "Выделить".into(), "PrtScn".into()),
            MenuRow::Line,
            MenuRow::Item(MenuIcon::Quit, "Выход".into(), String::new()),
        ];
        let (w, h) = menu_size(&rows, 1.0);
        assert_eq!((w, h), (364, 36 + 1 + 8 + 36 + 8 + 64));
        let at = |y: i32| menu_at(&rows, 1.0, Point { x: 100, y: y + 32 });
        assert_eq!(at(4), Some(0));
        assert_eq!(at(40), None, "the gap before the line");
        assert_eq!(at(44), None, "the line");
        assert_eq!(at(60), Some(2));
        assert_eq!(
            menu_at(&rows, 1.0, Point { x: 2, y: 40 }),
            None,
            "the shadow"
        );
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let mut pm = PixmapMut::from_bytes(&mut buf, w, h).unwrap();
        draw_menu(&mut pm, &rows, 1.0, Some(0), false);
        assert_eq!(buf[3], 0, "corners stay transparent");
    }

    #[test]
    fn toast_draws_a_shot_and_finds_its_buttons() {
        let (w, h) = toast_size(1.0);
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let shot = [200u8, 200, 200, 255].repeat(40 * 20);
        let long = "очень длинное имя файла ".repeat(10);
        let tt = Toast {
            scale: 1.0,
            title: "Снимок скопирован",
            sub: &long,
            shot: (&shot, 40, 20),
            hover: Some(ToastAction::Text),
        };
        let mut pm = PixmapMut::from_bytes(&mut buf, w, h).unwrap();
        draw_toast(&mut pm, &tt);
        let at = |x: u32, y: u32| buf[((y * w + x) * 4) as usize];
        assert_eq!(at(60, 48), 200, "the shot fills the thumbnail");
        assert_eq!(buf[3], 0, "corners stay transparent");
        let (b, a, _) = toast_buttons(1.0)[1];
        let p = Point {
            x: b.left + 2,
            y: b.top + 2,
        };
        assert_eq!(a, ToastAction::Text);
        assert_eq!(toast_at(1.0, p), Some(ToastAction::Text));
        assert_eq!(toast_at(1.0, Point { x: 5, y: 5 }), None);
        let cut = fit_text(&fonts().regular, &long, 12.0, 200.0);
        assert!(cut.ends_with('…') && text_w(&fonts().regular, &cut, 12.0) <= 200.0);
    }

    #[test]
    fn paint_region_copies_inside_the_outline_only() {
        let (w, h) = (20u32, 20u32);
        let src = vec![200u8; (w * h * 4) as usize];
        let mut buf = vec![10u8; (w * h * 4) as usize];
        let pt = |x, y| Pt { x, y };
        let pts = [pt(5.0, 5.0), pt(15.0, 5.0), pt(15.0, 15.0), pt(5.0, 15.0)];
        let from = PixmapRef::from_bytes(&src, w, h).unwrap();
        let mut pm = PixmapMut::from_bytes(&mut buf, w, h).unwrap();
        paint_region(&mut pm, from, &pts);
        let at = |x: u32, y: u32| buf[((y * w + x) * 4) as usize];
        assert_eq!(at(10, 10), 200);
        assert_eq!(at(2, 2), 10);
    }

    #[test]
    fn rail_leaves_room_for_a_selection_on_the_left() {
        let mon = Rect {
            left: 0,
            top: 0,
            width: 1440,
            height: 900,
        };
        let draw = |tool| Draw {
            monitor: mon,
            lasso_mode: false,
            lasso: None,
            tool,
            color: COLORS[0],
            size: SIZES[1],
            blur: annotate::BlurStyle::default(),
            can_undo: false,
            can_redo: false,
            live: None,
            caret: None,
        };
        let mut sc = scene(None, false);
        sc.tools = Some(draw(Some(Tool::Arrow)));
        let lay = layout(&sc);
        assert_eq!(lay.strip.unwrap().left, 24);
        assert!(lay.options.is_some());
        assert_eq!(lay.buttons[0].1, Control::Select);
        let sel = Rect {
            left: 60,
            top: 200,
            width: 300,
            height: 200,
        };
        let mut sc = scene(Some(sel), true);
        sc.tools = Some(draw(None));
        let rail = layout(&sc).strip.unwrap();
        assert_eq!(rail.right(), 1440 - 24);
    }

    #[test]
    fn svg_subset_parses_all_icons() {
        for b in [
            Button::Copy,
            Button::Save,
            Button::Edit,
            Button::Text,
            Button::Translate,
            Button::Cancel,
        ] {
            for d in icon_data(b) {
                assert!(svg_path(d, 0.0, 0.0, 1.0).is_some(), "{b:?}: {d}");
            }
        }
        for t in TOOLS {
            let more = UNDO
                .iter()
                .chain(&REDO)
                .chain(&DROP)
                .chain(&SELECT)
                .chain(&LASSO);
            for d in tool_icon(t).iter().chain(more) {
                assert!(svg_path(d, 0.0, 0.0, 1.0).is_some(), "{t:?}: {d}");
            }
        }
    }
}

#[cfg(test)]
mod preview {
    use super::*;

    #[test]
    #[ignore]
    fn render_loupe() {
        let dir = std::path::PathBuf::from(std::env::var("SNAP_PREVIEW").expect("SNAP_PREVIEW"));
        let (w, h) = (420u32, 300u32);
        let mon = Rect {
            left: 0,
            top: 0,
            width: 420,
            height: 300,
        };
        let mut frame = [54u8, 36, 26, 255].repeat((w * h) as usize);
        for y in 60..180u32 {
            for x in 80..300u32 {
                let i = ((y * w + x) * 4) as usize;
                let v = if (x + y) % 9 < 3 { 250 } else { 49 };
                frame[i..i + 4].copy_from_slice(&[v, v / 2 + 40, 255 - v / 2, 255]);
            }
        }
        let sc = Scene {
            scale: 1.0,
            monitor: mon,
            cursor_monitor: mon,
            selection: None,
            confirmed: false,
            dragging: false,
            hover: None,
            hover_button: None,
            loupe: Some(Loupe {
                at: Point { x: 120, y: 90 },
                frame: &frame,
                width: w,
                height: h,
            }),
            tools: None,
        };
        let lay = layout(&sc);
        let mut buf = frame.clone();
        let mut pm = PixmapMut::from_bytes(&mut buf, w, h).unwrap();
        draw(&mut pm, &sc, &lay);
        snapling_core::storage::save_bgra(
            &buf,
            w,
            h,
            &dir,
            std::path::Path::new("loupe"),
            snapling_core::config::Format::Png,
            90,
        )
        .unwrap();
    }

    #[test]
    #[ignore]
    fn render_tray_menu() {
        let dir = std::path::PathBuf::from(std::env::var("SNAP_PREVIEW").expect("SNAP_PREVIEW"));
        let item = |i, a: &str, b: &str| MenuRow::Item(i, a.into(), b.into());
        let rows = vec![
            item(MenuIcon::Region, "Выделить область", "PrtScn"),
            item(MenuIcon::Screen, "Весь экран", "Shift+PrtScn"),
            item(MenuIcon::Repeat, "Повторить область", "Ctrl+PrtScn"),
            item(MenuIcon::Text, "Распознать текст", "Ctrl+Shift+O"),
            MenuRow::Line,
            item(MenuIcon::History, "История", ""),
            item(MenuIcon::Folder, "Папка снимков", ""),
            item(MenuIcon::Settings, "Настройки", ""),
            MenuRow::Line,
            item(MenuIcon::Quit, "Выход", ""),
        ];
        for (name, light, scale) in [("menu_dark", false, 1.0), ("menu_light", true, 1.5)] {
            let (w, h) = menu_size(&rows, scale);
            let mut pm = tiny_skia::Pixmap::new(w, h).unwrap();
            pm.fill(tiny_skia::Color::from_rgba8(26, 36, 54, 255));
            draw_menu(&mut pm.as_mut(), &rows, scale, Some(0), light);
            let path = snapling_core::storage::save_bgra(
                pm.data(),
                w,
                h,
                &dir,
                std::path::Path::new(name),
                snapling_core::config::Format::Png,
                90,
            )
            .unwrap();
            println!("{}", path.display());
        }
    }

    /// `SNAP_PREVIEW=<dir> cargo test -p snapling-scene preview -- --ignored`: the mockup scene as PNG.
    #[test]
    #[ignore]
    fn render_mockup_scene() {
        let dir = std::path::PathBuf::from(std::env::var("SNAP_PREVIEW").expect("SNAP_PREVIEW"));
        let (w, h) = (1440u32, 900u32);
        let sel = Rect {
            left: 368,
            top: 210,
            width: 488,
            height: 214,
        };
        let mut orig = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                let panel = (96..856).contains(&x) && (80..640).contains(&y);
                let light = (900..1360).contains(&x) && (160..760).contains(&y);
                let stripe = ((x / 40 + y / 40) % 2) as u8 * 18;
                let (r, g, b) = if light {
                    (247, 249, 252)
                } else if panel {
                    (49 + stripe, 51, 56)
                } else {
                    (26, 36, 54)
                };
                orig[i..i + 4].copy_from_slice(&[b, g, r, 255]);
            }
        }
        let mut buf = orig.clone();
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                if !sel.contains(Point { x, y }) {
                    let i = ((y as u32 * w + x as u32) * 4) as usize;
                    for (c, k) in [(0, 16u32), (1, 8), (2, 4)] {
                        buf[i + c] = ((u32::from(buf[i + c]) + k) / 2) as u8;
                    }
                }
            }
        }
        let mon = Rect {
            left: 0,
            top: 0,
            width: 1440,
            height: 900,
        };
        let live = annotate::Element {
            kind: annotate::Kind::Arrow(
                Pt { x: 420.0, y: 380.0 },
                Pt { x: 700.0, y: 280.0 },
                annotate::Head::Sharp,
            ),
            color: COLORS[0],
            size: SIZES[1],
        };
        for (name, sc) in [
            (
                "selection",
                Scene {
                    scale: 1.0,
                    monitor: mon,
                    cursor_monitor: mon,
                    selection: Some(sel),
                    confirmed: true,
                    dragging: false,
                    hover: None,
                    hover_button: Some(Control::Action(Button::Edit)),
                    loupe: None,
                    tools: Some(Draw {
                        monitor: mon,
                        lasso_mode: false,
                        lasso: None,
                        tool: Some(Tool::Blur),
                        color: COLORS[0],
                        size: SIZES[1],
                        blur: annotate::BlurStyle {
                            gauss: true,
                            strength: 1,
                        },
                        can_undo: true,
                        can_redo: false,
                        live: Some(&live),
                        caret: Some((Pt { x: 400.0, y: 230.0 }, "Привет")),
                    }),
                },
            ),
            (
                "hover",
                Scene {
                    scale: 1.0,
                    monitor: mon,
                    cursor_monitor: mon,
                    selection: None,
                    confirmed: false,
                    dragging: false,
                    hover: Some(Hover {
                        rect: Rect {
                            left: 900,
                            top: 160,
                            width: 460,
                            height: 600,
                        },
                        target: Target::Window,
                        app: "Firefox",
                    }),
                    hover_button: None,
                    loupe: None,
                    tools: None,
                },
            ),
        ] {
            let mut b = buf.clone();
            let lay = layout(&sc);
            let mut pm = PixmapMut::from_bytes(&mut b, w, h).unwrap();
            draw(&mut pm, &sc, &lay);
            let path = snapling_core::storage::save_bgra(
                &b,
                w,
                h,
                &dir,
                std::path::Path::new(name),
                snapling_core::config::Format::Png,
                90,
            )
            .unwrap();
            println!("{}", path.display());
        }
    }
}

#[cfg(test)]
mod bench {
    use super::*;

    /// `cargo test --release -p snapling-scene bench_frame -- --ignored --nocapture`:
    /// one overlay frame on the widest screen, panels and glass included.
    #[test]
    #[ignore]
    fn bench_frame() {
        let (w, h) = (4480u32, 1527u32);
        let mon = Rect {
            left: 0,
            top: 0,
            width: 2560,
            height: 1527,
        };
        let sel = Rect {
            left: 600,
            top: 300,
            width: 900,
            height: 500,
        };
        let mut buf = vec![70u8; (w * h * 4) as usize];
        let sc = Scene {
            scale: 1.0,
            monitor: mon,
            cursor_monitor: mon,
            selection: Some(sel),
            confirmed: true,
            dragging: false,
            hover: None,
            hover_button: None,
            loupe: None,
            tools: Some(Draw {
                monitor: mon,
                lasso_mode: false,
                lasso: None,
                tool: Some(Tool::Arrow),
                color: COLORS[0],
                size: SIZES[1],
                blur: annotate::BlurStyle::default(),
                can_undo: true,
                can_redo: false,
                live: None,
                caret: None,
            }),
        };
        let frame = buf.clone();
        for _ in 0..4 {
            buf.copy_from_slice(&frame);
            let t0 = std::time::Instant::now();
            let lay = layout(&sc);
            let laid = t0.elapsed().as_micros();
            let mut pm = PixmapMut::from_bytes(&mut buf, w, h).unwrap();
            draw(&mut pm, &sc, &lay);
            println!("layout {laid} us, frame {} us", t0.elapsed().as_micros());
        }
    }
}

#[cfg(test)]
mod fullscreen {
    use super::*;

    #[test]
    fn fullscreen_outline_stays_visible() {
        let (w, h) = (800u32, 600u32);
        let mon = Rect {
            left: 0,
            top: 0,
            width: 800,
            height: 600,
        };
        let mut buf = vec![30u8; (w * h * 4) as usize];
        let sc = Scene {
            scale: 1.0,
            monitor: mon,
            cursor_monitor: mon,
            selection: None,
            confirmed: false,
            dragging: false,
            hover: Some(Hover {
                rect: mon,
                target: Target::Window,
                app: "Valorant",
            }),
            hover_button: None,
            loupe: None,
            tools: None,
        };
        let lay = layout(&sc);
        let mut pm = PixmapMut::from_bytes(&mut buf, w, h).unwrap();
        draw(&mut pm, &sc, &lay);
        let i = ((300 * w + 1) * 4) as usize;
        assert_eq!(
            [buf[i], buf[i + 1], buf[i + 2]],
            [0x1F, 0x8A, 0xFF],
            "orange at the left edge"
        );
    }
}
