// SPDX-License-Identifier: AGPL-3.0-only
//! Storyboards: a script split into shots, each with a picture prompt and a
//! length, so each shot can be drawn and then brought to life.
//! Shots are marked by blank lines, or by "SHOT"/"INT."/"EXT."/"SCENE" headings.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shot {
    pub n: usize,
    pub heading: String,
    pub action: String,
    pub dialogue: Vec<String>,
    pub seconds: f64,
    pub prompt: String,
}

fn is_heading(l: &str) -> bool {
    let u = l.trim_start().to_ascii_uppercase();
    u.starts_with("SHOT") || u.starts_with("INT.") || u.starts_with("EXT.") || u.starts_with("SCENE")
}

/// About 2.5 words a second spoken, at least 2 s and at most `max` (one clip).
fn length(action: &str, dialogue: &[String], max: f64) -> f64 {
    let spoken: usize = dialogue.iter().map(|d| d.split_once(": ").map_or(d.as_str(), |x| x.1).split_whitespace().count()).sum();
    let base = if spoken > 0 { spoken as f64 / 2.5 + 0.5 } else { 2.0 + action.split_whitespace().count() as f64 / 10.0 };
    base.clamp(2.0, max.max(2.0))
}

fn speech(t: &str) -> Option<String> {
    if let Some((who, said)) = t.split_once(':') {
        let who = who.trim();
        if !who.is_empty() && who.len() <= 24 && who.chars().all(|c| c.is_alphabetic() || c == ' ') && who == who.to_uppercase() {
            return Some(format!("{who}: {}", said.trim()));
        }
    }
    t.starts_with('"').then(|| t.trim_matches('"').to_string())
}

pub fn split(script: &str, style: &str, max: f64) -> Vec<Shot> {
    let mut blocks: Vec<Vec<&str>> = vec![Vec::new()];
    for line in script.lines() {
        let blank = line.trim().is_empty();
        if (blank || is_heading(line)) && !blocks.last().unwrap().is_empty() {
            blocks.push(Vec::new());
        }
        if !blank {
            blocks.last_mut().unwrap().push(line);
        }
    }
    let mut out = Vec::new();
    for b in blocks.into_iter().filter(|b| !b.is_empty()) {
        let (heading, rest) = if is_heading(b[0]) { (b[0].trim().to_string(), &b[1..]) } else { (String::new(), &b[..]) };
        let mut action = Vec::new();
        let mut dialogue = Vec::new();
        for l in rest {
            let t = l.trim();
            match speech(t) {
                Some(s) => dialogue.push(s),
                None => action.push(t),
            }
        }
        let action = action.join(" ");
        if action.is_empty() && dialogue.is_empty() {
            continue;
        }
        let place = heading.split_once(['.', ':']).map(|(_, p)| p.trim()).unwrap_or("");
        let what = if action.is_empty() { "a person speaking" } else { action.as_str() };
        let prompt = [place, what, style].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(", ");
        out.push(Shot { n: out.len() + 1, seconds: length(&action, &dialogue, max), heading, action, dialogue, prompt });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn splits_by_headings_and_blanks() {
        let s = "INT. KITCHEN - NIGHT\nA kettle boils.\nANNA: Is anyone home?\n\nShe opens the door.\nEXT. GARDEN\nRain falls.";
        let shots = split(s, "film still", 5.0);
        assert_eq!(shots.len(), 3);
        assert_eq!(shots[0].dialogue, vec!["ANNA: Is anyone home?"]);
        assert!(shots[0].prompt.starts_with("KITCHEN - NIGHT, A kettle boils."), "{}", shots[0].prompt);
        assert!(shots[0].prompt.ends_with("film still"));
        assert_eq!(shots[2].n, 3);
    }
    #[test]
    fn lengths_capped() {
        let long = format!("BOB: {}", "word ".repeat(60));
        let shots = split(&long, "", 5.0);
        assert_eq!(shots[0].seconds, 5.0);
        assert_eq!(shots[0].prompt, "a person speaking");
    }
}
