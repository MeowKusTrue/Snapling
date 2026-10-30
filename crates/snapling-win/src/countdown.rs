//! Seconds left before a delayed shot: a small click-through mark, never in the shot itself.

use crate::toast::{blit, helper_window, under_cursor};
use snapling_scene::{COUNT_SIZE, draw_count};
use std::cell::Cell;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, HWND_TOPMOST, KillTimer, PostThreadMessageW, SW_HIDE, SW_SHOWNOACTIVATE,
    SWP_NOACTIVATE, SetTimer, SetWindowDisplayAffinity, SetWindowPos, ShowWindow,
    WDA_EXCLUDEFROMCAPTURE, WM_HOTKEY, WM_TIMER, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT,
};
use windows::core::w;

thread_local! {
    static HWND_COUNT: Cell<HWND> = Cell::new(HWND::default());
    static LEFT: Cell<u32> = const { Cell::new(0) };
    static HOTKEY: Cell<i32> = const { Cell::new(0) };
    static SCALE: Cell<f32> = const { Cell::new(1.0) };
}

pub fn init() -> Result<(), String> {
    let ex =
        WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT;
    let hwnd = helper_window(w!("SnaplingCountdown"), Some(wndproc), ex)?;
    let _ = unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) };
    HWND_COUNT.set(hwnd);
    Ok(())
}

fn render(hwnd: HWND) {
    let (scale, left) = (SCALE.get(), LEFT.get());
    let side = (COUNT_SIZE * scale).ceil() as u32;
    blit(hwnd, side, side, |pm| draw_count(pm, left, scale));
}

/// Shows `seconds`, then sends this thread the hotkey `id`. A running count stays as it is.
pub fn start(seconds: u32, id: i32) {
    let hwnd = HWND_COUNT.get();
    if hwnd.is_invalid() || LEFT.get() > 0 {
        return;
    }
    let (_, work, scale) = under_cursor();
    let side = (COUNT_SIZE * scale).ceil() as i32;
    let x = (work.left + work.right - side) / 2;
    let y = work.top + (48.0 * scale) as i32;
    LEFT.set(seconds.max(1));
    HOTKEY.set(id);
    SCALE.set(scale);
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, side, side, SWP_NOACTIVATE);
    }
    render(hwnd);
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetTimer(Some(hwnd), 1, 1000, None);
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg != WM_TIMER {
        return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
    }
    let left = LEFT.get().saturating_sub(1);
    LEFT.set(left);
    if left > 0 {
        render(hwnd);
        return LRESULT(0);
    }
    unsafe {
        let _ = KillTimer(Some(hwnd), 1);
        let _ = ShowWindow(hwnd, SW_HIDE);
        let id = WPARAM(HOTKEY.get() as usize);
        let _ = PostThreadMessageW(GetCurrentThreadId(), WM_HOTKEY, id, LPARAM(0));
    }
    LRESULT(0)
}
