//! Linux and macOS. No process waits for keys: compositors bind a key to `snapling --region`,
//! so every action is one short run; the tray only starts such runs.

use crate::app_dir;
use snapling_core::config::{Action, Config, Engine, Format, RecordFormat, Save};
use snapling_core::i18n::{t, tf};
use snapling_core::record::{Added, Stitcher};
use snapling_core::selection::Rect;
use snapling_core::sound::{self, Sound};
use snapling_core::storage::{self, Stamp};
use snapling_core::{annotate, history};
use snapling_overlay::{Options, Outcome, Shot};
use snapling_translate::paddle;
use snapling_unix as unix;
use std::path::{Path, PathBuf};

fn config() -> Config {
    Config::load_or_create(&crate::ui::config_path()).unwrap_or_default()
}

fn history_file() -> PathBuf {
    app_dir("LOCALAPPDATA").join("history.jsonl")
}

fn models_dir() -> PathBuf {
    app_dir("LOCALAPPDATA").join("models")
}

fn temp_dir() -> PathBuf {
    app_dir("LOCALAPPDATA").join("edit")
}

fn play(cfg: &Config, s: Sound) {
    if cfg.sound.enabled
        && let Ok(wav) = sound::load(&cfg.sound, s)
    {
        let _ = unix::play(&wav);
    }
}

fn save_dir(save: &Save) -> Result<PathBuf, String> {
    let dir = match save.dir.trim() {
        "" => unix::pictures_dir().map(|p| p.join("Snapling")),
        d => Some(PathBuf::from(d)),
    }
    .ok_or(t("папка «Изображения» не найдена — укажи [save] dir"))?;
    match storage::shape_error(&dir) {
        Some(e) => Err(e),
        None => Ok(dir),
    }
}

/// Local time; `date` saves a timezone table in here.
fn stamp() -> Stamp {
    let text = std::process::Command::new("date")
        .arg("+%Y %m %d %H %M %S")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    let mut parts = text
        .split_whitespace()
        .map(|p| p.parse::<u16>().unwrap_or(0));
    let mut next = || parts.next().unwrap_or(0);
    Stamp {
        year: next(),
        month: next() as u8,
        day: next() as u8,
        hour: next() as u8,
        minute: next() as u8,
        second: next() as u8,
    }
}

fn remember(path: &Path, width: u32, height: u32, kind: &str, text: &str) {
    let entry = history::Entry::new(
        history::time_string(stamp()),
        &path.to_string_lossy(),
        (width, height),
        "",
        kind,
        text,
    );
    let _ = history::append(&history_file(), &entry);
}

fn save(cfg: &Config, img: &Shot, kind: &str) -> Result<PathBuf, String> {
    let dir = storage::prepare_dir(&save_dir(&cfg.save)?)?;
    let name = storage::render_name(&cfg.save.name, stamp(), "");
    let path = storage::save_bgra(
        &img.bgra,
        img.width,
        img.height,
        &dir,
        &name,
        cfg.save.format,
        cfg.save.jpeg_quality,
    )?;
    remember(&path, img.width, img.height, kind, "");
    Ok(path)
}

fn copy_shot(bgra: &[u8], width: u32, height: u32) -> Result<(), String> {
    unix::copy_png(&storage::encode_png(bgra, width, height)?)
}

fn fail(cfg: &Config, e: &str) {
    play(cfg, Sound::Error);
    unix::warn(e);
}

/// What the usual shot does: clipboard, the sound, the folder, the notification.
fn deliver(cfg: &Config, img: &Shot, kind: &str, force_copy: bool) {
    play(cfg, Sound::Capture);
    let copied = force_copy || cfg.capture.copy;
    if copied && let Err(e) = copy_shot(&img.bgra, img.width, img.height) {
        return fail(cfg, &e);
    }
    let saved = cfg.save.auto_save.then(|| save(cfg, img, kind));
    if let Some(Err(e)) = &saved {
        return fail(cfg, e);
    }
    if cfg.capture.toast {
        let body = saved
            .and_then(Result::ok)
            .map_or(String::new(), |p| p.display().to_string());
        let title = if copied {
            t("Снимок скопирован")
        } else {
            t("Снимок сохранён")
        };
        unix::notify(title, &body);
    }
    if cfg.capture.open_editor {
        open_editor(img);
    }
}

fn spawn_self(flag: &str, path: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let started = std::process::Command::new(exe).arg(flag).arg(path).spawn();
    started.map(|_| ()).map_err(|e| e.to_string())
}

fn temp_file(extension: &str) -> Result<PathBuf, String> {
    let dir = temp_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    Ok(dir.join(format!("{nanos}.{extension}")))
}

fn open_editor(img: &Shot) {
    let opened = temp_file("raw").and_then(|path| {
        annotate::write_raw(&path, &img.bgra, img.width, img.height, "")
            .map_err(|e| e.to_string())?;
        spawn_self("--edit", &path)
    });
    if let Err(e) = opened {
        unix::warn(&e);
    }
}

fn pin(img: &Shot) -> Result<(), String> {
    let path = temp_file("png")?;
    std::fs::write(
        &path,
        storage::encode_png(&img.bgra, img.width, img.height)?,
    )
    .map_err(|e| e.to_string())?;
    spawn_self("--pin", &path)
}

/// `snapling --pin <file>`: blocks until the pin is closed; a file from `temp_dir` then goes.
pub fn run_pin(path: &Path) {
    let res = storage::load_bgra(path).and_then(|(bgra, width, height)| {
        snapling_overlay::pin(Shot {
            x: 0,
            y: 0,
            width,
            height,
            bgra,
        })
    });
    if path.starts_with(temp_dir()) {
        let _ = std::fs::remove_file(path);
    }
    if let Err(e) = res {
        unix::warn(&e);
    }
}

/// OCR has one engine here: PaddleOCR, with the model asked for on first use.
fn read_text(bgra: &[u8], width: u32, height: u32) -> Result<String, String> {
    let models = models_dir();
    if !paddle::installed(&models) {
        let q = tf(
            "Для PaddleOCR нужна модель, ~{0} МБ.\nСкачать её с huggingface.co ({1})?\n\nПосле скачивания распознавание работает без интернета.",
            &[("0", &paddle::DOWNLOAD_MB), ("1", &paddle::REPO)],
        );
        if !unix::ask(&q) {
            return Ok(String::new());
        }
        paddle::download(&models, unix::secret_get(PROXY).as_deref())?;
    }
    paddle::recognize(&models, bgra, width, height)
}

fn recognize(cfg: &Config, img: &Shot, translate: bool) {
    let text = match read_text(&img.bgra, img.width, img.height) {
        Ok(t) if t.trim().is_empty() => return play(cfg, Sound::Error),
        Ok(t) => t,
        Err(e) => return fail(cfg, &tf("Не удалось распознать текст.\n{e}", &[("e", &e)])),
    };
    remember(Path::new(""), img.width, img.height, "ocr", &text);
    if !translate {
        if let Err(e) = unix::copy_text(&text) {
            return fail(cfg, &e);
        }
        play(cfg, Sound::Copy);
        if !cfg.ocr.panel {
            return;
        }
    }
    let flag = if translate { "--translate" } else { "--text" };
    let opened = temp_file("txt").and_then(|path| {
        std::fs::write(&path, &text).map_err(|e| e.to_string())?;
        spawn_self(flag, &path)
    });
    if let Err(e) = opened {
        unix::warn(&e);
    }
}

fn area_of(img: &Shot) -> Rect {
    Rect {
        left: img.x,
        top: img.y,
        width: img.width as i32,
        height: img.height as i32,
    }
}

fn start_recording(cfg: &Config, cap: &unix::Capture, img: &Shot) -> Result<(), String> {
    let dir = storage::prepare_dir(&save_dir(&cfg.save)?)?;
    let name = storage::render_name(&cfg.save.name, stamp(), "");
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let (video, gif) = match cfg.record.format {
        RecordFormat::Mp4 => (storage::unique(path.with_extension("mp4")), None),
        RecordFormat::Gif => (
            temp_file("mp4")?,
            Some(storage::unique(path.with_extension("gif"))),
        ),
    };
    unix::start_recording(
        area_of(img),
        cap.origin,
        cap.scale,
        cfg.record.fps,
        &video,
        gif,
    )?;
    play(cfg, Sound::Capture);
    unix::notify(
        t("Запись экрана"),
        t("Идёт запись. Нажми то же сочетание, чтобы остановить."),
    );
    Ok(())
}

fn stop_recording(cfg: &Config, run: unix::Running) {
    match unix::stop_recording(run) {
        Ok(path) => {
            remember(&path, 0, 0, "record", "");
            if cfg.capture.copy {
                let _ = unix::copy_file(&path);
            }
            play(cfg, Sound::Copy);
            unix::notify(t("Запись экрана"), &path.display().to_string());
        }
        Err(e) => fail(cfg, &tf("Запись не удалась.\n{e}", &[("e", &e)])),
    }
}

const SCROLL_STEPS: usize = 200;
const SCROLL_WAIT: std::time::Duration = std::time::Duration::from_millis(220);
const MAX_ROWS: usize = 32_000;

fn long_shot(cap: &unix::Capture, img: &Shot) -> Result<Shot, String> {
    let area = area_of(img);
    let (origin, scale) = (cap.origin, cap.scale);
    let first = unix::capture_area(area, origin, scale)?;
    let mut stitched = Stitcher::new(&first, img.width, img.height);
    for _ in 0..SCROLL_STEPS {
        if stitched.rows() >= MAX_ROWS {
            break;
        }
        unix::scroll_down(area, origin, scale)?;
        std::thread::sleep(SCROLL_WAIT);
        let frame = unix::capture_area(area, origin, scale)?;
        if !matches!(stitched.add(&frame), Added::Grew(_)) {
            break;
        }
    }
    let (bgra, width, height) = stitched.finish();
    Ok(Shot {
        x: img.x,
        y: img.y,
        width,
        height,
        bgra,
    })
}

fn after(cfg: &Config, cap: &unix::Capture, img: Shot, outcome: Outcome) -> Result<(), String> {
    match outcome {
        Outcome::Copy => deliver(cfg, &img, "region", true),
        Outcome::Save => {
            play(cfg, Sound::Capture);
            save(cfg, &img, "region")?;
        }
        Outcome::Edit => open_editor(&img),
        Outcome::Text => recognize(cfg, &img, false),
        Outcome::Translate => recognize(cfg, &img, true),
        Outcome::Pin => {
            play(cfg, Sound::Capture);
            pin(&img)?;
        }
        Outcome::Record => start_recording(cfg, cap, &img)?,
        Outcome::Scroll => {
            let long = long_shot(cap, &img)?;
            deliver(cfg, &long, "long", false);
        }
        Outcome::Color => {
            let hex = format!("#{:02X}{:02X}{:02X}", img.bgra[2], img.bgra[1], img.bgra[0]);
            unix::copy_text(&hex)?;
            play(cfg, Sound::Copy);
        }
    }
    Ok(())
}

fn region(cfg: &Config, auto: Option<Outcome>) -> Result<(), String> {
    let cap = unix::capture()?;
    let opts = Options {
        scale: 0.0,
        auto,
        instant: cfg.capture.instant_copy,
        loupe: cfg.capture.loupe,
        blur: annotate::BlurStyle {
            gauss: cfg.blur.gauss,
            strength: cfg.blur.strength.min(2),
        },
        targets: cap.targets.clone(),
    };
    match snapling_overlay::run(cap.shot.clone(), opts)? {
        Some((img, outcome)) => after(cfg, &cap, img, outcome),
        None => Ok(()),
    }
}

/// Runs one action of a flag; `false`: not an action flag.
pub fn run_flag(flag: &str) -> bool {
    let cfg = config();
    let res = match flag {
        "--region" => region(&cfg, None),
        "--screen" => unix::capture().map(|cap| deliver(&cfg, &cap.shot, "screen", false)),
        "--ocr" => region(&cfg, Some(Outcome::Text)),
        "--ocr-translate" => region(&cfg, Some(Outcome::Translate)),
        "--long" => region(&cfg, Some(Outcome::Scroll)),
        "--delay" => {
            std::thread::sleep(std::time::Duration::from_secs(u64::from(
                cfg.capture.delay.clamp(1, 60),
            )));
            region(&cfg, None)
        }
        "--record" => match unix::recording() {
            Some(run) => {
                stop_recording(&cfg, run);
                Ok(())
            }
            None => region(&cfg, Some(Outcome::Record)),
        },
        _ => return false,
    };
    if let Err(e) = res {
        fail(&cfg, &e);
    }
    true
}

/// Colors of a 32×32 RGBA icon as the ARGB the tray takes.
#[cfg(target_os = "linux")]
fn argb(rgba: &[u8]) -> Vec<u8> {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[3], p[0], p[1], p[2]])
        .collect()
}

/// No flag: the tray where there is one. It also opens the welcome screen once.
pub fn run_tray() {
    let cfg = config();
    let now = history::time_string(stamp());
    if let Ok(old) = history::prune(&history_file(), &now, cfg.history.keep_days)
        && cfg.history.delete_files
    {
        let files: Vec<&str> = old
            .iter()
            .map(|e| e.path.as_str())
            .filter(|p| !p.is_empty())
            .collect();
        unix::to_trash(&files);
    }
    if !cfg.ui.welcomed {
        let path = crate::ui::config_path();
        let _ = Config::update(&path, "ui", "welcomed", true);
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe).arg("--welcome").spawn();
        }
    }
    #[cfg(target_os = "linux")]
    {
        let row = |a: Action, flag: &'static str| Some((a.title().to_string(), flag));
        let rows = vec![
            row(Action::CaptureRegion, "--region"),
            row(Action::CaptureScreen, "--screen"),
            row(Action::DelayedRegion, "--delay"),
            row(Action::Record, "--record"),
            row(Action::LongShot, "--long"),
            row(Action::RecognizeText, "--ocr"),
            None,
            Some((t("Галерея").to_string(), "--history")),
            Some((t("Настройки").to_string(), "--settings")),
            None,
            Some((t("Выход").to_string(), "--quit")),
        ];
        let icon = crate::icon::frame_spark(32, [0xFF; 3], [0xFF, 0x8A, 0x1F]);
        if let Err(e) = snapling_unix::tray::run_forever(rows, argb(&icon)) {
            unix::warn(&e);
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = Action::ALL;
        eprintln!("snapling --region | --screen | --ocr | --record | --long | --settings");
    }
}

pub fn editor_hooks() -> crate::editor::Hooks {
    crate::editor::Hooks {
        copy: copy_shot,
        save: |bgra, w, h, app| {
            let cfg = config().save;
            let dir = storage::prepare_dir(&save_dir(&cfg)?)?;
            let name = storage::render_name(&cfg.name, stamp(), app);
            let path = storage::save_bgra(bgra, w, h, &dir, &name, cfg.format, cfg.jpeg_quality)?;
            remember(&path, w, h, "edit", "");
            Ok(path)
        },
        save_as: |bgra, w, h, app| {
            let cfg = config().save;
            let name = storage::render_name(&cfg.name, stamp(), app);
            let name = name.file_name().unwrap_or_default().to_string_lossy();
            let Some(path) = unix::save_file(&format!("{name}.png")) else {
                return Ok(None);
            };
            let format = match path.extension().and_then(|e| e.to_str()) {
                Some("webp") => Format::Webp,
                Some("jpg" | "jpeg") => Format::Jpeg,
                _ => Format::Png,
            };
            let dir = path.parent().unwrap_or(Path::new("."));
            let file = Path::new(path.file_name().unwrap_or_default());
            let saved = storage::save_bgra(bgra, w, h, dir, file, format, cfg.jpeg_quality)?;
            remember(&saved, w, h, "edit", "");
            Ok(Some(saved))
        },
        text: |bgra, width, height| {
            let img = Shot {
                x: 0,
                y: 0,
                width,
                height,
                bgra: bgra.to_vec(),
            };
            std::thread::spawn(move || recognize(&config(), &img, true));
        },
        warn: unix::warn,
    }
}

const PROXY: &str = "proxy";

pub fn settings_hooks() -> crate::settings::Hooks {
    crate::settings::Hooks {
        warn: unix::warn,
        autostart: unix::autostart,
        set_autostart: unix::set_autostart,
        pick_folder: unix::pick_folder,
        record_keys: |_| {},
        recorded_combo: || None,
        hotkey_free: |_| true,
        autostart_blocked: || false,
        snipping_owns: || false,
        free_printscreen: || Ok(()),
        models: models_dir(),
        can_update: false,
        ask: unix::ask,
        run_update: |_| Ok(()),
        get_proxy: || unix::secret_get(PROXY),
        set_proxy: |url| {
            if !url.is_empty() {
                snapling_translate::parse_proxy(url)?;
            }
            unix::secret_set(PROXY, url)
        },
        reserved: |_| None,
        open: unix::open,
        pick_toml: || unix::pick_file("toml"),
        save_toml: || unix::save_file("snapling-settings.toml"),
        log: PathBuf::new(),
        default_dir: || unix::pictures_dir().map(|p| p.join("Snapling")),
        pick_wav: || unix::pick_file("wav"),
        preview: |cfg, s| unix::play(&sound::load(cfg, s)?),
    }
}

pub fn history_hooks() -> crate::history_ui::Hooks {
    crate::history_ui::Hooks {
        warn: unix::warn,
        file: history_file(),
        now: || history::time_string(stamp()),
        copy_image: |path| {
            let (bgra, w, h) = storage::load_bgra(path)?;
            copy_shot(&bgra, w, h)
        },
        copy_text: unix::copy_text,
        copy_file: unix::copy_file,
        edit: |path, _| {
            let (bgra, width, height) = storage::load_bgra(path)?;
            open_editor(&Shot {
                x: 0,
                y: 0,
                width,
                height,
                bgra,
            });
            Ok(())
        },
        reveal: unix::reveal,
        pin: |path| spawn_self("--pin", path),
        recognize: |path| {
            let (bgra, w, h) = storage::load_bgra(path)?;
            read_text(&bgra, w, h)
        },
        open_folder: || match save_dir(&config().save).and_then(|d| storage::prepare_dir(&d)) {
            Ok(d) => unix::open(&d),
            Err(e) => unix::warn(&e),
        },
    }
}

pub fn text_hooks() -> crate::translate_ui::Hooks {
    let cfg = config();
    crate::translate_ui::Hooks {
        copy_text: unix::copy_text,
        warn: unix::warn,
        models: models_dir(),
        online: cfg.translate.online,
        target: cfg.translate.target.clone(),
        engine: Engine::Paddle.name(),
        get_key: |s| unix::secret_get(s.name()),
        set_key: |s, k| unix::secret_set(s.name(), k),
        get_proxy: || unix::secret_get(PROXY),
    }
}
