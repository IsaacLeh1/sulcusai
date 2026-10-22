// SPDX-License-Identifier: AGPL-3.0-only
//! Songs in an artist's style: the Studio's "Sound like", or "sing as Taylor
//! Swift, a song about bears" in the description. The music model doesn't
//! know names, so the chat model describes the artist's sound in plain words
//! for it (genre, the kind of voice, instruments, the records' sound) and how
//! they write, for the lyrics writer, which writes words of its own that way.
//! The voice is one of the same kind, never the artist's own.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use regex::Regex;
use serde::Serialize;
use serde_json::{json, Value};

use super::{MusicSpec, Request};
use crate::AppState;

/// An artist's style in plain words.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Style {
    pub genre: String,
    /// "female", "male", "duet", "choir", "rap", or "" when it's unclear.
    pub voice: String,
    /// How the lead voice sounds and is sung.
    pub vocal: String,
    pub instruments: Vec<String>,
    /// Their records' sound and era.
    pub production: String,
    pub mood: String,
    pub bpm: Option<u32>,
    /// How they write lyrics.
    pub writing: String,
}

impl Style {
    /// Their sound for the music model's description: the voice and the records.
    pub fn sound(&self) -> String {
        [self.vocal.as_str(), self.production.as_str()].iter().map(|s| s.trim().trim_end_matches('.')).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ")
    }

    /// Onto a song: the name for the lyrics writer, their sound and way of
    /// writing, and their genre, voice, mood, instruments and tempo where
    /// the person chose none.
    pub fn apply(&self, name: &str, spec: &mut MusicSpec) {
        let empty = |o: &Option<String>| o.as_deref().map_or(true, |s| s.trim().is_empty());
        let given = |s: &str| Some(s.to_string()).filter(|s| !s.trim().is_empty());
        spec.sound_like = Some(name.to_string());
        if empty(&spec.artist_sound) {
            spec.artist_sound = given(&self.sound());
        }
        if empty(&spec.artist_writing) {
            spec.artist_writing = given(&self.writing);
        }
        if empty(&spec.genre) {
            spec.genre = given(&self.genre);
        }
        if empty(&spec.voice) {
            spec.voice = given(&self.voice);
        }
        if empty(&spec.mood) {
            spec.mood = given(&self.mood);
        }
        if spec.instruments.is_empty() {
            spec.instruments = self.instruments.clone();
        }
        if spec.bpm.is_none() {
            spec.bpm = self.bpm;
        }
    }
}

/// Words that start a description rather than a name ("sing like a bird").
const NOT_NAMES: &[&str] = &["a", "an", "my", "your", "our", "someone", "somebody", "if", "it", "me", "us", "them", "him", "her", "this", "that", "they", "you"];

/// An artist named in a song's description, and the description without
/// them: "Sing as Taylor Swift a song about bears" gives ("Taylor Swift",
/// "a song about bears").
pub fn named_in(prompt: &str) -> Option<(String, String)> {
    static ASKED: OnceLock<Regex> = OnceLock::new();
    static ENDS: OnceLock<Regex> = OnceLock::new();
    let asked = ASKED.get_or_init(|| {
        Regex::new(r"(?i)\b(?:(?:sing|sings|sung|sang|singing|performed|perform|write it|written|sound|sounds|sounding)\s+(?:as|like|by)|in the style of|à la)\s+").unwrap()
    });
    let ends = ENDS.get_or_init(|| {
        Regex::new(r"(?i)[,.;:?\n]|\s(?:a|an|about|with|that|who|for|singing|in|on|but|would|does|sings|to|and make|and write)\s").unwrap()
    });
    let m = asked.find(prompt)?;
    let after = &prompt[m.end()..];
    let cut = ends.find(after).map_or(after.len(), |e| e.start());
    let name = after[..cut].trim().trim_end_matches(['\'', '"']).trim_start_matches(['\'', '"']).trim();
    let words: Vec<&str> = name.split_whitespace().collect();
    if words.is_empty() || words.len() > 5 || name.chars().count() > 40 || NOT_NAMES.contains(&words[0].to_lowercase().as_str()) {
        return None;
    }
    let rest = format!("{} {}", &prompt[..m.start()], &after[cut..]);
    let rest = rest.split_whitespace().collect::<Vec<_>>().join(" ");
    let rest = rest.trim_matches(|c: char| c == ',' || c == '-' || c == ' ').trim_start_matches(['.', ';', ':']).trim();
    Some((name.to_string(), if rest.is_empty() { "a song".into() } else { rest.to_string() }))
}

/// What the chat model is asked. (Told the generator "doesn't know anyone's
/// name" and to answer {"known": false} for an unknown artist, Qwen3 4B
/// answered that for Taylor Swift.)
fn messages(name: &str) -> Vec<Value> {
    vec![
        json!({ "role": "system", "content":
            "You describe a singer's or band's music for a music generator that only understands plain descriptions. \
             Reply with only a JSON object with these keys: \
             \"genre\" (their main genre), \"voice\" (female, male, duet, choir or rap), \
             \"vocal\" (how the lead voice sounds and is sung, in a few words: tone, range, vibrato, grit, accent), \
             \"instruments\" (a list of the instruments their songs use most), \
             \"production\" (the sound of their records and their era, in a few words), \"mood\" (one word), \"bpm\" (a typical tempo, a number), \
             \"writing\" (how they write lyrics: themes, imagery, point of view, rhyme and hooks, in one or two sentences). \
             Use describing words only, with no names of people, bands, songs or albums. \
             Only if you have never heard of them, set \"genre\" to \"unknown\"." }),
        json!({ "role": "user", "content": format!("Describe the music of: {name}") }),
    ]
}

/// The chat model's answer about an artist, as it said it.
pub(crate) async fn ask(state: &Arc<AppState>, name: &str) -> Result<(String, String), String> {
    let (ep, spec, _hold) = crate::background_endpoint(state).await.map_err(|e| format!("There's no chat model to read the artist's style ({e})."))?;
    let raw = crate::chat::complete(&ep, messages(name), json!({ "temperature": 0.2 }), 500).await?;
    Ok((spec.name, raw))
}

/// The style from the chat model's answer: Ok(None) when it doesn't know them.
fn parse(raw: &str, name: &str) -> Result<Option<Style>, String> {
    let text = crate::chat::strip_thinking(raw);
    let json = text.find('{').and_then(|a| text.rfind('}').filter(|b| *b > a).map(|b| &text[a..=b])).ok_or("it didn't describe them")?;
    let v: Value = serde_json::from_str(json).map_err(|e| format!("its description couldn't be read ({e})"))?;
    let unknown = v.get("genre").and_then(Value::as_str).is_some_and(|g| g.trim().eq_ignore_ascii_case("unknown"));
    if v.get("known").and_then(Value::as_bool) == Some(false) || unknown {
        return Ok(None);
    }
    // Their name stays out, whatever the model wrote.
    let named = Regex::new(&format!(r"(?i)\b{}('s)?\b", regex::escape(name.trim()))).ok();
    let clean = |s: &str, most: usize| -> String {
        let s = named.as_ref().map_or(s.to_string(), |r| r.replace_all(s, "").to_string());
        let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
        s.trim_matches(|c: char| c == ',' || c == ' ').chars().take(most).collect()
    };
    let text_of = |k: &str, most: usize| v.get(k).and_then(Value::as_str).map(|s| clean(s, most)).unwrap_or_default();
    let voice = text_of("voice", 40).to_lowercase();
    let has = |w: &[&str]| voice.split(|c: char| !c.is_alphanumeric()).any(|x| w.contains(&x));
    let voice = if has(&["duet"]) {
        "duet"
    } else if has(&["choir"]) {
        "choir"
    } else if has(&["rap", "rapper", "rapped"]) {
        "rap"
    } else if has(&["female", "woman"]) {
        "female"
    } else if has(&["male", "man"]) {
        "male"
    } else {
        ""
    };
    let instruments: Vec<String> = match v.get("instruments") {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(|s| clean(s, 30)).collect(),
        Some(Value::String(s)) => s.split(',').map(|s| clean(s, 30)).collect(),
        _ => Vec::new(),
    };
    let bpm = match v.get("bpm") {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|b| (40.0..=220.0).contains(b))
    .map(|b| b.round() as u32);
    let style = Style {
        genre: text_of("genre", 40),
        voice: voice.into(),
        vocal: text_of("vocal", 160),
        instruments: instruments.into_iter().filter(|i| !i.is_empty()).take(6).collect(),
        production: text_of("production", 120),
        mood: text_of("mood", 24),
        bpm,
        writing: text_of("writing", 400),
    };
    if style.genre.is_empty() && style.vocal.is_empty() && style.production.is_empty() {
        return Err("it didn't describe them".into());
    }
    Ok(Some(style))
}

/// The artist's style, from the chat model (kept for the rest of the
/// session). Ok(None): it doesn't know them.
pub async fn look_up(state: &Arc<AppState>, name: &str) -> Result<Option<Style>, String> {
    static KNOWN: OnceLock<Mutex<HashMap<String, Style>>> = OnceLock::new();
    let key = name.trim().to_lowercase();
    if let Some(s) = KNOWN.get_or_init(Default::default).lock().unwrap().get(&key) {
        return Ok(Some(s.clone()));
    }
    let (model, raw) = ask(state, name).await?;
    let style = parse(&raw, name).map_err(|e| format!("{model} couldn't describe {name}: {e}."))?;
    if let Some(s) = &style {
        crate::applog::line("media", &format!("Style for {name:?}: {}", s.sound()));
        KNOWN.get_or_init(Default::default).lock().unwrap().insert(key, s.clone());
    }
    Ok(style)
}

/// A song asked in an artist's style: their style read and put on the
/// request, and their name taken out of the description. None when no
/// artist is asked for (or "sounds like summer" turned out not to be one).
pub async fn resolve(state: &Arc<AppState>, req: &Request, progress: &(dyn Fn(&str, f64) + Sync)) -> Option<Request> {
    if req.op != "music" {
        return None;
    }
    let mut spec = req.music.clone().unwrap_or_default();
    if !matches!(spec.task.as_deref(), None | Some("new" | "cover")) {
        return None;
    }
    let named = named_in(&req.prompt);
    let given = spec.sound_like.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let name = given.clone().or_else(|| named.as_ref().map(|(n, _)| n.clone()))?;
    let known = if spec.artist_sound.as_deref().is_some_and(|s| !s.trim().is_empty()) {
        true
    } else {
        progress("Reading the artist's style", 0.01);
        match look_up(state, &name).await {
            Ok(Some(style)) => {
                style.apply(&name, &mut spec);
                true
            }
            Ok(None) => {
                crate::applog::line("media", &format!("The chat model doesn't know {name:?} as an artist"));
                false
            }
            Err(e) => {
                crate::applog::line("media", &format!("Couldn't read {name:?}'s style ({e})"));
                false
            }
        }
    };
    if given.is_none() && !known {
        return None;
    }
    let mut out = req.clone();
    // The name stays out of the music model's description.
    if let Some((_, rest)) = named.filter(|(n, _)| known && n.eq_ignore_ascii_case(&name)) {
        out.prompt = rest;
    }
    spec.sound_like = Some(name);
    out.music = Some(spec);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_artist_is_found_in_the_description() {
        assert_eq!(named_in("Sing as Taylor Swift a song about Bears"), Some(("Taylor Swift".into(), "a song about Bears".into())));
        assert_eq!(named_in("a song about rain in the style of Lewis Capaldi"), Some(("Lewis Capaldi".into(), "a song about rain".into())));
        assert_eq!(named_in("Sing like Lewis Capaldi about heartbreak, slow piano"), Some(("Lewis Capaldi".into(), "about heartbreak, slow piano".into())));
        assert_eq!(named_in("make it sound like Florence and the Machine, with harps").map(|n| n.0), Some("Florence and the Machine".into()));
        assert_eq!(named_in("sung by Adele. A ballad about the sea").map(|n| n.0), Some("Adele".into()));
        assert_eq!(named_in("sing like a bird about freedom"), None, "not a name");
        assert_eq!(named_in("a road trip with old friends"), None);
        assert_eq!(named_in("Sing as Taylor Swift").map(|n| n.1), Some("a song".into()));
    }

    #[test]
    fn the_style_is_read_without_names() {
        let raw = r#"Here: {"known": true, "genre": "Country pop", "voice": "Female", "vocal": "bright, clear voice with a light twang, like Taylor Swift's", "instruments": ["acoustic guitar", "banjo", "drums"], "production": "polished 2010s pop production", "mood": "Hopeful", "bpm": "118", "writing": "Taylor Swift writes diary-like stories with vivid details and a big chorus hook."}"#;
        let s = parse(raw, "Taylor Swift").unwrap().unwrap();
        assert_eq!(s.voice, "female");
        assert_eq!(s.bpm, Some(118));
        assert!(!s.vocal.contains("Taylor") && !s.writing.contains("Taylor"), "{s:?}");
        assert_eq!(s.sound(), "bright, clear voice with a light twang, like, polished 2010s pop production");
        assert_eq!(parse(r#"{"known": false}"#, "Nobody"), Ok(None));
        assert_eq!(parse(r#"{"genre": "Unknown", "voice": ""}"#, "Nobody"), Ok(None));
        assert!(parse("I don't know", "x").is_err());
        // Onto a song: the person's own choices stay.
        let mut spec = MusicSpec { genre: Some("Rock".into()), ..Default::default() };
        s.apply("Taylor Swift", &mut spec);
        assert_eq!(spec.genre.as_deref(), Some("Rock"));
        assert_eq!(spec.voice.as_deref(), Some("female"));
        assert_eq!(spec.instruments.len(), 3);
        assert!(spec.artist_writing.as_deref().is_some_and(|w| w.contains("diary-like")));
        let caption = spec.caption("a song about bears", false);
        assert!(caption.contains("bright, clear voice") && !caption.contains("Taylor"), "{caption}");
        assert!(spec.lyrics_guide().contains("Write the way Taylor Swift writes songs"), "the writer is told the name");
        assert!(spec.lyrics_guide().contains("never use lines, hooks or titles from their songs"));
    }
}
