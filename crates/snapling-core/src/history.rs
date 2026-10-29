//! Capture history: one JSON object per line, append-only, local file.
//! Removing an entry later never deletes the image itself.

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
        };
        append(&f, &e).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&f)
            .unwrap()
            .write_all(b"{broken\n")
            .unwrap();
        append(&f, &e).unwrap();
        assert_eq!(load(&f), vec![e.clone(), e]);
        let _ = std::fs::remove_dir_all(f.parent().unwrap());
    }
}
