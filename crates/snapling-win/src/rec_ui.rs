//! What shows during a recording or a long shot: a frame around the area and a pill with the
//! time. Both stay out of the capture; a click on the pill stops the recording.

use crate::lparam_point;
use crate::toast::{blit, helper_window, under_cursor};
use snapling_core::i18n::{t, tf};
use snapling_core::selection::Rect;
use snapling_scene::{PILL_SIZE, draw_pill, draw_rec_frame, pill_button};
use std::cell::Cell;
use std::time::Instant;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, HWND_TOPMOST, KillTimer, MA_NOACTIVATE, SW_HIDE, SW_SHOWNOACTIVATE,
    SWP_NOACTIVATE, SetTimer, SetWindowDisplayAffinity, SetWindowPos, ShowWindow,
    WDA_EXCLUDEFROMCAPTURE, WINDOW_EX_STYLE, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_TIMER,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
};
use windows::core::{PCWSTR, w};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    Stop,
    Pause,
    Cancel,
}

/// Frame thickness outside the area, so it never lands in a frame.
const EDGE: i32 = 4;

thread_local! {
    static FRAME: Cell<HWND> = Cell::new(HWND::default());
    static PILL: Cell<HWND> = Cell::new(HWND::default());
    static ASKED: Cell<Option<Ask>> = const { Cell::new(None) };
    static SCROLLING: Cell<bool> = const { Cell::new(false) };
    static SCALE: Cell<f32> = const { Cell::new(1.0) };
    static BEGIN: Cell<Option<Instant>> = const { Cell::new(None) };
    /// Since when the recording is paused.
    static PAUSED: Cell<Option<Instant>> = const { Cell::new(None) };
}

fn window(class: PCWSTR, extra: WINDOW_EX_STYLE) -> Result<HWND, String> {
    let ex = WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | extra;
    let hwnd = helper_window(class, Some(wndproc), ex)?;
    let _ = unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) };
    Ok(hwnd)
}

pub fn init() -> Result<(), String> {
    FRAME.set(window(w!("SnaplingRecordFrame"), WS_EX_TRANSPARENT)?);
    PILL.set(window(w!("SnaplingRecordPill"), WINDOW_EX_STYLE(0))?);
    Ok(())
}

fn label() -> String {
    if SCROLLING.get() {
        return t("Прокрутка · Esc — стоп").to_string();
    }
    let secs = BEGIN.get().map_or(0, |b| match PAUSED.get() {
        Some(p) => p.duration_since(b).as_secs(),
        None => b.elapsed().as_secs(),
    });
    tf(
        "Стоп · {0}:{1}",
        &[("0", &(secs / 60)), ("1", &format!("{:02}", secs % 60))],
    )
}

fn render_pill() {
    let hwnd = PILL.get();
    let s = SCALE.get();
    let (w, h) = ((PILL_SIZE.0 * s) as u32, (PILL_SIZE.1 * s) as u32);
    let pause = if PAUSED.get().is_some() {
        t("Дальше")
    } else {
        t("Пауза")
    };
    let buttons: &[&str] = if SCROLLING.get() {
        &[]
    } else {
        &[pause, t("Отменить")]
    };
    blit(hwnd, w, h, |pm| draw_pill(pm, &label(), buttons, s));
}

/// Paused or going on again: the time stops and the button reads «Дальше».
pub fn set_paused(on: bool) {
    match (on, PAUSED.get(), BEGIN.get()) {
        (true, None, _) => PAUSED.set(Some(Instant::now())),
        // The pause is left out of the time by starting it that much later.
        (false, Some(p), Some(b)) => {
            BEGIN.set(Some(b + p.elapsed()));
            PAUSED.set(None);
        }
        _ => {}
    }
    render_pill();
}

/// Marks `area` (desktop pixels). `scrolling`: a long shot, nothing to click.
pub fn show(area: Rect, scrolling: bool) {
    let (frame, pill) = (FRAME.get(), PILL.get());
    if frame.is_invalid() || pill.is_invalid() {
        return;
    }
    let (_, work, s) = under_cursor();
    ASKED.set(None);
    SCROLLING.set(scrolling);
    SCALE.set(s);
    BEGIN.set(Some(Instant::now()));
    PAUSED.set(None);
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

/// What the pill was clicked for since the last call.
pub fn asked() -> Option<Ask> {
    ASKED.take()
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_TIMER => render_pill(),
        WM_LBUTTONUP => {
            let width = PILL_SIZE.0 * SCALE.get();
            let ask = match pill_button(lparam_point(lp).x as f32, width, 2) {
                None => Ask::Stop,
                Some(0) => Ask::Pause,
                Some(_) => Ask::Cancel,
            };
            ASKED.set(Some(ask));
        }
        _ => return unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
    LRESULT(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grab;

    /// `cargo test -p snapling-win rec_ui -- --ignored --nocapture`: shows the frame and the pill
    /// for a moment and looks at the screen through the capture a recording uses.
    #[test]
    #[ignore]
    fn the_pill_stays_out_of_the_capture() {
        init().unwrap();
        let area = Rect {
            left: 40,
            top: 200,
            width: 400,
            height: 200,
        };
        let before = grab(area.left, 120, 400, 100, false).unwrap();
        show(area, false);
        std::thread::sleep(std::time::Duration::from_millis(300));
        let during = grab(area.left, 120, 400, 100, false).unwrap();
        hide();
        let changed = before
            .as_chunks::<4>()
            .0
            .iter()
            .zip(during.as_chunks::<4>().0)
            .filter(|(a, b)| a != b)
            .count();
        assert!(changed < 2000, "{changed} pixels changed");
    }
}
