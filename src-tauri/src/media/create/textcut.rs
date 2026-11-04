// SPDX-License-Identifier: AGPL-3.0-only
//! Editing video by its transcript: strike words, keep the rest. Turns the
//! kept words into time ranges and an FFmpeg filter that joins them.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Word {
    pub text: String,
    pub start: f64,
    pub end: f64,
    #[serde(default)]
    pub removed: bool,
}

/// Kept ranges. Gaps between kept words shorter than `bridge` stay, so
/// breaths and short pauses aren't chopped; `pad` keeps word edges whole.
pub fn keep_ranges(words: &[Word], len: f64, pad: f64, bridge: f64) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = Vec::new();
    for w in words.iter().filter(|w| !w.removed && w.end > w.start) {
        let s = (w.start - pad).max(0.0);
        let e = (w.end + pad).min(len);
        match out.last_mut() {
            Some(last) if s - last.1 <= bridge => last.1 = last.1.max(e),
            _ => out.push((s, e)),
        }
    }
    out
}

/// A filter_complex that trims each range from input 0 and joins them.
pub fn filter(ranges: &[(f64, f64)], with_audio: bool) -> String {
    let mut f = String::new();
    let mut joined = String::new();
    for (i, (s, e)) in ranges.iter().enumerate() {
        f.push_str(&format!("[0:v]trim=start={s:.3}:end={e:.3},setpts=PTS-STARTPTS[v{i}];"));
        joined.push_str(&format!("[v{i}]"));
        if with_audio {
            f.push_str(&format!("[0:a]atrim=start={s:.3}:end={e:.3},asetpts=PTS-STARTPTS[a{i}];"));
            joined.push_str(&format!("[a{i}]"));
        }
    }
    let a = u8::from(with_audio);
    f.push_str(&format!("{joined}concat=n={}:v=1:a={a}[outv]{}", ranges.len(), if with_audio { "[outa]" } else { "" }));
    f
}

/// The transcript as it will read after the cut.
pub fn kept_text(words: &[Word]) -> String {
    words.iter().filter(|w| !w.removed).map(|w| w.text.trim()).collect::<Vec<_>>().join(" ")
}

/// Strikes filler words ("um", "uh", ...) without touching the rest.
pub fn strike_fillers(words: &mut [Word]) -> usize {
    const FILLERS: &[&str] = &["um", "uh", "erm", "uhm", "hmm", "mm"];
    let mut n = 0;
    for w in words.iter_mut() {
        let t: String = w.text.trim().trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
        if !w.removed && FILLERS.contains(&t.as_str()) {
            w.removed = true;
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    fn w(t: &str, s: f64, e: f64, r: bool) -> Word {
        Word { text: t.into(), start: s, end: e, removed: r }
    }
    #[test]
    fn removes_struck_words() {
        let ws = [w("so", 0.0, 0.3, false), w("um", 0.4, 0.9, true), w("hello", 2.0, 2.5, false), w("there", 2.6, 3.0, false)];
        let r = keep_ranges(&ws, 4.0, 0.05, 0.2);
        assert_eq!(r.len(), 2);
        assert!((r[1].0 - 1.95).abs() < 1e-9 && (r[1].1 - 3.05).abs() < 1e-9);
        assert_eq!(kept_text(&ws), "so hello there");
    }
    #[test]
    fn filter_shape() {
        let f = filter(&[(0.0, 1.0), (2.0, 3.0)], true);
        assert!(f.contains("concat=n=2:v=1:a=1[outv][outa]"));
        assert!(f.contains("atrim=start=2.000:end=3.000"));
        assert!(!filter(&[(0.0, 1.0)], false).contains("atrim"));
    }
    #[test]
    fn fillers() {
        let mut ws = vec![w("Um,", 0.0, 0.2, false), w("yes", 0.3, 0.5, false), w("uh", 0.6, 0.7, false)];
        assert_eq!(strike_fillers(&mut ws), 2);
        assert_eq!(kept_text(&ws), "yes");
    }
}
