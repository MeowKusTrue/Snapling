use snapling_core::config::{Action, Config};
use snapling_core::hotkey::{self, Hotkey};
use snapling_core::selection::Rect;
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};
use tracing::{error, info, warn};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, PostQuitMessage, SetTimer, TranslateMessage, WM_HOTKEY,
    WM_TIMER,
};

const LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;
const CONFIG_POLL_MS: u32 = 1500;

thread_local! {
    /// Last confirmed region in screen coordinates, for "repeat region".
    static LAST_REGION: Cell<Option<Rect>> = const { Cell::new(None) };
}

fn app_dir(env: &str) -> PathBuf {
    std::env::var_os(env)
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Snapling")
}

fn init_log() {
    let dir = app_dir("LOCALAPPDATA");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("snapling.log");
    let too_big = std::fs::metadata(&path).is_ok_and(|m| m.len() > LOG_MAX_BYTES);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(!too_big)
        .write(true)
        .truncate(too_big)
        .open(&path);
    if let Ok(file) = file {
        tracing_subscriber::fmt()
            .with_writer(std::sync::Mutex::new(file))
            .init();
    }
    std::panic::set_hook(Box::new(|p| error!("panic: {p}")));
}

fn tray_icon() -> Icon {
    const N: u32 = 32;
    let rgba = crate::icon::frame_spark(N, [0x29, 0x5B, 0xE8], [0xFF, 0x8A, 0x1F]);
    Icon::from_rgba(rgba, N, N).expect("valid icon")
}

fn copy(img: &snapling_win::Image, t0: Instant) {
    match snapling_win::copy_image(img) {
        Ok(()) => info!(
            "copied {}x{} in {} ms",
            img.width,
            img.height,
            t0.elapsed().as_millis()
        ),
        Err(e) => error!("clipboard: {e}"),
    }
}

/// Grabs all monitors and copies `screen_rect` of it, or everything when `None`.
fn capture_rect(screen_rect: Option<Rect>) {
    let t0 = Instant::now();
    let img = match snapling_win::capture_virtual_screen() {
        Ok(img) => img,
        Err(e) => return error!("capture: {e}"),
    };
    let Some(r) = screen_rect else {
        return copy(&img, t0);
    };
    let local = Rect {
        left: r.left - img.x,
        top: r.top - img.y,
        ..r
    };
    match img.crop(local) {
        Some(part) => copy(&part, t0),
        None => warn!("region {r:?} is off screen"),
    }
}

fn capture_region() {
    if snapling_win::overlay::is_open() {
        return;
    }
    if snapling_win::fullscreen_game() {
        info!("game mode: fullscreen app, capture without overlay");
        return capture_rect(None);
    }
    let t0 = Instant::now();
    let img = match snapling_win::capture_virtual_screen() {
        Ok(img) => img,
        Err(e) => return error!("capture: {e}"),
    };
    let grab = t0.elapsed().as_millis();
    let shown = snapling_win::overlay::show(img, |res| {
        if let Some(img) = res {
            LAST_REGION.set(Some(Rect {
                left: img.x,
                top: img.y,
                width: img.width as i32,
                height: img.height as i32,
            }));
            copy(&img, Instant::now());
        }
    });
    match shown {
        Ok(()) => info!(
            "overlay: grab {grab} ms, shown {} ms",
            t0.elapsed().as_millis()
        ),
        Err(e) => error!("overlay: {e}"),
    }
}

fn run_action(a: Action) {
    match a {
        Action::CaptureRegion => capture_region(),
        Action::CaptureScreen => capture_rect(None),
        Action::CaptureWindow => {
            let r = snapling_win::targets::foreground_window().map(|r| Rect {
                left: r.left,
                top: r.top,
                width: r.right - r.left,
                height: r.bottom - r.top,
            });
            capture_rect(r);
        }
        Action::RepeatRegion => match LAST_REGION.get() {
            Some(r) => capture_rect(Some(r)),
            None => capture_region(),
        },
    }
}

fn hotkey_id(a: Action) -> i32 {
    Action::ALL.iter().position(|x| *x == a).unwrap_or(0) as i32 + 1
}

/// Re-registers everything and returns problems for one combined message.
fn apply_hotkeys(cfg: &Config) -> Vec<String> {
    for a in Action::ALL {
        snapling_win::unregister_hotkey(hotkey_id(a));
    }
    let (bindings, mut problems) = hotkey::check(&cfg.hotkeys, hotkey::reserved_on_windows);
    for (a, hk) in bindings {
        match snapling_win::register_hotkey(hotkey_id(a), &hk) {
            Ok(()) => info!("hotkey {a:?} = {hk}"),
            Err(e) => problems.push(format!("«{}»: {e}", a.title())),
        }
        if hk == Hotkey::parse("PrintScreen").expect("valid")
            && snapling_win::snipping_owns_printscreen()
        {
            problems.push(
                "PrtScn сейчас открывает «Ножницы» Windows. Выключить: Параметры → \
                 Специальные возможности → Клавиатура → «Использовать клавишу PrtScn…»."
                    .into(),
            );
        }
    }
    problems
}

fn report(problems: &[String]) {
    if problems.is_empty() {
        return;
    }
    for p in problems {
        warn!("hotkeys: {p}");
    }
    let list: Vec<String> = problems.iter().map(|p| format!("• {p}")).collect();
    snapling_win::warn(&format!(
        "Горячие клавиши:\n\n{}\n\nИзменить: меню в трее → «Настройки (config.toml)».",
        list.join("\n")
    ));
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

pub fn run() {
    snapling_win::set_dpi_aware();
    init_log();
    if !snapling_win::single_instance() {
        warn!("already running, exit");
        return;
    }
    info!(
        "{} {} start",
        snapling_core::APP_NAME,
        snapling_core::VERSION
    );

    let cfg_dir = app_dir("APPDATA");
    let cfg_path = cfg_dir.join("config.toml");
    let cfg = Config::load_or_create(&cfg_path).unwrap_or_else(|e| {
        snapling_win::warn(&format!(
            "Ошибка в config.toml, используются настройки по умолчанию.\n{e}"
        ));
        Config::default()
    });
    let mut cfg_mtime = modified(&cfg_path);
    if let Err(e) = snapling_win::overlay::init() {
        error!("overlay init: {e}");
    }
    report(&apply_hotkeys(&cfg));

    let region = MenuItem::new("Выделить область", true, None);
    let screen = MenuItem::new("Весь экран", true, None);
    let open_cfg = MenuItem::new("Настройки (config.toml)", true, None);
    let quit = MenuItem::new("Выход", true, None);
    let menu = Menu::new();
    let _ = menu.append_items(&[
        &region,
        &screen,
        &PredefinedMenuItem::separator(),
        &open_cfg,
        &PredefinedMenuItem::separator(),
        &quit,
    ]);
    let _tray = match TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip(snapling_core::APP_NAME)
        .with_icon(tray_icon())
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            error!("tray: {e}");
            snapling_win::warn(&format!("Не удалось создать значок в трее: {e}"));
            return;
        }
    };
    unsafe { SetTimer(None, 0, CONFIG_POLL_MS, None) };

    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
        match msg.message {
            WM_HOTKEY => {
                if let Some(a) = Action::ALL
                    .into_iter()
                    .find(|a| hotkey_id(*a) == msg.wParam.0 as i32)
                {
                    run_action(a);
                }
            }
            // Live reload: a saved config.toml applies without restart.
            WM_TIMER if msg.hwnd.is_invalid() => {
                let now = modified(&cfg_path);
                if now != cfg_mtime {
                    cfg_mtime = now;
                    match Config::load_or_create(&cfg_path) {
                        Ok(cfg) => {
                            info!("config reloaded");
                            report(&apply_hotkeys(&cfg));
                        }
                        Err(e) => {
                            warn!("config: {e}");
                            snapling_win::warn(&format!(
                                "Ошибка в config.toml, оставлены прежние настройки.\n{e}"
                            ));
                        }
                    }
                }
            }
            _ => {}
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            if ev.id == region.id() {
                capture_region();
            } else if ev.id == screen.id() {
                capture_rect(None);
            } else if ev.id == open_cfg.id() {
                let _ = std::process::Command::new("notepad").arg(&cfg_path).spawn();
            } else if ev.id == quit.id() {
                unsafe { PostQuitMessage(0) };
            }
        }
    }
    info!("exit");
}
