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
/// 40 in 40 seconds hardly at all.
pub fn words_for(seconds: f32) -> usize {
    (seconds * 0.6).round().max(12.0) as usize
}

/// How long a song needs to be for these lyrics to be heard (seconds).
pub fn seconds_for(lyrics: &str) -> f32 {
    sung_lines(lyrics) as f32 * SECS_PER_LINE
}

fn messages(prompt: &str, seconds: f32, language: Option<&str>) -> Vec<serde_json::Value> {
    let lang = language
        .filter(|l| !l.is_empty() && *l != "auto")
        .map(|l| format!(" Write every line in {}.", crate::translate::language_name(l)))
        .unwrap_or_else(|| " Write them in the language the description is written in, unless it asks for another.".into());
    vec![
        json!({ "role": "system", "content": format!(
            "You write song lyrics to be sung. Reply with the lyrics only: about {} short lines, with section tags in square brackets on their own lines, \
             such as [Verse], [Chorus] and [Bridge]. No title, no notes, no quotes, no Markdown.{lang}",
            lines_for(seconds)
        ) }),
        json!({ "role": "user", "content": format!("A song: {}", prompt.trim()) }),
    ]
}

/// Lyrics from one model's answer, when they're lyrics.
fn usable(raw: &str) -> Option<String> {
    let text = tidy(crate::chat::strip_thinking(raw));
    (sung_lines(&text) >= 3).then_some(text)
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
            let n = t.split_whitespace().count();
            // At least two lines, whatever their length.
            if sung == max || (sung >= 2 && words + n > max_words) {
                break;
            }
            sung += 1;
            words += n;
        }
        out.push(line);
    }
    // No tag or blank line at the end.
    while out.last().is_some_and(|l| {
        let t = l.trim();
        t.is_empty() || (t.starts_with('[') && t.ends_with(']'))
    }) {
        out.pop();
    }
    out.join("\n")
}

/// Writes lyrics for a song with the chat model (the one loaded, else the
/// default), else a language model a picture model brought (FLUX.2 klein's
/// Qwen3, for one). Run before the music engine starts, so they don't share
/// the graphics card.
pub async fn write(state: &Arc<AppState>, prompt: &str, seconds: f32, language: Option<&str>) -> Result<String, NotWritten> {
    let msgs = messages(prompt, seconds, language);
    let mut why = None;
    match crate::background_endpoint(state).await {
        Ok((ep, spec, _hold)) => {
            for temperature in [0.8, 0.6] {
                // Room for every line (about 14 tokens each), and some over.
                let room = lines_for(seconds) * 14 + 250;
                match crate::chat::complete(&ep, msgs.clone(), json!({ "temperature": temperature }), room).await {
                    Ok(raw) => match usable(&raw).map(|l| fit_lines(&l, lines_for(seconds), words_for(seconds))) {
                        Some(l) => return Ok(l),
                        None => why = Some(NotWritten::Unusable(format!("{} answered without lyrics", spec.name))),
                    },
                    Err(e) => why = Some(NotWritten::Unusable(e)),
                }
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
                let said = h.ask(msgs.clone(), lines_for(seconds) * 14 + 250, 0.8).await;
                h.done(state).await;
                match said.as_deref().ok().and_then(usable).map(|l| fit_lines(&l, lines_for(seconds), words_for(seconds))) {
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
        // Long lines: cut by words too, keeping two lines at least.
        let long = "[Verse]\none two three four five six seven eight\nnine ten eleven twelve thirteen fourteen fifteen sixteen\nmore words here";
        assert_eq!(fit_lines(long, 5, 12).lines().count(), 3);
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
        let m = messages("upbeat folk", 30.0, Some("es"));
        let sys = m[0]["content"].as_str().unwrap();
        assert!(sys.contains("Write every line in Spanish.") && sys.contains("about 4 short lines"), "{sys}");
        let auto = messages("upbeat folk", 120.0, Some("auto"));
        assert!(auto[0]["content"].as_str().unwrap().contains("language the description is written in"));
        assert!(auto[0]["content"].as_str().unwrap().contains("about 16 short lines"));
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
