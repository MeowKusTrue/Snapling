//! What shows during a recording or a long shot: a frame around the area and a pill with the
//! time. Both stay out of the capture; a click on the pill stops the recording.

use crate::toast::{blit, under_cursor};
use snapling_core::i18n::{t, tf};
use snapling_core::selection::Rect;
use snapling_scene::{PILL_SIZE, draw_pill, draw_rec_frame};
use std::cell::Cell;
use std::time::Instant;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, HWND_TOPMOST, KillTimer, MA_NOACTIVATE, RegisterClassW,
    SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SetTimer, SetWindowDisplayAffinity, SetWindowPos,
    ShowWindow, WDA_EXCLUDEFROMCAPTURE, WINDOW_EX_STYLE, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_TIMER,
    WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
    WS_POPUP,
};
use windows::core::w;

/// Frame thickness outside the area, so it never lands in a frame.
const EDGE: i32 = 4;

thread_local! {
    static FRAME: Cell<HWND> = Cell::new(HWND::default());
    static PILL: Cell<HWND> = Cell::new(HWND::default());
    static STOP: Cell<bool> = const { Cell::new(false) };
    static SCROLLING: Cell<bool> = const { Cell::new(false) };
    static SCALE: Cell<f32> = const { Cell::new(1.0) };
    static BEGIN: Cell<Option<Instant>> = const { Cell::new(None) };
}

fn window(style: WINDOW_EX_STYLE) -> Result<HWND, String> {
    unsafe {
        let hinst = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | style,
            w!("SnaplingRecording"),
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
        let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
        Ok(hwnd)
    }
}

pub fn init() -> Result<(), String> {
    unsafe {
        let hinst = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: w!("SnaplingRecording"),
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            return Err("RegisterClassW".into());
        }
    }
    FRAME.set(window(WS_EX_TRANSPARENT)?);
    PILL.set(window(WINDOW_EX_STYLE(0))?);
    Ok(())
}

fn label() -> String {
    if SCROLLING.get() {
        return t("Прокрутка · Esc — стоп").to_string();
    }
    let secs = BEGIN.get().map_or(0, |b| b.elapsed().as_secs());
    tf(
        "● {0}:{1} · Стоп",
        &[("0", &(secs / 60)), ("1", &format!("{:02}", secs % 60))],
    )
}

fn render_pill() {
    let hwnd = PILL.get();
    let s = SCALE.get();
    let (w, h) = ((PILL_SIZE.0 * s) as u32, (PILL_SIZE.1 * s) as u32);
    blit(hwnd, w, h, |pm| draw_pill(pm, &label(), s));
}

/// Marks `area` (desktop pixels). `scrolling`: a long shot, nothing to click.
pub fn show(area: Rect, scrolling: bool) {
    let (frame, pill) = (FRAME.get(), PILL.get());
    if frame.is_invalid() || pill.is_invalid() {
        return;
    }
    let (_, work, s) = under_cursor();
    STOP.set(false);
    SCROLLING.set(scrolling);
    SCALE.set(s);
    BEGIN.set(Some(Instant::now()));
    let (fw, fh) = (area.width + EDGE * 2, area.height + EDGE * 2);
    let (pw, ph) = ((PILL_SIZE.0 * s) as i32, (PILL_SIZE.1 * s) as i32);
    let px = (area.left + (area.width - pw) / 2).max(work.left);
    let above = area.top - ph - EDGE * 3;
    let py = if above >= work.top {
        above
    } else {
        area.bottom() + EDGE * 3
    };
    let py = py.min(work.bottom - ph);
    unsafe {
        let place = |h: HWND, x, y, w, ht| {
            let _ = SetWindowPos(h, Some(HWND_TOPMOST), x, y, w, ht, SWP_NOACTIVATE);
        };
        place(frame, area.left - EDGE, area.top - EDGE, fw, fh);
        place(pill, px, py, pw, ph);
    }
    blit(frame, fw as u32, fh as u32, |pm| draw_rec_frame(pm, s));
    render_pill();
    unsafe {
        let _ = ShowWindow(frame, SW_SHOWNOACTIVATE);
        let _ = ShowWindow(pill, SW_SHOWNOACTIVATE);
        SetTimer(Some(pill), 1, 500, None);
    }
}

pub fn hide() {
    let (frame, pill) = (FRAME.get(), PILL.get());
    unsafe {
        let _ = KillTimer(Some(pill), 1);
        let _ = ShowWindow(frame, SW_HIDE);
        let _ = ShowWindow(pill, SW_HIDE);
    }
}

/// The pill was clicked since the last call.
pub fn stop_asked() -> bool {
    STOP.take()
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_TIMER => render_pill(),
        WM_LBUTTONUP => STOP.set(true),
        _ => return unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
    LRESULT(0)
}
