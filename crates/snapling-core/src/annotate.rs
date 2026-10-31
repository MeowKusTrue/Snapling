//! Annotation model, undo/redo and rasterizer for the editor.
//! Shapes follow EiSen app/src/lib/editor (elements.ts, store.ts, exportPipeline.ts);
//! rendering is tiny-skia so the export is the same on every OS.

use crate::i18n::tf;
pub use ab_glyph::FontRef;
use ab_glyph::{Font, OutlineCurve, ScaleFont};
use serde::Deserialize;
use tiny_skia::{FillRule, LineCap, LineJoin, Mask, Paint, PathBuilder, Rect, Stroke, Transform};
pub use tiny_skia::{Pixmap, PixmapMut};

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Arrow(Pt, Pt, ArrowStyle),
    /// Arrow along a hand-drawn stroke.
    Curve(Vec<Pt>, ArrowStyle),
    Line(Pt, Pt),
    Rect(Pt, Pt),
    Ellipse(Pt, Pt),
    Pen(Vec<Pt>),
    /// Translucent wide stroke.
    Marker(Vec<Pt>),
    /// Position, content, font family (`""`: bundled).
    Text(Pt, String, &'static str),
    /// Numbered badge centered at the point.
    Step(Pt, u32, StepStyle),
    /// Pixelated region: unlike blur it cannot be reversed.
    Pixelate(Pt, Pt),
    /// Brush stroke that hides what is under it.
    Blur(Vec<Pt>, BlurStyle),
}

/// Arrow end, «Наконечник» in the editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Head {
    #[default]
    Sharp,
    /// Open chevron with round ends.
    Soft,
    /// Heads on both ends.
    Double,
}

impl Head {
    pub const ALL: [Head; 3] = [Head::Sharp, Head::Soft, Head::Double];

    pub fn key(self) -> &'static str {
        match self {
            Head::Sharp => "sharp",
            Head::Soft => "soft",
            Head::Double => "double",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ArrowStyle {
    pub head: Head,
    /// The head is where the drag began, not where it ended.
    pub to_start: bool,
}

/// Look of a numbered badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepStyle {
    /// Filled circle with a contrasting ring.
    #[default]
    Solid,
    /// Colored ring around a contrasting fill.
    Ring,
    /// Filled circle over a soft shadow.
    Shadow,
    /// Rounded square.
    Square,
    /// Map pin pointing down.
    Pin,
}

impl StepStyle {
    pub const ALL: [StepStyle; 5] = [
        StepStyle::Solid,
        StepStyle::Ring,
        StepStyle::Shadow,
        StepStyle::Square,
        StepStyle::Pin,
    ];

    pub fn key(self) -> &'static str {
        match self {
            StepStyle::Solid => "solid",
            StepStyle::Ring => "ring",
            StepStyle::Shadow => "shadow",
            StepStyle::Square => "square",
            StepStyle::Pin => "pin",
        }
    }
}

/// What new shapes look like, `[draw]` in the config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DrawStyle {
    pub arrow: ArrowStyle,
    /// The arrow follows the hand instead of a straight line.
    pub free: bool,
    pub step: StepStyle,
    /// Font family of new text, `""`: bundled.
    pub font: &'static str,
}

impl DrawStyle {
    /// A new arrow starting at `at`.
    pub fn new_arrow(self, at: Pt) -> Kind {
        if self.free {
            Kind::Curve(vec![at], self.arrow)
        } else {
            Kind::Arrow(at, at, self.arrow)
        }
    }
}

/// Brush blur: «cubes» (pixelate, irreversible) or Gaussian; strength 0..=2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BlurStyle {
    pub gauss: bool,
    pub strength: u8,
}

impl BlurStyle {
    /// Pixel block for cubes, box radius for Gaussian.
    pub fn amount(self) -> u32 {
        let i = usize::from(self.strength.min(2));
        if self.gauss {
            [4, 8, 14][i]
        } else {
            [8, 14, 22][i]
        }
    }
}

pub fn blur_width(size: f32) -> f32 {
    size * 8.0
}

#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    pub kind: Kind,
    pub color: [u8; 3],
    pub size: f32,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Doc {
    pub elements: Vec<Element>,
    /// Applied on export; elements outside are cut.
    pub crop: Option<(Pt, Pt)>,
}

/// Snapshot undo like EiSen's store: a doc is a few KB, so cloning is cheap.
#[derive(Debug, Default)]
pub struct History {
    pub doc: Doc,
    undo: Vec<Doc>,
    redo: Vec<Doc>,
}

impl History {
    pub fn change(&mut self, f: impl FnOnce(&mut Doc)) {
        let before = self.doc.clone();
        f(&mut self.doc);
        if self.doc != before {
            self.undo.push(before);
            self.redo.clear();
        }
    }

    /// Undo point before edits made directly on `doc` (dragging, sliders).
    pub fn snapshot(&mut self) {
        self.undo.push(self.doc.clone());
        self.redo.clear();
    }

    pub fn push(&mut self, el: Element) {
        if !tiny(&el) {
            self.change(|d| d.elements.push(el));
        }
    }

    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.undo.pop() else {
            return false;
        };
        self.redo.push(std::mem::replace(&mut self.doc, prev));
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(std::mem::replace(&mut self.doc, next));
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn next_step(&self) -> u32 {
        self.doc
            .elements
            .iter()
            .filter_map(|e| match e.kind {
                Kind::Step(_, n, _) => Some(n),
                _ => None,
            })
            .max()
            .unwrap_or(0)
            + 1
    }
}

/// Accidental clicks with a drag tool (EiSen isTiny).
fn tiny(e: &Element) -> bool {
    let short = |a: Pt, b: Pt| (a.x - b.x).abs() < 3.0 && (a.y - b.y).abs() < 3.0;
    match &e.kind {
        Kind::Arrow(a, b, _) | Kind::Line(a, b) => short(*a, *b),
        Kind::Rect(a, b) | Kind::Ellipse(a, b) | Kind::Pixelate(a, b) => {
            (a.x - b.x).abs() < 3.0 || (a.y - b.y).abs() < 3.0
        }
        Kind::Pen(p) | Kind::Marker(p) | Kind::Blur(p, _) => p.is_empty(),
        Kind::Curve(p, _) => p.iter().all(|q| short(p[0], *q)),
        Kind::Text(_, t, _) => t.trim().is_empty(),
        Kind::Step(..) => false,
    }
}

/// Drag update of a shape being drawn. Shift: lines snap to 45°, boxes become squares.
/// Strokes skip points closer than `min_step` (image px) to keep them light.
pub fn drag_to(kind: &mut Kind, p: Pt, shift: bool, min_step: f32) {
    match kind {
        Kind::Arrow(a, b, _) | Kind::Line(a, b) => *b = if shift { snap45(*a, p) } else { p },
        Kind::Rect(a, b) | Kind::Ellipse(a, b) | Kind::Pixelate(a, b) => {
            *b = if shift { square(*a, p) } else { p }
        }
        Kind::Pen(pts) | Kind::Marker(pts) | Kind::Blur(pts, _) | Kind::Curve(pts, _)
            if pts
                .last()
                .is_none_or(|l| (l.x - p.x).hypot(l.y - p.y) > min_step) =>
        {
            pts.push(p);
        }
        _ => {}
    }
}

fn snap45(a: Pt, p: Pt) -> Pt {
    let (dx, dy) = (p.x - a.x, p.y - a.y);
    let step = std::f32::consts::FRAC_PI_4;
    let ang = (dy.atan2(dx) / step).round() * step;
    let len = dx.hypot(dy);
    Pt {
        x: a.x + len * ang.cos(),
        y: a.y + len * ang.sin(),
    }
}

fn square(a: Pt, p: Pt) -> Pt {
    let s = (p.x - a.x).abs().max((p.y - a.y).abs());
    Pt {
        x: a.x + s * (p.x - a.x).signum(),
        y: a.y + s * (p.y - a.y).signum(),
    }
}

/// Corners of the box around the element's points, widened by its stroke.
pub fn bounds(e: &Element) -> (Pt, Pt) {
    let (pts, pad): (Vec<Pt>, f32) = match &e.kind {
        Kind::Arrow(a, b, _) => (vec![*a, *b], arrow_head_len(e.size)),
        Kind::Line(a, b) | Kind::Rect(a, b) | Kind::Ellipse(a, b) | Kind::Pixelate(a, b) => {
            (vec![*a, *b], e.size)
        }
        Kind::Curve(p, _) => (p.clone(), arrow_head_len(e.size)),
        Kind::Pen(p) => (p.clone(), e.size),
        Kind::Marker(p) => (p.clone(), marker_width(e.size) / 2.0),
        Kind::Blur(p, _) => (p.clone(), blur_width(e.size) / 2.0),
        Kind::Text(at, s, family) => {
            let (w, h) = text_size_family(family, s, text_px(e.size));
            let end = Pt {
                x: at.x + w,
                y: at.y + h,
            };
            (vec![*at, end], 2.0)
        }
        Kind::Step(c, n, style) => {
            let r = step_radius(e.size, *n);
            (vec![*c], if *style == StepStyle::Pin { r * 1.8 } else { r })
        }
    };
    let fold =
        |f: fn(f32, f32) -> f32, g: fn(&Pt) -> f32, init: f32| pts.iter().map(g).fold(init, f);
    (
        Pt {
            x: fold(f32::min, |p| p.x, f32::MAX) - pad,
            y: fold(f32::min, |p| p.y, f32::MAX) - pad,
        },
        Pt {
            x: fold(f32::max, |p| p.x, f32::MIN) + pad,
            y: fold(f32::max, |p| p.y, f32::MIN) + pad,
        },
    )
}

/// The element under `p`, topmost first; a few pixels of slack.
pub fn pick(doc: &Doc, p: Pt) -> Option<usize> {
    doc.elements.iter().rposition(|e| {
        let (a, b) = bounds(e);
        p.x >= a.x - 4.0 && p.x <= b.x + 4.0 && p.y >= a.y - 4.0 && p.y <= b.y + 4.0
    })
}

pub fn translate(kind: &mut Kind, dx: f32, dy: f32) {
    let mv = |p: &mut Pt| {
        p.x += dx;
        p.y += dy;
    };
    match kind {
        Kind::Arrow(a, b, _)
        | Kind::Line(a, b)
        | Kind::Rect(a, b)
        | Kind::Ellipse(a, b)
        | Kind::Pixelate(a, b) => {
            mv(a);
            mv(b);
        }
        Kind::Pen(p) | Kind::Marker(p) | Kind::Blur(p, _) | Kind::Curve(p, _) => {
            p.iter_mut().for_each(mv)
        }
        Kind::Text(a, ..) | Kind::Step(a, ..) => mv(a),
    }
}

/// Closed outline through `pts`.
pub fn polygon(pts: &[Pt]) -> Option<tiny_skia::Path> {
    let (first, rest) = pts.split_first()?;
    let mut pb = PathBuilder::new();
    pb.move_to(first.x, first.y);
    for p in rest {
        pb.line_to(p.x, p.y);
    }
    pb.close();
    pb.finish()
}

/// Makes everything outside the polygon fully transparent (BGRA, points in image pixels).
// ponytail: hard edge. A soft one needs premultiplied BGRA in every consumer of the image.
pub fn cut_polygon(bgra: &mut [u8], w: u32, h: u32, pts: &[Pt]) {
    let Some(mut mask) = Mask::new(w, h) else {
        return;
    };
    if let Some(path) = polygon(pts) {
        mask.fill_path(&path, FillRule::Winding, false, Transform::identity());
    }
    for (px, m) in bgra.as_chunks_mut::<4>().0.iter_mut().zip(mask.data()) {
        if *m == 0 {
            *px = [0; 4];
        }
    }
}

pub fn arrow_head_len(size: f32) -> f32 {
    (size * 3.2).max(12.0)
}

/// Head triangle (tip, left, right) as in EiSen arrowHeadGeometry.
pub fn arrow_head(a: Pt, b: Pt, size: f32) -> [Pt; 3] {
    const SPREAD: f32 = std::f32::consts::PI / 5.5;
    let angle = (b.y - a.y).atan2(b.x - a.x);
    let len = arrow_head_len(size);
    let side = |s: f32| Pt {
        x: b.x - len * (angle + s).cos(),
        y: b.y - len * (angle + s).sin(),
    };
    [b, side(-SPREAD), side(SPREAD)]
}

/// Shaft and head triangles of an arrow along `pts`, drawn from the first point to the last.
/// A stroke's head follows the last stretch of it, not the jitter of its final pixels.
pub fn arrow_parts(pts: &[Pt], size: f32, style: ArrowStyle) -> (Vec<Pt>, Vec<[Pt; 3]>) {
    let mut shaft = pts.to_vec();
    let mut heads = Vec::new();
    if pts.len() < 2 {
        return (shaft, heads);
    }
    let dist = |a: Pt, b: Pt| (a.x - b.x).hypot(a.y - b.y);
    let len = arrow_head_len(size);
    let mut head_on_last_point = |shaft: &mut Vec<Pt>| {
        let tip = shaft[shaft.len() - 1];
        let tail = shaft
            .iter()
            .rev()
            .find(|p| dist(**p, tip) >= len)
            .copied()
            .unwrap_or(shaft[0]);
        let head = arrow_head(tail, tip, size);
        heads.push(head);
        if style.head != Head::Soft {
            let base = Pt {
                x: (head[1].x + head[2].x) / 2.0,
                y: (head[1].y + head[2].y) / 2.0,
            };
            while shaft.len() > 1 && dist(shaft[shaft.len() - 1], tip) < dist(base, tip) {
                shaft.pop();
            }
            shaft.push(base);
        }
    };
    if style.head == Head::Double || !style.to_start {
        head_on_last_point(&mut shaft);
    }
    if style.head == Head::Double || style.to_start {
        shaft.reverse();
        head_on_last_point(&mut shaft);
        shaft.reverse();
    }
    (shaft, heads)
}

pub fn marker_width(size: f32) -> f32 {
    size * 4.0
}

pub fn step_radius(size: f32, n: u32) -> f32 {
    let digits = n.to_string().len() as f32;
    (10.0 + size * 2.5) * (1.0 + 0.32 * (digits - 1.0))
}

pub fn text_px(size: f32) -> f32 {
    14.0 + size * 4.0
}

pub fn pixel_block(size: f32) -> u32 {
    (size * 3.0).clamp(8.0, 32.0) as u32
}

/// Black or white, whichever reads on `c` (EiSen stepGlyphAndRing).
pub fn contrast(c: [u8; 3]) -> [u8; 3] {
    let lin = |v: u8| {
        let s = f32::from(v) / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    let l = 0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2]);
    if l > 0.45 {
        [15, 23, 42]
    } else {
        [255, 255, 255]
    }
}

fn font() -> FontRef<'static> {
    FontRef::try_from_slice(epaint_default_fonts::UBUNTU_LIGHT).expect("bundled font")
}

fn paint(c: [u8; 3], a: u8) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c[0], c[1], c[2], a);
    p.anti_alias = true;
    p
}

fn stroke(width: f32) -> Stroke {
    Stroke {
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    }
}

fn rect(a: Pt, b: Pt) -> Option<Rect> {
    Rect::from_ltrb(a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y))
}

fn polyline(points: &[Pt]) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    let first = points.first()?;
    pb.move_to(first.x, first.y);
    if points.len() == 1 {
        pb.line_to(first.x + 0.01, first.y);
    }
    for p in &points[1..] {
        pb.line_to(p.x, p.y);
    }
    pb.finish()
}

/// Glyph outlines of `text` with its top-left at `at`.
fn text_path(text: &str, at: Pt, px: f32) -> Option<tiny_skia::Path> {
    text_path_in(&font(), text, at, px)
}

/// Same with any font (the overlay uses Inter).
pub fn text_path_in(font: &FontRef<'_>, text: &str, at: Pt, px: f32) -> Option<tiny_skia::Path> {
    text_path_fonts(&[font], text, at, px)
}

/// The family's font, then the bundled one for the glyphs it lacks.
fn with_family<R>(family: &str, f: impl FnOnce(&[&FontRef<'_>]) -> R) -> R {
    let base = font();
    match crate::fonts::font(family) {
        Some(own) => f(&[&own, &base]),
        None => f(&[&base]),
    }
}

/// Index of the first font that has the glyph.
fn pick_font(fonts: &[&FontRef<'_>], c: char) -> usize {
    fonts.iter().position(|f| f.glyph_id(c).0 != 0).unwrap_or(0)
}

/// Line metrics come from the first font.
fn text_path_fonts(fonts: &[&FontRef<'_>], text: &str, at: Pt, px: f32) -> Option<tiny_skia::Path> {
    let first = fonts[0].as_scaled(px);
    let line = first.ascent() - first.descent() + first.line_gap();
    let mut pb = PathBuilder::new();
    for (row, s) in text.lines().enumerate() {
        let base = at.y + first.ascent() + line * row as f32;
        let mut x = at.x;
        let mut prev = None;
        for c in s.chars() {
            let which = pick_font(fonts, c);
            let font = fonts[which];
            let sf = font.as_scaled(px);
            let (sx, sy) = (sf.h_scale_factor(), sf.v_scale_factor());
            let id = font.glyph_id(c);
            if let Some((from, p)) = prev
                && from == which
            {
                x += sf.kern(p, id);
            }
            if let Some(o) = font.outline(id) {
                let map = |p: ab_glyph::Point| (x + p.x * sx, base - p.y * sy);
                let mut last: Option<(f32, f32)> = None;
                for curve in &o.curves {
                    let (start, end) = match *curve {
                        OutlineCurve::Line(a, b) => (a, b),
                        OutlineCurve::Quad(a, _, b) => (a, b),
                        OutlineCurve::Cubic(a, _, _, b) => (a, b),
                    };
                    let s0 = map(start);
                    if last != Some(s0) {
                        if last.is_some() {
                            pb.close();
                        }
                        pb.move_to(s0.0, s0.1);
                    }
                    match *curve {
                        OutlineCurve::Line(_, b) => {
                            let b = map(b);
                            pb.line_to(b.0, b.1);
                        }
                        OutlineCurve::Quad(_, c, b) => {
                            let (c, b) = (map(c), map(b));
                            pb.quad_to(c.0, c.1, b.0, b.1);
                        }
                        OutlineCurve::Cubic(_, c1, c2, b) => {
                            let (c1, c2, b) = (map(c1), map(c2), map(b));
                            pb.cubic_to(c1.0, c1.1, c2.0, c2.1, b.0, b.1);
                        }
                    }
                    last = Some(map(end));
                }
                if last.is_some() {
                    pb.close();
                }
            }
            x += sf.h_advance(id);
            prev = Some((which, id));
        }
    }
    pb.finish()
}

/// Width and height of `text` at `px`, for centering and the editor's text box.
pub fn text_size(text: &str, px: f32) -> (f32, f32) {
    text_size_in(&font(), text, px)
}

pub fn text_size_in(font: &FontRef<'_>, text: &str, px: f32) -> (f32, f32) {
    text_size_fonts(&[font], text, px)
}

/// Same in a font family, `""` being the bundled font.
pub fn text_size_family(family: &str, text: &str, px: f32) -> (f32, f32) {
    with_family(family, |fonts| text_size_fonts(fonts, text, px))
}

fn text_size_fonts(fonts: &[&FontRef<'_>], text: &str, px: f32) -> (f32, f32) {
    let sf = fonts[0].as_scaled(px);
    let line = sf.ascent() - sf.descent() + sf.line_gap();
    let w = text
        .lines()
        .map(|s| {
            s.chars()
                .map(|c| {
                    let font = fonts[pick_font(fonts, c)];
                    font.as_scaled(px).h_advance(font.glyph_id(c))
                })
                .sum::<f32>()
        })
        .fold(0.0, f32::max);
    (w, line * text.lines().count().max(1) as f32)
}

/// Fills a solid text with a contrasting halo; `weight` thickens the light face.
fn draw_text(pm: &mut PixmapMut<'_>, family: &str, text: &str, at: Pt, px: f32, color: [u8; 3]) {
    let Some(path) = with_family(family, |fonts| text_path_fonts(fonts, text, at, px)) else {
        return;
    };
    let id = Transform::identity();
    pm.stroke_path(
        &path,
        &paint(contrast(color), 230),
        &stroke(px * 0.16),
        id,
        None,
    );
    pm.fill_path(&path, &paint(color, 255), FillRule::Winding, id, None);
    pm.stroke_path(&path, &paint(color, 255), &stroke(px * 0.045), id, None);
}

fn pixelate(pm: &mut PixmapMut<'_>, a: Pt, b: Pt, block: u32) {
    let (w, h) = (pm.width(), pm.height());
    let x0 = (a.x.min(b.x).max(0.0) as u32).min(w);
    let y0 = (a.y.min(b.y).max(0.0) as u32).min(h);
    let x1 = (a.x.max(b.x).max(0.0).ceil() as u32).min(w);
    let y1 = (a.y.max(b.y).max(0.0).ceil() as u32).min(h);
    pixelate_area(
        pm.data_mut(),
        w,
        (x0, y0, x1, y1),
        (i64::from(x0), i64::from(y0)),
        block,
    );
}

/// Averages `block`-sized cells of `area`, the grid anchored at `grid`.
fn pixelate_area(
    data: &mut [u8],
    w: u32,
    area: (u32, u32, u32, u32),
    grid: (i64, i64),
    block: u32,
) {
    let (x0, y0, x1, y1) = area;
    let b = i64::from(block);
    // First cell start on the grid, possibly before the area.
    let snap = |v: u32, g: i64| i64::from(v) - (i64::from(v) - g).rem_euclid(b);
    for cy in (snap(y0, grid.1)..i64::from(y1)).step_by(block as usize) {
        for cx in (snap(x0, grid.0)..i64::from(x1)).step_by(block as usize) {
            let (bx, by) = (cx.max(i64::from(x0)) as u32, cy.max(i64::from(y0)) as u32);
            let (ex, ey) = (((cx + b) as u32).min(x1), ((cy + b) as u32).min(y1));
            let mut sum = [0u32; 4];
            for y in by..ey {
                for x in bx..ex {
                    let i = ((y * w + x) * 4) as usize;
                    for c in 0..4 {
                        sum[c] += u32::from(data[i + c]);
                    }
                }
            }
            let n = (ex - bx) * (ey - by);
            let avg = sum.map(|s| (s / n) as u8);
            for y in by..ey {
                for x in bx..ex {
                    let i = ((y * w + x) * 4) as usize;
                    data[i..i + 4].copy_from_slice(&avg);
                }
            }
        }
    }
}

/// Three box passes ≈ a Gaussian of sigma ≈ `radius`, on premultiplied pixels.
pub fn box_blur(pm: &mut Pixmap, radius: usize) {
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

/// Copy of the pixels under `(x0, y0, x1, y1)`, clamped to the buffer.
fn grab(pm: &PixmapMut<'_>, x0: i32, y0: i32, x1: i32, y1: i32) -> Option<(Pixmap, i32, i32)> {
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let (x0, y0, x1, y1) = (x0.max(0), y0.max(0), x1.min(w), y1.min(h));
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

/// Brush that pixelates (grid fixed to the image, so strokes join seamlessly) or blurs.
fn blur_brush(pm: &mut PixmapMut<'_>, pts: &[Pt], width: f32, style: BlurStyle) {
    let Some(outline) = polyline(pts).and_then(|p| p.stroke(&stroke(width), 1.0)) else {
        return;
    };
    let b = outline.bounds();
    let amount = style.amount() as i32;
    let pad = if style.gauss { amount * 3 } else { amount };
    let Some((mut src, x0, y0)) = grab(
        pm,
        b.left() as i32 - pad,
        b.top() as i32 - pad,
        b.right().ceil() as i32 + pad,
        b.bottom().ceil() as i32 + pad,
    ) else {
        return;
    };
    if style.gauss {
        box_blur(&mut src, amount as usize);
    } else {
        let (w, h) = (src.width(), src.height());
        pixelate_area(
            src.data_mut(),
            w,
            (0, 0, w, h),
            (-i64::from(x0), -i64::from(y0)),
            amount as u32,
        );
    }
    let shader = tiny_skia::Pattern::new(
        src.as_ref(),
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
    pm.fill_path(&outline, &p, FillRule::Winding, Transform::identity(), None);
}

fn draw_arrow(pm: &mut PixmapMut<'_>, pts: &[Pt], e: &Element, style: ArrowStyle) {
    let id = Transform::identity();
    let solid = paint(e.color, 255);
    let (shaft, heads) = arrow_parts(pts, e.size, style);
    if let Some(p) = polyline(&shaft) {
        pm.stroke_path(&p, &solid, &stroke(e.size), id, None);
    }
    for [tip, l, r] in heads {
        if style.head == Head::Soft {
            if let Some(p) = polyline(&[l, tip, r]) {
                pm.stroke_path(&p, &solid, &stroke(e.size), id, None);
            }
        } else if let Some(p) = polygon(&[tip, l, r]) {
            pm.fill_path(&p, &solid, FillRule::Winding, id, None);
            pm.stroke_path(&p, &solid, &stroke(e.size * 0.5), id, None);
        }
    }
}

/// Square with corners of radius `r`, centered at `c`.
fn rounded_square(c: Pt, half: f32, r: f32) -> Option<tiny_skia::Path> {
    let (x0, y0, x1, y1) = (c.x - half, c.y - half, c.x + half, c.y + half);
    let mut pb = PathBuilder::new();
    pb.move_to(x0 + r, y0);
    pb.line_to(x1 - r, y0);
    pb.quad_to(x1, y0, x1, y0 + r);
    pb.line_to(x1, y1 - r);
    pb.quad_to(x1, y1, x1 - r, y1);
    pb.line_to(x0 + r, y1);
    pb.quad_to(x0, y1, x0, y1 - r);
    pb.line_to(x0, y0 + r);
    pb.quad_to(x0, y0, x0 + r, y0);
    pb.close();
    pb.finish()
}

/// Circle of radius `r` around `c` that narrows to a point below it.
fn pin(c: Pt, r: f32) -> Option<tiny_skia::Path> {
    const K: f32 = 0.5523;
    let (tx, ty) = (c.x, c.y + r * 1.8);
    let mut pb = PathBuilder::new();
    pb.move_to(tx, ty);
    pb.cubic_to(
        c.x - 0.4 * r,
        c.y + 1.25 * r,
        c.x - r,
        c.y + 0.7 * r,
        c.x - r,
        c.y,
    );
    pb.cubic_to(c.x - r, c.y - K * r, c.x - K * r, c.y - r, c.x, c.y - r);
    pb.cubic_to(c.x + K * r, c.y - r, c.x + r, c.y - K * r, c.x + r, c.y);
    pb.cubic_to(
        c.x + r,
        c.y + 0.7 * r,
        c.x + 0.4 * r,
        c.y + 1.25 * r,
        tx,
        ty,
    );
    pb.close();
    pb.finish()
}

fn draw_step(pm: &mut PixmapMut<'_>, c: Pt, n: u32, style: StepStyle, e: &Element) {
    let id = Transform::identity();
    let r = step_radius(e.size, n);
    let other = contrast(e.color);
    let circle = PathBuilder::from_circle(c.x, c.y, r);
    let thin = Some((other, (r * 0.1).max(1.5)));
    let (path, fill, ring, ink) = match style {
        StepStyle::Solid => (circle, e.color, Some((other, (r * 0.12).max(2.0))), other),
        StepStyle::Ring => (circle, other, Some((e.color, (r * 0.2).max(2.5))), e.color),
        StepStyle::Shadow => {
            for (grow, alpha) in [(1.34, 22), (1.22, 30), (1.1, 44)] {
                if let Some(p) = PathBuilder::from_circle(c.x, c.y + r * 0.14, r * grow) {
                    pm.fill_path(&p, &paint([0, 0, 0], alpha), FillRule::Winding, id, None);
                }
            }
            (circle, e.color, None, other)
        }
        StepStyle::Square => (rounded_square(c, r * 0.92, r * 0.32), e.color, thin, other),
        StepStyle::Pin => (pin(c, r), e.color, thin, other),
    };
    if let Some(p) = path {
        pm.fill_path(&p, &paint(fill, 255), FillRule::Winding, id, None);
        if let Some((color, width)) = ring {
            pm.stroke_path(&p, &paint(color, 255), &stroke(width), id, None);
        }
    }
    let label = n.to_string();
    let px = r * 1.1;
    let (w, h) = text_size(&label, px);
    let at = Pt {
        x: c.x - w / 2.0,
        y: c.y - h / 2.0,
    };
    if let Some(p) = text_path(&label, at, px) {
        pm.fill_path(&p, &paint(ink, 255), FillRule::Winding, id, None);
        pm.stroke_path(&p, &paint(ink, 255), &stroke(px * 0.06), id, None);
    }
}

pub fn draw(pm: &mut PixmapMut<'_>, e: &Element) {
    let id = Transform::identity();
    let solid = paint(e.color, 255);
    match &e.kind {
        Kind::Line(a, b) => {
            if let Some(p) = polyline(&[*a, *b]) {
                pm.stroke_path(&p, &solid, &stroke(e.size), id, None);
            }
        }
        Kind::Arrow(a, b, style) => draw_arrow(pm, &[*a, *b], e, *style),
        Kind::Curve(pts, style) => draw_arrow(pm, pts, e, *style),
        Kind::Rect(a, b) => {
            if let Some(p) = rect(*a, *b).map(PathBuilder::from_rect) {
                pm.stroke_path(&p, &solid, &stroke(e.size), id, None);
            }
        }
        Kind::Ellipse(a, b) => {
            if let Some(p) = rect(*a, *b).and_then(PathBuilder::from_oval) {
                pm.stroke_path(&p, &solid, &stroke(e.size), id, None);
            }
        }
        Kind::Pen(pts) => {
            if let Some(p) = polyline(pts) {
                pm.stroke_path(&p, &solid, &stroke(e.size), id, None);
            }
        }
        Kind::Marker(pts) => {
            if let Some(p) = polyline(pts) {
                pm.stroke_path(
                    &p,
                    &paint(e.color, 115),
                    &stroke(marker_width(e.size)),
                    id,
                    None,
                );
            }
        }
        Kind::Text(at, s, family) => draw_text(pm, family, s, *at, text_px(e.size), e.color),
        Kind::Step(c, n, style) => draw_step(pm, *c, *n, *style, e),
        Kind::Pixelate(a, b) => pixelate(pm, *a, *b, pixel_block(e.size)),
        Kind::Blur(pts, style) => blur_brush(pm, pts, blur_width(e.size), *style),
    }
}

/// Base image converted once from BGRA (alpha 255, so already premultiplied).
pub fn pixmap_from_bgra(bgra: &[u8], w: u32, h: u32) -> Option<Pixmap> {
    let mut data = bgra.to_vec();
    for px in data.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    Pixmap::from_vec(data, tiny_skia::IntSize::from_wh(w, h)?)
}

/// Base + all elements, uncropped (the editor shows the crop as a dimmed frame).
pub fn compose(base: &Pixmap, doc: &Doc) -> Pixmap {
    let mut pm = base.clone();
    for e in &doc.elements {
        draw(&mut pm.as_mut(), e);
    }
    pm
}

/// Final image as BGRA, cropped: (bgra, width, height).
pub fn export(base: &Pixmap, doc: &Doc) -> (Vec<u8>, u32, u32) {
    let pm = compose(base, doc);
    let (w, h) = (pm.width(), pm.height());
    let (x0, y0, x1, y1) = match doc.crop {
        Some((a, b)) => {
            let c = |v: f32, max: u32| (v.max(0.0).round() as u32).min(max);
            let (x0, x1) = (c(a.x.min(b.x), w), c(a.x.max(b.x), w));
            let (y0, y1) = (c(a.y.min(b.y), h), c(a.y.max(b.y), h));
            if x1 > x0 && y1 > y0 {
                (x0, y0, x1, y1)
            } else {
                (0, 0, w, h)
            }
        }
        None => (0, 0, w, h),
    };
    let mut out = Vec::with_capacity(((x1 - x0) * (y1 - y0) * 4) as usize);
    let data = pm.data();
    for y in y0..y1 {
        let row = &data[((y * w + x0) * 4) as usize..((y * w + x1) * 4) as usize];
        for px in row.as_chunks::<4>().0 {
            out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    }
    (out, x1 - x0, y1 - y0)
}

/// Hand-off file from the tray process to the editor process:
/// `SNAPRAW1`, width, height (u32 LE), app name length (u16 LE), app name, BGRA.
pub fn write_raw(
    path: &std::path::Path,
    bgra: &[u8],
    w: u32,
    h: u32,
    app: &str,
) -> std::io::Result<()> {
    let app = &app.as_bytes()[..app.len().min(255)];
    let mut buf = Vec::with_capacity(18 + app.len() + bgra.len());
    buf.extend_from_slice(b"SNAPRAW1");
    buf.extend_from_slice(&w.to_le_bytes());
    buf.extend_from_slice(&h.to_le_bytes());
    buf.extend_from_slice(&(app.len() as u16).to_le_bytes());
    buf.extend_from_slice(app);
    buf.extend_from_slice(bgra);
    std::fs::write(path, buf)
}

pub struct Raw {
    pub bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub app: String,
}

pub fn read_raw(path: &std::path::Path) -> Result<Raw, String> {
    let bad = || tf("{0}: повреждённый файл снимка", &[("0", &path.display())]);
    let mut d = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if d.len() < 18 || &d[..8] != b"SNAPRAW1" {
        return Err(bad());
    }
    let u32_at = |i: usize| u32::from_le_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]);
    let (width, height) = (u32_at(8), u32_at(12));
    let n = usize::from(u16::from_le_bytes([d[16], d[17]]));
    let start = 18 + n;
    let size = (width as usize)
        .checked_mul(height as usize)
        .and_then(|v| v.checked_mul(4))
        .ok_or_else(bad)?;
    if width == 0 || height == 0 || d.len() != start + size {
        return Err(bad());
    }
    let app = String::from_utf8_lossy(&d[18..start]).into_owned();
    d.drain(..start);
    Ok(Raw {
        bgra: d,
        width,
        height,
        app,
    })
}

#[cfg(test)]
mod cut_tests {
    use super::*;

    #[test]
    fn cut_polygon_clears_outside_only() {
        let (w, h) = (20u32, 20u32);
        let mut px = vec![200u8; (w * h * 4) as usize];
        let pt = |x, y| Pt { x, y };
        let pts = [pt(5.0, 5.0), pt(15.0, 5.0), pt(15.0, 15.0), pt(5.0, 15.0)];
        cut_polygon(&mut px, w, h, &pts);
        let at = |x: u32, y: u32| px[((y * w + x) * 4 + 3) as usize];
        assert_eq!(at(10, 10), 200);
        assert_eq!(at(1, 1), 0);
        assert_eq!(at(18, 10), 0);
    }

    #[test]
    fn pick_takes_topmost_and_translate_moves_every_point() {
        let mut doc = Doc::default();
        let el = |kind| Element {
            kind,
            color: [1, 2, 3],
            size: 4.0,
        };
        let (a, b) = (Pt { x: 10.0, y: 10.0 }, Pt { x: 50.0, y: 50.0 });
        doc.elements.push(el(Kind::Rect(a, b)));
        doc.elements.push(el(Kind::Line(a, b)));
        assert_eq!(pick(&doc, Pt { x: 30.0, y: 30.0 }), Some(1));
        assert_eq!(pick(&doc, Pt { x: 90.0, y: 90.0 }), None);
        translate(&mut doc.elements[0].kind, 5.0, -5.0);
        assert_eq!(
            doc.elements[0].kind,
            Kind::Rect(Pt { x: 15.0, y: 5.0 }, Pt { x: 55.0, y: 45.0 })
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = 120;
    const H: u32 = 80;

    fn base() -> Pixmap {
        let bgra: Vec<u8> = (0..W * H)
            .flat_map(|i| [(i % 251) as u8, 40, 200, 255])
            .collect();
        pixmap_from_bgra(&bgra, W, H).unwrap()
    }

    fn el(kind: Kind) -> Element {
        Element {
            kind,
            color: [255, 59, 48],
            size: 4.0,
        }
    }

    fn px(pm: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * pm.width() + x) * 4) as usize;
        pm.data()[i..i + 4].try_into().unwrap()
    }

    fn p(x: f32, y: f32) -> Pt {
        Pt { x, y }
    }

    #[test]
    fn undo_redo_and_steps() {
        let mut h = History::default();
        assert!(!h.undo());
        h.push(el(Kind::Step(
            p(10.0, 10.0),
            h.next_step(),
            StepStyle::Solid,
        )));
        h.push(el(Kind::Step(p(30.0, 10.0), h.next_step(), StepStyle::Pin)));
        assert_eq!(h.next_step(), 3);
        h.push(el(Kind::Rect(p(0.0, 0.0), p(1.0, 1.0))));
        assert_eq!(h.doc.elements.len(), 2, "tiny shapes are dropped");
        assert!(h.undo());
        assert_eq!(h.next_step(), 2);
        assert!(h.redo());
        assert!(!h.redo());
        h.undo();
        h.change(|d| d.crop = Some((p(0.0, 0.0), p(50.0, 50.0))));
        assert!(!h.can_redo(), "new change clears redo");
    }

    #[test]
    fn arrow_head_goes_where_the_style_says() {
        let (a, b) = (p(10.0, 40.0), p(110.0, 40.0));
        let tip = |to_start| {
            let style = ArrowStyle {
                head: Head::Sharp,
                to_start,
            };
            let (_, heads) = arrow_parts(&[a, b], 4.0, style);
            assert_eq!(heads.len(), 1);
            heads[0][0]
        };
        assert_eq!(tip(false), b);
        assert_eq!(tip(true), a);
        let both = ArrowStyle {
            head: Head::Double,
            to_start: false,
        };
        assert_eq!(arrow_parts(&[a, b], 4.0, both).1.len(), 2);
    }

    #[test]
    fn stroke_arrow_head_follows_the_last_stretch_not_the_jitter() {
        let pts = [p(0.0, 0.0), p(50.0, 0.0), p(100.0, 0.0), p(100.5, 3.0)];
        let (shaft, heads) = arrow_parts(&pts, 4.0, ArrowStyle::default());
        let [tip, l, r] = heads[0];
        assert_eq!(tip, pts[3]);
        assert!((l.x + r.x) / 2.0 < tip.x, "points along the stroke");
        assert!(shaft.len() >= 2);
    }

    #[test]
    fn a_curve_needs_more_than_a_click() {
        let curve = |pts: Vec<Pt>| Element {
            kind: Kind::Curve(pts, ArrowStyle::default()),
            color: [0; 3],
            size: 2.0,
        };
        assert!(tiny(&curve(vec![p(5.0, 5.0)])));
        assert!(!tiny(&curve(vec![p(5.0, 5.0), p(40.0, 5.0)])));
    }

    #[test]
    fn every_step_style_paints_its_digit_area() {
        let b = base();
        for style in StepStyle::ALL {
            let mut doc = Doc::default();
            doc.elements.push(Element {
                kind: Kind::Step(p(60.0, 40.0), 1, style),
                color: [200, 0, 0],
                size: 4.0,
            });
            assert_ne!(compose(&b, &doc).data(), b.data(), "{style:?}");
        }
    }

    #[test]
    fn shapes_paint_their_color() {
        let b = base();
        for kind in [
            Kind::Line(p(10.0, 40.0), p(110.0, 40.0)),
            Kind::Arrow(p(10.0, 40.0), p(110.0, 40.0), ArrowStyle::default()),
            Kind::Rect(p(20.0, 20.0), p(100.0, 60.0)),
            Kind::Pen(vec![p(10.0, 40.0), p(60.0, 40.0), p(110.0, 40.0)]),
        ] {
            let mut doc = Doc::default();
            let probe = match kind {
                Kind::Rect(..) => (60, 20),
                _ => (60, 40),
            };
            doc.elements.push(el(kind.clone()));
            let pm = compose(&b, &doc);
            assert_eq!(px(&pm, probe.0, probe.1), [255, 59, 48, 255], "{kind:?}");
        }
    }

    #[test]
    fn marker_is_translucent() {
        let b = base();
        let mut doc = Doc::default();
        doc.elements
            .push(el(Kind::Marker(vec![p(10.0, 40.0), p(110.0, 40.0)])));
        let got = px(&compose(&b, &doc), 60, 40);
        assert_ne!(got, px(&b, 60, 40));
        assert_ne!(got, [255, 59, 48, 255]);
    }

    #[test]
    fn pixelate_makes_uniform_blocks() {
        let b = base();
        let mut doc = Doc::default();
        doc.elements
            .push(el(Kind::Pixelate(p(0.0, 0.0), p(24.0, 24.0))));
        let pm = compose(&b, &doc);
        let block = pixel_block(4.0);
        assert_eq!(px(&pm, 0, 0), px(&pm, block - 1, block - 1));
        assert_ne!(px(&b, 0, 0), px(&b, block - 1, 0), "source varies");
        assert_eq!(px(&pm, 50, 50), px(&b, 50, 50), "outside untouched");
    }

    #[test]
    fn text_and_steps_render_glyphs() {
        let b = base();
        for kind in [
            Kind::Text(p(5.0, 5.0), "Привет\nmir".into(), ""),
            Kind::Step(p(60.0, 40.0), 12, StepStyle::Solid),
        ] {
            let mut doc = Doc::default();
            doc.elements.push(el(kind.clone()));
            let pm = compose(&b, &doc);
            let changed = pm
                .data()
                .chunks(4)
                .zip(b.data().chunks(4))
                .filter(|(a, b)| a != b)
                .count();
            assert!(changed > 200, "{kind:?}: {changed}");
        }
        let (w1, h1) = text_size("ab", 30.0);
        let (w2, h2) = text_size("abab\nx", 30.0);
        assert!(w2 > w1 && h2 > h1);
    }

    #[test]
    fn export_crops_to_bgra() {
        let b = base();
        let mut doc = Doc {
            crop: Some((p(100.0, 70.0), p(10.0, 20.0))),
            ..Doc::default()
        };
        let (bgra, w, h) = export(&b, &doc);
        assert_eq!((w, h), (90, 50));
        assert_eq!(bgra.len(), 90 * 50 * 4);
        let src = px(&b, 10, 20);
        assert_eq!(&bgra[..4], &[src[2], src[1], src[0], 255]);
        doc.crop = Some((p(5.0, 5.0), p(5.0, 50.0)));
        assert_eq!(export(&b, &doc).1, W, "empty crop = whole image");
    }

    #[test]
    fn blur_brush_changes_only_the_stroke() {
        let b = base();
        for gauss in [false, true] {
            let style = BlurStyle { gauss, strength: 1 };
            let mut doc = Doc::default();
            doc.elements.push(Element {
                kind: Kind::Blur(vec![p(10.0, 40.0), p(110.0, 40.0)], style),
                color: [0, 0, 0],
                size: 2.0,
            });
            let pm = compose(&b, &doc);
            assert_ne!(
                px(&pm, 60, 40),
                px(&b, 60, 40),
                "gauss={gauss}: under the stroke"
            );
            assert_eq!(px(&pm, 60, 5), px(&b, 60, 5), "gauss={gauss}: far from it");
        }
        let mut doc = Doc::default();
        doc.elements.push(Element {
            kind: Kind::Blur(vec![p(20.0, 40.0), p(100.0, 40.0)], BlurStyle::default()),
            color: [0, 0, 0],
            size: 4.0,
        });
        let pm = compose(&b, &doc);
        assert_eq!(
            px(&pm, 32, 32),
            px(&pm, 39, 39),
            "cubes sit on the image grid"
        );
    }

    #[test]
    fn raw_roundtrip_and_rejects_garbage() {
        let path = std::env::temp_dir().join(format!("snapling-raw-{}.raw", std::process::id()));
        let bgra = vec![7u8; 3 * 2 * 4];
        write_raw(&path, &bgra, 3, 2, "Discord.exe").unwrap();
        let r = read_raw(&path).unwrap();
        assert_eq!(
            (r.width, r.height, r.app.as_str(), r.bgra),
            (3, 2, "Discord.exe", bgra)
        );
        std::fs::write(&path, b"SNAPRAW1\xff\xff\xff\xff\xff\xff\xff\xff\0\0").unwrap();
        assert!(read_raw(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
