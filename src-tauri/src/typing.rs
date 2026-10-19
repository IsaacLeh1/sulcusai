// SPDX-License-Identifier: AGPL-3.0-only
//! Help typing in the chat boxes, each a feature the person turns on:
//! - autocomplete: the rest of a word, or of something they've asked
//!   before, from their own chats (the window does the matching);
//! - autocorrect: a misspelled word fixed when it's finished, by Windows'
//!   spell checker, only when it's sure (Windows only);
//! - a suggested next message once a reply is done, from the model that
//!   just answered (on this PC only: never an online model).
//!
//! Everything stays on this PC.

use std::collections::HashMap;

use serde::Serialize;
use serde_json::json;

use crate::features::{self, Feature};
use crate::{db, AppStateRef};

/// The person's own words and requests, for autocomplete.
#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct Vocabulary {
    /// Words of 4 letters or more, most used first, with how often.
    pub words: Vec<(String, u32)>,
    /// Things they've asked, newest first (one line, under 200 characters).
    pub prompts: Vec<String>,
}

const MAX_WORDS: usize = 8000;
const MAX_PROMPTS: usize = 400;

/// Counts words (their own count three times as much as replies') and
/// collects their short requests.
pub fn vocabulary_of<'a>(messages: impl Iterator<Item = (&'a str, &'a str)>) -> Vocabulary {
    let mut counts: HashMap<String, u32> = HashMap::new();
    let mut prompts: Vec<String> = Vec::new();
    for (role, text) in messages {
        let weight = if role == "user" { 3 } else { 1 };
        for word in text.split(|c: char| !(c.is_alphabetic() || c == '\'')).map(|w| w.trim_matches('\'')) {
            // Lowercase or capitalized words; not codes or shouting.
            let n = word.chars().count();
            if (4..=24).contains(&n) && word.chars().skip(1).all(|c| !c.is_uppercase()) {
                *counts.entry(word.to_lowercase()).or_default() += weight;
            }
        }
        let line = text.trim();
        if role == "user" && !line.contains('\n') && (8..200).contains(&line.chars().count()) && !prompts.iter().any(|p| p == line) {
            prompts.push(line.to_string());
        }
    }
    let mut words: Vec<(String, u32)> = counts.into_iter().filter(|(_, n)| *n >= 2).collect();
    words.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    words.truncate(MAX_WORDS);
    prompts.truncate(MAX_PROMPTS);
    Vocabulary { words, prompts }
}

/// Autocomplete's words and requests, from the newest chats (hidden ones
/// left out). Empty when autocomplete is off.
#[tauri::command(async)]
pub fn typing_vocabulary(state: AppStateRef) -> Result<Vocabulary, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    if !features::is_on(&conn, Feature::Autocomplete) {
        return Ok(Vocabulary::default());
    }
    let hidden = crate::hidden::list(&conn);
    let mut all: Vec<(String, String)> = Vec::new();
    for chat in db::list_chats(&conn, &c).into_iter().filter(|ch| !hidden.contains(&ch.id)).take(60) {
        let msgs = db::messages(&conn, &c, &chat.id);
        // Newest first, so the newest requests come first.
        all.extend(msgs.into_iter().rev().take(200).filter(|m| m.role == "user" || m.role == "assistant").map(|m| (m.role, m.content)));
    }
    Ok(vocabulary_of(all.iter().map(|(r, t)| (r.as_str(), t.as_str()))))
}

/// Windows' spell checker language for the window's language.
#[cfg_attr(not(windows), allow(dead_code))]
fn spell_language(lang: &str) -> &'static str {
    match lang.split(['-', '_']).next().unwrap_or("en") {
        "de" => "de-DE",
        "es" => "es-ES",
        "fr" => "fr-FR",
        "pt" => "pt-BR",
        "ja" => "ja-JP",
        "zh" => "zh-CN",
        _ => "en-US",
    }
}

/// Whether a finished word is worth checking: letters only (an apostrophe
/// allowed), 2 to 30 of them, lowercase or capitalized like a sentence's
/// first word. Not names in the middle of a sentence, codes or shouting.
pub fn checkable(word: &str) -> bool {
    let n = word.chars().count();
    (2..=30).contains(&n)
        && word.chars().all(|c| c.is_alphabetic() || c == '\'')
        && word.chars().skip(1).all(|c| !c.is_uppercase())
}

#[cfg_attr(not(windows), allow(dead_code))]
/// Edit distance between two short words, case ignored; swapping two
/// neighbouring letters counts as one ("teh" for "the").
pub fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.to_lowercase().chars().collect();
    let b: Vec<char> = b.to_lowercase().chars().collect();
    let (n, m) = (a.len(), b.len());
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for j in 0..=m {
        d[0][j] = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[n][m]
}

#[cfg_attr(not(windows), allow(dead_code))]
/// The fix to use for a word the checker flagged: its own replacement
/// ("teh" → "the"), else its first suggestion when close enough to be
/// sure (one letter off in short words, two in longer ones), in the
/// word's own case.
pub fn choose(word: &str, replacement: Option<&str>, suggestions: &[String]) -> Option<String> {
    let fix = match replacement {
        Some(r) if !r.trim().is_empty() => r.to_string(),
        _ => {
            let first = suggestions.first()?;
            let n = word.chars().count();
            let allowed = if n < 4 { return None } else if n <= 6 { 1 } else { 2 };
            if first.contains(' ') || distance(word, first) > allowed {
                return None;
            }
            first.clone()
        }
    };
    if fix.to_lowercase() == word.to_lowercase() {
        return None;
    }
    // Keep a capital the person typed (the first word of a sentence).
    let capital = word.chars().next().is_some_and(char::is_uppercase);
    Some(if capital {
        let mut c = fix.chars();
        c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or(fix)
    } else {
        fix
    })
}

#[cfg(windows)]
mod win {
    use windows::core::HSTRING;
    use windows::Win32::Globalization::{ISpellChecker, ISpellCheckerFactory, ISpellingError, SpellCheckerFactory, CORRECTIVE_ACTION_GET_SUGGESTIONS, CORRECTIVE_ACTION_REPLACE};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};

    thread_local! {
        static CHECKERS: std::cell::RefCell<std::collections::HashMap<String, Option<ISpellChecker>>> = Default::default();
    }

    fn checker(lang: &str) -> Option<ISpellChecker> {
        CHECKERS.with(|c| {
            c.borrow_mut()
                .entry(lang.to_string())
                .or_insert_with(|| unsafe {
                    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                    let factory: ISpellCheckerFactory = CoCreateInstance(&SpellCheckerFactory, None, CLSCTX_INPROC_SERVER).ok()?;
                    let tag = HSTRING::from(lang);
                    if !factory.IsSupported(&tag).ok()?.as_bool() {
                        return None;
                    }
                    factory.CreateSpellChecker(&tag).ok()
                })
                .clone()
        })
    }

    /// The checker's verdict on one word: None if it's spelled right (or the
    /// language isn't installed); else its replacement, if it has one, and
    /// its suggestions.
    pub fn check(word: &str, lang: &str) -> Option<(Option<String>, Vec<String>)> {
        let sc = checker(lang)?;
        unsafe {
            let errors = sc.Check(&HSTRING::from(word)).ok()?;
            let mut err: Option<ISpellingError> = None;
            if errors.Next(&mut err).is_err() {
                return None;
            }
            let err = err?;
            let action = err.CorrectiveAction().ok()?;
            if action == CORRECTIVE_ACTION_REPLACE {
                let p = err.Replacement().ok()?;
                let r = p.to_string().ok();
                CoTaskMemFree(Some(p.0 as _));
                return Some((r, Vec::new()));
            }
            if action != CORRECTIVE_ACTION_GET_SUGGESTIONS {
                return None;
            }
            let list = sc.Suggest(&HSTRING::from(word)).ok()?;
            let mut out = Vec::new();
            for _ in 0..3 {
                let mut one = [windows::core::PWSTR::null()];
                let mut got = 0u32;
                if list.Next(&mut one, Some(&mut got)).is_err() || got == 0 {
                    break;
                }
                if let Ok(s) = one[0].to_string() {
                    out.push(s);
                }
                CoTaskMemFree(Some(one[0].0 as _));
            }
            Some((None, out))
        }
    }
}

/// The fix for a word just finished in a chat box, if Windows' spell checker
/// is sure of one. None when autocorrect is off, the word is fine, or it's
/// a word the person uses (their own names and terms).
#[tauri::command(async)]
pub fn typing_fix(state: AppStateRef, word: String, lang: String, known: Option<bool>) -> Option<String> {
    let word = word.trim();
    if !checkable(word) || known == Some(true) || !features::is_on(&state.db.lock().unwrap(), Feature::Autocorrect) {
        return None;
    }
    #[cfg(windows)]
    {
        let (replacement, suggestions) = win::check(word, spell_language(&lang))?;
        choose(word, replacement.as_deref(), &suggestions)
    }
    #[cfg(not(windows))]
    {
        let _ = lang;
        None
    }
}

/// Whether autocorrect can work here (Windows' spell checker is there).
#[tauri::command(async)]
pub fn typing_can_correct(lang: String) -> bool {
    #[cfg(windows)]
    {
        win::check("hello", spell_language(&lang)).is_none() && win::check("helo", spell_language(&lang)).is_some()
    }
    #[cfg(not(windows))]
    {
        let _ = lang;
        false
    }
}

/// A reply that's the model answering, not a message the person would send.
fn sounds_like_a_reply(s: &str) -> bool {
    let l = s.trim_start().to_lowercase();
    ["sure", "certainly", "of course", "here is", "here's", "i can", "i'd be happy", "as an ai", "absolutely", "great question"].iter().any(|p| l.starts_with(p))
}

/// The model's guess cleaned up: one line, no quotes or label, at most 140
/// characters; None if it's empty or reads like an answer.
pub fn clean_next(raw: &str) -> Option<String> {
    let line = raw.lines().map(str::trim).find(|l| !l.is_empty())?;
    let line = line.trim_start_matches(|c: char| c == '-' || c == '*' || c == '>').trim();
    let line = line.strip_prefix("User:").or_else(|| line.strip_prefix("Next message:")).unwrap_or(line).trim();
    let line = line.trim_matches(|c: char| c == '"' || c == '“' || c == '”' || c == '\'').trim();
    if line.chars().count() < 3 || sounds_like_a_reply(line) {
        return None;
    }
    Some(if line.chars().count() > 140 { line.chars().take(140).collect::<String>().trim_end().to_string() } else { line.to_string() })
}

/// The message the person is most likely to send next in a chat, guessed by
/// the model already running on this PC (none when it's off, the chat uses
/// an online model, or no model is running: nothing is loaded for this).
#[tauri::command]
pub async fn typing_next(state: AppStateRef<'_>, chat_id: String) -> Result<Option<String>, String> {
    let c = state.cipher()?;
    let (chat, history) = {
        let conn = state.db.lock().unwrap();
        if !features::is_on(&conn, Feature::NextPrompt) {
            return Ok(None);
        }
        let Some(chat) = db::chat(&conn, &c, &chat_id) else { return Ok(None) };
        (chat.clone(), db::messages(&conn, &c, &chat_id))
    };
    if chat.model_id.as_deref().is_some_and(crate::cloud::is_cloud) {
        return Ok(None);
    }
    let Some((_, ep)) = state.engine.lock().await.current() else { return Ok(None) };
    if ep.cloud.is_some() {
        return Ok(None);
    }
    let shown: Vec<String> = history
        .iter()
        .filter(|m| (m.role == "user" || m.role == "assistant") && !m.content.trim().is_empty())
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|m| {
            let text: String = m.content.chars().take(800).collect();
            format!("{}: {text}", if m.role == "user" { "User" } else { "Assistant" })
        })
        .collect();
    if shown.is_empty() {
        return Ok(None);
    }
    let messages = vec![
        json!({ "role": "system", "content": "You guess the next message the user will send in a chat with their assistant. \
            Reply with only that message, written as the user would write it (their language and style), in under 15 words. \
            It's what they'd ask or say next, not an answer. No quotes, no label." }),
        json!({ "role": "user", "content": format!("The chat so far:\n\n{}\n\nThe user's next message:", shown.join("\n\n")) }),
    ];
    let raw = tokio::time::timeout(std::time::Duration::from_secs(12), crate::chat::complete(&ep, messages, json!({ "temperature": 0.4 }), 40))
        .await
        .map_err(|_| "Took too long.".to_string())??;
    Ok(clean_next(&raw))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vocabulary_counts_their_words_and_keeps_their_requests() {
        let msgs = [
            ("user", "Summarize my quarterly budget spreadsheet"),
            ("assistant", "Your quarterly budget shows spending rose."),
            ("user", "Summarize my quarterly budget spreadsheet"),
            ("user", "a\nmulti-line request that isn't one"),
            ("assistant", "Isaac's spreadsheet"),
        ];
        let v = vocabulary_of(msgs.iter().copied());
        let count = |w: &str| v.words.iter().find(|(x, _)| x == w).map(|(_, n)| *n);
        assert_eq!(count("quarterly"), Some(7), "twice by them (3 each) and once in a reply");
        assert_eq!(count("spreadsheet"), Some(7));
        assert_eq!(v.prompts, vec!["Summarize my quarterly budget spreadsheet".to_string()], "once, one-line only");
        assert!(count("rose").is_none(), "seen once: too rare");
    }

    #[test]
    fn only_sure_fixes_are_made() {
        // Windows' own replacement wins.
        assert_eq!(choose("teh", Some("the"), &[]), Some("the".into()));
        // Close suggestions are used, in the word's case.
        assert_eq!(choose("recieve", None, &["receive".into()]), Some("receive".into()));
        assert_eq!(choose("Recieve", None, &["receive".into()]), Some("Receive".into()));
        // Too far off, too short, or several words: left alone.
        assert_eq!(choose("qwrty", None, &["quarterly".into()]), None);
        assert_eq!(choose("abt", None, &["about".into()]), None);
        assert_eq!(choose("alot", None, &["a lot".into()]), None);
        // Which words are checked at all.
        assert!(checkable("teh") && checkable("Recieve") && checkable("don't"));
        assert!(!checkable("iPhone") && !checkable("NASA") && !checkable("x2") && !checkable("a"));
        assert_eq!(distance("teh", "the"), 1, "a swap is one");
        assert_eq!(distance("recieve", "receive"), 1);
    }

    #[test]
    fn a_next_message_guess_is_one_clean_line_from_the_user() {
        assert_eq!(clean_next("\"Can you make it shorter?\"\n").as_deref(), Some("Can you make it shorter?"));
        assert_eq!(clean_next("User: and for next week?").as_deref(), Some("and for next week?"));
        assert_eq!(clean_next("Sure! Here is a shorter version."), None);
        assert_eq!(clean_next("   \n  "), None);
        assert_eq!(clean_next(&"word ".repeat(60)).map(|s| s.chars().count() <= 140), Some(true));
    }

    /// Windows' own spell checker, where it's installed (skipped elsewhere).
    #[cfg(windows)]
    #[test]
    fn windows_spell_checker_fixes_common_typos_and_leaves_good_words() {
        let Some((r, s)) = win::check("recieve", "en-US") else { return };
        eprintln!("spell checker: recieve -> {r:?} {s:?}");
        assert_eq!(choose("recieve", r.as_deref(), &s).as_deref(), Some("receive"), "{r:?} {s:?}");
        assert!(win::check("receive", "en-US").is_none(), "spelled right");
        assert!(win::check("spreadsheet", "en-US").is_none());
    }

    #[test]
    fn languages_map_to_windows_spell_checkers() {
        assert_eq!(spell_language("en"), "en-US");
        assert_eq!(spell_language("de"), "de-DE");
        assert_eq!(spell_language("pt-BR"), "pt-BR");
    }
}
