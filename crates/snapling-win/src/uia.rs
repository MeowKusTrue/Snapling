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
const MAX_SIBLINGS: usize = 400;

pub struct Request {
    pub seq: u64,
    pub hwnd: isize,
    pub point: POINT,
}

/// Latest finished chain: (window, screen rects outermost first).
pub static RESULT: Mutex<Option<(isize, Vec<RECT>)>> = Mutex::new(None);

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
                *slot = Some((req.hwnd, chain));
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
    p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
}

fn element_chain(
    uia: &IUIAutomation,
    walker: &IUIAutomationTreeWalker,
    hwnd: HWND,
    p: POINT,
) -> Vec<RECT> {
    let t0 = Instant::now();
    let mut chain = Vec::new();
    let Ok(mut cur) = (unsafe { uia.ElementFromHandle(hwnd) }) else {
        return chain;
    };
    for _ in 0..MAX_DEPTH {
        let Ok(r) = (unsafe { cur.CurrentBoundingRectangle() }) else {
            break;
        };
        chain.push(r);
        if t0.elapsed() > BUDGET {
            break;
        }
        match child_at(walker, &cur, p) {
            Some(c) => cur = c,
            None => break,
        }
    }
    chain
}

fn child_at(
    walker: &IUIAutomationTreeWalker,
    parent: &IUIAutomationElement,
    p: POINT,
) -> Option<IUIAutomationElement> {
    let mut hits = Vec::new();
    let mut child = unsafe { walker.GetFirstChildElement(parent) }.ok();
    for _ in 0..MAX_SIBLINGS {
        let Some(c) = child else { break };
        let hit = unsafe {
            c.CurrentBoundingRectangle().is_ok_and(|r| contains(&r, p))
                && !c.CurrentIsOffscreen().is_ok_and(|o| o.as_bool())
        };
        child = unsafe { walker.GetNextSiblingElement(&c) }.ok();
        if hit {
            hits.push(c);
        }
    }
    // Overlapping siblings: later ones paint on top in web apps, and Chromium puts an empty
    // legacy pane first (Discord stopped at it). Prefer the last hit that has children.
    let with_kids = hits
        .iter()
        .rposition(|c| unsafe { walker.GetFirstChildElement(c) }.is_ok());
    match with_kids {
        Some(i) => Some(hits.swap_remove(i)),
        None => hits.pop(),
    }
}

#[cfg(test)]
mod live {
    use super::*;

    #[test]
    #[ignore = "reads real windows on this desktop"]
    fn chains_for_top_windows() {
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
                "{}x{} depth {} in {:?}",
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
