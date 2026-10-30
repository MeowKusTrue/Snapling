use crate::config::{Action, Hotkeys};
use crate::i18n::{t, tf};
use std::fmt;

const NAMED: &[&str] = &[
    "PrintScreen",
    "Space",
    "Tab",
    "Enter",
    "Escape",
    "Insert",
    "Delete",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "Up",
    "Down",
    "Left",
    "Right",
    "Pause",
    "ScrollLock",
];

/// Every key a hotkey can use, canonical spelling.
pub fn key_names() -> impl Iterator<Item = String> {
    let letters = ('A'..='Z').chain('0'..='9').map(String::from);
    let fkeys = (1..=24).map(|n| format!("F{n}"));
    letters
        .chain(fkeys)
        .chain(NAMED.iter().map(|n| n.to_string()))
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Hotkey {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    /// Canonical: `A`-`Z`, `0`-`9`, `F1`-`F24` or an entry of `NAMED`.
    pub key: String,
}

impl Hotkey {
    pub fn parse(s: &str) -> Result<Self, String> {
        let mut hk = Hotkey::default();
        for part in s.split('+').map(str::trim) {
            let flag = match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => &mut hk.ctrl,
                "alt" => &mut hk.alt,
                "shift" => &mut hk.shift,
                "win" | "super" | "meta" => &mut hk.win,
                _ => {
                    if !hk.key.is_empty() {
                        return Err(tf("«{s}»: больше одной клавиши", &[("s", &s)]));
                    }
                    hk.key = canonical_key(part).ok_or_else(|| {
                        tf(
                            "«{s}»: неизвестная клавиша «{part}»",
                            &[("s", &s), ("part", &part)],
                        )
                    })?;
                    continue;
                }
            };
            if *flag {
                return Err(tf("«{s}»: модификатор повторяется", &[("s", &s)]));
            }
            *flag = true;
        }
        if hk.key.is_empty() {
            return Err(tf("«{s}»: нет основной клавиши", &[("s", &s)]));
        }
        Ok(hk)
    }
}

fn canonical_key(k: &str) -> Option<String> {
    let up = k.to_ascii_uppercase();
    if up.len() == 1 && up.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Some(up);
    }
    if let Some(n) = up.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()) {
        return (1..=24).contains(&n).then(|| format!("F{n}"));
    }
    let alias = match up.as_str() {
        "PRTSC" | "PRTSCN" | "PRINT" => "PrintScreen",
        "ESC" => "Escape",
        "DEL" => "Delete",
        "INS" => "Insert",
        "RETURN" => "Enter",
        "PGUP" => "PageUp",
        "PGDN" => "PageDown",
        _ => k,
    };
    NAMED
        .iter()
        .find(|n| n.eq_ignore_ascii_case(alias))
        .map(|n| n.to_string())
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (on, name) in [
            (self.ctrl, "Ctrl"),
            (self.alt, "Alt"),
            (self.shift, "Shift"),
            (self.win, "Win"),
        ] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        f.write_str(&self.key)
    }
}

/// The other action already bound to this combo.
pub fn taken_by(hotkeys: &Hotkeys, action: Action, hk: &Hotkey) -> Option<Action> {
    Action::ALL
        .into_iter()
        .filter(|a| *a != action)
        .find(|a| Hotkey::parse(hotkeys.combo(*a)).is_ok_and(|h| h == *hk))
}

/// Parsed bindings plus readable problems. Syntax errors and duplicates are dropped;
/// OS-reserved combos are kept (registration may still work) but warned about.
pub fn check(
    hotkeys: &Hotkeys,
    reserved: fn(&Hotkey) -> Option<&'static str>,
) -> (Vec<(Action, Hotkey)>, Vec<String>) {
    let mut ok: Vec<(Action, Hotkey)> = Vec::new();
    let mut problems = Vec::new();
    for a in Action::ALL {
        let combo = hotkeys.combo(a).trim();
        if combo.is_empty() {
            continue;
        }
        let hk = match Hotkey::parse(combo) {
            Ok(hk) => hk,
            Err(e) => {
                problems.push(format!("«{}»: {e}", a.title()));
                continue;
            }
        };
        if let Some((other, _)) = ok.iter().find(|(_, h)| *h == hk) {
            problems.push(tf(
                "«{0}»: {hk} уже назначено на «{1}»",
                &[("hk", &hk), ("0", &a.title()), ("1", &other.title())],
            ));
            continue;
        }
        if let Some(why) = reserved(&hk) {
            problems.push(format!("«{}»: {hk} — {why}", a.title()));
        }
        ok.push((a, hk));
    }
    (ok, problems)
}

/// Combos Windows or every app already uses. Alt+PrintScreen is left out on purpose:
/// Snapling replaces Windows' "copy active window" with the same action.
pub fn reserved_on_windows(hk: &Hotkey) -> Option<&'static str> {
    let k = hk.key.as_str();
    let fkey_high = k
        .strip_prefix('F')
        .and_then(|n| n.parse::<u8>().ok())
        .is_some_and(|n| n >= 13);
    Some(match (hk.ctrl, hk.alt, hk.shift, hk.win, k) {
        (false, false, true, true, "S") => t("занято «Ножницами» Windows"),
        (false, false, false, true, "PrintScreen") => t("снимок экрана Windows в «Изображения»"),
        (false, true, false, true, "PrintScreen" | "R") => t("занято Xbox Game Bar"),
        (_, _, _, true, _) => t("сочетания с Win часто заняты Windows"),
        (true, true, false, false, "Delete") => t("системное сочетание Windows"),
        (true, false, true, false, "Escape") => t("открывает диспетчер задач"),
        (true, false, false, false, "Escape") => t("открывает меню «Пуск»"),
        (false, true, false, false, "Tab" | "F4" | "Escape") => t("системное сочетание Windows"),
        (
            true,
            false,
            false,
            false,
            "A" | "C" | "F" | "N" | "O" | "P" | "S" | "T" | "V" | "W" | "X" | "Y" | "Z",
        ) => t("перехватит стандартное сочетание во всех программах"),
        (false, false, false, false, "PrintScreen" | "Pause" | "ScrollLock") => return None,
        (false, false, false, false, _) if !fkey_high => {
            t("без модификаторов клавиша перестанет работать в других программах")
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taken_by_finds_the_other_action_only() {
        let h = Hotkeys::default();
        let screen = Hotkey::parse("shift+printscreen").unwrap();
        assert_eq!(
            taken_by(&h, Action::CaptureRegion, &screen),
            Some(Action::CaptureScreen)
        );
        assert_eq!(taken_by(&h, Action::CaptureScreen, &screen), None);
    }

    #[test]
    fn check_reports_bad_duplicate_and_reserved() {
        let hk = Hotkeys {
            capture_region: "PrintScreen".into(),
            capture_screen: "printscreen".into(),
            capture_window: "Ctrl+Foo".into(),
            repeat_region: "Win+Shift+S".into(),
            ..Hotkeys::default()
        };
        let (ok, problems) = check(&hk, reserved_on_windows);
        let actions: Vec<Action> = ok.iter().map(|(a, _)| *a).collect();
        assert_eq!(actions, [Action::CaptureRegion, Action::RepeatRegion]);
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(problems[0].contains("уже назначено на «Выделить область»"));
        assert!(problems[1].contains("неизвестная клавиша"));
        assert!(problems[2].contains("Ножницами"));
    }

    #[test]
    fn defaults_are_clean_and_empty_disables() {
        let (ok, problems) = check(&Hotkeys::default(), reserved_on_windows);
        assert_eq!(ok.len(), 4);
        assert!(problems.is_empty(), "{problems:?}");
        let off = Hotkeys {
            capture_screen: " ".into(),
            ..Hotkeys::default()
        };
        assert_eq!(check(&off, reserved_on_windows).0.len(), 3);
    }

    #[test]
    fn reserved_table() {
        let r = |s: &str| reserved_on_windows(&Hotkey::parse(s).unwrap()).is_some();
        for bad in [
            "Win+PrintScreen",
            "Win+E",
            "Ctrl+C",
            "Alt+F4",
            "A",
            "F5",
            "Ctrl+Shift+Escape",
        ] {
            assert!(r(bad), "{bad}");
        }
        for fine in [
            "PrintScreen",
            "Alt+PrintScreen",
            "Ctrl+Shift+S",
            "F13",
            "Ctrl+Alt+S",
            "Pause",
        ] {
            assert!(!r(fine), "{fine}");
        }
    }

    #[test]
    fn parses_and_round_trips() {
        for (input, out) in [
            ("Ctrl+Alt+PrintScreen", "Ctrl+Alt+PrintScreen"),
            ("shift + ctrl + s", "Ctrl+Shift+S"),
            ("Win+PrtScn", "Win+PrintScreen"),
            ("f12", "F12"),
            ("Alt+pgdn", "Alt+PageDown"),
            ("Ctrl+1", "Ctrl+1"),
        ] {
            assert_eq!(Hotkey::parse(input).unwrap().to_string(), out, "{input}");
        }
    }

    #[test]
    fn rejects_bad_input() {
        for bad in [
            "",
            "Ctrl",
            "Ctrl+Ctrl+A",
            "A+B",
            "F25",
            "F0",
            "Ctrl+Foo",
            "Ctrl+",
        ] {
            assert!(Hotkey::parse(bad).is_err(), "{bad}");
        }
    }
}
