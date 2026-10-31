//! The selection overlay drawn 1:1 from the «Выделение области» mockup, on any OS.
//! The platform window blits the frozen frame (dimmed outside, original inside the
//! selection) into a BGRA buffer, then calls [`draw`]. Colors are written as BGRA.

mod draw;
mod glass;
mod icons;
mod layout;
mod menu;
mod paint;
mod pill;
pub mod session;
mod toast;

#[cfg(test)]
mod bench;
#[cfg(test)]
mod fullscreen;
#[cfg(test)]
mod preview;
#[cfg(test)]
mod tests;

pub use draw::*;
pub use glass::*;
use icons::*;
pub use layout::*;
pub use menu::*;
use paint::*;
pub use pill::*;
pub use toast::*;

use snapling_core::annotate::{self, FontRef, Pt};
use snapling_core::color::ColorFormat;
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
    Pin,
    Record,
    Scroll,
    /// Brings the tool rail back next to a finished selection.
    Draw,
    Blur,
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
    /// Eyedropper on or off.
    Picker,
    /// Text format of a picked color, an index into `ColorFormat::ALL`.
    ColorFormat(usize),
}

/// Tool rail and annotations, on screen from the first frame.
pub struct Draw<'a> {
    /// Monitor the rail stays on: the one under the cursor at open.
    pub monitor: Rect,
    /// The rail was dragged here; `None`: its usual place.
    pub rail_at: Option<Point>,
    pub rail_hidden: bool,
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
    /// Eyedropper is on: a click takes the color.
    pub picker: bool,
    pub color_format: ColorFormat,
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
    pub format: ColorFormat,
}

/// A BGRA pixel as text.
pub fn color_text(px: &[u8], format: ColorFormat) -> String {
    format.text([px[2], px[1], px[0]])
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
    /// Top of the rail, the part that drags it.
    pub grip: Option<Rect>,
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
const BAR: [Option<Button>; 12] = [
    Some(Button::Copy),
    Some(Button::Save),
    Some(Button::Edit),
    Some(Button::Text),
    Some(Button::Translate),
    Some(Button::Pin),
    Some(Button::Record),
    Some(Button::Scroll),
    Some(Button::Draw),
    Some(Button::Blur),
    None,
    Some(Button::Cancel),
];

const HINTS_SELECTED: [(&str, &str); 11] = [
    ("Enter", n("копировать")),
    ("S", n("сохранить")),
    ("E", n("редактор")),
    ("O", n("текст")),
    ("P", n("закрепить")),
    ("G", n("запись")),
    ("D", n("длинный")),
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
