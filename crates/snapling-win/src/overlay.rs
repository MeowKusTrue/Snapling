//! Fullscreen region-selection overlay over the virtual screen: the Win32 window and the blits.
//! Everything it does is in `snapling_scene::session`; this file feeds it events and adds what
//! only Windows knows: monitors, windows and their elements through UI Automation.

use crate::{Image, dib_header, lparam_point, targets, uia};
use snapling_core::annotate::{ArrowStyle, BlurStyle, DrawStyle, Head, StepStyle};
use snapling_core::color::ColorFormat;
use snapling_core::i18n::t;
use snapling_core::selection::{Point, Rect};
use snapling_scene::session::{Key, Mods, Options, Pointer, Session, Shot, Step};
pub use snapling_scene::session::{Outcome, Prefs};
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

type Done = Box<dyn FnOnce(Option<(Image, Outcome)>)>;

struct State {
    session: Session,
    origin: (i32, i32),
    width: u32,
    height: u32,
    back: HBITMAP,
    back_bits: *mut u8,
    /// Area of the last drawn decorations, to clear them on the next frame.
    decor: Option<Rect>,
    /// Window handles, in the order of `Options::targets`.
    windows: Vec<isize>,
    chain_seq: u64,
    prefs: Prefs,
    on_done: Option<Done>,
}

const WM_UIA: u32 = 0x8000 + 1;

thread_local! {
    static HWND_OVERLAY: Cell<HWND> = Cell::new(HWND::default());
    /// `[capture] instant_copy`.
    static INSTANT: Cell<bool> = const { Cell::new(false) };
    /// `[capture] loupe`.
    static LOUPE: Cell<bool> = const { Cell::new(true) };
    /// Color settings a new overlay starts with.
    static COLOR: Cell<(bool, ColorFormat)> = const { Cell::new((false, ColorFormat::Hex)) };
    /// `[capture] rail`.
    static RAIL: Cell<Option<Point>> = const { Cell::new(None) };
    /// Told about what the panels changed, to keep it for the next time.
    static ON_PREFS: Cell<Option<fn(Prefs)>> = const { Cell::new(None) };
    /// `[blur]`, the brush a new overlay starts with.
    static BLUR: Cell<BlurStyle> = const {
        Cell::new(BlurStyle {
            gauss: false,
            strength: 0,
        })
    };
    /// `[draw]`.
    static STYLE: Cell<DrawStyle> = const {
        Cell::new(DrawStyle {
            arrow: ArrowStyle {
                head: Head::Sharp,
                to_start: false,
            },
            free: false,
            step: StepStyle::Solid,
            font: "",
        })
    };
    static ON_STRETCH: Cell<Option<fn(usize)>> = const { Cell::new(None) };
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

/// Sends what the session wants looked up, and what changed in the panels.
fn pump(s: &mut State) {
    if let Some(ask) = s.session.take_ask() {
        let hwnd = s.windows[ask.window];
        if ask.entered {
            s.session.set_app(crate::app_name_of(hwnd));
        }
        s.chain_seq += 1;
        let mouse = s.session.mouse();
        let req = uia::Request {
            seq: s.chain_seq,
            hwnd,
            point: POINT {
                x: mouse.x + s.origin.0,
                y: mouse.y + s.origin.1,
            },
        };
        UIA.with(|u| u.borrow().as_ref().map(|tx| tx.send(req)));
    }
    let prefs = s.session.prefs();
    if prefs != s.prefs {
        s.prefs = prefs;
        if let Some(sink) = ON_PREFS.get() {
            sink(prefs);
        }
    }
}

/// Merges a finished UIA chain if it still belongs to the hovered window.
fn apply_uia() -> bool {
    let Some((hwnd, rects)) = uia::RESULT.lock().ok().and_then(|mut r| r.take()) else {
        return false;
    };
    with(|s| {
        let Some(window) = s.windows.iter().position(|w| *w == hwnd) else {
            return false;
        };
        let origin = s.origin;
        let rects = rects.into_iter().map(|r| rect_from(r, origin)).collect();
        s.session.add_levels(window, rects)
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
    let (back, back_bits) = dib(img.width, img.height)?;
    let (x, y, w, h) = (img.x, img.y, img.width as i32, img.height as i32);
    let (windows, rects): (Vec<isize>, Vec<(Rect, String)>) = targets::windows()
        .into_iter()
        .map(|(id, r)| (id, (rect_from(r, (x, y)), String::new())))
        .unzip();
    let monitors = targets::monitors()
        .into_iter()
        .map(|r| {
            let r = rect_from(r, (x, y));
            let center = Point {
                x: r.left + r.width / 2,
                y: r.top + r.height / 2,
            };
            (r, monitor_scale((x, y), center))
        })
        .collect();
    let mut cursor = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut cursor);
    }
    let (picker, color_format) = COLOR.get();
    let opts = Options {
        scale: 1.0,
        auto,
        instant: INSTANT.get(),
        loupe: LOUPE.get(),
        picker,
        color_format,
        blur: BLUR.get(),
        style: STYLE.get(),
        on_stretch: ON_STRETCH.get(),
        targets: rects,
        monitors,
        mouse: Point {
            x: cursor.x - x,
            y: cursor.y - y,
        },
        rail_at: RAIL.get(),
    };
    let shot = Shot {
        x,
        y,
        width: img.width,
        height: img.height,
        bgra: img.bgra,
    };
    let session = Session::new(shot, opts);
    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            prefs: session.prefs(),
            session,
            origin: (x, y),
            width: img.width,
            height: img.height,
            back,
            back_bits,
            decor: None,
            windows,
            chain_seq: 0,
            on_done: Some(Box::new(on_done)),
        })
    });
    with(pump);
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
    let Some(mut s) = STATE.with(|st| st.borrow_mut().take()) else {
        return;
    };
    snapling_scene::drop_cache();
    let result = out.and_then(|out| {
        let shot = s.session.result(out)?;
        let img = Image {
            x: shot.x,
            y: shot.y,
            width: shot.width,
            height: shot.height,
            bgra: shot.bgra,
        };
        Some((img, out))
    });
    unsafe {
        let _ = ReleaseCapture();
        let _ = ShowWindow(hwnd, SW_HIDE);
        let _ = DeleteObject(s.back.into());
    }
    if let Some(cb) = s.on_done {
        cb(result);
    }
}

/// What a step of the session asks of the window; `true`: the overlay goes on.
fn step(hwnd: HWND, step: Step) -> bool {
    match step {
        Step::Stay => return true,
        Step::Cancel => finish(hwnd, None),
        Step::Done(out) => finish(hwnd, Some(out)),
    }
    false
}

/// DPI scale of the monitor at `p`.
fn monitor_scale(origin: (i32, i32), p: Point) -> f32 {
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
        dx as f32 / 96.0
    }
}

fn to_rect(r: Rect) -> RECT {
    RECT {
        left: r.left,
        top: r.top,
        right: r.right(),
        bottom: r.bottom(),
    }
}

fn from_rect(r: RECT) -> Rect {
    rect_from(r, (0, 0))
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

/// Repaints the old and the new decoration areas only (`full`: everything).
// ponytail: a full blit per mouse move while drawing; track the shape's box if it ever lags.
fn invalidate(hwnd: HWND, full: bool) {
    let areas = with(|s| {
        let wide = s.session.repaint_all();
        (s.decor, s.session.scene_bounds(), wide)
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

fn paint(hwnd: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        with(|s| draw(s, hdc, ps.rcPaint));
        let _ = EndPaint(hwnd, &ps);
    }
}

/// The session draws into the back buffer, the changed part goes to the screen.
unsafe fn draw(s: &mut State, hdc: HDC, clip: RECT) {
    let len = s.width as usize * s.height as usize * 4;
    unsafe {
        let _ = GdiFlush();
        let bits = std::slice::from_raw_parts_mut(s.back_bits, len);
        let area = to_rect(s.session.render_area(bits, from_rect(clip)));
        let r = union(clip, area);
        let back = CreateCompatibleDC(Some(hdc));
        let old = SelectObject(back, s.back.into());
        let _ = BitBlt(
            hdc,
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            Some(back),
            r.left,
            r.top,
            SRCCOPY,
        );
        SelectObject(back, old);
        let _ = DeleteDC(back);
    }
    s.decor = s.session.drawn_bounds();
}

fn mods() -> Mods {
    let down = |k: u16| unsafe { GetKeyState(k as i32) < 0 };
    Mods {
        shift: down(VK_SHIFT.0),
        ctrl: down(VK_CONTROL.0),
    }
}

fn set_cursor() {
    let id = with(|s| match s.session.cursor() {
        Pointer::Cross => IDC_CROSS,
        Pointer::Hand => IDC_HAND,
        Pointer::Move => IDC_SIZEALL,
        Pointer::Text => IDC_IBEAM,
        Pointer::ResizeNwSe => IDC_SIZENWSE,
        Pointer::ResizeNeSw => IDC_SIZENESW,
    })
    .unwrap_or(IDC_CROSS);
    unsafe {
        if let Ok(c) = LoadCursorW(None, PCWSTR(id.0)) {
            SetCursor(Some(c));
        }
    }
}

/// The key by its place, so shortcuts work on any keyboard layout. Typed text comes by `WM_CHAR`.
fn key_of(vk: u16) -> Option<Key> {
    match vk {
        v if v == VK_ESCAPE.0 => Some(Key::Escape),
        v if v == VK_RETURN.0 => Some(Key::Enter),
        v if v == VK_BACK.0 => Some(Key::Backspace),
        v if v == VK_LEFT.0 => Some(Key::Left),
        v if v == VK_RIGHT.0 => Some(Key::Right),
        v if v == VK_UP.0 => Some(Key::Up),
        v if v == VK_DOWN.0 => Some(Key::Down),
        0x30..=0x39 | 0x41..=0x5A => Some(Key::Char((vk as u8 as char).to_ascii_lowercase())),
        _ => None,
    }
}

fn key_down(hwnd: HWND, vk: u16) {
    let typing = with(|s| s.session.typing()) == Some(true);
    let key = key_of(vk).filter(|k| !typing || !matches!(k, Key::Char(_)));
    let Some(key) = key else { return };
    let m = mods();
    let Some(done) = with(|s| s.session.key(key, m)) else {
        return;
    };
    if step(hwnd, done) {
        invalidate(hwnd, true);
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let res = handle(hwnd, msg, wp, lp);
    if matches!(
        msg,
        WM_LBUTTONDOWN | WM_MOUSEMOVE | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_KEYDOWN
    ) {
        with(pump);
    }
    res
}

fn handle(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => paint(hwnd),
        WM_ERASEBKGND => return LRESULT(1),
        WM_SETCURSOR => {
            set_cursor();
            return LRESULT(1);
        }
        WM_LBUTTONDOWN => {
            let (p, m) = (lparam_point(lp), mods());
            if let Some(done) = with(|s| s.session.press(p, m))
                && step(hwnd, done)
            {
                unsafe { SetCapture(hwnd) };
                invalidate(hwnd, false);
            }
        }
        WM_MOUSEMOVE => {
            let (p, m) = (lparam_point(lp), mods());
            if with(|s| s.session.moved(p, m)) == Some(true) {
                invalidate(hwnd, false);
            }
        }
        WM_LBUTTONUP => {
            let done = with(|s| s.session.release());
            unsafe {
                let _ = ReleaseCapture();
            }
            if let Some(done) = done
                && step(hwnd, done)
            {
                invalidate(hwnd, true);
            }
        }
        WM_LBUTTONDBLCLK => {
            let p = lparam_point(lp);
            if let Some(done) = with(|s| s.session.double_click(p)) {
                step(hwnd, done);
            }
        }
        // The first press takes the selection back, the next one closes.
        WM_RBUTTONDOWN | WM_MBUTTONDOWN => {
            if let Some(done) = with(|s| s.session.cancel_click())
                && step(hwnd, done)
            {
                unsafe {
                    let _ = ReleaseCapture();
                }
                invalidate(hwnd, true);
            }
        }
        WM_MOUSEWHEEL => {
            let outward = ((wp.0 >> 16) & 0xFFFF) as u16 as i16 > 0;
            if with(|s| s.session.scroll(outward)) == Some(true) {
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
            let typed = ch.and_then(|c| {
                with(|s| {
                    let on = s.session.typing();
                    if on {
                        s.session.key(Key::Char(c), Mods::default());
                    }
                    on
                })
            });
            if typed == Some(true) {
                invalidate(hwnd, false);
            }
        }
        WM_KEYDOWN | WM_KEYUP => {
            let down = msg == WM_KEYDOWN;
            let vk = wp.0 as u16;
            if vk == VK_SPACE.0 {
                with(|s| s.session.set_pan(down));
            } else if vk == VK_SHIFT.0 {
                with(|s| s.session.shift(down));
                invalidate(hwnd, false);
            } else if down {
                key_down(hwnd, vk);
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

/// Look of the shapes a new overlay draws.
pub fn set_style(style: DrawStyle) {
    STYLE.set(style);
}

/// Plays the sound of a selection growing, `None`: silent.
pub fn set_stretch(play: Option<fn(usize)>) {
    ON_STRETCH.set(play);
}

/// Eyedropper and color format a new overlay starts with.
pub fn set_color(picker: bool, format: ColorFormat) {
    COLOR.set((picker, format));
}

/// Where the tool rail opens, on the desktop.
pub fn set_rail(at: Option<Point>) {
    RAIL.set(at);
}

/// Called with the panel settings whenever they change.
pub fn on_prefs(sink: fn(Prefs)) {
    ON_PREFS.set(Some(sink));
}

/// Format of a picked color.
pub fn color_format() -> ColorFormat {
    COLOR.get().1
}

/// Magnifier at the cursor while aiming.
pub fn set_loupe(on: bool) {
    LOUPE.set(on);
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
    /// `cargo test --release -p snapling-win bench_copy -- --ignored --nocapture`: what a shot
    /// costs on the way to the clipboard, the clipboard call itself excluded.
    #[test]
    #[ignore]
    fn bench_copy() {
        let (w, h) = (1920u32, 1080u32);
        let mut bgra = vec![0u8; (w * h * 4) as usize];
        for (i, px) in bgra.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let (x, y) = (i as u32 % w, i as u32 / w);
            let v = ((x * 7 + y * 13) ^ (x * y / 64)) as u8;
            *px = [v, v / 2, 255 - v, 255];
        }
        let t0 = std::time::Instant::now();
        let mut flipped = vec![0u8; bgra.len()];
        for (out, src) in flipped
            .chunks_exact_mut(w as usize * 4)
            .zip(bgra.chunks_exact(w as usize * 4).rev())
        {
            out.copy_from_slice(src);
        }
        let dib = t0.elapsed();
        let t0 = std::time::Instant::now();
        let png = snapling_core::storage::encode_png(&bgra, w, h).unwrap();
        println!(
            "dib {dib:?}, png {:?} ({} KB)",
            t0.elapsed(),
            png.len() / 1024
        );
        assert_ne!(std::hint::black_box(&flipped)[0], 1);
    }
}
