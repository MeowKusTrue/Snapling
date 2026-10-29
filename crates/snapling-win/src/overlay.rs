//! Fullscreen region-selection overlay over the virtual screen (Win32 + GDI).
//! Interaction follows EiSen's overlay (MIT, © 2026 Tran Trong Thuc):
//! drag to select, Shift = square, Space = pan, corners resize, drag inside moves,
//! arrows nudge (Shift ×10), Enter/C/double-click copy, S save, E editor, O text (OCR), Esc/right click cancel.
//! Snapling additions: hover highlights the window/element under the cursor, click picks it,
//! the wheel walks element levels (up = outer).

use crate::{Image, crop_bgra, dib_header, targets, uia};
use snapling_core::selection::{Corner, Hit, HoverChain, Point, Rect, Selection, toolbar_position};
use std::cell::{Cell, RefCell};
use std::sync::mpsc::Sender;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BITMAPINFO, BeginPaint, BitBlt, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateCompatibleDC,
    CreateDIBSection, CreateFontW, CreatePen, CreateSolidBrush, DEFAULT_CHARSET, DIB_RGB_COLORS,
    DT_CALCRECT, DT_CENTER, DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject, DrawTextW, EndPaint,
    FW_SEMIBOLD, FillRect, GetMonitorInfoW, HBITMAP, HDC, InvalidateRect, MONITOR_DEFAULTTONEAREST,
    MONITORINFO, MonitorFromPoint, OUT_DEFAULT_PRECIS, PAINTSTRUCT, PS_SOLID, RoundRect, SRCCOPY,
    SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
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

// theme.json: selection.dim_overlay_rgba = rgba(0,0,0,0.48) → keep 52% of each channel.
const DIM_KEEP: u32 = 133;
const FRAME: u32 = 0xFFFFFF;
const FRAME_OUTER: u32 = 0x111827;
const ACCENT: u32 = 0x295BE8;
const ACCENT_HOVER: u32 = 0x2049C7;
const SURFACE: u32 = 0x171D2A;
const SURFACE_ALT: u32 = 0x222B3C;
const BORDER: u32 = 0x354158;
const TEXT: u32 = 0xF3F6FC;

type Done = Box<dyn FnOnce(Option<(Image, Outcome)>)>;

/// What the user did with a confirmed selection.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Copy,
    Save,
    Edit,
    Text,
}

#[derive(Clone, Copy, PartialEq)]
enum Action {
    Save,
    Copy,
    Edit,
    Text,
    Cancel,
}

struct State {
    origin: (i32, i32),
    width: u32,
    height: u32,
    orig: HBITMAP,
    orig_bits: *const u8,
    dim: HBITMAP,
    back: HBITMAP,
    sel: Selection,
    mouse: Point,
    decor: RECT,
    buttons: Vec<(RECT, Action)>,
    hover_btn: Option<usize>,
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

fn color(hex: u32) -> COLORREF {
    COLORREF(((hex >> 16) & 0xFF) | (hex & 0xFF00) | ((hex & 0xFF) << 16))
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
    let (back, _) = dib(img.width, img.height)?;
    unsafe {
        std::ptr::copy_nonoverlapping(img.bgra.as_ptr(), orig_bits, len);
        let dim = std::slice::from_raw_parts_mut(dim_bits, len);
        for (d, s) in dim.iter_mut().zip(&img.bgra) {
            *d = ((*s as u32 * DIM_KEEP) >> 8) as u8;
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
            sel: Selection::new(w, h),
            mouse: Point::default(),
            decor: RECT::default(),
            buttons: Vec::new(),
            hover_btn: None,
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

fn inflate(r: RECT, d: i32) -> RECT {
    RECT {
        left: r.left - d,
        top: r.top - d,
        right: r.right + d,
        bottom: r.bottom + d,
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

fn in_rect(r: &RECT, p: Point) -> bool {
    p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
}

/// While dragging only the old and new decoration areas are repainted.
fn invalidate(hwnd: HWND, full: bool) {
    let areas = with(|s| {
        let new = s.sel.rect().or(s.chain.current()).map(|r| {
            let scale = monitor_at(
                s.origin,
                Point {
                    x: r.left,
                    y: r.top,
                },
            )
            .1;
            inflate(to_rect(r), px(64.0, scale))
        });
        (s.decor, new)
    });
    unsafe {
        match areas {
            Some((old, new)) if !full => {
                let _ = InvalidateRect(Some(hwnd), Some(&old), false);
                if let Some(n) = new {
                    let _ = InvalidateRect(Some(hwnd), Some(&n), false);
                }
            }
            _ => {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
        }
    }
}

unsafe fn fill(hdc: HDC, r: RECT, hex: u32) {
    unsafe {
        let b = CreateSolidBrush(color(hex));
        FillRect(hdc, &r, b);
        let _ = DeleteObject(b.into());
    }
}

unsafe fn frame(hdc: HDC, r: RECT, t: i32, hex: u32) {
    unsafe {
        fill(
            hdc,
            RECT {
                bottom: r.top + t,
                ..r
            },
            hex,
        );
        fill(
            hdc,
            RECT {
                top: r.bottom - t,
                ..r
            },
            hex,
        );
        fill(
            hdc,
            RECT {
                right: r.left + t,
                ..r
            },
            hex,
        );
        fill(
            hdc,
            RECT {
                left: r.right - t,
                ..r
            },
            hex,
        );
    }
}

unsafe fn rounded(hdc: HDC, r: RECT, radius: i32, fill_hex: u32, border: u32) {
    unsafe {
        let brush = CreateSolidBrush(color(fill_hex));
        let pen = CreatePen(PS_SOLID, 1, color(border));
        let ob = SelectObject(hdc, brush.into());
        let op = SelectObject(hdc, pen.into());
        let _ = RoundRect(hdc, r.left, r.top, r.right, r.bottom, radius, radius);
        SelectObject(hdc, ob);
        SelectObject(hdc, op);
        let _ = DeleteObject(brush.into());
        let _ = DeleteObject(pen.into());
    }
}

unsafe fn text_size(hdc: HDC, text: &str) -> (i32, i32) {
    let mut buf: Vec<u16> = text.encode_utf16().collect();
    let mut r = RECT::default();
    unsafe { DrawTextW(hdc, &mut buf, &mut r, DT_CALCRECT | DT_SINGLELINE) };
    (r.right - r.left, r.bottom - r.top)
}

unsafe fn text(hdc: HDC, s: &str, mut r: RECT) {
    let mut buf: Vec<u16> = s.encode_utf16().collect();
    unsafe {
        DrawTextW(
            hdc,
            &mut buf,
            &mut r,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        )
    };
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

fn paint(hwnd: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        with(|s| draw(s, hdc, ps.rcPaint));
        let _ = EndPaint(hwnd, &ps);
    }
}

unsafe fn draw(s: &mut State, hdc: HDC, clip: RECT) {
    unsafe {
        let back = CreateCompatibleDC(Some(hdc));
        let src = CreateCompatibleDC(Some(hdc));
        let old_back = SelectObject(back, s.back.into());
        let old_src = SelectObject(src, s.dim.into());
        blit(back, src, clip);

        s.buttons.clear();
        let mut decor = RECT::default();
        let hovering = s.sel.rect().is_none() && !s.sel.is_dragging();
        let shown = s
            .sel
            .rect()
            .or_else(|| hovering.then(|| s.chain.current()).flatten());
        if let Some(r) = shown {
            SelectObject(src, s.orig.into());
            let sr = to_rect(r);
            let inter = RECT {
                left: sr.left.max(clip.left),
                top: sr.top.max(clip.top),
                right: sr.right.min(clip.right),
                bottom: sr.bottom.min(clip.bottom),
            };
            if inter.right > inter.left && inter.bottom > inter.top {
                blit(back, src, inter);
            }

            let (mon, scale) = monitor_at(
                s.origin,
                Point {
                    x: r.right() - 1,
                    y: r.bottom() - 1,
                },
            );
            let t = px(1.0, scale);
            frame(back, inflate(sr, 3 * t), t, FRAME_OUTER);
            frame(back, inflate(sr, 2 * t), 2 * t, FRAME);
            decor = inflate(sr, 3 * t);

            let idle = s.sel.confirmed() && !s.sel.is_dragging();
            if s.sel.confirmed() {
                let hs = px(8.0, scale);
                for (cx, cy) in [
                    (sr.left, sr.top),
                    (sr.right, sr.top),
                    (sr.left, sr.bottom),
                    (sr.right, sr.bottom),
                ] {
                    let h = RECT {
                        left: cx - hs / 2,
                        top: cy - hs / 2,
                        right: cx + hs / 2,
                        bottom: cy + hs / 2,
                    };
                    fill(back, inflate(h, t), FRAME);
                    fill(back, h, ACCENT);
                    decor = union(decor, inflate(h, t));
                }
            }

            let font = CreateFontW(
                -px(13.0, scale),
                0,
                0,
                0,
                FW_SEMIBOLD.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                0,
                w!("Segoe UI"),
            );
            let old_font = SelectObject(back, font.into());
            SetBkMode(back, TRANSPARENT);
            SetTextColor(back, color(TEXT));

            let label = format!("{} × {}", r.width, r.height);
            let (lw, lh) = text_size(back, &label);
            let (pw, ph, gap) = (px(8.0, scale), px(4.0, scale), px(8.0, scale));
            let (bw, bh) = (lw + 2 * pw, lh + 2 * ph);
            let ly = if sr.top - gap - bh >= mon.top {
                sr.top - gap - bh
            } else {
                sr.top + gap
            };
            let lx = if sr.top - gap - bh >= mon.top {
                sr.left
            } else {
                sr.left + gap
            };
            let lr = RECT {
                left: lx,
                top: ly,
                right: lx + bw,
                bottom: ly + bh,
            };
            rounded(back, lr, px(6.0, scale), SURFACE, BORDER);
            text(back, &label, lr);
            decor = union(decor, lr);

            if idle {
                let items = [
                    ("Копировать", Action::Copy),
                    ("Сохранить", Action::Save),
                    ("Редактор", Action::Edit),
                    ("Текст", Action::Text),
                    ("Отмена", Action::Cancel),
                ];
                let (bp, bhh, bgap, pad) = (
                    px(14.0, scale),
                    px(32.0, scale),
                    px(6.0, scale),
                    px(6.0, scale),
                );
                let widths: Vec<i32> = items
                    .iter()
                    .map(|(t, _)| text_size(back, t).0 + 2 * bp)
                    .collect();
                let tw = widths.iter().sum::<i32>() + bgap * (items.len() as i32 - 1) + 2 * pad;
                let th = bhh + 2 * pad;
                let (pos, _) = toolbar_position(r, mon, tw, th, px(12.0, scale));
                let bar = RECT {
                    left: pos.x,
                    top: pos.y,
                    right: pos.x + tw,
                    bottom: pos.y + th,
                };
                rounded(back, bar, px(10.0, scale), SURFACE, BORDER);
                let mut x = bar.left + pad;
                for (i, ((label, action), bw)) in items.iter().zip(&widths).enumerate() {
                    let br = RECT {
                        left: x,
                        top: bar.top + pad,
                        right: x + bw,
                        bottom: bar.top + pad + bhh,
                    };
                    let hover = s.hover_btn == Some(i);
                    let bg = match (action, hover) {
                        (Action::Copy, false) => ACCENT,
                        (Action::Copy, true) => ACCENT_HOVER,
                        (_, false) => SURFACE_ALT,
                        (_, true) => BORDER,
                    };
                    rounded(back, br, px(6.0, scale), bg, bg);
                    text(back, label, br);
                    s.buttons.push((br, *action));
                    x += bw + bgap;
                }
                decor = union(decor, bar);
            }
            SelectObject(back, old_font);
            let _ = DeleteObject(font.into());
        }
        s.decor = decor;

        blit(hdc, back, clip);
        SelectObject(back, old_back);
        SelectObject(src, old_src);
        let _ = DeleteDC(back);
        let _ = DeleteDC(src);
    }
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

fn button_at(s: &State, p: Point) -> Option<usize> {
    s.buttons.iter().position(|(r, _)| in_rect(r, p))
}

fn act(hwnd: HWND, a: Action) {
    let out = match a {
        Action::Copy => Some(Outcome::Copy),
        Action::Save => Some(Outcome::Save),
        Action::Edit => Some(Outcome::Edit),
        Action::Text => Some(Outcome::Text),
        Action::Cancel => None,
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
                if let Some(i) = button_at(s, p) {
                    return Some(s.buttons[i].1);
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
