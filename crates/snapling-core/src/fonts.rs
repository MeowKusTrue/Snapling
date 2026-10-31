//! Installed fonts for the text tool. The scan takes a moment, so it runs once, on first use.

use ab_glyph::FontRef;
use fontdb::{Database, Family, Query};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

static DB: LazyLock<Database> = LazyLock::new(|| {
    let mut db = Database::new();
    db.load_system_fonts();
    db
});

/// Family names, sorted without regard to case.
pub fn families() -> &'static [&'static str] {
    static NAMES: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
        let mut names: Vec<&str> = DB
            .faces()
            .filter_map(|f| f.families.first().map(|(n, _)| n.as_str()))
            .collect();
        names.sort_by_key(|n| n.to_lowercase());
        names.dedup();
        names
    });
    &NAMES
}

/// A name from the config as the style keeps it. `""` is the bundled font.
// ponytail: leaks a few bytes per config reload, an interning table if names ever churn.
pub fn intern(name: &str) -> &'static str {
    if name.is_empty() {
        ""
    } else {
        Box::leak(name.to_owned().into_boxed_str())
    }
}

/// File contents and face index.
pub type Face = (&'static [u8], u32);

/// The face of a family; kept for the life of the process.
pub fn data(family: &str) -> Option<Face> {
    static LOADED: LazyLock<Mutex<HashMap<String, Option<Face>>>> = LazyLock::new(Mutex::default);
    if family.is_empty() {
        return None;
    }
    *LOADED
        .lock()
        .ok()?
        .entry(family.to_owned())
        .or_insert_with(|| {
            let families = [Family::Name(family)];
            let query = Query {
                families: &families,
                ..Query::default()
            };
            let id = DB.query(&query)?;
            DB.with_face_data(id, |bytes, index| {
                (&*Box::leak(bytes.to_vec().into_boxed_slice()), index)
            })
        })
}

pub fn font(family: &str) -> Option<FontRef<'static>> {
    let (bytes, index) = data(family)?;
    FontRef::try_from_slice_and_index(bytes, index).ok()
}

/// The family after or before `now` in the list, the bundled font sitting between the ends.
pub fn neighbor(now: &str, forward: bool) -> &'static str {
    let list = families();
    let at = list.iter().position(|f| *f == now);
    let next = match (at, forward) {
        (None, true) => list.first(),
        (None, false) => list.last(),
        (Some(i), true) => list.get(i + 1),
        (Some(i), false) => i.checked_sub(1).and_then(|i| list.get(i)),
    };
    next.copied().unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neighbors_walk_the_whole_list_and_return() {
        let mut now = "";
        for _ in 0..=families().len() {
            now = neighbor(now, true);
        }
        assert_eq!(now, "");
        assert_eq!(neighbor(neighbor("", false), true), "");
    }

    #[test]
    fn unknown_family_has_no_font() {
        assert!(font("").is_none());
        assert!(font("No Such Font 123").is_none());
    }
}
