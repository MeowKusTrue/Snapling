//! The selection overlay drawn 1:1 from the «Выделение области» mockup, on any OS.
//! The platform window blits the frozen frame (dimmed outside, original inside the
//! selection) into a BGRA buffer, then calls [`draw`]. Colors are written as BGRA.

use snapling_core::annotate::{self, FontRef, Pt};
use snapling_core::selection::{Point, Rect};
use std::sync::OnceLock;
use tiny_skia::{
    FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, PixmapMut, Stroke, Transform,
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
            Target::Window => "Окно",
            Target::Element => "Элемент",
            Target::Screen => "Экран",
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
    pub hover_button: Option<Button>,
}

#[derive(Debug, Default, Clone)]
pub struct Layout {
    pub buttons: Vec<(Rect, Button)>,
    /// Everything drawn outside the selection itself, inflated for blur and shadows.
    pub bounds: Option<Rect>,
    toolbar: Option<Rect>,
    size_pill: Option<(Rect, String)>,
    hover_pill: Option<Rect>,
    hint_bar: Option<Rect>,
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
const BTN: f32 = 40.0;
const BAR_PAD: f32 = 6.0;
const GAP: f32 = 4.0;

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

const HINTS: [(&str, &str); 7] = [
    ("Enter", "копировать"),
    ("S", "сохранить"),
    ("E", "редактор"),
    ("O", "текст"),
    ("R", "перевод"),
    ("Shift", "квадрат"),
    ("Esc", "отмена"),
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
            regular: f(include_bytes!("../../../assets/fonts/Inter-Regular.ttf")),
            semibold: f(include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf")),
            mono: f(include_bytes!(
                "../../../assets/fonts/JetBrainsMono-SemiBold.ttf"
            )),
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
    (12.0 + 18.0 + 8.0 + 16.0) * s + text_w(&fonts().semibold, "Копировать", 14.0 * s)
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
        let (w, h) = (text_w(&f.mono, &label, 12.0 * s) + 20.0 * s, 24.0 * s);
        let pill = above_or_inside(sel, sc.monitor, w, h, 14.0 * s, s);
        out.bounds = union(out.bounds, inflate(pill, blur));
        out.size_pill = Some((pill, label));
        out.bounds = union(out.bounds, inflate(sel, (10.0 * s) as i32));
        if sc.confirmed && !sc.dragging {
            let tw = 2.0 * BAR_PAD * s + primary_w(s) + 5.0 * BTN * s + 9.0 * s + 6.0 * GAP * s;
            let th = (BTN + 2.0 * BAR_PAD) * s;
            let gap = 18.0 * s;
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
            for b in BAR {
                let w = match b {
                    Some(Button::Copy) => primary_w(s),
                    Some(_) => BTN * s,
                    None => 9.0 * s,
                };
                if let Some(b) = b {
                    out.buttons.push((r(bx, by, w, BTN * s), b));
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
    if !sc.dragging {
        let fs = 12.5 * s;
        let mut w = 32.0 * s + 18.0 * s * (HINTS.len() - 1) as f32;
        for (k, v) in HINTS {
            w += text_w(&f.semibold, k, fs) + text_w(&f.regular, &format!(" {v}"), fs);
        }
        let h = 34.0 * s;
        let m = sc.cursor_monitor;
        let bar = r(
            m.left as f32 + (m.width as f32 - w) / 2.0,
            m.bottom() as f32 - 24.0 * s - h,
            w,
            h,
        );
        out.bounds = union(out.bounds, inflate(bar, blur));
        out.hint_bar = Some(bar);
    }
    out
}

fn hover_text(hv: &Hover<'_>) -> String {
    let tail = "колесо — уровень · клик — выбрать";
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
fn box_blur(pm: &mut Pixmap, radius: usize) {
    let (w, h) = (pm.width() as usize, pm.height() as usize);
    if radius == 0 || w == 0 || h == 0 {
        return;
    }
    let data = pm.data_mut();
    let mut tmp = vec![0u8; data.len()];
    for _ in 0..3 {
        blur_pass(data, &mut tmp, w, h, radius, true);
        blur_pass(&tmp, data, w, h, radius, false);
    }
}

fn blur_pass(src: &[u8], dst: &mut [u8], w: usize, h: usize, rad: usize, horizontal: bool) {
    let (len, lines) = if horizontal { (w, h) } else { (h, w) };
    let at = |line: usize, i: usize| {
        if horizontal {
            (line * w + i) * 4
        } else {
            (i * w + line) * 4
        }
    };
    let win = (2 * rad + 1) as u32;
    for line in 0..lines {
        let mut sum = [0u32; 4];
        for k in 0..win as usize {
            let i = at(line, k.saturating_sub(rad).min(len - 1));
            for c in 0..4 {
                sum[c] += u32::from(src[i + c]);
            }
        }
        for i in 0..len {
            let o = at(line, i);
            for c in 0..4 {
                dst[o + c] = (sum[c] / win) as u8;
            }
            let add = at(line, (i + rad + 1).min(len - 1));
            let sub = at(line, i.saturating_sub(rad));
            for c in 0..4 {
                sum[c] = sum[c] + u32::from(src[add + c]) - u32::from(src[sub + c]);
            }
        }
    }
}

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

/// Frosted panel: blurred backdrop, tint, hairline, optional drop shadow.
fn glass(pm: &mut PixmapMut<'_>, at: Rect, rad: f32, tint: [u8; 4], blur: f32, shadow: bool) {
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
            box_blur(&mut layer, (pad / 3).max(1) as usize);
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
        box_blur(&mut back, b.max(1) as usize);
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

fn icon(pm: &mut PixmapMut<'_>, b: Button, cx: f32, cy: f32, size: f32, c: [u8; 4]) {
    let k = size / 24.0;
    let (x, y) = (cx - size / 2.0, cy - size / 2.0);
    for d in icon_data(b) {
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
    if let Some(sel) = sc.selection {
        let (x, y, w, h) = (
            sel.left as f32,
            sel.top as f32,
            sel.width as f32,
            sel.height as f32,
        );
        let ring = |d: f32| {
            let f = fit(
                r(x - d, y - d, w + 2.0 * d, h + 2.0 * d),
                sc.monitor,
                0.75 * s,
            );
            rr(f, 2.0 + d)
        };
        if let Some(p) = ring(2.25 * s) {
            pm.stroke_path(&p, &paint([17, 24, 39, 140]), &stroke(1.5 * s), id, None);
        }
        if let Some(p) = ring(0.75 * s) {
            pm.stroke_path(&p, &paint([255, 255, 255, 255]), &stroke(1.5 * s), id, None);
        }
        if sc.confirmed {
            for (hx, hy) in [(x, y), (x + w, y), (x, y + h), (x + w, y + h)] {
                if let Some(p) = PathBuilder::from_circle(hx, hy + 2.0 * s, 8.0 * s) {
                    pm.fill_path(&p, &paint([0, 0, 0, 50]), FillRule::Winding, id, None);
                }
                if let Some(p) = PathBuilder::from_circle(hx, hy, 7.0 * s) {
                    pm.fill_path(
                        &p,
                        &paint([255, 255, 255, 255]),
                        FillRule::Winding,
                        id,
                        None,
                    );
                    pm.stroke_path(&p, &paint(BLUE), &stroke(2.0 * s), id, None);
                }
            }
        }
    }
    if let Some((pill, label)) = &lay.size_pill {
        glass(pm, *pill, pill.height as f32 / 2.0, GLASS, 20.0 * s, false);
        let cy = pill.top as f32 + pill.height as f32 / 2.0;
        text(
            pm,
            &f.mono,
            label,
            pill.left as f32 + 10.0 * s,
            cy,
            12.0 * s,
            TEXT,
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
    if let Some(bar) = lay.toolbar {
        glass(pm, bar, 16.0 * s, GLASS, 24.0 * s, true);
        {
            // Top highlight of the mockup's inset shadow.
            let mut pb = PathBuilder::new();
            pb.move_to(bar.left as f32 + 16.0 * s, bar.top as f32 + 1.0);
            pb.line_to(bar.right() as f32 - 16.0 * s, bar.top as f32 + 1.0);
            if let Some(top) = pb.finish() {
                pm.stroke_path(&top, &paint([255, 255, 255, 20]), &stroke(1.0), id, None);
            }
        }
        for &(b_rect, b) in &lay.buttons {
            let hovered = sc.hover_button == Some(b);
            let (bx, by, bw, bh) = (
                b_rect.left as f32,
                b_rect.top as f32,
                b_rect.width as f32,
                b_rect.height as f32,
            );
            let cy = by + bh / 2.0;
            if b == Button::Copy {
                let fill = if hovered {
                    [0x37, 0x67, 0xEE, 255]
                } else {
                    BLUE
                };
                if let Some(p) = rr(b_rect, 11.0 * s) {
                    pm.fill_path(&p, &paint(fill), FillRule::Winding, id, None);
                }
                let mut pb = PathBuilder::new();
                pb.move_to(bx + 11.0 * s, by + 1.0);
                pb.line_to(bx + bw - 11.0 * s, by + 1.0);
                if let Some(top) = pb.finish() {
                    pm.stroke_path(&top, &paint([255, 255, 255, 46]), &stroke(1.0), id, None);
                }
                icon(
                    pm,
                    b,
                    bx + 12.0 * s + 9.0 * s,
                    cy,
                    18.0 * s,
                    [255, 255, 255, 255],
                );
                text(
                    pm,
                    &f.semibold,
                    "Копировать",
                    bx + 38.0 * s,
                    cy,
                    14.0 * s,
                    [255, 255, 255, 255],
                );
            } else {
                if hovered && let Some(p) = rr(b_rect, 11.0 * s) {
                    pm.fill_path(&p, &paint([255, 255, 255, 20]), FillRule::Winding, id, None);
                }
                let (size, c) = if b == Button::Cancel {
                    (18.0, ICON_MUTED)
                } else {
                    (19.0, ICON)
                };
                icon(pm, b, bx + bw / 2.0, cy, size * s, c);
            }
        }
        if let Some(&(cancel, _)) = lay.buttons.iter().find(|(_, b)| *b == Button::Cancel) {
            let dx = cancel.left as f32 - (GAP + 4.5) * s;
            let cy = cancel.top as f32 + cancel.height as f32 / 2.0;
            let mut pb = PathBuilder::new();
            pb.move_to(dx, cy - 11.0 * s);
            pb.line_to(dx, cy + 11.0 * s);
            if let Some(p) = pb.finish() {
                pm.stroke_path(
                    &p,
                    &paint([255, 255, 255, 31]),
                    &Stroke {
                        width: s,
                        ..Stroke::default()
                    },
                    id,
                    None,
                );
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
        for (k, v) in HINTS {
            x += text(pm, &f.semibold, k, x, cy, fs, TEXT);
            x += text(pm, &f.regular, &format!(" {v}"), x, cy, fs, ICON_MUTED);
            x += 18.0 * s;
        }
    }
}

pub fn button_at(lay: &Layout, p: Point) -> Option<Button> {
    lay.buttons
        .iter()
        .find(|(r, _)| r.contains(p))
        .map(|(_, b)| *b)
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
        assert_eq!((bar.left, bar.top, bar.height), (368, 442, 52));
        assert_eq!(lay.buttons.len(), 6);
        assert_eq!(lay.buttons[0].1, Button::Copy);
        let pill = lay.size_pill.as_ref().unwrap();
        assert_eq!((pill.0.left, pill.0.top), (368, 172));
        assert_eq!(pill.1, "488 × 214");
        let p = Point {
            x: lay.buttons[1].0.left + 5,
            y: lay.buttons[1].0.top + 5,
        };
        assert_eq!(button_at(&lay, p), Some(Button::Save));
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
            px(sel.left as u32 - 3, sel.top as u32),
            [255, 255, 255],
            "white handle"
        );
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
    }
}

#[cfg(test)]
mod preview {
    use super::*;

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
                    hover_button: Some(Button::Edit),
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
