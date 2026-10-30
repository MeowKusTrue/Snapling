//! Tray menu, `assets/design/TrayWindows.dc.html`: a layered popup in the tray process.
//! It takes focus so that a click anywhere else closes it; drawing comes from `snapling-scene`.

use crate::toast::{blit, under_cursor};
use snapling_core::selection::Point;
use snapling_scene::{MENU_SHADOW, MenuIcon, MenuRow, draw_menu, menu_at, menu_size};
use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_DOWN, VK_ESCAPE, VK_RETURN, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, HWND_TOPMOST, IDC_ARROW, LoadCursorW, RegisterClassW, SW_HIDE,
    SW_SHOW, SWP_NOACTIVATE, SetForegroundWindow, SetWindowPos, ShowWindow, WA_INACTIVE,
    WM_ACTIVATE, WM_KEYDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONUP, WNDCLASSW, WS_EX_LAYERED,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::w;

const GAP: f32 = 12.0;
/// A press on the tray icon closes the open menu; its release must not reopen it.
const REOPEN: Duration = Duration::from_millis(250);

struct State {
    rows: Vec<MenuRow>,
    scale: f32,
    light: bool,
    hover: Option<usize>,
}

thread_local! {
    static HWND_MENU: Cell<HWND> = Cell::new(HWND::default());
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    static PICKED: Cell<Option<MenuIcon>> = const { Cell::new(None) };
    static CLOSED: Cell<Option<Instant>> = const { Cell::new(None) };
}

pub fn init() -> Result<(), String> {
    unsafe {
        let hinst = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        let class = w!("SnaplingMenu");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            return Err("RegisterClassW".into());
        }
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
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
        HWND_MENU.set(hwnd);
    }
    Ok(())
}

/// What the user chose since the last call.
pub fn picked() -> Option<MenuIcon> {
    PICKED.take()
}

pub fn show(rows: Vec<MenuRow>, light: bool) {
    let hwnd = HWND_MENU.get();
    let reopened = CLOSED.get().is_some_and(|t| t.elapsed() < REOPEN);
    if hwnd.is_invalid() || reopened {
        return;
    }
    let (cursor, work, scale) = under_cursor();
    let (w, h) = menu_size(&rows, scale);
    let (w, h) = (w as i32, h as i32);
    let gap = (GAP * scale) as i32 - (MENU_SHADOW * scale) as i32;
    let x = (cursor.x - w / 2).clamp(work.left + gap, (work.right - gap - w).max(work.left + gap));
    let y = if cursor.y > (work.top + work.bottom) / 2 {
        work.bottom - gap - h
    } else {
        work.top + gap
    };
    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            rows,
            scale,
            light,
            hover: None,
        })
    });
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE);
    }
    render(hwnd);
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
}

fn hide(hwnd: HWND) {
    if STATE.with(|s| s.borrow_mut().take()).is_some() {
        CLOSED.set(Some(Instant::now()));
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
}

fn render(hwnd: HWND) {
    STATE.with(|st| {
        let st = st.borrow();
        let Some(s) = st.as_ref() else { return };
        let (w, h) = menu_size(&s.rows, s.scale);
        blit(hwnd, w, h, |pm| {
            draw_menu(pm, &s.rows, s.scale, s.hover, s.light);
        });
    });
}

fn set_hover(hwnd: HWND, hover: Option<usize>) {
    let changed = STATE.with(|s| {
        s.borrow_mut()
            .as_mut()
            .is_some_and(|s| std::mem::replace(&mut s.hover, hover) != hover)
    });
    if changed {
        render(hwnd);
    }
}

fn row_at(lp: LPARAM) -> Option<usize> {
    let p = Point {
        x: (lp.0 & 0xFFFF) as u16 as i16 as i32,
        y: ((lp.0 >> 16) & 0xFFFF) as u16 as i16 as i32,
    };
    STATE.with(|s| {
        let s = s.borrow();
        let s = s.as_ref()?;
        menu_at(&s.rows, s.scale, p)
    })
}

/// Next item from the hovered one, skipping lines and wrapping around.
fn step(forward: bool) -> Option<usize> {
    STATE.with(|s| {
        let s = s.borrow();
        let s = s.as_ref()?;
        let n = s.rows.len();
        let from = s.hover.unwrap_or(if forward { n - 1 } else { 0 });
        (1..=n)
            .map(|d| {
                if forward {
                    (from + d) % n
                } else {
                    (from + n - d) % n
                }
            })
            .find(|&i| matches!(s.rows[i], MenuRow::Item(..)))
    })
}

fn choose(hwnd: HWND, row: Option<usize>) {
    let chosen = STATE.with(|s| match s.borrow().as_ref()?.rows.get(row?)? {
        MenuRow::Item(icon, ..) => Some(*icon),
        MenuRow::Line => None,
    });
    hide(hwnd);
    PICKED.set(chosen);
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEMOVE => {
            let mut track = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            let _ = unsafe { TrackMouseEvent(&mut track) };
            set_hover(hwnd, row_at(lp));
        }
        WM_MOUSELEAVE => set_hover(hwnd, None),
        WM_LBUTTONUP | WM_RBUTTONUP => choose(hwnd, row_at(lp)),
        WM_ACTIVATE if (wp.0 & 0xFFFF) as u32 == WA_INACTIVE => hide(hwnd),
        WM_KEYDOWN => match wp.0 as u16 {
            k if k == VK_ESCAPE.0 => hide(hwnd),
            k if k == VK_DOWN.0 => set_hover(hwnd, step(true)),
            k if k == VK_UP.0 => set_hover(hwnd, step(false)),
            k if k == VK_RETURN.0 => {
                let row = STATE.with(|s| s.borrow().as_ref()?.hover);
                if row.is_some() {
                    choose(hwnd, row);
                }
            }
            _ => {}
        },
        _ => return unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
    LRESULT(0)
}
