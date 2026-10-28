//! Region selection geometry and interaction state.
//! Ported from EiSen `app/src/lib/overlay/geometry.ts` and the pointer logic of
//! `app/src/routes/overlay/+page.svelte` (MIT, © 2026 Tran Trong Thuc).
//! Coordinates are physical pixels relative to the captured image.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    pub fn right(&self) -> i32 {
        self.left + self.width
    }
    pub fn bottom(&self) -> i32 {
        self.top + self.height
    }
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.left && p.x < self.right() && p.y >= self.top && p.y < self.bottom()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    Nw,
    Ne,
    Sw,
    Se,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Corner(Corner),
    Inside,
    Outside,
}

pub const MIN_SELECTION_PX: i32 = 10;

/// Normalized rectangle between two points; `square` locks 1:1 on the larger side.
pub fn selection_rect(p1: Point, p2: Point, square: bool) -> Rect {
    let mut left = p1.x.min(p2.x);
    let mut top = p1.y.min(p2.y);
    let mut width = (p1.x - p2.x).abs();
    let mut height = (p1.y - p2.y).abs();
    if square {
        let side = width.max(height);
        width = side;
        height = side;
        if p2.x < p1.x {
            left = p1.x - side;
        }
        if p2.y < p1.y {
            top = p1.y - side;
        }
    }
    Rect {
        left,
        top,
        width,
        height,
    }
}

/// Offset by (dx, dy), clamped inside `bounds` when given.
pub fn shift_rect(r: Rect, dx: i32, dy: i32, bounds: Option<Rect>) -> Rect {
    let mut left = r.left + dx;
    let mut top = r.top + dy;
    if let Some(b) = bounds {
        left = left.min(b.right() - r.width).max(b.left);
        top = top.min(b.bottom() - r.height).max(b.top);
    }
    Rect { left, top, ..r }
}

/// The corner that stays fixed while `corner` follows the pointer.
pub fn opposite_corner(r: Rect, corner: Corner) -> Point {
    match corner {
        Corner::Nw => Point {
            x: r.right(),
            y: r.bottom(),
        },
        Corner::Ne => Point {
            x: r.left,
            y: r.bottom(),
        },
        Corner::Sw => Point {
            x: r.right(),
            y: r.top,
        },
        Corner::Se => Point {
            x: r.left,
            y: r.top,
        },
    }
}

/// Toolbar top-left: below the selection, else above, else inside it.
/// `bounds` is the monitor the selection ends on, so the bar never lands in a dead zone.
pub fn toolbar_position(r: Rect, bounds: Rect, tw: i32, th: i32, gap: i32) -> (Point, bool) {
    let tw = tw + gap;
    let x = (r.right() - tw + gap)
        .max(bounds.left + gap)
        .min(bounds.right() - tw);
    let below = r.bottom() + gap;
    if below + th + gap <= bounds.bottom() {
        return (Point { x, y: below }, false);
    }
    let above = r.top - th - gap;
    if above >= bounds.top + gap * 2 {
        return (Point { x, y: above }, false);
    }
    let ix = (r.right() - tw).min(bounds.right() - tw).max(r.left + gap);
    let iy = (r.bottom() - th - gap)
        .min(bounds.bottom() - th - gap)
        .max(r.top + gap);
    (Point { x: ix, y: iy }, true)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Idle,
    Drawing,
    Moving,
    Resizing,
}

/// Pointer-driven selection: draw, move, resize by corners, pan with Space.
#[derive(Debug, Clone)]
pub struct Selection {
    bounds: Rect,
    start: Point,
    current: Point,
    last: Point,
    mode: Mode,
    confirmed: bool,
    square: bool,
    pan: bool,
}

impl Selection {
    pub fn new(width: i32, height: i32) -> Self {
        Self {
            bounds: Rect {
                left: 0,
                top: 0,
                width,
                height,
            },
            start: Point::default(),
            current: Point::default(),
            last: Point::default(),
            mode: Mode::Idle,
            confirmed: false,
            square: false,
            pan: false,
        }
    }

    fn clamp(&self, p: Point) -> Point {
        Point {
            x: p.x.clamp(0, self.bounds.width),
            y: p.y.clamp(0, self.bounds.height),
        }
    }

    /// Current rectangle; `None` before anything is drawn.
    pub fn rect(&self) -> Option<Rect> {
        let r = selection_rect(
            self.start,
            self.current,
            self.square && self.mode != Mode::Moving,
        );
        (r.width > 0 && r.height > 0).then_some(r)
    }

    pub fn confirmed(&self) -> bool {
        self.confirmed
    }

    pub fn is_dragging(&self) -> bool {
        self.mode != Mode::Idle
    }

    pub fn hit(&self, p: Point, handle: i32) -> Hit {
        let Some(r) = self.rect().filter(|_| self.confirmed) else {
            return Hit::Outside;
        };
        for (c, cx, cy) in [
            (Corner::Nw, r.left, r.top),
            (Corner::Ne, r.right(), r.top),
            (Corner::Sw, r.left, r.bottom()),
            (Corner::Se, r.right(), r.bottom()),
        ] {
            if (p.x - cx).abs() <= handle && (p.y - cy).abs() <= handle {
                return Hit::Corner(c);
            }
        }
        if r.contains(p) {
            Hit::Inside
        } else {
            Hit::Outside
        }
    }

    pub fn press(&mut self, p: Point, square: bool, handle: i32) {
        let p = self.clamp(p);
        self.square = square;
        self.last = p;
        match self.hit(p, handle) {
            Hit::Corner(c) => {
                let r = self.rect().unwrap_or_default();
                self.start = opposite_corner(r, c);
                self.current = p;
                self.mode = Mode::Resizing;
            }
            Hit::Inside => self.mode = Mode::Moving,
            Hit::Outside => {
                self.start = p;
                self.current = p;
                self.confirmed = false;
                self.mode = Mode::Drawing;
            }
        }
    }

    pub fn drag(&mut self, p: Point, square: bool) {
        if self.mode == Mode::Idle {
            return;
        }
        let p = self.clamp(p);
        self.square = square;
        if self.mode == Mode::Moving || self.pan {
            self.nudge(p.x - self.last.x, p.y - self.last.y);
        } else {
            self.current = p;
        }
        self.last = p;
    }

    pub fn release(&mut self) {
        if self.mode == Mode::Idle {
            return;
        }
        let was_drawing = self.mode == Mode::Drawing;
        if let Some(r) = self.rect() {
            let r = shift_rect(r, 0, 0, Some(self.bounds));
            self.start = Point {
                x: r.left,
                y: r.top,
            };
            self.current = Point {
                x: r.right(),
                y: r.bottom(),
            };
        }
        self.mode = Mode::Idle;
        self.pan = false;
        self.square = false;
        if was_drawing {
            self.confirmed = self
                .rect()
                .is_some_and(|r| r.width >= MIN_SELECTION_PX && r.height >= MIN_SELECTION_PX);
            if !self.confirmed {
                self.start = Point::default();
                self.current = Point::default();
            }
        }
    }

    /// Space held while drawing pans the whole rectangle.
    pub fn set_pan(&mut self, on: bool) {
        self.pan = on && self.mode == Mode::Drawing;
    }

    pub fn nudge(&mut self, dx: i32, dy: i32) {
        let Some(r) = self.rect() else { return };
        let r = shift_rect(r, dx, dy, Some(self.bounds));
        let (sx, sy) = (
            r.left - self.start.x.min(self.current.x),
            r.top - self.start.y.min(self.current.y),
        );
        self.start.x += sx;
        self.start.y += sy;
        self.current.x += sx;
        self.current.y += sy;
    }

    /// Selects `r` (e.g. a detected window), clipped to bounds.
    pub fn set_rect(&mut self, r: Rect) {
        let b = self.bounds;
        let l = r.left.max(b.left);
        let t = r.top.max(b.top);
        self.start = Point { x: l, y: t };
        self.current = Point {
            x: r.right().min(b.right()).max(l),
            y: r.bottom().min(b.bottom()).max(t),
        };
        self.mode = Mode::Idle;
        self.confirmed = self.rect().is_some();
    }
}

/// Nested targets under the cursor, outermost (window) first.
/// Starts at the innermost; the wheel walks outward/inward.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HoverChain {
    rects: Vec<Rect>,
    level: usize,
}

impl HoverChain {
    pub fn new(mut rects: Vec<Rect>) -> Self {
        rects.retain(|r| r.width > 0 && r.height > 0);
        rects.dedup();
        let level = rects.len().saturating_sub(1);
        Self { rects, level }
    }

    pub fn current(&self) -> Option<Rect> {
        self.rects.get(self.level).copied()
    }

    pub fn len(&self) -> usize {
        self.rects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }

    pub fn outermost(&self) -> Option<Rect> {
        self.rects.first().copied()
    }

    pub fn deepest(&self) -> Option<Rect> {
        self.rects.last().copied()
    }

    /// Keeps the outer levels still under `p` (shown until fresh inner levels arrive).
    pub fn retain_containing(&mut self, p: Point) {
        let keep = self.rects.iter().take_while(|r| r.contains(p)).count();
        self.rects.truncate(keep);
        self.level = keep.saturating_sub(1);
    }

    /// Returns true when the highlighted level changed.
    pub fn step(&mut self, outward: bool) -> bool {
        let next = if outward {
            self.level.saturating_sub(1)
        } else {
            (self.level + 1).min(self.rects.len().saturating_sub(1))
        };
        let changed = next != self.level;
        self.level = next;
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: i32, y: i32) -> Point {
        Point { x, y }
    }
    fn r(left: i32, top: i32, width: i32, height: i32) -> Rect {
        Rect {
            left,
            top,
            width,
            height,
        }
    }
    const VIEW: Rect = Rect {
        left: 0,
        top: 0,
        width: 1920,
        height: 1080,
    };

    #[test]
    fn selection_rect_cases_from_eisen() {
        assert_eq!(
            selection_rect(p(100, 150), p(300, 400), false),
            r(100, 150, 200, 250)
        );
        assert_eq!(
            selection_rect(p(400, 500), p(150, 200), false),
            r(150, 200, 250, 300)
        );
        assert_eq!(
            selection_rect(p(100, 100), p(400, 200), true),
            r(100, 100, 300, 300)
        );
        assert_eq!(
            selection_rect(p(500, 500), p(300, 450), true),
            r(300, 300, 200, 200)
        );
    }

    #[test]
    fn toolbar_cases_from_eisen() {
        let (pos, inside) = toolbar_position(r(400, 200, 600, 400), VIEW, 320, 40, 16);
        assert!(!inside);
        assert_eq!(pos, p(680, 616));
        let (pos, inside) = toolbar_position(r(400, 700, 600, 350), VIEW, 320, 40, 16);
        assert!(!inside);
        assert_eq!(pos.y, 644);
        let (pos, inside) = toolbar_position(r(0, 10, 1920, 1060), VIEW, 320, 40, 16);
        assert!(inside);
        assert!(pos.y <= 1080 - 40 - 16 && pos.x <= 1920 - 336);
        let (pos, _) = toolbar_position(r(1800, 200, 200, 200), VIEW, 320, 40, 16);
        assert!(pos.x <= 1920 - 336 && pos.x >= 16);
    }

    #[test]
    fn toolbar_stays_on_second_monitor() {
        let mon2 = r(2560, 0, 1920, 1080);
        let (pos, _) = toolbar_position(r(2600, 100, 300, 200), mon2, 320, 40, 16);
        assert!(pos.x >= mon2.left + 16 && pos.x + 336 <= mon2.right());
    }

    #[test]
    fn shift_rect_cases_from_eisen() {
        assert_eq!(
            shift_rect(r(100, 100, 200, 150), 50, -30, None),
            r(150, 70, 200, 150)
        );
        let s = shift_rect(r(100, 100, 200, 150), -500, 2000, Some(VIEW));
        assert_eq!(s, r(0, 930, 200, 150));
    }

    #[test]
    fn opposite_corners_from_eisen() {
        let rc = r(10, 20, 100, 50);
        assert_eq!(opposite_corner(rc, Corner::Nw), p(110, 70));
        assert_eq!(opposite_corner(rc, Corner::Ne), p(10, 70));
        assert_eq!(opposite_corner(rc, Corner::Sw), p(110, 20));
        assert_eq!(opposite_corner(rc, Corner::Se), p(10, 20));
    }

    #[test]
    fn draw_move_resize_flow() {
        let mut s = Selection::new(1920, 1080);
        s.press(p(100, 100), false, 8);
        s.drag(p(300, 250), false);
        s.release();
        assert!(s.confirmed());
        assert_eq!(s.rect(), Some(r(100, 100, 200, 150)));

        s.press(p(200, 200), false, 8);
        assert!(s.is_dragging());
        s.drag(p(210, 190), false);
        s.release();
        assert_eq!(s.rect(), Some(r(110, 90, 200, 150)));

        s.press(p(310, 240), false, 8);
        s.drag(p(410, 290), false);
        s.release();
        assert_eq!(s.rect(), Some(r(110, 90, 300, 200)));
    }

    #[test]
    fn tiny_selection_is_dropped() {
        let mut s = Selection::new(1920, 1080);
        s.press(p(100, 100), false, 8);
        s.drag(p(105, 300), false);
        s.release();
        assert!(!s.confirmed());
        assert_eq!(s.rect(), None);
    }

    #[test]
    fn clamps_to_bounds_and_moves_inside() {
        let mut s = Selection::new(1920, 1080);
        s.press(p(-50, -50), false, 8);
        s.drag(p(5000, 5000), false);
        s.release();
        assert_eq!(s.rect(), Some(r(0, 0, 1920, 1080)));
        s.nudge(10, 10);
        assert_eq!(s.rect(), Some(r(0, 0, 1920, 1080)));
    }

    #[test]
    fn space_pans_while_drawing() {
        let mut s = Selection::new(1920, 1080);
        s.press(p(100, 100), false, 8);
        s.drag(p(200, 200), false);
        s.set_pan(true);
        s.drag(p(250, 230), false);
        s.set_pan(false);
        s.release();
        assert_eq!(s.rect(), Some(r(150, 130, 100, 100)));
    }

    #[test]
    fn shift_square_is_kept_on_release() {
        let mut s = Selection::new(1920, 1080);
        s.press(p(100, 100), true, 8);
        s.drag(p(400, 200), true);
        s.release();
        assert_eq!(s.rect(), Some(r(100, 100, 300, 300)));
    }

    #[test]
    fn set_rect_clips_and_confirms() {
        let mut s = Selection::new(1920, 1080);
        s.set_rect(r(-10, 500, 300, 900));
        assert!(s.confirmed());
        assert_eq!(s.rect(), Some(r(0, 500, 290, 580)));
        s.set_rect(r(5000, 5000, 10, 10));
        assert!(!s.confirmed());
    }

    #[test]
    fn hover_chain_walks_levels() {
        let win = r(0, 0, 800, 600);
        let pane = r(10, 10, 300, 500);
        let button = r(20, 20, 80, 30);
        let mut h = HoverChain::new(vec![win, pane, pane, button, r(0, 0, 0, 5)]);
        assert_eq!(h.current(), Some(button));
        assert!(h.step(true));
        assert_eq!(h.current(), Some(pane));
        assert!(h.step(true));
        assert!(!h.step(true));
        assert_eq!(h.current(), Some(win));
        assert!(h.step(false));
        assert!(h.step(false));
        assert!(!h.step(false));
        assert_eq!(h.current(), Some(button));
        assert_eq!(HoverChain::new(vec![]).current(), None);

        h.retain_containing(p(200, 400));
        assert_eq!(h.current(), Some(pane));
        assert_eq!(h.outermost(), Some(win));
        assert_eq!(h.deepest(), Some(pane));
        h.retain_containing(p(5000, 5000));
        assert_eq!(h.current(), None);
    }
}
