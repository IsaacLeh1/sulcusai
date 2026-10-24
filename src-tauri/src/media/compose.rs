// SPDX-License-Identifier: AGPL-3.0-only
//! Compose: a song written as notes, the way a music program holds one.
//! The chat model writes a plan (the sections, their bars and chords, how
//! the drums, bass, chords and melody play in each, and the words); the
//! arranger here turns it into notes on the bar grid, for any tempo and
//! time signature, so everything lands in place. The window plays the notes
//! on sampled instruments (a General MIDI sound set), and every note can be
//! edited there. Asked for a change ("a busier bass in the chorus"), the
//! chat model changes the plan of that section, and only that section's
//! notes are written again. "Make it real" has the music model record a
//! version that follows the arrangement and sings the words (`make_real`).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// A section of the plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlanSection {
    /// "Intro", "Verse", "Chorus", "Bridge", "Solo", "Outro"…
    pub name: String,
    pub bars: u32,
    /// A chord a bar ("Am", "F", "C/E", "G7"); fewer repeat, more are cut.
    pub chords: Vec<String>,
    /// none, light, steady, busy, half-time.
    pub drums: String,
    /// none, roots, pulse, walking, syncopated.
    pub bass: String,
    /// The chords part: none, held, arpeggio, strummed, stabs.
    pub keys: String,
    /// The melody plays here (sung where there are lines).
    pub melody: bool,
    /// The words sung here, a line each.
    pub lines: Vec<String>,
    /// 1 (quiet) to 5 (biggest).
    pub energy: u8,
    /// Changed to write the section's notes again a new way.
    pub take: u64,
}

impl Default for PlanSection {
    fn default() -> Self {
        PlanSection { name: "Verse".into(), bars: 8, chords: vec!["C".into(), "G".into(), "Am".into(), "F".into()], drums: "steady".into(), bass: "roots".into(), keys: "held".into(), melody: true, lines: Vec::new(), energy: 3, take: 0 }
    }
}

/// The instruments of the four parts, by name (`program`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Instruments {
    pub chords: String,
    pub melody: String,
    pub bass: String,
    pub drums: String,
}

impl Default for Instruments {
    fn default() -> Self {
        Instruments { chords: "piano".into(), melody: "voice".into(), bass: "electric bass".into(), drums: "standard kit".into() }
    }
}

/// The song's plan, as the chat model writes it and the person changes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Plan {
    pub title: String,
    /// Quarter notes a minute, in any time signature (as music programs count it).
    pub bpm: u32,
    /// Beats in a bar, and the note that counts a beat (4: quarter, 8: eighth).
    pub beats: u32,
    pub unit: u32,
    /// "C major", "A minor".
    pub key: String,
    /// pop, rock, folk, ballad, hip hop, funk, jazz, electronic, waltz…
    pub style: String,
    pub instruments: Instruments,
    pub sections: Vec<PlanSection>,
    /// Kept for its new notes: the same plan arranges the same way.
    pub seed: u64,
    /// What the person asked for, kept for "Make it real".
    pub description: String,
    /// Who sings when it's made real: "female", "male", "duet", "choir" or "" (as described).
    pub voice: String,
    /// The decade it's from ("1980s"; "" any): how it's arranged and made real.
    pub era: String,
    /// Every song option picked in the Music tab (emotion, ways of singing,
    /// harmonies, production…): what "Make it real" asks the music model for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub song: Option<super::MusicSpec>,
    /// The language it's sung in (a two-letter code; none: the words').
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

impl Default for Plan {
    fn default() -> Self {
        Plan {
            title: String::new(),
            bpm: 100,
            beats: 4,
            unit: 4,
            key: "C major".into(),
            style: "pop".into(),
            instruments: Instruments::default(),
            sections: Vec::new(),
            seed: 1,
            description: String::new(),
            voice: String::new(),
            era: String::new(),
            song: None,
            language: None,
        }
    }
}

/// A note, in beats from the song's start.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub beat: f32,
    pub len: f32,
    pub pitch: u8,
    pub vel: u8,
}

/// A part: its instrument and notes. Drums play on MIDI channel 10.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Part {
    /// "melody", "chords", "bass" or "drums".
    pub name: String,
    pub instrument: String,
    pub notes: Vec<Note>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Composition {
    pub plan: Plan,
    pub parts: Vec<Part>,
}

impl Plan {
    pub fn bar_beats(&self) -> f32 {
        self.beats.max(1) as f32
    }
    /// Where each section starts, in beats, and the song's length.
    pub fn starts(&self) -> (Vec<f32>, f32) {
        let mut at = 0.0;
        let mut out = Vec::new();
        for s in &self.sections {
            out.push(at);
            at += s.bars as f32 * self.bar_beats();
        }
        (out, at)
    }
    pub fn seconds(&self) -> f64 {
        // Beats are the time signature's unit: an eighth is half a quarter.
        self.starts().1 as f64 * 4.0 / self.unit.max(1) as f64 * 60.0 / self.bpm.max(20) as f64
    }
    /// The words, tagged by section, as the music model sings them.
    pub fn lyrics(&self) -> String {
        self.sections
            .iter()
            .map(|s| if s.lines.is_empty() { format!("[{}]", s.name) } else { format!("[{}]\n{}", s.name, s.lines.join("\n")) })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
    /// "4/4", "6/8".
    pub fn time(&self) -> String {
        format!("{}/{}", self.beats, self.unit)
    }
}

// ---------- chords and keys ----------

const NAMES: [(&str, i32); 17] =
    [("C", 0), ("C#", 1), ("Db", 1), ("D", 2), ("D#", 3), ("Eb", 3), ("E", 4), ("F", 5), ("F#", 6), ("Gb", 6), ("G", 7), ("G#", 8), ("Ab", 8), ("A", 9), ("A#", 10), ("Bb", 10), ("B", 11)];

/// A note name's pitch class and what follows it ("F#m7" gives 6, "m7").
fn root(s: &str) -> Option<(i32, &str)> {
    let s = s.trim();
    // Two-letter names first ("C#" before "C").
    let mut best: Option<(i32, &str)> = None;
    for (n, pc) in NAMES {
        if let Some(rest) = s.strip_prefix(n) {
            if best.is_none_or(|(_, r)| rest.len() < r.len()) {
                best = Some((pc, rest));
            }
        }
    }
    best
}

/// A chord's pitch classes above its root, its root and its bass note:
/// "Am7" is [0, 3, 7, 10] on A; "C/E" plays C with E below.
pub fn chord(name: &str) -> Option<(i32, Vec<i32>, i32)> {
    let (main, bass) = match name.split_once('/') {
        Some((m, b)) => (m, root(b).map(|(pc, _)| pc)),
        None => (name, None),
    };
    let (r, q) = root(main)?;
    let q = q.trim().to_lowercase();
    let iv: &[i32] = match q.as_str() {
        "" | "maj" | "major" => &[0, 4, 7],
        "m" | "min" | "minor" | "-" => &[0, 3, 7],
        "dim" | "o" => &[0, 3, 6],
        "aug" | "+" => &[0, 4, 8],
        "sus2" => &[0, 2, 7],
        "sus" | "sus4" => &[0, 5, 7],
        "7" | "dom7" => &[0, 4, 7, 10],
        "maj7" | "m7+" | "ma7" | "major7" => &[0, 4, 7, 11],
        "m7" | "min7" | "-7" => &[0, 3, 7, 10],
        "m7b5" | "ø" | "half-dim" => &[0, 3, 6, 10],
        "dim7" | "o7" => &[0, 3, 6, 9],
        "6" => &[0, 4, 7, 9],
        "m6" => &[0, 3, 7, 9],
        "add9" | "add2" => &[0, 4, 7, 14],
        "madd9" => &[0, 3, 7, 14],
        "9" => &[0, 4, 7, 10, 14],
        "m9" => &[0, 3, 7, 10, 14],
        "maj9" => &[0, 4, 7, 11, 14],
        "5" | "power" => &[0, 7],
        "7sus4" => &[0, 5, 7, 10],
        _ => &[0, 4, 7],
    };
    Some((r, iv.to_vec(), bass.unwrap_or(r)))
}

/// A key's scale, as pitch classes from its tonic: "A minor" is A natural minor.
pub fn scale(key: &str) -> (i32, Vec<i32>) {
    let low = key.to_lowercase();
    let tonic = root(key.trim()).map_or(0, |(pc, _)| pc);
    let minor = low.contains("minor") || low.split_whitespace().next().is_some_and(|w| w.ends_with('m') && !w.ends_with("maj"));
    (tonic, if minor { vec![0, 2, 3, 5, 7, 8, 10] } else { vec![0, 2, 4, 5, 7, 9, 11] })
}

// ---------- the arranger ----------

/// A small, steady random source: the same plan arranges the same way.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

/// General MIDI drums.
const KICK: u8 = 36;
const SNARE: u8 = 38;
const HAT: u8 = 42;
const OPEN_HAT: u8 = 46;
const CRASH: u8 = 49;
const RIDE: u8 = 51;
const TOMS: [u8; 4] = [50, 48, 47, 45];

/// The beats a bar's accents fall on: its groups (7/8 is 2+2+3, 6/8 is 3+3,
/// 5/4 is 3+2, 9/8 3+3+3, 12/8 3+3+3+3; x/4 counts every beat).
fn groups(p: &Plan) -> Vec<f32> {
    let n = p.beats.max(1);
    let sizes: Vec<u32> = match (n, p.unit) {
        (6, 8) => vec![3, 3],
        (9, 8) => vec![3, 3, 3],
        (12, 8) => vec![3, 3, 3, 3],
        (7, 8) => vec![2, 2, 3],
        (5, 8) => vec![3, 2],
        (5, 4) => vec![3, 2],
        (7, 4) => vec![4, 3],
        _ => vec![1; n as usize],
    };
    let mut at = 0.0;
    sizes
        .iter()
        .map(|s| {
            let b = at;
            at += *s as f32;
            b
        })
        .collect()
}

/// The subdivision the hi-hat and arpeggios move in, in beats: eighths
/// for x/4, every beat for x/8.
fn step(p: &Plan) -> f32 {
    if p.unit == 8 {
        1.0
    } else {
        0.5
    }
}

fn drums(p: &Plan, s: &PlanSection, start: f32, next: Option<&PlanSection>, first: bool) -> Vec<Note> {
    let style = s.drums.as_str();
    if style == "none" {
        return Vec::new();
    }
    let bb = p.bar_beats();
    let g = groups(p);
    let sub = step(p);
    let base = 60 + s.energy.min(5) * 10;
    let mut out = Vec::new();
    let mut hit = |beat: f32, pitch: u8, vel: u8| out.push(Note { beat, len: 0.25, pitch, vel: vel.min(127) });
    let fill = next.is_some_and(|n| n.energy >= s.energy && n.drums != "none") && s.bars >= 2;
    for bar in 0..s.bars {
        let b0 = start + bar as f32 * bb;
        let last = bar + 1 == s.bars;
        if bar == 0 && !first && s.energy >= 3 {
            hit(b0, CRASH, base + 10);
        }
        // Accents: the kick on the first group, the snare (or a softer
        // one) on the next; half-time keeps only the middle snare.
        let backbeats: Vec<f32> = if g.len() >= 4 && p.unit == 4 { vec![g[1], g[3]] } else if g.len() >= 2 { vec![g[g.len() / 2]] } else { vec![] };
        let kicks: Vec<f32> = match style {
            "light" => vec![0.0],
            "busy" => {
                let mut k = vec![0.0];
                if bb >= 4.0 {
                    k.extend([1.5, 2.0, 2.75]);
                } else if g.len() > 2 {
                    k.push(g[2]);
                }
                k
            }
            "half-time" => vec![0.0],
            _ => {
                let mut k = vec![0.0];
                if bb >= 4.0 && p.unit == 4 {
                    k.push(2.0);
                    // Every other bar pushes ahead of the beat, so bars don't repeat exactly.
                    if bar % 2 == 1 {
                        k.push(2.5);
                    }
                } else if g.len() >= 3 {
                    k.push(g[2]);
                }
                k
            }
        };
        let snares: Vec<f32> = match style {
            "half-time" => vec![bb / 2.0],
            "light" => Vec::new(),
            _ => backbeats.clone(),
        };
        let fill_from = if fill && last { bb - 2.0f32.min(bb / 2.0) } else { bb };
        for k in kicks.iter().filter(|k| **k < fill_from) {
            hit(b0 + k, KICK, base + 15);
        }
        for sn in snares.iter().filter(|x| **x < fill_from) {
            hit(b0 + sn, SNARE, base + 12);
        }
        // The hats (a ride in the quietest), sixteenths when busy.
        let hat_step = if style == "busy" && p.unit == 4 { 0.25 } else { sub };
        let (cymbal, soft) = if style == "light" { (RIDE, 20) } else { (HAT, 0) };
        let mut t = 0.0;
        while t < fill_from - 1e-3 {
            let on_group = g.iter().any(|x| (x - t).abs() < 1e-3);
            let open = style == "steady" && s.energy >= 4 && (t - (bb - sub)).abs() < 1e-3;
            hit(b0 + t, if open { OPEN_HAT } else { cymbal }, base - soft - if on_group { 0 } else { 15 });
            t += hat_step;
        }
        // A fill into a bigger section: toms and snare down the last beats.
        if fill && last {
            let mut t = fill_from;
            let mut i = 0;
            while t < bb - 1e-3 {
                hit(b0 + t, if i % 2 == 0 { SNARE } else { TOMS[(i / 2) % 4] }, base + 5 + (i as u8) * 3);
                t += 0.5f32.min(sub);
                i += 1;
            }
        }
    }
    out
}

/// The chord of each bar: the section's list, repeated as needed.
fn bar_chords(s: &PlanSection) -> Vec<(i32, Vec<i32>, i32)> {
    let parsed: Vec<_> = s.chords.iter().filter_map(|c| chord(c)).collect();
    let fallback = vec![(0, vec![0, 4, 7], 0)];
    let list = if parsed.is_empty() { &fallback } else { &parsed };
    (0..s.bars as usize).map(|i| list[i % list.len()].clone()).collect()
}

fn bass(p: &Plan, s: &PlanSection, start: f32, rng: &mut Rng) -> Vec<Note> {
    if s.bass == "none" {
        return Vec::new();
    }
    let bb = p.bar_beats();
    let chords = bar_chords(s);
    let low = |pc: i32| (36 + (pc - 36).rem_euclid(12)) as u8; // C2..B2
    let vel = 75 + s.energy.min(5) * 6;
    let g = groups(p);
    let mut out = Vec::new();
    for (bar, (r, iv, b)) in chords.iter().enumerate() {
        let b0 = start + bar as f32 * bb;
        let root = low(*b);
        let fifth = low(r + 7);
        let next_root = chords.get(bar + 1).map_or(root, |c| low(c.2));
        match s.bass.as_str() {
            "pulse" => {
                let st = step(p);
                let mut t = 0.0;
                while t < bb - 1e-3 {
                    out.push(Note { beat: b0 + t, len: st * 0.9, pitch: root, vel: vel - if g.iter().any(|x| (x - t).abs() < 1e-3) { 0 } else { 10 } });
                    t += st;
                }
            }
            "walking" => {
                let third = low(r + iv.get(1).copied().unwrap_or(4));
                let tones = [root, third, fifth];
                for i in 0..bb as usize {
                    let pitch = if i + 1 == bb as usize && bb >= 3.0 {
                        // Approach the next chord a step away.
                        if next_root > root { next_root.saturating_sub(1) } else { next_root + 1 }
                    } else {
                        tones[i % 3]
                    };
                    out.push(Note { beat: b0 + i as f32, len: 0.9, pitch, vel });
                }
            }
            "syncopated" => {
                let pattern: Vec<(f32, u8)> = if bb >= 4.0 { vec![(0.0, root), (1.5, root), (2.0, fifth), (3.5, root)] } else { g.iter().map(|x| (*x, root)).collect() };
                for (t, pitch) in pattern {
                    out.push(Note { beat: b0 + t, len: 0.45, pitch, vel: vel + (rng.below(8) as u8) });
                }
            }
            // Roots: the root on each group's start, held to the next.
            _ => {
                // Every fourth bar walks up (or down) into the next chord on its last beat.
                let walk = bar % 4 == 3 && bb >= 3.0 && s.energy > 2 && next_root != root;
                let stop = if walk { bb - 1.0 } else { bb };
                for (i, t) in g.iter().enumerate() {
                    if i > 0 && s.energy <= 2 || *t >= stop - 1e-3 {
                        break;
                    }
                    let end = g.get(i + 1).copied().unwrap_or(bb).min(stop);
                    let span = if s.energy <= 2 { bb } else { end - t };
                    out.push(Note { beat: b0 + t, len: span * 0.95, pitch: if i % 2 == 1 && g.len() > 2 { fifth } else { root }, vel });
                }
                if walk {
                    let pitch = if next_root > root { next_root - 1 } else { next_root + 1 };
                    out.push(Note { beat: b0 + bb - 1.0, len: 0.9, pitch, vel });
                }
            }
        }
    }
    out
}

/// A chord's notes around middle C, as close as can be to the last chord's.
fn voicing(r: i32, iv: &[i32], last: &[u8]) -> Vec<u8> {
    let tones: Vec<i32> = iv.iter().map(|i| (r + i).rem_euclid(12)).collect();
    let mut best: Option<(i32, Vec<u8>)> = None;
    for inv in 0..tones.len() {
        let mut notes = Vec::new();
        let mut prev = 52; // from E3 up
        for k in 0..tones.len() {
            let pc = tones[(inv + k) % tones.len()];
            let mut n = prev + (pc - prev).rem_euclid(12);
            if n == prev && k > 0 {
                n += 12;
            }
            notes.push(n);
            prev = n;
        }
        let cost: i32 = if last.is_empty() { (notes[0] - 57).abs() } else { notes.iter().zip(last).map(|(a, b)| (a - *b as i32).abs()).sum() };
        if best.as_ref().is_none_or(|(c, _)| cost < *c) {
            best = Some((cost, notes.iter().map(|n| *n as u8).collect()));
        }
    }
    best.map(|b| b.1).unwrap_or_default()
}

fn keys(p: &Plan, s: &PlanSection, start: f32, last: &mut Vec<u8>) -> Vec<Note> {
    if s.keys == "none" {
        return Vec::new();
    }
    let bb = p.bar_beats();
    let vel = 55 + s.energy.min(5) * 6;
    let g = groups(p);
    let mut out = Vec::new();
    for (bar, (r, iv, _)) in bar_chords(s).iter().enumerate() {
        let b0 = start + bar as f32 * bb;
        let v = voicing(*r, iv, last);
        *last = v.clone();
        match s.keys.as_str() {
            "arpeggio" => {
                let st = step(p);
                let mut t = 0.0;
                let mut i = 0;
                let up: Vec<u8> = v.iter().copied().chain(v.iter().rev().skip(1).take(v.len().saturating_sub(2)).copied()).collect();
                while t < bb - 1e-3 {
                    out.push(Note { beat: b0 + t, len: st * 1.6, pitch: up[i % up.len()], vel: vel - 5 });
                    t += st;
                    i += 1;
                }
            }
            "strummed" => {
                let hits: Vec<f32> = if bb >= 4.0 && p.unit == 4 { vec![0.0, 1.5, 2.0, 3.0, 3.5] } else { g.clone() };
                for (h, t) in hits.iter().enumerate() {
                    let len = hits.get(h + 1).copied().unwrap_or(bb) - t;
                    for (k, n) in v.iter().enumerate() {
                        out.push(Note { beat: b0 + t + k as f32 * 0.02, len: (len - 0.05).max(0.2), pitch: *n, vel: vel - if h == 0 { 0 } else { 8 } });
                    }
                }
            }
            "stabs" => {
                let offs: Vec<f32> = if bb >= 4.0 && p.unit == 4 { vec![0.5, 1.5, 2.5, 3.5] } else { g.iter().map(|x| x + step(p)).filter(|x| *x < bb).collect() };
                for t in offs {
                    for n in &v {
                        out.push(Note { beat: b0 + t, len: 0.3, pitch: *n, vel });
                    }
                }
            }
            // Held: the whole bar.
            _ => {
                for n in &v {
                    out.push(Note { beat: b0, len: bb * 0.98, pitch: *n, vel });
                }
            }
        }
    }
    out
}

/// Syllables in a sung line (vowel groups; a silent final e left out).
pub fn syllables(line: &str) -> usize {
    line.split(|c: char| !c.is_alphabetic() && c != '\'')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let w = w.to_lowercase();
            let chars: Vec<char> = w.chars().collect();
            let vowel = |c: char| "aeiouy".contains(c);
            let mut n = 0;
            let mut prev = false;
            for c in &chars {
                let v = vowel(*c);
                if v && !prev {
                    n += 1;
                }
                prev = v;
            }
            // A silent final e ("love", "smile"), not "-le" or "-ee" ("candle", "coffee").
            let back = |k: usize| chars.len().checked_sub(k).map(|i| chars[i]);
            let consonant_le = w.ends_with("le") && back(3).is_some_and(|c| !vowel(c));
            if w.ends_with('e') && !consonant_le && back(2).is_some_and(|c| !vowel(c)) && n > 1 {
                n -= 1;
            }
            n.max(1)
        })
        .sum()
}

/// The notes a singer sings (MIDI): a man C3-E4, a woman C4-E5, and for a
/// duet, a choir or no one named, between them.
pub fn voice_range(voice: &str) -> (i32, i32) {
    match voice.trim().to_lowercase().as_str() {
        "male" | "man" | "rap" => (48, 64),
        "female" | "woman" => (60, 76),
        _ => (55, 72),
    }
}

/// A pitch `steps` notes of the scale away (a pitch off the scale starts
/// from the nearest one on it).
fn scale_step(tonic: i32, sc: &[i32], pitch: i32, steps: i32) -> i32 {
    let on = |n: i32| sc.contains(&(n - tonic).rem_euclid(12));
    let mut p = (0..12).flat_map(|d| [pitch - d, pitch + d]).find(|n| on(*n)).unwrap_or(pitch);
    for _ in 0..steps.abs() {
        p += steps.signum();
        while !on(p) {
            p += steps.signum();
        }
    }
    p
}

/// The melody, written the way songs repeat themselves. Sung lines share
/// one rhythm and one shape a section (each pair's second line answers the
/// first and comes home to the key's root), a note a syllable at about the
/// speed people sing, chord tones on the strong beats, the last syllable held
/// a bar at most. Without words, the lead plays a riff (`riff`).
fn melody(p: &Plan, s: &PlanSection, start: f32, rng: &mut Rng) -> Vec<Note> {
    if !s.melody {
        return Vec::new();
    }
    if s.lines.is_empty() {
        return riff(p, s, start, rng);
    }
    let bb = p.bar_beats();
    // Seconds a beat lasts.
    let spb = 60.0 / p.bpm.max(20) as f32 * 4.0 / p.unit.max(1) as f32;
    let (tonic, sc) = scale(&p.key);
    let chords = bar_chords(s);
    // The singer's range: made real, the music model follows the tune's
    // pitch, and a man's song written up at C4-E5 came back sung by a woman.
    let (lo, hi) = voice_range(&p.voice);
    let vel = 80 + s.energy.min(5) * 5;
    let nearest = |target: i32, ok: &dyn Fn(i32) -> bool| (0..12).flat_map(|d| [target - d, target + d]).find(|n| ok(*n) && (lo..=hi).contains(n)).unwrap_or(target.clamp(lo, hi));
    let lines: Vec<usize> = s.lines.iter().map(|l| syllables(l).max(1)).collect();
    let bars_per = (s.bars as usize / lines.len()).max(1);
    // One rhythm for the section's lines, at about the speed people sing
    // (three syllables a second), packed closer when a line has little room.
    let room = bars_per as f32 * bb - step(p) * 2.0;
    let most = *lines.iter().max().unwrap_or(&1);
    let fit = (room / most as f32).max(0.25);
    let natural = 0.33 / spb;
    let choices: &[f32] = if p.unit == 8 { &[0.5, 1.0, 2.0, 3.0] } else { &[0.25, 0.5, 1.0, 2.0] };
    let gap = choices.iter().copied().filter(|g| *g <= fit + 1e-3).min_by(|a, b| (a - natural).abs().total_cmp(&(b - natural).abs())).unwrap_or(choices[0]);
    // Some syllables held twice as long, where there's room for them.
    let held: Vec<bool> = (0..32).map(|_| rng.below(4) == 0).collect();
    // One shape: up toward the middle of the line, down after it.
    let shape: Vec<i32> = (0..32).map(|k| if k < 5 { [1, 1, 2, 0, -1][rng.below(5) as usize] } else { [-1, -1, 0, -2, 1][rng.below(5) as usize] }).collect();
    // The key's root near the bottom of the range.
    let home = {
        let h = lo + (tonic - lo).rem_euclid(12);
        if h > lo + 6 {
            h - 12
        } else {
            h
        }
    };
    let first = scale_step(tonic, &sc, home, [2, 4, 4, 7][rng.below(4) as usize]);
    let mut out = Vec::new();
    for (li, count) in lines.iter().copied().enumerate() {
        let bar0 = li * bars_per;
        if bar0 >= s.bars as usize {
            break;
        }
        let span = bars_per.min(s.bars as usize - bar0) as f32 * bb;
        let line_start = start + bar0 as f32 * bb;
        let long = |k: usize| held[k % held.len()] && k + 1 < count;
        let units: f32 = (0..count.saturating_sub(1)).map(|k| if long(k) { 2.0 } else { 1.0 }).sum();
        let lengthen = units * gap <= room;
        let answer = li % 2 == 1;
        let mut t = 0.0;
        let mut pitch = first;
        for k in 0..count {
            let bar = bar0 + (t / bb) as usize;
            let (r, iv, _) = &chords[bar.min(chords.len() - 1)];
            let strong = (t % bb).abs() < 1e-3 || groups(p).iter().any(|x| ((t % bb) - x).abs() < 1e-3);
            if k > 0 {
                pitch = scale_step(tonic, &sc, pitch, shape[(k - 1) % shape.len()]);
            }
            let last = k + 1 == count;
            let chord_tone = |n: i32| iv.iter().any(|i| (r + i - n).rem_euclid(12) == 0);
            let note = if last {
                // A call ends open (on the fifth or second), its answer at home.
                let end = |n: i32| (n - tonic).rem_euclid(12) == if answer { 0 } else { sc[4] };
                nearest(pitch, &end)
            } else if strong {
                nearest(pitch, &chord_tone)
            } else {
                nearest(pitch, &|n| sc.contains(&(n - tonic).rem_euclid(12)))
            };
            pitch = note;
            let dur = if lengthen && long(k) { gap * 2.0 } else { gap };
            // The last syllable held, a bar at most.
            let len = if last { (span - t - step(p)).min(bb).max(gap) } else { dur * 0.9 };
            out.push(Note { beat: line_start + t, len, pitch: note as u8, vel: vel - if strong { 0 } else { 8 } });
            t += dur;
        }
    }
    out
}

/// A lead instrument's riff: a bar-long figure on each bar's chord, played
/// three bars running and changed on the fourth (A A A B), the same all
/// through the section.
fn riff(p: &Plan, s: &PlanSection, start: f32, rng: &mut Rng) -> Vec<Note> {
    let bb = p.bar_beats();
    let sub = step(p);
    let slots = (bb / sub).round().max(1.0) as usize;
    let g = groups(p);
    let (tonic, sc) = scale(&p.key);
    let vel = 78 + s.energy.min(5) * 6;
    // A figure: where its notes fall in the bar, and how far each moves along the scale.
    let mut figure = || -> Vec<(usize, i32)> {
        let mut f = vec![(0, 0)];
        for k in 1..slots {
            let on_group = g.iter().any(|x| (x / sub - k as f32).abs() < 1e-3);
            if rng.below(100) < if on_group { 70 } else { 40 } {
                f.push((k, [1, 2, -1, 1, 2, -2, 0, 3][rng.below(8) as usize]));
            }
        }
        if f.len() < 3 {
            f.push((slots / 2, 2));
        }
        f.sort_by_key(|x| x.0);
        f.dedup_by_key(|x| x.0);
        f
    };
    let (a, b) = (figure(), figure());
    let chords = bar_chords(s);
    let mut out = Vec::new();
    for (bar, (r, _, _)) in chords.iter().enumerate() {
        let fig = if bar % 4 == 3 || bar + 1 == chords.len() && chords.len() > 2 { &b } else { &a };
        // The chord's root in the riff's range.
        let mut pitch = 57 + (r - 57).rem_euclid(12);
        let b0 = start + bar as f32 * bb;
        for (i, (slot, steps)) in fig.iter().enumerate() {
            if i > 0 {
                pitch = scale_step(tonic, &sc, pitch, *steps).clamp(52, 79);
            }
            let next = fig.get(i + 1).map_or(slots, |x| x.0);
            let len = (next - slot) as f32 * sub * 0.9;
            out.push(Note { beat: b0 + *slot as f32 * sub, len, pitch: pitch as u8, vel: vel - if *slot == 0 { 0 } else { 10 } });
        }
    }
    out
}

/// The parts of a plan: every section's notes, in place on the bar grid.
pub fn arrange(p: &Plan) -> Vec<Part> {
    let (starts, _) = p.starts();
    let (mut d, mut b, mut c, mut m) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut last_voicing = Vec::new();
    for (i, s) in p.sections.iter().enumerate() {
        let mut rng = Rng(p.seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(i as u64 * 7919 + s.take * 104_729 + 1) | 1);
        d.extend(drums(p, s, starts[i], p.sections.get(i + 1), i == 0));
        b.extend(bass(p, s, starts[i], &mut rng));
        c.extend(keys(p, s, starts[i], &mut last_voicing));
        m.extend(melody(p, s, starts[i], &mut rng));
    }
    let ins = &p.instruments;
    vec![
        Part { name: "melody".into(), instrument: ins.melody.clone(), notes: m },
        Part { name: "chords".into(), instrument: ins.chords.clone(), notes: c },
        Part { name: "bass".into(), instrument: ins.bass.clone(), notes: b },
        Part { name: "drums".into(), instrument: ins.drums.clone(), notes: d },
    ]
}

/// Where a section lies, in beats.
fn span(p: &Plan, index: usize) -> (f32, f32) {
    let (starts, end) = p.starts();
    let a = starts.get(index).copied().unwrap_or(end);
    (a, starts.get(index + 1).copied().unwrap_or(end))
}

/// The notes from `a` to `b` taken out (each part's), the later ones moved
/// back to close the gap; and what was taken, from `a`.
fn cut(parts: &[Part], a: f32, b: f32) -> (Vec<Part>, Vec<Part>) {
    let inside = |n: &Note| n.beat >= a - 1e-4 && n.beat < b - 1e-4;
    let rest = parts
        .iter()
        .map(|p| Part { notes: p.notes.iter().filter(|n| !inside(n)).map(|n| if n.beat >= b - 1e-4 { Note { beat: n.beat - (b - a), ..*n } } else { *n }).collect(), ..p.clone() })
        .collect();
    let clip = parts.iter().map(|p| Part { notes: p.notes.iter().filter(|n| inside(n)).map(|n| Note { beat: n.beat - a, ..*n }).collect(), ..p.clone() }).collect();
    (rest, clip)
}

/// Room made at `at`: the notes from there on moved later by `len`, and
/// `clip` (notes from 0) put in it.
fn paste(parts: &[Part], at: f32, len: f32, clip: &[Part]) -> Vec<Part> {
    parts
        .iter()
        .map(|p| {
            let mut notes: Vec<Note> = p.notes.iter().map(|n| if n.beat >= at - 1e-4 { Note { beat: n.beat + len, ..*n } } else { *n }).collect();
            if let Some(c) = clip.iter().find(|c| c.name == p.name) {
                notes.extend(c.notes.iter().filter(|n| n.beat < len - 1e-4).map(|n| Note { beat: n.beat + at, ..*n }));
            }
            notes.sort_by(|x, y| x.beat.total_cmp(&y.beat));
            Part { notes, ..p.clone() }
        })
        .collect()
}

/// A section's notes as the arranger writes them (from 0), in the song as `plan` has it.
fn fresh(plan: &Plan, index: usize) -> Vec<Part> {
    let (a, b) = span(plan, index);
    cut(&arrange(plan), a, b).1
}

/// What can be done to a section.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum SectionOp {
    /// Its plan changed (by the person or the chat model): its notes are written again.
    Set { section: PlanSection },
    /// The same plan, new notes (a new take on its melody).
    Again,
    /// A new section after it.
    Add { section: PlanSection },
    /// A copy after it, notes as they are (edits included).
    Duplicate,
    Delete,
}

/// A section changed, added, copied or taken out, every other note kept as
/// it is (edited ones included) and moved with its section.
pub fn apply(c: &Composition, index: usize, op: SectionOp) -> Result<Composition, String> {
    if index >= c.plan.sections.len() {
        return Err("That section isn't there.".into());
    }
    let mut plan = c.plan.clone();
    let (a, b) = span(&c.plan, index);
    let parts = match op {
        SectionOp::Set { section } => {
            plan.sections[index] = section;
            plan = tidy(plan);
            let (rest, _) = cut(&c.parts, a, b);
            let (na, nb) = span(&plan, index);
            paste(&rest, na, nb - na, &fresh(&plan, index))
        }
        SectionOp::Again => {
            plan.sections[index].take += 1;
            let (rest, _) = cut(&c.parts, a, b);
            paste(&rest, a, b - a, &fresh(&plan, index))
        }
        SectionOp::Add { section } => {
            plan.sections.insert(index + 1, section);
            plan = tidy(plan);
            let (na, nb) = span(&plan, index + 1);
            paste(&c.parts, na, nb - na, &fresh(&plan, index + 1))
        }
        SectionOp::Duplicate => {
            let copy = plan.sections[index].clone();
            plan.sections.insert(index + 1, copy);
            let (_, clip) = cut(&c.parts, a, b);
            paste(&c.parts, b, b - a, &clip)
        }
        SectionOp::Delete => {
            if plan.sections.len() == 1 {
                return Err("A song needs at least one section.".into());
            }
            plan.sections.remove(index);
            cut(&c.parts, a, b).0
        }
    };
    Ok(Composition { plan, parts })
}

// ---------- instruments and MIDI ----------

/// The General MIDI program for an instrument's name (melodic parts), and
/// the drum kit's (channel 10).
pub fn program(name: &str, drums: bool) -> u8 {
    let n = name.to_lowercase();
    if drums {
        return match n.as_str() {
            s if s.contains("room") => 8,
            s if s.contains("power") || s.contains("rock") => 16,
            s if s.contains("808") => 25,
            s if s.contains("electronic") => 24,
            s if s.contains("jazz") => 32,
            s if s.contains("brush") => 40,
            s if s.contains("orchestra") => 48,
            _ => 0,
        };
    }
    const LIST: &[(&str, u8)] = &[
        ("electric piano", 4),
        ("rhodes", 4),
        ("piano", 0),
        ("organ", 16),
        ("nylon", 24),
        ("acoustic guitar", 25),
        ("clean guitar", 27),
        ("distorted", 30),
        ("electric guitar", 27),
        ("guitar", 25),
        ("acoustic bass", 32),
        ("fretless", 35),
        ("synth bass", 38),
        ("bass", 33),
        ("violin", 40),
        ("cello", 42),
        ("strings", 48),
        ("choir", 52),
        ("voice", 52),
        ("trumpet", 56),
        ("brass", 61),
        ("sax", 65),
        ("flute", 73),
        ("synth lead", 80),
        ("lead", 80),
        ("pad", 89),
        ("synth", 81),
        ("bells", 14),
        ("marimba", 12),
    ];
    LIST.iter().find(|(k, _)| n.contains(k)).map_or(0, |(_, p)| *p)
}

/// The composition as a standard MIDI file (format 1): its tempo and time
/// signature, a track a part with its instrument, the drums on channel 10.
pub fn midi(c: &Composition) -> Vec<u8> {
    const TPQ: u16 = 480;
    let p = &c.plan;
    // A beat is the time signature's unit: an eighth is half a quarter.
    let per_beat = if p.unit == 8 { TPQ as f32 / 2.0 } else { TPQ as f32 * 4.0 / p.unit.max(1) as f32 };
    let ticks = |beats: f32| (beats.max(0.0) * per_beat).round() as u32;
    let vlq = |mut v: u32, out: &mut Vec<u8>| {
        let mut bytes = vec![(v & 0x7F) as u8];
        v >>= 7;
        while v > 0 {
            bytes.push(((v & 0x7F) as u8) | 0x80);
            v >>= 7;
        }
        out.extend(bytes.iter().rev());
    };
    let mut file = Vec::new();
    file.extend_from_slice(b"MThd");
    file.extend_from_slice(&6u32.to_be_bytes());
    file.extend_from_slice(&1u16.to_be_bytes());
    file.extend_from_slice(&(c.parts.len() as u16 + 1).to_be_bytes());
    file.extend_from_slice(&TPQ.to_be_bytes());
    let mut track = |events: Vec<u8>| {
        file.extend_from_slice(b"MTrk");
        file.extend_from_slice(&(events.len() as u32).to_be_bytes());
        file.extend_from_slice(&events);
    };
    // Tempo (microseconds a quarter note) and time signature.
    let quarter_bpm = p.bpm.max(20) as f32;
    let us = (60_000_000.0 / quarter_bpm).round() as u32;
    let mut head = vec![0x00, 0xFF, 0x51, 0x03, (us >> 16) as u8, (us >> 8) as u8, us as u8];
    let den = match p.unit {
        2 => 1,
        8 => 3,
        16 => 4,
        _ => 2,
    };
    head.extend_from_slice(&[0x00, 0xFF, 0x58, 0x04, p.beats.clamp(1, 32) as u8, den, 24, 8]);
    let name = p.title.as_bytes();
    head.extend_from_slice(&[0x00, 0xFF, 0x03]);
    vlq(name.len() as u32, &mut head);
    head.extend_from_slice(name);
    head.extend_from_slice(&[0x00, 0xFF, 0x2F, 0x00]);
    track(head);
    let mut channel = 0u8;
    for part in &c.parts {
        let drums = part.name == "drums";
        let ch = if drums {
            9
        } else {
            let x = channel;
            channel = if channel == 8 { 10 } else { (channel + 1) % 16 };
            x
        };
        let mut bytes = Vec::new();
        let label = format!("{} ({})", part.name, part.instrument);
        bytes.extend_from_slice(&[0x00, 0xFF, 0x03]);
        vlq(label.len() as u32, &mut bytes);
        bytes.extend_from_slice(label.as_bytes());
        bytes.extend_from_slice(&[0x00, 0xC0 | ch, program(&part.instrument, drums)]);
        let mut events: Vec<(u32, bool, u8, u8)> = Vec::new();
        for n in &part.notes {
            events.push((ticks(n.beat), true, n.pitch.min(127), n.vel.clamp(1, 127)));
            events.push((ticks(n.beat + n.len.max(0.05)), false, n.pitch.min(127), 0));
        }
        // Note-offs before note-ons at the same tick.
        events.sort_by_key(|e| (e.0, e.1));
        let mut now = 0;
        for (t, on, pitch, vel) in events {
            vlq(t - now, &mut bytes);
            bytes.extend_from_slice(&[if on { 0x90 } else { 0x80 } | ch, pitch, vel]);
            now = t;
        }
        bytes.extend_from_slice(&[0x00, 0xFF, 0x2F, 0x00]);
        track(bytes);
    }
    file
}

// ---------- the chat model's plan ----------

/// The plan's form for the chat model (a JSON schema it's held to).
fn plan_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "title": { "type": "string" },
            "bpm": { "type": "integer", "minimum": 40, "maximum": 220 },
            "key": { "type": "string" },
            "style": { "type": "string" },
            "instruments": { "type": "object", "properties": {
                "chords": { "type": "string" }, "melody": { "type": "string" }, "bass": { "type": "string" }, "drums": { "type": "string" } },
                "required": ["chords", "melody", "bass", "drums"] },
            "sections": { "type": "array", "minItems": 1, "maxItems": 14, "items": { "type": "object", "properties": {
                "name": { "type": "string" },
                "bars": { "type": "integer", "minimum": 1, "maximum": 32 },
                "chords": { "type": "array", "items": { "type": "string" }, "minItems": 1, "maxItems": 32 },
                "drums": { "enum": ["none", "light", "steady", "busy", "half-time"] },
                "bass": { "enum": ["none", "roots", "pulse", "walking", "syncopated"] },
                "keys": { "enum": ["none", "held", "arpeggio", "strummed", "stabs"] },
                "melody": { "type": "boolean" },
                "energy": { "type": "integer", "minimum": 1, "maximum": 5 },
                "lines": { "type": "array", "items": { "type": "string" }, "maxItems": 8 } },
                "required": ["name", "bars", "chords", "drums", "bass", "keys", "melody", "energy", "lines"] } }
        },
        "required": ["title", "bpm", "key", "style", "instruments", "sections"]
    })
}

const GUIDE: &str = "You arrange songs the way a producer plans them in a music program. Answer with the plan as JSON only.\n\
- Sections in order (Intro, Verse, Pre-chorus, Chorus, Bridge, Solo, Outro), each with its bars (usually 4 or 8) and a chord a bar that fits the key.\n\
- drums: none, light, steady, busy or half-time. bass: none, roots, pulse, walking or syncopated. keys (the chords part): none, held, arpeggio, strummed or stabs.\n\
- energy 1 to 5: quieter verses, bigger choruses, a calm intro and outro.\n\
- melody true where the tune plays. lines: the words sung in that section, a line each, about one line every two bars; none in instrumental sections.\n\
- instruments in plain words: chords (piano, electric piano, organ, acoustic guitar, electric guitar, strings, pad), melody (voice, synth lead, flute, violin, trumpet, sax), bass (electric bass, acoustic bass, synth bass), drums (standard kit, jazz kit, brush kit, electronic kit, 808 kit).";

/// The JSON in a reply (a cloud model may wrap it in words or fences).
fn json_of(raw: &str) -> &str {
    match (raw.find('{'), raw.rfind('}')) {
        (Some(a), Some(b)) if b > a => &raw[a..=b],
        _ => raw,
    }
}

/// The plan from the chat model, held to `plan_schema`: the description,
/// and what the person set (tempo, time signature, key, style, length).
pub async fn write_plan(state: &std::sync::Arc<crate::AppState>, ask: &Ask) -> Result<Plan, String> {
    let (ep, spec, _hold) = crate::background_endpoint(state).await.map_err(|e| format!("Writing the arrangement needs a chat model: {e}"))?;
    let mut wants = format!("Plan this song: {}.", ask.description.trim().trim_end_matches('.'));
    wants.push_str(&format!(" Time signature {}/{}.", ask.beats, ask.unit));
    if let Some(b) = ask.bpm {
        wants.push_str(&format!(" Tempo {b} beats a minute."));
    }
    if let Some(k) = ask.key.as_deref().filter(|k| !k.is_empty()) {
        wants.push_str(&format!(" Key {k}."));
    }
    if let Some(s) = ask.style.as_deref().filter(|s| !s.is_empty()) {
        wants.push_str(&format!(" Style {s}."));
    }
    if !ask.era.trim().is_empty() {
        let e = ask.era.trim();
        wants.push_str(&format!(" A song from the {e}: arrange it the way songs were then, with that era's chords, tempo, instruments and drum kit, and words written the way they were then."));
    }
    // The sound picked in the Music tab, as the music model will be asked for it.
    if let Some(sound) = ask.song.as_ref().map(|s| s.caption("", !ask.vocals)).filter(|s| !s.trim().is_empty()) {
        wants.push_str(&format!(" The sound: {}.", sound.trim_end_matches('.')));
    }
    // Their own lyrics, or a structure they picked: those sections, in that order.
    let own: Vec<(String, Vec<String>)> = ask.lyrics.as_deref().filter(|_| ask.vocals).map(lyric_blocks).unwrap_or_default();
    let given: Vec<(String, Vec<String>)> = if !own.is_empty() {
        own.clone()
    } else {
        ask.song.as_ref().map(|s| s.sections.iter().take(14).map(|n| (n.clone(), Vec::new())).collect()).unwrap_or_default()
    };
    let mut schema = plan_schema();
    if given.is_empty() {
        wants.push_str(&format!(" About {} sections.", ask.sections.clamp(2, 12)));
    } else {
        let list: Vec<String> = given
            .iter()
            .map(|(n, l)| if !own.is_empty() && l.is_empty() { format!("{n} (played, no words)") } else if l.is_empty() { n.clone() } else { format!("{n} ({} lines of their words)", l.len()) })
            .collect();
        wants.push_str(&format!(" Use exactly these sections, in this order: {}.", list.join(", ")));
        if !own.is_empty() {
            wants.push_str(" The words are the user's own and are put in afterwards: give each section enough bars to sing its lines (about two bars a line).");
        }
        schema["properties"]["sections"]["minItems"] = json!(given.len());
        schema["properties"]["sections"]["maxItems"] = json!(given.len());
    }
    if !ask.vocals {
        wants.push_str(" Instrumental: no lines anywhere; a lead instrument (not a voice) plays the tune, melody true in most sections.");
    }
    let msgs = vec![json!({ "role": "system", "content": GUIDE }), json!({ "role": "user", "content": wants })];
    let mut last = String::new();
    for temperature in [0.7, 0.4] {
        let raw = crate::chat::complete(&ep, msgs.clone(), json!({ "temperature": temperature, "response_format": { "type": "json_schema", "json_schema": { "name": "plan", "schema": schema } } }), 3000).await?;
        match serde_json::from_str::<Plan>(json_of(&raw)) {
            Ok(mut p) if !p.sections.is_empty() => {
                p.song = ask.song.clone();
                p.language = ask.language.clone().filter(|l| !l.is_empty() && l != "auto");
                // Their sections, named as they are, with their words.
                if !given.is_empty() {
                    let filler = p.sections.last().cloned().unwrap_or_default();
                    p.sections.resize(given.len(), filler);
                    for (s, (name, lines)) in p.sections.iter_mut().zip(&given) {
                        s.name = name.clone();
                        if !own.is_empty() {
                            s.lines = lines.clone();
                            s.melody = s.melody || !lines.is_empty();
                        }
                    }
                }
                p.beats = ask.beats;
                p.unit = ask.unit;
                if let Some(b) = ask.bpm {
                    p.bpm = b;
                }
                if let Some(k) = ask.key.clone().filter(|k| !k.is_empty()) {
                    p.key = k;
                }
                p.seed = ask.seed;
                p.description = ask.description.trim().to_string();
                p.voice = ask.voice.clone();
                p.era = ask.era.trim().to_string();
                if !ask.vocals {
                    p.sections.iter_mut().for_each(|s| s.lines.clear());
                    return Ok(tidy(p));
                }
                let mut p = tidy(p);
                // Too few words to sing fill the song with band (a 40-second
                // verse got two lines, and the song came out "mostly
                // instrumental"): the words are written again to fit.
                let want = wanted_lines(&p);
                let have: usize = want.iter().map(|(i, _)| p.sections[*i].lines.len()).sum();
                let need: usize = want.iter().map(|(_, n)| n).sum();
                // Their own lyrics are sung as they wrote them.
                if own.is_empty() && (have as f32) < need as f32 * 0.75 {
                    match fill_lines(&ep, &p, &want).await {
                        Ok(lines) => {
                            for ((i, _), l) in want.iter().zip(lines) {
                                p.sections[*i].lines = l;
                            }
                        }
                        Err(e) => crate::applog::line("media", &format!("Compose: the words couldn't be filled in ({e}); {have} lines of {need} kept")),
                    }
                }
                return Ok(p);
            }
            Ok(_) => last = format!("{} planned no sections", spec.name),
            Err(e) => last = format!("{} wrote a plan that couldn't be read ({e})", spec.name),
        }
    }
    Err(last)
}

/// The sections that are sung, and how many lines each needs for its length
/// (a line every `lyrics::SECS_PER_LINE`, as songs are heard best; in
/// pairs, so they rhyme): melody sections other than the band's own (intro,
/// outro, solo, instrumental), and any the plan already gave words.
pub fn wanted_lines(p: &Plan) -> Vec<(usize, usize)> {
    let spb = 60.0 / p.bpm.max(20) as f32 * 4.0 / p.unit.max(1) as f32;
    let played = |name: &str| {
        let n = name.to_lowercase();
        ["intro", "outro", "solo", "instrumental", "interlude", "break"].iter().any(|w| n.contains(w))
    };
    p.sections
        .iter()
        .enumerate()
        .filter(|(_, s)| s.melody && (!s.lines.is_empty() || !played(&s.name)))
        .map(|(i, s)| {
            let secs = s.bars as f32 * p.bar_beats() * spb;
            let n = (secs / super::lyrics::SECS_PER_LINE / 2.0).round().max(1.0) as usize * 2;
            (i, n.min(12))
        })
        .collect()
}

/// The song's words written again to fill its sung sections (`wanted_lines`),
/// a JSON list a section held to its exact count, keeping what was written
/// where it fits.
async fn fill_lines(ep: &crate::engine::Endpoint, p: &Plan, want: &[(usize, usize)]) -> Result<Vec<Vec<String>>, String> {
    let mut props = serde_json::Map::new();
    let mut order = Vec::new();
    let mut shape = Vec::new();
    for (k, (i, n)) in want.iter().enumerate() {
        let s = &p.sections[*i];
        let key = format!("s{k}");
        props.insert(key.clone(), json!({ "type": "array", "items": { "type": "string" }, "minItems": n, "maxItems": n }));
        order.push(key.clone());
        let now = if s.lines.is_empty() { String::new() } else { format!(" (written so far: {})", s.lines.join(" / ")) };
        shape.push(format!("{key}: {} ({} bars), {n} lines{now}", s.name, s.bars));
    }
    let schema = json!({ "type": "object", "properties": props, "required": order });
    let era = if p.era.is_empty() { String::new() } else { format!(" It's a song from the {}: words written the way songs were then.", p.era) };
    let ask = format!(
        "Write the words for this song: {}. Title \"{}\", style {}.{era}\nEach sung section in order, with exactly this many lines:\n{}\n\
         Four to eight words a line, lines rhyming in pairs, a chorus with a hook that comes back the same each time, verses that tell the story. \
         Keep the lines already written where they fit. Answer with JSON only: each key a list of that section's lines.",
        p.description,
        p.title,
        p.style,
        shape.join("\n")
    );
    let msgs = vec![json!({ "role": "user", "content": ask })];
    let raw = crate::chat::complete(ep, msgs, json!({ "temperature": 0.7, "response_format": { "type": "json_schema", "json_schema": { "name": "words", "schema": schema } } }), 3000).await?;
    let v: Value = serde_json::from_str(json_of(&raw)).map_err(|e| format!("the words couldn't be read ({e})"))?;
    order
        .iter()
        .zip(want)
        .map(|(key, (_, n))| {
            let lines: Vec<String> = v[key].as_array().map(|a| a.iter().filter_map(Value::as_str).map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()).unwrap_or_default();
            if lines.len() + 1 >= *n {
                Ok(lines)
            } else {
                Err(format!("{key} came back with {} of {n} lines", lines.len()))
            }
        })
        .collect()
}

/// A changed section from the chat model, from the person's words ("a
/// busier bass", "end on a held chord"); the rest of the plan stays.
pub async fn edit_section(state: &std::sync::Arc<crate::AppState>, plan: &Plan, index: usize, change: &str) -> Result<PlanSection, String> {
    let section = plan.sections.get(index).ok_or("That section isn't there.")?;
    let (ep, _, _hold) = crate::background_endpoint(state).await.map_err(|e| format!("Changing the arrangement needs a chat model: {e}"))?;
    let schema = &plan_schema()["properties"]["sections"]["items"];
    let ask = format!(
        "The song: {} in {} at {} bpm, {}/{}, style {}. Change its {} section as asked: {}.\nThe section now: {}\nAnswer with the changed section as JSON only (the same fields).",
        plan.title,
        plan.key,
        plan.bpm,
        plan.beats,
        plan.unit,
        plan.style,
        section.name,
        change.trim(),
        serde_json::to_string(section).unwrap_or_default()
    );
    let msgs = vec![json!({ "role": "system", "content": GUIDE }), json!({ "role": "user", "content": ask })];
    let raw = crate::chat::complete(&ep, msgs, json!({ "temperature": 0.5, "response_format": { "type": "json_schema", "json_schema": { "name": "section", "schema": schema } } }), 1500).await?;
    let mut changed: PlanSection = serde_json::from_str(json_of(&raw)).map_err(|e| format!("The changed section couldn't be read ({e})"))?;
    changed.take = section.take;
    Ok(changed)
}

/// What a person asks Compose for.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Ask {
    pub description: String,
    pub beats: u32,
    pub unit: u32,
    pub bpm: Option<u32>,
    pub key: Option<String>,
    pub style: Option<String>,
    /// How many sections, roughly.
    pub sections: u32,
    pub vocals: bool,
    /// Who sings when it's made real ("female", "male"…; "" as described).
    pub voice: String,
    /// The decade it's from ("1980s"; "" any).
    pub era: String,
    pub seed: u64,
    /// Every option picked in the Music tab, kept for "Make it real" and
    /// told to the planner (the sound it should be arranged for).
    pub song: Option<super::MusicSpec>,
    /// Their own lyrics: the plan is built around their sections and lines.
    pub lyrics: Option<String>,
    pub language: Option<String>,
}

/// Lyrics as sections: each `[Tag]` and the lines under it ("[Chorus -
/// screamed]" is a Chorus); lines before any tag are a Verse.
pub fn lyric_blocks(lyrics: &str) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for line in lyrics.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if line.starts_with('[') && line.ends_with(']') {
            let name = line[1..line.len() - 1].split(" - ").next().unwrap_or("").trim();
            out.push((if name.is_empty() { "Verse".into() } else { name.to_string() }, Vec::new()));
        } else {
            if out.is_empty() {
                out.push(("Verse".into(), Vec::new()));
            }
            out.last_mut().map(|b| b.1.push(line.to_string()));
        }
    }
    out.truncate(14);
    out
}

/// A plan within bounds: known styles, at least a bar a section, a chord
/// list, tempo and time signature that the arranger can play.
pub fn tidy(mut p: Plan) -> Plan {
    p.bpm = p.bpm.clamp(40, 220);
    p.beats = p.beats.clamp(1, 16);
    p.unit = if [2, 4, 8, 16].contains(&p.unit) { p.unit } else { 4 };
    if p.seed == 0 {
        p.seed = 1;
    }
    // The key's mode follows its home chord as the chords play it (a plan in
    // "F# major" whose chords are F#m is in F# minor: the tune would clash).
    let (tonic, _) = scale(&p.key);
    let home: Vec<bool> = p.sections.iter().flat_map(|s| &s.chords).filter_map(|c| chord(c)).filter(|(r, _, _)| *r == tonic).map(|(_, iv, _)| iv.contains(&3)).collect();
    let minor_chords = home.iter().filter(|m| **m).count();
    let minor_key = scale(&p.key).1[2] == 3;
    if !home.is_empty() && (minor_chords * 2 > home.len()) != minor_key {
        let name = p.key.split_whitespace().next().unwrap_or("C").trim_end_matches('m').to_string();
        p.key = format!("{name} {}", if minor_key { "major" } else { "minor" });
    }
    let pick = |v: &str, ok: &[&str], dflt: &str| if ok.contains(&v) { v.to_string() } else { dflt.to_string() };
    for s in &mut p.sections {
        s.bars = s.bars.clamp(1, 64);
        s.energy = s.energy.clamp(1, 5);
        s.drums = pick(&s.drums, &["none", "light", "steady", "busy", "half-time"], "steady");
        s.bass = pick(&s.bass, &["none", "roots", "pulse", "walking", "syncopated"], "roots");
        s.keys = pick(&s.keys, &["none", "held", "arpeggio", "strummed", "stabs"], "held");
        s.chords.retain(|c| chord(c).is_some());
        if s.chords.is_empty() {
            s.chords = vec!["C".into()];
        }
        s.lines.retain(|l| !l.trim().is_empty());
        if s.name.trim().is_empty() {
            s.name = "Verse".into();
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(beats: u32, unit: u32) -> Plan {
        tidy(Plan {
            title: "Test".into(),
            bpm: 100,
            beats,
            unit,
            key: "A minor".into(),
            sections: vec![
                PlanSection { name: "Verse".into(), bars: 4, chords: vec!["Am".into(), "F".into(), "C".into(), "G".into()], energy: 2, lines: vec!["Morning light on the window".into(), "Coffee warm in my hands".into()], ..Default::default() },
                PlanSection { name: "Chorus".into(), bars: 4, chords: vec!["F".into(), "G".into(), "Am".into()], drums: "busy".into(), bass: "pulse".into(), keys: "strummed".into(), energy: 5, ..Default::default() },
            ],
            ..Default::default()
        })
    }

    #[test]
    fn chords_and_keys_are_read() {
        assert_eq!(chord("Am7"), Some((9, vec![0, 3, 7, 10], 9)));
        assert_eq!(chord("C/E"), Some((0, vec![0, 4, 7], 4)));
        assert_eq!(chord("F#m"), Some((6, vec![0, 3, 7], 6)));
        assert_eq!(chord("Bbmaj7").map(|c| c.0), Some(10));
        assert_eq!(chord("xyz"), None);
        assert_eq!(scale("A minor"), (9, vec![0, 2, 3, 5, 7, 8, 10]));
        assert_eq!(scale("Eb major").0, 3);
        assert_eq!(syllables("Morning light on the window"), 7);
        assert_eq!(syllables("Coffee warm in my hands"), 6);
        assert_eq!(syllables("I love your smile, candle"), 6);
        // A key whose home chord is played minor is minor.
        let p = tidy(Plan { key: "F# Major".into(), sections: vec![PlanSection { chords: vec!["F#m".into(), "E".into(), "D".into()], ..Default::default() }], ..Default::default() });
        assert_eq!(p.key, "F# minor");
        let p = tidy(Plan { key: "A minor".into(), sections: vec![PlanSection { chords: vec!["Am".into(), "F".into()], ..Default::default() }], ..Default::default() });
        assert_eq!(p.key, "A minor");
    }

    /// Every note lands inside its section's bars, the drums on the grid, in
    /// 4/4, 3/4, 6/8 and 7/8 alike; the sung lines get a note a syllable.
    #[test]
    fn the_arrangement_is_in_place_in_any_time() {
        for (beats, unit) in [(4, 4), (3, 4), (6, 8), (7, 8), (5, 4)] {
            let p = plan(beats, unit);
            let parts = arrange(&p);
            let (starts, end) = p.starts();
            assert_eq!(end, 8.0 * beats as f32, "{beats}/{unit}");
            for part in &parts {
                assert!(part.notes.iter().all(|n| n.beat >= 0.0 && n.beat < end), "{} in {beats}/{unit}", part.name);
            }
            let drums = &parts.iter().find(|x| x.name == "drums").unwrap().notes;
            assert!(drums.iter().any(|n| n.pitch == KICK && n.beat == 0.0), "a kick on the one");
            assert!(drums.iter().all(|n| (n.beat * 4.0).fract() == 0.0), "on the sixteenth grid");
            // A kick at the start of every bar.
            for bar in 0..8 {
                assert!(drums.iter().any(|n| n.pitch == KICK && n.beat == bar as f32 * beats as f32), "bar {bar} in {beats}/{unit}");
            }
            // The verse's sung notes: one a syllable (7 + 6), all inside it.
            let sung: Vec<&Note> = parts[0].notes.iter().filter(|n| n.beat < starts[1]).collect();
            assert_eq!(sung.len(), 13, "{beats}/{unit}");
            assert!(sung.iter().all(|n| (55..=72).contains(&n.pitch)), "no singer named: between a man's and a woman's range");
            // A man sings it low, a woman high.
            for (voice, range) in [("male", 48..=64), ("female", 60..=76)] {
                let mut q = p.clone();
                q.voice = voice.into();
                let notes = arrange(&q)[0].notes.iter().filter(|n| n.beat < starts[1]).map(|n| n.pitch).collect::<Vec<_>>();
                assert!(notes.iter().all(|n| range.contains(n)), "{voice} in {beats}/{unit}: {notes:?}");
            }
            assert!(sung.iter().all(|n| n.len <= beats as f32), "a held syllable lasts a bar at most");
        }
    }

    /// Changing, adding, copying or taking out a section leaves every other
    /// note as it was (edits too), moved with its section.
    #[test]
    fn sections_change_and_the_rest_stays_in_place() {
        let p = plan(4, 4);
        let mut c = Composition { plan: p.clone(), parts: arrange(&p) };
        let count = |c: &Composition, i: usize| c.parts[i].notes.len();
        // An edit in the verse and one in the chorus.
        c.parts[1].notes.push(Note { beat: 1.0, len: 1.0, pitch: 90, vel: 100 });
        c.parts[1].notes.push(Note { beat: 17.0, len: 1.0, pitch: 91, vel: 100 });
        let at = |c: &Composition, pitch: u8| c.parts[1].notes.iter().find(|n| n.pitch == pitch).map(|n| n.beat);

        // The chorus's bass walks, now over eight bars: the verse's edit stays.
        let mut chorus = c.plan.sections[1].clone();
        chorus.bass = "walking".into();
        chorus.bars = 8;
        let set = apply(&c, 1, SectionOp::Set { section: chorus }).unwrap();
        assert_eq!(at(&set, 90), Some(1.0));
        assert_eq!(at(&set, 91), None, "the chorus is written again");
        assert_eq!(set.parts[2].notes.iter().filter(|n| n.beat >= 16.0).count(), 32, "walking: a note a beat for eight bars");
        assert_eq!(set.plan.starts().1, 48.0);

        // A new take on the verse: its melody changes, the chorus stays.
        let again = apply(&c, 0, SectionOp::Again).unwrap();
        assert_eq!(at(&again, 91), Some(17.0));
        assert_eq!(again.plan.sections[0].take, 1);

        // A section added after the verse moves the chorus (and its edit) later.
        let bridge = PlanSection { name: "Bridge".into(), bars: 2, ..Default::default() };
        let added = apply(&c, 0, SectionOp::Add { section: bridge }).unwrap();
        assert_eq!(added.plan.sections[1].name, "Bridge");
        assert_eq!(at(&added, 91), Some(25.0));
        assert_eq!(at(&added, 90), Some(1.0));

        // The verse copied: its notes, edit included, twice; the chorus after.
        let copied = apply(&c, 0, SectionOp::Duplicate).unwrap();
        assert_eq!(copied.parts[1].notes.iter().filter(|n| n.pitch == 90).map(|n| n.beat).collect::<Vec<_>>(), vec![1.0, 17.0]);
        assert_eq!(at(&copied, 91), Some(33.0));
        assert_eq!(count(&copied, 0), count(&c, 0) + c.parts[0].notes.iter().filter(|n| n.beat < 16.0).count());

        // The verse taken out: the chorus moves to the start.
        let gone = apply(&c, 0, SectionOp::Delete).unwrap();
        assert_eq!(at(&gone, 91), Some(1.0));
        assert_eq!(at(&gone, 90), None);
        assert!(apply(&gone, 0, SectionOp::Delete).is_err(), "the last section stays");
    }

    /// Their own lyrics become the sections, in order, with their lines.
    #[test]
    fn lyrics_become_sections() {
        let b = lyric_blocks("Walking in\n\n[Chorus - screamed]\nHold on\nHold on tight\n[Instrumental]\n[Verse 2]\nBack again");
        assert_eq!(
            b,
            vec![
                ("Verse".to_string(), vec!["Walking in".to_string()]),
                ("Chorus".to_string(), vec!["Hold on".to_string(), "Hold on tight".to_string()]),
                ("Instrumental".to_string(), vec![]),
                ("Verse 2".to_string(), vec!["Back again".to_string()]),
            ]
        );
    }

    /// Sung sections get a line about every 7.5 seconds, in pairs; played
    /// ones none. A 40-second 6/8 verse needs six, not the two it was given.
    #[test]
    fn sung_sections_get_lines_for_their_length() {
        let mut p = plan(6, 8);
        p.bpm = 72;
        p.sections[0].bars = 16;
        p.sections.insert(0, PlanSection { name: "Intro".into(), bars: 8, ..Default::default() });
        assert_eq!(wanted_lines(&p), vec![(1, 6), (2, 2)]);
        // An intro the plan gave words is sung.
        p.sections[0].lines = vec!["Hey".into()];
        assert_eq!(wanted_lines(&p)[0], (0, 2));
    }

    /// Songs repeat themselves: lines of a section share one rhythm, and a
    /// lead's riff plays the same figure bar after bar (changed on the fourth).
    #[test]
    fn tunes_repeat_with_variation() {
        let mut p = plan(4, 4);
        p.sections[0].lines = vec!["Walking down the road at night".into(), "Singing to the moon so bright".into()];
        let sung = &arrange(&p)[0].notes;
        let onsets = |from: f32, to: f32| sung.iter().filter(|n| n.beat >= from && n.beat < to).map(|n| n.beat - from).collect::<Vec<_>>();
        assert_eq!(onsets(0.0, 8.0), onsets(8.0, 16.0), "both lines (7 syllables) in the same rhythm");
        p.sections[1].melody = true;
        p.sections[1].chords = vec!["Am".into()];
        let lead = &arrange(&p)[0].notes;
        let bar = |b: usize| lead.iter().filter(|n| n.beat >= 16.0 + b as f32 * 4.0 && n.beat < 20.0 + b as f32 * 4.0).map(|n| (n.beat % 4.0, n.pitch)).collect::<Vec<_>>();
        assert!(bar(0).len() >= 3);
        assert_eq!(bar(0), bar(1));
        assert_eq!(bar(0), bar(2));
    }

    #[test]
    fn instruments_and_midi() {
        assert_eq!((program("Electric piano", false), program("synth bass", false), program("jazz kit", true)), (4, 38, 32));
        let p = plan(6, 8);
        let c = Composition { parts: arrange(&p), plan: p };
        let f = midi(&c);
        assert_eq!(&f[..4], b"MThd");
        assert_eq!(f.windows(4).filter(|w| w == b"MTrk").count(), 5);
        // 6/8: numerator 6, denominator 2^3.
        assert!(f.windows(6).any(|w| w == [0xFF, 0x58, 0x04, 6, 3, 24]));
        // 100 quarters a minute: 600,000 us a quarter.
        assert!(f.windows(6).any(|w| w == [0xFF, 0x51, 0x03, 0x09, 0x27, 0xC0]));
        // Eight bars of 6/8 at 100: 24 quarters, 14.4 seconds.
        assert!((c.plan.seconds() - 14.4).abs() < 1e-6);
        assert!(c.plan.lyrics().starts_with("[Verse]\nMorning light"));
    }
}
