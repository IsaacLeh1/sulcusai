// SPDX-License-Identifier: AGPL-3.0-only
//! Language partner: the model sets a sentence to put into the language being
//! learned, and the answer is scored here, word by word, against the model's
//! reference. Scoring stays local and repeatable so the same answer always
//! gets the same mark, whatever the model thinks of it that day.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WordMark {
    pub word: String,
    /// "ok", "accent" (right but for an accent), "wrong" or "extra".
    pub mark: &'static str,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Score {
    pub score: u8,
    pub words: Vec<WordMark>,
    /// Reference words the answer left out.
    pub missing: Vec<String>,
}

/// Lowercase with the common Latin accents taken off, so "café" and "cafe"
/// line up and an accent slip can be told apart from a wrong word.
pub fn fold(s: &str) -> String {
    s.chars()
        .flat_map(|c| c.to_lowercase())
        .map(|c| match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ą' => 'a',
            'ç' | 'ć' | 'č' => 'c',
            'è' | 'é' | 'ê' | 'ë' | 'ę' | 'ě' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ñ' | 'ń' | 'ň' => 'n',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' => 'o',
            'ù' | 'ú' | 'û' | 'ü' | 'ů' => 'u',
            'ý' | 'ÿ' => 'y',
            'ś' | 'š' => 's',
            'ź' | 'ż' | 'ž' => 'z',
            'ł' => 'l',
            'ř' => 'r',
            'ß' => 's',
            other => other,
        })
        .collect()
}

fn words(s: &str) -> Vec<String> {
    s.split(|c: char| c.is_whitespace() || (c.is_ascii_punctuation() && c != '\'') || "¿¡«»“”„。、，！？".contains(c))
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect()
}

/// Word-level alignment (longest common subsequence on folded words), then
/// a mark per answer word. Accent slips count half.
pub fn score(reference: &str, answer: &str) -> Score {
    let r = words(reference);
    let a = words(answer);
    let rf: Vec<String> = r.iter().map(|w| fold(w)).collect();
    let af: Vec<String> = a.iter().map(|w| fold(w)).collect();
    let (n, m) = (rf.len(), af.len());
    let mut t = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            t[i][j] = if rf[i] == af[j] { t[i + 1][j + 1] + 1 } else { t[i + 1][j].max(t[i][j + 1]) };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut marks = Vec::new();
    let mut missing = Vec::new();
    let mut points = 0.0f32;
    while i < n || j < m {
        if i < n && j < m && rf[i] == af[j] {
            let exact = r[i] == a[j];
            points += if exact { 1.0 } else { 0.5 };
            marks.push(WordMark { word: a[j].clone(), mark: if exact { "ok" } else { "accent" } });
            i += 1;
            j += 1;
        } else if j < m && (i >= n || t[i][j + 1] >= t[i + 1][j]) {
            marks.push(WordMark { word: a[j].clone(), mark: if i < n { "wrong" } else { "extra" } });
            j += 1;
        } else {
            missing.push(r[i].clone());
            i += 1;
        }
    }
    // Extra words cost as much as missing ones, so padding can't raise a mark.
    let denom = n.max(m).max(1) as f32;
    let score = if n == 0 { 0 } else { ((points / denom) * 100.0).round().clamp(0.0, 100.0) as u8 };
    Score { score, words: marks, missing }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Progress {
    pub language: String,
    pub rounds: u32,
    pub total: u32,
    pub streak: u32,
    pub best_streak: u32,
    /// The last 20 scores, newest last.
    pub recent: Vec<u8>,
}

/// A pass is 80 or more; the streak counts passes in a row.
pub fn record(p: &mut Progress, s: u8) {
    p.rounds += 1;
    p.total += s as u32;
    if s >= 80 {
        p.streak += 1;
        p.best_streak = p.best_streak.max(p.streak);
    } else {
        p.streak = 0;
    }
    p.recent.push(s);
    if p.recent.len() > 20 {
        p.recent.remove(0);
    }
}

/// The level to set next: up after five strong rounds, down after weak ones.
pub fn next_level(level: u8, p: &Progress) -> u8 {
    let last: Vec<u8> = p.recent.iter().rev().take(5).copied().collect();
    if last.len() == 5 && last.iter().all(|&s| s >= 90) {
        (level + 1).min(6)
    } else if last.len() >= 3 && last.iter().take(3).all(|&s| s < 50) {
        level.saturating_sub(1).max(1)
    } else {
        level.clamp(1, 6)
    }
}

pub const LEVELS: [&str; 6] = ["A1", "A2", "B1", "B2", "C1", "C2"];

pub fn round_prompt(language: &str, level: u8, topic: &str) -> String {
    let lvl = LEVELS[(level.clamp(1, 6) - 1) as usize];
    let topic = if topic.trim().is_empty() { "everyday life".to_string() } else { topic.trim().chars().take(80).collect() };
    format!(
        "You are a friendly language partner. Write one short English sentence about {topic} for a learner at CEFR \
         level {lvl} to put into {language}, and its most natural {language} translation. Answer in JSON as \
         {{\"english\": \"...\", \"reference\": \"...\", \"hint\": \"one word or grammar point to watch\"}}."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_answer_scores_full() {
        let s = score("Je voudrais un café.", "je voudrais un café");
        assert_eq!(s.score, 100);
        assert!(s.missing.is_empty());
    }

    #[test]
    fn accent_slip_counts_half() {
        let s = score("Je voudrais un café", "Je voudrais un cafe");
        assert_eq!(s.score, 88);
        assert_eq!(s.words[3].mark, "accent");
    }

    #[test]
    fn wrong_and_missing_words_are_marked() {
        let s = score("El perro come carne", "El gato come");
        assert_eq!(s.words[1].mark, "wrong");
        assert_eq!(s.missing, vec!["perro".to_string(), "carne".to_string()]);
        assert_eq!(s.score, 50);
    }

    #[test]
    fn padding_does_not_raise_the_mark() {
        let plain = score("Hola amigo", "Hola");
        let padded = score("Hola amigo", "Hola hola hola hola");
        assert!(padded.score <= plain.score);
    }

    #[test]
    fn empty_reference_scores_zero() {
        assert_eq!(score("", "anything").score, 0);
    }

    #[test]
    fn streaks_and_levels() {
        let mut p = Progress::default();
        for _ in 0..5 {
            record(&mut p, 95);
        }
        assert_eq!(p.best_streak, 5);
        assert_eq!(next_level(2, &p), 3);
        for _ in 0..3 {
            record(&mut p, 20);
        }
        assert_eq!(p.streak, 0);
        assert_eq!(next_level(2, &p), 1);
        assert_eq!(next_level(1, &p), 1);
        for _ in 0..30 {
            record(&mut p, 70);
        }
        assert_eq!(p.recent.len(), 20);
    }
}
