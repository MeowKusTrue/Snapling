//! Elements inside a window under the cursor (buttons, panes, chat messages…) via UI Automation.
//! Runs on its own thread: cross-process UIA calls can take tens of ms and must not stall the overlay.
//! Note: querying Chromium/Electron apps (Chrome, Discord) switches on their accessibility tree.

use std::sync::Mutex;
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTreeWalker,
};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

const BUDGET: Duration = Duration::from_millis(250);
const MAX_DEPTH: usize = 24;

pub struct Request {
    pub seq: u64,
    pub hwnd: isize,
    pub point: POINT,
}

/// Latest finished chain: (window, request, screen rects outermost first).
pub static RESULT: Mutex<Option<(isize, u64, Vec<RECT>)>> = Mutex::new(None);

/// Results are announced with `PostMessageW(notify, msg, seq, 0)`.
pub fn spawn(notify: isize, msg: u32) -> Sender<Request> {
    let (tx, rx) = channel::<Request>();
    std::thread::spawn(move || {
        let walker = unsafe {
            if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() {
                return;
            }
            let Ok(uia) =
                CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
            else {
                return;
            };
            let Ok(walker) = uia.ControlViewWalker() else {
                return;
            };
            (uia, walker)
        };
        while let Ok(mut req) = rx.recv() {
            while let Ok(newer) = rx.try_recv() {
                req = newer;
            }
            let chain = element_chain(&walker.0, &walker.1, HWND(req.hwnd as _), req.point);
            if let Ok(mut slot) = RESULT.lock() {
                *slot = Some((req.hwnd, req.seq, chain));
            }
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(notify as _)),
                    msg,
                    WPARAM(req.seq as usize),
                    LPARAM(0),
                );
            }
        }
    });
    tx
}

fn contains(r: &RECT, p: POINT) -> bool {
    r.right > r.left
        && r.bottom > r.top
        && p.x >= r.left
        && p.x < r.right
        && p.y >= r.top
        && p.y < r.bottom
}

fn visible_rect(element: &IUIAutomationElement, p: POINT) -> Option<RECT> {
    unsafe {
        element
            .CurrentBoundingRectangle()
            .ok()
            .filter(|r| contains(r, p))
    }
}

/// The element actually hit at the point, then its parents. Walking every sibling is both slower
/// and ambiguous: overlapping panes do not have a reliable paint order in the UIA tree.
fn point_chain(
    uia: &IUIAutomation,
    walker: &IUIAutomationTreeWalker,
    root: &IUIAutomationElement,
    p: POINT,
) -> Vec<RECT> {
    let started = Instant::now();
    let Ok(mut cur) = (unsafe { uia.ElementFromPoint(p) }) else {
        return Vec::new();
    };
    let mut chain = Vec::new();
    for _ in 0..MAX_DEPTH {
        let is_root = unsafe { uia.CompareElements(&cur, root) }.is_ok_and(|same| same.as_bool());
        if let Some(r) = visible_rect(&cur, p) {
            chain.push(r);
        } else if is_root {
            // Keep the root slot: the session replaces it with the exact Win32 frame bounds.
            chain.push(unsafe { cur.CurrentBoundingRectangle() }.unwrap_or_default());
        } else if chain.is_empty() {
            return Vec::new();
        }
        // Some Chromium ancestors have no bounds; their children still provide useful levels.
        if is_root {
            chain.reverse();
            return chain;
        }
        if started.elapsed() >= BUDGET {
            break;
        }
        let Ok(parent) = (unsafe { walker.GetParentElement(&cur) }) else {
            break;
        };
        cur = parent;
    }
    // A hit from another top-level window must not be attached to the requested one.
    Vec::new()
}

fn element_chain(
    uia: &IUIAutomation,
    walker: &IUIAutomationTreeWalker,
    hwnd: HWND,
    p: POINT,
) -> Vec<RECT> {
    let Ok(root) = (unsafe { uia.ElementFromHandle(hwnd) }) else {
        return Vec::new();
    };
    let chain = point_chain(uia, walker, &root, p);
    if !chain.is_empty() {
        return chain;
    }
    // No usable inner element: retain window-level selection instead of guessing a sibling.
    visible_rect(&root, p).into_iter().collect()
}

#[cfg(test)]
mod live {
    use super::*;

    #[test]
    #[ignore = "reads real windows on this desktop"]
    fn chains_for_top_windows() {
        crate::set_dpi_aware();
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap() };
        let uia: IUIAutomation =
            unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }.unwrap();
        let walker = unsafe { uia.ControlViewWalker() }.unwrap();
        let t = Instant::now();
        let wins = crate::targets::windows();
        println!(
            "windows: {} in {:?}, monitors: {}",
            wins.len(),
            t.elapsed(),
            crate::targets::monitors().len()
        );
        for (h, r) in wins.iter().take(6) {
            let p = POINT {
                x: (r.left + r.right) / 2,
                y: (r.top + r.bottom) / 2,
            };
            let t = Instant::now();
            let chain = element_chain(&uia, &walker, HWND(*h as _), p);
            println!(
                "{} {}x{} depth {} in {:?}",
                crate::app_name_of(*h),
                r.right - r.left,
                r.bottom - r.top,
                chain.len(),
                t.elapsed()
            );
        }
    }
}

#[cfg(test)]
mod probe {
    use super::*;

    #[test]
    #[ignore = "probes a real window: SNAP_HWND=<decimal hwnd>"]
    fn probe_window() {
        crate::set_dpi_aware();
        let hwnd: isize = std::env::var("SNAP_HWND").unwrap().parse().unwrap();
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap() };
        let uia: IUIAutomation =
            unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }.unwrap();
        let walker = unsafe { uia.ControlViewWalker() }.unwrap();
        let mut r = RECT::default();
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetWindowRect(HWND(hwnd as _), &mut r).unwrap()
        };
        for round in 0..3 {
            for (fx, fy) in [(0.1, 0.5), (0.5, 0.5), (0.5, 0.9), (0.9, 0.3)] {
                let p = POINT {
                    x: r.left + ((r.right - r.left) as f32 * fx) as i32,
                    y: r.top + ((r.bottom - r.top) as f32 * fy) as i32,
                };
                let t = Instant::now();
                let chain = element_chain(&uia, &walker, HWND(hwnd as _), p);
                println!(
                    "round {round} ({fx},{fy}) depth {} in {:?}",
                    chain.len(),
                    t.elapsed()
                );
            }
            std::thread::sleep(Duration::from_millis(1500));
        }
    }
}
