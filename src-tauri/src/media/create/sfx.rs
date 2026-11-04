// SPDX-License-Identifier: AGPL-3.0-only
//! Sound effects from words: a request turned into a short prompt and length
//! for the music model, which makes sound as well as songs when asked for
//! "no music, no vocals".

use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Effect {
    pub prompt: String,
    pub seconds: f64,
    pub looped: bool,
}

/// "3 seconds of rain on a tin roof, looping" -> prompt, length, loop.
pub fn plan(request: &str) -> Effect {
    let low = request.to_lowercase();
    let looped = ["loop", "looping", "seamless", "ambience", "ambient", "background"].iter().any(|k| low.contains(k));
    let words: Vec<&str> = low.split_whitespace().collect();
    let mut seconds = None;
    for (i, w) in words.iter().enumerate() {
        let num = w.trim_end_matches(['s', ',']).parse::<f64>().ok();
        let unit = words.get(i + 1).copied().unwrap_or("");
        if let Some(n) = num {
            if w.ends_with('s') || unit.starts_with("sec") {
                seconds = Some(n);
            } else if unit.starts_with("min") {
                seconds = Some(n * 60.0);
            }
        }
    }
    let default = if looped { 20.0 } else { 4.0 };
    let skip = ["seconds", "second", "secs", "sec", "of", "a", "an", "minute", "minutes", "looping", "loop", "sound", "effect", "make", "me"];
    let what: Vec<&str> = request
        .split_whitespace()
        .filter(|w| {
            let l = w.to_lowercase();
            let l = l.trim_matches(|c: char| !c.is_alphanumeric());
            !skip.contains(&l) && l.trim_end_matches('s').parse::<f64>().is_err()
        })
        .collect();
    let what = what.join(" ").trim_matches([',', '.', ' ']).to_string();
    Effect { prompt: format!("sound effect: {what}, realistic foley, no music, no vocals"), seconds: seconds.unwrap_or(default).clamp(1.0, 120.0), looped }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_length_and_loop() {
        let e = plan("30 seconds of rain on a tin roof, looping");
        assert_eq!(e.seconds, 30.0);
        assert!(e.looped);
        assert!(e.prompt.contains("rain on tin roof"), "{}", e.prompt);
        let d = plan("a door slam");
        assert_eq!(d.seconds, 4.0);
        assert!(!d.looped);
        assert_eq!(plan("2 minutes of forest ambience").seconds, 120.0);
        assert_eq!(plan("5s whoosh").seconds, 5.0);
    }
}
