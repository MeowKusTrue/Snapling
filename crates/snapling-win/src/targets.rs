//! Snapshot of hover targets at capture time: visible top-level windows in z-order and monitors.

use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GetClassNameW, GetLayeredWindowAttributes, GetWindowLongPtrW,
    GetWindowRect, IsIconic, IsWindowVisible, LAYERED_WINDOW_ATTRIBUTES_FLAGS, LWA_ALPHA,
    WS_EX_LAYERED, WS_EX_TRANSPARENT,
};
use windows::core::BOOL;

/// Screen coordinates, topmost first.
pub fn windows() -> Vec<(isize, RECT)> {
    let mut out: Vec<(isize, RECT)> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(on_window), LPARAM(&raw mut out as isize));
    }
    out
}

pub fn monitors() -> Vec<RECT> {
    let mut out: Vec<RECT> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(on_monitor), LPARAM(&raw mut out as isize));
    }
    out
}

extern "system" fn on_monitor(_: HMONITOR, _: HDC, r: *mut RECT, out: LPARAM) -> BOOL {
    unsafe { (*(out.0 as *mut Vec<RECT>)).push(*r) };
    true.into()
}

extern "system" fn on_window(hwnd: HWND, out: LPARAM) -> BOOL {
    if let Some(r) = target_rect(hwnd) {
        unsafe { (*(out.0 as *mut Vec<(isize, RECT)>)).push((hwnd.0 as isize, r)) };
    }
    true.into()
}

/// Bounds without the invisible resize border; `None` for windows the user cannot see.
pub(crate) fn target_rect(hwnd: HWND) -> Option<RECT> {
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return None;
        }
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, (&raw mut cloaked).cast(), 4);
        if cloaked != 0 {
            return None;
        }
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        if ex & WS_EX_TRANSPARENT.0 != 0 {
            return None;
        }
        if ex & WS_EX_LAYERED.0 != 0 {
            let (mut alpha, mut flags) = (255u8, LAYERED_WINDOW_ATTRIBUTES_FLAGS(0));
            let _ = GetLayeredWindowAttributes(hwnd, None, Some(&mut alpha), Some(&mut flags));
            if flags.0 & LWA_ALPHA.0 != 0 && alpha == 0 {
                return None;
            }
        }
        let mut class = [0u16; 32];
        let n = GetClassNameW(hwnd, &mut class) as usize;
        let class = String::from_utf16_lossy(&class[..n]);
        if class == "Progman" || class == "WorkerW" {
            return None;
        }
        let mut r = RECT::default();
        let dwm = DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&raw mut r).cast(),
            size_of::<RECT>() as u32,
        );
        if dwm.is_err() {
            GetWindowRect(hwnd, &mut r).ok()?;
        }
        (r.right > r.left && r.bottom > r.top).then_some(r)
    }
}

/// Foreground window bounds (screen coordinates); `None` for the desktop or hidden windows.
pub fn foreground_window() -> Option<RECT> {
    let hwnd = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
    (!hwnd.is_invalid()).then(|| target_rect(hwnd)).flatten()
}
