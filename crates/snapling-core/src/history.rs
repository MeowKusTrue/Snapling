//! Capture history: one JSON object per line, append-only, local file.
//! Removing an entry later never deletes the image itself.

use crate::i18n::tf;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// Local time, `YYYY-MM-DDTHH:MM:SS`.
    pub time: String,
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub app: String,
    pub kind: String,
    /// Recognized text, searchable. Entries from OCR alone have an empty `path`.
    #[serde(default)]
    pub text: String,
}

pub fn time_string(t: crate::storage::Stamp) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    )
}

pub fn append(file: &Path, e: &Entry) -> Result<(), String> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    }
    let line = serde_json::to_string(e).map_err(|err| err.to_string())?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .map_err(|err| format!("{}: {err}", file.display()))?;
    writeln!(f, "{line}").map_err(|err| format!("{}: {err}", file.display()))
}

/// All readable entries, oldest first; broken lines are skipped.
pub fn load(file: &Path) -> Vec<Entry> {
    std::fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Drops the first equal entry. The image file stays on disk.
// ponytail: rewrites the file; a capture finishing at that moment can lose its line. Fine by hand.
pub fn remove(file: &Path, e: &Entry) -> Result<(), String> {
    let mut entries = load(file);
    if let Some(i) = entries.iter().position(|x| x == e) {
        entries.remove(i);
    }
    write_all(file, &entries)
}

/// Keeps recognized text on the first equal entry, so the search finds it.
pub fn set_text(file: &Path, e: &Entry, text: &str) -> Result<(), String> {
    let mut entries = load(file);
    if let Some(x) = entries.iter_mut().find(|x| *x == e) {
        x.text = text.to_string();
    }
    write_all(file, &entries)
}

/// Drops entries older than `days`; 0 keeps everything. Returns the dropped ones.
pub fn prune(file: &Path, now: &str, days: u32) -> Result<Vec<Entry>, String> {
    if days == 0 {
        return Ok(Vec::new());
    }
    let (old, kept): (Vec<Entry>, Vec<Entry>) = load(file)
        .into_iter()
        .partition(|e| days_ago(&e.time, now).is_some_and(|d| d > i64::from(days)));
    if !old.is_empty() {
        write_all(file, &kept)?;
    }
    Ok(old)
}

fn write_all(file: &Path, entries: &[Entry]) -> Result<(), String> {
    let mut text = String::new();
    for x in entries {
        text += &serde_json::to_string(x).map_err(|err| err.to_string())?;
        text.push('\n');
    }
    let part = file.with_extension("jsonl.part");
    std::fs::write(&part, text)
        .and_then(|()| std::fs::rename(&part, file))
        .map_err(|err| format!("{}: {err}", file.display()))
}

fn day(time: &str) -> Option<i64> {
    let (y, m, d): (i64, i64, i64) = (
        time.get(0..4)?.parse().ok()?,
        time.get(5..7)?.parse().ok()?,
        time.get(8..10)?.parse().ok()?,
    );
    let (y, m) = if m <= 2 { (y - 1, m + 12) } else { (y, m) };
    Some(365 * y + y / 4 - y / 100 + y / 400 + 153 * (m + 1) / 5 + d)
}

/// Whole days between two `time_string`s; `None` for a broken string.
pub fn days_ago(time: &str, now: &str) -> Option<i64> {
    Some(day(now)? - day(time)?)
}

/// «Сегодня, 14:21», «Вчера, 22:17» or «27.10».
pub fn when(time: &str, now: &str) -> String {
    let hm = time.get(11..16).unwrap_or("");
    match days_ago(time, now) {
        Some(0) => tf("Сегодня, {hm}", &[("hm", &hm)]),
        Some(1) => tf("Вчера, {hm}", &[("hm", &hm)]),
        _ => match (time.get(8..10), time.get(5..7)) {
            (Some(d), Some(m)) => format!("{d}.{m}"),
            _ => time.to_string(),
        },
    }
}

/// Case-insensitive search over the app, the file name and the recognized text.
/// `Some` holds the matching text line for the card (empty for other matches).
pub fn matches(e: &Entry, query: &str) -> Option<String> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Some(String::new());
    }
    if let Some(line) = e.text.lines().find(|l| l.to_lowercase().contains(&q)) {
        return Some(line.trim().chars().take(40).collect());
    }
    (e.app.to_lowercase().contains(&q) || e.path.to_lowercase().contains(&q)).then(String::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_and_load_round_trip() {
        let f = std::env::temp_dir()
            .join(format!("snapling-history-{}", std::process::id()))
            .join("history.jsonl");
        let e = Entry {
            time: "2026-10-29T14:21:07".into(),
            path: r#"E:\Photos\a "b".png"#.into(),
            width: 10,
            height: 20,
            app: "Discord".into(),
            kind: "region".into(),
            text: String::new(),
        };
        append(&f, &e).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&f)
            .unwrap()
            .write_all(b"{broken\n")
            .unwrap();
        append(&f, &e).unwrap();
        assert_eq!(load(&f), vec![e.clone(), e.clone()]);
        remove(&f, &e).unwrap();
        assert_eq!(load(&f), vec![e.clone()]);
        set_text(&f, &e, "счёт").unwrap();
        assert_eq!(load(&f)[0].text, "счёт");
        let _ = std::fs::remove_dir_all(f.parent().unwrap());
    }

    #[test]
    fn prune_drops_only_old_entries() {
        let f = std::env::temp_dir()
            .join(format!("snapling-prune-{}", std::process::id()))
            .join("history.jsonl");
        let at = |time: &str| Entry {
            time: time.into(),
            path: "p".into(),
            width: 1,
            height: 1,
            app: String::new(),
            kind: "region".into(),
            text: String::new(),
        };
        append(&f, &at("2026-01-01T10:00:00")).unwrap();
        append(&f, &at("2026-10-29T10:00:00")).unwrap();
        append(&f, &at("not a date")).unwrap();
        let now = "2026-10-30T10:00:00";
        assert!(prune(&f, now, 0).unwrap().is_empty());
        assert_eq!(prune(&f, now, 30).unwrap()[0].time, "2026-01-01T10:00:00");
        assert_eq!(load(&f).len(), 2, "a broken date stays");
        let _ = std::fs::remove_dir_all(f.parent().unwrap());
    }

    #[test]
    fn old_lines_without_text_load() {
        let l = r#"{"time":"t","path":"p","width":1,"height":2,"app":"a","kind":"k"}"#;
        assert_eq!(serde_json::from_str::<Entry>(l).unwrap().text, "");
    }

    #[test]
    fn when_counts_days_over_month_and_leap_day() {
        let now = "2024-03-01T09:00:00";
        assert_eq!(when("2024-03-01T08:05:00", now), "Сегодня, 08:05");
        assert_eq!(when("2024-02-29T22:17:00", now), "Вчера, 22:17");
        assert_eq!(when("2024-02-28T22:17:00", now), "28.02");
        assert_eq!(
            days_ago("2023-12-31T00:00:00", "2024-01-01T00:00:00"),
            Some(1)
        );
        assert_eq!(when("сломано", now), "сломано");
    }

    #[test]
    fn matches_finds_text_line_ignoring_case() {
        let e = Entry {
            time: String::new(),
            path: r"E:\a.png".into(),
            width: 1,
            height: 1,
            app: "Discord".into(),
            kind: "region".into(),
            text: "первая\nСчёт на 2 400".into(),
        };
        assert_eq!(matches(&e, "счёт").as_deref(), Some("Счёт на 2 400"));
        assert_eq!(matches(&e, "disc").as_deref(), Some(""));
        assert_eq!(matches(&e, "нет"), None);
        assert_eq!(matches(&e, "  ").as_deref(), Some(""));
    }
}
