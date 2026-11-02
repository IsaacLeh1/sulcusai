// SPDX-License-Identifier: AGPL-3.0-only
//! What a song is asked to be (`MusicSpec`): its voice, key, structure,
//! instruments and effects, and how each becomes the music model's caption.

use super::*;

/// A song as the person shaped it in the Studio or asked in a chat: its
/// style, instruments, voice and beat (folded into the music model's
/// description), its tempo, key and time signature (the music model's own
/// settings), and work on a song they have.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct MusicSpec {
    /// "Pop", "Jazz", "Lo-fi hip hop"…
    pub genre: Option<String>,
    /// "Happy", "Dark", "Dreamy"…
    pub mood: Option<String>,
    pub instruments: Vec<String>,
    /// The singing: "female", "male", "duet", "choir", "rap" (None: as described).
    pub voice: Option<String>,
    /// The beat's feel: "four on the floor", "trap hi-hats", "swing"…
    pub beat: Option<String>,
    /// Beats a minute (None: the music model picks).
    pub bpm: Option<u32>,
    /// "C major", "F# minor".
    pub key: Option<String>,
    /// "4/4", "3/4", "6/8".
    pub time_signature: Option<String>,
    /// Work on a song they have (`audio`): "cover" (the same song in another
    /// style), "repaint" (a part made again), "extend" (more after its end),
    /// "add" (a new part on top), "extract" (one part alone), "complete"
    /// (a full arrangement around a lone part). None or "new": a new song.
    pub task: Option<String>,
    /// The gallery's song to work on.
    pub audio: Option<String>,
    /// A song whose sound (voices and instruments) the new one follows.
    pub reference: Option<String>,
    /// The part to make again, in seconds from the start (repaint); for
    /// extending, how many seconds to add is `extend_by`.
    pub region: Option<[f32; 2]>,
    pub extend_by: Option<f32>,
    /// The part to add, take out or complete with ("vocals", "drums"…).
    pub track: Option<String>,
    /// A cover: how close to the original it stays (0 to 1).
    pub strength: Option<f32>,
    /// How the lead sings: "smooth", "powerful", "breathy", "raspy", "soulful",
    /// "falsetto", "whispered", "belting".
    pub vocal_style: Option<String>,
    /// Who sings harmonies: "female", "male", "mixed", "choir" (None: no sung harmonies).
    pub harmony_voice: Option<String>,
    /// The instrument playing a harmony line ("strings", "piano"…; None: none).
    pub harmony_instrument: Option<String>,
    /// Where harmonies come in: "chorus" (the default), "throughout", "ending".
    pub harmony_where: Option<String>,
    /// How they're sung: "thirds" (close), "octave", "stacked", "call" (call and
    /// response), "oohs".
    pub harmony_style: Option<String>,
    /// Layered on in a second pass by a model that adds parts (fuller; slower).
    pub harmony_layer: bool,
    /// The instrument that carries the melody's hook and a solo.
    pub lead_instrument: Option<String>,
    /// The song's shape: "verse-chorus", "verse-chorus-bridge", "intro-outro",
    /// "build-drop", "aaba", "story".
    pub structure: Option<String>,
    /// Sections in the order chosen ("Intro", "Verse", "Chorus", "Chorus"…);
    /// when given, they're the shape (`structure` is only a starting point).
    pub sections: Vec<String>,
    /// How lines rhyme: "aabb", "abab", "abcb", "aaaa", "free".
    pub rhyme: Option<String>,
    /// How it moves: "steady", "builds", "big-chorus", "quiet".
    pub energy: Option<String>,
    /// How it ends: "fade", "big-finish", "hard-stop".
    pub ending: Option<String>,
    /// The singer's voice in the music model's words ("warm alto with a
    /// slight rasp"): a saved artist's, the same in each of their songs.
    pub voice_detail: Option<String>,
    /// A saved artist (`persona::Persona`, by id or name): their sound and
    /// voice for this song, and the song kept as their voice if it's their first.
    pub persona: Option<String>,
    /// The length follows the song (`auto_seconds`): a length named in the
    /// description, else what the lyrics or the sections need, else what
    /// the lyrics written for it come to.
    pub auto_length: bool,
    /// Sung in a voice the person recorded (`own_voice`, by id): the song is
    /// made by a singer of their sex, then its voice is sung again in theirs
    /// and in their range (the version before kept out of the gallery, as
    /// its original).
    pub own_voice: Option<String>,
    /// Of the versions made, only the one whose words were heard best is
    /// kept (`keep_clearest`).
    pub best_of: bool,
    /// Each song gets cover art from the picture model, made next (`cover_art`).
    pub cover_art: bool,
    /// A custom model trained on the person's songs (`train`, by id): the
    /// song is made on ACE-Step 1.5 Base with it.
    pub custom: Option<String>,
    /// How strongly the custom model is followed (1 as trained).
    pub custom_scale: Option<f32>,
    /// How closely the description is followed, 0 to 1 (Suno's Style
    /// Influence): the music model's guidance (`music::guidance`). None: its own.
    pub influence: Option<f32>,
    /// How far from the usual, 0 to 1 (Suno's Weirdness): the words, the
    /// description and YuE2's melody writing. None: in between.
    pub weirdness: Option<f32>,
    /// Styles, instruments or sounds to leave out, comma separated.
    pub exclude: Option<String>,
    /// The song's name, given by the person (else its hook names it).
    pub title: Option<String>,
    /// The sound of the recording: "studio", "live", "lo-fi", "acoustic",
    /// "modern" (older songs may say a decade, as `era` does now).
    pub production: Option<String>,
    /// The decade the song is from, in its writing, instruments and sound:
    /// "1950s" to "2020s".
    pub era: Option<String>,
    /// The feeling in the words and the voice: "heartbroken", "joyful"…
    pub emotion: Option<String>,
    /// How the voice is used, and where: screamed choruses, heavy vibrato throughout.
    pub vocal_effects: Vec<VocalEffect>,
    /// An artist whose style the song follows. The name goes to the lyrics
    /// writer only; the music model gets `artist_sound`.
    pub sound_like: Option<String>,
    /// Their sound in plain words, no names (`artist::look_up` fills it).
    pub artist_sound: Option<String>,
    /// How they write lyrics, for the lyrics writer.
    pub artist_writing: Option<String>,
}

/// A way of singing, and the part of the song it's on.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct VocalEffect {
    /// "vibrato", "belting", "falsetto", "screamed", "growled", "raspy",
    /// "breathy", "whispered", "spoken", "rapped", "autotune", "runs",
    /// "operatic", or the person's own words.
    pub how: String,
    /// "all" (the default), "verse", "pre-chorus", "chorus", "bridge", "final-chorus".
    pub on: String,
}

impl VocalEffect {
    /// As the music model's description puts it ("screamed vocals").
    pub(super) fn described(&self) -> String {
        let how = self.how.trim().to_lowercase();
        match how.as_str() {
            "vibrato" => "vocals with heavy vibrato".into(),
            "belting" => "powerful belted vocals".into(),
            "falsetto" => "falsetto vocals".into(),
            "screamed" => "screamed vocals, screamo".into(),
            "growled" => "growled vocals".into(),
            "raspy" => "raspy, gritty vocals".into(),
            "breathy" => "breathy vocals".into(),
            "whispered" => "whispered vocals".into(),
            "spoken" => "spoken word vocals".into(),
            "rapped" => "rapped vocals".into(),
            "autotune" => "auto-tuned vocals".into(),
            "runs" => "soulful vocal runs and riffs".into(),
            "operatic" => "operatic vocals".into(),
            other if other.contains("vocal") || other.contains("voice") => other.to_string(),
            other => format!("{other} vocals"),
        }
    }

    /// As one word on a section's tag ("[Chorus - screamed]"): one only,
    /// since more were sung as words.
    pub(super) fn tag_word(&self) -> String {
        let how = self.how.trim().to_lowercase();
        match how.as_str() {
            "rapped" => "rap".into(),
            "autotune" => "auto-tuned".into(),
            "runs" => "vocal runs".into(),
            "vibrato" => "heavy vibrato".into(),
            other => other.chars().take(24).collect(),
        }
    }

    /// The part of the song, as words ("on the choruses").
    pub(super) fn place(&self) -> &'static str {
        match self.on.trim().to_lowercase().as_str() {
            "verse" => "on the verses",
            "pre-chorus" => "on the pre-choruses",
            "chorus" => "on the choruses",
            "bridge" => "on the bridge",
            "final-chorus" => "on the final chorus",
            _ => "throughout",
        }
    }
}

/// Sections in order for each song shape.
pub(super) const STRUCTURES: &[(&str, &[&str])] = &[
    ("verse-chorus", &["Verse", "Chorus", "Verse", "Chorus"]),
    ("verse-chorus-bridge", &["Verse", "Chorus", "Verse", "Chorus", "Bridge", "Chorus"]),
    ("intro-outro", &["Intro", "Verse", "Chorus", "Verse", "Chorus", "Outro"]),
    ("build-drop", &["Intro", "Verse", "Pre-Chorus", "Chorus", "Verse", "Pre-Chorus", "Chorus"]),
    ("aaba", &["Verse", "Verse", "Bridge", "Verse"]),
    ("story", &["Verse", "Verse", "Verse"]),
];

/// The music model's name for a harmony instrument's part (`music::PARTS`).
pub fn part_for(instrument: &str) -> Option<&'static str> {
    let i = instrument.trim().to_lowercase();
    let has = |w: &[&str]| w.iter().any(|x| i.contains(x));
    Some(if has(&["string", "violin", "cello", "viola"]) {
        "strings"
    } else if has(&["piano", "organ", "keys", "keyboard", "rhodes"]) {
        "keyboard"
    } else if has(&["synth", "pad"]) {
        "synth"
    } else if has(&["brass", "trumpet", "horn", "trombone"]) {
        "brass"
    } else if has(&["flute", "sax", "clarinet", "woodwind", "oboe"]) {
        "woodwinds"
    } else if has(&["guitar"]) {
        "guitar"
    } else {
        return None;
    })
}

impl MusicSpec {
    pub(super) fn some(o: &Option<String>) -> Option<String> {
        o.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| s.to_lowercase())
    }

    /// Who sings the harmonies, in the music model's words ("male", "male and female").
    pub(super) fn harmony_singers(&self) -> Option<String> {
        Self::some(&self.harmony_voice).map(|v| match v.as_str() {
            "mixed" => "male and female".to_string(),
            other => other.to_string(),
        })
    }

    /// Where the harmonies come in, as words.
    pub(super) fn harmony_place(&self) -> &'static str {
        match Self::some(&self.harmony_where).as_deref() {
            Some("throughout") => "throughout the song",
            Some("ending") => "on the final chorus",
            _ => "on the choruses",
        }
    }

    /// The sung harmonies, described ("female backing harmony vocals in close thirds on the choruses").
    pub fn harmony_vocals(&self) -> Option<String> {
        let who = self.harmony_singers()?;
        let how = match Self::some(&self.harmony_style).as_deref() {
            Some("octave") => "doubling the melody an octave apart",
            Some("stacked") => "in rich stacked gospel-style harmonies",
            Some("call") => "answering the lead in call and response",
            Some("oohs") => "singing soft oohs and aahs",
            _ => "in close harmony a third above the lead",
        };
        Some(format!("{who} backing harmony vocals {how} {}", self.harmony_place()))
    }

    /// The instrument harmony, described ("strings playing a harmony line under the melody on the choruses").
    pub fn harmony_line(&self) -> Option<String> {
        Self::some(&self.harmony_instrument).map(|i| format!("{i} playing a harmony line under the melody {}", self.harmony_place()))
    }

    /// The sections, in order, for the lyrics writer: the ones chosen, else
    /// the shape's (empty: the writer's own choice).
    pub fn sections(&self) -> Vec<String> {
        let chosen: Vec<String> = self.sections.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).take(24).collect();
        if !chosen.is_empty() {
            return chosen;
        }
        let s = Self::some(&self.structure).unwrap_or_default();
        STRUCTURES.iter().find(|(id, _)| *id == s).map_or(Vec::new(), |(_, list)| list.iter().map(|x| x.to_string()).collect())
    }

    /// What the lyrics writer is told beyond the song's description and its
    /// sections (`lyrics::plan`): the artist whose way of writing it follows,
    /// the feeling, how lines rhyme, lines fit for screaming or rapping, and
    /// echoes in brackets for the harmony voices.
    pub fn lyrics_guide(&self) -> String {
        let mut guide = String::new();
        if let Some(t) = self.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
            guide.push_str(&format!(" The song is called \"{t}\": build the chorus's hook around those words."));
        }
        let left_out = self.excluded();
        if !left_out.is_empty() {
            guide.push_str(&format!(" Leave these out of the words: {}.", left_out.join(", ")));
        }
        match self.weirdness {
            Some(w) if w >= 0.7 => guide.push_str(" Be daring: surprising images, unusual turns of phrase and an unexpected structure."),
            Some(w) if w <= 0.25 => guide.push_str(" Keep it familiar: plain, classic images and a standard song structure."),
            _ => {}
        }
        if let Some(name) = self.sound_like.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            let how = Self::some(&self.artist_writing).map(|w| format!(" ({})", w.trim_end_matches('.'))).unwrap_or_default();
            guide.push_str(&format!(" Write the way {name} writes songs{how}, in words of your own: never use lines, hooks or titles from their songs."));
        }
        if let Some(e) = Self::some(&self.emotion) {
            guide.push_str(&format!(" The words carry this feeling: {e}."));
        }
        if let Some(e) = Self::some(&self.era) {
            guide.push_str(&format!(" It's a song from the {e}: write the words the way songs were written then, with nothing in them from after that time."));
        }
        for fx in &self.vocal_effects {
            let rough = match fx.how.trim().to_lowercase().as_str() {
                "screamed" | "growled" => "screamed: make them short, raw and forceful",
                "rapped" => "rapped: give them rhythm and rhymes inside the lines, and they can be longer",
                "spoken" => "spoken, not sung",
                "whispered" => "whispered: make them soft and close",
                _ => continue,
            };
            let place = match fx.on.trim().to_lowercase().as_str() {
                "verse" => "under [Verse] tags",
                "pre-chorus" => "under [Pre-Chorus] tags",
                "chorus" => "under [Chorus] tags",
                "bridge" => "under the [Bridge] tag",
                "final-chorus" => "of the last [Chorus]",
                _ => "of the song",
            };
            guide.push_str(&format!(" The lines {place} are {rough}."));
        }
        if let Some(r) = Self::some(&self.rhyme) {
            guide.push_str(match r.as_str() {
                "aabb" => " Rhyme in pairs (AABB): lines 1 and 2 rhyme, then lines 3 and 4.",
                "abab" => " Rhyme alternate lines (ABAB): lines 1 and 3 rhyme, and lines 2 and 4.",
                "abcb" => " Rhyme the second and fourth lines of each four (ABCB).",
                "aaaa" => " Make all the lines of a section rhyme with each other (AAAA).",
                "free" => " Don't rhyme: write free verse.",
                _ => "",
            });
        }
        if self.harmony_singers().is_some() {
            let place = match Self::some(&self.harmony_where).as_deref() {
                Some("throughout") => "on some lines in every section",
                Some("ending") => "on the lines of the last chorus",
                _ => "on the chorus lines",
            };
            guide.push_str(&format!(
                " Add short backing-vocal echoes in round brackets at the end of lines {place}, for example: We'll find our way (find our way). \
                 Only the harmony singers sing what's in brackets, so keep them to two or three words."
            ));
        }
        guide
    }

    /// Lyrics with each section's way of singing on its tag ("[Chorus]"
    /// becomes "[Chorus - screamed]"): one word a tag, the part's own effect
    /// before the whole song's (those are in the description). Tags that
    /// already say something are left as they are.
    pub fn mark_sections(&self, lyrics: &str) -> String {
        let specific: Vec<&VocalEffect> = self.vocal_effects.iter().filter(|f| !f.how.trim().is_empty() && f.place() != "throughout").collect();
        if specific.is_empty() {
            return lyrics.to_string();
        }
        let last_chorus = lyrics.lines().enumerate().filter(|(_, l)| l.trim().eq_ignore_ascii_case("[chorus]")).map(|(i, _)| i).last();
        lyrics
            .lines()
            .enumerate()
            .map(|(i, line)| {
                let t = line.trim();
                let Some(name) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')).filter(|n| !n.contains('-') || n.eq_ignore_ascii_case("pre-chorus")) else {
                    return line.to_string();
                };
                // "Verse 2" is a verse.
                let kind = name.trim_end_matches(|c: char| c.is_ascii_digit() || c == ' ').to_lowercase();
                let fits = |f: &&&VocalEffect| match f.on.trim().to_lowercase().as_str() {
                    "final-chorus" => Some(i) == last_chorus,
                    on => on == kind,
                };
                let fx = specific.iter().find(|f| f.on.trim().eq_ignore_ascii_case("final-chorus") && fits(f)).or_else(|| specific.iter().find(|f| fits(f)));
                match fx {
                    Some(f) => format!("[{name} - {}]", f.tag_word()),
                    None => line.to_string(),
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Rapped rather than sung: a rap voice, a rap style (hip hop, trap,
    /// drill, grime), rapping over the whole song, or a rap asked for in
    /// the person's words.
    pub fn flow(&self, words: &str) -> lyrics::Flow {
        let voice = Self::some(&self.voice);
        let genre = Self::some(&self.genre).unwrap_or_default();
        // Lo-fi hip hop is soft singing or none.
        let style = ["hip hop", "hip-hop", "hiphop", "rap", "trap", "drill", "grime", "boom bap"].iter().any(|w| genre.contains(w)) && !["lo-fi", "lofi", "chill"].iter().any(|w| genre.contains(w));
        let rapped = self.vocal_effects.iter().any(|f| f.how.trim().eq_ignore_ascii_case("rapped") && matches!(f.on.trim(), "" | "all"));
        let w = words.to_lowercase();
        let said = w.split(|c: char| !c.is_alphanumeric() && c != '-').any(|x| matches!(x, "rap" | "raps" | "rapper" | "rapping" | "rapped" | "freestyle" | "hip-hop" | "hiphop"))
            || w.contains("hip hop");
        let rap = voice.as_deref() == Some("rap") || rapped || ((style || said) && !matches!(voice.as_deref(), Some("choir")));
        if rap {
            lyrics::Flow::Rap
        } else {
            lyrics::Flow::Sung
        }
    }

    /// Says enough about the song to make one without other words.
    pub fn describes(&self) -> bool {
        self.genre.as_deref().is_some_and(|g| !g.trim().is_empty()) || !self.instruments.is_empty()
    }

    /// The music model's description: the person's words, then the style,
    /// mood, instruments, voice, harmonies, beat and arrangement as ACE-Step's
    /// captions put them.
    pub fn caption(&self, words: &str, instrumental: bool) -> String {
        let mut parts: Vec<String> = Vec::new();
        // Said again last (the singer's sex).
        let mut tail: Option<String> = None;
        let words = words.trim().trim_end_matches('.');
        if !words.is_empty() {
            parts.push(words.to_string());
        }
        let some = |o: &Option<String>| o.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        if let Some(g) = some(&self.genre) {
            parts.push(g.to_lowercase());
        }
        if let Some(e) = Self::some(&self.era) {
            parts.push(format!("{e} style, a song from the {e} with the instruments and recording sound of that era"));
        }
        // An artist's sound in plain words (the music model doesn't know names).
        if let Some(a) = some(&self.artist_sound) {
            parts.push(a.trim_end_matches('.').to_string());
        }
        if let Some(m) = some(&self.mood) {
            parts.push(format!("{} mood", m.to_lowercase()));
        }
        if let Some(e) = Self::some(&self.emotion) {
            parts.push(if instrumental { format!("{e} feeling") } else { format!("{e}, emotional vocal delivery") });
        }
        let left_out = self.excluded();
        let inst: Vec<String> = self.instruments.iter().map(|i| i.trim().to_lowercase()).filter(|i| !i.is_empty() && !names_any(i, &left_out)).collect();
        if !inst.is_empty() {
            parts.push(inst.join(", "));
        }
        if let Some(l) = Self::some(&self.lead_instrument) {
            parts.push(format!("{l} lead melody and solo"));
        }
        if instrumental {
            parts.push("instrumental, no vocals".into());
        } else {
            let style = Self::some(&self.vocal_style).map(|s| format!("{s} ")).unwrap_or_default();
            let rap = self.flow(words) == lyrics::Flow::Rap;
            let singer = match some(&self.voice).map(|v| v.to_lowercase()) {
                Some(v) => Some(match v.as_str() {
                    "female" | "male" if rap => format!("{style}{v} rapper, rap vocals"),
                    "female" | "male" => format!("{style}{v} lead vocals"),
                    "rap" => format!("{style}rap vocals, rapper"),
                    "choir" => format!("{style}choir vocals"),
                    "duet" => format!("{style}male and female duet vocals"),
                    other => format!("{style}{other} vocals"),
                }),
                None if rap => Some(format!("{style}rap vocals, rapper")),
                None if !style.is_empty() => Some(format!("{style}vocals")),
                None => None,
            };
            // Who sings goes first after the person's words, and a man or a
            // woman is said again at the end: placed among the instruments it
            // was lost (a male voice asked for came out female). ACE-Step's
            // guide: repeat what matters most.
            if let Some(s) = singer {
                parts.insert(usize::from(!words.is_empty()), s);
                if let Some(v) = some(&self.voice).map(|v| v.to_lowercase()).filter(|v| v == "male" || v == "female") {
                    let who = if v == "male" { "man" } else { "woman" };
                    tail = Some(format!("sung by a {who}, {v} voice throughout"));
                }
            }
            if let Some(d) = some(&self.voice_detail) {
                parts.push(d.trim_end_matches('.').to_string());
            }
            // Rapped, not sung: said plainly, with the flow (rap sung as a
            // melody came out half-sung and thin).
            if rap {
                parts.push("rapping with a tight rhythmic flow, spoken rhymes on the beat, not sung".into());
            }
            for fx in self.vocal_effects.iter().filter(|f| !f.how.trim().is_empty()) {
                parts.push(format!("{} {}", fx.described(), fx.place()));
            }
            if let Some(h) = self.harmony_vocals() {
                parts.push(h);
            }
        }
        if let Some(h) = self.harmony_line() {
            parts.push(h);
        }
        if let Some(b) = some(&self.beat) {
            parts.push(format!("{} beat", b.to_lowercase()));
        }
        if instrumental && !self.sections().is_empty() {
            let named: Vec<String> = self.sections().iter().map(|s| s.to_lowercase()).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
            parts.push(format!("arranged with {}", named.join(", ")));
        }
        if let Some(e) = Self::some(&self.energy) {
            parts.push(match e.as_str() {
                "builds" => "builds up gradually".into(),
                "big-chorus" => "soft verses and a big, powerful chorus".into(),
                "quiet" => "quiet and intimate".into(),
                "steady" => "steady energy".into(),
                other => other.to_string(),
            });
        }
        if let Some(p) = Self::some(&self.production) {
            parts.push(match p.as_str() {
                "studio" => "polished studio production".into(),
                "live" => "live band recording".into(),
                "lo-fi" => "lo-fi production".into(),
                "acoustic" => "acoustic, unplugged".into(),
                "modern" => "modern production".into(),
                decade if decade.ends_with('s') => format!("{decade} production"),
                other => format!("{other} production"),
            });
        }
        if let Some(e) = Self::some(&self.ending) {
            parts.push(match e.as_str() {
                "fade" => "fades out at the end".into(),
                "big-finish" => "ends with a big finish".into(),
                "hard-stop" => "ends abruptly".into(),
                other => other.to_string(),
            });
        }
        // Weirdness, Suno's way: far from the usual at the top, by the book
        // at the bottom.
        match self.weirdness {
            Some(w) if w >= 0.7 => parts.push("experimental, unexpected arrangement, unusual sounds and surprising turns".into()),
            Some(w) if w <= 0.25 => parts.push("classic, familiar arrangement".into()),
            _ => {}
        }
        // What's left out: no part that names it (the person's own words
        // stay as written).
        if !left_out.is_empty() {
            let own = usize::from(!words.is_empty());
            let mut i = 0;
            parts.retain(|p| {
                i += 1;
                i <= own || !names_any(p, &left_out)
            });
        }
        parts.extend(tail);
        parts.join(", ")
    }

    /// What's to be left out, each lowercased without a leading "no".
    pub fn excluded(&self) -> Vec<String> {
        let list = self.exclude.as_deref().unwrap_or_default();
        list.split([',', ';', '\n'])
            .map(|s| {
                let s = s.trim().to_lowercase();
                s.strip_prefix("no ").or_else(|| s.strip_prefix("without ")).unwrap_or(&s).trim().to_string()
            })
            .filter(|s| !s.is_empty())
            .collect()
    }
}

/// Whether a phrase names any of `terms` as whole words ("drums" is in
/// "punchy live drums", not in "drumsticks").
pub(super) fn names_any(phrase: &str, terms: &[String]) -> bool {
    let norm = |s: &str| format!(" {} ", s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" "));
    let p = norm(phrase);
    terms.iter().any(|t| {
        let t = norm(t);
        t.trim().len() > 1 && p.contains(&t)
    })
}
