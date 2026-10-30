use crate::app_dir;
use snapling_core::config::{Action, Config, Engine, Format, Save, SoundCfg, Theme, TrayIcon};
use snapling_core::hotkey::{self, Hotkey};
use snapling_core::i18n::{t, tf};
use snapling_core::selection::Rect;
use snapling_core::sound::{self, Sound};
use snapling_core::{annotate, history, storage};
use snapling_scene::{MenuIcon, MenuRow, ToastAction};
use snapling_win::overlay::Outcome;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Instant, SystemTime};
use tracing::{error, info, warn};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, PostQuitMessage, SetTimer, TranslateMessage, WM_HOTKEY,
    WM_TIMER,
};

const DEEPL_PROXY: &str = "DeepLProxy";
const LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;
const CONFIG_POLL_MS: u32 = 1500;

thread_local! {
    /// Last confirmed region in screen coordinates, for "repeat region".
    static LAST_REGION: Cell<Option<Rect>> = const { Cell::new(None) };
    /// Validated save folder (created lazily on first save) and its settings; `None` = saving off.
    static SAVE: RefCell<Option<(PathBuf, Save)>> = const { RefCell::new(None) };
}

/// Tray windows: at most one of each.
#[derive(Default)]
struct Window(Option<std::process::Child>);

impl Window {
    fn open(&mut self, flag: &str) {
        if let Some(c) = &mut self.0
            && matches!(c.try_wait(), Ok(None))
        {
            return;
        }
        let started = std::env::current_exe()
            .and_then(|exe| std::process::Command::new(exe).arg(flag).spawn());
        match started {
            Ok(c) => self.0 = Some(c),
            Err(e) => {
                error!("{flag}: {e}");
                snapling_win::warn(&tf("Не удалось открыть окно.\n{e}", &[("e", &e)]));
            }
        }
    }
}

/// Sound settings and pre-rendered WAVs; read from the saver thread too.
type Wavs = [&'static [u8]; 3];
static SOUND: Mutex<Option<(SoundCfg, Wavs)>> = Mutex::new(None);

thread_local! {
    /// `[capture] copy`: clipboard for hotkey captures (Enter in the overlay always copies).
    static COPY_ON_CAPTURE: Cell<bool> = const { Cell::new(true) };
    /// The language problem check ran at least once.
    static CHECKED_OCR: Cell<bool> = const { Cell::new(false) };
    /// `[capture] toast`.
    static TOAST: Cell<bool> = const { Cell::new(true) };
    /// `[capture] open_editor`.
    static EDITOR_ON_CAPTURE: Cell<bool> = const { Cell::new(false) };
    /// `[ocr] language`.
    static OCR_LANG: RefCell<String> = RefCell::new("auto".into());
    /// `[ocr] engine = "paddle"`.
    static OCR_PADDLE: Cell<bool> = const { Cell::new(false) };
    /// `[capture] delay`.
    static DELAY: Cell<u32> = const { Cell::new(3) };
    /// `[ocr] panel`.
    static OCR_PANEL: Cell<bool> = const { Cell::new(true) };
    /// What the last warning listed: the same problems are not shown again on every reload.
    static REPORTED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
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

/// Returns problems with the user's own files; those events fall back to the pack.
fn apply_sound(cfg: &SoundCfg) -> Vec<String> {
    let Ok(mut current) = SOUND.lock() else {
        return Vec::new();
    };
    let same = |c: &SoundCfg| {
        (
            c.volume,
            c.pack,
            &c.capture_file,
            &c.copy_file,
            &c.error_file,
        ) == (
            cfg.volume,
            cfg.pack,
            &cfg.capture_file,
            &cfg.copy_file,
            &cfg.error_file,
        )
    };
    let changed = current.as_ref().is_none_or(|(c, _)| !same(c));
    let mut problems = Vec::new();
    let wavs = match current.take() {
        Some((_, w)) if !changed => w,
        // ponytail: leaks ~40 KB per volume change (PlaySound reads the buffer after returning);
        // free old buffers after PlaySound(NULL) if volume becomes a live slider.
        _ => [Sound::Capture, Sound::Copy, Sound::Error].map(|s| {
            let wav = sound::load(cfg, s).unwrap_or_else(|e| {
                problems.push(tf("Звук: {e}", &[("e", &e)]));
                sound::wav(cfg.pack, s, cfg.volume)
            });
            &*Box::leak(wav.into_boxed_slice())
        }),
    };
    *current = Some((cfg.clone(), wavs));
    problems
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

fn menu_rows(cfg: &Config) -> Vec<MenuRow> {
    let act = |i, a: Action| {
        let keys = cfg.hotkeys.combo(a).trim().replace("PrintScreen", "PrtScn");
        MenuRow::Item(i, a.title().to_string(), keys)
    };
    let item = |i, s: &str| MenuRow::Item(i, s.to_string(), String::new());
    vec![
        act(MenuIcon::Region, Action::CaptureRegion),
        act(MenuIcon::Screen, Action::CaptureScreen),
        act(MenuIcon::Repeat, Action::RepeatRegion),
        act(MenuIcon::Delay, Action::DelayedRegion),
        act(MenuIcon::Text, Action::RecognizeText),
        MenuRow::Line,
        item(MenuIcon::History, t("Галерея")),
        item(MenuIcon::Folder, t("Папка снимков")),
        item(MenuIcon::Settings, t("Настройки")),
        MenuRow::Line,
        item(MenuIcon::Quit, t("Выход")),
    ]
}

fn menu_light(cfg: &Config) -> bool {
    match cfg.ui.theme {
        Theme::Light => true,
        Theme::Dark | Theme::Black => false,
        Theme::System => snapling_win::taskbar_is_light(),
    }
}

/// White mark for a dark panel, near-black for a light one, as `assets/brand/tray-*.svg`.
fn tray_white(mode: TrayIcon) -> bool {
    match mode {
        TrayIcon::Auto => !snapling_win::taskbar_is_light(),
        TrayIcon::White => true,
        TrayIcon::Black => false,
    }
}

fn tray_icon(white: bool) -> Icon {
    const N: u32 = 32;
    let frame = if white { [0xFF; 3] } else { [0x1A; 3] };
    let rgba = crate::icon::frame_spark(N, frame, [0xFF, 0x8A, 0x1F]);
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
                    text: String::new(),
                };
                if let Err(e) = history::append(&history_file(), &entry) {
                    warn!("history: {e}");
                }
            }
            Err(e) => {
                error!("save: {e}");
                play(Sound::Error);
                snapling_win::warn(&tf("Не удалось сохранить снимок.\n{e}", &[("e", &e)]));
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
    let copied = force_copy || COPY_ON_CAPTURE.get();
    if copied {
        copy(&img, t0);
    }
    let auto_save = SAVE.with(|s| s.borrow().as_ref().is_some_and(|(_, c)| c.auto_save));
    if TOAST.get() && !EDITOR_ON_CAPTURE.get() && !snapling_win::fullscreen_game() {
        notify(&img, &app, copied);
    }
    match (auto_save, EDITOR_ON_CAPTURE.get()) {
        (true, true) => {
            let copy = snapling_win::Image {
                bgra: img.bgra.clone(),
                ..img
            };
            save_async(copy, app.clone(), kind);
            open_editor(img, app);
        }
        (true, false) => save_async(img, app, kind),
        (false, true) => open_editor(img, app),
        (false, false) => {}
    }
}

/// Notification with the shot; its buttons get the image, so nothing stays behind after it.
fn notify(img: &snapling_win::Image, app: &str, copied: bool) {
    let shot = snapling_win::Image {
        bgra: img.bgra.clone(),
        ..*img
    };
    let title = if copied {
        t("Снимок скопирован")
    } else {
        t("Снимок готов")
    };
    let sub = if app.is_empty() {
        format!("{} × {}", img.width, img.height)
    } else {
        format!("{app} · {} × {}", img.width, img.height)
    };
    let app = app.to_string();
    snapling_win::toast::show(
        shot,
        title.to_string(),
        sub,
        Box::new(move |action, img| match action {
            ToastAction::Editor => open_editor(img, app),
            ToastAction::Text => recognize(img),
            ToastAction::Folder => open_shots_folder(),
        }),
    );
}

fn open_shots_folder() {
    let dir = SAVE.with(|s| s.borrow().as_ref().map(|(d, _)| d.clone()));
    match dir.map(|d| storage::prepare_dir(&d)) {
        Some(Ok(d)) => open_path(&d),
        Some(Err(e)) => snapling_win::warn(&e),
        None => snapling_win::warn(t("Сохранение отключено — проверь папку в настройках.")),
    }
}

/// OCR without a saved file still lands in the history: its text can be searched and copied.
fn remember_text(img: &snapling_win::Image, text: &str) {
    let entry = history::Entry {
        time: history::time_string(snapling_win::local_stamp()),
        path: String::new(),
        width: img.width,
        height: img.height,
        app: String::new(),
        kind: "ocr".into(),
        text: text.to_string(),
    };
    if let Err(e) = history::append(&history_file(), &entry) {
        warn!("history: {e}");
    }
}

fn edit_dir() -> PathBuf {
    app_dir("LOCALAPPDATA").join("edit")
}

fn models_dir() -> PathBuf {
    app_dir("LOCALAPPDATA").join("models")
}

/// Text of a shot. Missing PaddleOCR model → asks before downloading; refused → Windows OCR.
fn read_text(img: &snapling_win::Image, lang: &str, paddle: bool) -> Result<String, String> {
    use snapling_translate::paddle as engine;
    let models = models_dir();
    let windows = || snapling_win::ocr::recognize(&img.bgra, img.width, img.height, lang);
    if !paddle {
        return windows();
    }
    if !engine::installed(&models) {
        let q = tf(
            "Для PaddleOCR нужна модель, ~{0} МБ.
Скачать её с huggingface.co ({1})?

После скачивания распознавание работает без интернета.",
            &[("0", &engine::DOWNLOAD_MB), ("1", &engine::REPO)],
        );
        if !snapling_win::ask(&q) {
            return windows();
        }
        info!("model download: {}", engine::FOLDER);
        engine::download(&models)?;
    }
    engine::recognize(&models, &img.bgra, img.width, img.height)
}

/// OCR here, then the translation window process: it loads the model and frees it on close,
/// and offers the download when a model is missing.
fn translate(img: snapling_win::Image) {
    let lang = OCR_LANG.with(|l| l.borrow().clone());
    let paddle = OCR_PADDLE.get();
    std::thread::spawn(move || {
        let text = match read_text(&img, &lang, paddle) {
            Ok(t) if t.trim().is_empty() => {
                info!("translate: no text");
                return play(Sound::Error);
            }
            Ok(t) => t,
            Err(e) => {
                error!("ocr: {e}");
                play(Sound::Error);
                return snapling_win::warn(&tf(
                    "Не удалось распознать текст.
{e}",
                    &[("e", &e)],
                ));
            }
        };
        remember_text(&img, &text);
        text_window(&text, "--translate");
    });
}

/// The text window process; it deletes the file after reading.
fn text_window(text: &str, mode: &str) {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let path = edit_dir().join(format!("{nanos}.txt"));
    let started = std::fs::create_dir_all(edit_dir())
        .and_then(|()| std::fs::write(&path, text))
        .and_then(|()| std::env::current_exe())
        .and_then(|exe| std::process::Command::new(exe).arg(mode).arg(&path).spawn());
    if let Err(e) = started {
        let _ = std::fs::remove_file(&path);
        error!("translate window: {e}");
        snapling_win::warn(&tf(
            "Не удалось открыть окно перевода.
{e}",
            &[("e", &e)],
        ));
    }
}

/// Clipboard and models folder for the translation window process.
pub fn text_hooks() -> crate::translate_ui::Hooks {
    let cfg = Config::load_or_create(&app_dir("APPDATA").join("config.toml")).unwrap_or_default();
    crate::translate_ui::Hooks {
        copy_text: snapling_win::copy_text,
        warn: snapling_win::warn,
        models: models_dir(),
        online: cfg.translate.online,
        target: cfg.translate.target.clone(),
        engine: cfg.ocr.engine.name(),
        get_key: |s| snapling_win::secret::get(s.name()),
        set_key: |s, k| snapling_win::secret::set(s.name(), k),
        get_proxy: || snapling_win::secret::get(DEEPL_PROXY),
    }
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
                snapling_win::warn(&tf("Не удалось открыть редактор.\n{e}", &[("e", &e)]));
            }
        }
    });
}

fn remember_edit(path: &Path, width: u32, height: u32, app: &str) {
    let entry = history::Entry {
        time: history::time_string(snapling_win::local_stamp()),
        path: path.to_string_lossy().to_string(),
        width,
        height,
        app: app.into(),
        kind: "edit".into(),
        text: String::new(),
    };
    let _ = history::append(&history_file(), &entry);
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
            remember_edit(&path, w, h, app);
            Ok(path)
        },
        save_as: |bgra, w, h, app| {
            let cfg = Config::load_or_create(&app_dir("APPDATA").join("config.toml"))?.save;
            let name = storage::render_name(&cfg.name, snapling_win::local_stamp(), app);
            let name = name.file_name().unwrap_or_default().to_string_lossy();
            let Some(path) = snapling_win::save_file_dialog(&name) else {
                return Ok(None);
            };
            let ext = path.extension().unwrap_or_default().to_string_lossy();
            let format = match ext.to_lowercase().as_str() {
                "webp" => Format::Webp,
                "jpg" | "jpeg" => Format::Jpeg,
                _ => Format::Png,
            };
            let dir = path.parent().unwrap_or(Path::new("."));
            let file = Path::new(path.file_name().unwrap_or_default());
            let saved = storage::save_bgra(bgra, w, h, dir, file, format, cfg.jpeg_quality)?;
            remember_edit(&saved, w, h, app);
            Ok(Some(saved))
        },
        text: |bgra, width, height| {
            if let Ok(cfg) = Config::load_or_create(&app_dir("APPDATA").join("config.toml")) {
                OCR_LANG.with(|l| *l.borrow_mut() = cfg.ocr.language.trim().to_string());
            }
            translate(snapling_win::Image {
                x: 0,
                y: 0,
                width,
                height,
                bgra: bgra.to_vec(),
            });
        },
        warn: snapling_win::warn,
    }
}

fn open_path(path: &Path) {
    let _ = std::process::Command::new("explorer").arg(path).spawn();
}

/// `snapling --pin <file>`: a shot from the gallery pinned on its own.
pub fn run_pin(path: &Path) {
    let res = load_image(path).and_then(|mut img| {
        (img.x, img.y) = (120, 120);
        snapling_win::pin::run_alone(img)
    });
    if let Err(e) = res {
        snapling_win::warn(&e);
    }
}

/// Registry, folder dialog and key polling for the settings process.
pub fn settings_hooks() -> crate::settings::Hooks {
    crate::settings::Hooks {
        warn: snapling_win::warn,
        autostart: snapling_win::autostart,
        set_autostart: snapling_win::set_autostart,
        pick_folder: snapling_win::pick_folder,
        record_keys: snapling_win::record_keys,
        recorded_combo: snapling_win::recorded_combo,
        hotkey_free: snapling_win::hotkey_free,
        autostart_blocked: snapling_win::autostart_blocked,
        snipping_owns: snapling_win::snipping_owns_printscreen,
        free_printscreen: snapling_win::free_printscreen,
        models: models_dir(),
        get_proxy: || snapling_win::secret::get(DEEPL_PROXY),
        set_proxy: |url| {
            if url.is_empty() {
                snapling_win::secret::delete(DEEPL_PROXY);
                return Ok(());
            }
            snapling_translate::parse_proxy(url)?;
            snapling_win::secret::set(DEEPL_PROXY, url)
        },
        reserved: hotkey::reserved_on_windows,
        open: open_path,
        log: app_dir("LOCALAPPDATA").join("snapling.log"),
        default_dir: || snapling_win::pictures_dir().map(|p| p.join("Snapling")),
        pick_wav: snapling_win::pick_wav,
        pick_toml: snapling_win::pick_toml,
        save_toml: snapling_win::save_toml,
        preview: |cfg, s| {
            // PlaySound reads the buffer after returning; the window lives seconds to minutes.
            snapling_win::play_wav(Box::leak(sound::load(cfg, s)?.into_boxed_slice()));
            Ok(())
        },
    }
}

fn load_image(path: &Path) -> Result<snapling_win::Image, String> {
    let (bgra, width, height) = storage::load_bgra(path)?;
    Ok(snapling_win::Image {
        x: 0,
        y: 0,
        width,
        height,
        bgra,
    })
}

/// Clipboard, editor and folders for the history process.
pub fn history_hooks() -> crate::history_ui::Hooks {
    crate::history_ui::Hooks {
        warn: snapling_win::warn,
        file: history_file(),
        now: || history::time_string(snapling_win::local_stamp()),
        copy_image: |path| snapling_win::copy_image(&load_image(path)?),
        copy_text: snapling_win::copy_text,
        edit: |path, app| {
            open_editor(load_image(path)?, app.to_string());
            Ok(())
        },
        reveal: |path| {
            let _ = std::process::Command::new("explorer")
                .arg(format!("/select,{}", path.display()))
                .spawn();
        },
        pin: |path| {
            let started = std::env::current_exe().and_then(|exe| {
                std::process::Command::new(exe)
                    .arg("--pin")
                    .arg(path)
                    .spawn()
            });
            started.map(|_| ()).map_err(|e| e.to_string())
        },
        recognize: |path| {
            let cfg = Config::load_or_create(&app_dir("APPDATA").join("config.toml"))?;
            let paddle = cfg.ocr.engine == Engine::Paddle;
            read_text(&load_image(path)?, cfg.ocr.language.trim(), paddle)
        },
        open_folder: || {
            let cfg = Config::load_or_create(&app_dir("APPDATA").join("config.toml"));
            match cfg.and_then(|c| storage::prepare_dir(&save_dir(&c.save)?)) {
                Ok(d) => open_path(&d),
                Err(e) => snapling_win::warn(&e),
            }
        },
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

/// Selection overlay; `auto` runs that action as soon as something is selected.
fn capture_region(auto: Option<Outcome>) {
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
    let shown = snapling_win::overlay::show(img, auto, move |res| {
        let Some((img, outcome)) = res else { return };
        if outcome == Outcome::Color {
            let hex = snapling_scene::color_hex(&img.bgra);
            info!("color {hex}");
            return match snapling_win::copy_text(&hex) {
                Ok(()) => play(Sound::Copy),
                Err(e) => {
                    error!("clipboard: {e}");
                    play(Sound::Error);
                }
            };
        }
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
            Outcome::Text => recognize(img),
            Outcome::Translate => translate(img),
            Outcome::Pin => {
                play(Sound::Capture);
                if let Err(e) = snapling_win::pin::show(img) {
                    error!("pin: {e}");
                    play(Sound::Error);
                }
            }
            Outcome::Color => {}
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
        Action::CaptureRegion => capture_region(None),
        Action::DelayedRegion => {
            snapling_win::countdown::start(DELAY.get(), hotkey_id(Action::CaptureRegion));
        }
        Action::RecognizeText => capture_region(Some(Outcome::Text)),
        Action::TranslateText => capture_region(Some(Outcome::Translate)),
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
            None => capture_region(None),
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
        None => Err(t("папка «Изображения» не найдена — укажи [save] dir").to_string()),
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
            vec![tf("Сохранение отключено: {e}", &[("e", &e)])]
        }
    }
}

fn apply_config(cfg: &Config) {
    let mut problems = apply_hotkeys(cfg);
    problems.extend(apply_save(cfg));
    problems.extend(apply_sound(&cfg.sound));
    COPY_ON_CAPTURE.set(cfg.capture.copy);
    EDITOR_ON_CAPTURE.set(cfg.capture.open_editor);
    TOAST.set(cfg.capture.toast);
    snapling_win::overlay::set_instant_copy(cfg.capture.instant_copy);
    snapling_win::overlay::set_loupe(cfg.capture.loupe);
    DELAY.set(cfg.capture.delay.clamp(1, 60));
    snapling_win::overlay::set_blur(cfg.blur.gauss, cfg.blur.strength.min(2));
    apply_ocr(&cfg.ocr.language);
    OCR_PADDLE.set(cfg.ocr.engine == Engine::Paddle);
    OCR_PANEL.set(cfg.ocr.panel);
    let hide = cfg.privacy.hide_from_capture;
    match snapling_win::overlay::set_hidden_from_capture(hide) {
        Ok(()) => info!("overlay hidden from capture: {hide}"),
        Err(e) if hide => problems.push(tf(
            "Скрыть выделение от демонстрации экрана не удалось (нужна Windows 10 2004+): {e}",
            &[("e", &e)],
        )),
        Err(e) => warn!("display affinity: {e}"),
    }
    report(&problems);
}

/// Checks the language off the UI thread: WinRT must not change this thread's COM apartment.
fn apply_ocr(lang: &str) {
    let lang = lang.trim().to_string();
    let same = OCR_LANG.with(|l| l.replace(lang.clone())) == lang;
    if same && CHECKED_OCR.replace(true) {
        return;
    }
    std::thread::spawn(move || {
        info!("ocr languages: {:?}", snapling_win::ocr::languages());
        if let Some(p) = snapling_win::ocr::language_problem(&lang) {
            warn!("{p}");
            snapling_win::warn(&tf(
                "Настройки:
{p}",
                &[("p", &p)],
            ));
        }
    });
}

/// OCR on a worker thread, text to the clipboard and the text window.
fn recognize(img: snapling_win::Image) {
    let lang = OCR_LANG.with(|l| l.borrow().clone());
    let (paddle, panel) = (OCR_PADDLE.get(), OCR_PANEL.get());
    std::thread::spawn(move || {
        let t0 = Instant::now();
        match read_text(&img, &lang, paddle) {
            Ok(text) if text.trim().is_empty() => {
                info!("ocr: no text");
                play(Sound::Error);
            }
            Ok(text) => {
                info!(
                    "ocr: {} chars in {} ms",
                    text.chars().count(),
                    t0.elapsed().as_millis()
                );
                remember_text(&img, &text);
                match snapling_win::copy_text(&text) {
                    Ok(()) => play(Sound::Copy),
                    Err(e) => {
                        error!("clipboard: {e}");
                        play(Sound::Error);
                    }
                }
                if panel {
                    text_window(&text, "--text");
                }
            }
            Err(e) => {
                error!("ocr: {e}");
                play(Sound::Error);
                snapling_win::warn(&tf(
                    "Не удалось распознать текст.
{e}",
                    &[("e", &e)],
                ));
            }
        }
    });
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
                t(
                    "PrtScn сейчас открывает «Ножницы» Windows. Исправить: настройки Snapling → \
                 Горячие клавиши.",
                )
                .into(),
            );
        }
    }
    problems
}

fn report(problems: &[String]) {
    if REPORTED.with(|r| r.replace(problems.to_vec())) == problems {
        return;
    }
    if problems.is_empty() {
        return;
    }
    for p in problems {
        warn!("config: {p}");
    }
    let list: Vec<String> = problems.iter().map(|p| format!("• {p}")).collect();
    snapling_win::warn(&tf(
        "Настройки:\n\n{0}\n\nИзменить: меню в трее → «Настройки».",
        &[("0", &list.join("\n"))],
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
    if cfg!(not(debug_assertions)) {
        snapling_win::refresh_autostart();
    }
    info!(
        "{} {} start",
        snapling_core::APP_NAME,
        snapling_core::VERSION
    );

    let cfg_dir = app_dir("APPDATA");
    let cfg_path = cfg_dir.join("config.toml");
    let mut cfg = Config::load_or_create(&cfg_path).unwrap_or_else(|e| {
        snapling_win::warn(&tf(
            "Ошибка в config.toml, используются настройки по умолчанию.\n{e}",
            &[("e", &e)],
        ));
        Config::default()
    });
    let mut cfg_mtime = modified(&cfg_path);
    if let Err(e) = snapling_win::overlay::init() {
        error!("overlay init: {e}");
    }
    if let Err(e) = snapling_win::toast::init() {
        error!("toast init: {e}");
    }
    if let Err(e) = snapling_win::menu::init() {
        error!("menu init: {e}");
    }
    if let Err(e) = snapling_win::countdown::init() {
        error!("countdown init: {e}");
    }
    apply_config(&cfg);
    let now = history::time_string(snapling_win::local_stamp());
    match history::prune(&history_file(), &now, cfg.history.keep_days) {
        Ok(old) if old.is_empty() => {}
        Ok(old) => {
            info!("gallery: {} old entries dropped", old.len());
            let files: Vec<String> = old
                .into_iter()
                .map(|e| e.path)
                .filter(|p| !p.is_empty() && Path::new(p).is_file())
                .collect();
            if cfg.history.delete_files && !files.is_empty() {
                std::thread::spawn(move || match snapling_win::to_recycle_bin(&files) {
                    Ok(()) => info!("gallery: {} files to the recycle bin", files.len()),
                    Err(e) => warn!("gallery: {e}"),
                });
            }
        }
        Err(e) => warn!("gallery: {e}"),
    }

    let mut tray_mode = cfg.ui.tray_icon;
    let mut white = tray_white(tray_mode);
    let tray = match TrayIconBuilder::new()
        .with_tooltip(snapling_core::APP_NAME)
        .with_icon(tray_icon(white))
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            error!("tray: {e}");
            snapling_win::warn(&tf("Не удалось создать значок в трее: {e}", &[("e", &e)]));
            return;
        }
    };
    unsafe { SetTimer(None, 0, CONFIG_POLL_MS, None) };
    let mut settings_window = Window::default();
    let mut history_window = Window::default();
    let mut welcome_window = Window::default();
    if !cfg.ui.welcomed {
        welcome_window.open("--welcome");
        if let Err(e) = Config::update(&cfg_path, "ui", "welcomed", true) {
            warn!("welcome: {e}");
        }
    }

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
                let want = tray_white(tray_mode);
                if want != white {
                    white = want;
                    let _ = tray.set_icon(Some(tray_icon(white)));
                }
                let now = modified(&cfg_path);
                if now != cfg_mtime {
                    cfg_mtime = now;
                    match Config::load_or_create(&cfg_path) {
                        Ok(new) => {
                            info!("config reloaded");
                            crate::ui::apply_language(&new.ui.language);
                            apply_config(&new);
                            tray_mode = new.ui.tray_icon;
                            cfg = new;
                        }
                        Err(e) => {
                            warn!("config: {e}");
                            snapling_win::warn(&tf(
                                "Ошибка в config.toml, оставлены прежние настройки.\n{e}",
                                &[("e", &e)],
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
        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click {
                button: MouseButton::Left | MouseButton::Right,
                button_state: MouseButtonState::Up,
                ..
            } = ev
            {
                snapling_win::menu::show(menu_rows(&cfg), menu_light(&cfg));
            }
        }
        match snapling_win::menu::picked() {
            Some(MenuIcon::Region) => run_action(Action::CaptureRegion),
            Some(MenuIcon::Screen) => run_action(Action::CaptureScreen),
            Some(MenuIcon::Repeat) => run_action(Action::RepeatRegion),
            Some(MenuIcon::Delay) => run_action(Action::DelayedRegion),
            Some(MenuIcon::Text) => run_action(Action::RecognizeText),
            Some(MenuIcon::History) => history_window.open("--history"),
            Some(MenuIcon::Folder) => open_shots_folder(),
            Some(MenuIcon::Settings) => settings_window.open("--settings"),
            Some(MenuIcon::Quit) => unsafe { PostQuitMessage(0) },
            None => {}
        }
    }
    info!("exit");
}
