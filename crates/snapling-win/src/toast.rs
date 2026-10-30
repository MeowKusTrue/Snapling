//! Notification after a shot, `assets/design/Toast.dc.html`: a layered window in the tray process.
//! It never takes focus and leaves by itself; drawing comes from `snapling-scene`.

use crate::{Image, dib_header};
use snapling_scene::{Toast, ToastAction, draw_toast, toast_at, toast_size};
use std::cell::{Cell, RefCell};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BLENDFUNCTION, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromPoint, SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetCursorPos, GetWindowRect, HWND_TOPMOST, IDC_ARROW,
    KillTimer, LoadCursorW, MA_NOACTIVATE, RegisterClassW, SW_HIDE, SW_SHOWNOACTIVATE,
    SWP_NOACTIVATE, SetTimer, SetWindowDisplayAffinity, SetWindowPos, ShowWindow, ULW_ALPHA,
    UpdateLayeredWindow, WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WM_LBUTTONUP, WM_MOUSEACTIVATE,
    WM_MOUSEMOVE, WM_TIMER, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, w};

const LIFE_MS: u32 = 5000;
const MARGIN: f32 = 16.0;

type OnClick = Box<dyn FnOnce(ToastAction, Image)>;

struct State {
    img: Image,
    title: String,
    sub: String,
    scale: f32,
    hover: Option<ToastAction>,
    on_click: OnClick,
}

thread_local! {
    static HWND_TOAST: Cell<HWND> = Cell::new(HWND::default());
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// A hidden popup of a class of its own, for the small windows of the tray process.
pub(crate) fn helper_window(
    class: PCWSTR,
    proc: windows::Win32::UI::WindowsAndMessaging::WNDPROC,
    ex: windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE,
) -> Result<HWND, String> {
    unsafe {
        let hinst = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        let wc = WNDCLASSW {
            lpfnWndProc: proc,
            hInstance: hinst.into(),
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            return Err("RegisterClassW".into());
        }
        CreateWindowExW(
            ex,
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
        .map_err(|e| format!("CreateWindowExW: {e}"))
    }
}

pub fn init() -> Result<(), String> {
    let ex = WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
    HWND_TOAST.set(helper_window(w!("SnaplingToast"), Some(wndproc), ex)?);
    Ok(())
}

/// Cursor, work area of the monitor under it, and that monitor's scale.
pub(crate) fn under_cursor() -> (POINT, RECT, f32) {
    let mut cursor = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut cursor);
        let mon = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let (mut dx, mut dy) = (96, 96);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        (cursor, mi.rcWork, dx as f32 / 96.0)
    }
}

/// Bottom right of the work area of the monitor under the cursor, and that monitor's scale.
fn place() -> (i32, i32, f32) {
    let (_, r, scale) = under_cursor();
    let (w, h) = toast_size(scale);
    let gap = (MARGIN * scale) as i32;
    (r.right - w as i32 - gap, r.bottom - h as i32 - gap, scale)
}

pub fn show(img: Image, title: String, sub: String, on_click: OnClick) {
    let hwnd = HWND_TOAST.get();
    if hwnd.is_invalid() {
        return;
    }
    let (x, y, scale) = place();
    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            img,
            title,
            sub,
            scale,
            hover: None,
            on_click,
        })
    });
    let (w, h) = toast_size(scale);
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            x,
            y,
            w as i32,
            h as i32,
            SWP_NOACTIVATE,
        );
    }
    render(hwnd);
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetTimer(Some(hwnd), 1, LIFE_MS, None);
    }
}

fn hide(hwnd: HWND) {
    unsafe {
        let _ = KillTimer(Some(hwnd), 1);
        let _ = ShowWindow(hwnd, SW_HIDE);
    }
    STATE.with(|s| *s.borrow_mut() = None);
}

/// Puts a freshly drawn transparent buffer on a layered window.
pub(crate) fn blit(hwnd: HWND, w: u32, h: u32, draw: impl FnOnce(&mut tiny_skia::PixmapMut<'_>)) {
    let bmi = BITMAPINFO {
        bmiHeader: dib_header(w, -(h as i32)),
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    unsafe {
        let Ok(bmp) = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
            return;
        };
        let buf = std::slice::from_raw_parts_mut(bits.cast::<u8>(), (w * h * 4) as usize);
        if let Some(mut pm) = tiny_skia::PixmapMut::from_bytes(buf, w, h) {
            draw(&mut pm);
        }
        let dc = CreateCompatibleDC(None);
        let old = SelectObject(dc, bmp.into());
        let size = SIZE {
            cx: w as i32,
            cy: h as i32,
        };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
            ..Default::default()
        };
        let _ = UpdateLayeredWindow(
            hwnd,
            None,
            None,
            Some(&size),
            Some(dc),
            Some(&POINT::default()),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );
        SelectObject(dc, old);
        let _ = DeleteDC(dc);
        let _ = DeleteObject(bmp.into());
    }
}

fn render(hwnd: HWND) {
    STATE.with(|st| {
        let st = st.borrow();
        let Some(s) = st.as_ref() else { return };
        let (w, h) = toast_size(s.scale);
        blit(hwnd, w, h, |pm| {
            let toast = Toast {
                scale: s.scale,
                title: &s.title,
                sub: &s.sub,
                shot: (&s.img.bgra, s.img.width, s.img.height),
                hover: s.hover,
            };
            draw_toast(pm, &toast);
        });
    });
}

fn action_at(lp: LPARAM) -> Option<ToastAction> {
    let p = crate::lparam_point(lp);
    let scale = STATE.with(|s| s.borrow().as_ref().map(|s| s.scale))?;
    toast_at(scale, p)
}

fn cursor_inside(hwnd: HWND) -> bool {
    let (mut p, mut r) = (POINT::default(), RECT::default());
    unsafe {
        let _ = GetCursorPos(&mut p);
        let _ = GetWindowRect(hwnd, &mut r);
    }
    p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE => {
            let hover = action_at(lp);
            let changed = STATE.with(|s| {
                s.borrow_mut()
                    .as_mut()
                    .is_some_and(|s| std::mem::replace(&mut s.hover, hover) != hover)
            });
            if changed {
                render(hwnd);
            }
        }
        // Stays while the pointer is on it, so a button is never pulled away.
        WM_TIMER if !cursor_inside(hwnd) => hide(hwnd),
        WM_TIMER => {}
        WM_LBUTTONUP => {
            let action = action_at(lp);
            let taken = STATE.with(|s| s.borrow_mut().take());
            unsafe {
                let _ = KillTimer(Some(hwnd), 1);
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            if let (Some(a), Some(s)) = (action, taken) {
                (s.on_click)(a, s.img);
            }
        }
        _ => return unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
    LRESULT(0)
}

/// Same rule as the overlay: not visible in screen sharing when asked.
pub fn set_hidden_from_capture(hide: bool) {
    let mode = if hide {
        WDA_EXCLUDEFROMCAPTURE
    } else {
        WDA_NONE
    };
    let _ = unsafe { SetWindowDisplayAffinity(HWND_TOAST.get(), mode) };
}
