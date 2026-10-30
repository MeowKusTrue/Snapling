//! Everything the selection overlay does, with no window: input goes in, a frame and an outcome
//! come out. Same behavior as `snapling-win/overlay.rs`, which keeps its own Win32 copy.

use snapling_core::annotate::{self, BlurStyle, Element, Head, History, Kind, Pt};
use snapling_core::selection::{Corner, Hit, MIN_SELECTION_PX, Point, Rect, Selection};
use snapling_scene::{
    Button, COLORS, Control, Draw, Hover, Layout, Loupe, SIZES, Scene, TOOLS, Target, Tool,
};

const DIM: [u32; 3] = [16, 8, 4];
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
    /// DPI scale of the monitor; mockup sizes are at 1.0.
    pub scale: f32,
    /// Selecting ends the overlay with this outcome (OCR hotkeys).
    pub auto: Option<Outcome>,
    /// Selecting copies at once, skipping the action toolbar.
    pub instant: bool,
    pub loupe: bool,
    pub blur: BlurStyle,
    /// Windows under the overlay, topmost first, with their app names.
    pub targets: Vec<(Rect, String)>,
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
    history: History,
    live: Option<Element>,
    typing: Option<(Pt, String)>,
}

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
    opts: Options,
}

fn pen_point(p: Point) -> Pt {
    Pt {
        x: p.x as f32,
        y: p.y as f32,
    }
}

fn dim_into(dst: &mut [u8], src: &[u8], a: u32) {
    let keep = 256 - a;
    for (d, p) in dst
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(src.as_chunks::<4>().0)
    {
        let mix = |c: usize| ((u32::from(p[c]) * keep + DIM[c] * a) >> 8) as u8;
        *d = [mix(0), mix(1), mix(2), 255];
    }
}

impl Session {
    pub fn new(shot: Shot, opts: Options) -> Self {
        let len = shot.bgra.len();
        Self {
            annot: shot.bgra.clone(),
            dim: vec![0; len],
            dim_level: None,
            sel: Selection::new(shot.width as i32, shot.height as i32),
            mouse: Point::default(),
            layout: Layout::default(),
            hover_btn: None,
            pen: Pen {
                tool: None,
                lasso_mode: false,
                color: COLORS[0],
                size: SIZES[1],
                blur: opts.blur,
                history: History::default(),
                live: None,
                typing: None,
            },
            armed: false,
            lasso: Vec::new(),
            stroke: Vec::new(),
            shot,
            opts,
        }
    }

    fn bounds(&self) -> Rect {
        Rect {
            left: 0,
            top: 0,
            width: self.shot.width as i32,
            height: self.shot.height as i32,
        }
    }

    fn handle_size(&self) -> i32 {
        (8.0 * self.opts.scale).round() as i32
    }

    fn free(&self) -> bool {
        self.pen.tool.is_none() && !self.pen.lasso_mode
    }

    fn hover_rect(&self) -> Option<(Rect, Target, &str)> {
        if self.sel.rect().is_some() || self.sel.is_dragging() || !self.free() {
            return None;
        }
        let win = self.opts.targets.iter().find(|t| t.0.contains(self.mouse));
        Some(match win {
            Some((r, app)) => (*r, Target::Window, app.as_str()),
            None => (self.bounds(), Target::Screen, ""),
        })
    }

    fn loupe_on(&self) -> bool {
        let aiming = !self.sel.confirmed() || self.sel.is_dragging();
        self.opts.loupe && aiming && self.free() && self.hover_btn.is_none()
    }

    fn scene(&self) -> Scene<'_> {
        let drawing = !self.stroke.is_empty();
        let bounds = self.bounds();
        let lasso = if drawing { &self.stroke } else { &self.lasso };
        let hover = self
            .hover_rect()
            .map(|(rect, target, app)| Hover { rect, target, app });
        Scene {
            scale: self.opts.scale,
            monitor: bounds,
            cursor_monitor: bounds,
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
            }),
            tools: Some(Draw {
                monitor: bounds,
                lasso_mode: self.pen.lasso_mode,
                lasso: (!lasso.is_empty()).then_some(lasso.as_slice()),
                tool: self.pen.tool,
                color: self.pen.color,
                size: self.pen.size,
                blur: self.pen.blur,
                can_undo: !self.pen.history.doc.elements.is_empty(),
                can_redo: false,
                live: self.pen.live.as_ref(),
                caret: self
                    .pen
                    .typing
                    .as_ref()
                    .map(|(at, text)| (*at, text.as_str())),
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
        let level = self.dim_wanted();
        if self.dim_level != Some(level) {
            self.dim_level = Some(level);
            dim_into(&mut self.dim, &self.annot, level);
        }
        back.copy_from_slice(&self.dim);
        let (w, h) = (self.shot.width, self.shot.height);
        let lasso_on = !self.lasso.is_empty() || !self.stroke.is_empty();
        if let Some(r) = self.sel.rect().filter(|_| !lasso_on) {
            let row = w as usize * 4;
            for y in r.top.max(0)..r.bottom().min(h as i32) {
                let at = y as usize * row;
                let (l, e) = (
                    r.left.max(0) as usize * 4,
                    r.right().min(w as i32) as usize * 4,
                );
                back[at + l..at + e].copy_from_slice(&self.annot[at + l..at + e]);
            }
        }
        let Some(mut pm) = tiny_skia::PixmapMut::from_bytes(back, w, h) else {
            return;
        };
        let lay = {
            let sc = self.scene();
            if let (Some(pts), Some(from)) = (
                sc.tools.as_ref().and_then(|t| t.lasso),
                tiny_skia::PixmapRef::from_bytes(&self.annot, w, h),
            ) {
                snapling_scene::paint_region(&mut pm, from, pts);
            }
            let lay = snapling_scene::layout(&sc);
            snapling_scene::draw(&mut pm, &sc, &lay);
            lay
        };
        self.layout = lay;
    }

    /// Re-renders `annot` = shot + committed shapes.
    fn bake(&mut self) {
        self.annot.copy_from_slice(&self.shot.bgra);
        let (w, h) = (self.shot.width, self.shot.height);
        if let Some(mut pm) = tiny_skia::PixmapMut::from_bytes(&mut self.annot, w, h) {
            for e in &self.pen.history.doc.elements {
                snapling_scene::draw_element(&mut pm, e);
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
                kind: Kind::Text(at, text.trim_end().to_string()),
                color: self.pen.color,
                size: self.pen.size,
            };
            self.commit(el);
        }
    }

    fn set_mode(&mut self, lasso: bool) {
        self.pen.tool = None;
        self.pen.lasso_mode = lasso;
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
            Tool::Arrow => Kind::Arrow(at, at, Head::Sharp),
            Tool::Line => Kind::Line(at, at),
            Tool::Rect => Kind::Rect(at, at),
            Tool::Ellipse => Kind::Ellipse(at, at),
            Tool::Pen => Kind::Pen(vec![at]),
            Tool::Marker => Kind::Marker(vec![at]),
            Tool::Blur => Kind::Blur(vec![at], pen.blur),
            Tool::Step => Kind::Step(at, pen.history.next_step()),
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

    fn control(&mut self, c: Control) -> Step {
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
                    Button::Cancel => Step::Cancel,
                };
            }
            Control::Select | Control::Lasso => self.set_mode(c == Control::Lasso),
            Control::Tool(t) => self.pen.tool = (self.pen.tool != Some(t)).then_some(t),
            Control::Color(i) => self.pen.color = COLORS[i],
            Control::Size(i) => self.pen.size = SIZES[i],
            Control::Gauss(g) => self.pen.blur.gauss = g,
            Control::Strength(i) => self.pen.blur.strength = i,
            Control::Undo | Control::Redo => {
                self.pen.typing = None;
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
        if let Some(c) = snapling_scene::button_at(&self.layout, p) {
            return self.control(c);
        }
        let hs = self.handle_size();
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

    pub fn moved(&mut self, p: Point, mods: Mods) {
        self.mouse = p;
        let at = pen_point(p);
        if let Some(el) = &mut self.pen.live {
            annotate::drag_to(&mut el.kind, at, mods.shift, 1.5);
        } else if let Some(last) = self.stroke.last() {
            if (last.x - at.x).hypot(last.y - at.y) >= 2.0 {
                self.stroke.push(at);
            }
        } else if self.sel.is_dragging() {
            self.sel.drag(p, mods.shift);
        } else {
            self.hover_btn = snapling_scene::button_at(&self.layout, p);
        }
    }

    pub fn release(&mut self) -> Step {
        let selected = if let Some(el) = self.pen.live.take() {
            self.commit(el);
            false
        } else if !self.stroke.is_empty() {
            self.close_lasso();
            self.sel.confirmed()
        } else if std::mem::take(&mut self.armed) {
            self.sel.release();
            // A click without a drag picks the highlighted window or the screen.
            if !self.sel.confirmed()
                && let Some((r, ..)) = self.hover_rect()
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

    pub fn double_click(&mut self, p: Point) -> Step {
        if self.pen.tool.is_none() && self.sel.hit(p, 0) == Hit::Inside {
            Step::Done(Outcome::Copy)
        } else {
            Step::Stay
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
                's' if confirmed => Step::Done(Outcome::Save),
                _ => Step::Stay,
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
        if self.hover_btn.is_some() {
            return Pointer::Hand;
        }
        let hit = if self.lasso.is_empty() {
            self.sel.hit(self.mouse, self.handle_size())
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
