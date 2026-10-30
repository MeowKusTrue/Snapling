//! Fullscreen region-selection overlay over the virtual screen. The window and frame blits
//! are Win32/GDI; everything drawn on top comes from `snapling-scene` (the shared mockup renderer).
//! Interaction follows EiSen's overlay (MIT, © 2026 Tran Trong Thuc):
//! drag to select, Shift = square, Space = pan, corners resize, drag inside moves,
//! arrows nudge (Shift ×10), Enter/C/double-click copy, S save, E editor, O text (OCR), R translate, Esc/right click cancel.
//! Snapling additions: hover highlights the window/element under the cursor, click picks it,
//! the wheel walks element levels (up = outer). The tool rail is always on screen: tools 1–9
//! draw anywhere, before or after selecting, and committed shapes are baked into `annot`.
//! L selects a freeform outline, V goes back to the rectangle.

use crate::{Image, crop_bgra, dib_header, targets, uia};
use snapling_core::annotate::{self, BlurStyle, Element, Head, History, Kind, Pt};
use snapling_core::i18n::t;
use snapling_core::selection::{Corner, Hit, HoverChain, MIN_SELECTION_PX, Point, Rect, Selection};
use snapling_scene::{
    Button, COLORS, Control, Draw, Hover, Layout, SIZES, Scene, TOOLS, Target, Tool,
};
use std::cell::{Cell, RefCell};
use std::sync::mpsc::Sender;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BITMAPINFO, BeginPaint, BitBlt, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, EndPaint, GdiFlush, GetMonitorInfoW, HBITMAP, HDC, InvalidateRect,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, PAINTSTRUCT, SRCCOPY, SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, VK_BACK, VK_CONTROL, VK_DOWN, VK_ESCAPE,
    VK_LEFT, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_DBLCLKS, CreateWindowExW, DefWindowProcW, GetCursorPos, HWND_TOPMOST, IDC_CROSS, IDC_HAND,
    IDC_IBEAM, IDC_SIZEALL, IDC_SIZENESW, IDC_SIZENWSE, LoadCursorW, RegisterClassW, SW_HIDE,
    SW_SHOW, SWP_NOACTIVATE, SetCursor, SetForegroundWindow, SetWindowPos, ShowWindow, WM_ACTIVATE,
    WM_CHAR, WM_ERASEBKGND, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MBUTTONDOWN, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT, WM_RBUTTONDOWN, WM_SETCURSOR, WNDCLASSW,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, w};

// Mockup: rgba(4,8,16,a) over the frame, as BGR; a in 1/256.
const DIM: [u32; 3] = [16, 8, 4];
const DIM_SELECT: u32 = 108;
const DIM_PAINT: u32 = 56;

type Done = Box<dyn FnOnce(Option<(Image, Outcome)>)>;

/// What the user did with a confirmed selection.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Copy,
    Save,
    Edit,
    Text,
    Translate,
}

struct State {
    origin: (i32, i32),
    width: u32,
    height: u32,
    orig: HBITMAP,
    orig_bits: *const u8,
    dim: HBITMAP,
    dim_bits: *mut u8,
    /// Darkness `dim` was built with; `None` = rebuild, its annotations are stale.
    dim_level: Option<u32>,
    /// The frame with committed annotations: what the selection shows and what is copied.
    annot: HBITMAP,
    annot_bits: *mut u8,
    back: HBITMAP,
    back_bits: *mut u8,
    sel: Selection,
    mouse: Point,
    /// Area of the last drawn decorations, to clear them on the next frame.
    decor: Option<Rect>,
    layout: Layout,
    hover_btn: Option<Control>,
    /// Exe name of the hovered window, for the hover pill.
    app: String,
    pen: Pen,
    /// Selecting ends the overlay with this outcome (OCR hotkeys).
    auto: Option<Outcome>,
    /// The press started a selection, so its release may finish one. A press on a rail
    /// button must not pick the window under that button.
    armed: bool,
    /// Finished freeform outline; `sel` then holds its bounding box.
    lasso: Vec<Pt>,
    /// Outline being drawn.
    stroke: Vec<Pt>,
    /// Monitor of the tool rail.
    rail: Rect,
    windows: Vec<(isize, Rect)>,
    monitors: Vec<Rect>,
    chain: HoverChain,
    chain_win: Option<isize>,
    chain_seq: u64,
    on_done: Option<Done>,
}

/// In-overlay editor state.
struct Pen {
    tool: Option<Tool>,
    /// Freeform selection instead of the rectangle.
    lasso_mode: bool,
    color: [u8; 3],
    size: f32,
    blur: BlurStyle,
    history: History,
    /// Shape being dragged.
    live: Option<Element>,
    /// Text being typed and where.
    typing: Option<(Pt, String)>,
}

impl Default for Pen {
    fn default() -> Self {
        Self {
            tool: None,
            lasso_mode: false,
            color: COLORS[0],
            size: SIZES[1],
            blur: BLUR.get(),
            history: History::default(),
            live: None,
            typing: None,
        }
    }
}

const WM_UIA: u32 = 0x8000 + 1;

thread_local! {
    static HWND_OVERLAY: Cell<HWND> = Cell::new(HWND::default());
    /// `[capture] instant_copy`.
    static INSTANT: Cell<bool> = const { Cell::new(false) };
    /// `[blur]`, the brush a new overlay starts with.
    static BLUR: Cell<BlurStyle> = const {
        Cell::new(BlurStyle {
            gauss: false,
            strength: 0,
        })
    };
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    static UIA: RefCell<Option<Sender<uia::Request>>> = const { RefCell::new(None) };
}

fn rect_from(r: RECT, origin: (i32, i32)) -> Rect {
    Rect {
        left: r.left - origin.0,
        top: r.top - origin.1,
        width: r.right - r.left,
        height: r.bottom - r.top,
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

/// Window → monitor fallback under `p`, plus a UIA request for the elements inside.
/// Returns whether the highlighted rect changed.
fn update_hover(s: &mut State, p: Point) -> bool {
    if s.sel.confirmed() || s.sel.is_dragging() || s.pen.tool.is_some() || s.pen.lasso_mode {
        return false;
    }
    let before = s.chain.current();
    let win = s.windows.iter().find(|(_, r)| r.contains(p)).copied();
    let id = win.map(|w| w.0);
    if id == s.chain_win && s.chain.deepest().is_some_and(|r| r.contains(p)) {
        return false;
    }
    if id == s.chain_win {
        s.chain.retain_containing(p);
    } else {
        let base = win
            .map(|w| w.1)
            .or_else(|| s.monitors.iter().find(|m| m.contains(p)).copied());
        s.chain = HoverChain::new(base.into_iter().collect());
        s.chain_win = id;
        s.app = id.map(crate::app_name_of).unwrap_or_default();
    }
    if let Some(hwnd) = id {
        s.chain_seq += 1;
        let req = uia::Request {
            seq: s.chain_seq,
            hwnd,
            point: POINT {
                x: p.x + s.origin.0,
                y: p.y + s.origin.1,
            },
        };
        UIA.with(|u| u.borrow().as_ref().map(|tx| tx.send(req)));
    }
    before != s.chain.current()
}

/// Merges a finished UIA chain if it still belongs to the hovered window.
/// Any finished chain for the hovered window is useful even if the cursor moved on
/// (replies take 60–180 ms in Discord): its levels still under the cursor are kept.
fn apply_uia() -> bool {
    let Some((hwnd, rects)) = uia::RESULT.lock().ok().and_then(|mut r| r.take()) else {
        return false;
    };
    with(|s| {
        let Some(base) = s.chain.outermost() else {
            return false;
        };
        if Some(hwnd) != s.chain_win || s.sel.confirmed() || s.sel.is_dragging() {
            return false;
        }
        let mut all = vec![base];
        all.extend(
            rects
                .into_iter()
                .skip(1)
                .map(|r| clip(rect_from(r, s.origin), base))
                .take_while(|r| r.contains(s.mouse)),
        );
        let fresh = HoverChain::new(all);
        if fresh.deepest() == s.chain.deepest() || fresh.len() < s.chain.len() {
            return false;
        }
        s.chain = fresh;
        true
    })
    .unwrap_or(false)
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.borrow_mut().as_mut().map(f))
}

pub fn is_open() -> bool {
    STATE.with(|s| s.borrow().is_some())
}

/// Registers the class and creates the hidden window once, so `show` is instant.
pub fn init() -> Result<(), String> {
    unsafe {
        let hinst = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        let class = w!("SnaplingOverlay");
        let wc = WNDCLASSW {
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: class,
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            return Err("RegisterClassW".into());
        }
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class,
            w!("Snapling"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinst.into()),
            None,
        )
        .map_err(|e| format!("CreateWindowExW: {e}"))?;
        HWND_OVERLAY.set(hwnd);
        UIA.with(|u| *u.borrow_mut() = Some(uia::spawn(hwnd.0 as isize, WM_UIA)));
    }
    Ok(())
}

fn dib(width: u32, height: u32) -> Result<(HBITMAP, *mut u8), String> {
    let bmi = BITMAPINFO {
        bmiHeader: dib_header(width, -(height as i32)),
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    let bmp = unsafe { CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) }
        .map_err(|e| format!("CreateDIBSection: {e}"))?;
    Ok((bmp, bits.cast()))
}

/// Shows the frozen frame; `on_done` gets the cropped region, or `None` on cancel.
pub fn show(
    img: Image,
    auto: Option<Outcome>,
    on_done: impl FnOnce(Option<(Image, Outcome)>) + 'static,
) -> Result<(), String> {
    let hwnd = HWND_OVERLAY.get();
    if hwnd.is_invalid() || is_open() {
        return Err(t("overlay недоступен").into());
    }
    let len = img.bgra.len();
    let (orig, orig_bits) = dib(img.width, img.height)?;
    let (dim, dim_bits) = dib(img.width, img.height)?;
    let (back, back_bits) = dib(img.width, img.height)?;
    let (annot, annot_bits) = dib(img.width, img.height)?;
    unsafe {
        std::ptr::copy_nonoverlapping(img.bgra.as_ptr(), orig_bits, len);
        std::ptr::copy_nonoverlapping(img.bgra.as_ptr(), annot_bits, len);
    }
    let (x, y, w, h) = (img.x, img.y, img.width as i32, img.height as i32);
    let windows = targets::windows()
        .into_iter()
        .map(|(id, r)| (id, rect_from(r, (x, y))))
        .collect();
    let monitors = targets::monitors()
        .into_iter()
        .map(|r| rect_from(r, (x, y)))
        .collect();
    let mut cursor = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut cursor);
    }
    let mouse = Point {
        x: cursor.x - x,
        y: cursor.y - y,
    };
    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            origin: (x, y),
            width: img.width,
            height: img.height,
            orig,
            orig_bits,
            dim,
            dim_bits,
            dim_level: None,
            back,
            annot,
            annot_bits,
            back_bits,
            sel: Selection::new(w, h),
            mouse,
            decor: None,
            layout: Layout::default(),
            hover_btn: None,
            app: String::new(),
            pen: Pen::default(),
            auto,
            armed: false,
            lasso: Vec::new(),
            stroke: Vec::new(),
            rail: monitor_at((x, y), mouse).0,
            windows,
            monitors,
            chain: HoverChain::default(),
            chain_win: None,
            chain_seq: 0,
            on_done: Some(Box::new(on_done)),
        })
    });
    drop(img);
    with(|s| update_hover(s, mouse));
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE);
        let _ = InvalidateRect(Some(hwnd), None, false);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
    }
    Ok(())
}

fn finish(hwnd: HWND, out: Option<Outcome>) {
    if out.is_some() {
        with(|s| {
            if let Some(el) = s.pen.live.take() {
                commit(s, el);
            }
            commit_text(s);
        });
    }
    let Some(s) = STATE.with(|st| st.borrow_mut().take()) else {
        return;
    };
    let result = if let Some(out) = out {
        s.sel.rect().filter(|_| s.sel.confirmed()).and_then(|r| {
            let len = s.width as usize * s.height as usize * 4;
            let src = unsafe { std::slice::from_raw_parts(s.annot_bits, len) };
            let mut img = crop_bgra(src, s.width, s.height, s.origin, r)?;
            if !s.lasso.is_empty() {
                let (dx, dy) = ((img.x - s.origin.0) as f32, (img.y - s.origin.1) as f32);
                let pts: Vec<Pt> = s
                    .lasso
                    .iter()
                    .map(|p| Pt {
                        x: p.x - dx,
                        y: p.y - dy,
                    })
                    .collect();
                annotate::cut_polygon(&mut img.bgra, img.width, img.height, &pts);
            }
            Some((img, out))
        })
    } else {
        None
    };
    unsafe {
        let _ = ReleaseCapture();
        let _ = ShowWindow(hwnd, SW_HIDE);
        for b in [s.orig, s.dim, s.annot, s.back] {
            let _ = DeleteObject(b.into());
        }
    }
    if let Some(cb) = s.on_done {
        cb(result);
    }
}

/// Monitor bounds (overlay coordinates) and DPI scale at `p`.
fn monitor_at(origin: (i32, i32), p: Point) -> (Rect, f32) {
    unsafe {
        let hmon = MonitorFromPoint(
            POINT {
                x: p.x + origin.0,
                y: p.y + origin.1,
            },
            MONITOR_DEFAULTTONEAREST,
        );
        let mut mi = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(hmon, &mut mi);
        let (mut dx, mut dy) = (96, 96);
        let _ = GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let m = mi.rcMonitor;
        let r = Rect {
            left: m.left - origin.0,
            top: m.top - origin.1,
            width: m.right - m.left,
            height: m.bottom - m.top,
        };
        (r, dx as f32 / 96.0)
    }
}

fn px(v: f32, scale: f32) -> i32 {
    (v * scale).round().max(1.0) as i32
}

fn handle_size(s: &State, p: Point) -> i32 {
    px(8.0, monitor_at(s.origin, p).1)
}

fn to_rect(r: Rect) -> RECT {
    RECT {
        left: r.left,
        top: r.top,
        right: r.right(),
        bottom: r.bottom(),
    }
}

fn union(a: RECT, b: RECT) -> RECT {
    if a.right <= a.left {
        return b;
    }
    RECT {
        left: a.left.min(b.left),
        top: a.top.min(b.top),
        right: a.right.max(b.right),
        bottom: a.bottom.max(b.bottom),
    }
}

/// Scene for the current state; the caller lays it out and draws it.
fn scene(s: &State) -> Scene<'_> {
    let drawing = !s.stroke.is_empty();
    let free = s.pen.tool.is_none() && !s.pen.lasso_mode;
    let hovering = s.sel.rect().is_none() && !s.sel.is_dragging() && free;
    let hover = hovering
        .then(|| s.chain.current())
        .flatten()
        .map(|rect| Hover {
            rect,
            target: match s.chain_win {
                None => Target::Screen,
                Some(_) if s.chain.current() == s.chain.outermost() => Target::Window,
                Some(_) => Target::Element,
            },
            app: &s.app,
        });
    let anchor = s
        .sel
        .rect()
        .or(hover.as_ref().map(|h| h.rect))
        .map_or(s.mouse, |r| Point {
            x: r.right() - 1,
            y: r.bottom() - 1,
        });
    let (monitor, scale) = monitor_at(s.origin, anchor);
    let lasso = if drawing { &s.stroke } else { &s.lasso };
    Scene {
        scale,
        monitor,
        cursor_monitor: monitor_at(s.origin, s.mouse).0,
        selection: s.sel.rect().filter(|_| !drawing),
        confirmed: s.sel.confirmed() && !drawing,
        dragging: s.sel.is_dragging() || drawing,
        hover,
        hover_button: s.hover_btn,
        tools: Some(Draw {
            monitor: s.rail,
            lasso_mode: s.pen.lasso_mode,
            lasso: (!lasso.is_empty()).then_some(lasso.as_slice()),
            tool: s.pen.tool,
            color: s.pen.color,
            size: s.pen.size,
            blur: s.pen.blur,
            can_undo: s.pen.history.can_undo(),
            can_redo: s.pen.history.can_redo(),
            live: s.pen.live.as_ref(),
            caret: s.pen.typing.as_ref().map(|(p, t)| (*p, t.as_str())),
        }),
    }
}

fn dim_wanted(s: &State) -> u32 {
    if s.sel.rect().is_none() && s.pen.tool.is_some() {
        DIM_PAINT
    } else {
        DIM_SELECT
    }
}

/// Rebuilds `dim` from `annot` when the darkness changed or shapes were committed.
fn sync_dim(s: &mut State) {
    let a = dim_wanted(s);
    if s.dim_level == Some(a) {
        return;
    }
    s.dim_level = Some(a);
    let len = s.width as usize * s.height as usize * 4;
    unsafe {
        let _ = GdiFlush();
        let src = std::slice::from_raw_parts(s.annot_bits, len);
        let dst = std::slice::from_raw_parts_mut(s.dim_bits, len);
        dim_into(dst, src, a);
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

/// Repaints the old and the new decoration areas only (`full`: everything). Shapes can be
/// anywhere on screen, so drawing them repaints it all.
// ponytail: a full blit per mouse move while drawing; track the shape's box if it ever lags.
fn invalidate(hwnd: HWND, full: bool) {
    let areas = with(|s| {
        let new = snapling_scene::layout(&scene(s)).bounds;
        let wide = s.pen.live.is_some() || s.pen.typing.is_some();
        (s.decor, new, wide || s.dim_level != Some(dim_wanted(s)))
    });
    unsafe {
        match areas {
            Some((old, new, wide)) if !full && !wide => {
                for r in [old, new].into_iter().flatten() {
                    let _ = InvalidateRect(Some(hwnd), Some(&to_rect(r)), false);
                }
            }
            _ => {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
        }
    }
}

unsafe fn blit(dst: HDC, src: HDC, r: RECT) {
    unsafe {
        let _ = BitBlt(
            dst,
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            Some(src),
            r.left,
            r.top,
            SRCCOPY,
        );
    }
}

fn intersect(a: RECT, b: RECT) -> Option<RECT> {
    let r = RECT {
        left: a.left.max(b.left),
        top: a.top.max(b.top),
        right: a.right.min(b.right),
        bottom: a.bottom.min(b.bottom),
    };
    (r.right > r.left && r.bottom > r.top).then_some(r)
}

fn paint(hwnd: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        with(|s| draw(s, hdc, ps.rcPaint));
        let _ = EndPaint(hwnd, &ps);
    }
}

/// Frame into the back buffer (dimmed, original inside the selection), the scene on top,
/// then the dirty rect to the screen. The frame is re-blitted under every decoration so
/// glass never blurs its own previous frame.
unsafe fn draw(s: &mut State, hdc: HDC, clip: RECT) {
    sync_dim(s);
    let sc = scene(s);
    let lay = snapling_scene::layout(&sc);
    let screen = RECT {
        left: 0,
        top: 0,
        right: s.width as i32,
        bottom: s.height as i32,
    };
    let area = lay
        .bounds
        .and_then(|b| intersect(union(clip, to_rect(b)), screen))
        .unwrap_or(clip);
    unsafe {
        let back = CreateCompatibleDC(Some(hdc));
        let src = CreateCompatibleDC(Some(hdc));
        let old_back = SelectObject(back, s.back.into());
        let old_src = SelectObject(src, s.dim.into());
        blit(back, src, area);
        let lasso = sc.tools.as_ref().and_then(|t| t.lasso);
        if let Some(inter) = sc
            .selection
            .filter(|_| lasso.is_none())
            .and_then(|r| intersect(to_rect(r), area))
        {
            SelectObject(src, s.annot.into());
            blit(back, src, inter);
        }
        let _ = GdiFlush();
        let len = s.width as usize * s.height as usize * 4;
        let bits = std::slice::from_raw_parts_mut(s.back_bits, len);
        if let Some(mut pm) = tiny_skia::PixmapMut::from_bytes(bits, s.width, s.height) {
            let frame = std::slice::from_raw_parts(s.annot_bits, len);
            if let (Some(pts), Some(from)) = (
                lasso,
                tiny_skia::PixmapRef::from_bytes(frame, s.width, s.height),
            ) {
                snapling_scene::paint_region(&mut pm, from, pts);
            }
            snapling_scene::draw(&mut pm, &sc, &lay);
        }
        blit(hdc, back, union(clip, area));
        SelectObject(back, old_back);
        SelectObject(src, old_src);
        let _ = DeleteDC(back);
        let _ = DeleteDC(src);
    }
    s.decor = lay.bounds;
    s.layout = lay;
}

fn lparam_point(lp: LPARAM) -> Point {
    Point {
        x: (lp.0 & 0xFFFF) as u16 as i16 as i32,
        y: ((lp.0 >> 16) & 0xFFFF) as u16 as i16 as i32,
    }
}

fn shift_down() -> bool {
    unsafe { GetKeyState(VK_SHIFT.0 as i32) < 0 }
}

fn button_at(s: &State, p: Point) -> Option<Control> {
    snapling_scene::button_at(&s.layout, p)
}

fn act(hwnd: HWND, b: Button) {
    let out = match b {
        Button::Copy => Some(Outcome::Copy),
        Button::Save => Some(Outcome::Save),
        Button::Edit => Some(Outcome::Edit),
        Button::Text => Some(Outcome::Text),
        Button::Translate => Some(Outcome::Translate),
        Button::Cancel => None,
    };
    finish(hwnd, out);
}

/// Re-renders `annot` = frame + committed shapes (on commit and undo only).
fn bake(s: &mut State) {
    let len = s.width as usize * s.height as usize * 4;
    unsafe {
        let _ = GdiFlush();
        std::ptr::copy_nonoverlapping(s.orig_bits, s.annot_bits, len);
        let bits = std::slice::from_raw_parts_mut(s.annot_bits, len);
        if let Some(mut pm) = tiny_skia::PixmapMut::from_bytes(bits, s.width, s.height) {
            for e in &s.pen.history.doc.elements {
                snapling_scene::draw_element(&mut pm, e);
            }
        }
    }
    s.dim_level = None;
}

fn commit(s: &mut State, el: Element) {
    s.pen.history.push(el);
    bake(s);
}

fn commit_text(s: &mut State) {
    if let Some((at, text)) = s.pen.typing.take() {
        let el = Element {
            kind: Kind::Text(at, text.trim_end().to_string()),
            color: s.pen.color,
            size: s.pen.size,
        };
        commit(s, el);
    }
}

/// Pointer in image coordinates.
fn pen_point(p: Point) -> Pt {
    Pt {
        x: p.x as f32,
        y: p.y as f32,
    }
}

/// V or L: leaves the drawing tool and drops a freeform outline.
fn set_mode(s: &mut State, lasso: bool) {
    s.pen.tool = None;
    s.pen.lasso_mode = lasso;
    if lasso || !s.lasso.is_empty() {
        s.lasso.clear();
        s.sel = Selection::new(s.width as i32, s.height as i32);
    }
}

/// Mouse released: the outline becomes the selection, a stray click changes nothing.
fn close_lasso(s: &mut State) {
    let pts = std::mem::take(&mut s.stroke);
    let min = |f: fn(&Pt) -> f32| pts.iter().map(f).fold(f32::MAX, f32::min);
    let max = |f: fn(&Pt) -> f32| pts.iter().map(f).fold(f32::MIN, f32::max);
    let (x0, y0) = (min(|p| p.x) as i32, min(|p| p.y) as i32);
    let (x1, y1) = (max(|p| p.x) as i32 + 1, max(|p| p.y) as i32 + 1);
    if x1 - x0 < MIN_SELECTION_PX || y1 - y0 < MIN_SELECTION_PX {
        return;
    }
    s.lasso = pts;
    s.sel = Selection::new(s.width as i32, s.height as i32);
    s.sel.set_rect(Rect {
        left: x0,
        top: y0,
        width: x1 - x0,
        height: y1 - y0,
    });
}

/// Press with a drawing tool: starts a shape, places a number or opens the text caret.
fn pen_down(s: &mut State, tool: Tool, p: Point) {
    commit_text(s);
    let at = pen_point(p);
    let pen = &s.pen;
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
            s.pen.typing = Some((at, String::new()));
            return;
        }
    };
    let el = Element {
        kind,
        color: pen.color,
        size: pen.size,
    };
    if tool == Tool::Step {
        commit(s, el);
    } else {
        s.pen.live = Some(el);
    }
}

fn control(hwnd: HWND, c: Control) {
    if let Control::Action(b) = c {
        return act(hwnd, b);
    }
    with(|s| {
        if matches!(c, Control::Select | Control::Lasso) {
            return set_mode(s, c == Control::Lasso);
        }
        let pen = &mut s.pen;
        match c {
            Control::Tool(t) => pen.tool = (pen.tool != Some(t)).then_some(t),
            Control::Color(i) => pen.color = COLORS[i],
            Control::Size(i) => pen.size = SIZES[i],
            Control::Gauss(g) => pen.blur.gauss = g,
            Control::Strength(i) => pen.blur.strength = i,
            Control::Undo | Control::Redo => {
                pen.typing = None;
                let changed = if c == Control::Undo {
                    pen.history.undo()
                } else {
                    pen.history.redo()
                };
                if changed {
                    bake(s);
                }
            }
            Control::Action(_) | Control::Select | Control::Lasso => {}
        }
    });
    invalidate(
        hwnd,
        matches!(
            c,
            Control::Undo | Control::Redo | Control::Select | Control::Lasso
        ),
    );
}

fn set_cursor() {
    let id = with(|s| {
        let mut pt = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut pt);
        }
        let p = Point {
            x: pt.x - s.origin.0,
            y: pt.y - s.origin.1,
        };
        if button_at(s, p).is_some() {
            return IDC_HAND;
        }
        let hit = if s.lasso.is_empty() {
            s.sel.hit(p, handle_size(s, p))
        } else {
            Hit::Outside
        };
        match (s.pen.tool, hit) {
            (_, Hit::Corner(Corner::Nw | Corner::Se)) => IDC_SIZENWSE,
            (_, Hit::Corner(_)) => IDC_SIZENESW,
            (Some(Tool::Text), _) => IDC_IBEAM,
            (None, Hit::Inside) if !s.pen.lasso_mode => IDC_SIZEALL,
            _ => IDC_CROSS,
        }
    })
    .unwrap_or(IDC_CROSS);
    unsafe {
        if let Ok(c) = LoadCursorW(None, PCWSTR(id.0)) {
            SetCursor(Some(c));
        }
    }
}

fn ctrl_down() -> bool {
    unsafe { GetKeyState(VK_CONTROL.0 as i32) < 0 }
}

/// Keys while text is being typed; `true` = consumed.
fn typing_key(hwnd: HWND, vk: u16) -> bool {
    let typing = with(|s| s.pen.typing.is_some()) == Some(true);
    if !typing {
        return false;
    }
    with(|s| match vk {
        v if v == VK_RETURN.0 && shift_down() => {
            if let Some((_, t)) = &mut s.pen.typing {
                t.push('\n');
            }
        }
        v if v == VK_RETURN.0 => commit_text(s),
        v if v == VK_ESCAPE.0 => s.pen.typing = None,
        v if v == VK_BACK.0 => {
            if let Some((_, t)) = &mut s.pen.typing {
                t.pop();
            }
        }
        _ => {}
    });
    invalidate(hwnd, true);
    true
}

/// Letter keys of the confirmed selection.
fn outcome_key(vk: u16) -> Option<Outcome> {
    match u8::try_from(vk).ok()? {
        b'C' => Some(Outcome::Copy),
        b'S' => Some(Outcome::Save),
        b'E' => Some(Outcome::Edit),
        b'O' => Some(Outcome::Text),
        b'R' => Some(Outcome::Translate),
        _ => None,
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => paint(hwnd),
        WM_ERASEBKGND => return LRESULT(1),
        WM_SETCURSOR => {
            set_cursor();
            return LRESULT(1);
        }
        WM_LBUTTONDOWN => {
            let p = lparam_point(lp);
            let clicked = with(|s| {
                s.mouse = p;
                if let Some(c) = button_at(s, p) {
                    return Some(c);
                }
                let hs = handle_size(s, p);
                let corner = s.lasso.is_empty() && matches!(s.sel.hit(p, hs), Hit::Corner(_));
                if let Some(tool) = s.pen.tool.filter(|_| !corner) {
                    pen_down(s, tool, p);
                } else if s.pen.lasso_mode && s.pen.tool.is_none() {
                    s.stroke = vec![pen_point(p)];
                } else {
                    s.armed = true;
                    s.sel.press(p, shift_down(), hs);
                }
                None
            })
            .flatten();
            match clicked {
                Some(c) => control(hwnd, c),
                None => unsafe {
                    SetCapture(hwnd);
                    invalidate(hwnd, false);
                },
            }
        }
        WM_MOUSEMOVE => {
            let p = lparam_point(lp);
            let repaint = with(|s| {
                s.mouse = p;
                let at = pen_point(p);
                if let Some(el) = &mut s.pen.live {
                    annotate::drag_to(&mut el.kind, at, shift_down(), 1.5);
                    return Some(false);
                }
                if let Some(last) = s.stroke.last() {
                    if (last.x - at.x).hypot(last.y - at.y) >= 2.0 {
                        s.stroke.push(at);
                    }
                    return Some(false);
                }
                if s.sel.is_dragging() {
                    s.sel.drag(p, shift_down());
                    return Some(false);
                }
                if update_hover(s, p) {
                    return Some(false);
                }
                let hover = button_at(s, p);
                (hover != s.hover_btn).then(|| {
                    s.hover_btn = hover;
                    false
                })
            })
            .flatten();
            if let Some(full) = repaint {
                invalidate(hwnd, full);
            }
        }
        WM_LBUTTONUP => {
            let selected = with(|s| {
                if let Some(el) = s.pen.live.take() {
                    commit(s, el);
                    return false;
                }
                if !s.stroke.is_empty() {
                    close_lasso(s);
                    return s.sel.confirmed();
                }
                if !std::mem::take(&mut s.armed) {
                    return false;
                }
                s.sel.release();
                // A click without a drag picks the highlighted window/element.
                let m = s.mouse;
                if !s.sel.confirmed()
                    && let Some(r) = s.chain.current().filter(|r| r.contains(m))
                {
                    s.sel.set_rect(r);
                }
                s.sel.confirmed()
            });
            unsafe {
                let _ = ReleaseCapture();
            }
            let auto = with(|s| s.auto).flatten();
            let instant = INSTANT.get().then_some(Outcome::Copy);
            match auto.or(instant).filter(|_| selected == Some(true)) {
                Some(out) => finish(hwnd, Some(out)),
                None => invalidate(hwnd, true),
            }
        }
        WM_LBUTTONDBLCLK => {
            let p = lparam_point(lp);
            if with(|s| s.pen.tool.is_none() && s.sel.hit(p, 0) == Hit::Inside) == Some(true) {
                finish(hwnd, Some(Outcome::Copy));
            }
        }
        WM_RBUTTONDOWN | WM_MBUTTONDOWN => finish(hwnd, None),
        WM_MOUSEWHEEL => {
            let outward = ((wp.0 >> 16) & 0xFFFF) as u16 as i16 > 0;
            let changed = with(|s| {
                !s.sel.confirmed()
                    && s.pen.tool.is_none()
                    && !s.pen.lasso_mode
                    && s.chain.step(outward)
            });
            if changed == Some(true) {
                invalidate(hwnd, false);
            }
        }
        WM_UIA => {
            if apply_uia() {
                invalidate(hwnd, false);
            }
        }
        WM_ACTIVATE if wp.0 & 0xFFFF == 0 => finish(hwnd, None),
        WM_CHAR => {
            let ch = char::from_u32(wp.0 as u32).filter(|c| !c.is_control());
            let typed = with(|s| match (&mut s.pen.typing, ch) {
                (Some((_, t)), Some(c)) => {
                    t.push(c);
                    true
                }
                _ => false,
            });
            if typed == Some(true) {
                invalidate(hwnd, false);
            }
        }
        WM_KEYDOWN | WM_KEYUP => {
            let down = msg == WM_KEYDOWN;
            let vk = wp.0 as u16;
            if down && typing_key(hwnd, vk) {
                return LRESULT(0);
            }
            let confirmed = with(|s| s.sel.confirmed()) == Some(true);
            match vk {
                v if v == VK_ESCAPE.0 && down => {
                    if with(|s| s.pen.tool.take().is_some()) == Some(true) {
                        invalidate(hwnd, false);
                    } else {
                        finish(hwnd, None);
                    }
                }
                v if down && ctrl_down() && (v == 0x5A || v == 0x59) => {
                    let undo = v == 0x5A && !shift_down();
                    control(hwnd, if undo { Control::Undo } else { Control::Redo });
                }
                v if down && (0x31..=0x39).contains(&v) => {
                    control(hwnd, Control::Tool(TOOLS[(v - 0x31) as usize]));
                }
                0x56 if down && !ctrl_down() => control(hwnd, Control::Select),
                0x4C if down && !ctrl_down() => control(hwnd, Control::Lasso),
                v if down && confirmed && v == VK_RETURN.0 => finish(hwnd, Some(Outcome::Copy)),
                // S and Ctrl+S both save, as EiSen does.
                v if down && confirmed && outcome_key(v).is_some() => {
                    finish(hwnd, outcome_key(v));
                }
                v if v == VK_SPACE.0 => {
                    with(|s| s.sel.set_pan(down));
                }
                v if v == VK_SHIFT.0 => {
                    with(|s| {
                        let m = s.mouse;
                        s.sel.drag(m, down);
                    });
                    invalidate(hwnd, false);
                }
                v if down && [VK_LEFT.0, VK_RIGHT.0, VK_UP.0, VK_DOWN.0].contains(&v) => {
                    let step = if shift_down() { 10 } else { 1 };
                    let (dx, dy) = match v {
                        v if v == VK_LEFT.0 => (-step, 0),
                        v if v == VK_RIGHT.0 => (step, 0),
                        v if v == VK_UP.0 => (0, -step),
                        _ => (0, step),
                    };
                    with(|s| {
                        if s.sel.confirmed() && s.lasso.is_empty() {
                            s.sel.nudge(dx, dy);
                        }
                    });
                    invalidate(hwnd, true);
                }
                _ => {}
            }
        }
        _ => return unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
    LRESULT(0)
}

/// Starting style of the blur brush.
pub fn set_blur(gauss: bool, strength: u8) {
    BLUR.set(BlurStyle { gauss, strength });
}

/// Selecting copies and closes at once, skipping the action toolbar.
pub fn set_instant_copy(on: bool) {
    INSTANT.set(on);
}

/// Hides the overlay from screen sharing and recording (Discord, OBS, Teams) while it stays
/// visible to the user. Windows 10 2004+; older systems return an error.
pub fn set_hidden_from_capture(hide: bool) -> Result<(), String> {
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE,
    };
    let hwnd = HWND_OVERLAY.get();
    if hwnd.is_invalid() {
        return Err(t("overlay не создан").into());
    }
    let mode = if hide {
        WDA_EXCLUDEFROMCAPTURE
    } else {
        WDA_NONE
    };
    crate::toast::set_hidden_from_capture(hide);
    unsafe { SetWindowDisplayAffinity(hwnd, mode) }.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dim_keeps_black_near_black_and_alpha_opaque() {
        let src = [255u8, 255, 255, 0, 0, 0, 0, 0];
        let mut dst = [0u8; 8];
        dim_into(&mut dst, &src, DIM_SELECT);
        assert_eq!(dst[3], 255);
        assert!(dst[0] > 140 && dst[0] < 160, "{dst:?}");
        assert!(dst[4] < 10, "{dst:?}");
    }

    /// `cargo test --release -p snapling-win bench_dim -- --ignored --nocapture`
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
}
