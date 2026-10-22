// SPDX-License-Identifier: AGPL-3.0-only
//! The listener: a song is heard the way a person would hear it. Speech
//! recognition (Whisper) says which of the lyrics come through and where
//! each line is sung. What a person would notice is written in plain words,
//! and the stretches whose words weren't clear can be made again
//! (`remake`), keeping the rest.

use serde::{Deserialize, Serialize};

use crate::speech::Segment;

/// A sung line, and how well it came through.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LineHeard {
    /// The section it's in ("Chorus").
    pub section: String,
    pub text: String,
    /// The share of its words heard (0 to 1).
    pub heard: f32,
    /// Where it's sung, in seconds (None: not found).
    pub at: Option<[f64; 2]>,
}

/// What the listener heard in a song.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Review {
    /// Lyric words heard, of all sung (0 and 0 when speech recognition
    /// wasn't there to listen).
    pub heard: usize,
    pub words: usize,
    pub lines: Vec<LineHeard>,
    /// Stretches whose words weren't clear, in seconds: what `remake` makes again.
    pub unclear: Vec<[f64; 2]>,
    /// What a person would notice.
    pub notes: Vec<String>,
    /// 0 to 100: most of it the share of words heard.
    pub score: u8,
}

/// A line under this share of its words heard isn't clear.
const CLEAR: f32 = 0.5;
/// Padding around a stretch made again (seconds), so it joins the rest smoothly.
const PAD: f64 = 1.0;

/// A word's own words: a contraction is its full words, as the lyrics and
/// speech recognition may write either ("should've" heard for "should
/// have" counted as two words missed).
fn expanded(w: &str) -> Vec<String> {
    let w = w.replace('’', "'");
    let full = |base: &str, rest: &str| vec![base.to_string(), rest.to_string()];
    let irregular = [("can't", ["can", "not"]), ("won't", ["will", "not"]), ("shan't", ["shall", "not"]), ("ain't", ["is", "not"])];
    if let Some((_, f)) = irregular.iter().find(|(c, _)| *c == w) {
        return f.iter().map(|s| s.to_string()).collect();
    }
    for (end, word) in [("n't", "not"), ("'ve", "have"), ("'re", "are"), ("'ll", "will"), ("'d", "would"), ("'m", "am")] {
        if let Some(base) = w.strip_suffix(end).filter(|b| !b.is_empty()) {
            return full(base, word);
        }
    }
    if let Some(base) = w.strip_suffix("'s").filter(|b| !b.is_empty()) {
        return if ["it", "he", "she", "that", "what", "there", "here", "who", "where", "how", "let"].contains(&base) { full(base, if base == "let" { "us" } else { "is" }) } else { vec![base.to_string()] };
    }
    vec![w.trim_matches('\'').to_string()]
}

fn words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '\'' && c != '’')
        .flat_map(expanded)
        .filter(|w| !w.is_empty())
        .collect()
}

/// The sung lines of lyrics, with their section: tags ("[Chorus - screamed]")
/// name the section; parenthesised asides and empty lines are skipped.
pub fn sung_lines(lyrics: &str) -> Vec<(String, String)> {
    let mut section = String::new();
    let mut out = Vec::new();
    for line in lyrics.lines().map(str::trim) {
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].split(" - ").next().unwrap_or("").trim().to_string();
            continue;
        }
        if line.is_empty() || (line.starts_with('(') && line.ends_with(')')) || words(line).is_empty() {
            continue;
        }
        out.push((section.clone(), line.to_string()));
    }
    out
}

/// The share of `line`'s words found in `heard` (each heard word used once).
fn overlap(line: &[String], heard: &[String]) -> f32 {
    if line.is_empty() {
        return 0.0;
    }
    let mut pool: Vec<&String> = heard.iter().collect();
    let mut found = 0;
    for w in line {
        if let Some(i) = pool.iter().position(|h| *h == w) {
            pool.swap_remove(i);
            found += 1;
        }
    }
    found as f32 / line.len() as f32
}

/// Takes `line`'s words out of the segments they were heard in, so a line
/// sung again (a chorus) is matched to where it's sung next.
fn use_up(line: &[String], heard: &mut [Vec<String>]) {
    for w in line {
        if let Some(pool) = heard.iter_mut().find(|p| p.contains(w)) {
            let i = pool.iter().position(|h| h == w).unwrap();
            pool.remove(i);
        }
    }
}

/// Each lyric line matched to what was heard, in order: a line is looked for
/// in the next few segments (alone, or with the one after for a line split
/// across two), never before the last line found, and words a line was
/// matched to are used up, so a chorus sung twice is matched twice.
pub fn match_lines(lines: &[(String, String)], segments: &[Segment]) -> Vec<LineHeard> {
    let mut heard: Vec<Vec<String>> = segments.iter().map(|s| words(&s.text)).collect();
    let mut from = 0;
    let mut out = Vec::with_capacity(lines.len());
    for (section, text) in lines {
        let w = words(text);
        let window = from..(from + 6).min(segments.len());
        let mut best: (f32, Option<(usize, usize)>) = (0.0, None);
        for j in window.clone() {
            let one = overlap(&w, &heard[j]);
            if one > best.0 {
                best = (one, Some((j, j)));
            }
        }
        // Two segments only when the line starts in the first and together
        // they hold clearly more of it than any one does.
        let single = best.0;
        for j in window.filter(|j| j + 1 < segments.len()) {
            if overlap(&w, &heard[j]) == 0.0 {
                continue;
            }
            let two: Vec<String> = heard[j].iter().chain(&heard[j + 1]).cloned().collect();
            let both = overlap(&w, &two);
            if both > single + 0.2 && both > best.0 {
                best = (both, Some((j, j + 1)));
            }
        }
        let at = best.1.filter(|_| best.0 >= 0.3).map(|(a, b)| {
            from = a;
            use_up(&w, &mut heard[a..=b]);
            [segments[a].start, segments[b].end]
        });
        out.push(LineHeard { section: section.clone(), text: text.clone(), heard: best.0, at });
    }
    out
}

/// Stretches of unclear lines, padded and joined. A line that wasn't found
/// sits between the lines around it that were.
pub fn unclear_spans(lines: &[LineHeard], length: f64) -> Vec<[f64; 2]> {
    let mut spans: Vec<[f64; 2]> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if l.heard >= CLEAR {
            continue;
        }
        let at = l.at.or_else(|| {
            let before = lines[..i].iter().rev().find_map(|x| x.at).map_or(0.0, |a| a[1]);
            let after = lines[i + 1..].iter().find_map(|x| x.at).map_or(length, |a| a[0]);
            (after > before).then_some([before, after])
        });
        let Some([a, b]) = at else { continue };
        let span = [(a - PAD).max(0.0), (b + PAD).min(length)];
        match spans.last_mut() {
            Some(last) if span[0] <= last[1] => last[1] = last[1].max(span[1]),
            _ => spans.push(span),
        }
    }
    spans
}

fn clock(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// The review of a song. `heard`: what speech recognition heard (None: no
/// speech model to listen with); `length` in seconds.
pub fn review(lyrics: &str, heard: Option<&[Segment]>, length: f64) -> Review {
    let lines = sung_lines(lyrics);
    let mut r = Review::default();
    match heard {
        Some(segments) if !lines.is_empty() => {
            r.lines = match_lines(&lines, segments);
            r.words = r.lines.iter().map(|l| words(&l.text).len()).sum();
            r.heard = r.lines.iter().map(|l| (l.heard * words(&l.text).len() as f32).round() as usize).sum();
            r.unclear = unclear_spans(&r.lines, length);
            let share = r.heard as f32 / r.words.max(1) as f32;
            if r.unclear.is_empty() {
                r.notes.push(format!("The words come through clearly ({} of {} heard).", r.heard, r.words));
            } else {
                for s in &r.unclear {
                    let sections: Vec<String> = r.lines.iter().filter(|l| l.heard < CLEAR && l.at.is_none_or(|a| a[1] > s[0] && a[0] < s[1])).map(|l| l.section.clone()).filter(|x| !x.is_empty()).fold(Vec::new(), |mut v, x| {
                        if !v.contains(&x) {
                            v.push(x);
                        }
                        v
                    });
                    let in_part = if sections.is_empty() { String::new() } else { format!(" ({})", sections.join(", ")) };
                    r.notes.push(format!("The words from {} to {}{in_part} aren't clear.", clock(s[0]), clock(s[1])));
                }
            }
            r.score = (share * 100.0).round() as u8;
        }
        Some(_) => r.score = 100,
        None => r.notes.push("Install a speech model (Models, Speech) and the listener also hears which words come through.".into()),
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start: f64, end: f64, text: &str) -> Segment {
        Segment { start, end, text: text.into() }
    }

    const LYRICS: &str = "[Verse]\nMorning light on the window\nCoffee warm in my hands\n\n[Chorus - belting]\nSing it loud, let it carry\nOver rooftops and trees\n\n[Chorus]\nSing it loud, let it carry\nOver rooftops and trees";

    #[test]
    fn lines_are_matched_in_order_and_a_repeated_chorus_twice() {
        let heard = [
            seg(2.0, 5.0, "Morning light on the window"),
            seg(5.0, 8.0, "coffee warm in my hands"),
            seg(10.0, 14.0, "Sing it loud, let it carry over rooftops and trees"),
            seg(30.0, 33.0, "sing it loud let it carry"),
            seg(33.0, 36.0, "mumble mumble"),
        ];
        let lines = match_lines(&sung_lines(LYRICS), &heard);
        assert_eq!(lines.len(), 6);
        assert_eq!(lines[0].section, "Verse");
        assert_eq!(lines[2].section, "Chorus", "a tag's effect isn't part of the section");
        assert_eq!(lines[2].at, Some([10.0, 14.0]));
        assert_eq!(lines[3].at, Some([10.0, 14.0]), "two lines in one segment");
        assert_eq!(lines[4].at, Some([30.0, 33.0]), "the second chorus is the later one");
        assert!(lines[5].heard < 0.3 && lines[5].at.is_none());
        let r = review(LYRICS, Some(&heard), 40.0);
        assert_eq!((r.heard, r.words), (26, 30));
        assert_eq!(r.unclear, vec![[32.0, 40.0]], "the last line, after the one before it, to the end");
        assert!(r.notes[0].contains("0:32 to 0:40 (Chorus)"), "{:?}", r.notes);
        assert_eq!(r.score, 87);
    }

    /// Contractions count as their full words either way they're written.
    #[test]
    fn contractions_are_their_full_words() {
        assert_eq!(words("I should've held on"), ["i", "should", "have", "held", "on"]);
        assert_eq!(words("I'm sorry, I don't know, it's late"), ["i", "am", "sorry", "i", "do", "not", "know", "it", "is", "late"]);
        assert_eq!(words("Can't stop, won't stop, John's car"), ["can", "not", "stop", "will", "not", "stop", "john", "car"]);
        assert_eq!(overlap(&words("I should have held on"), &words("I should've held on")), 1.0);
    }

    #[test]
    fn a_clear_song_and_one_with_no_listener() {
        let heard = [seg(0.0, 4.0, "Morning light on the window, coffee warm in my hands")];
        let r = review("[Verse]\nMorning light on the window\nCoffee warm in my hands", Some(&heard), 10.0);
        assert!(r.unclear.is_empty() && r.score == 100 && r.notes[0].contains("clearly"), "{r:?}");
        let r = review(LYRICS, None, 40.0);
        assert_eq!((r.score, r.notes.len()), (0, 1));
        assert!(r.notes[0].contains("Install a speech model"), "{:?}", r.notes);
    }
}
