//! Dictionary proposals preserve byte ranges; nothing is replaced without a user's choice.
#[derive(Clone, Debug)]
pub struct Suggestion {
    pub start: usize,
    pub end: usize,
    pub word: String,
    pub replacements: Vec<String>,
}

/// Keep identifiers, numbers, URLs and email addresses out of dictionary correction.
pub fn words(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut offset = 0;
    for chunk in text.split_inclusive(char::is_whitespace) {
        if !chunk
            .chars()
            .any(|c| c.is_numeric() || "_@/\\:".contains(c))
        {
            let mut start = None;
            for (at, ch) in chunk
                .char_indices()
                .chain(std::iter::once((chunk.len(), ' ')))
            {
                if ch.is_alphabetic() {
                    start.get_or_insert(at);
                } else if let Some(from) = start.take() {
                    let word = &chunk[from..at];
                    if word.chars().count() >= 3 && !word.chars().all(char::is_uppercase) {
                        out.push((offset + from, word));
                    }
                }
            }
        }
        offset += chunk.len();
    }
    out
}

pub fn apply(text: &mut String, suggestion: &Suggestion, replacement: &str) -> bool {
    if !suggestion.replacements.iter().any(|s| s == replacement)
        || text.get(suggestion.start..suggestion.end) != Some(&suggestion.word)
    {
        return false;
    }
    text.replace_range(suggestion.start..suggestion.end, replacement);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_ranges_and_replacements_preserve_numbers_identifiers_and_source() {
        let original = "Превет, мир! hello id_123 https://example.com user@host.com 12345 NASA";
        let words = words(original);
        assert_eq!(
            words.iter().map(|(_, w)| *w).collect::<Vec<_>>(),
            ["Превет", "мир", "hello"]
        );
        let (start, word) = words[0];
        let s = Suggestion {
            start,
            end: start + word.len(),
            word: word.into(),
            replacements: vec!["Привет".into()],
        };
        let mut text = original.to_string();
        assert!(!apply(&mut text, &s, "Другое"));
        assert!(apply(&mut text, &s, "Привет"));
        assert!(text.starts_with("Привет, мир!"));
        assert!(!apply(&mut text, &s, "Привет"));
    }
}
