//! Spelling: the red underlines in Larrier's text boxes and the
//! corrections in their right-click menu. Latin-script words are checked
//! with Hunspell, the engine Chrome's spelling is built on, and the English
//! dictionary from SCOWL (app/dict/en, the word list Chrome's English
//! dictionary comes from); other scripts with Windows' spell checker.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, RwLock};

static DICT: OnceLock<RwLock<spellbook::Dictionary>> = OnceLock::new();
/// Changes when the user's word list does: underlines are drawn again.
pub static WORDS: dioxus::prelude::GlobalSignal<u64> = dioxus::prelude::Signal::global(|| 0);
/// Words already checked: a long message is checked again on every key.
static SEEN: Mutex<Option<HashMap<String, bool>>> = Mutex::new(None);

/// The English dictionary with the user's own words, built on first use
/// (`warm` builds it ahead, off the UI thread).
fn dict() -> &'static RwLock<spellbook::Dictionary> {
    DICT.get_or_init(|| {
        let mut d = spellbook::Dictionary::new(include_str!("../dict/en/index.aff"), include_str!("../dict/en/index.dic")).expect("the bundled dictionary parses");
        for w in user_words() {
            let _ = d.add(&w);
        }
        RwLock::new(d)
    })
}

/// Build the dictionary now, so the first check doesn't wait for it.
pub fn warm() {
    std::thread::spawn(|| {
        dict();
    });
}

fn latin(word: &str) -> bool {
    word.chars().all(|c| c.is_ascii_alphabetic() || c == '\'' || ('\u{00C0}'..='\u{024F}').contains(&c))
}

/// Words the user added ("Add to Dictionary"), one per line.
fn user_file() -> std::path::PathBuf {
    larrier_core::store::data_root().join("dictionary.txt")
}

fn user_words() -> Vec<String> {
    std::fs::read_to_string(user_file()).unwrap_or_default().lines().map(str::trim).filter(|w| !w.is_empty()).map(String::from).collect()
}

/// Typographic apostrophes read as plain ones.
fn plain(word: &str) -> String {
    word.replace('\u{2019}', "'")
}

/// Whether a word is misspelled: checked with Hunspell, so it is cheap
/// enough for every key (`false` while the dictionary is still being built,
/// and for other scripts, which only the right-click menu checks).
pub fn misspelled(word: &str) -> bool {
    let word = plain(word);
    if !latin(&word) {
        return false;
    }
    if let Some(bad) = SEEN.lock().ok().and_then(|s| s.as_ref().and_then(|m| m.get(&word).copied())) {
        return bad;
    }
    let Some(d) = DICT.get() else { return false };
    let bad = !d.read().map(|d| d.check(&word)).unwrap_or(true);
    if let Ok(mut s) = SEEN.lock() {
        let m = s.get_or_insert_with(HashMap::new);
        if m.len() > 20_000 {
            m.clear();
        }
        m.insert(word, bad);
    }
    bad
}

/// The misspelled words in `text` to underline: (start, end) byte ranges.
/// As in Chrome, the word being typed at the end isn't marked yet; nor are
/// pieces of paths, addresses, code names and numbers.
pub fn underlined(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    if DICT.get().is_none() {
        return out;
    }
    let is_word = |c: char| c.is_alphabetic() || c == '\'' || c == '\u{2019}';
    let glued = |c: Option<char>| c.is_some_and(|c| c.is_ascii_digit() || "/\\._-@#:=<>`$~%&+*|[]{}()".contains(c));
    let chars: Vec<(usize, char)> = text.char_indices().take(30_000).collect();
    let mut i = 0;
    while i < chars.len() {
        if !is_word(chars[i].1) {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && is_word(chars[i].1) {
            i += 1;
        }
        let (a, b) = (chars[start].0, chars.get(i).map(|c| c.0).unwrap_or(text.len()));
        let quotes = |c: char| c == '\'' || c == '\u{2019}';
        let word = text[a..b].trim_matches(quotes);
        let before = start.checked_sub(1).map(|j| chars[j].1);
        let after = chars.get(i).map(|c| c.1);
        let typing = b == text.len();
        if word.chars().count() > 1 && !typing && !glued(before) && !glued(after) && misspelled(word) {
            let off = text[a..b].find(word).unwrap_or(0);
            out.push((a + off, a + off + word.len()));
        }
    }
    out
}

/// A misspelled word's suggestions, best first. `None` when it is spelled
/// right or no checker covers its language.
pub fn suggest(word: &str) -> Option<Vec<String>> {
    let w = plain(word);
    if latin(&w) {
        let d = dict().read().ok()?;
        if d.check(&w) {
            return None;
        }
        let mut out = Vec::new();
        d.suggest(&w, &mut out);
        out.retain(|s| s != &w);
        out.truncate(6);
        return Some(out);
    }
    imp::suggest(word)
}

/// Teach the checkers a word (Larrier's own word list, and Windows' for the
/// scripts it checks), so it stops being underlined.
pub fn add(word: &str) -> bool {
    let w = plain(word);
    let mut words = user_words();
    if !words.iter().any(|x| x == &w) {
        words.push(w.clone());
        if std::fs::write(user_file(), words.join("\n") + "\n").is_err() {
            return false;
        }
    }
    if let Ok(mut d) = dict().write() {
        let _ = d.add(&w);
    }
    if let Ok(mut s) = SEEN.lock() {
        if let Some(m) = s.as_mut() {
            m.remove(&w);
        }
    }
    let _ = imp::add(word);
    true
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hunspell_corrects_like_chrome() {
        let s = |w: &str| suggest(w).unwrap_or_default();
        assert!(s("recieve").contains(&"receive".to_string()), "{:?}", s("recieve"));
        assert!(s("definately").contains(&"definitely".to_string()), "{:?}", s("definately"));
        assert_eq!(suggest("receive"), None, "a word spelled right has no corrections");
        assert_eq!(suggest("don't"), None);
        assert!(suggest("fdSsf").is_some(), "nonsense is misspelled");
    }

    #[test]
    fn underlines_finished_words_only() {
        dict();
        let text = "teh parser in src/foo.rs and `helpr` at https://exmaple.com needs recieve";
        let marked: Vec<&str> = underlined(text).into_iter().map(|(a, b)| &text[a..b]).collect();
        // The last word is still being typed; paths, code and links aren't words.
        assert_eq!(marked, ["teh"]);
        let done = "please recieve it ";
        assert_eq!(underlined(done).into_iter().map(|(a, b)| &done[a..b]).collect::<Vec<_>>(), ["recieve"]);
    }
}
