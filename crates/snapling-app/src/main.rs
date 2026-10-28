#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

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

const HK_CAPTURE_SCREEN: i32 = 1;
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
    let mut rgba = Vec::with_capacity((N * N * 4) as usize);
    for y in 0..N {
        for x in 0..N {
            let inner = (8..24).contains(&x) && (8..24).contains(&y);
            rgba.extend_from_slice(if inner {
                &[255, 255, 255, 255]
            } else {
                &[124, 58, 237, 255]
            });
        }
    }
    Icon::from_rgba(rgba, N, N).expect("valid icon")
}

fn capture_screen() {
    let t0 = Instant::now();
    let img = match snapling_win::capture_virtual_screen() {
        Ok(img) => img,
        Err(e) => return error!("capture: {e}"),
    };
    let t1 = Instant::now();
    if let Err(e) = snapling_win::copy_image(&img) {
        return error!("clipboard: {e}");
    }
    info!(
        "capture {}x{}: grab {} ms, clipboard {} ms",
        img.width,
        img.height,
        (t1 - t0).as_millis(),
        t1.elapsed().as_millis()
    );
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

    match Hotkey::parse(&cfg.hotkeys.capture_screen)
        .and_then(|hk| snapling_win::register_hotkey(HK_CAPTURE_SCREEN, &hk).map(|_| hk))
    {
        Ok(hk) => info!("hotkey capture_screen = {hk}"),
        Err(e) => {
            warn!("hotkey: {e}");
            snapling_win::warn(&e);
        }
    }

    let capture = MenuItem::new("Снимок экрана", true, None);
    let open_cfg = MenuItem::new("Папка настроек", true, None);
    let quit = MenuItem::new("Выход", true, None);
    let menu = Menu::new();
    let _ = menu.append_items(&[&capture, &open_cfg, &PredefinedMenuItem::separator(), &quit]);
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
        if msg.message == WM_HOTKEY && msg.wParam.0 == HK_CAPTURE_SCREEN as usize {
            capture_screen();
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            if ev.id == capture.id() {
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
