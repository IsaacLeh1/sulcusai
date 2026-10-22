// SPDX-License-Identifier: AGPL-3.0-only
//! How a song is built, for the song view: its sections (verse, chorus…)
//! where their words are sung, and its parts (the voice, other instruments,
//! bass, drums) with their notes, as the song view draws them and a MIDI
//! file keeps them. The parts come from HTDemucs (`stems::separate`), the
//! notes from Basic Pitch and the drums' onsets (`notes`), the sections
//! from the listener's timed lines (`listen`).

use serde::{Deserialize, Serialize};

use super::listen::{LineHeard, Review};
use super::notes::Note;

/// A stretch of the song: "Verse", "Chorus", or the band alone ("Intro",
/// "Instrumental", "Outro").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Section {
    pub name: String,
    pub start: f64,
    pub end: f64,
    /// Its lines, as sung.
    #[serde(default)]
    pub lines: Vec<String>,
}

/// A part and its notes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Part {
    /// "vocals", "other", "bass" or "drums".
    pub name: String,
    pub notes: Vec<Note>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SongMap {
    pub length: f64,
    pub sections: Vec<Section>,
    pub parts: Vec<Part>,
}

/// Band-only stretches at least this long get a section of their own.
const BAND_ALONE: f64 = 4.0;

/// The song's sections from its lines: a run of lines under one tag is a
/// section (a chorus sung twice is two), from its first sung line to the
/// next section's start, so the sections cover the song; the band alone
/// before, between and after are an intro, instrumentals and an outro.
pub fn sections(lines: &[LineHeard], length: f64) -> Vec<Section> {
    if lines.is_empty() {
        return vec![Section { name: "Song".into(), start: 0.0, end: length, lines: Vec::new() }];
    }
    // Runs of lines under one tag, with the times found for them.
    let mut runs: Vec<(String, Vec<&LineHeard>)> = Vec::new();
    let mut last_tag: Option<&str> = None;
    for (i, l) in lines.iter().enumerate() {
        // A new run when the tag changes, or when the same tag comes again
        // after a gap in the lyrics (seen as a line already sung before it).
        let repeat = runs.last().is_some_and(|(_, ls)| ls.first().is_some_and(|f| f.text == l.text) && i > 0);
        if last_tag != Some(l.section.as_str()) || repeat {
            runs.push((l.section.clone(), Vec::new()));
        }
        runs.last_mut().unwrap().1.push(l);
        last_tag = Some(l.section.as_str());
    }
    // Each run's sung span, where any of its lines was found; and whether
    // lines after the last one found weren't (the section then runs on
    // rather than leaving the band alone after it).
    let mut sung: Vec<(String, Option<(f64, f64)>, Vec<String>)> = Vec::new();
    let mut runs_on: Vec<bool> = Vec::new();
    for (name, ls) in runs {
        let at: Vec<[f64; 2]> = ls.iter().filter_map(|l| l.at).collect();
        let span = (!at.is_empty()).then(|| (at.iter().map(|a| a[0]).fold(f64::MAX, f64::min), at.iter().map(|a| a[1]).fold(0.0, f64::max)));
        runs_on.push(ls.last().is_some_and(|l| l.at.is_none()));
        sung.push((if name.is_empty() { "Verse".into() } else { name }, span, ls.iter().map(|l| l.text.clone()).collect()));
    }
    // A run not heard sits halfway between the ones around it.
    for i in 0..sung.len() {
        if sung[i].1.is_some() {
            continue;
        }
        let before = sung[..i].iter().rev().find_map(|s| s.1).map_or(0.0, |s| s.1);
        let after = sung[i + 1..].iter().find_map(|s| s.1).map_or(length, |s| s.0);
        sung[i].1 = Some((before, after.max(before)));
    }
    let mut out: Vec<Section> = Vec::new();
    let mut at = 0.0;
    for (i, (name, span, lines)) in sung.iter().enumerate() {
        let (start, end) = span.unwrap();
        let before_runs_on = i > 0 && runs_on[i - 1];
        if start - at >= BAND_ALONE && !before_runs_on {
            out.push(Section { name: if i == 0 { "Intro".into() } else { "Instrumental".into() }, start: at, end: start, lines: Vec::new() });
        } else if let Some(prev) = out.last_mut() {
            prev.end = start.max(prev.start);
        }
        let start = if out.is_empty() { 0.0 } else { start };
        out.push(Section { name: name.clone(), start, end: end.max(start), lines: lines.clone() });
        at = end.max(start);
    }
    if length - at >= BAND_ALONE && !runs_on.last().copied().unwrap_or(false) {
        out.push(Section { name: "Outro".into(), start: at, end: length, lines: Vec::new() });
    } else if let Some(last) = out.last_mut() {
        last.end = length;
    }
    if out.is_empty() {
        out.push(Section { name: "Song".into(), start: 0.0, end: length, lines: Vec::new() });
    }
    out
}

/// The map of a song, from its review's lines and its parts' notes.
pub fn map(review: Option<&Review>, parts: Vec<Part>, length: f64) -> SongMap {
    SongMap { length, sections: sections(review.map_or(&[][..], |r| &r.lines), length), parts }
}

/// The map as a standard MIDI file (format 1): a track a part, the drums
/// on channel 10, at 120 beats a minute (480 ticks a beat), so a second is
/// 960 ticks.
pub fn midi(m: &SongMap) -> Vec<u8> {
    const TPQ: u16 = 480;
    let ticks = |secs: f32| (secs.max(0.0) as f64 * 960.0).round() as u32;
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
    file.extend_from_slice(&(m.parts.len() as u16 + 1).to_be_bytes());
    file.extend_from_slice(&TPQ.to_be_bytes());
    let mut track = |events: Vec<u8>| {
        file.extend_from_slice(b"MTrk");
        file.extend_from_slice(&(events.len() as u32).to_be_bytes());
        file.extend_from_slice(&events);
    };
    // The tempo track: 120 bpm (500,000 microseconds a beat).
    let mut tempo = vec![0x00, 0xFF, 0x51, 0x03, 0x07, 0xA1, 0x20];
    tempo.extend_from_slice(&[0x00, 0xFF, 0x2F, 0x00]);
    track(tempo);
    let mut channel = 0u8;
    for part in &m.parts {
        let ch = if part.name == "drums" {
            9
        } else {
            let c = channel;
            channel = (channel + 1) % 9;
            c
        };
        let mut events: Vec<(u32, u8, u8, u8)> = Vec::new();
        for n in &part.notes {
            events.push((ticks(n.start), 0x90 | ch, n.pitch, n.velocity.max(1)));
            events.push((ticks(n.end.max(n.start + 0.02)), 0x80 | ch, n.pitch, 0));
        }
        // Note-offs before note-ons at the same tick.
        events.sort_by_key(|e| (e.0, e.1 & 0xF0 == 0x90));
        let mut bytes = Vec::new();
        // The track's name.
        let name = part.name.as_bytes();
        bytes.extend_from_slice(&[0x00, 0xFF, 0x03]);
        vlq(name.len() as u32, &mut bytes);
        bytes.extend_from_slice(name);
        let mut now = 0;
        for (t, status, pitch, vel) in events {
            vlq(t - now, &mut bytes);
            bytes.extend_from_slice(&[status, pitch, vel]);
            now = t;
        }
        bytes.extend_from_slice(&[0x00, 0xFF, 0x2F, 0x00]);
        track(bytes);
    }
    file
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(section: &str, text: &str, at: Option<[f64; 2]>) -> LineHeard {
        LineHeard { section: section.into(), text: text.into(), heard: if at.is_some() { 1.0 } else { 0.0 }, at }
    }

    #[test]
    fn sections_cover_the_song_with_the_band_alone_named() {
        let lines = [
            line("Verse", "a", Some([8.0, 11.0])),
            line("Verse", "b", Some([11.0, 14.0])),
            line("Chorus", "c", Some([15.0, 18.0])),
            line("Chorus", "d", Some([18.0, 20.0])),
            line("Verse 2", "e", Some([30.0, 33.0])),
            line("Chorus", "c", Some([34.0, 37.0])),
        ];
        let s = sections(&lines, 50.0);
        let names: Vec<&str> = s.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, ["Intro", "Verse", "Chorus", "Instrumental", "Verse 2", "Chorus", "Outro"]);
        assert_eq!((s[0].start, s[0].end), (0.0, 8.0));
        assert_eq!((s[1].end, s[2].start), (15.0, 15.0), "a section runs to the next");
        assert_eq!((s[3].start, s[3].end), (20.0, 30.0), "ten seconds of band between");
        assert_eq!(s[2].lines, ["c", "d"]);
        assert_eq!((s[6].start, s[6].end), (37.0, 50.0));
        // A chorus whose last line wasn't heard runs on to the end: the
        // band isn't alone after it.
        let unclear = [line("Verse", "a", Some([1.0, 4.0])), line("Chorus", "c", Some([10.0, 13.0])), line("Chorus", "d", None)];
        let s = sections(&unclear, 40.0);
        assert_eq!(s.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["Verse", "Instrumental", "Chorus"], "six seconds of band between, but none after");
        assert_eq!(s[2].end, 40.0);
        // Nothing heard: the whole song is one section.
        let one = sections(&[], 30.0);
        assert_eq!((one.len(), one[0].name.as_str(), one[0].end), (1, "Song", 30.0));
    }

    #[test]
    fn a_midi_file_holds_every_part() {
        let m = SongMap {
            length: 4.0,
            sections: Vec::new(),
            parts: vec![
                Part { name: "vocals".into(), notes: vec![Note { start: 0.5, end: 1.0, pitch: 60, velocity: 90 }] },
                Part { name: "drums".into(), notes: vec![Note { start: 0.0, end: 0.1, pitch: 36, velocity: 100 }] },
            ],
        };
        let f = midi(&m);
        assert_eq!(&f[..4], b"MThd");
        assert_eq!(u16::from_be_bytes([f[10], f[11]]), 3, "a tempo track and one a part");
        assert_eq!(f.windows(4).filter(|w| w == b"MTrk").count(), 3);
        // The voice's note on channel 1 at 0.5 s (480 ticks: 0x83 0x60 as a variable-length number).
        assert!(f.windows(6).any(|w| w == [0x83, 0x60, 0x90, 60, 90, 0x83]) || f.windows(5).any(|w| w == [0x83, 0x60, 0x90, 60, 90]));
        // The kick on channel 10.
        assert!(f.windows(4).any(|w| w == [0x00, 0x99, 36, 100]));
    }
}
