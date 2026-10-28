use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub hotkeys: Hotkeys,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    pub capture_screen: String,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            capture_screen: "PrintScreen".into(),
        }
    }
}

impl Config {
    /// Missing file: writes defaults. Broken file: error, file left untouched.
    pub fn load_or_create(path: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let cfg = Self::default();
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                }
                let text = toml::to_string(&cfg).map_err(|e| e.to_string())?;
                std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))?;
                Ok(cfg)
            }
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
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
}
