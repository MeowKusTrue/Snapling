//! Local Hunspell protocol; dictionaries are supplied by the user's package manager.
use snapling_core::spelling::{self, Suggestion};
use std::io::Write;
use std::process::{Command, Stdio};

pub fn check(text: &str) -> Result<Vec<Suggestion>, String> {
    let words: Vec<_> = spelling::words(text).into_iter().take(256).collect();
    if words.is_empty() {
        return Ok(Vec::new());
    }
    let mut child = Command::new("hunspell").args(["-a", "-i", "UTF-8", "-d", "ru_RU,en_US"])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
        .map_err(|e| format!("Для подсказок установи Hunspell и словари ru_RU/en_US (hunspell-ru, hunspell-en_us): {e}"))?;
    let input = words
        .iter()
        .map(|(_, word)| format!("^{word}\n"))
        .collect::<String>();
    // A writer prevents pipe deadlock on large suggestion responses.
    let stdin = child.stdin.take().ok_or("Hunspell: stdin unavailable")?;
    let writer = std::thread::spawn(move || {
        let mut stdin = stdin;
        stdin.write_all(input.as_bytes())
    });
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    writer
        .join()
        .map_err(|_| "Hunspell: input worker failed")?
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "Hunspell: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(parse(&words, &String::from_utf8_lossy(&output.stdout)))
}

fn parse(words: &[(usize, &str)], output: &str) -> Vec<Suggestion> {
    let mut proposals = Vec::new();
    let mut lines = output
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('@'));
    for &(start, word) in words {
        let Some(line) = lines.next() else {
            break;
        };
        if !line.starts_with('&') && !line.starts_with('?') {
            continue;
        }
        let Some((_, candidates)) = line.split_once(':') else {
            continue;
        };
        let replacements: Vec<_> = candidates
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty() && *s != word)
            .take(5)
            .map(str::to_string)
            .collect();
        if !replacements.is_empty() {
            proposals.push(Suggestion {
                start,
                end: start + word.len(),
                word: word.into(),
                replacements,
            });
        }
    }
    proposals
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hunspell_protocol_keeps_unicode_ranges_and_ignores_correct_words() {
        let input = "Превет hello";
        let proposals = parse(
            &spelling::words(input),
            "@(#) Hunspell\n& Превет 1 1: Привет, Привёт\n\n*\n\n",
        );
        assert_eq!(proposals.len(), 1);
        let mut text = input.to_string();
        assert!(spelling::apply(&mut text, &proposals[0], "Привет"));
        assert_eq!(text, "Привет hello");
    }
}
