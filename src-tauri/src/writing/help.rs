// SPDX-License-Identifier: AGPL-3.0-only
//! Writing help: how a text reads (sentence length, reading ease), its tone,
//! and grammar slips. The checks are plain rules so they run instantly and
//! without a model; the model is only asked for a rewrite in another tone.

use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Issue {
    /// Byte offsets into the text, for highlighting.
    pub start: usize,
    pub end: usize,
    pub kind: &'static str,
    pub message: String,
    pub fix: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Report {
    pub words: usize,
    pub sentences: usize,
    pub avg_sentence: f32,
    /// Flesch reading ease, 0 (hard) to 100 (easy).
    pub ease: f32,
    /// Rough school grade (Flesch-Kincaid).
    pub grade: f32,
    pub long_sentences: usize,
    /// "formal", "friendly", "neutral", "urgent" or "unsure".
    pub tone: &'static str,
    pub issues: Vec<Issue>,
}

fn syllables(word: &str) -> usize {
    let w: Vec<char> = word.to_lowercase().chars().filter(|c| c.is_alphabetic()).collect();
    if w.is_empty() {
        return 0;
    }
    let vowel = |c: char| "aeiouy".contains(c);
    let mut n = 0;
    let mut prev = false;
    for &c in &w {
        let v = vowel(c);
        if v && !prev {
            n += 1;
        }
        prev = v;
    }
    // A silent final "e" ("make"), but not "-le" ("table").
    if w.len() > 2 && w[w.len() - 1] == 'e' && w[w.len() - 2] != 'l' && n > 1 {
        n -= 1;
    }
    n.max(1)
}

fn sentences(text: &str) -> Vec<&str> {
    text.split_inclusive(['.', '!', '?', '\n']).map(str::trim).filter(|s| s.chars().any(char::is_alphanumeric)).collect()
}

fn tone(text: &str) -> &'static str {
    let l = format!(" {} ", text.to_lowercase());
    let count = |list: &[&str]| list.iter().filter(|w| l.contains(*w)).count();
    let formal = count(&[" dear ", " sincerely", " regards", " therefore", " furthermore", " pursuant", " kindly ", " we would like to "]);
    let friendly = count(&[" hey", " hi ", "thanks!", " cheers", " awesome", " :)", " lol", " haha", " gonna ", " wanna "]);
    let urgent = count(&[" asap", " urgent", " immediately", " right now", " deadline", " at once"]) + text.matches("!!").count();
    let caps = text.split_whitespace().filter(|w| w.len() > 3 && w.chars().all(|c| !c.is_lowercase()) && w.chars().any(char::is_alphabetic)).count();
    let urgent = urgent + caps / 2;
    match (formal, friendly, urgent) {
        (_, _, u) if u >= 2 => "urgent",
        (f, g, _) if f > g => "formal",
        (f, g, _) if g > f => "friendly",
        (0, 0, _) => "neutral",
        _ => "unsure",
    }
}

fn starts_with_vowel_sound(word: &str) -> bool {
    let w = word.to_lowercase();
    // Words whose spelling and sound disagree.
    if ["hour", "honest", "honor", "heir"].iter().any(|p| w.starts_with(p)) {
        return true;
    }
    if ["uni", "use", "usu", "uti", "eu", "one", "once", "ur"].iter().any(|p| w.starts_with(p)) {
        return false;
    }
    w.starts_with(['a', 'e', 'i', 'o', 'u'])
}

fn tokens(text: &str) -> Vec<(usize, &str)> {
    let mut toks = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices() {
        let wordy = c.is_alphanumeric() || c == '\'';
        match (wordy, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                toks.push((s, &text[s..i]));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        toks.push((s, &text[s..]));
    }
    toks
}

fn grammar(text: &str) -> Vec<Issue> {
    let mut out = Vec::new();
    let toks = tokens(text);
    for w in toks.windows(2) {
        let ((s0, a), (s1, b)) = (w[0], w[1]);
        let between = &text[s0 + a.len()..s1];
        if !between.trim().is_empty() {
            continue;
        }
        if a.eq_ignore_ascii_case(b) && a.chars().any(char::is_alphabetic) {
            out.push(Issue { start: s0, end: s1 + b.len(), kind: "repeat", message: format!("\"{a}\" twice in a row"), fix: Some(a.to_string()) });
        } else if b.chars().next().is_some_and(char::is_alphabetic) {
            if a == "a" && starts_with_vowel_sound(b) {
                out.push(Issue { start: s0, end: s0 + 1, kind: "article", message: format!("\"an {b}\" reads better"), fix: Some("an".into()) });
            } else if a == "an" && !starts_with_vowel_sound(b) {
                out.push(Issue { start: s0, end: s0 + 2, kind: "article", message: format!("\"a {b}\" reads better"), fix: Some("a".into()) });
            }
        }
    }
    for &(s, w) in &toks {
        if w == "i" || w.starts_with("i'") {
            out.push(Issue { start: s, end: s + 1, kind: "capital", message: "\"I\" is written as a capital".into(), fix: Some("I".into()) });
        }
    }
    let mut i = 0;
    while let Some(off) = text[i..].find("  ") {
        let at = i + off;
        let run = text[at..].chars().take_while(|&c| c == ' ').count();
        out.push(Issue { start: at, end: at + run, kind: "space", message: "extra spaces".into(), fix: Some(" ".into()) });
        i = at + run;
    }
    let mut after_stop = true;
    for (i, c) in text.char_indices() {
        if after_stop && c.is_alphabetic() {
            let abbrev = ["e.g. ", "i.e. ", "etc. ", "vs. "].iter().any(|a| text[..i].to_lowercase().ends_with(a));
            if c.is_lowercase() && !abbrev {
                out.push(Issue { start: i, end: i + c.len_utf8(), kind: "capital", message: "a sentence starts with a capital".into(), fix: Some(c.to_uppercase().collect()) });
            }
            after_stop = false;
        } else if matches!(c, '.' | '!' | '?') {
            after_stop = true;
        } else if !c.is_whitespace() && !matches!(c, '"' | '\'' | '(' | ')') {
            after_stop = false;
        }
    }
    out.sort_by_key(|x| x.start);
    out.dedup_by_key(|x| x.start);
    out
}

pub fn check(text: &str) -> Report {
    let sents = sentences(text);
    let ws: Vec<&str> = text.split_whitespace().filter(|w| w.chars().any(char::is_alphabetic)).collect();
    let words = ws.len();
    let nsent = sents.len().max(usize::from(words > 0));
    let syl: usize = ws.iter().map(|w| syllables(w)).sum();
    let (ease, grade, avg) = if words == 0 {
        (0.0, 0.0, 0.0)
    } else {
        let wps = words as f32 / nsent as f32;
        let spw = syl as f32 / words as f32;
        ((206.835 - 1.015 * wps - 84.6 * spw).clamp(0.0, 100.0), (0.39 * wps + 11.8 * spw - 15.59).max(0.0), wps)
    };
    Report {
        words,
        sentences: nsent,
        avg_sentence: (avg * 10.0).round() / 10.0,
        ease: ease.round(),
        grade: (grade * 10.0).round() / 10.0,
        long_sentences: sents.iter().filter(|s| s.split_whitespace().count() > 25).count(),
        tone: if words == 0 { "neutral" } else { tone(text) },
        issues: grammar(text),
    }
}

/// Applies every fix, last first so earlier offsets stay valid; overlapping
/// fixes after the first are skipped.
pub fn apply_fixes(text: &str, issues: &[Issue]) -> String {
    let mut out = text.to_string();
    let mut sorted: Vec<&Issue> = issues.iter().filter(|i| i.fix.is_some()).collect();
    sorted.sort_by_key(|i| std::cmp::Reverse(i.start));
    let mut floor = usize::MAX;
    for i in sorted {
        if i.end <= floor && i.end <= out.len() && out.is_char_boundary(i.start) && out.is_char_boundary(i.end) {
            out.replace_range(i.start..i.end, i.fix.as_deref().unwrap_or_default());
            floor = i.start;
        }
    }
    out
}

pub const TONES: &[&str] = &["formal", "friendly", "concise", "warm", "confident", "plain"];

pub fn rewrite_prompt(tone: &str) -> Result<String, String> {
    if !TONES.contains(&tone) {
        return Err(format!("Pick one of: {}.", TONES.join(", ")));
    }
    Ok(format!(
        "Rewrite the user's text so it reads {tone}. Keep the meaning, facts, names and the language it is written in. \
         Fix grammar and spelling. Reply with only the rewritten text."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syllable_counts() {
        assert_eq!(syllables("cat"), 1);
        assert_eq!(syllables("make"), 1);
        assert_eq!(syllables("table"), 2);
        assert_eq!(syllables("beautiful"), 3);
    }

    #[test]
    fn easy_and_hard_text() {
        let r = check("The cat sat. The dog ran. We had fun.");
        assert_eq!(r.sentences, 3);
        assert!(r.ease > 90.0, "{}", r.ease);
        let hard = check("Notwithstanding considerable organizational complexity, institutional stakeholders systematically underestimated implementation difficulties.");
        assert!(hard.ease < 20.0 && hard.grade > 15.0);
    }

    #[test]
    fn grammar_slips_are_found_and_fixed() {
        let text = "i think this is is a apple.  it was an useful day.";
        let r = check(text);
        let kinds: Vec<&str> = r.issues.iter().map(|i| i.kind).collect();
        for k in ["repeat", "article", "space", "capital"] {
            assert!(kinds.contains(&k), "{kinds:?}");
        }
        assert_eq!(apply_fixes(text, &r.issues), "I think this is an apple. It was a useful day.");
    }

    #[test]
    fn hour_and_university_articles() {
        assert!(check("It took an hour.").issues.is_empty());
        assert!(check("She went to a university.").issues.is_empty());
        assert_eq!(check("It took a hour.").issues[0].kind, "article");
    }

    #[test]
    fn abbreviations_do_not_need_capitals() {
        assert!(check("Bring fruit, e.g. apples.").issues.is_empty());
    }

    #[test]
    fn tone_is_read() {
        assert_eq!(check("Dear Ms Lee, we would like to confirm the date. Sincerely, Sam").tone, "formal");
        assert_eq!(check("hey! thanks! gonna be awesome").tone, "friendly");
        assert_eq!(check("Send it ASAP!! This is URGENT").tone, "urgent");
        assert_eq!(check("The report is attached.").tone, "neutral");
    }

    #[test]
    fn empty_text() {
        let r = check("   ");
        assert_eq!((r.words, r.sentences), (0, 0));
    }

    #[test]
    fn unknown_tone_is_refused() {
        assert!(rewrite_prompt("sarcastic").is_err());
        assert!(rewrite_prompt("formal").unwrap().contains("formal"));
    }
}
