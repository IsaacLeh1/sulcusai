// SPDX-License-Identifier: AGPL-3.0-only
//! How the reviewer checks one thing in a picture. Asked yes or no, the
//! picture reader (Qwen3.5 9B) passed a yellow cucumber as orange, a table
//! clock at ten past ten as a wall clock at half past seven, and a faint
//! dog left after "remove the dog" as no dog (the learning evaluation,
//! 2026-10-20). So whatever can be read out is read out, and code compares
//! it: the words written, the time on a clock, how many there are, a
//! thing's color. Something that must be gone is looked for even when
//! faint. Anything else is still a yes or no.

use serde_json::{json, Value};

use super::prompts;
use crate::vision::Helper;

/// What a checklist item needs looking at.
#[derive(Debug, Clone, PartialEq)]
pub enum Check {
    /// Words that must be written, spelt as here.
    Text(String),
    /// A clock showing this time (minutes after 12 on a 12-hour dial).
    Time(u32),
    /// This many of something.
    Count(u32, String),
    /// A thing (second) in a color (first).
    Color(String, String),
    /// Something that must not be there at all.
    Absent(String),
    /// Anything else: yes or no.
    Plain,
}

const NUMBERS: &[&str] = &["zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve"];

fn number(w: &str) -> Option<u32> {
    let w = w.trim().to_lowercase();
    w.parse::<u32>().ok().or_else(|| NUMBERS.iter().position(|n| *n == w).map(|i| i as u32))
}

/// Colors by their usual name, with the words that count as the same.
const COLORS: &[&[&str]] = &[
    &["red", "crimson", "scarlet", "maroon"],
    &["orange", "amber", "tangerine"],
    &["yellow", "golden", "lemon"],
    &["gold", "golden", "yellow"],
    &["green", "lime", "olive", "emerald"],
    &["blue", "navy", "azure", "cobalt", "turquoise"],
    &["purple", "violet", "lavender", "lilac", "plum"],
    &["pink", "magenta", "rose", "fuchsia"],
    &["white", "cream", "ivory"],
    &["black", "charcoal"],
    &["grey", "gray", "silver"],
    &["gray", "grey", "silver"],
    &["silver", "grey", "gray"],
    &["brown", "tan", "chocolate"],
    &["beige", "tan", "cream"],
];

fn is_color(w: &str) -> bool {
    COLORS.iter().any(|g| g[0] == w)
}

/// Words after which a thing's name has ended ("a red door on the left").
const ENDS: &[&str] = &["on", "in", "with", "and", "at", "next", "beside", "near", "under", "above", "behind", "holding", "wearing", "to", "of", "that", "which", "is", "are"];

/// The words in quotes, if any.
fn quoted(s: &str) -> Option<String> {
    let re = regex::Regex::new(r#"["“”]([^"“”]{1,80})["“”]"#).ok()?;
    re.captures(s).map(|c| c[1].trim().to_string()).filter(|w| !w.is_empty())
}

/// What an item needs looking at.
pub fn check_of(item: &str) -> Check {
    let t = item.trim().trim_end_matches('.').trim();
    if let Some(words) = quoted(t) {
        return Check::Text(words);
    }
    let low = t.to_lowercase();
    if low.starts_with("no ") {
        return Check::Absent(t[3..].trim().to_string());
    }
    if ["clock", "watch", "time"].iter().any(|w| low.contains(w)) {
        if let Some(m) = time_in(&low) {
            return Check::Time(m);
        }
    }
    let words: Vec<&str> = t.split_whitespace().collect();
    let skip = usize::from(words.first().is_some_and(|w| ["exactly", "only"].contains(&w.to_lowercase().as_str())));
    if let Some(n) = words.get(skip).and_then(|w| number(w)).filter(|n| (1..=20).contains(n)) {
        if words.len() > skip + 1 {
            return Check::Count(n, words[skip + 1..].join(" "));
        }
    }
    let ws: Vec<String> = low.split_whitespace().map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '-').to_string()).filter(|w| !w.is_empty()).collect();
    let start = usize::from(ws.first().is_some_and(|w| ["a", "an", "the"].contains(&w.as_str())));
    // "a pink lemon", "an orange cucumber on a plate"
    if let Some(color) = ws.get(start).filter(|w| is_color(w)) {
        let thing: Vec<&str> = ws[start + 1..].iter().map(String::as_str).take_while(|w| !ENDS.contains(w) && !is_color(w)).collect();
        if !thing.is_empty() && ws.get(start + 1).is_some_and(|w| !is_color(w)) {
            return Check::Color(color.clone(), thing.join(" "));
        }
    }
    // "the letters are red"
    if ws.len() >= 3 && ["is", "are"].contains(&ws[ws.len() - 2].as_str()) && is_color(&ws[ws.len() - 1]) {
        let thing = ws[start..ws.len() - 2].join(" ");
        if !thing.is_empty() {
            return Check::Color(ws[ws.len() - 1].clone(), thing);
        }
    }
    Check::Plain
}

/// A time of day in words or figures ("half past seven", "7:30", "a quarter
/// to four", "ten o'clock"), as minutes after 12 on a 12-hour dial.
pub fn time_in(s: &str) -> Option<u32> {
    let low = s.to_lowercase().replace('-', " ");
    if let Some(c) = regex::Regex::new(r"\b(\d{1,2})[:.](\d{2})\b").ok()?.captures(&low) {
        let (h, m): (u32, u32) = (c[1].parse().ok()?, c[2].parse().ok()?);
        return (h <= 24 && m < 60).then_some((h % 12) * 60 + m);
    }
    let w: Vec<&str> = low.split(|c: char| !c.is_alphanumeric() && c != '\'').filter(|w| !w.is_empty()).collect();
    let hour = |i: usize| w.get(i).and_then(|x| number(x)).filter(|h| (1..=12).contains(h));
    for i in 0..w.len() {
        // How many minutes the words from i say, and where "past" or "to" is.
        let (minutes, at): (u32, usize) = match w[i] {
            "half" => (30, i + 1),
            "quarter" => (15, i + 1),
            "twenty" if w.get(i + 1) == Some(&"five") => (25, i + 2),
            "twenty" => (20, i + 1),
            "ten" => (10, i + 1),
            "five" => (5, i + 1),
            "o'clock" | "oclock" if i > 0 => {
                if let Some(h) = hour(i - 1) {
                    return Some((h % 12) * 60);
                }
                continue;
            }
            _ => continue,
        };
        match w.get(at) {
            Some(&"past") | Some(&"after") => {
                if let Some(h) = hour(at + 1) {
                    return Some((h % 12) * 60 + minutes);
                }
            }
            Some(&"to") | Some(&"before") => {
                if let Some(h) = hour(at + 1) {
                    return Some(((h % 12) * 60 + 720 - minutes) % 720);
                }
            }
            _ => {}
        }
    }
    None
}

/// Two times on a 12-hour dial within ten minutes of each other.
pub fn times_match(a: u32, b: u32) -> bool {
    let d = (a as i32 - b as i32).rem_euclid(720);
    d.min(720 - d) <= 10
}

fn plain_letters(s: &str) -> String {
    s.to_uppercase().chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

fn edits(a: &[char], b: &[char]) -> usize {
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let here = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != cb)).min(row[j] + 1).min(here + 1);
            prev = here;
        }
    }
    row[b.len()]
}

/// The words read out of a picture include the ones wanted, spelt the same
/// (one slip in eight letters at most; spaces and line breaks don't count).
pub fn text_matches(want: &str, read: &str) -> bool {
    let want: Vec<char> = plain_letters(want).chars().filter(|c| *c != ' ').collect();
    let read: Vec<char> = plain_letters(read).chars().filter(|c| *c != ' ').collect();
    if want.is_empty() {
        return true;
    }
    let slips = (want.len() / 8).max(1);
    for len in want.len().saturating_sub(slips).max(1)..=want.len() + slips {
        if read.len() < len {
            break;
        }
        if read.windows(len).any(|w| edits(&want, w) <= slips) {
            return true;
        }
    }
    false
}

/// The color named matches the one wanted (or a word for the same color).
pub fn color_matches(want: &str, said: &str) -> bool {
    let same: &[&str] = COLORS.iter().find(|g| g[0] == want).copied().unwrap_or(&[]);
    let said = said.to_lowercase();
    said.split(|c: char| !c.is_alphabetic()).filter(|w| !w.is_empty()).any(|w| w == want || same.iter().any(|s| w == *s || w.starts_with(s)))
}

fn yes_no() -> Value {
    json!({ "type": "object", "properties": { "answer": { "type": "string", "enum": ["yes", "no"] } }, "required": ["answer"] })
}

fn one(key: &str, max: u32) -> Value {
    json!({ "type": "object", "properties": { key: { "type": "string", "maxLength": max } }, "required": [key] })
}

/// What the reviewer read out, printed for tests and by-hand runs when
/// SULCUSAI_LOOK_LOG is set (never in the app's log: it's the person's words).
fn seen(item: &str, read: &Value) {
    if std::env::var_os("SULCUSAI_LOOK_LOG").is_some() {
        eprintln!("  look: {item} -> {read}");
    }
}

/// Whether the picture (as a data URL) shows `item`.
pub async fn shown(reader: &Helper, url: &str, item: &str) -> Result<bool, String> {
    Ok(match check_of(item) {
        Check::Text(want) => {
            let q = "Read every word written anywhere in this picture, exactly as it is spelt, letter by letter (even if it is misspelt), in reading order. If there are none, answer \"none\".";
            let v = prompts::ask_about(reader, url, q, one("words", 400)).await?;
            seen(item, &v["words"]);
            text_matches(&want, v["words"].as_str().unwrap_or_default())
        }
        Check::Time(want) => {
            let q = "What time does the clock in this picture show? Look at where the short hour hand and the long minute hand point. Answer as H:MM on a 12-hour clock, or \"none\" if there is no clock.";
            let v = prompts::ask_about(reader, url, q, one("time", 20)).await?;
            seen(item, &v["time"]);
            time_in(v["time"].as_str().unwrap_or_default()).is_some_and(|t| times_match(t, want))
        }
        Check::Count(n, what) => {
            let q = format!("How many {what} are in this picture? Count each one, even partly hidden ones.");
            let v = prompts::ask_about(reader, url, &q, prompts::count_schema()).await?;
            seen(item, &v["count"]);
            v["count"].as_u64() == Some(n as u64)
        }
        // Every one of them: asked for "the kite" among four, it named one
        // color and three kites counted as missing.
        Check::Color(color, what) => {
            let q = format!("What color is each {what} in this picture? List the color of every one you can see, in one or two words each, or answer \"none\" if there is no {what}.");
            let v = prompts::ask_about(reader, url, &q, one("colors", 120)).await?;
            seen(item, &v["colors"]);
            color_matches(&color, v["colors"].as_str().unwrap_or_default())
        }
        Check::Absent(what) => {
            let q = format!("Look closely at the whole picture, including faint, blurry, see-through or partly hidden shapes. Is there any {what}, or anything shaped like one? Answer yes or no.");
            let v = prompts::ask_about(reader, url, &q, yes_no()).await?;
            seen(item, &v["answer"]);
            v["answer"].as_str() == Some("no")
        }
        Check::Plain => {
            let (q, want) = prompts::item_question(item);
            let v = prompts::ask_about(reader, url, &format!("{q} Answer yes or no."), yes_no()).await?;
            seen(item, &v["answer"]);
            (v["answer"].as_str() == Some("yes")) == want
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_item_gets_the_look_it_needs() {
        assert_eq!(check_of("a sign that says \"HARBOR BOOKS\""), Check::Text("HARBOR BOOKS".into()));
        assert_eq!(check_of("the crate says “HANDLE WITH CARE”"), Check::Text("HANDLE WITH CARE".into()));
        assert_eq!(check_of("the clock shows half past seven"), Check::Time(7 * 60 + 30));
        assert_eq!(check_of("a wall clock showing a quarter past three"), Check::Time(3 * 60 + 15));
        assert_eq!(check_of("three blue mugs on the top shelf"), Check::Count(3, "blue mugs on the top shelf".into()));
        assert_eq!(check_of("exactly four kites in the sky"), Check::Count(4, "kites in the sky".into()));
        assert_eq!(check_of("only one water bottle"), Check::Count(1, "water bottle".into()));
        assert_eq!(check_of("a pink lemon"), Check::Color("pink".into(), "lemon".into()));
        assert_eq!(check_of("an orange cucumber"), Check::Color("orange".into(), "cucumber".into()));
        assert_eq!(check_of("a blue kite in the sky"), Check::Color("blue".into(), "kite".into()));
        assert_eq!(check_of("the letters are red"), Check::Color("red".into(), "letters".into()));
        assert_eq!(check_of("no dog in the picture"), Check::Absent("dog in the picture".into()));
        assert_eq!(check_of("No bicycle left leaning on the tree."), Check::Absent("bicycle left leaning on the tree".into()));
        // Not something to read out.
        assert_eq!(check_of("a boy in a green hoodie on the left"), Check::Plain);
        assert_eq!(check_of("a red and white striped awning"), Check::Plain, "two colors: no one color to ask about");
        assert_eq!(check_of("an orange on the table"), Check::Plain, "the fruit, not a color");
        assert_eq!(check_of("the painter holds a palette in her left hand"), Check::Plain);
        assert_eq!(check_of("north light"), Check::Plain);
    }

    #[test]
    fn times_are_read_in_words_or_figures() {
        assert_eq!(time_in("7:30"), Some(450));
        assert_eq!(time_in("19:30"), Some(450));
        assert_eq!(time_in("10:10 PM"), Some(610));
        assert_eq!(time_in("a quarter to four"), Some(3 * 60 + 45));
        assert_eq!(time_in("ten o'clock"), Some(600));
        assert_eq!(time_in("twenty-five past six"), Some(6 * 60 + 25));
        assert_eq!(time_in("five to twelve"), Some(11 * 60 + 55));
        assert_eq!(time_in("twenty past nine"), Some(9 * 60 + 20));
        assert_eq!(time_in("none"), None);
        assert!(times_match(450, 455));
        assert!(!times_match(450, 610), "ten past ten isn't half past seven");
        assert!(times_match(715, 5), "across twelve");
    }

    #[test]
    fn written_words_must_be_spelt_right() {
        assert!(text_matches("MORNING BREW", "Morning Brew coffee"));
        assert!(text_matches("HAPPY 5TH", "HAPPY\n5TH"), "a line break doesn't count");
        assert!(text_matches("HANDLE WITH CARE", "HANDLE WITH CAR"), "one slip in eight letters");
        assert!(!text_matches("HANDLE WITH CARE", "HADNEL WIHT CRAE"));
        assert!(!text_matches("SUNRISE BAKERY", "SUNRSIE BAKRY"));
        assert!(!text_matches("FRESH COFFEE", "none"));
        assert!(text_matches("OPEN", "OPEN 24 HOURS"));
    }

    #[test]
    fn colors_compare_by_name_and_near_names() {
        assert!(color_matches("orange", "orange"));
        assert!(color_matches("orange", "yellow-orange"));
        assert!(!color_matches("orange", "yellow"), "the cucumber that was yellow");
        assert!(color_matches("grey", "gray"));
        assert!(color_matches("gold", "golden yellow"));
        assert!(color_matches("pink", "light pink"));
        assert!(!color_matches("pink", "red"));
        assert!(!color_matches("blue", "none"));
    }
}
