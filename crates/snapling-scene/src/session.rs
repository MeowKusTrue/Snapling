//! Everything the selection overlay does, with no window: input goes in, a frame and an outcome
//! come out. Interaction follows EiSen's overlay (MIT, © 2026 Tran Trong Thuc). The windows that
//! feed it are `snapling-win/overlay.rs` and `snapling-overlay`.

use snapling_core::annotate::{
    self, BlurStyle, DrawStyle, Element, Head, History, Kind, Pt, StepStyle,
};
use snapling_core::color::ColorFormat;
use snapling_core::selection::{Corner, Hit, HoverChain, MIN_SELECTION_PX, Point, Rect, Selection};
use std::time::{Duration, Instant};

use crate::{
    Button, COLORS, Control, Draw, Hover, Layout, Loupe, SIZES, Scene, TOOLS, Target, Tool,
    dim_into,
};

const DIM_SELECT: u32 = 108;
const DIM_PAINT: u32 = 56;

/// What the user did with the selection.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Copy,
    Save,
    Edit,
    Text,
    Translate,
    Pin,
    Record,
    Scroll,
    /// `I`: the image is the one pixel under the cursor.
    Color,
}

/// A captured picture, BGRA; `x`, `y` are its place on the desktop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shot {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
pub struct Options {
    /// DPI scale of the monitor; mockup sizes are at 1.0. 0: ask the window.
    pub scale: f32,
    /// Selecting ends the overlay with this outcome (OCR hotkeys).
    pub auto: Option<Outcome>,
    /// Selecting copies at once, skipping the action toolbar.
    pub instant: bool,
    pub loupe: bool,
    pub picker: bool,
    pub color_format: ColorFormat,
    pub blur: BlurStyle,
    pub style: DrawStyle,
    /// Plays the stretch sound at this level, `snapling_core::sound::SELECT_LEVELS` of them.
    pub on_stretch: Option<fn(usize)>,
    /// Windows under the overlay, topmost first, with their app names.
    pub targets: Vec<(Rect, String)>,
    /// Monitors and their DPI scales; empty: the shot is one monitor of `scale`.
    pub monitors: Vec<(Rect, f32)>,
    /// Cursor at open.
    pub mouse: Point,
    /// Where the tool rail was left, on the desktop; `None`: its usual place.
    pub rail_at: Option<Point>,
}

/// What the panels change and the next overlay should remember.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prefs {
    pub blur: BlurStyle,
    pub style: DrawStyle,
    pub picker: bool,
    pub color_format: ColorFormat,
    /// Desktop position of the rail.
    pub rail_at: Option<Point>,
}

/// The window asks for the elements of the hovered target.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Ask {
    /// Index in `Options::targets`.
    pub window: usize,
    /// The cursor came to this target just now: its app name is wanted too.
    pub entered: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    Escape,
    Enter,
    Backspace,
    Left,
    Right,
    Up,
    Down,
    Char(char),
}

/// Modifier state with a key or a mouse event.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    Stay,
    /// Closed without a result.
    Cancel,
    Done(Outcome),
}

struct Pen {
    tool: Option<Tool>,
    lasso_mode: bool,
    color: [u8; 3],
    size: f32,
    blur: BlurStyle,
    style: DrawStyle,
    history: History,
    live: Option<Element>,
    typing: Option<(Pt, String)>,
    picker: bool,
    color_format: ColorFormat,
}

/// A grain of the stretch sound per this much growth, no closer than the gap: each one cuts off
/// the one before. `sound::SELECT_LEVELS` caps the level.
const STRETCH_STEP: i32 = 30;
const STRETCH_GAP: Duration = Duration::from_millis(55);
const STRETCH_PER_LEVEL: i32 = 180;

pub struct Session {
    shot: Shot,
    /// The shot with committed shapes: what the selection shows and what is copied.
    annot: Vec<u8>,
    dim: Vec<u8>,
    dim_level: Option<u32>,
    sel: Selection,
    mouse: Point,
    layout: Layout,
    hover_btn: Option<Control>,
    pen: Pen,
    /// The press started a selection, so its release may finish one.
    armed: bool,
    lasso: Vec<Pt>,
    stroke: Vec<Pt>,
    /// Monitor of the rail.
    rail: Rect,
    rail_at: Option<Point>,
    /// The position the next overlay gets: the end of the last drag.
    rail_kept: Option<Point>,
    on_stretch: Option<fn(usize)>,
    /// Width plus height at the last grain of the stretch sound, and when it played.
    stretch: (i32, Option<Instant>),
    /// Press point inside the rail while it is being dragged.
    rail_grab: Option<Point>,
    /// The rail stays out next to a finished selection.
    rail_open: bool,
    chain: HoverChain,
    chain_win: Option<usize>,
    /// App name of the hovered window.
    app: String,
    ask: Option<Ask>,
    /// The next repaint takes the whole screen.
    repaint_all: bool,
    opts: Options,
}

fn pen_point(p: Point) -> Pt {
    Pt {
        x: p.x as f32,
        y: p.y as f32,
    }
}

fn clip(r: Rect, to: Rect) -> Rect {
    let l = r.left.max(to.left);
    let t = r.top.max(to.top);
    Rect {
        left: l,
        top: t,
        width: (r.right().min(to.right()) - l).max(0),
        height: (r.bottom().min(to.bottom()) - t).max(0),
    }
}

/// Copies `area` between two frames `width` pixels wide.
fn copy_area(dst: &mut [u8], src: &[u8], width: u32, area: Rect) {
    let row = width as usize * 4;
    let (l, e) = (area.left as usize * 4, area.right() as usize * 4);
    for y in area.top as usize..area.bottom() as usize {
        let at = y * row;
        dst[at + l..at + e].copy_from_slice(&src[at + l..at + e]);
    }
}

impl Session {
    pub fn new(shot: Shot, opts: Options) -> Self {
        let len = shot.bgra.len();
        let rail_at = opts.rail_at.map(|p| Point {
            x: p.x - shot.x,
            y: p.y - shot.y,
        });
        let mouse = opts.mouse;
        let mut s = Self {
            annot: shot.bgra.clone(),
            dim: vec![0; len],
            dim_level: None,
            sel: Selection::new(shot.width as i32, shot.height as i32),
            mouse,
            layout: Layout::default(),
            hover_btn: None,
            pen: Pen {
                tool: None,
                lasso_mode: false,
                color: COLORS[0],
                size: SIZES[1],
                blur: opts.blur,
                style: opts.style,
                history: History::default(),
                live: None,
                typing: None,
                picker: opts.picker,
                color_format: opts.color_format,
            },
            armed: false,
            lasso: Vec::new(),
            stroke: Vec::new(),
            rail: Rect::default(),
            rail_at,
            rail_kept: opts.rail_at,
            on_stretch: opts.on_stretch,
            stretch: (0, None),
            rail_grab: None,
            rail_open: false,
            chain: HoverChain::default(),
            chain_win: None,
            app: String::new(),
            ask: None,
            repaint_all: false,
            shot,
            opts,
        };
        s.rail = s.monitor_at(rail_at.unwrap_or(mouse)).0;
        s.update_hover(mouse);
        s
    }

    fn bounds(&self) -> Rect {
        Rect {
            left: 0,
            top: 0,
            width: self.shot.width as i32,
            height: self.shot.height as i32,
        }
    }

    /// The monitor at `p`, or the nearest, with its scale.
    fn monitor_at(&self, p: Point) -> (Rect, f32) {
        let away = |m: &&(Rect, f32)| {
            let dx = (m.0.left - p.x).max(p.x - m.0.right() + 1).max(0);
            let dy = (m.0.top - p.y).max(p.y - m.0.bottom() + 1).max(0);
            dx + dy
        };
        let near = self.opts.monitors.iter().min_by_key(away);
        near.copied().unwrap_or((self.bounds(), self.opts.scale))
    }

    fn handle_size(&self, p: Point) -> i32 {
        (8.0 * self.monitor_at(p).1).round().max(1.0) as i32
    }

    fn free(&self) -> bool {
        self.pen.tool.is_none() && !self.pen.lasso_mode
    }

    /// Window or monitor under `p`; a window also asks for its elements.
    /// Returns whether the highlighted rect changed.
    fn update_hover(&mut self, p: Point) -> bool {
        if self.sel.confirmed() || self.sel.is_dragging() || !self.free() {
            return false;
        }
        let before = self.chain.current();
        let win = self.opts.targets.iter().position(|t| t.0.contains(p));
        let same = win == self.chain_win && !self.chain.is_empty();
        if same && self.chain.deepest().is_some_and(|r| r.contains(p)) {
            return false;
        }
        if same {
            self.chain.retain_containing(p);
        } else {
            let monitors = &self.opts.monitors;
            let base = win
                .map(|i| self.opts.targets[i].0)
                .or_else(|| monitors.iter().map(|m| m.0).find(|m| m.contains(p)))
                .or_else(|| monitors.is_empty().then(|| self.bounds()));
            self.chain = HoverChain::new(base.into_iter().collect());
            self.chain_win = win;
            self.app = win.map_or_else(String::new, |i| self.opts.targets[i].1.clone());
        }
        if let Some(window) = win {
            self.ask = Some(Ask {
                window,
                entered: !same,
            });
        }
        before != self.chain.current()
    }

    /// What the window should look up after the last event.
    pub fn take_ask(&mut self) -> Option<Ask> {
        self.ask.take()
    }

    pub fn set_app(&mut self, name: String) {
        self.app = name;
    }

    /// Elements of a target, outermost first. Any chain for the hovered target is useful even if
    /// the cursor moved on (replies take 60–180 ms in Discord): its levels under the cursor stay.
    pub fn add_levels(&mut self, window: usize, rects: Vec<Rect>) -> bool {
        let Some(base) = self.chain.outermost() else {
            return false;
        };
        if Some(window) != self.chain_win || self.sel.confirmed() || self.sel.is_dragging() {
            return false;
        }
        let mut all = vec![base];
        all.extend(
            rects
                .into_iter()
                .skip(1)
                .map(|r| clip(r, base))
                .take_while(|r| r.contains(self.mouse)),
        );
        let fresh = HoverChain::new(all);
        if fresh.deepest() == self.chain.deepest() || fresh.len() < self.chain.len() {
            return false;
        }
        self.chain = fresh;
        true
    }

    /// Wheel: one element level out or in.
    pub fn scroll(&mut self, outward: bool) -> bool {
        !self.sel.confirmed() && self.free() && self.chain.step(outward)
    }

    pub fn typing(&self) -> bool {
        self.pen.typing.is_some()
    }

    /// Area of everything on top of the frame.
    pub fn scene_bounds(&self) -> Option<Rect> {
        crate::layout(&self.scene()).bounds
    }

    /// Bounds of what the last `render` drew.
    pub fn drawn_bounds(&self) -> Option<Rect> {
        self.layout.bounds
    }

    /// The next frame changes more than the scene: shapes can be anywhere on screen.
    pub fn repaint_all(&mut self) -> bool {
        let wide = self.pen.live.is_some() || self.pen.typing.is_some();
        std::mem::take(&mut self.repaint_all) || wide || self.dim_level != Some(self.dim_wanted())
    }

    /// A finished selection has its own panel: the rail waits for a tool.
    fn rail_hidden(&self) -> bool {
        self.sel.confirmed() && !self.rail_open && self.pen.tool.is_none() && !self.pen.picker
    }

    fn loupe_on(&self) -> bool {
        let aiming = !self.sel.confirmed() || self.sel.is_dragging();
        let on = self.pen.picker || (self.opts.loupe && aiming);
        on && self.free() && self.hover_btn.is_none() && self.rail_grab.is_none()
    }

    fn scene(&self) -> Scene<'_> {
        let drawing = !self.stroke.is_empty();
        let lasso = if drawing { &self.stroke } else { &self.lasso };
        let hovering = self.sel.rect().is_none() && !self.sel.is_dragging() && self.free();
        let hover = hovering
            .then(|| self.chain.current())
            .flatten()
            .map(|rect| Hover {
                rect,
                target: match self.chain_win {
                    None => Target::Screen,
                    Some(_) if self.chain.current() == self.chain.outermost() => Target::Window,
                    Some(_) => Target::Element,
                },
                app: &self.app,
            });
        let anchor = self
            .sel
            .rect()
            .or(hover.as_ref().map(|h| h.rect))
            .map_or(self.mouse, |r| Point {
                x: r.right() - 1,
                y: r.bottom() - 1,
            });
        let (monitor, scale) = self.monitor_at(anchor);
        Scene {
            scale,
            monitor,
            cursor_monitor: self.monitor_at(self.mouse).0,
            selection: self.sel.rect().filter(|_| !drawing),
            confirmed: self.sel.confirmed() && !drawing,
            dragging: self.sel.is_dragging() || drawing,
            hover,
            hover_button: self.hover_btn,
            loupe: self.loupe_on().then_some(Loupe {
                at: self.mouse,
                frame: &self.annot,
                width: self.shot.width,
                height: self.shot.height,
                format: self.pen.color_format,
            }),
            tools: Some(Draw {
                monitor: self.rail,
                rail_at: self.rail_at,
                rail_hidden: self.rail_hidden(),
                lasso_mode: self.pen.lasso_mode,
                lasso: (!lasso.is_empty()).then_some(lasso.as_slice()),
                tool: self.pen.tool,
                color: self.pen.color,
                size: self.pen.size,
                blur: self.pen.blur,
                style: self.pen.style,
                can_undo: self.pen.history.can_undo(),
                can_redo: self.pen.history.can_redo(),
                live: self.pen.live.as_ref(),
                caret: self
                    .pen
                    .typing
                    .as_ref()
                    .map(|(at, text)| (*at, text.as_str())),
                picker: self.pen.picker,
                color_format: self.pen.color_format,
            }),
        }
    }

    fn dim_wanted(&self) -> u32 {
        if self.sel.rect().is_none() && self.pen.tool.is_some() {
            DIM_PAINT
        } else {
            DIM_SELECT
        }
    }

    /// One whole frame into `back`, BGRA of the shot's size.
    pub fn render(&mut self, back: &mut [u8]) {
        self.render_area(back, self.bounds());
    }

    /// `dirty` and the scene into `back`: the frame is laid again under every decoration, so
    /// glass never blurs its own last frame. Returns the area that changed.
    pub fn render_area(&mut self, back: &mut [u8], dirty: Rect) -> Rect {
        let level = self.dim_wanted();
        if self.dim_level != Some(level) {
            self.dim_level = Some(level);
            dim_into(&mut self.dim, &self.annot, level);
        }
        let (w, h) = (self.shot.width, self.shot.height);
        let all = self.bounds();
        let sc = self.scene();
        let lay = crate::layout(&sc);
        let area = lay
            .bounds
            .and_then(|b| crate::union(Some(dirty), b))
            .map_or(dirty, |u| clip(u, all));
        copy_area(back, &self.dim, w, area);
        let lasso = sc.tools.as_ref().and_then(|t| t.lasso);
        if let Some(r) = sc.selection.filter(|_| lasso.is_none()) {
            copy_area(back, &self.annot, w, clip(r, area));
        }
        if let Some(mut pm) = tiny_skia::PixmapMut::from_bytes(back, w, h) {
            if let (Some(pts), Some(from)) =
                (lasso, tiny_skia::PixmapRef::from_bytes(&self.annot, w, h))
            {
                crate::paint_region(&mut pm, from, pts);
            }
            crate::draw(&mut pm, &sc, &lay);
        }
        self.layout = lay;
        area
    }

    /// Re-renders `annot` = shot + committed shapes.
    fn bake(&mut self) {
        self.annot.copy_from_slice(&self.shot.bgra);
        let (w, h) = (self.shot.width, self.shot.height);
        if let Some(mut pm) = tiny_skia::PixmapMut::from_bytes(&mut self.annot, w, h) {
            for e in &self.pen.history.doc.elements {
                crate::draw_element(&mut pm, e);
            }
        }
        self.dim_level = None;
    }

    fn commit(&mut self, el: Element) {
        self.pen.history.push(el);
        self.bake();
    }

    fn commit_text(&mut self) {
        if let Some((at, text)) = self.pen.typing.take() {
            let el = Element {
                kind: Kind::Text(at, text.trim_end().to_string(), self.pen.style.font),
                color: self.pen.color,
                size: self.pen.size,
            };
            self.commit(el);
        }
    }

    fn set_mode(&mut self, lasso: bool) {
        self.pen.tool = None;
        self.pen.lasso_mode = lasso;
        self.repaint_all = true;
        if lasso || !self.lasso.is_empty() {
            self.lasso.clear();
            self.sel = Selection::new(self.shot.width as i32, self.shot.height as i32);
        }
    }

    fn close_lasso(&mut self) {
        let pts = std::mem::take(&mut self.stroke);
        let min = |f: fn(&Pt) -> f32| pts.iter().map(f).fold(f32::MAX, f32::min);
        let max = |f: fn(&Pt) -> f32| pts.iter().map(f).fold(f32::MIN, f32::max);
        let (x0, y0) = (min(|p| p.x) as i32, min(|p| p.y) as i32);
        let (x1, y1) = (max(|p| p.x) as i32 + 1, max(|p| p.y) as i32 + 1);
        if x1 - x0 < MIN_SELECTION_PX || y1 - y0 < MIN_SELECTION_PX {
            return;
        }
        self.lasso = pts;
        self.sel = Selection::new(self.shot.width as i32, self.shot.height as i32);
        self.sel.set_rect(Rect {
            left: x0,
            top: y0,
            width: x1 - x0,
            height: y1 - y0,
        });
    }

    fn pen_down(&mut self, tool: Tool, p: Point) {
        self.commit_text();
        let at = pen_point(p);
        let pen = &self.pen;
        let kind = match tool {
            Tool::Arrow => pen.style.new_arrow(at),
            Tool::Line => Kind::Line(at, at),
            Tool::Rect => Kind::Rect(at, at),
            Tool::Ellipse => Kind::Ellipse(at, at),
            Tool::Pen => Kind::Pen(vec![at]),
            Tool::Marker => Kind::Marker(vec![at]),
            Tool::Blur => Kind::Blur(vec![at], pen.blur),
            Tool::Step => Kind::Step(at, pen.history.next_step(), pen.style.step),
            Tool::Text => {
                self.pen.typing = Some((at, String::new()));
                return;
            }
        };
        let el = Element {
            kind,
            color: pen.color,
            size: pen.size,
        };
        if tool == Tool::Step {
            self.commit(el);
        } else {
            self.pen.live = Some(el);
        }
    }

    /// What the panels look like now, for the next overlay.
    pub fn prefs(&self) -> Prefs {
        Prefs {
            blur: self.pen.blur,
            style: self.pen.style,
            picker: self.pen.picker,
            color_format: self.pen.color_format,
            rail_at: self.rail_kept,
        }
    }

    fn control(&mut self, c: Control) -> Step {
        let c = if c == Control::Action(Button::Blur) {
            Control::Tool(Tool::Blur)
        } else {
            c
        };
        match c {
            Control::Action(b) => {
                return match b {
                    Button::Copy => Step::Done(Outcome::Copy),
                    Button::Save => Step::Done(Outcome::Save),
                    Button::Edit => Step::Done(Outcome::Edit),
                    Button::Text => Step::Done(Outcome::Text),
                    Button::Translate => Step::Done(Outcome::Translate),
                    Button::Pin => Step::Done(Outcome::Pin),
                    Button::Record => Step::Done(Outcome::Record),
                    Button::Scroll => Step::Done(Outcome::Scroll),
                    Button::Draw => {
                        self.rail_open = true;
                        Step::Stay
                    }
                    Button::Blur => Step::Stay,
                    Button::Cancel => Step::Cancel,
                };
            }
            Control::Select | Control::Lasso => self.set_mode(c == Control::Lasso),
            Control::Tool(t) => {
                self.rail_open = true;
                self.pen.tool = (self.pen.tool != Some(t)).then_some(t);
            }
            Control::Color(i) => self.pen.color = COLORS[i],
            Control::Size(i) => self.pen.size = SIZES[i],
            Control::Gauss(g) => self.pen.blur.gauss = g,
            Control::Strength(i) => self.pen.blur.strength = i,
            Control::Picker => self.pen.picker = !self.pen.picker,
            Control::ColorFormat(i) => self.pen.color_format = ColorFormat::ALL[i],
            Control::Head(i) => self.pen.style.arrow.head = Head::ALL[i],
            Control::Free(f) => self.pen.style.free = f,
            Control::ToStart(b) => self.pen.style.arrow.to_start = b,
            Control::StepStyle(i) => self.pen.style.step = StepStyle::ALL[i],
            Control::Font(forward) => {
                self.pen.style.font = snapling_core::fonts::neighbor(self.pen.style.font, forward);
            }
            Control::Undo | Control::Redo => {
                self.pen.typing = None;
                self.repaint_all = true;
                let changed = if c == Control::Undo {
                    self.pen.history.undo()
                } else {
                    self.pen.history.redo()
                };
                if changed {
                    self.bake();
                }
            }
        }
        Step::Stay
    }

    pub fn press(&mut self, p: Point, mods: Mods) -> Step {
        self.mouse = p;
        self.update_hover(p);
        if let Some(c) = crate::button_at(&self.layout, p) {
            return self.control(c);
        }
        if let Some(g) = self.layout.grip.filter(|g| g.contains(p)) {
            self.rail_grab = Some(Point {
                x: p.x - g.left,
                y: p.y - g.top,
            });
            return Step::Stay;
        }
        if self.pen.picker && self.pen.tool.is_none() {
            return Step::Done(Outcome::Color);
        }
        let hs = self.handle_size(p);
        let corner = self.lasso.is_empty() && matches!(self.sel.hit(p, hs), Hit::Corner(_));
        if let Some(tool) = self.pen.tool.filter(|_| !corner) {
            self.pen_down(tool, p);
        } else if self.pen.lasso_mode && self.pen.tool.is_none() {
            self.stroke = vec![pen_point(p)];
        } else {
            self.armed = true;
            self.sel.press(p, mods.shift, hs);
        }
        Step::Stay
    }

    /// Returns whether anything on screen changed.
    pub fn moved(&mut self, p: Point, mods: Mods) -> bool {
        self.mouse = p;
        let at = pen_point(p);
        if let Some(g) = self.rail_grab {
            self.rail = self.monitor_at(p).0;
            self.rail_at = Some(Point {
                x: p.x - g.x,
                y: p.y - g.y,
            });
            return true;
        }
        if let Some(el) = &mut self.pen.live {
            annotate::drag_to(&mut el.kind, at, mods.shift, 1.5);
            return true;
        }
        if let Some(last) = self.stroke.last() {
            if (last.x - at.x).hypot(last.y - at.y) >= 2.0 {
                self.stroke.push(at);
            }
            return true;
        }
        if self.sel.is_dragging() {
            self.sel.drag(p, mods.shift);
            self.stretch_sound();
            return true;
        }
        let hovered = self.update_hover(p);
        let button = crate::button_at(&self.layout, p);
        let changed = button != self.hover_btn;
        self.hover_btn = button;
        hovered || changed || self.loupe_on()
    }

    fn stretch_sound(&mut self) {
        let (Some(play), Some(r)) = (self.on_stretch, self.sel.rect()) else {
            return;
        };
        let size = r.width + r.height;
        let now = Instant::now();
        let quiet = self
            .stretch
            .1
            .is_none_or(|at| now.duration_since(at) >= STRETCH_GAP);
        if quiet && (size - self.stretch.0).abs() >= STRETCH_STEP {
            self.stretch = (size, Some(now));
            play((size / STRETCH_PER_LEVEL) as usize);
        }
    }

    pub fn release(&mut self) -> Step {
        self.stretch = (0, None);
        if self.rail_grab.take().is_some() {
            self.rail_kept = self.rail_at.map(|p| Point {
                x: p.x + self.shot.x,
                y: p.y + self.shot.y,
            });
            return Step::Stay;
        }
        let selected = if let Some(el) = self.pen.live.take() {
            self.commit(el);
            false
        } else if !self.stroke.is_empty() {
            self.close_lasso();
            self.sel.confirmed()
        } else if std::mem::take(&mut self.armed) {
            self.sel.release();
            // A click without a drag picks the highlighted window, element or screen.
            let m = self.mouse;
            if !self.sel.confirmed()
                && let Some(r) = self.chain.current().filter(|r| r.contains(m))
            {
                self.sel.set_rect(r);
            }
            self.sel.confirmed()
        } else {
            false
        };
        let instant = self.opts.instant.then_some(Outcome::Copy);
        match self.opts.auto.or(instant).filter(|_| selected) {
            Some(out) => Step::Done(out),
            None => Step::Stay,
        }
    }

    /// Right button: takes the selection back; with none left, closes.
    pub fn cancel_click(&mut self) -> Step {
        if self.sel.rect().is_none() && self.lasso.is_empty() && self.stroke.is_empty() {
            return Step::Cancel;
        }
        self.sel = Selection::new(self.shot.width as i32, self.shot.height as i32);
        self.lasso.clear();
        self.stroke.clear();
        self.armed = false;
        self.rail_open = false;
        self.update_hover(self.mouse);
        Step::Stay
    }

    pub fn double_click(&mut self, p: Point) -> Step {
        if self.pen.tool.is_none() && self.sel.hit(p, 0) == Hit::Inside {
            Step::Done(Outcome::Copy)
        } else {
            Step::Stay
        }
    }

    /// The window's scale, once it exists, when the options left it at 0.
    pub fn set_scale(&mut self, scale: f32) {
        if self.opts.scale == 0.0 {
            self.opts.scale = scale;
        }
    }

    pub fn mouse(&self) -> Point {
        self.mouse
    }

    pub fn set_pan(&mut self, on: bool) {
        self.sel.set_pan(on);
    }

    /// Shift pressed or let go during a drag.
    pub fn shift(&mut self, down: bool) {
        let m = self.mouse;
        self.sel.drag(m, down);
    }

    fn typing_key(&mut self, key: Key, mods: Mods) {
        match key {
            Key::Enter if mods.shift => {
                if let Some((_, t)) = &mut self.pen.typing {
                    t.push('\n');
                }
            }
            Key::Enter => self.commit_text(),
            Key::Escape => self.pen.typing = None,
            Key::Backspace => {
                if let Some((_, t)) = &mut self.pen.typing {
                    t.pop();
                }
            }
            Key::Char(c) => {
                if let Some((_, t)) = &mut self.pen.typing {
                    t.push(c);
                }
            }
            _ => {}
        }
    }

    pub fn key(&mut self, key: Key, mods: Mods) -> Step {
        if self.pen.typing.is_some() {
            self.typing_key(key, mods);
            return Step::Stay;
        }
        let confirmed = self.sel.confirmed();
        let step = if mods.shift { 10 } else { 1 };
        match key {
            Key::Escape if self.pen.tool.take().is_some() => Step::Stay,
            Key::Escape => Step::Cancel,
            Key::Enter if confirmed => Step::Done(Outcome::Copy),
            Key::Left | Key::Right | Key::Up | Key::Down if confirmed && self.lasso.is_empty() => {
                let (dx, dy) = match key {
                    Key::Left => (-step, 0),
                    Key::Right => (step, 0),
                    Key::Up => (0, -step),
                    _ => (0, step),
                };
                self.sel.nudge(dx, dy);
                Step::Stay
            }
            Key::Char(c) if mods.ctrl => match c.to_ascii_lowercase() {
                'z' if !mods.shift => self.control(Control::Undo),
                'z' | 'y' => self.control(Control::Redo),
                'v' | 'l' | 'i' => Step::Stay,
                // Ctrl+S saves and Ctrl+C copies, as the plain letters do.
                c => self.letter(c, confirmed),
            },
            Key::Char(c) => self.letter(c.to_ascii_lowercase(), confirmed),
            _ => Step::Stay,
        }
    }

    fn letter(&mut self, c: char, confirmed: bool) -> Step {
        if let Some(i) = c.to_digit(10).filter(|d| (1..=9).contains(d)) {
            return self.control(Control::Tool(TOOLS[i as usize - 1]));
        }
        let out = match c {
            'v' => return self.control(Control::Select),
            'l' => return self.control(Control::Lasso),
            'i' => return Step::Done(Outcome::Color),
            'c' => Outcome::Copy,
            's' => Outcome::Save,
            'e' => Outcome::Edit,
            'o' => Outcome::Text,
            'r' => Outcome::Translate,
            'p' => Outcome::Pin,
            'g' => Outcome::Record,
            'd' => Outcome::Scroll,
            _ => return Step::Stay,
        };
        if confirmed {
            Step::Done(out)
        } else {
            Step::Stay
        }
    }

    /// Which pointer fits the spot.
    pub fn cursor(&self) -> Pointer {
        let p = self.mouse;
        if crate::button_at(&self.layout, p).is_some() {
            return Pointer::Hand;
        }
        if self.rail_grab.is_some() || self.layout.grip.is_some_and(|g| g.contains(p)) {
            return Pointer::Move;
        }
        let hit = if self.lasso.is_empty() {
            self.sel.hit(p, self.handle_size(p))
        } else {
            Hit::Outside
        };
        match (self.pen.tool, hit) {
            (_, Hit::Corner(Corner::Nw | Corner::Se)) => Pointer::ResizeNwSe,
            (_, Hit::Corner(_)) => Pointer::ResizeNeSw,
            (Some(Tool::Text), _) => Pointer::Text,
            (None, Hit::Inside) if !self.pen.lasso_mode => Pointer::Move,
            _ => Pointer::Cross,
        }
    }

    /// The picture an outcome works on.
    pub fn result(&mut self, out: Outcome) -> Option<Shot> {
        if let Some(el) = self.pen.live.take() {
            self.commit(el);
        }
        self.commit_text();
        let (w, h) = (self.shot.width as i32, self.shot.height as i32);
        if out == Outcome::Color {
            let (x, y) = (self.mouse.x, self.mouse.y);
            if x < 0 || y < 0 || x >= w || y >= h {
                return None;
            }
            let at = (y as usize * w as usize + x as usize) * 4;
            return Some(Shot {
                x: x + self.shot.x,
                y: y + self.shot.y,
                width: 1,
                height: 1,
                bgra: self.annot[at..at + 4].to_vec(),
            });
        }
        let r = self.sel.rect().filter(|_| self.sel.confirmed())?;
        let (x0, y0) = (r.left.max(0), r.top.max(0));
        let (x1, y1) = (r.right().min(w), r.bottom().min(h));
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let (cw, ch) = ((x1 - x0) as u32, (y1 - y0) as u32);
        let row = w as usize * 4;
        let mut bgra = Vec::with_capacity((cw * ch * 4) as usize);
        for y in y0..y1 {
            let at = y as usize * row + x0 as usize * 4;
            bgra.extend_from_slice(&self.annot[at..at + cw as usize * 4]);
        }
        if !self.lasso.is_empty() {
            let (dx, dy) = (x0 as f32, y0 as f32);
            let pts: Vec<Pt> = self
                .lasso
                .iter()
                .map(|p| Pt {
                    x: p.x - dx,
                    y: p.y - dy,
                })
                .collect();
            annotate::cut_polygon(&mut bgra, cw, ch, &pts);
        }
        Some(Shot {
            x: x0 + self.shot.x,
            y: y0 + self.shot.y,
            width: cw,
            height: ch,
            bgra,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pointer {
    Cross,
    Hand,
    Move,
    Text,
    ResizeNwSe,
    ResizeNeSw,
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Mods = Mods {
        shift: false,
        ctrl: false,
    };

    fn session(opts: Options) -> Session {
        let shot = Shot {
            x: 100,
            y: 50,
            width: 400,
            height: 300,
            bgra: [10, 20, 30, 255].repeat(400 * 300),
        };
        Session::new(shot, Options { scale: 1.0, ..opts })
    }

    fn pt(x: i32, y: i32) -> Point {
        Point { x, y }
    }

    fn drag(s: &mut Session, a: Point, b: Point) -> Step {
        s.moved(a, NONE);
        s.press(a, NONE);
        s.moved(b, NONE);
        s.release()
    }

    #[test]
    fn dim_keeps_black_near_black_and_alpha_opaque() {
        let src = [255u8, 255, 255, 0, 0, 0, 0, 0];
        let mut dst = [0u8; 8];
        dim_into(&mut dst, &src, DIM_SELECT);
        assert_eq!(dst[3], 255);
        assert!(dst[0] > 140 && dst[0] < 160, "{dst:?}");
        assert!(dst[4] < 10, "{dst:?}");
    }

    /// `cargo test --release -p snapling-scene bench_dim -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_dim() {
        let len = 4480 * 1527 * 4;
        let src = vec![90u8; len];
        let mut dst = vec![0u8; len];
        for _ in 0..3 {
            let t0 = std::time::Instant::now();
            dim_into(std::hint::black_box(&mut dst), &src, DIM_SELECT);
            std::hint::black_box(&dst);
            println!("dim 4480x1527: {} us", t0.elapsed().as_micros());
        }
    }

    #[test]
    fn the_rail_waits_for_a_tool_next_to_a_finished_selection() {
        let mut s = session(Options::default());
        assert!(!s.rail_hidden());
        drag(&mut s, pt(20, 30), pt(120, 80));
        assert!(s.rail_hidden());
        assert_eq!(s.control(Control::Action(Button::Draw)), Step::Stay);
        assert!(!s.rail_hidden());
        s.cancel_click();
        drag(&mut s, pt(20, 30), pt(120, 80));
        assert!(s.rail_hidden());
        s.key(Key::Char('3'), NONE);
        assert!(!s.rail_hidden());
    }

    #[test]
    fn the_rail_moves_by_its_grip_and_keeps_the_end_of_the_drag() {
        let mut s = session(Options::default());
        let mut back = vec![0u8; 400 * 300 * 4];
        s.render(&mut back);
        let g = s.layout.grip.unwrap();
        let from = pt(g.left + 4, g.top + 4);
        s.press(from, NONE);
        assert_eq!(s.prefs().rail_at, None, "only the end of a drag is kept");
        assert!(s.moved(pt(from.x + 90, from.y + 5), NONE));
        assert_eq!(s.release(), Step::Stay);
        let kept = s.prefs().rail_at.unwrap();
        assert_eq!((kept.x, kept.y), (g.left + 190, g.top + 55), "desktop");
        assert_eq!(s.sel.rect(), None, "the drag selected nothing");
        s.render(&mut back);
        assert_eq!(s.layout.grip.unwrap().left, g.left + 90);
    }

    #[test]
    fn a_partial_frame_touches_only_its_area_and_the_scene() {
        let mut s = session(Options {
            loupe: false,
            ..Options::default()
        });
        let mut back = vec![7u8; 400 * 300 * 4];
        let dirty = Rect {
            left: 300,
            top: 10,
            width: 20,
            height: 20,
        };
        let area = s.render_area(&mut back, dirty);
        assert!(area.left <= 300 && area.right() >= 320);
        assert_ne!(back[(20 * 400 + 310) * 4], 7, "inside");
    }

    #[test]
    fn the_hover_walks_elements_and_a_click_takes_the_level_shown() {
        let win = Rect {
            left: 50,
            top: 40,
            width: 100,
            height: 80,
        };
        let inner = Rect {
            left: 60,
            top: 50,
            width: 30,
            height: 30,
        };
        let mut s = session(Options {
            targets: vec![(win, String::new())],
            ..Options::default()
        });
        s.moved(pt(70, 60), NONE);
        let ask = s.take_ask().unwrap();
        assert_eq!((ask.window, ask.entered), (0, true));
        assert!(s.add_levels(0, vec![win, inner]));
        assert_eq!(s.chain.current(), Some(inner));
        s.press(pt(70, 60), NONE);
        s.release();
        assert_eq!(s.sel.rect(), Some(inner));
    }

    #[test]
    fn each_monitor_has_its_own_scale() {
        let a = Rect {
            left: 0,
            top: 0,
            width: 200,
            height: 300,
        };
        let b = Rect { left: 200, ..a };
        let s = session(Options {
            monitors: vec![(a, 1.0), (b, 2.0)],
            ..Options::default()
        });
        assert_eq!(s.handle_size(pt(10, 10)), 8);
        assert_eq!(s.handle_size(pt(300, 10)), 16);
        assert_eq!(s.monitor_at(pt(900, 10)).0, b, "the nearest one");
    }

    #[test]
    fn a_drag_selects_and_enter_copies_that_area() {
        let mut s = session(Options::default());
        assert_eq!(drag(&mut s, pt(20, 30), pt(120, 80)), Step::Stay);
        assert_eq!(s.key(Key::Enter, NONE), Step::Done(Outcome::Copy));
        let got = s.result(Outcome::Copy).unwrap();
        assert_eq!((got.width, got.height), (100, 50));
        assert_eq!((got.x, got.y), (120, 80), "desktop coordinates");
        assert_eq!(got.bgra.len(), 100 * 50 * 4);
    }

    #[test]
    fn a_click_takes_the_window_under_it_or_the_screen() {
        let win = Rect {
            left: 50,
            top: 40,
            width: 100,
            height: 80,
        };
        let opts = Options {
            targets: vec![(win, "Firefox".into())],
            ..Options::default()
        };
        let mut s = session(opts.clone());
        drag(&mut s, pt(60, 60), pt(60, 60));
        assert_eq!(s.sel.rect(), Some(win));
        let mut s = session(opts);
        drag(&mut s, pt(300, 250), pt(300, 250));
        assert_eq!(s.sel.rect(), Some(s.bounds()));
    }

    #[test]
    fn auto_and_instant_finish_on_release() {
        let mut s = session(Options {
            auto: Some(Outcome::Text),
            ..Options::default()
        });
        let end = drag(&mut s, pt(20, 30), pt(120, 80));
        assert_eq!(end, Step::Done(Outcome::Text));
        let mut s = session(Options {
            instant: true,
            ..Options::default()
        });
        let end = drag(&mut s, pt(20, 30), pt(120, 80));
        assert_eq!(end, Step::Done(Outcome::Copy));
    }

    #[test]
    fn drawn_shapes_land_in_the_result_and_undo_removes_them() {
        let mut s = session(Options::default());
        s.key(Key::Char('3'), NONE);
        assert_eq!(s.pen.tool, Some(Tool::Rect));
        drag(&mut s, pt(40, 40), pt(90, 90));
        assert_eq!(s.pen.history.doc.elements.len(), 1);
        assert_ne!(s.annot, s.shot.bgra);
        let ctrl = Mods {
            ctrl: true,
            shift: false,
        };
        s.key(Key::Char('z'), ctrl);
        assert_eq!(s.annot, s.shot.bgra);
    }

    #[test]
    fn panels_change_the_style_of_new_shapes() {
        let mut s = session(Options::default());
        s.control(Control::Head(2));
        s.control(Control::ToStart(true));
        s.control(Control::Free(true));
        s.control(Control::StepStyle(4));
        let style = s.prefs().style;
        assert_eq!(style.arrow.head, Head::Double);
        assert!(style.arrow.to_start && style.free);
        assert_eq!(style.step, StepStyle::Pin);
        assert!(matches!(
            style.new_arrow(Pt::default()),
            Kind::Curve(_, a) if a == style.arrow
        ));
    }

    #[test]
    fn stretching_plays_grains_with_gaps_and_stays_quiet_when_moved() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static PLAYED: AtomicUsize = AtomicUsize::new(0);
        let mut s = session(Options {
            on_stretch: Some(|_| {
                PLAYED.fetch_add(1, Ordering::SeqCst);
            }),
            ..Options::default()
        });
        s.sel.set_rect(Rect {
            left: 0,
            top: 0,
            width: 100,
            height: 100,
        });
        s.stretch_sound();
        assert_eq!(PLAYED.load(Ordering::SeqCst), 1);
        s.stretch_sound();
        assert_eq!(PLAYED.load(Ordering::SeqCst), 1, "same size, no grain");
        s.sel.set_rect(Rect {
            left: 0,
            top: 0,
            width: 500,
            height: 100,
        });
        s.stretch_sound();
        assert_eq!(PLAYED.load(Ordering::SeqCst), 1, "inside the gap");
    }

    #[test]
    fn text_keys_go_to_the_caret_not_to_shortcuts() {
        let mut s = session(Options::default());
        s.key(Key::Char('7'), NONE);
        s.press(pt(50, 50), NONE);
        s.release();
        for c in "sv".chars() {
            assert_eq!(s.key(Key::Char(c), NONE), Step::Stay);
        }
        s.key(Key::Backspace, NONE);
        assert_eq!(s.pen.typing.as_ref().unwrap().1, "s");
        s.key(Key::Enter, NONE);
        assert_eq!(s.pen.history.doc.elements.len(), 1);
    }

    #[test]
    fn i_picks_the_pixel_under_the_cursor() {
        let mut s = session(Options::default());
        s.moved(pt(5, 6), NONE);
        assert_eq!(s.key(Key::Char('I'), NONE), Step::Done(Outcome::Color));
        let px = s.result(Outcome::Color).unwrap();
        assert_eq!(
            (px.x, px.y, px.bgra.as_slice()),
            (105, 56, &[10, 20, 30, 255][..])
        );
    }

    #[test]
    fn the_picker_takes_the_color_on_a_click_and_remembers_the_format() {
        let mut s = session(Options {
            picker: true,
            color_format: ColorFormat::Rgb,
            ..Options::default()
        });
        assert!(s.loupe_on(), "the loupe comes with the picker");
        s.moved(pt(5, 6), NONE);
        assert_eq!(s.press(pt(5, 6), NONE), Step::Done(Outcome::Color));
        assert_eq!(s.prefs().color_format, ColorFormat::Rgb);
        s.control(Control::ColorFormat(4));
        assert_eq!(s.prefs().color_format, ColorFormat::Cmyk);
        s.control(Control::Picker);
        assert!(!s.prefs().picker);
    }

    #[test]
    fn the_blur_button_picks_the_blur_brush() {
        let mut s = session(Options::default());
        assert_eq!(s.control(Control::Action(Button::Blur)), Step::Stay);
        assert_eq!(s.pen.tool, Some(Tool::Blur));
    }

    #[test]
    fn the_right_button_clears_first_and_closes_second() {
        let mut s = session(Options::default());
        drag(&mut s, pt(20, 30), pt(120, 80));
        assert_eq!(s.cancel_click(), Step::Stay);
        assert_eq!(s.sel.rect(), None);
        assert_eq!(s.cancel_click(), Step::Cancel);
    }

    #[test]
    fn a_frame_keeps_the_selection_bright_and_the_rest_dim() {
        let mut s = session(Options::default());
        drag(&mut s, pt(20, 30), pt(220, 180));
        let mut back = vec![0u8; 400 * 300 * 4];
        s.render(&mut back);
        let at = |x: usize, y: usize| back[(y * 400 + x) * 4];
        assert_eq!(at(100, 100), 10, "inside");
        assert_ne!(at(380, 280), 10, "outside");
    }

    #[test]
    fn a_lasso_cuts_the_result() {
        let mut s = session(Options::default());
        s.key(Key::Char('l'), NONE);
        s.moved(pt(50, 50), NONE);
        s.press(pt(50, 50), NONE);
        for (x, y) in [(150, 50), (150, 150), (50, 150)] {
            s.moved(pt(x, y), NONE);
        }
        s.release();
        let got = s.result(Outcome::Copy).unwrap();
        assert!(got.width >= 100);
        assert_eq!(got.bgra[3], 255, "the square keeps its corner");
    }
}
