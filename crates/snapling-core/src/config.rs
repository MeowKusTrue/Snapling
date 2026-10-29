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
"#;

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub hotkeys: Hotkeys,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Hotkeys {
    pub capture_region: String,
    pub capture_screen: String,
    pub capture_window: String,
    pub repeat_region: String,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            capture_region: "PrintScreen".into(),
            capture_screen: "Shift+PrintScreen".into(),
            capture_window: "Alt+PrintScreen".into(),
            repeat_region: "Ctrl+PrintScreen".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    CaptureRegion,
    CaptureScreen,
    CaptureWindow,
    RepeatRegion,
}

impl Action {
    pub const ALL: [Action; 4] = [
        Action::CaptureRegion,
        Action::CaptureScreen,
        Action::CaptureWindow,
        Action::RepeatRegion,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Action::CaptureRegion => "Выделить область",
            Action::CaptureScreen => "Весь экран",
            Action::CaptureWindow => "Активное окно",
            Action::RepeatRegion => "Повторить область",
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
        }
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
    fn missing_keys_fall_back_to_defaults() {
        let cfg: Config = toml::from_str("[hotkeys]\ncapture_screen = \"\"").unwrap();
        assert_eq!(cfg.hotkeys.capture_screen, "");
        assert_eq!(cfg.hotkeys.capture_region, "PrintScreen");
    }
}
