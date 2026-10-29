//! Fullscreen region-selection overlay over the virtual screen. The window and frame blits
//! are Win32/GDI; everything drawn on top comes from `snapling-scene` (the shared mockup renderer).
//! Interaction follows EiSen's overlay (MIT, © 2026 Tran Trong Thuc):
//! drag to select, Shift = square, Space = pan, corners resize, drag inside moves,
//! arrows nudge (Shift ×10), Enter/C/double-click copy, S save, E editor, O text (OCR), R translate, Esc/right click cancel.
//! Snapling additions: hover highlights the window/element under the cursor, click picks it,
//! the wheel walks element levels (up = outer).

use crate::{Image, crop_bgra, dib_header, targets, uia};
use snapling_core::selection::{Corner, Hit, HoverChain, Point, Rect, Selection};
use snapling_scene::{Button, Hover, Layout, Scene, Target};
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
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RETURN,
    VK_RIGHT, VK_SHIFT, VK_SPACE, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_DBLCLKS, CreateWindowExW, DefWindowProcW, GetCursorPos, HWND_TOPMOST, IDC_CROSS, IDC_HAND,
    IDC_SIZEALL, IDC_SIZENESW, IDC_SIZENWSE, LoadCursorW, RegisterClassW, SW_HIDE, SW_SHOW,
    SWP_NOACTIVATE, SetCursor, SetForegroundWindow, SetWindowPos, ShowWindow, WM_ACTIVATE,
    WM_ERASEBKGND, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MBUTTONDOWN, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT, WM_RBUTTONDOWN, WM_SETCURSOR, WNDCLASSW,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, w};

// Mockup: rgba(4,8,16,.5) over the frame, as BGR.
const DIM: [u32; 3] = [16, 8, 4];

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
    back: HBITMAP,
    back_bits: *mut u8,
    sel: Selection,
    mouse: Point,
    /// Area of the last drawn decorations, to clear them on the next frame.
    decor: Option<Rect>,
    layout: Layout,
    hover_btn: Option<Button>,
    /// Exe name of the hovered window, for the hover pill.
    app: String,
    windows: Vec<(isize, Rect)>,
    monitors: Vec<Rect>,
    chain: HoverChain,
    chain_win: Option<isize>,
    chain_seq: u64,
    on_done: Option<Done>,
}

const WM_UIA: u32 = 0x8000 + 1;

thread_local! {
    static HWND_OVERLAY: Cell<HWND> = Cell::new(HWND::default());
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
    if s.sel.confirmed() || s.sel.is_dragging() {
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
    on_done: impl FnOnce(Option<(Image, Outcome)>) + 'static,
) -> Result<(), String> {
    let hwnd = HWND_OVERLAY.get();
    if hwnd.is_invalid() || is_open() {
        return Err("overlay недоступен".into());
    }
    let len = img.bgra.len();
    let (orig, orig_bits) = dib(img.width, img.height)?;
    let (dim, dim_bits) = dib(img.width, img.height)?;
    let (back, back_bits) = dib(img.width, img.height)?;
    unsafe {
        std::ptr::copy_nonoverlapping(img.bgra.as_ptr(), orig_bits, len);
        let dim = std::slice::from_raw_parts_mut(dim_bits, len);
        for (i, (d, s)) in dim.iter_mut().zip(&img.bgra).enumerate() {
            *d = match i % 4 {
                3 => 255,
                c => ((u32::from(*s) + DIM[c]) / 2) as u8,
            };
        }
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
    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            origin: (x, y),
            width: img.width,
            height: img.height,
            orig,
            orig_bits,
            dim,
            back,
            back_bits,
            sel: Selection::new(w, h),
            mouse: Point::default(),
            decor: None,
            layout: Layout::default(),
            hover_btn: None,
            app: String::new(),
            windows,
            monitors,
            chain: HoverChain::default(),
            chain_win: None,
            chain_seq: 0,
            on_done: Some(Box::new(on_done)),
        })
    });
    drop(img);
    let mut cursor = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut cursor);
    }
    with(|s| {
        s.mouse = Point {
            x: cursor.x - x,
            y: cursor.y - y,
        };
        let m = s.mouse;
        update_hover(s, m);
    });
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
    let Some(s) = STATE.with(|st| st.borrow_mut().take()) else {
        return;
    };
    let result = if let Some(out) = out {
        s.sel.rect().filter(|_| s.sel.confirmed()).and_then(|r| {
            let len = s.width as usize * s.height as usize * 4;
            let src = unsafe { std::slice::from_raw_parts(s.orig_bits, len) };
            crop_bgra(src, s.width, s.height, s.origin, r).map(|img| (img, out))
        })
    } else {
        None
    };
    unsafe {
        let _ = ReleaseCapture();
        let _ = ShowWindow(hwnd, SW_HIDE);
        for b in [s.orig, s.dim, s.back] {
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
    let hovering = s.sel.rect().is_none() && !s.sel.is_dragging();
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
    Scene {
        scale,
        monitor,
        cursor_monitor: monitor_at(s.origin, s.mouse).0,
        selection: s.sel.rect(),
        confirmed: s.sel.confirmed(),
        dragging: s.sel.is_dragging(),
        hover,
        hover_button: s.hover_btn,
    }
}

/// Repaints the old and the new decoration areas only (`full`: everything).
fn invalidate(hwnd: HWND, full: bool) {
    let areas = with(|s| {
        let new = snapling_scene::layout(&scene(s)).bounds;
        (s.decor, new)
    });
    unsafe {
        match areas {
            Some((old, new)) if !full => {
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
        if let Some(inter) = sc.selection.and_then(|r| intersect(to_rect(r), area)) {
            SelectObject(src, s.orig.into());
            blit(back, src, inter);
        }
        let _ = GdiFlush();
        let len = s.width as usize * s.height as usize * 4;
        let bits = std::slice::from_raw_parts_mut(s.back_bits, len);
        if let Some(mut pm) = tiny_skia::PixmapMut::from_bytes(bits, s.width, s.height) {
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

fn button_at(s: &State, p: Point) -> Option<Button> {
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
        match s.sel.hit(p, handle_size(s, p)) {
            Hit::Corner(Corner::Nw | Corner::Se) => IDC_SIZENWSE,
            Hit::Corner(_) => IDC_SIZENESW,
            Hit::Inside => IDC_SIZEALL,
            Hit::Outside => IDC_CROSS,
        }
    })
    .unwrap_or(IDC_CROSS);
    unsafe {
        if let Ok(c) = LoadCursorW(None, PCWSTR(id.0)) {
            SetCursor(Some(c));
        }
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
                if let Some(b) = button_at(s, p) {
                    return Some(b);
                }
                let hs = handle_size(s, p);
                s.sel.press(p, shift_down(), hs);
                None
            })
            .flatten();
            match clicked {
                Some(a) => act(hwnd, a),
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
                    true
                })
            })
            .flatten();
            if let Some(full) = repaint {
                invalidate(hwnd, full);
            }
        }
        WM_LBUTTONUP => {
            with(|s| {
                s.sel.release();
                // A click without a drag picks the highlighted window/element.
                let m = s.mouse;
                if !s.sel.confirmed()
                    && let Some(r) = s.chain.current().filter(|r| r.contains(m))
                {
                    s.sel.set_rect(r);
                }
            });
            unsafe {
                let _ = ReleaseCapture();
            }
            invalidate(hwnd, true);
        }
        WM_LBUTTONDBLCLK => {
            let p = lparam_point(lp);
            if with(|s| s.sel.hit(p, 0) == Hit::Inside) == Some(true) {
                finish(hwnd, Some(Outcome::Copy));
            }
        }
        WM_RBUTTONDOWN | WM_MBUTTONDOWN => finish(hwnd, None),
        WM_MOUSEWHEEL => {
            let outward = ((wp.0 >> 16) & 0xFFFF) as u16 as i16 > 0;
            let changed = with(|s| !s.sel.confirmed() && s.chain.step(outward));
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
        WM_KEYDOWN | WM_KEYUP => {
            let down = msg == WM_KEYDOWN;
            let vk = wp.0 as u16;
            match vk {
                v if v == VK_ESCAPE.0 && down => finish(hwnd, None),
                v if (v == VK_RETURN.0 || v == b'C' as u16) && down => {
                    if with(|s| s.sel.confirmed()) == Some(true) {
                        finish(hwnd, Some(Outcome::Copy));
                    }
                }
                // S and Ctrl+S both save, as EiSen's S.
                v if v == b'S' as u16 && down => {
                    if with(|s| s.sel.confirmed()) == Some(true) {
                        finish(hwnd, Some(Outcome::Save));
                    }
                }
                v if v == b'E' as u16 && down => {
                    if with(|s| s.sel.confirmed()) == Some(true) {
                        finish(hwnd, Some(Outcome::Edit));
                    }
                }
                v if v == b'O' as u16 && down => {
                    if with(|s| s.sel.confirmed()) == Some(true) {
                        finish(hwnd, Some(Outcome::Text));
                    }
                }
                v if v == b'R' as u16 && down => {
                    if with(|s| s.sel.confirmed()) == Some(true) {
                        finish(hwnd, Some(Outcome::Translate));
                    }
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
                        if s.sel.confirmed() {
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

/// Hides the overlay from screen sharing and recording (Discord, OBS, Teams) while it stays
/// visible to the user. Windows 10 2004+; older systems return an error.
pub fn set_hidden_from_capture(hide: bool) -> Result<(), String> {
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE,
    };
    let hwnd = HWND_OVERLAY.get();
    if hwnd.is_invalid() {
        return Err("overlay не создан".into());
    }
    let mode = if hide {
        WDA_EXCLUDEFROMCAPTURE
    } else {
        WDA_NONE
    };
    unsafe { SetWindowDisplayAffinity(hwnd, mode) }.map_err(|e| e.to_string())
}
