// SPDX-License-Identifier: AGPL-3.0-only
//! Dialogue tags: a script with speakers and how lines are said, e.g.
//! `ANNA [whispers]: Come here.` or `Come here [laughs] now.`
//! Each line becomes a part for the narration engine with its voice and
//! delivery; tags the engine can't voice become pauses or are dropped.

use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Part {
    pub speaker: String,
    pub text: String,
    /// How it's said: whispers, shouts, sad, ...
    pub delivery: Vec<String>,
    /// Sounds placed in the line (laughs, sighs), in order.
    pub sounds: Vec<String>,
    /// Silence after the part, in seconds.
    pub pause_after: f64,
}

const SOUNDS: &[&str] = &["laughs", "laugh", "sighs", "sigh", "coughs", "cough", "gasps", "gasp", "clears throat", "sniffs", "chuckles"];

fn pause(tag: &str) -> Option<f64> {
    let t = tag.trim().to_lowercase();
    if t == "pause" || t == "beat" {
        return Some(0.7);
    }
    if t == "long pause" {
        return Some(1.5);
    }
    t.strip_prefix("pause ").and_then(|s| s.trim_end_matches('s').trim().parse::<f64>().ok()).map(|s| s.clamp(0.1, 10.0))
}

/// Splits "[a] text [b] more" into the plain text and the tags in it.
fn untag(s: &str) -> (String, Vec<String>) {
    let mut text = String::new();
    let mut tags = Vec::new();
    let mut rest = s;
    while let Some(o) = rest.find('[') {
        let Some(c) = rest[o..].find(']') else { break };
        text.push_str(&rest[..o]);
        tags.push(rest[o + 1..o + c].trim().to_lowercase());
        rest = &rest[o + c + 1..];
    }
    text.push_str(rest);
    (text.split_whitespace().collect::<Vec<_>>().join(" "), tags)
}

pub fn parse(script: &str, narrator: &str) -> Vec<Part> {
    let mut out: Vec<Part> = Vec::new();
    for line in script.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let (who, said) = match line.split_once(':') {
            Some((w, s)) if w.len() <= 40 && !w.contains('.') => (w, s),
            _ => ("", line),
        };
        let (name, name_tags) = untag(who);
        let (text, line_tags) = untag(said);
        let mut part = Part { speaker: if name.is_empty() { narrator.to_string() } else { name }, text, delivery: Vec::new(), sounds: Vec::new(), pause_after: 0.0 };
        for t in name_tags.into_iter().chain(line_tags) {
            if let Some(p) = pause(&t) {
                part.pause_after += p;
            } else if SOUNDS.contains(&t.as_str()) {
                part.sounds.push(t);
            } else if !t.is_empty() {
                part.delivery.push(t);
            }
        }
        if part.text.is_empty() && part.sounds.is_empty() {
            // A tag alone on its line ("[pause]") belongs to the line before.
            if let Some(prev) = out.last_mut() {
                prev.pause_after += part.pause_after;
            }
            continue;
        }
        out.push(part);
    }
    out
}

/// How a turn is read: speed for the voice, loudness, silence after.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    pub speed: f32,
    pub gain: f32,
    pub pause: f32,
}

impl Default for Shape {
    fn default() -> Self {
        Shape { speed: 1.0, gain: 1.0, pause: 0.0 }
    }
}

fn shape_of(p: &Part) -> Shape {
    let mut s = Shape { pause: p.pause_after as f32, ..Default::default() };
    for d in &p.delivery {
        match d.as_str() {
            "whispers" | "whispering" | "quietly" | "softly" => s.gain *= 0.55,
            "shouts" | "shouting" | "yells" | "loudly" => s.gain *= 1.4,
            "fast" | "quickly" | "excited" | "excitedly" | "rushed" => s.speed *= 1.15,
            "slowly" | "slow" | "sad" | "sadly" | "tired" => s.speed *= 0.88,
            _ => {}
        }
    }
    s
}

/// A shape for each turn of a reading, matched to the tagged script by text
/// (turns are read with their tags already taken out).
pub fn shapes(script: &str, turns: &[&str]) -> Vec<Shape> {
    let parts = parse(script, "");
    let mut k = 0;
    turns
        .iter()
        .map(|t| {
            let at = parts[k.min(parts.len())..].iter().position(|p| !p.text.is_empty() && t.starts_with(p.text.trim_end_matches(['*']).trim()));
            match at {
                Some(i) => {
                    k += i + 1;
                    shape_of(&parts[k - 1])
                }
                None => Shape::default(),
            }
        })
        .collect()
}

/// Speakers in order of first line, for picking a voice each.
pub fn speakers(parts: &[Part]) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for p in parts {
        if !v.contains(&p.speaker) {
            v.push(p.speaker.clone());
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn speakers_delivery_and_sounds() {
        let p = parse("ANNA [whispers]: Come here. [laughs]\nThe wind rose.\n[pause 2s]\nBOB: [shouts] No!", "Narrator");
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].speaker, "ANNA");
        assert_eq!(p[0].text, "Come here.");
        assert_eq!(p[0].delivery, vec!["whispers"]);
        assert_eq!(p[0].sounds, vec!["laughs"]);
        assert_eq!(p[1].speaker, "Narrator");
        assert_eq!(p[1].pause_after, 2.0);
        assert_eq!(p[2].delivery, vec!["shouts"]);
        assert_eq!(speakers(&p), vec!["ANNA", "Narrator", "BOB"]);
    }
    #[test]
    fn shapes_follow_turns() {
        let s = shapes("A [whispers]: Hi there.\nB: [shouts] Go! [pause 1s]\nA: fine", &["Hi there.", "Go!", "fine"]);
        assert_eq!(s[0].gain, 0.55);
        assert_eq!((s[1].gain, s[1].pause), (1.4, 1.0));
        assert_eq!(s[2], Shape::default());
        assert_eq!(shapes("x", &["unknown"])[0], Shape::default());
    }
    #[test]
    fn sentence_with_colon_and_dot_is_narration() {
        let p = parse("At 5 p.m.: dinner.", "N");
        assert_eq!(p[0].speaker, "N");
    }
}
