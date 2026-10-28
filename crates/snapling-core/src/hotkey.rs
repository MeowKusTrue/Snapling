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
                        return Err(format!("«{s}»: больше одной клавиши"));
                    }
                    hk.key = canonical_key(part)
                        .ok_or_else(|| format!("«{s}»: неизвестная клавиша «{part}»"))?;
                    continue;
                }
            };
            if *flag {
                return Err(format!("«{s}»: модификатор повторяется"));
            }
            *flag = true;
        }
        if hk.key.is_empty() {
            return Err(format!("«{s}»: нет основной клавиши"));
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

#[cfg(test)]
mod tests {
    use super::*;

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
