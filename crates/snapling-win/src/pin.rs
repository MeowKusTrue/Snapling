//! A shot pinned over all windows, in the tray process. Dragged by any point, the wheel scales
//! it, Ctrl+C copies it; Esc, a right click or a double click closes it.

use crate::{Image, copy_image, dib_header, style_window};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BITMAPINFO, BeginPaint, DIB_RGB_COLORS, EndPaint, HALFTONE, InvalidateRect, PAINTSTRUCT,
    SRCCOPY, SetStretchBltMode, StretchDIBits,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, HTCAPTION,
    HWND_TOPMOST, IDC_SIZEALL, LoadCursorW, MSG, PostQuitMessage, RegisterClassW, SW_SHOW,
    SWP_NOMOVE, SetWindowPos, ShowWindow, TranslateMessage, WM_DESTROY, WM_KEYDOWN, WM_MOUSEWHEEL,
    WM_NCHITTEST, WM_NCLBUTTONDBLCLK, WM_NCRBUTTONDOWN, WM_PAINT, WNDCLASSW, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::w;

const MIN_SCALE: f32 = 0.2;
const MAX_SCALE: f32 = 4.0;
const WHEEL_STEP: f32 = 1.1;

struct Pin {
    img: Image,
    scale: f32,
}

thread_local! {
    static PINS: RefCell<HashMap<isize, Pin>> = RefCell::new(HashMap::new());
    static REGISTERED: Cell<bool> = const { Cell::new(false) };
    /// A process of its own: it ends with its last pin.
    static ALONE: Cell<bool> = const { Cell::new(false) };
}

/// Scale after one wheel step.
fn scaled(scale: f32, up: bool) -> f32 {
    let k = if up { WHEEL_STEP } else { 1.0 / WHEEL_STEP };
    (scale * k).clamp(MIN_SCALE, MAX_SCALE)
}

pub fn show(mut img: Image) -> Result<(), String> {
    // Outside a freeform selection the pin shows white, as the clipboard does.
    for px in img.bgra.as_chunks_mut::<4>().0 {
        if px[3] == 0 {
            *px = [255; 4];
        }
    }
    unsafe {
        let hinst = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        let class = w!("SnaplingPin");
        if !REGISTERED.replace(true) {
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: hinst.into(),
                lpszClassName: class,
                hCursor: LoadCursorW(None, IDC_SIZEALL).unwrap_or_default(),
                ..Default::default()
            };
            if RegisterClassW(&wc) == 0 {
                return Err("RegisterClassW".into());
            }
        }
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class,
            w!("Snapling"),
            WS_POPUP,
            img.x,
            img.y,
            img.width as i32,
            img.height as i32,
            None,
            None,
            Some(hinst.into()),
            None,
        )
        .map_err(|e| format!("CreateWindowExW: {e}"))?;
        PINS.with(|p| {
            p.borrow_mut()
                .insert(hwnd.0 as isize, Pin { img, scale: 1.0 })
        });
        style_window(hwnd.0 as isize);
        let _ = ShowWindow(hwnd, SW_SHOW);
    }
    Ok(())
}

/// Pins `img` and keeps this process alive until the pin is closed.
pub fn run_alone(img: Image) -> Result<(), String> {
    ALONE.set(true);
    show(img)?;
    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

fn paint(hwnd: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        PINS.with(|p| {
            let p = p.borrow();
            let Some(pin) = p.get(&(hwnd.0 as isize)) else {
                return;
            };
            let (w, h) = (pin.img.width as i32, pin.img.height as i32);
            let bmi = BITMAPINFO {
                bmiHeader: dib_header(pin.img.width, -h),
                ..Default::default()
            };
            SetStretchBltMode(hdc, HALFTONE);
            StretchDIBits(
                hdc,
                0,
                0,
                (w as f32 * pin.scale) as i32,
                (h as f32 * pin.scale) as i32,
                0,
                0,
                w,
                h,
                Some(pin.img.bgra.as_ptr().cast()),
                &bmi,
                DIB_RGB_COLORS,
                SRCCOPY,
            );
        });
        let _ = EndPaint(hwnd, &ps);
    }
}

fn wheel(hwnd: HWND, up: bool) {
    let size = PINS.with(|p| {
        let mut p = p.borrow_mut();
        let pin = p.get_mut(&(hwnd.0 as isize))?;
        pin.scale = scaled(pin.scale, up);
        let side = |v: u32| (v as f32 * pin.scale) as i32;
        Some((side(pin.img.width), side(pin.img.height)))
    });
    if let Some((w, h)) = size {
        unsafe {
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, w, h, SWP_NOMOVE);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => return LRESULT(HTCAPTION as isize),
        WM_PAINT => paint(hwnd),
        WM_MOUSEWHEEL => wheel(hwnd, ((wp.0 >> 16) & 0xFFFF) as u16 as i16 > 0),
        WM_NCLBUTTONDBLCLK | WM_NCRBUTTONDOWN => {
            let _ = unsafe { DestroyWindow(hwnd) };
        }
        WM_KEYDOWN if wp.0 as u16 == VK_ESCAPE.0 => {
            let _ = unsafe { DestroyWindow(hwnd) };
        }
        WM_KEYDOWN if wp.0 == 0x43 && unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0 => {
            PINS.with(|p| {
                if let Some(pin) = p.borrow().get(&(hwnd.0 as isize)) {
                    let _ = copy_image(&pin.img);
                }
            });
        }
        WM_DESTROY => {
            let empty = PINS.with(|p| {
                let mut p = p.borrow_mut();
                p.remove(&(hwnd.0 as isize));
                p.is_empty()
            });
            if empty && ALONE.get() {
                unsafe { PostQuitMessage(0) };
            }
        }
        _ => return unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
    LRESULT(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wheel_stops_at_the_limits() {
        assert!(scaled(1.0, true) > 1.0);
        assert!(scaled(1.0, false) < 1.0);
        assert_eq!(scaled(MAX_SCALE, true), MAX_SCALE);
        assert_eq!(scaled(MIN_SCALE, false), MIN_SCALE);
    }
}
