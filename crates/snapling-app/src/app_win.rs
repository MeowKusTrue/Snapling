use snapling_core::config::{Action, Config, Save, SoundCfg};
use snapling_core::hotkey::{self, Hotkey};
use snapling_core::selection::Rect;
use snapling_core::sound::{self, Sound};
use snapling_core::{annotate, history, storage};
use snapling_win::overlay::Outcome;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
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
    /// Validated save folder (created lazily on first save) and its settings; `None` = saving off.
    static SAVE: RefCell<Option<(PathBuf, Save)>> = const { RefCell::new(None) };
}

/// Set by the background saver.
static LAST_SAVED: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Sound settings and pre-rendered WAVs; read from the saver thread too.
type Wavs = [&'static [u8]; 3];
static SOUND: Mutex<Option<(SoundCfg, Wavs)>> = Mutex::new(None);

thread_local! {
    /// `[capture] copy`: clipboard for hotkey captures (Enter in the overlay always copies).
    static COPY_ON_CAPTURE: Cell<bool> = const { Cell::new(true) };
}

fn play(s: Sound) {
    let Some((cfg, wavs)) = SOUND.lock().ok().and_then(|g| g.clone()) else {
        return;
    };
    if !cfg.enabled || (cfg.mute_in_games && snapling_win::fullscreen_game()) {
        return;
    }
    let i = match s {
        Sound::Capture => 0,
        Sound::Copy => 1,
        Sound::Error => 2,
    };
    snapling_win::play_wav(wavs[i]);
}

fn apply_sound(cfg: &SoundCfg) {
    let Ok(mut current) = SOUND.lock() else {
        return;
    };
    let volume_changed = current.as_ref().is_none_or(|(c, _)| c.volume != cfg.volume);
    let wavs = match current.take() {
        Some((_, w)) if !volume_changed => w,
        // ponytail: leaks ~40 KB per volume change (PlaySound reads the buffer after returning);
        // free old buffers after PlaySound(NULL) if volume becomes a live slider.
        _ => [Sound::Capture, Sound::Copy, Sound::Error]
            .map(|s| &*Box::leak(sound::wav(s, cfg.volume).into_boxed_slice())),
    };
    *current = Some((cfg.clone(), wavs));
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
        Err(e) => {
            error!("clipboard: {e}");
            play(Sound::Error);
        }
    }
}

fn history_file() -> PathBuf {
    app_dir("LOCALAPPDATA").join("history.jsonl")
}

/// Encodes and writes on a worker thread so PNG of a 4K screen never blocks the UI.
fn save_async(img: snapling_win::Image, app: String, kind: &'static str) {
    let Some((dir, cfg)) = SAVE.with(|s| s.borrow().clone()) else {
        warn!("save skipped: no valid folder");
        return;
    };
    let stamp = snapling_win::local_stamp();
    std::thread::spawn(move || {
        let t0 = Instant::now();
        let name = storage::render_name(&cfg.name, stamp, &app);
        let saved = storage::prepare_dir(&dir).and_then(|d| {
            storage::save_bgra(
                &img.bgra,
                img.width,
                img.height,
                &d,
                &name,
                cfg.format,
                cfg.jpeg_quality,
            )
        });
        match saved {
            Ok(path) => {
                info!(
                    "saved {} in {} ms",
                    path.display(),
                    t0.elapsed().as_millis()
                );
                let entry = history::Entry {
                    time: history::time_string(stamp),
                    path: path.to_string_lossy().to_string(),
                    width: img.width,
                    height: img.height,
                    app,
                    kind: kind.into(),
                };
                if let Err(e) = history::append(&history_file(), &entry) {
                    warn!("history: {e}");
                }
                if let Ok(mut last) = LAST_SAVED.lock() {
                    *last = Some(path);
                }
            }
            Err(e) => {
                error!("save: {e}");
                play(Sound::Error);
                snapling_win::warn(&format!("Не удалось сохранить снимок.\n{e}"));
            }
        }
    });
}

/// Shutter sound, copy (if enabled or `force_copy`), then save when `auto_save` is on.
fn deliver(
    img: snapling_win::Image,
    t0: Instant,
    app: String,
    kind: &'static str,
    force_copy: bool,
) {
    play(Sound::Capture);
    if force_copy || COPY_ON_CAPTURE.get() {
        copy(&img, t0);
    }
    if SAVE.with(|s| s.borrow().as_ref().is_some_and(|(_, c)| c.auto_save)) {
        save_async(img, app, kind);
    }
}

fn edit_dir() -> PathBuf {
    app_dir("LOCALAPPDATA").join("edit")
}

/// Hands the shot to a separate editor process through a raw file it deletes after reading.
fn open_editor(img: snapling_win::Image, app: String) {
    std::thread::spawn(move || {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = edit_dir().join(format!("{nanos}.raw"));
        let started = std::fs::create_dir_all(edit_dir())
            .and_then(|()| annotate::write_raw(&path, &img.bgra, img.width, img.height, &app))
            .and_then(|()| std::env::current_exe())
            .and_then(|exe| {
                std::process::Command::new(exe)
                    .arg("--edit")
                    .arg(&path)
                    .spawn()
            });
        match started {
            Ok(_) => info!("editor: {}x{}", img.width, img.height),
            Err(e) => {
                let _ = std::fs::remove_file(&path);
                error!("editor: {e}");
                play(Sound::Error);
                snapling_win::warn(&format!("Не удалось открыть редактор.\n{e}"));
            }
        }
    });
}

/// Clipboard and save folder for the editor process (reads config.toml itself).
pub fn editor_hooks() -> crate::editor::Hooks {
    crate::editor::Hooks {
        copy: |bgra, w, h| {
            snapling_win::copy_image(&snapling_win::Image {
                x: 0,
                y: 0,
                width: w,
                height: h,
                bgra: bgra.to_vec(),
            })
        },
        save: |bgra, w, h, app| {
            let cfg = Config::load_or_create(&app_dir("APPDATA").join("config.toml"))?.save;
            let stamp = snapling_win::local_stamp();
            let dir = storage::prepare_dir(&save_dir(&cfg)?)?;
            let name = storage::render_name(&cfg.name, stamp, app);
            let path = storage::save_bgra(bgra, w, h, &dir, &name, cfg.format, cfg.jpeg_quality)?;
            let entry = history::Entry {
                time: history::time_string(stamp),
                path: path.to_string_lossy().to_string(),
                width: w,
                height: h,
                app: app.into(),
                kind: "edit".into(),
            };
            let _ = history::append(&history_file(), &entry);
            Ok(path)
        },
        warn: snapling_win::warn,
    }
}

fn capture_failed(e: String) {
    error!("capture: {e}");
    play(Sound::Error);
}

/// Grabs all monitors and delivers `screen_rect` of it, or everything when `None`.
fn capture_rect(screen_rect: Option<Rect>, kind: &'static str) {
    let app = snapling_win::foreground_app_name();
    let t0 = Instant::now();
    let img = match snapling_win::capture_virtual_screen() {
        Ok(img) => img,
        Err(e) => return capture_failed(e),
    };
    let Some(r) = screen_rect else {
        return deliver(img, t0, app, kind, false);
    };
    let local = Rect {
        left: r.left - img.x,
        top: r.top - img.y,
        ..r
    };
    match img.crop(local) {
        Some(part) => deliver(part, t0, app, kind, false),
        None => warn!("region {r:?} is off screen"),
    }
}

fn capture_region() {
    if snapling_win::overlay::is_open() {
        return;
    }
    if snapling_win::fullscreen_game() {
        info!("game mode: fullscreen app, capture without overlay");
        return capture_rect(None, "screen");
    }
    let app = snapling_win::foreground_app_name();
    let t0 = Instant::now();
    let img = match snapling_win::capture_virtual_screen() {
        Ok(img) => img,
        Err(e) => return capture_failed(e),
    };
    let grab = t0.elapsed().as_millis();
    let shown = snapling_win::overlay::show(img, move |res| {
        let Some((img, outcome)) = res else { return };
        LAST_REGION.set(Some(Rect {
            left: img.x,
            top: img.y,
            width: img.width as i32,
            height: img.height as i32,
        }));
        match outcome {
            Outcome::Copy => deliver(img, Instant::now(), app, "region", true),
            Outcome::Save => {
                play(Sound::Capture);
                save_async(img, app, "region");
            }
            Outcome::Edit => {
                play(Sound::Capture);
                open_editor(img, app);
            }
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
        Action::CaptureScreen => capture_rect(None, "screen"),
        Action::CaptureWindow => {
            let r = snapling_win::targets::foreground_window().map(|r| Rect {
                left: r.left,
                top: r.top,
                width: r.right - r.left,
                height: r.bottom - r.top,
            });
            capture_rect(r, "window");
        }
        Action::RepeatRegion => match LAST_REGION.get() {
            Some(r) => capture_rect(Some(r), "repeat"),
            None => capture_region(),
        },
    }
}

/// Resolves the save folder (shape check only; created on first save).
fn save_dir(save: &Save) -> Result<PathBuf, String> {
    let dir = match save.dir.trim() {
        "" => snapling_win::pictures_dir().map(|p| p.join("Snapling")),
        d => Some(PathBuf::from(d)),
    };
    match dir {
        None => Err("папка «Изображения» не найдена — укажи [save] dir".to_string()),
        Some(d) => match storage::shape_error(&d) {
            Some(e) => Err(e),
            None => Ok(d),
        },
    }
}

fn apply_save(cfg: &Config) -> Vec<String> {
    match save_dir(&cfg.save) {
        Ok(d) => {
            info!(
                "save dir {} ({:?}, auto_save {})",
                d.display(),
                cfg.save.format,
                cfg.save.auto_save
            );
            SAVE.with(|s| *s.borrow_mut() = Some((d, cfg.save.clone())));
            Vec::new()
        }
        Err(e) => {
            SAVE.with(|s| *s.borrow_mut() = None);
            vec![format!("Сохранение отключено: {e}")]
        }
    }
}

fn apply_config(cfg: &Config) {
    let mut problems = apply_hotkeys(cfg);
    problems.extend(apply_save(cfg));
    apply_sound(&cfg.sound);
    COPY_ON_CAPTURE.set(cfg.capture.copy);
    let hide = cfg.privacy.hide_from_capture;
    match snapling_win::overlay::set_hidden_from_capture(hide) {
        Ok(()) => info!("overlay hidden from capture: {hide}"),
        Err(e) if hide => problems.push(format!(
            "Скрыть выделение от демонстрации экрана не удалось (нужна Windows 10 2004+): {e}"
        )),
        Err(e) => warn!("display affinity: {e}"),
    }
    report(&problems);
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
        warn!("config: {p}");
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
    let _ = std::fs::remove_dir_all(edit_dir());
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
    apply_config(&cfg);

    let region = MenuItem::new("Выделить область", true, None);
    let screen = MenuItem::new("Весь экран", true, None);
    let last = MenuItem::new("Последний снимок", true, None);
    let folder = MenuItem::new("Папка снимков", true, None);
    let open_cfg = MenuItem::new("Настройки (config.toml)", true, None);
    let quit = MenuItem::new("Выход", true, None);
    let menu = Menu::new();
    let _ = menu.append_items(&[
        &region,
        &screen,
        &PredefinedMenuItem::separator(),
        &last,
        &folder,
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
                            apply_config(&cfg);
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
                capture_rect(None, "screen");
            } else if ev.id == last.id() {
                match LAST_SAVED.lock().ok().and_then(|l| l.clone()) {
                    Some(p) => {
                        let _ = std::process::Command::new("explorer")
                            .arg(format!("/select,{}", p.display()))
                            .spawn();
                    }
                    None => snapling_win::warn("В этом сеансе снимков ещё не сохранено."),
                }
            } else if ev.id == folder.id() {
                let dir = SAVE.with(|s| s.borrow().as_ref().map(|(d, _)| d.clone()));
                match dir.map(|d| storage::prepare_dir(&d)) {
                    Some(Ok(d)) => {
                        let _ = std::process::Command::new("explorer").arg(d).spawn();
                    }
                    Some(Err(e)) => snapling_win::warn(&e),
                    None => snapling_win::warn(
                        "Сохранение отключено — проверь [save] dir в config.toml.",
                    ),
                }
            } else if ev.id == open_cfg.id() {
                let _ = std::process::Command::new("notepad").arg(&cfg_path).spawn();
            } else if ev.id == quit.id() {
                unsafe { PostQuitMessage(0) };
            }
        }
    }
    info!("exit");
}
