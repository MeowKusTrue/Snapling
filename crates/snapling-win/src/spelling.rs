//! The installed Windows spelling dictionaries; no text leaves the machine.
use snapling_core::spelling::{self, Suggestion};
use windows::Win32::Globalization::{ISpellChecker, ISpellCheckerFactory, SpellCheckerFactory};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize,
};
use windows::core::{HSTRING, PWSTR};

pub fn check(text: &str) -> Result<Vec<Suggestion>, String> {
    // This API runs on a dedicated UI worker. Balance even an existing apartment.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok() }.map_err(|e| e.to_string())?;
    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }
    let _apartment = Apartment;
    check_inner(text).map_err(|e| {
        format!(
            "Словарь Windows: {e}. Проверь установленные языки ru-RU и en-US в параметрах Windows."
        )
    })
}

fn check_inner(text: &str) -> windows::core::Result<Vec<Suggestion>> {
    unsafe {
        let factory: ISpellCheckerFactory =
            CoCreateInstance(&SpellCheckerFactory, None, CLSCTX_INPROC_SERVER)?;
        let mut ru: Option<ISpellChecker> = None;
        let mut en: Option<ISpellChecker> = None;
        let mut suggestions = Vec::new();
        for (start, word) in spelling::words(text).into_iter().take(256) {
            let russian = word
                .chars()
                .any(|c| ('А'..='я').contains(&c) || c == 'ё' || c == 'Ё');
            let cached = if russian { &mut ru } else { &mut en };
            let checker = match cached {
                Some(checker) => checker,
                None => cached.insert(factory.CreateSpellChecker(&HSTRING::from(if russian {
                    "ru-RU"
                } else {
                    "en-US"
                }))?),
            };
            let mut error = None;
            checker.Check(&HSTRING::from(word))?.Next(&mut error).ok()?;
            if error.is_none() {
                continue;
            }
            let candidates = checker.Suggest(&HSTRING::from(word))?;
            let mut replacements = Vec::new();
            for _ in 0..5 {
                let mut value = [PWSTR::null()];
                let mut count = 0;
                candidates.Next(&mut value, Some(&mut count)).ok()?;
                if count == 0 {
                    break;
                }
                let replacement = value[0].to_string();
                CoTaskMemFree(Some(value[0].0.cast()));
                if let Ok(s) = replacement
                    && s != word
                    && !replacements.contains(&s)
                {
                    replacements.push(s);
                }
            }
            if !replacements.is_empty() {
                suggestions.push(Suggestion {
                    start,
                    end: start + word.len(),
                    word: word.into(),
                    replacements,
                });
            }
        }
        Ok(suggestions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires the installed Windows English spelling dictionary"]
    fn installed_dictionary_proposes_without_changing_the_source() {
        let text = "helllo world";
        let proposals = check(text).unwrap();
        assert!(
            proposals
                .iter()
                .any(|p| p.word == "helllo" && p.replacements.iter().any(|s| s == "hello")),
            "{proposals:?}"
        );
        assert_eq!(text, "helllo world");
    }
}
