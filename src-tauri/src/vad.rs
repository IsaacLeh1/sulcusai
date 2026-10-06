// SPDX-License-Identifier: AGPL-3.0-only
//! Voice activity detection by loudness against a learned noise floor.
//!
//! It only decides when someone starts and stops talking (for voice mode
//! turn-taking and for cutting meeting audio at pauses). Speech recognition
//! runs its own, more careful voice detection on what this passes along.

use crate::audio::RATE;

/// 20 ms frames.
pub const FRAME: usize = (RATE / 50) as usize;

#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// Speech must last this long to count as someone talking.
    pub start_ms: u32,
    /// Silence this long ends the utterance.
    pub end_ms: u32,
    /// Quietest level ever treated as speech (RMS).
    pub min_level: f32,
    /// How far above the noise floor speech must be.
    pub ratio: f32,
}

impl Default for Config {
    fn default() -> Self {
        Config { start_ms: 160, end_ms: 800, min_level: 0.006, ratio: 3.0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Speech began this many samples before the end of the pushed audio.
    Start { back: usize },
    End,
}

/// Frames of history for the noise floor (3 s).
const FLOOR_FRAMES: usize = 150;

pub struct Vad {
    cfg: Config,
    noise: f32,
    /// Recent frame levels; the quietest is the room's noise floor (speech
    /// has gaps between words, so this holds even while someone talks).
    history: std::collections::VecDeque<f32>,
    speech_run: u32,
    silence_run: u32,
    active: bool,
    pending: Vec<f32>,
    /// Multiplies the threshold, e.g. while the PC is talking, so its own
    /// voice coming back through the microphone isn't taken for the user.
    pub boost: f32,
}

impl Vad {
    pub fn new(cfg: Config) -> Vad {
        Vad {
            cfg,
            noise: cfg.min_level / cfg.ratio,
            history: std::collections::VecDeque::with_capacity(FLOOR_FRAMES),
            speech_run: 0, silence_run: 0, active: false, pending: Vec::new(), boost: 1.0 }
    }

    pub fn active(&self) -> bool {
        self.active
    }

    pub fn threshold(&self) -> f32 {
        (self.noise * self.cfg.ratio).max(self.cfg.min_level) * self.boost
    }

    fn frames(ms: u32) -> u32 {
        (ms / 20).max(1)
    }

    pub fn push(&mut self, samples: &[f32]) -> Vec<Event> {
        self.pending.extend_from_slice(samples);
        let mut events = Vec::new();
        let mut consumed = 0;
        while self.pending.len() - consumed >= FRAME {
            let frame = &self.pending[consumed..consumed + FRAME];
            consumed += FRAME;
            let level = crate::audio::rms(frame);
            if self.history.len() == FLOOR_FRAMES {
                self.history.pop_front();
            }
            self.history.push_back(level);
            if self.history.len() >= 25 {
                self.noise = self.history.iter().copied().fold(f32::MAX, f32::min);
            }
            let threshold = self.threshold();
            // Once talking, a quieter level keeps it going (hysteresis).
            let loud = if self.active { level > threshold * 0.6 } else { level > threshold };
            if loud {
                self.speech_run += 1;
                self.silence_run = 0;
            } else {
                self.silence_run += 1;
                self.speech_run = 0;
            }
            if !self.active && self.speech_run >= Self::frames(self.cfg.start_ms) {
                self.active = true;
                let back = (self.speech_run as usize) * FRAME + (self.pending.len() - consumed);
                events.push(Event::Start { back });
            } else if self.active && self.silence_run >= Self::frames(self.cfg.end_ms) {
                self.active = false;
                events.push(Event::End);
            }
        }
        self.pending.drain(..consumed);
        events
    }

    /// Forgets any utterance in progress (the noise floor is kept).
    pub fn reset(&mut self) {
        self.active = false;
        self.speech_run = 0;
        self.silence_run = 0;
        self.pending.clear();
    }
}

/// A stretch of speech cut out of a live stream.
#[derive(Debug, Clone)]
pub struct Utterance {
    /// Position of the first sample in the whole stream.
    pub start: u64,
    pub samples: Vec<f32>,
}

impl Utterance {
    pub fn start_secs(&self) -> f64 {
        self.start as f64 / RATE as f64
    }

    pub fn secs(&self) -> f64 {
        self.samples.len() as f64 / RATE as f64
    }
}

#[derive(Debug, Clone)]
pub enum Phrase {
    /// Someone started talking (voice mode uses this to stop speaking).
    Started,
    Done(Utterance),
}

/// Cuts a live stream into utterances at pauses, keeping a little audio from
/// before each start so first syllables aren't clipped. Long speech is cut
/// at `max_secs` so transcripts keep up.
pub struct Phraser {
    pub vad: Vad,
    pre: std::collections::VecDeque<f32>,
    current: Option<Utterance>,
    pos: u64,
    rest: Vec<f32>,
    max: usize,
}

const PRE_ROLL: usize = (RATE as usize) * 6 / 10;
/// Silence kept after the last word.
const TAIL: usize = (RATE as usize) / 4;

impl Phraser {
    pub fn new(cfg: Config, max_secs: f64) -> Phraser {
        Phraser {
            vad: Vad::new(cfg),
            pre: std::collections::VecDeque::with_capacity(PRE_ROLL + FRAME),
            current: None,
            pos: 0,
            rest: Vec::new(),
            max: (max_secs * RATE as f64) as usize,
        }
    }

    /// Samples taken in so far.
    pub fn position(&self) -> u64 {
        self.pos
    }

    pub fn speaking(&self) -> bool {
        self.current.is_some()
    }

    pub fn push(&mut self, samples: &[f32]) -> Vec<Phrase> {
        self.rest.extend_from_slice(samples);
        let mut out = Vec::new();
        let frames = self.rest.len() / FRAME;
        let data: Vec<f32> = self.rest.drain(..frames * FRAME).collect();
        for frame in data.chunks(FRAME) {
            let events = self.vad.push(frame);
            self.pos += frame.len() as u64;
            match &mut self.current {
                Some(u) => u.samples.extend_from_slice(frame),
                None => {
                    self.pre.extend(frame.iter().copied());
                    while self.pre.len() > PRE_ROLL {
                        self.pre.pop_front();
                    }
                }
            }
            for e in events {
                match e {
                    Event::Start { .. } => {
                        if self.current.is_none() {
                            let samples: Vec<f32> = self.pre.drain(..).collect();
                            self.current = Some(Utterance { start: self.pos - samples.len() as u64, samples });
                            out.push(Phrase::Started);
                        }
                    }
                    Event::End => {
                        if let Some(mut u) = self.current.take() {
                            // Drop most of the trailing silence that ended it.
                            let silence = (self.vad.cfg.end_ms as usize * RATE as usize / 1000).saturating_sub(TAIL);
                            u.samples.truncate(u.samples.len().saturating_sub(silence));
                            out.push(Phrase::Done(u));
                        }
                    }
                }
            }
            if self.current.as_ref().is_some_and(|u| u.samples.len() >= self.max) {
                let u = self.current.take().unwrap();
                self.current = Some(Utterance { start: self.pos, samples: Vec::new() });
                out.push(Phrase::Done(u));
            }
        }
        out
    }

    /// Ends the stream: whatever speech is in progress.
    pub fn flush(&mut self) -> Option<Utterance> {
        self.vad.reset();
        self.current.take().filter(|u| u.samples.len() > FRAME * 10)
    }

    /// Forgets speech in progress (e.g. it was the PC's own voice).
    pub fn discard(&mut self) {
        self.current = None;
        self.pre.clear();
        self.vad.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(ms: u32, amp: f32) -> Vec<f32> {
        let n = (RATE * ms / 1000) as usize;
        (0..n).map(|i| (i as f32 * 0.07).sin() * amp).collect()
    }

    /// Syllables with short gaps, like real speech.
    fn speechy(ms: u32) -> Vec<f32> {
        let mut out = Vec::new();
        while out.len() < (RATE * ms / 1000) as usize {
            out.extend(tone(250, 0.2));
            out.extend(noise(80, 0.001));
        }
        out.truncate((RATE * ms / 1000) as usize);
        out
    }

    fn noise(ms: u32, amp: f32) -> Vec<f32> {
        let n = (RATE * ms / 1000) as usize;
        let mut x: u32 = 12345;
        (0..n)
            .map(|_| {
                x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
                ((x >> 16) as f32 / 32768.0 - 1.0) * amp
            })
            .collect()
    }

    #[test]
    fn speech_between_silences_starts_and_ends() {
        let mut v = Vad::new(Config::default());
        assert!(v.push(&noise(1000, 0.002)).is_empty());
        let ev = v.push(&tone(600, 0.2));
        assert!(matches!(ev[..], [Event::Start { .. }]));
        assert!(v.active());
        // Short pauses don't end it.
        assert!(v.push(&noise(300, 0.002)).is_empty());
        v.push(&tone(300, 0.2));
        let ev = v.push(&noise(1200, 0.002));
        assert_eq!(ev, vec![Event::End]);
    }

    #[test]
    fn a_click_is_not_speech() {
        let mut v = Vad::new(Config::default());
        v.push(&noise(500, 0.002));
        assert!(v.push(&tone(60, 0.5)).is_empty());
        assert!(v.push(&noise(500, 0.002)).is_empty());
    }

    #[test]
    fn steady_background_noise_is_learned() {
        let mut v = Vad::new(Config::default());
        // A noisy room: loud enough to trip a fixed threshold at first...
        v.push(&noise(4000, 0.02));
        // ...but after a while it is the floor, and speech must rise above it.
        v.reset();
        assert!(v.push(&noise(1000, 0.02)).is_empty());
        assert!(!v.push(&tone(500, 0.2)).is_empty());
    }

    #[test]
    fn start_reports_how_far_back_speech_began() {
        let mut v = Vad::new(Config::default());
        v.push(&noise(500, 0.001));
        let ev = v.push(&tone(400, 0.2));
        let Event::Start { back } = ev[0] else { panic!() };
        // At least the 160 ms needed to decide, at most the whole tone.
        assert!(back >= FRAME * 8 && back <= (RATE as usize * 400 / 1000), "{back}");
    }

    #[test]
    fn phraser_cuts_utterances_with_true_positions() {
        let mut p = Phraser::new(Config::default(), 30.0);
        let mut all = Vec::new();
        all.extend(p.push(&noise(1000, 0.002)));
        all.extend(p.push(&tone(1500, 0.2)));
        all.extend(p.push(&noise(1500, 0.002)));
        all.extend(p.push(&tone(800, 0.2)));
        all.extend(p.push(&noise(1500, 0.002)));
        let done: Vec<&Utterance> = all.iter().filter_map(|e| if let Phrase::Done(u) = e { Some(u) } else { None }).collect();
        assert_eq!(all.iter().filter(|e| matches!(e, Phrase::Started)).count(), 2);
        assert_eq!(done.len(), 2);
        // Starts a little before the speech (pre-roll), never after it.
        assert!(done[0].start_secs() > 0.35 && done[0].start_secs() <= 1.0, "{}", done[0].start_secs());
        assert!(done[0].secs() > 1.5 && done[0].secs() < 2.6, "{}", done[0].secs());
        assert!(done[1].start_secs() > 3.4 && done[1].start_secs() <= 4.0, "{}", done[1].start_secs());
    }

    #[test]
    fn phraser_splits_long_speech_and_flushes_the_rest() {
        let mut p = Phraser::new(Config::default(), 2.0);
        p.push(&noise(500, 0.002));
        let ev = p.push(&speechy(5000));
        let done = ev.iter().filter(|e| matches!(e, Phrase::Done(_))).count();
        assert_eq!(done, 2);
        let rest = p.flush().unwrap();
        assert!(rest.secs() > 0.5);
        assert!(p.flush().is_none());
    }

    #[test]
    fn boost_ignores_quieter_echo() {
        let mut v = Vad::new(Config::default());
        v.push(&noise(500, 0.001));
        v.boost = 6.0;
        assert!(v.push(&tone(500, 0.02)).is_empty());
        assert!(!v.push(&tone(500, 0.3)).is_empty());
    }
}
