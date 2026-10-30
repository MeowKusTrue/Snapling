use crate::i18n::t;
use serde::Deserialize;
use std::path::Path;

/// Written on first start; the comments are the hotkey documentation for now.
pub const TEMPLATE: &str = r#"# Snapling — настройки. Изменения применяются сразу после сохранения файла.

[hotkeys]
# Формат: модификаторы + клавиша через «+». Модификаторы: Ctrl, Alt, Shift, Win.
# Клавиши: A–Z, 0–9, F1–F24, PrintScreen, Space, Tab, Enter, Escape, Insert, Delete,
#          Home, End, PageUp, PageDown, Up, Down, Left, Right, Pause, ScrollLock.
# Пустая строка "" — действие без горячей клавиши.

# Выделить область (с подсветкой окон и элементов)
capture_region = "PrintScreen"
# Весь экран (все мониторы) сразу в буфер
capture_screen = "Shift+PrintScreen"
# Активное окно сразу в буфер
capture_window = "Alt+PrintScreen"
# Повторить последнюю выделенную область
repeat_region = "Ctrl+PrintScreen"
# Выделить область и сразу распознать текст
recognize = ""
# Выделить область и сразу распознать и перевести
translate = ""

[save]
# Папка для снимков. Пусто — «Изображения\Snapling».
# Путь в одинарных кавычках: dir = 'E:\Photos' (или с прямыми слешами: "E:/Photos").
dir = ''
# Формат: "png" (без потерь), "webp" (без потерь, меньше), "jpeg" (с потерями).
format = "png"
# Качество JPEG, 1–100.
jpeg_quality = 90
# Имя файла без расширения. {yyyy} {MM} {dd} {HH} {mm} {ss} — дата и время, {app} — программа.
# «/» создаёт подпапки.
name = "{yyyy}-{MM}/Snapling_{yyyy}-{MM}-{dd}_{HH}-{mm}-{ss}_{app}"
# Сохранять каждый скопированный снимок в папку автоматически.
auto_save = true

[capture]
# Копировать снимок в буфер после Shift/Alt/Ctrl+PrtScn. Enter в выделении копирует всегда.
copy = true
# Открывать редактор после снимка по Shift/Alt/Ctrl+PrtScn.
open_editor = false
# Выделил — сразу скопировано и закрыто, без панели действий.
instant_copy = false
# Уведомление после снимка: миниатюра и кнопки.
toast = true

[blur]
# Размытие кистью при запуске: false — кубики, true — гаусс. Сила 0, 1 или 2.
gauss = false
strength = 0

[sound]
enabled = true
# Набор: "soft" (мягкий), "real" (реалистичный), "system" (системный, как в Linux).
pack = "soft"
# Свои звуки, файлы WAV. Пусто — звук из набора.
capture_file = ''
copy_file = ''
error_file = ''
# Громкость 0–100.
volume = 60
# Без звуков, пока запущена игра на весь экран.
mute_in_games = true

[privacy]
# Прятать выделение от демонстрации экрана и записи (Discord, OBS, Teams). Windows 10 2004+.
hide_from_capture = true

[ocr]
# Язык распознавания текста (O в выделении): "auto" — по языкам Windows, или "ru", "en".
# Распознавание работает без интернета, встроенным движком Windows.
language = "auto"

[translate]
# Онлайн-перевод. "off" — только офлайн (модели на компьютере).
# "deepl" — в окне перевода появится кнопка «DeepL»: текст уходит в DeepL только по её нажатию.
# Ключ API вводится в окне перевода и хранится в диспетчере учётных данных Windows, не здесь.
online = "off"

[ui]
# Язык: "auto" — как в системе, "ru", "en" или свой файл из папки i18n рядом с этим файлом.
language = "auto"
# Тема окон: "system", "dark", "light".
theme = "system"
# Значок в трее: "auto" — под цвет панели задач, "white", "black".
tray_icon = "auto"
"#;

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub hotkeys: Hotkeys,
    pub save: Save,
    pub capture: Capture,
    pub sound: SoundCfg,
    pub privacy: Privacy,
    pub ocr: Ocr,
    pub translate: Translate,
    pub blur: Blur,
    pub ui: Ui,
}

/// Starting style of the blur brush.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Blur {
    pub gauss: bool,
    pub strength: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Dark,
    Light,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ui {
    pub language: String,
    pub theme: Theme,
    pub tray_icon: TrayIcon,
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            language: "auto".into(),
            theme: Theme::default(),
            tray_icon: TrayIcon::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrayIcon {
    #[default]
    Auto,
    White,
    Black,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Capture {
    pub copy: bool,
    pub open_editor: bool,
    pub instant_copy: bool,
    pub toast: bool,
}

impl Default for Capture {
    fn default() -> Self {
        Self {
            copy: true,
            open_editor: false,
            instant_copy: false,
            toast: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SoundPack {
    #[default]
    Soft,
    Real,
    System,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SoundCfg {
    pub enabled: bool,
    pub pack: SoundPack,
    /// Own WAV files; empty = the pack's sound.
    pub capture_file: String,
    pub copy_file: String,
    pub error_file: String,
    pub volume: u8,
    pub mute_in_games: bool,
}

impl Default for SoundCfg {
    fn default() -> Self {
        Self {
            enabled: true,
            pack: SoundPack::default(),
            capture_file: String::new(),
            copy_file: String::new(),
            error_file: String::new(),
            volume: 60,
            mute_in_games: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Privacy {
    pub hide_from_capture: bool,
}

impl Default for Privacy {
    fn default() -> Self {
        Self {
            hide_from_capture: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Online {
    #[default]
    Off,
    Deepl,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Translate {
    pub online: Online,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ocr {
    pub language: String,
}

impl Default for Ocr {
    fn default() -> Self {
        Self {
            language: "auto".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    #[default]
    Png,
    Webp,
    Jpeg,
}

impl Format {
    pub fn ext(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Webp => "webp",
            Format::Jpeg => "jpg",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Save {
    /// Empty: the platform's Pictures folder + `Snapling`.
    pub dir: String,
    pub format: Format,
    pub jpeg_quality: u8,
    pub name: String,
    pub auto_save: bool,
}

impl Default for Save {
    fn default() -> Self {
        Self {
            dir: String::new(),
            format: Format::Png,
            jpeg_quality: 90,
            name: "{yyyy}-{MM}/Snapling_{yyyy}-{MM}-{dd}_{HH}-{mm}-{ss}_{app}".into(),
            auto_save: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Hotkeys {
    pub capture_region: String,
    pub capture_screen: String,
    pub capture_window: String,
    pub repeat_region: String,
    pub recognize: String,
    pub translate: String,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            capture_region: "PrintScreen".into(),
            capture_screen: "Shift+PrintScreen".into(),
            capture_window: "Alt+PrintScreen".into(),
            repeat_region: "Ctrl+PrintScreen".into(),
            recognize: String::new(),
            translate: String::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    CaptureRegion,
    CaptureScreen,
    CaptureWindow,
    RepeatRegion,
    RecognizeText,
    TranslateText,
}

impl Action {
    pub const ALL: [Action; 6] = [
        Action::CaptureRegion,
        Action::CaptureScreen,
        Action::CaptureWindow,
        Action::RepeatRegion,
        Action::RecognizeText,
        Action::TranslateText,
    ];

    /// Key in the `[hotkeys]` section.
    pub fn key(self) -> &'static str {
        match self {
            Action::CaptureRegion => "capture_region",
            Action::CaptureScreen => "capture_screen",
            Action::CaptureWindow => "capture_window",
            Action::RepeatRegion => "repeat_region",
            Action::RecognizeText => "recognize",
            Action::TranslateText => "translate",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Action::CaptureRegion => t("Выделить область"),
            Action::CaptureScreen => t("Весь экран"),
            Action::CaptureWindow => t("Активное окно"),
            Action::RepeatRegion => t("Повторить область"),
            Action::RecognizeText => t("Распознать текст"),
            Action::TranslateText => t("Текст + перевод"),
        }
    }
}

impl Hotkeys {
    pub fn combo(&self, a: Action) -> &str {
        match a {
            Action::CaptureRegion => &self.capture_region,
            Action::CaptureScreen => &self.capture_screen,
            Action::CaptureWindow => &self.capture_window,
            Action::RepeatRegion => &self.repeat_region,
            Action::RecognizeText => &self.recognize,
            Action::TranslateText => &self.translate,
        }
    }

    pub fn set_combo(&mut self, a: Action, combo: String) {
        *match a {
            Action::CaptureRegion => &mut self.capture_region,
            Action::CaptureScreen => &mut self.capture_screen,
            Action::CaptureWindow => &mut self.capture_window,
            Action::RepeatRegion => &mut self.repeat_region,
            Action::RecognizeText => &mut self.recognize,
            Action::TranslateText => &mut self.translate,
        } = combo;
    }
}

impl Config {
    /// Missing file: writes the commented template. Broken file: error, file left untouched.
    pub fn load_or_create(path: &Path) -> Result<Self, String> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                }
                std::fs::write(path, TEMPLATE).map_err(|e| format!("{}: {e}", path.display()))?;
                TEMPLATE.to_string()
            }
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Changes one key in the file, keeping comments. A result that no longer parses is not
    /// written. The tray notices the new mtime and applies it.
    pub fn update(
        path: &Path,
        section: &str,
        key: &str,
        value: impl Into<toml::Value>,
    ) -> Result<(), String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let text = set_value(&text, section, key, value.into());
        toml::from_str::<Config>(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let part = path.with_extension("toml.part");
        std::fs::write(&part, text)
            .and_then(|()| std::fs::rename(&part, path))
            .map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Replaces `key` inside `[section]`, or adds the key or the section when missing.
fn set_value(text: &str, section: &str, key: &str, value: toml::Value) -> String {
    let line = format!("{key} = {value}");
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let header = format!("[{section}]");
    let bare = |l: &str| l.split('#').next().unwrap_or("").trim().to_string();
    let Some(start) = lines.iter().position(|l| bare(l) == header) else {
        lines.extend([String::new(), header, line]);
        return lines.join("\n") + "\n";
    };
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.trim_start().starts_with('['))
        .map_or(lines.len(), |i| start + 1 + i);
    let found = lines[start + 1..end].iter().position(|l| {
        l.trim_start()
            .strip_prefix(key)
            .is_some_and(|rest| rest.trim_start().starts_with('='))
    });
    match found {
        Some(i) => lines[start + 1 + i] = line,
        None => {
            let last = lines[..end]
                .iter()
                .rposition(|l| !l.trim().is_empty())
                .unwrap_or(start);
            lines.insert(last + 1, line);
        }
    }
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("snapling-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("config.toml")
    }

    #[test]
    fn creates_defaults_then_reads_them() {
        let p = tmp("create");
        assert_eq!(Config::load_or_create(&p).unwrap(), Config::default());
        assert_eq!(Config::load_or_create(&p).unwrap(), Config::default());
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn partial_file_keeps_defaults() {
        let p = tmp("partial");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "").unwrap();
        assert_eq!(Config::load_or_create(&p).unwrap(), Config::default());
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn broken_file_is_error_and_untouched() {
        let p = tmp("broken");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "hotkeys = [").unwrap();
        assert!(Config::load_or_create(&p).is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "hotkeys = [");
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn typo_in_hotkey_name_is_reported() {
        let err = toml::from_str::<Config>("[hotkeys]\ncapture_regoin = \"F1\"").unwrap_err();
        assert!(err.to_string().contains("capture_regoin"), "{err}");
    }

    #[test]
    fn set_value_keeps_comments_and_quotes_paths() {
        let text = set_value(TEMPLATE, "save", "dir", r"E:\Photos".into());
        let text = set_value(&text, "sound", "volume", toml::Value::Integer(30));
        let text = set_value(&text, "capture", "copy", false.into());
        assert!(text.contains("# Формат: \"png\""));
        let cfg: Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg.save.dir, r"E:\Photos");
        assert_eq!(cfg.sound.volume, 30);
        assert!(!cfg.capture.copy);
        assert!(cfg.sound.enabled);
    }

    #[test]
    fn set_value_adds_missing_key_and_section() {
        let v = toml::Value::Integer(5);
        let text = set_value("[sound]\n\n[privacy]\n", "sound", "volume", v);
        assert_eq!(text, "[sound]\nvolume = 5\n\n[privacy]\n");
        let text = set_value("[sound]\n", "ui", "theme", "dark".into());
        let cfg: Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg.ui.theme, Theme::Dark);
    }

    #[test]
    fn set_value_skips_longer_key_with_same_prefix() {
        let v = toml::Value::Integer(3);
        let text = set_value("[a]\ncopy_all = 1\ncopy = 2\n", "a", "copy", v);
        assert_eq!(text, "[a]\ncopy_all = 1\ncopy = 3\n");
    }

    #[test]
    fn missing_keys_fall_back_to_defaults() {
        let cfg: Config = toml::from_str("[hotkeys]\ncapture_screen = \"\"").unwrap();
        assert_eq!(cfg.hotkeys.capture_screen, "");
        assert_eq!(cfg.hotkeys.capture_region, "PrintScreen");
    }
}
