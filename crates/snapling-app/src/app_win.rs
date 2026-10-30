use crate::app_dir;
use snapling_core::config::{Action, Config, Format, Save, SoundCfg};
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
                snapling_win::warn(&format!("Не удалось открыть окно.\n{e}"));
            }
        }
    }
}

/// Set by the background saver.
static LAST_SAVED: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Sound settings and pre-rendered WAVs; read from the saver thread too.
type Wavs = [&'static [u8]; 3];
static SOUND: Mutex<Option<(SoundCfg, Wavs)>> = Mutex::new(None);

thread_local! {
    /// `[capture] copy`: clipboard for hotkey captures (Enter in the overlay always copies).
    static COPY_ON_CAPTURE: Cell<bool> = const { Cell::new(true) };
    /// The language problem check ran at least once.
    static CHECKED_OCR: Cell<bool> = const { Cell::new(false) };
    /// `[capture] open_editor`.
    static EDITOR_ON_CAPTURE: Cell<bool> = const { Cell::new(false) };
    /// `[ocr] language`.
    static OCR_LANG: RefCell<String> = RefCell::new("auto".into());
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
                problems.push(format!("Звук: {e}"));
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
                    text: String::new(),
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
    let auto_save = SAVE.with(|s| s.borrow().as_ref().is_some_and(|(_, c)| c.auto_save));
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

/// OCR here, then the translation window process (it loads the model and frees it on close).
/// Missing model → asks before downloading: the only network access of offline translation.
fn translate(img: snapling_win::Image) {
    let lang = OCR_LANG.with(|l| l.borrow().clone());
    std::thread::spawn(move || {
        let text = match snapling_win::ocr::recognize(&img.bgra, img.width, img.height, &lang) {
            Ok(t) if t.trim().is_empty() => {
                info!("translate: no text");
                return play(Sound::Error);
            }
            Ok(t) => t,
            Err(e) => {
                error!("ocr: {e}");
                play(Sound::Error);
                return snapling_win::warn(&format!(
                    "Не удалось распознать текст.
{e}"
                ));
            }
        };
        remember_text(&img, &text);
        let dir = snapling_translate::detect(&text);
        let models = models_dir();
        if !snapling_translate::installed(&models, dir) {
            let q = format!(
                "Для офлайн-перевода ({}) нужна модель OPUS-MT, ~{} МБ.
Скачать её с huggingface.co (Xenova/{})?

После скачивания перевод работает без интернета.",
                dir.label(),
                snapling_translate::DOWNLOAD_MB,
                dir.model()
            );
            if !snapling_win::ask(&q) {
                return;
            }
            let t0 = Instant::now();
            info!("model download: {}", dir.model());
            if let Err(e) = snapling_translate::download(&models, dir) {
                error!("model download: {e}");
                play(Sound::Error);
                return snapling_win::warn(&format!(
                    "Не удалось скачать модель перевода.
{e}"
                ));
            }
            info!("model downloaded in {} s", t0.elapsed().as_secs());
        }
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = edit_dir().join(format!("{nanos}.txt"));
        let started = std::fs::create_dir_all(edit_dir())
            .and_then(|()| std::fs::write(&path, &text))
            .and_then(|()| std::env::current_exe())
            .and_then(|exe| {
                std::process::Command::new(exe)
                    .arg("--translate")
                    .arg(&path)
                    .spawn()
            });
        if let Err(e) = started {
            let _ = std::fs::remove_file(&path);
            error!("translate window: {e}");
            snapling_win::warn(&format!(
                "Не удалось открыть окно перевода.
{e}"
            ));
        }
    });
}

/// Clipboard and models folder for the translation window process.
pub fn text_hooks() -> crate::translate_ui::Hooks {
    crate::translate_ui::Hooks {
        copy_text: snapling_win::copy_text,
        warn: snapling_win::warn,
        models: models_dir(),
        online: Config::load_or_create(&app_dir("APPDATA").join("config.toml"))
            .is_ok_and(|c| c.translate.online == snapling_core::config::Online::Deepl),
        get_key: || snapling_win::secret::get("DeepL"),
        set_key: |k| snapling_win::secret::set("DeepL", k),
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
                snapling_win::warn(&format!("Не удалось открыть редактор.\n{e}"));
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

/// Registry, folder dialog and key polling for the settings process.
pub fn settings_hooks() -> crate::settings::Hooks {
    crate::settings::Hooks {
        warn: snapling_win::warn,
        autostart: snapling_win::autostart,
        set_autostart: snapling_win::set_autostart,
        pick_folder: snapling_win::pick_folder,
        pressed_combo: snapling_win::pressed_combo,
        pause_hotkeys: snapling_win::pause_hotkeys,
        reserved: hotkey::reserved_on_windows,
        open: open_path,
        log: app_dir("LOCALAPPDATA").join("snapling.log"),
        default_dir: || snapling_win::pictures_dir().map(|p| p.join("Snapling")),
        pick_wav: snapling_win::pick_wav,
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
        open: open_path,
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
            Outcome::Text => recognize(img),
            Outcome::Translate => translate(img),
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
    problems.extend(apply_sound(&cfg.sound));
    COPY_ON_CAPTURE.set(cfg.capture.copy);
    EDITOR_ON_CAPTURE.set(cfg.capture.open_editor);
    snapling_win::overlay::set_instant_copy(cfg.capture.instant_copy);
    apply_ocr(&cfg.ocr.language);
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
            snapling_win::warn(&format!(
                "Настройки:
{p}"
            ));
        }
    });
}

/// OCR on a worker thread, text to the clipboard.
fn recognize(img: snapling_win::Image) {
    let lang = OCR_LANG.with(|l| l.borrow().clone());
    std::thread::spawn(move || {
        let t0 = Instant::now();
        match snapling_win::ocr::recognize(&img.bgra, img.width, img.height, &lang) {
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
            }
            Err(e) => {
                error!("ocr: {e}");
                play(Sound::Error);
                snapling_win::warn(&format!(
                    "Не удалось распознать текст.
{e}"
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
                "PrtScn сейчас открывает «Ножницы» Windows. Выключить: Параметры → \
                 Специальные возможности → Клавиатура → «Использовать клавишу PrtScn…»."
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
    snapling_win::warn(&format!(
        "Настройки:\n\n{}\n\nИзменить: меню в трее → «Настройки».",
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
    let history_item = MenuItem::new("История", true, None);
    let settings_item = MenuItem::new("Настройки", true, None);
    let quit = MenuItem::new("Выход", true, None);
    let menu = Menu::new();
    let _ = menu.append_items(&[
        &region,
        &screen,
        &PredefinedMenuItem::separator(),
        &last,
        &history_item,
        &folder,
        &settings_item,
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
    let mut settings_window = Window::default();
    let mut history_window = Window::default();

    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
        match msg.message {
            WM_HOTKEY if snapling_win::hotkeys_paused() => {}
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
                    None => {
                        snapling_win::warn("Сохранение отключено — проверь папку в настройках.")
                    }
                }
            } else if ev.id == settings_item.id() {
                settings_window.open("--settings");
            } else if ev.id == history_item.id() {
                history_window.open("--history");
            } else if ev.id == quit.id() {
                unsafe { PostQuitMessage(0) };
            }
        }
    }
    info!("exit");
}
