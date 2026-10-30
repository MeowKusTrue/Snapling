//! Interface languages. The key is the Russian text itself: code reads `t("Настройки")`,
//! `assets/i18n/en.toml` holds `"Настройки" = "Settings"`, and a string with no translation shows
//! in Russian. A language is one file: built in below, or dropped into `<config>/i18n/`.
//! `python tools/i18n_sync.py` adds new strings to every file.

use std::collections::HashMap;
use std::fmt::Display;
use std::path::Path;
use std::sync::RwLock;

const BUILT_IN: [(&str, &str); 1] = [("en", include_str!("../../../assets/i18n/en.toml"))];

type Table = HashMap<String, &'static str>;

/// `None` = Russian, the keys themselves.
static TABLE: RwLock<Option<&'static Table>> = RwLock::new(None);

fn parse(text: &str) -> Option<(String, Table)> {
    let mut table: HashMap<String, String> = toml::from_str(text).ok()?;
    let name = table.remove("_name").filter(|n| !n.is_empty())?;
    let table = table
        .into_iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| (k, &*Box::leak(v.into_boxed_str())))
        .collect();
    Some((name, table))
}

/// (code, name) of every language; `dir` is the user's folder of extra files.
pub fn languages(dir: &Path) -> Vec<(String, String)> {
    let mut out = vec![("ru".to_string(), "Русский".to_string())];
    let names = |text: &str| parse(text).map(|(n, _)| n);
    for (code, text) in BUILT_IN {
        out.extend(names(text).map(|n| (code.to_string(), n)));
    }
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let code = path.file_stem().map(|c| c.to_string_lossy().to_string());
        if path.extension().is_some_and(|e| e == "toml")
            && let Some(code) = code.filter(|c| !out.iter().any(|(k, _)| k == c))
            && let Some(n) = std::fs::read_to_string(&path)
                .ok()
                .as_deref()
                .and_then(names)
        {
            out.push((code, n));
        }
    }
    out
}

/// Picks a language: `code` from the config, or `system` when it says "auto". An unknown code or
/// a broken file leaves Russian.
pub fn set(code: &str, system: &str, dir: &Path) {
    let code = if code == "auto" { system } else { code };
    let text = BUILT_IN
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, text)| text.to_string())
        .or_else(|| std::fs::read_to_string(dir.join(format!("{code}.toml"))).ok());
    let table = text
        .as_deref()
        .and_then(parse)
        .map(|(_, table)| &*Box::leak(Box::new(table)));
    if let Ok(mut now) = TABLE.write() {
        *now = table;
    }
}

/// Translated text.
pub fn t(key: &'static str) -> &'static str {
    let table = TABLE.read().ok().and_then(|g| *g);
    table.and_then(|m| m.get(key).copied()).unwrap_or(key)
}

/// Translated text with `{name}` placeholders filled in.
pub fn tf(key: &'static str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = t(key).to_string();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), &value.to_string());
    }
    out
}

/// Marks a Russian string in a `const` for `tools/i18n_sync.py`; translate it with `t` when shown.
pub const fn n(key: &'static str) -> &'static str {
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slots(s: &str) -> Vec<String> {
        let mut v: Vec<String> = s
            .split('{')
            .skip(1)
            .filter_map(|p| p.split_once('}').map(|x| x.0.to_string()))
            .collect();
        v.sort();
        v
    }

    #[test]
    fn built_in_files_parse_with_every_placeholder_kept() {
        for (code, text) in BUILT_IN {
            let (name, table) = parse(text).unwrap_or_else(|| panic!("{code}.toml is broken"));
            assert!(!name.is_empty());
            for (k, v) in table {
                assert_eq!(slots(&k), slots(v), "{code}: {k}");
            }
        }
    }

    #[test]
    fn missing_string_falls_back_to_russian_and_placeholders_fill() {
        let dir = std::env::temp_dir().join(format!("snapling-i18n-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("xx.toml"),
            "_name = \"Test\"\n\"Привет {who}\" = \"Hello {who}\"\n",
        )
        .unwrap();
        assert!(
            languages(&dir)
                .iter()
                .any(|(c, n)| c == "xx" && n == "Test")
        );
        set("xx", "ru", &dir);
        assert_eq!(tf("Привет {who}", &[("who", &"Мир")]), "Hello Мир");
        assert_eq!(t("Нет перевода"), "Нет перевода");
        set("auto", "ru", &dir);
        assert_eq!(tf("Привет {who}", &[("who", &1)]), "Привет 1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn broken_language_file_stays_russian() {
        let dir = std::env::temp_dir().join(format!("snapling-i18n-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("zz.toml"), "это не toml = = =").unwrap();
        set("zz", "ru", &dir);
        assert_eq!(t("Настройки"), "Настройки");
        assert!(!languages(&dir).iter().any(|(c, _)| c == "zz"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
