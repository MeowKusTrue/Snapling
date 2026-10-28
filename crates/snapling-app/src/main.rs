#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod icon;

use snapling_core::config::Config;
use snapling_core::hotkey::Hotkey;
use std::path::PathBuf;
use std::time::Instant;
use tracing::{error, info, warn};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, PostQuitMessage, TranslateMessage, WM_HOTKEY,
};

const HK_CAPTURE_REGION: i32 = 1;
const HK_CAPTURE_SCREEN: i32 = 2;
const LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;

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
    let rgba = icon::frame_spark(N, [0x29, 0x5B, 0xE8], [0xFF, 0x8A, 0x1F]);
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

fn capture_screen() {
    let t0 = Instant::now();
    match snapling_win::capture_virtual_screen() {
        Ok(img) => copy(&img, t0),
        Err(e) => error!("capture: {e}"),
    }
}

fn capture_region() {
    if snapling_win::overlay::is_open() {
        return;
    }
    let t0 = Instant::now();
    let img = match snapling_win::capture_virtual_screen() {
        Ok(img) => img,
        Err(e) => return error!("capture: {e}"),
    };
    let grab = t0.elapsed().as_millis();
    let shown = snapling_win::overlay::show(img, |res| {
        if let Some(img) = res {
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

fn register(id: i32, name: &str, combo: &str) {
    if combo.trim().is_empty() {
        return;
    }
    match Hotkey::parse(combo).and_then(|hk| snapling_win::register_hotkey(id, &hk).map(|_| hk)) {
        Ok(hk) => info!("hotkey {name} = {hk}"),
        Err(e) => {
            warn!("hotkey {name}: {e}");
            snapling_win::warn(&e);
        }
    }
}

fn main() {
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
    let cfg = Config::load_or_create(&cfg_dir.join("config.toml")).unwrap_or_else(|e| {
        snapling_win::warn(&format!(
            "Ошибка в config.toml, используются настройки по умолчанию.\n{e}"
        ));
        Config::default()
    });
    if let Err(e) = snapling_win::overlay::init() {
        error!("overlay init: {e}");
    }
    register(
        HK_CAPTURE_REGION,
        "capture_region",
        &cfg.hotkeys.capture_region,
    );
    register(
        HK_CAPTURE_SCREEN,
        "capture_screen",
        &cfg.hotkeys.capture_screen,
    );

    let region = MenuItem::new("Выделить область", true, None);
    let screen = MenuItem::new("Весь экран", true, None);
    let open_cfg = MenuItem::new("Папка настроек", true, None);
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

    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
        if msg.message == WM_HOTKEY {
            match msg.wParam.0 as i32 {
                HK_CAPTURE_REGION => capture_region(),
                HK_CAPTURE_SCREEN => capture_screen(),
                _ => {}
            }
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            if ev.id == region.id() {
                capture_region();
            } else if ev.id == screen.id() {
                capture_screen();
            } else if ev.id == open_cfg.id() {
                let _ = std::process::Command::new("explorer").arg(&cfg_dir).spawn();
            } else if ev.id == quit.id() {
                unsafe { PostQuitMessage(0) };
            }
        }
    }
    info!("exit");
}
