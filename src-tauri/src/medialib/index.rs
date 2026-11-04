// SPDX-License-Identifier: AGPL-3.0-only
//! Transcripts in the Media index: who said what and when, chapters, search.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    #[serde(default)]
    pub speaker: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chapter {
    pub start: f64,
    pub end: f64,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hit {
    pub media: String,
    pub start: f64,
    pub speaker: Option<String>,
    pub text: String,
    pub score: u32,
}

/// Chapters start at long pauses, and never shorter than `min_len` seconds,
/// so a talk with many short breaths doesn't turn into dozens of chapters.
pub fn chapters(segs: &[Segment], pause: f64, min_len: f64) -> Vec<Chapter> {
    let mut out: Vec<Chapter> = Vec::new();
    let mut first = 0usize;
    for i in 0..segs.len() {
        let last = i + 1 == segs.len();
        let gap = if last { f64::INFINITY } else { segs[i + 1].start - segs[i].end };
        let long_enough = segs[i].end - segs[first].start >= min_len;
        if last || (gap >= pause && long_enough) {
            out.push(Chapter { start: segs[first].start, end: segs[i].end, title: title_of(&segs[first..=i]) });
            first = i + 1;
        }
    }
    out
}

/// The most used longer word in the chapter, for a plain title that a
/// model can later replace.
fn title_of(segs: &[Segment]) -> String {
    let mut counts: std::collections::HashMap<String, usize> = Default::default();
    for s in segs {
        for w in words(&s.text).filter(|w| w.chars().count() >= 5) {
            *counts.entry(w).or_default() += 1;
        }
    }
    let mut best: Vec<_> = counts.into_iter().collect();
    best.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    match best.first() {
        Some((w, _)) => {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
        }
        None => segs.first().map(|s| s.text.chars().take(40).collect()).unwrap_or_default(),
    }
}

fn words(t: &str) -> impl Iterator<Item = String> + '_ {
    t.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(|w| w.to_lowercase())
}

/// Time spoken by each speaker, longest first.
pub fn speakers(segs: &[Segment]) -> Vec<(String, f64)> {
    let mut m: std::collections::HashMap<String, f64> = Default::default();
    for s in segs {
        if let Some(sp) = &s.speaker {
            *m.entry(sp.clone()).or_default() += (s.end - s.start).max(0.0);
        }
    }
    let mut v: Vec<_> = m.into_iter().collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

/// Segments holding every word of the query, best first.
pub fn search<'a>(items: impl IntoIterator<Item = (&'a str, &'a [Segment])>, query: &str) -> Vec<Hit> {
    let q: Vec<String> = words(query).collect();
    if q.is_empty() {
        return vec![];
    }
    let mut hits = Vec::new();
    for (media, segs) in items {
        for s in segs {
            let w: Vec<String> = words(&s.text).collect();
            if q.iter().all(|t| w.iter().any(|x| x.starts_with(t.as_str()))) {
                let score = q.iter().map(|t| w.iter().filter(|x| *x == t).count() as u32).sum::<u32>() + 1;
                hits.push(Hit { media: media.to_string(), start: s.start, speaker: s.speaker.clone(), text: s.text.clone(), score });
            }
        }
    }
    hits.sort_by(|a, b| b.score.cmp(&a.score).then(a.media.cmp(&b.media)).then(a.start.total_cmp(&b.start)));
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start: f64, end: f64, sp: &str, text: &str) -> Segment {
        Segment { start, end, speaker: Some(sp.into()), text: text.into() }
    }

    #[test]
    fn chapters_split_at_long_pauses_only_when_long_enough() {
        let s = vec![
            seg(0.0, 30.0, "A", "Gardens need water"),
            seg(30.5, 70.0, "B", "water the gardens daily"),
            seg(75.0, 80.0, "A", "short"),
            seg(90.0, 140.0, "A", "Engines and engines"),
        ];
        let c = chapters(&s, 3.0, 60.0);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].end, 70.0);
        assert_eq!(c[0].title, "Gardens");
        assert_eq!(c[1].start, 75.0);
        assert_eq!(c[1].title, "Engines");
    }

    #[test]
    fn speakers_by_time() {
        let s = vec![seg(0.0, 10.0, "A", "x"), seg(10.0, 40.0, "B", "y"), seg(40.0, 45.0, "A", "z")];
        assert_eq!(speakers(&s), vec![("B".into(), 30.0), ("A".into(), 15.0)]);
    }

    #[test]
    fn search_needs_every_word() {
        let a = vec![seg(0.0, 1.0, "A", "The budget for March"), seg(5.0, 6.0, "B", "budget budget")];
        let b = vec![seg(2.0, 3.0, "C", "March budget review")];
        let hits = search([("a.mp4", a.as_slice()), ("b.mp3", b.as_slice())], "budget mar");
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|h| h.text.contains("March")));
        assert!(search([("a", a.as_slice())], "  ").is_empty());
    }
}
