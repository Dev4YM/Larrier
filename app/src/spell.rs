//! Spelling suggestions for the right-click menu, from Windows' spell
//! checker: the one the text boxes' red underlines come from.

/// A misspelled word's suggestions, best first. `None` when it is spelled
/// right or no checker covers its language.
pub fn suggest(word: &str) -> Option<Vec<String>> {
    imp::suggest(word)
}

/// Teach the checker a word (the user's dictionary), so it stops being
/// underlined.
pub fn add(word: &str) -> bool {
    imp::add(word)
}

#[cfg(windows)]
mod imp {
    use windows::core::{HSTRING, PWSTR};
    use windows::Win32::Globalization::{GetUserDefaultLocaleName, ISpellChecker, ISpellCheckerFactory, ISpellingError, SpellCheckerFactory};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};

    /// Languages to check a word in: its script's, else the user's own.
    fn languages(word: &str) -> Vec<String> {
        let has = |lo: u32, hi: u32| word.chars().any(|c| (lo..=hi).contains(&(c as u32)));
        if has(0x0600, 0x06FF) || has(0x0750, 0x077F) || has(0xFB50, 0xFDFF) || has(0xFE70, 0xFEFF) {
            return vec!["ar-SA".into(), "ar".into()];
        }
        if has(0x0590, 0x05FF) {
            return vec!["he-IL".into()];
        }
        let mut out = Vec::new();
        let mut buf = [0u16; 85];
        let n = unsafe { GetUserDefaultLocaleName(&mut buf) };
        if n > 1 {
            out.push(String::from_utf16_lossy(&buf[..n as usize - 1]));
        }
        for l in ["en-US", "en-GB"] {
            if !out.iter().any(|o| o == l) {
                out.push(l.into());
            }
        }
        out
    }

    fn checker(word: &str) -> Option<ISpellChecker> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let f: ISpellCheckerFactory = CoCreateInstance(&SpellCheckerFactory, None, CLSCTX_INPROC_SERVER).ok()?;
            for tag in languages(word) {
                let tag = HSTRING::from(tag.as_str());
                if f.IsSupported(&tag).map(|b| b.as_bool()).unwrap_or(false) {
                    if let Ok(c) = f.CreateSpellChecker(&tag) {
                        return Some(c);
                    }
                }
            }
            None
        }
    }

    pub fn suggest(word: &str) -> Option<Vec<String>> {
        let c = checker(word)?;
        let w = HSTRING::from(word);
        unsafe {
            let errors = c.Check(&w).ok()?;
            let mut first: Option<ISpellingError> = None;
            if errors.Next(&mut first).is_err() || first.is_none() {
                return None;
            }
            let list = c.Suggest(&w).ok()?;
            let mut out = Vec::new();
            while out.len() < 6 {
                let mut s = [PWSTR::null()];
                let mut n = 0u32;
                if list.Next(&mut s, Some(&mut n)).0 != 0 || n == 0 {
                    break;
                }
                if let Ok(t) = s[0].to_string() {
                    if !t.is_empty() && t != word {
                        out.push(t);
                    }
                }
                CoTaskMemFree(Some(s[0].0 as *const _));
            }
            Some(out)
        }
    }

    pub fn add(word: &str) -> bool {
        match checker(word) {
            Some(c) => unsafe { c.Add(&HSTRING::from(word)).is_ok() },
            None => false,
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn suggest(_word: &str) -> Option<Vec<String>> {
        None
    }

    pub fn add(_word: &str) -> bool {
        false
    }
}
