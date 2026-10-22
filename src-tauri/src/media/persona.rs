// SPDX-License-Identifier: AGPL-3.0-only
//! Artists the person made up, kept so their songs sound like one artist:
//! one voice (a fixed description of the singer in every song, and the
//! seed of their first song), their sound (style, instruments, production,
//! key, tempo, harmonies, ways of singing) and the way they write. Kept
//! encrypted in the settings.
//!
//! Their first song can also be followed as a reference (`--ref-audio`),
//! but only when asked: on ACE-Step 1.5 that blurred the words. Whisper
//! heard 5 of 18 lyric words following the whole song, and 0 of 14
//! following its vocals alone, against 14 of 18 and 14 of 17 without.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::MusicSpec;
use crate::crypto::Cipher;
use crate::db;

const KEY: &str = "music_personas";

/// A made-up artist.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Persona {
    pub id: String,
    pub name: String,
    /// Their voice in the music model's words ("warm alto with a slight
    /// rasp"), in every one of their songs' descriptions.
    pub voice_words: String,
    /// Their first song (unless another is picked): its seed starts their
    /// later songs, and its voice is followed when `follow_voice_song`.
    pub voice_song: Option<String>,
    pub seed: Option<i64>,
    pub follow_voice_song: bool,
    /// How their songs sound (the song-only parts left out: `sound_of`).
    pub sound: MusicSpec,
    /// The language they sing in ("en", "es"…; None: the lyrics').
    pub language: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// What of a song's settings belongs to its artist: everything but the
/// work on one song (task, source, region, part, strength), its sections,
/// its length and the artist link itself.
pub fn sound_of(spec: &MusicSpec) -> MusicSpec {
    MusicSpec {
        task: None,
        audio: None,
        region: None,
        extend_by: None,
        track: None,
        strength: None,
        reference: None,
        sections: Vec::new(),
        structure: None,
        auto_length: false,
        persona: None,
        voice_detail: None,
        ..spec.clone()
    }
}

/// Every saved artist, by name.
pub fn list(conn: &Connection, c: &Cipher) -> Vec<Persona> {
    let mut all: Vec<Persona> = db::get::<String>(conn, KEY).and_then(|s| c.decrypt(&s).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    all.sort_by_key(|p| p.name.to_lowercase());
    all
}

fn store(conn: &Connection, c: &Cipher, all: &[Persona]) -> Result<(), String> {
    db::set(conn, KEY, &c.encrypt(&serde_json::to_string(all).map_err(|e| e.to_string())?))
}

/// One by its id, or its name (any case).
pub fn find(conn: &Connection, c: &Cipher, id_or_name: &str) -> Option<Persona> {
    let k = id_or_name.trim();
    list(conn, c).into_iter().find(|p| p.id == k || p.name.eq_ignore_ascii_case(k))
}

/// Saves an artist (new without an id), its sound kept to what belongs to
/// an artist. Two can't share a name.
pub fn save(conn: &Connection, c: &Cipher, mut p: Persona) -> Result<Persona, String> {
    p.name = p.name.trim().chars().take(60).collect();
    if p.name.is_empty() {
        return Err("Give the artist a name.".into());
    }
    let mut all = list(conn, c);
    if all.iter().any(|o| o.name.eq_ignore_ascii_case(&p.name) && o.id != p.id) {
        return Err(format!("There's already an artist called {}.", p.name));
    }
    let now = db::now_ms();
    p.sound = sound_of(&p.sound);
    if p.voice_words.trim().is_empty() {
        p.voice_words = voice_words_for(&p.name, p.sound.voice.as_deref());
    }
    p.voice_words = p.voice_words.trim().chars().take(120).collect();
    p.updated_at = now;
    match all.iter_mut().find(|o| o.id == p.id && !p.id.is_empty()) {
        Some(o) => {
            p.created_at = o.created_at;
            *o = p.clone();
        }
        None => {
            p.id = uuid::Uuid::new_v4().to_string();
            p.created_at = now;
            all.push(p.clone());
        }
    }
    store(conn, c, &all)?;
    Ok(p)
}

pub fn remove(conn: &Connection, c: &Cipher, id: &str) -> Result<(), String> {
    let mut all = list(conn, c);
    all.retain(|p| p.id != id);
    store(conn, c, &all)
}

/// Their first song becomes their voice song, and its seed theirs, when
/// they have none yet (or theirs is gone from the gallery: `have`).
pub fn first_voice(conn: &Connection, c: &Cipher, id: &str, song: &str, seed: Option<i64>, have: impl Fn(&str) -> bool) -> Result<bool, String> {
    let mut all = list(conn, c);
    let Some(p) = all.iter_mut().find(|p| p.id == id && !p.voice_song.as_deref().is_some_and(|v| have(v))) else { return Ok(false) };
    p.voice_song = Some(song.to_string());
    p.seed = p.seed.or(seed);
    p.updated_at = db::now_ms();
    store(conn, c, &all)?;
    Ok(true)
}

/// A voice for a new artist, the same for the same name: a register and a
/// texture, so two artists don't sound alike.
pub fn voice_words_for(name: &str, voice: Option<&str>) -> String {
    let h = name.to_lowercase().bytes().fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32)) as usize;
    let tones: &[&str] = match voice.map(str::to_lowercase).as_deref() {
        Some("female") => &["warm alto", "bright soprano", "husky mezzo-soprano", "airy soprano", "smoky alto", "clear mezzo-soprano"],
        Some("male") => &["warm baritone", "bright tenor", "gravelly baritone", "smooth tenor", "deep bass-baritone", "soft high tenor"],
        Some("rap") => &["deep, punchy rap voice", "crisp, fast rap voice", "laid-back, raspy rap voice", "bright, energetic rap voice"],
        _ => &["warm mid-range voice", "bright, clear voice", "husky voice", "smooth, rich voice"],
    };
    let textures = ["with a slight rasp", "with soft vibrato", "with a clear, pure tone", "with a breathy edge", "with a gentle twang", "with a rich, full tone"];
    format!("{} {}", tones[h % tones.len()], textures[(h / 7) % textures.len()])
}

impl Persona {
    /// Onto a song: their voice words, their sound where the song didn't
    /// set its own (the person's choices for this song win), their voice
    /// song to follow when asked (`have`: whether it's still in the
    /// gallery), and their language. Their seed goes on the request (`with_persona`).
    pub fn apply(&self, spec: &mut MusicSpec, language: &mut Option<String>, have: impl Fn(&str) -> bool) {
        let s = &self.sound;
        let fill = |mine: &mut Option<String>, theirs: &Option<String>| {
            if mine.as_deref().map_or(true, |m| m.trim().is_empty()) {
                *mine = theirs.clone();
            }
        };
        fill(&mut spec.genre, &s.genre);
        fill(&mut spec.mood, &s.mood);
        fill(&mut spec.voice, &s.voice);
        fill(&mut spec.beat, &s.beat);
        fill(&mut spec.key, &s.key);
        fill(&mut spec.time_signature, &s.time_signature);
        fill(&mut spec.vocal_style, &s.vocal_style);
        fill(&mut spec.harmony_voice, &s.harmony_voice);
        fill(&mut spec.harmony_instrument, &s.harmony_instrument);
        fill(&mut spec.harmony_where, &s.harmony_where);
        fill(&mut spec.harmony_style, &s.harmony_style);
        fill(&mut spec.lead_instrument, &s.lead_instrument);
        fill(&mut spec.rhyme, &s.rhyme);
        fill(&mut spec.energy, &s.energy);
        fill(&mut spec.ending, &s.ending);
        fill(&mut spec.production, &s.production);
        fill(&mut spec.emotion, &s.emotion);
        fill(&mut spec.sound_like, &s.sound_like);
        fill(&mut spec.artist_sound, &s.artist_sound);
        fill(&mut spec.artist_writing, &s.artist_writing);
        fill(&mut spec.voice_detail, &Some(self.voice_words.clone()).filter(|w| !w.trim().is_empty()));
        if spec.instruments.is_empty() {
            spec.instruments = s.instruments.clone();
        }
        if spec.vocal_effects.is_empty() {
            spec.vocal_effects = s.vocal_effects.clone();
        }
        spec.bpm = spec.bpm.or(s.bpm);
        spec.harmony_layer |= s.harmony_layer;
        if spec.reference.is_none() && self.follow_voice_song {
            spec.reference = self.voice_song.clone().filter(|v| have(v));
        }
        if language.as_deref().map_or(true, |l| l.is_empty() || l == "auto") {
            *language = self.language.clone().or(language.take());
        }
        spec.persona = Some(self.id.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (Connection, Cipher) {
        let d = std::env::temp_dir().join(format!("sulcusai-persona-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        (db::open(&d.join("t.db")).unwrap(), Cipher::random())
    }

    /// Saved, found by name, kept apart by name, and removed; a voice of
    /// its own; the first song gives its seed; a song's own choices win; the
    /// voice song is followed only when asked.
    #[test]
    fn an_artist_keeps_its_sound_and_voice() {
        let (conn, c) = setup();
        let sound = MusicSpec {
            genre: Some("Indie pop".into()),
            voice: Some("female".into()),
            vocal_style: Some("breathy".into()),
            instruments: vec!["Piano".into()],
            bpm: Some(96),
            sections: vec!["Verse".into()],
            task: Some("cover".into()),
            ..Default::default()
        };
        let nova = save(&conn, &c, Persona { name: " Nova ".into(), sound, language: Some("en".into()), ..Default::default() }).unwrap();
        assert_eq!(nova.name, "Nova");
        assert!(nova.sound.sections.is_empty() && nova.sound.task.is_none(), "a song's own parts aren't the artist's");
        assert_eq!(nova.voice_words, voice_words_for("Nova", Some("female")), "a voice of her own");
        assert!(["alto", "soprano"].iter().any(|w| nova.voice_words.contains(w)), "{}", nova.voice_words);
        assert_ne!(voice_words_for("Nova", Some("female")), voice_words_for("Ivy", Some("female")), "two artists, two voices");
        assert!(save(&conn, &c, Persona { name: "nova".into(), ..Default::default() }).is_err(), "names are unique");
        assert_eq!(find(&conn, &c, "NOVA").map(|p| p.id), Some(nova.id.clone()));
        assert!(db::get::<String>(&conn, KEY).is_some_and(|s| !s.contains("Indie")), "kept encrypted");
        // The first song is their voice song and gives its seed; a second doesn't replace it.
        assert!(first_voice(&conn, &c, &nova.id, "song1", Some(42), |_| true).unwrap());
        assert!(!first_voice(&conn, &c, &nova.id, "song2", Some(7), |_| true).unwrap());
        // Deleted from the gallery: the next song takes its place, the seed stays.
        assert!(first_voice(&conn, &c, &nova.id, "song3", Some(9), |v| v != "song1").unwrap());
        assert!(first_voice(&conn, &c, &nova.id, "song1", Some(9), |v| v != "song3").unwrap());
        let nova = find(&conn, &c, &nova.id).unwrap();
        assert_eq!((nova.voice_song.as_deref(), nova.seed), (Some("song1"), Some(42)));
        // Onto a song: theirs where the song chose nothing, the song's own otherwise.
        let mut spec = MusicSpec { genre: Some("Rock".into()), ..Default::default() };
        let mut lang = None;
        nova.apply(&mut spec, &mut lang, |_| true);
        assert_eq!(spec.genre.as_deref(), Some("Rock"));
        assert_eq!(spec.voice.as_deref(), Some("female"));
        assert_eq!(spec.vocal_style.as_deref(), Some("breathy"));
        assert_eq!(spec.voice_detail.as_deref(), Some(nova.voice_words.as_str()));
        assert_eq!(spec.bpm, Some(96));
        assert_eq!(spec.reference, None, "her first song isn't followed unless asked");
        assert_eq!(spec.persona.as_deref(), Some(nova.id.as_str()));
        assert_eq!(lang.as_deref(), Some("en"));
        // Asked to follow it: followed while it's in the gallery.
        let follows = Persona { follow_voice_song: true, ..nova.clone() };
        let mut spec = MusicSpec::default();
        follows.apply(&mut spec, &mut None, |_| true);
        assert_eq!(spec.reference.as_deref(), Some("song1"));
        let mut spec = MusicSpec::default();
        follows.apply(&mut spec, &mut None, |_| false);
        assert_eq!(spec.reference, None);
        remove(&conn, &c, &nova.id).unwrap();
        assert!(list(&conn, &c).is_empty());
    }
}
