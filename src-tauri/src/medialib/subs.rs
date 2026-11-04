// SPDX-License-Identifier: AGPL-3.0-only
//! Subtitles: SRT and WebVTT in, SRT and WebVTT out.

use serde::{Deserialize, Serialize};

use super::index::Segment;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// Reads either format; blocks without a time line are skipped, since VTT
/// headers, notes and styles look like that.
pub fn parse(text: &str) -> Vec<Cue> {
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let mut out = Vec::new();
    for block in text.split("\n\n") {
        let mut lines = block.lines().skip_while(|l| !l.contains("-->"));
        let Some(times) = lines.next() else { continue };
        let mut parts = times.split("-->");
        let (Some(a), Some(b)) = (parts.next(), parts.next()) else { continue };
        let b = b.split_whitespace().next().unwrap_or("");
        let (Some(start), Some(end)) = (time(a.trim()), time(b)) else { continue };
        let body: Vec<&str> = lines.map(str::trim_end).filter(|l| !l.is_empty()).collect();
        out.push(Cue { start, end, text: strip_tags(&body.join("\n")) });
    }
    out
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut inside = false;
    for c in s.chars() {
        match c {
            '<' => inside = true,
            '>' if inside => inside = false,
            _ if !inside => out.push(c),
            _ => {}
        }
    }
    out
}

fn time(s: &str) -> Option<f64> {
    let s = s.replace(',', ".");
    let parts: Vec<&str> = s.split(':').collect();
    let (h, m, sec) = match parts.as_slice() {
        [h, m, s] => (h.parse::<f64>().ok()?, m.parse::<f64>().ok()?, s.parse::<f64>().ok()?),
        [m, s] => (0.0, m.parse::<f64>().ok()?, s.parse::<f64>().ok()?),
        _ => return None,
    };
    Some(h * 3600.0 + m * 60.0 + sec)
}

fn stamp(t: f64, sep: char) -> String {
    let ms = (t.max(0.0) * 1000.0).round() as u64;
    format!("{:02}:{:02}:{:02}{}{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, sep, ms % 1000)
}

/// Captions from a transcript, a speaker's name before their words when
/// the speaker changes.
pub fn captions(segs: &[Segment]) -> Vec<Cue> {
    let mut last: Option<&str> = None;
    segs.iter()
        .map(|s| {
            let sp = s.speaker.as_deref();
            let text = match sp {
                Some(n) if sp != last => format!("{n}: {}", s.text.trim()),
                _ => s.text.trim().to_string(),
            };
            last = sp;
            Cue { start: s.start, end: s.end, text }
        })
        .collect()
}

pub fn to_srt(cues: &[Cue]) -> String {
    cues.iter()
        .enumerate()
        .map(|(i, c)| format!("{}\n{} --> {}\n{}\n", i + 1, stamp(c.start, ','), stamp(c.end, ','), c.text))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn to_vtt(cues: &[Cue]) -> String {
    let mut out = String::from("WEBVTT\n");
    for c in cues {
        out.push_str(&format!("\n{} --> {}\n{}\n", stamp(c.start, '.'), stamp(c.end, '.'), c.text));
    }
    out
}

/// The cue showing at `t`, for the player.
pub fn at(cues: &[Cue], t: f64) -> Option<&Cue> {
    cues.iter().find(|c| c.start <= t && t < c.end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_srt_and_vtt() {
        let srt = "\u{feff}1\r\n00:00:01,500 --> 00:00:03,000\r\nHello <i>there</i>\r\n\r\n2\r\n00:01:00,000 --> 00:01:02,250\r\nTwo\r\nlines\r\n";
        let c = parse(srt);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0], Cue { start: 1.5, end: 3.0, text: "Hello there".into() });
        assert_eq!(c[1].text, "Two\nlines");
        let vtt = "WEBVTT\n\nNOTE a note\n\n00:05.000 --> 00:07.000 align:start\nShort form\n";
        let v = parse(vtt);
        assert_eq!(v, vec![Cue { start: 5.0, end: 7.0, text: "Short form".into() }]);
        assert_eq!(at(&c, 2.0).unwrap().text, "Hello there");
        assert!(at(&c, 10.0).is_none());
    }

    #[test]
    fn writes_round_trip_with_speakers() {
        let segs = vec![
            Segment { start: 0.0, end: 1.25, speaker: Some("Ana".into()), text: "Hi".into() },
            Segment { start: 1.25, end: 2.0, speaker: Some("Ana".into()), text: "again".into() },
            Segment { start: 3661.5, end: 3662.0, speaker: Some("Bo".into()), text: "Yes".into() },
        ];
        let cues = captions(&segs);
        assert_eq!(cues[1].text, "again");
        let srt = to_srt(&cues);
        assert!(srt.contains("01:01:01,500 --> 01:01:02,000\nBo: Yes"));
        assert_eq!(parse(&srt), cues);
        assert_eq!(parse(&to_vtt(&cues)), cues);
    }
}
