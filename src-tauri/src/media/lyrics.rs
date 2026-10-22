// SPDX-License-Identifier: AGPL-3.0-only
//! Lyrics for songs. acestep.cpp sings the `lyrics` it's given: section tags
//! on their own lines ("[Verse]", "[Chorus]") and the lines to sing. Given
//! "[Instrumental]" it doesn't sing; given "" its own music language model
//! writes lyrics from the description. Songs with vocals get lyrics written
//! here first (by the chat model, else another language model on this PC),
//! before the music engine takes the graphics card.

use std::sync::Arc;

use serde_json::json;

use crate::AppState;

/// What the music engine gets as lyrics.
#[derive(Debug, Clone, PartialEq)]
pub enum Words {
    /// No singing.
    Instrumental,
    /// These lines are sung (tidied into the engine's form).
    Sung(String),
    /// Nothing written here: the music model writes its own from the description.
    MusicModelWrites,
}

impl Words {
    /// The `lyrics` field of the engine's request.
    pub fn for_engine(&self) -> &str {
        match self {
            Words::Instrumental => INSTRUMENTAL,
            Words::Sung(l) => l,
            Words::MusicModelWrites => "",
        }
    }

    pub fn wants_vocals(&self) -> bool {
        !matches!(self, Words::Instrumental)
    }
}

/// The DiT was trained with this exact string as "no vocals".
pub const INSTRUMENTAL: &str = "[Instrumental]";

/// What to sing: nothing for sound effects and instrumentals, the person's
/// own lyrics or the ones written for the song when there are any, else the
/// music model writes them.
pub fn words(sound: bool, instrumental: bool, given: Option<&str>) -> Words {
    if sound || instrumental {
        return Words::Instrumental;
    }
    match given.map(tidy).filter(|l| sung_lines(l) > 0) {
        Some(l) => Words::Sung(l),
        None => Words::MusicModelWrites,
    }
}

/// Section names that become tags ("Verse 2:" → "[Verse 2]").
const SECTIONS: &[&str] = &[
    "verse", "chorus", "pre-chorus", "prechorus", "pre chorus", "post-chorus", "bridge", "intro", "outro", "hook", "refrain", "interlude", "breakdown", "drop",
    "final chorus", "instrumental", "instrumental break", "solo", "guitar solo", "coda", "tag", "rap", "spoken",
];

/// A line that names a section, as the engine's tag; None for a sung line.
fn section_tag(line: &str) -> Option<String> {
    let t = line.trim();
    // Markdown dressing: "**Chorus**", "## Verse 1", "_Bridge_".
    let bare = t.trim_matches(|c: char| matches!(c, '*' | '#' | '_' | ' ')).trim();
    let inner = bare
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .or_else(|| bare.strip_prefix('(').and_then(|s| s.strip_suffix(')')))
        .unwrap_or(bare)
        .trim()
        .trim_end_matches(':')
        .trim();
    if inner.is_empty() || inner.chars().count() > 30 {
        return None;
    }
    let low = inner.to_lowercase();
    // "Verse 1", "Chorus x2", "Verse 2 - softer".
    let name = low.split([' ', '-', ':']).filter(|w| !w.is_empty()).collect::<Vec<_>>();
    let known = SECTIONS.iter().any(|s| {
        let words: Vec<&str> = s.split([' ', '-']).collect();
        name.len() >= words.len()
            && name.iter().zip(&words).all(|(a, b)| a == b)
            && name[words.len()..].iter().all(|w| w.chars().all(|c| c.is_ascii_digit()) || matches!(*w, "x2" | "x3" | "2x" | "3x" | "repeat" | "again"))
    });
    // An already-tagged line keeps its own words ("[Chorus - female]").
    if bare.starts_with('[') && bare.ends_with(']') {
        return Some(format!("[{inner}]"));
    }
    if !known {
        return None;
    }
    let mut label = String::new();
    for (i, w) in inner.split(' ').filter(|w| !w.is_empty()).enumerate() {
        if i > 0 {
            label.push(' ');
        }
        let mut cs = w.chars();
        if let Some(f) = cs.next() {
            label.extend(f.to_uppercase());
            label.push_str(cs.as_str());
        }
    }
    Some(format!("[{label}]"))
}

/// "Here are the lyrics:", "Sure! Here's a song about…:", "Title: …".
fn is_preamble(line: &str) -> bool {
    let low = line.trim().trim_matches(|c: char| matches!(c, '*' | '#' | '_')).trim().to_lowercase();
    let opener = ["here", "sure", "okay", "ok,", "certainly", "of course", "below"].iter().any(|w| low.starts_with(w));
    (opener && (low.ends_with(':') || low.contains("lyric") || low.contains("song"))) || low.starts_with("title:") || low.starts_with("title -") || low.starts_with("song title")
}

/// Lyrics in the engine's form: section tags in square brackets on their
/// own lines, the sung lines without quotes or Markdown, one blank line
/// between sections, and a "[Verse]" tag first when there's none.
pub fn tidy(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut started = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with("```") {
            continue;
        }
        if !started && (line.is_empty() || is_preamble(line)) {
            continue;
        }
        if line.is_empty() {
            if out.last().is_some_and(|l| !l.is_empty()) {
                out.push(String::new());
            }
            continue;
        }
        started = true;
        if let Some(tag) = section_tag(line) {
            if out.last().is_some_and(|l| !l.is_empty()) {
                out.push(String::new());
            }
            out.push(tag);
            continue;
        }
        // A sung line: no Markdown emphasis or surrounding quotes.
        let mut l = line.trim_matches(|c: char| matches!(c, '*' | '_')).trim().to_string();
        for (a, b) in [('"', '"'), ('\u{201c}', '\u{201d}'), ('\'', '\'')] {
            if l.len() > 2 && l.starts_with(a) && l.ends_with(b) {
                l = l[a.len_utf8()..l.len() - b.len_utf8()].trim().to_string();
            }
        }
        if !l.is_empty() {
            out.push(l);
        }
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    if out.iter().any(|l| !l.is_empty()) && !out.iter().any(|l| l.starts_with('[') && l.ends_with(']')) {
        out.insert(0, "[Verse]".into());
    }
    out.join("\n")
}

/// Lines that are sung (not tags, not blank).
pub fn sung_lines(lyrics: &str) -> usize {
    lyrics.lines().map(str::trim).filter(|l| !l.is_empty() && !(l.starts_with('[') && l.ends_with(']'))).count()
}

/// The music model's plan has words to sing.
pub fn has_singing(planned: &str) -> bool {
    let t = planned.trim();
    !t.eq_ignore_ascii_case(INSTRUMENTAL) && sung_lines(t) > 0
}

/// Why lyrics couldn't be written here.
#[derive(Debug, Clone, PartialEq)]
pub enum NotWritten {
    /// No language model on this PC could be started.
    NoModel(String),
    /// One answered, but not with lyrics.
    Unusable(String),
}

/// What the person is told when a song meant to be sung came out without
/// words to sing.
pub fn no_singing_message(why: Option<&NotWritten>) -> String {
    match why {
        Some(NotWritten::Unusable(_)) => "Couldn't write lyrics: the chat model didn't write any it could sing. Add your own lyrics, or try again.".into(),
        _ => "Couldn't write lyrics: no AI chat model is available. Add your own lyrics, or install a chat model.".into(),
    }
}

/// The song turned out as wanted, or why not: vocals were asked for and
/// the music model's plan (`planned`, its lyrics) has nothing to sing.
pub fn check_sung(words: &Words, planned: &str, why: Option<&NotWritten>) -> Result<(), String> {
    if words.wants_vocals() && !has_singing(planned) {
        return Err(no_singing_message(why));
    }
    Ok(())
}

/// The language lyrics are in, as the music model's code, from their
/// script and their commonest words (English when it can't tell).
pub fn guess_language(text: &str) -> &'static str {
    // The sung lines only: "[Verse]" tags are English whatever the song.
    let text: String = text.lines().filter(|l| !l.trim_start().starts_with('[')).collect::<Vec<_>>().join("\n");
    let letters: Vec<char> = text.chars().filter(|c| c.is_alphabetic()).collect();
    let count = |lo: char, hi: char| letters.iter().filter(|c| (lo..=hi).contains(*c)).count();
    // Japanese mixes kana with Chinese characters: any real share of kana says it.
    if count('\u{3040}', '\u{30ff}') * 10 > letters.len().max(1) {
        return "ja";
    }
    let share = |lo: char, hi: char| count(lo, hi) * 4 > letters.len().max(1);
    for (lo, hi, code) in [('\u{ac00}', '\u{d7af}', "ko"), ('\u{4e00}', '\u{9fff}', "zh"), ('\u{0400}', '\u{04ff}', "ru"), ('\u{0600}', '\u{06ff}', "ar"), ('\u{0900}', '\u{097f}', "hi")] {
        if share(lo, hi) {
            return code;
        }
    }
    const WORDS: &[(&str, &[&str])] = &[
        ("en", &["the", "and", "you", "my", "i", "is", "in", "to", "of", "your", "we", "me", "it", "on", "with"]),
        ("es", &["el", "la", "de", "que", "y", "en", "los", "las", "mi", "tu", "es", "con", "por", "una", "del"]),
        ("fr", &["le", "la", "les", "de", "et", "je", "tu", "un", "une", "est", "dans", "pas", "mon", "qui", "des"]),
        ("de", &["der", "die", "das", "und", "ich", "du", "ist", "nicht", "ein", "mit", "mein", "dich", "wir", "zu", "auf"]),
        ("it", &["il", "la", "di", "che", "e", "un", "non", "per", "sono", "mi", "ti", "nel", "del", "una", "con"]),
        ("pt", &["o", "a", "de", "que", "e", "não", "um", "uma", "eu", "você", "meu", "com", "do", "da", "em"]),
    ];
    let words: Vec<String> = text.split(|c: char| !c.is_alphanumeric() && c != '\'').map(|w| w.to_lowercase()).filter(|w| !w.is_empty()).collect();
    WORDS
        .iter()
        .map(|(code, common)| (*code, words.iter().filter(|w| common.contains(&w.as_str())).count()))
        .max_by_key(|(code, n)| (*n, *code == "en"))
        .filter(|(_, n)| *n > 0)
        .map_or("en", |(code, _)| code)
}

/// The music model's code for a language given as a code or an English
/// name ("es", "Spanish"); None for "any language" or something unknown.
pub fn language_code(given: &str) -> Option<String> {
    let g = given.trim();
    if g.is_empty() || ["auto", "any", "unknown"].iter().any(|w| g.eq_ignore_ascii_case(w)) {
        return None;
    }
    let found = crate::translate::LANGUAGES.iter().find(|(c, n)| c.eq_ignore_ascii_case(g) || n.eq_ignore_ascii_case(g) || n.split(' ').next().is_some_and(|w| w.eq_ignore_ascii_case(g)));
    match found {
        Some((c, _)) => Some(c.split('-').next().unwrap_or(c).to_lowercase()),
        None => (g.len() == 2 && g.chars().all(|c| c.is_ascii_alphabetic())).then(|| g.to_lowercase()),
    }
}

/// Seconds of song each sung line needs to be heard: with 8 lines in 30
/// seconds the words came out mumbled (Whisper heard "Thank you."); 4
/// lines in 30, or 8 in 60, were heard line by line.
pub const SECS_PER_LINE: f32 = 7.5;

/// How many lines a song of this length can sing clearly.
pub fn lines_for(seconds: f32) -> u32 {
    (seconds / SECS_PER_LINE).round().clamp(4.0, 80.0) as u32
}

/// And how many words: 16 in 30 seconds were heard well, 21 about half,
/// 40 in 40 seconds hardly at all, and 8 lines of five or six words in 60
/// seconds line by line. The first seconds are mostly music, so it's a
/// second a word after the first twelve. (At 0.6 words a second, a minute's
/// song kept three lines of the writer's usual nine words.)
pub fn words_for(seconds: f32) -> usize {
    (seconds - 12.0).round().max(12.0) as usize
}

/// Seconds a rapped bar takes: a bar of four beats at 80 to 95 beats a
/// minute. Rap written as sung lines (six words, 7.5 s each) came out thin
/// and half-sung: Whisper heard 0.4 words a second.
pub const SECS_PER_BAR: f32 = 3.0;

/// How the words are delivered: sung lines, or rapped bars.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Flow {
    #[default]
    Sung,
    Rap,
}

impl Flow {
    fn secs_per_line(self) -> f32 {
        match self {
            Flow::Sung => SECS_PER_LINE,
            Flow::Rap => SECS_PER_BAR,
        }
    }

    /// Words the song can hold: rap runs two to three words a second.
    fn words(self, seconds: f32) -> usize {
        match self {
            Flow::Sung => words_for(seconds),
            Flow::Rap => (seconds * 3.0).round().max(24.0) as usize,
        }
    }

    /// The fewest lines a verse should get in a shape picked for the length.
    fn verse_at_least(self) -> f32 {
        match self {
            Flow::Sung => 3.5,
            Flow::Rap => 12.0,
        }
    }
}

/// Seconds a played section takes (no lines under it).
fn played_seconds(section: &str) -> f32 {
    if section.to_lowercase().contains("solo") {
        15.0
    } else {
        10.0
    }
}

/// A sung section's share of the lines, a verse's being 1. An intro or an
/// outro gets a couple of lines (they came out as long instrumental
/// openings when they had none); in rap the chorus is a short hook.
fn weight(section: &str, flow: Flow) -> f32 {
    let s = section.to_lowercase();
    if ["intro", "outro"].iter().any(|w| s.contains(w)) {
        0.25
    } else if ["pre", "bridge", "hook", "post", "tag", "coda"].iter().any(|w| s.contains(w)) {
        0.5
    } else if ["chorus", "refrain"].iter().any(|w| s.contains(w)) {
        if flow == Flow::Rap {
            0.5
        } else {
            1.0
        }
    } else if ["verse", "rap"].iter().any(|w| s.contains(w)) {
        1.0
    } else {
        0.75
    }
}

/// Song shapes by size, for a length given without one: the largest whose
/// sections get three or four lines and more.
const SHAPES: &[&[&str]] = &[
    &["Verse", "Chorus"],
    &["Verse", "Chorus", "Verse", "Chorus"],
    &["Intro", "Verse", "Chorus", "Verse", "Chorus", "Bridge", "Chorus", "Outro"],
    &["Intro", "Verse", "Pre-Chorus", "Chorus", "Verse", "Pre-Chorus", "Chorus", "Bridge", "Chorus", "Outro"],
    &["Intro", "Verse", "Pre-Chorus", "Chorus", "Verse", "Pre-Chorus", "Chorus", "Solo", "Bridge", "Chorus", "Chorus", "Outro"],
    &["Intro", "Verse", "Pre-Chorus", "Chorus", "Verse", "Pre-Chorus", "Chorus", "Instrumental", "Verse", "Pre-Chorus", "Chorus", "Bridge", "Chorus", "Chorus", "Outro"],
];

/// A section of the plan and the lines sung in it (0: played).
pub type Planned = (String, u32);

/// The song's sections and the lines under each: the ones chosen (else a
/// shape for the length), with the lines the length can sing (or rap)
/// clearly shared out by section, in even numbers so rhymes come in pairs.
pub fn plan(seconds: f32, chosen: &[String], flow: Flow) -> Vec<Planned> {
    let sung_lines = |list: &[String]| {
        let played: f32 = list.iter().filter(|s| played_tag(s)).map(|s| played_seconds(s)).sum();
        let units: f32 = list.iter().filter(|s| !played_tag(s)).map(|s| weight(s, flow)).sum();
        (((seconds - played) / flow.secs_per_line()).round().max(4.0), units)
    };
    let list: Vec<String> = if chosen.is_empty() {
        let shapes: Vec<Vec<String>> = SHAPES.iter().map(|s| s.iter().map(|x| x.to_string()).collect()).collect();
        shapes
            .iter()
            .rev()
            .find(|s| {
                let (lines, units) = sung_lines(s);
                lines >= units * flow.verse_at_least()
            })
            .unwrap_or(&shapes[0])
            .clone()
    } else {
        chosen.to_vec()
    };
    let (lines, units) = sung_lines(&list);
    let each = lines / units.max(0.5);
    let most = if flow == Flow::Rap { 24.0 } else { 16.0 };
    list.into_iter()
        .map(|s| {
            let n = if played_tag(&s) { 0 } else { ((each * weight(&s, flow) / 2.0).round() * 2.0).clamp(2.0, most) as u32 };
            (s, n)
        })
        .collect()
}

/// The plan as the writer reads it: "[Intro] 0 lines, [Verse] 4 lines, …".
/// (Told "[Intro] played (no lines)", Qwen3 4B wrote "played" under it.)
fn plan_words(plan: &[Planned]) -> String {
    let list = plan.iter().map(|(s, n)| format!("[{s}] {n} lines")).collect::<Vec<_>>().join(", ");
    if plan.iter().any(|(_, n)| *n == 0) {
        format!("{list}. A section with 0 lines is played by the band, not sung: write its tag alone, with nothing under it")
    } else {
        list
    }
}

/// Written lyrics without lines under a played section ("[Intro]" then
/// "played" or "(guitar)"): nothing is sung there.
fn without_played_lines(lyrics: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut played = false;
    for line in lyrics.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            played = played_tag(t);
        } else if played && !t.is_empty() {
            continue;
        }
        out.push(line);
    }
    out.join("\n")
}

/// How long a song needs to be for these lyrics to be heard (seconds).
pub fn seconds_for(lyrics: &str) -> f32 {
    sung_lines(lyrics) as f32 * SECS_PER_LINE
}

/// Seconds before the first words, in a length worked out from lyrics.
const LEAD_IN: f32 = 5.0;

/// A length worked out for Auto: rounded up to 5 seconds, 20 s to 10 minutes.
fn auto_round(secs: f32) -> f32 {
    ((secs / 5.0).ceil() * 5.0).clamp(20.0, 600.0)
}

/// The length these lyrics need, for Auto: each sung line its time (a
/// rapped bar less), or each word a second when the lines are long, plus
/// the played sections and a short lead-in.
pub fn length_of(lyrics: &str, flow: Flow) -> f32 {
    let (mut lines, mut words, mut played) = (0usize, 0usize, 0.0f32);
    for line in lyrics.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if line.starts_with('[') && line.ends_with(']') {
            if played_tag(line) {
                played += played_seconds(line);
            }
        } else {
            lines += 1;
            words += lead_words(line);
        }
    }
    let sung = match flow {
        Flow::Sung => (lines as f32 * SECS_PER_LINE).max(words as f32 + 12.0),
        Flow::Rap => (lines as f32 * SECS_PER_BAR).max(words as f32 / 3.0),
    };
    auto_round(sung + played + LEAD_IN)
}

/// The time a section takes when nothing else says: a verse four sung
/// lines (fourteen rapped bars), the others by their share (`weight`), two
/// lines at least; a played section its own seconds.
fn section_seconds(section: &str, flow: Flow) -> f32 {
    if played_tag(section) {
        return played_seconds(section);
    }
    let verse = if flow == Flow::Rap { 14.0 } else { 4.0 };
    (weight(section, flow) * verse).max(2.0) * flow.secs_per_line()
}

/// The length a chosen structure takes, for Auto.
pub fn length_of_sections(sections: &[String], flow: Flow) -> f32 {
    auto_round(sections.iter().map(|s| section_seconds(s, flow)).sum::<f32>() + LEAD_IN)
}

/// The length of a short piece the description asks for: a jingle, a
/// ringtone or an advert half a minute, a short song a minute. (Left to
/// the writer, "a little jingle for a bakery" came out 2:35 long.)
pub fn short_piece(text: &str) -> Option<f32> {
    let t = text.to_lowercase();
    let words: Vec<&str> = t.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let has = |w: &[&str]| words.iter().any(|x| w.contains(x));
    if has(&["jingle", "jingles", "ringtone", "ringtones", "advert", "commercial", "sting", "stinger", "snippet", "ad"]) {
        Some(30.0)
    } else if t.contains("short song") || t.contains("short rap") || t.contains("short tune") || t.contains("quick song") || (has(&["theme", "intro"]) && t.contains("show")) {
        Some(60.0)
    } else {
        None
    }
}

/// A length named in a description: "a 90-second jingle", "3 minutes",
/// "two and a half minutes", "a minute long". None when there's none (or
/// under 15 seconds, more likely about something else).
pub fn named_length(text: &str) -> Option<f32> {
    use regex::Regex;
    use std::sync::OnceLock;
    static NAMED: OnceLock<Regex> = OnceLock::new();
    let re = NAMED.get_or_init(|| {
        Regex::new(r"(?i)\b(\d+(?:\.\d+)?|a|an|one|two|three|four|five|six|seven|eight|nine|ten)(\s+and\s+a\s+half)?[\s-]*(minutes?|mins?|seconds?|secs?)\b").unwrap()
    });
    let c = re.captures(text)?;
    let n: f32 = match c[1].to_lowercase().as_str() {
        "a" | "an" | "one" => 1.0,
        "two" => 2.0,
        "three" => 3.0,
        "four" => 4.0,
        "five" => 5.0,
        "six" => 6.0,
        "seven" => 7.0,
        "eight" => 8.0,
        "nine" => 9.0,
        "ten" => 10.0,
        digits => digits.parse().ok()?,
    } + if c.get(2).is_some() { 0.5 } else { 0.0 };
    let secs = if c[3].to_lowercase().starts_with("min") { n * 60.0 } else { n };
    (secs >= 15.0).then(|| secs.min(600.0))
}

/// The writer's messages: the sections and their lines (`plan`), short
/// lines, and `guide`'s rules (rhyme, feeling, echoes in brackets for
/// harmony singers: `MusicSpec::lyrics_guide`). `retry`: what was wrong
/// with its last lines.
fn messages(prompt: &str, plan: &[Planned], language: Option<&str>, guide: &str, retry: Retry, flow: Flow) -> Vec<serde_json::Value> {
    let shorter = retry == Retry::Shorter;
    let lang = language
        .filter(|l| !l.is_empty() && *l != "auto")
        .map(|l| format!(" Write every line in {}.", crate::translate::language_name(l)))
        .unwrap_or_else(|| " Write them in the language the description is written in, unless it asks for another.".into());
    let total: u32 = plan.iter().map(|(_, n)| n).sum();
    let choruses = plan.iter().filter(|(s, _)| s.to_lowercase().contains("chorus") && !s.to_lowercase().contains("pre")).count();
    let same = if choruses > 1 { " Every [Chorus] has the same words." } else { "" };
    let short = match (flow, shorter) {
        (Flow::Sung, true) => " Keep every line very short: at most five words. Longer lines can't be heard.",
        (Flow::Sung, false) => " Keep every line short, four to six words, so every word can be heard.",
        (Flow::Rap, true) => " This is rap: every line is a bar of at most nine words. Longer bars can't be heard.",
        (Flow::Rap, false) => {
            " This is rap, not singing: write the verses as bars of eight to twelve words with a steady rhythm, rhymes inside the lines \
             as well as at their ends, and wordplay. Every [Chorus] is the hook: short, punchy lines that repeat."
        }
    };
    let what = if flow == Flow::Rap { "rap lyrics" } else { "song lyrics to be sung" };
    // Too few, and the music model filled the time repeating a chorus line.
    let fuller = if retry == Retry::Fuller { " Write every section in full, with exactly its number of lines: don't stop early or shorten a section." } else { "" };
    // No plan (Auto length): the writer picks the sections the song needs, and its length follows.
    let sections = if plan.is_empty() {
        // Lines take 7.5 s each here, so 26 lines are already over three minutes.
        let full = if flow == Flow::Rap {
            "a full rap has a two-bar intro, two verses of twelve to sixteen bars, a hook that repeats, and a two-bar outro"
        } else {
            "a full song has a two-line intro, two verses of four lines, a chorus of four lines that repeats, often a bridge of two lines, \
             and a two-line outro: about 20 to 26 lines in all, unless the user asks for a long one"
        };
        format!(
            "Choose the sections the song needs, in order, with the tags [Intro], [Verse], [Pre-Chorus], [Chorus], [Bridge] and [Outro], \
             to suit what the user asks for: a short piece (a jingle, a lullaby, a short verse) stays short, and {full}. Every [Chorus] has the same words."
        )
    } else {
        format!("Write these sections in this order, with this many lines under each: {}. That's {total} lines in all.{fuller}", plan_words(plan))
    };
    vec![
        json!({ "role": "system", "content": format!(
            "You write {what}. Reply with the lyrics only: section tags in square brackets on their own lines, and the lines under them. \
             No title, no notes, no quotes, no Markdown. {sections}{short}{same}{lang}{guide}"
        ) }),
        json!({ "role": "user", "content": format!("A song: {}", prompt.trim()) }),
    ]
}

/// Lyrics from one model's answer, when they're lyrics.
fn usable(raw: &str) -> Option<String> {
    let text = without_played_lines(&tidy(crate::chat::strip_thinking(raw)));
    (sung_lines(&text) >= 3).then_some(text)
}

/// The words the lead sings on a line: echoes in round brackets are the
/// harmony singers', so they don't crowd the lead.
fn lead_words(line: &str) -> usize {
    let mut depth = 0u32;
    let lead: String = line
        .chars()
        .filter(|c| match c {
            '(' => {
                depth += 1;
                false
            }
            ')' => {
                depth = depth.saturating_sub(1);
                false
            }
            _ => depth == 0,
        })
        .collect();
    lead.split_whitespace().count()
}

/// Written lyrics cut to the lines the song can sing clearly (the writer,
/// asked for 4 lines for 30 seconds, wrote 8 and more, and they came out
/// mumbled): whole lines kept in order, with their section tags; tags left
/// with nothing under them are dropped.
pub fn fit_lines(lyrics: &str, max: u32, max_words: usize) -> String {
    let mut out: Vec<&str> = Vec::new();
    let (mut sung, mut words) = (0, 0);
    for line in lyrics.lines() {
        let t = line.trim();
        let tag = t.starts_with('[') && t.ends_with(']');
        if !tag && !t.is_empty() {
            let n = lead_words(t);
            // At least three lines, whatever their length (two were sung over and over).
            if sung == max || (sung >= 3 && words + n > max_words) {
                break;
            }
            sung += 1;
            words += n;
        }
        out.push(line);
    }
    // No blank line at the end, nor a sung section's tag with nothing under
    // it; a played one ([Outro], [Solo]) stays, since it's meant empty.
    while out.last().is_some_and(|l| {
        let t = l.trim();
        t.is_empty() || (t.starts_with('[') && t.ends_with(']') && !played_tag(t))
    }) {
        out.pop();
    }
    out.join("\n")
}

/// The best lyrics written so far, and whether the last were cut for
/// lines too long to be heard.
#[derive(Default)]
struct Draft {
    text: String,
    too_long: bool,
    /// The last had far fewer lines than planned: the music model filled
    /// the time by repeating a chorus line over and over.
    too_few: bool,
}

impl Draft {
    /// One answer, fitted to the song: Some when it's whole (three lines
    /// in four of the plan written, and nine in ten of those kept), else
    /// kept if it's the best so far, to be asked again (shorter lines, or
    /// every line).
    fn take(&mut self, raw: &str, planned: u32, max_lines: u32, max_words: usize) -> Option<String> {
        let written = usable(raw)?;
        let fitted = fit_lines(&written, max_lines, max_words);
        let kept = sung_lines(&fitted);
        let wanted = sung_lines(&written).min(planned as usize);
        let (long, few) = (kept * 10 < wanted * 9, sung_lines(&written) * 4 < planned as usize * 3);
        if !long && !few {
            return Some(fitted);
        }
        crate::applog::line("media", &format!("Lyrics {}: {kept} of {planned} lines kept; asking again", if long { "too long to be heard" } else { "too short for the song" }));
        (self.too_long, self.too_few) = (long, few && !long);
        if kept > sung_lines(&self.text) {
            self.text = fitted;
        }
        None
    }

    /// What the next ask says about the last.
    fn retry(&self) -> Retry {
        match (self.too_long, self.too_few) {
            (true, _) => Retry::Shorter,
            (_, true) => Retry::Fuller,
            _ => Retry::First,
        }
    }
}

/// What the writer is told about its last try.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Retry {
    First,
    /// Its lines were too long to be heard.
    Shorter,
    /// It wrote far fewer lines than the sections need.
    Fuller,
}

/// A section tag for a part that's played, not sung.
fn played_tag(tag: &str) -> bool {
    let t = tag.to_lowercase();
    ["instrumental", "solo", "interlude", "break"].iter().any(|w| t.contains(w))
}

/// Writes lyrics for a song with the chat model (the one loaded, else the
/// default), else a language model a picture model brought (FLUX.2 klein's
/// Qwen3, for one). Run before the music engine starts, so they don't share
/// the graphics card.
pub async fn write(state: &Arc<AppState>, prompt: &str, seconds: Option<f32>, language: Option<&str>, sections: &[String], guide: &str, flow: Flow) -> Result<String, NotWritten> {
    // No length (Auto) and no sections: the writer picks them, and the
    // song's length follows the lyrics (`length_of`), so none are cut.
    let open = seconds.is_none() && sections.is_empty();
    let seconds = seconds.unwrap_or_else(|| length_of_sections(sections, flow));
    let plan = if open { Vec::new() } else { plan(seconds, sections, flow) };
    let planned: u32 = plan.iter().map(|(_, n)| n).sum();
    let max_lines = if open {
        80
    } else if flow == Flow::Rap {
        planned
    } else {
        planned.max(lines_for(seconds))
    };
    let max_words = if open { usize::MAX } else { flow.words(seconds) };
    // Room for every line and tag (about 14 tokens a line, 24 a bar), and some over.
    let per_line = if flow == Flow::Rap { 24 } else { 14 };
    let room = if open { per_line * 48 + 300 } else { planned * per_line + plan.len() as u32 * 6 + 250 };
    let mut why = None;
    let mut best = Draft::default();
    match crate::background_endpoint(state).await {
        Ok((ep, spec, _hold)) => {
            for temperature in [0.8, 0.6] {
                let msgs = messages(prompt, &plan, language, guide, best.retry(), flow);
                match crate::chat::complete(&ep, msgs, json!({ "temperature": temperature }), room).await {
                    Ok(raw) => match best.take(&raw, planned, max_lines, max_words) {
                        Some(l) => return Ok(l),
                        None if best.text.is_empty() => why = Some(NotWritten::Unusable(format!("{} answered without lyrics", spec.name))),
                        None => {}
                    },
                    Err(e) => why = Some(NotWritten::Unusable(e)),
                }
            }
            if !best.text.is_empty() {
                return Ok(best.text);
            }
        }
        Err(e) => crate::applog::line("media", &format!("No chat model to write lyrics ({e})")),
    }
    // A language model a picture model brought, on an engine of its own.
    let helpers: Vec<(String, std::path::PathBuf)> = super::models(state)
        .iter()
        .filter_map(|m| super::installed_file(state, m, "llm").map(|p| (format!("{}'s language model", m.name), p)))
        .collect();
    for (name, path) in helpers {
        match crate::vision::Helper::start(state, &name, &path, None).await {
            Ok(h) => {
                let mut said = None;
                for temperature in [0.8, 0.6] {
                    let msgs = messages(prompt, &plan, language, guide, best.retry(), flow);
                    if let Ok(raw) = h.ask(msgs, room, temperature).await {
                        said = best.take(&raw, planned, max_lines, max_words);
                        if said.is_some() {
                            break;
                        }
                    }
                }
                h.done(state).await;
                match said.or_else(|| (!best.text.is_empty()).then(|| best.text.clone())) {
                    Some(l) => {
                        crate::applog::line("media", &format!("Lyrics written by {name}"));
                        return Ok(l);
                    }
                    None => why = Some(NotWritten::Unusable(format!("{name} answered without lyrics"))),
                }
            }
            Err(e) => {
                crate::applog::line("media", &format!("Couldn't start {name} to write lyrics ({e})"));
                why = why.or(Some(NotWritten::NoModel(e)));
            }
        }
    }
    Err(why.unwrap_or_else(|| NotWritten::NoModel("no chat model is installed".into())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn written_lyrics_are_cut_to_what_fits() {
        let l = "[Verse]\nOne\nTwo\nThree\n\n[Chorus]\nFour\nFive\n\n[Bridge]\nSix";
        assert_eq!(fit_lines(l, 4, 99), "[Verse]\nOne\nTwo\nThree\n\n[Chorus]\nFour");
        assert_eq!(fit_lines(l, 3, 99), "[Verse]\nOne\nTwo\nThree");
        assert_eq!(fit_lines(l, 10, 99), l);
        // Long lines: cut by words too, keeping three lines at least.
        let long = "[Verse]\none two three four five six seven eight\nnine ten eleven twelve thirteen fourteen fifteen sixteen\nmore words here\nand a fourth";
        assert_eq!(fit_lines(long, 5, 12).lines().count(), 4);
    }

    #[test]
    fn echoes_in_brackets_are_the_harmony_singers() {
        assert_eq!(lead_words("We'll find our way (find our way)"), 4);
        assert_eq!(lead_words("(ooh) Hold on (hold on, hold on)"), 2);
        let l = "[Chorus]\nWe rise (we rise, we rise)\nWe shine (shine on)\nWe fly (we fly)\nTogether (together)";
        assert_eq!(fit_lines(l, 4, 7), l, "the echoes don't push lines out (7 lead words, 15 in all)");
    }

    #[test]
    fn played_sections_keep_their_empty_tags() {
        let l = "[Instrumental Intro]\n\n[Verse]\nOne\nTwo\nThree\n\n[Solo]";
        assert_eq!(fit_lines(l, 8, 99), l, "an instrumental intro and a solo have no words, on purpose");
        assert_eq!(fit_lines("[Verse]\nOne\nTwo\nThree\n\n[Chorus]", 8, 99), "[Verse]\nOne\nTwo\nThree", "an empty chorus goes");
        assert_eq!(fit_lines("[Verse]\nOne\nTwo\nThree\n\n[Outro]", 8, 99), "[Verse]\nOne\nTwo\nThree", "an outro is sung now: empty, it goes");
    }

    #[test]
    fn the_song_is_planned_for_its_length() {
        let lines = |p: &[Planned]| p.iter().map(|(_, n)| n).sum::<u32>();
        let shape = |p: &[Planned]| p.iter().map(|(s, n)| format!("{s} {n}")).collect::<Vec<_>>().join(", ");
        let sung = Flow::Sung;
        // A minute: a verse and a chorus of four lines (it was three lines in all).
        assert_eq!(shape(&plan(60.0, &[], sung)), "Verse 4, Chorus 4");
        assert_eq!(shape(&plan(120.0, &[], sung)), "Verse 4, Chorus 4, Verse 4, Chorus 4");
        // An intro and an outro are sung too (empty, the intro was a long instrumental opening).
        assert_eq!(shape(&plan(180.0, &[], sung)), "Intro 2, Verse 4, Chorus 4, Verse 4, Chorus 4, Bridge 2, Chorus 4, Outro 2");
        let long = plan(600.0, &[], sung);
        assert!(long.len() == 15 && lines(&long) >= 70, "{}", shape(&long));
        // The sections chosen, in their order, with the lines the length holds.
        let chosen: Vec<String> = ["Intro", "Verse", "Chorus", "Verse", "Chorus", "Bridge", "Chorus", "Chorus"].iter().map(|s| s.to_string()).collect();
        assert_eq!(shape(&plan(200.0, &chosen, sung)), "Intro 2, Verse 4, Chorus 4, Verse 4, Chorus 4, Bridge 2, Chorus 4, Chorus 4");
        // Too short for them: two lines each still.
        assert!(plan(30.0, &chosen, sung).iter().all(|(_, n)| *n >= 2));
        // An instrumental intro is played, with nothing under it.
        assert!(plan_words(&plan(60.0, &["Instrumental Intro".into(), "Verse".into()], sung)).starts_with("[Instrumental Intro] 0 lines, [Verse] 8 lines. A section with 0 lines is played"));
        assert_eq!(plan_words(&plan(60.0, &[], sung)), "[Verse] 4 lines, [Chorus] 4 lines");
        // What Qwen3 4B wrote under the played sections is taken out.
        let wrote = "[Instrumental Intro]\nplayed\n\n[Verse]\nOne\nTwo\nThree\n\n[Solo]\n(guitar wails)";
        assert_eq!(usable(wrote).as_deref(), Some("[Instrumental Intro]\n\n[Verse]\nOne\nTwo\nThree\n\n[Solo]"));
        // Rap: bars of about three seconds, long verses and a short hook.
        let rap = Flow::Rap;
        assert_eq!(shape(&plan(60.0, &[], rap)), "Verse 14, Chorus 6");
        assert_eq!(shape(&plan(180.0, &[], rap)), "Intro 4, Verse 14, Chorus 6, Verse 14, Chorus 6, Bridge 6, Chorus 6, Outro 4");
        assert_eq!(rap.words(60.0), 180);
        assert_eq!(words_for(30.0), 18);
        assert_eq!(words_for(60.0), 48);
    }

    /// Auto: the length a description names, and the length lyrics need.
    #[test]
    fn auto_lengths_are_worked_out() {
        assert_eq!(named_length("a 90-second jingle"), Some(90.0));
        assert_eq!(named_length("make it 3 minutes long"), Some(180.0));
        assert_eq!(named_length("two and a half minutes of chill"), Some(150.0));
        assert_eq!(named_length("about a minute"), Some(60.0));
        assert_eq!(named_length("a 2.5 min song"), Some(150.0));
        assert_eq!(named_length("10 minutes"), Some(600.0));
        assert_eq!(named_length("a song about the 5 seconds before midnight"), None, "too short to be a length");
        assert_eq!(named_length("a love song"), None);
        assert_eq!(short_piece("a cheerful little jingle for a neighborhood bakery"), Some(30.0));
        assert_eq!(short_piece("a short song for my mom's birthday"), Some(60.0));
        assert_eq!(short_piece("a theme song for my cooking show"), Some(60.0));
        assert_eq!(short_piece("a heartfelt pop song about leaving home"), None);
        assert_eq!(short_piece("a song about a road trip"), None, "\"ad\" only as a word");
        // Played sections add their time; rap bars are quicker than sung lines.
        assert_eq!(length_of("[Instrumental Intro]\n\n[Verse]\nOne two three\nFour five six\nSeven eight\nNine ten", Flow::Sung), 45.0);
        let bars = format!("[Verse]\n{}", vec!["eight words in a bar of a rap verse"; 16].join("\n"));
        assert_eq!(length_of(&bars, Flow::Rap), 55.0, "16 bars of 3 s, and a lead-in");
        assert_eq!(length_of(&bars, Flow::Sung), 165.0, "sung, the same lines take longer (a second a word)");
        assert_eq!(length_of_sections(&["Verse".into(), "Chorus".into()], Flow::Rap), 70.0, "14 bars and a 7-bar hook");
        // No plan: the writer picks the sections.
        let open = messages("a jingle for a bakery", &[], None, "", Retry::First, Flow::Sung);
        let sys = open[0]["content"].as_str().unwrap();
        assert!(sys.contains("Choose the sections the song needs") && sys.contains("a short piece") && !sys.contains("That's"), "{sys}");
    }

    #[test]
    fn lines_too_long_are_asked_again() {
        let mut best = Draft::default();
        // Eight lines of ten words for a minute: only four fit, so it's asked again.
        let wordy = format!("[Verse]\n{}", vec!["one two three four five six seven eight nine ten"; 8].join("\n"));
        assert_eq!(best.take(&wordy, 8, 8, words_for(60.0)), None);
        assert!(best.too_long && sung_lines(&best.text) == 4, "the best so far is kept");
        // Short lines: all eight kept.
        let short = format!("[Verse]\n{}\n\n[Chorus]\n{}", vec!["walking down the road"; 4].join("\n"), vec!["we sing it loud"; 4].join("\n"));
        assert_eq!(best.take(&short, 8, 8, words_for(60.0)).map(|l| sung_lines(&l)), Some(8));
        // A few lines short of the plan are fine as they are.
        assert!(best.take("[Verse]\na b c\nd e f\ng h i\n\n[Chorus]\nj k l\nm n o\np q r", 8, 8, 48).is_some());
        // Far fewer (three of eight) are asked for in full: the music model
        // filled a 2½-minute song repeating one chorus line.
        let mut few = Draft::default();
        assert_eq!(few.take("[Verse]\nshort line here\nand another\nand one more", 8, 8, 48), None);
        assert_eq!(few.retry(), Retry::Fuller);
        assert!(messages("x", &plan(60.0, &[], Flow::Sung), None, "", Retry::Fuller, Flow::Sung)[0]["content"].as_str().unwrap().contains("Write every section in full"));
    }

    #[test]
    fn lyrics_fit_the_length() {
        assert_eq!(lines_for(30.0), 4);
        assert_eq!(lines_for(60.0), 8);
        assert_eq!(lines_for(600.0), 80);
        let l = "[Verse]\nOne\nTwo\n\n[Chorus]\nThree\nFour\nFive\nSix\nSeven\nEight";
        assert_eq!(sung_lines(l), 8);
        assert_eq!(seconds_for(l), 60.0);
    }

    #[test]
    fn the_language_of_lyrics_is_guessed() {
        assert_eq!(guess_language("[Verse]\nSunrise paints the ridge in gold\nWe're rolling down the mountain road"), "en");
        assert_eq!(guess_language("[Verse]\nLa luna canta en el mar azul\nLas olas bailan con el viento"), "es");
        assert_eq!(guess_language("[Verse]\nJe marche dans la nuit et le vent"), "fr");
        assert_eq!(guess_language("[Verse]\nIch gehe durch die Nacht und du bist nicht hier"), "de");
        assert_eq!(guess_language("[Verse]\n夜空に光る星"), "ja");
        assert_eq!(guess_language("[Verse]\n밤하늘의 별"), "ko");
        assert_eq!(guess_language("[Verse]\n夜空中最亮的星"), "zh");
        assert_eq!(guess_language("[Verse]\nhmm hmm"), "en");
    }

    #[test]
    fn what_is_sung() {
        assert_eq!(words(true, false, Some("[Verse]\nla la la")), Words::Instrumental, "a sound effect never sings");
        assert_eq!(words(false, true, Some("[Verse]\nla la la")), Words::Instrumental);
        assert_eq!(words(false, true, None).for_engine(), "[Instrumental]");
        assert_eq!(words(false, false, None), Words::MusicModelWrites);
        assert_eq!(words(false, false, Some("   \n ")), Words::MusicModelWrites, "blank lyrics: the music model writes them");
        assert_eq!(words(false, false, Some("[Chorus]")), Words::MusicModelWrites, "only a tag: nothing to sing");
        assert_eq!(words(false, false, None).for_engine(), "");
        let mine = words(false, false, Some("Wheels on the highway\nSun on my face"));
        assert_eq!(mine, Words::Sung("[Verse]\nWheels on the highway\nSun on my face".into()), "a tag is added when there's none");
        assert!(mine.wants_vocals() && Words::MusicModelWrites.wants_vocals() && !Words::Instrumental.wants_vocals());
    }

    #[test]
    fn written_lyrics_take_the_engines_form() {
        let raw = "Sure! Here are the lyrics for your road trip song:\n\n**Title: Mountain Road**\n\n**Verse 1:**\n\"Wheels on the highway, sun on my face\"\nMaps on the dashboard, nowhere to be\n\n\n(Chorus)\nMountains are calling\nWe're never coming home\n\nBridge:\n*Stars over the canyon*\n```";
        assert_eq!(
            tidy(raw),
            "[Verse 1]\nWheels on the highway, sun on my face\nMaps on the dashboard, nowhere to be\n\n[Chorus]\nMountains are calling\nWe're never coming home\n\n[Bridge]\nStars over the canyon"
        );
        // Already in form: unchanged.
        let ok = "[Verse]\nOne line\nTwo lines\n\n[Chorus - softer]\nThree";
        assert_eq!(tidy(ok), ok);
        // A first line that only looks like an opener stays.
        assert_eq!(tidy("Here I am, standing tall\nNo one by my side"), "[Verse]\nHere I am, standing tall\nNo one by my side");
        assert_eq!(tidy("## Pre-Chorus\nhold on\nChorus x2\nlet go"), "[Pre-Chorus]\nhold on\n\n[Chorus X2]\nlet go");
        assert_eq!(sung_lines(&tidy(raw)), 5);
    }

    #[test]
    fn a_song_with_nothing_to_sing_says_why() {
        let wanted = Words::MusicModelWrites;
        assert!(check_sung(&wanted, "[Verse]\nla la la\nsing along", None).is_ok());
        let none = check_sung(&wanted, "", Some(&NotWritten::NoModel("no chat model".into()))).unwrap_err();
        assert_eq!(none, "Couldn't write lyrics: no AI chat model is available. Add your own lyrics, or install a chat model.");
        assert!(check_sung(&wanted, "[Instrumental]", None).is_err(), "the music model planned an instrumental");
        assert!(check_sung(&wanted, "[Intro]\n[Outro]", None).is_err(), "only tags");
        assert!(check_sung(&wanted, " ", Some(&NotWritten::Unusable("x".into()))).unwrap_err().contains("didn't write any"));
        assert!(check_sung(&Words::Instrumental, "", None).is_ok(), "an instrumental is fine without words");
        assert!(check_sung(&Words::Sung("[Verse]\na\nb\nc".into()), "[Verse]\na\nb\nc", None).is_ok());
    }

    #[test]
    fn the_writer_is_asked_for_the_language_by_name() {
        let m = messages("upbeat folk", &plan(30.0, &[], Flow::Sung), Some("es"), "", Retry::First, Flow::Sung);
        let sys = m[0]["content"].as_str().unwrap();
        assert!(sys.contains("Write every line in Spanish.") && sys.contains("[Verse] 2 lines, [Chorus] 2 lines. That's 4 lines in all."), "{sys}");
        let auto = messages("upbeat folk", &plan(120.0, &[], Flow::Sung), Some("auto"), "", Retry::First, Flow::Sung);
        let sys = auto[0]["content"].as_str().unwrap();
        assert!(sys.contains("language the description is written in"));
        assert!(sys.contains("That's 16 lines in all.") && sys.contains("Every [Chorus] has the same words.") && sys.contains("four to six words"), "{sys}");
        assert!(messages("folk", &plan(60.0, &[], Flow::Sung), None, "", Retry::Shorter, Flow::Sung)[0]["content"].as_str().unwrap().contains("at most five words"));
        // Rap is asked for as bars, not short sung lines.
        let rap = messages("a night job", &plan(60.0, &[], Flow::Rap), None, "", Retry::First, Flow::Rap);
        let sys = rap[0]["content"].as_str().unwrap();
        assert!(sys.starts_with("You write rap lyrics.") && sys.contains("bars of eight to twelve words") && sys.contains("[Verse] 14 lines") && !sys.contains("four to six words"), "{sys}");
        assert_eq!(language_code("Spanish").as_deref(), Some("es"));
        assert_eq!(language_code("ES").as_deref(), Some("es"));
        assert_eq!(language_code("chinese").as_deref(), Some("zh"));
        assert_eq!(language_code("zh-TW").as_deref(), Some("zh"));
        assert_eq!(language_code("auto"), None);
        assert_eq!(language_code("Klingon"), None);
        assert_eq!(usable("<think>hmm</think>[Verse]\na\nb"), None, "too short");
        assert_eq!(usable("<think>hmm</think>[Verse]\na\nb\nc").as_deref(), Some("[Verse]\na\nb\nc"));
    }
}
