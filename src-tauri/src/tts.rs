// SPDX-License-Identifier: AGPL-3.0-only
//! Speech output: turns text into audio with a local voice.
//!
//! Voices built into Windows work on every PC with nothing to download. Voice
//! ids carry their engine as a prefix (`system:<id>`) so other local voice
//! engines can be added alongside.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Voice {
    pub id: String,
    pub name: String,
    /// BCP-47 language tag, e.g. "en-US".
    pub language: String,
    pub gender: String,
    pub engine: &'static str,
    /// Languages a multilingual voice speaks (empty: just `language`).
    #[serde(default)]
    pub languages: Vec<String>,
}

/// Asks for a Windows voice even when natural voices are installed (the
/// speech self-test uses a fixed, known voice).
pub const WINDOWS_DEFAULT: &str = "system:";

/// Mono 16-bit audio at `rate`.
#[derive(Debug, Clone)]
pub struct Speech {
    pub rate: u32,
    pub samples: Vec<i16>,
}

/// Every voice available on this PC: natural voices first, if installed.
pub fn voices() -> Result<Vec<Voice>, String> {
    let mut out: Vec<Voice> = crate::natural::installed()
        .map(|i| {
            i.pack
                .styles
                .iter()
                .map(|s| Voice {
                    id: format!("supertonic:{}", s.id),
                    name: format!("Natural · {}", s.name),
                    language: "multi".into(),
                    gender: s.gender.clone(),
                    engine: "natural",
                    languages: i.pack.languages.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    out.extend(std::thread::spawn(imp::voices).join().map_err(|_| "Listing voices failed.".to_string())??);
    Ok(out)
}

/// Speaks `text` (plain text; see `speakable`). Blocks, so call it from a
/// blocking thread. `rate` is 1.0 for normal speed; `language` is a code
/// such as "en" (natural voices need it to read correctly).
pub fn synthesize(text: &str, voice: Option<&str>, language: Option<&str>, rate: f64) -> Result<Speech, String> {
    if let Some(style) = voice.and_then(|v| v.strip_prefix("supertonic:")) {
        return crate::natural::synthesize(text, style, language, rate.clamp(0.5, 2.0));
    }
    let system_id = voice.and_then(|v| v.strip_prefix("system:")).filter(|id| !id.is_empty());
    imp::synthesize(text, system_id, rate.clamp(0.5, 3.0))
}

fn speaks(v: &Voice, lang: &str) -> bool {
    v.languages.iter().any(|l| l == lang) || v.language.to_ascii_lowercase().starts_with(lang)
}

/// Picks the configured voice, else a natural voice that speaks the
/// language, else a Windows voice for it, else the first one.
pub fn pick_voice<'a>(voices: &'a [Voice], configured: Option<&str>, language: Option<&str>) -> Option<&'a Voice> {
    if let Some(v) = configured.and_then(|id| voices.iter().find(|v| v.id == id)) {
        return Some(v);
    }
    let lang = language.filter(|l| !l.is_empty()).map(str::to_ascii_lowercase);
    if let Some(v) = voices.iter().find(|v| v.engine == "natural" && speaks(v, lang.as_deref().unwrap_or("en"))) {
        return Some(v);
    }
    if let Some(lang) = language.filter(|l| !l.is_empty()) {
        let lang = lang.to_ascii_lowercase();
        if let Some(v) = voices.iter().find(|v| v.language.to_ascii_lowercase().starts_with(&lang)) {
            return Some(v);
        }
    }
    // Nobody speaks it: a Windows voice copes better than a natural one.
    voices.iter().find(|v| v.engine != "natural").or(voices.first())
}

/// Strips Markdown and other things that sound wrong read aloud.
pub fn speakable(text: &str) -> String {
    let mut out = String::new();
    let mut in_code = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("```") {
            if !in_code {
                out.push_str("(There's a code block here.)\n");
            }
            in_code = !in_code;
            continue;
        }
        if in_code || t.starts_with('|') && t.ends_with('|') && t.chars().all(|c| "|-: ".contains(c)) {
            continue;
        }
        let mut t = t.trim_start_matches('#').trim_start();
        for marker in ["- [ ] ", "- [x] ", "- ", "* ", "+ ", "> "] {
            if let Some(rest) = t.strip_prefix(marker) {
                t = rest;
                break;
            }
        }
        out.push_str(&inline_plain(t));
        out.push('\n');
    }
    let collapsed: Vec<&str> = out.split_whitespace().collect();
    collapsed.join(" ")
}

fn inline_plain(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // [text](url) -> text
        if c == '[' {
            if let Some(close) = chars[i..].iter().position(|&c| c == ']').map(|p| p + i) {
                if chars.get(close + 1) == Some(&'(') {
                    if let Some(end) = chars[close..].iter().position(|&c| c == ')').map(|p| p + close) {
                        out.extend(&chars[i + 1..close]);
                        i = end + 1;
                        continue;
                    }
                }
            }
        }
        if matches!(c, '*' | '_' | '`' | '~') {
            // Emphasis and code markers are silent; a lone underscore in a word isn't emphasis.
            let inside_word = c == '_' && i > 0 && chars[i - 1].is_alphanumeric() && chars.get(i + 1).is_some_and(|n| n.is_alphanumeric());
            if !inside_word {
                i += 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    // Bare links are read as "a link".
    out.split(' ')
        .map(|w| if w.starts_with("http://") || w.starts_with("https://") { "a link" } else { w })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Collects streamed text and hands back whole sentences, so speech can
/// start before the reply is finished.
#[derive(Default)]
pub struct Sentences {
    buf: String,
    in_code: bool,
}

impl Sentences {
    pub fn push(&mut self, delta: &str) -> Vec<String> {
        self.buf.push_str(delta);
        let mut out = Vec::new();
        loop {
            let Some(cut) = self.boundary() else { break };
            let piece: String = self.buf.drain(..cut).collect();
            self.take(piece, &mut out);
        }
        out
    }

    /// Whatever is left at the end of the reply.
    pub fn finish(&mut self) -> Vec<String> {
        let piece = std::mem::take(&mut self.buf);
        let mut out = Vec::new();
        self.take(piece, &mut out);
        out
    }

    fn take(&mut self, piece: String, out: &mut Vec<String>) {
        // Code blocks are skipped, with one short mention.
        let fences = piece.matches("```").count();
        let was_code = self.in_code;
        if fences % 2 == 1 {
            self.in_code = !self.in_code;
        }
        // Inside a block, including its closing fence: nothing to say.
        if was_code {
            return;
        }
        let text = speakable(&piece);
        if !text.trim().is_empty() && text.chars().any(char::is_alphanumeric) {
            out.push(text);
        }
    }

    fn boundary(&self) -> Option<usize> {
        let b = &self.buf;
        // Line ends are natural breaks (list items, paragraphs).
        if let Some(nl) = b.find('\n') {
            return Some(nl + 1);
        }
        let bytes = b.as_bytes();
        for (i, &c) in bytes.iter().enumerate() {
            if matches!(c, b'.' | b'!' | b'?') {
                let next = bytes.get(i + 1).copied();
                // "3.5", "e.g." and "…" mid-sentence aren't ends; a space after is.
                if next == Some(b' ') && i >= 2 && !b[..i].ends_with("e.g") && !b[..i].ends_with("i.e") && !b[..i].ends_with(" vs") {
                    return Some(i + 2);
                }
            }
        }
        // A very long sentence: break at a comma so speech isn't held up.
        if b.len() > 220 {
            if let Some(c) = b[..200].rfind(", ") {
                return Some(c + 2);
            }
        }
        None
    }
}

#[cfg(windows)]
mod imp {
    use windows::core::HSTRING;
    use windows::Media::SpeechSynthesis::{SpeechSynthesizer, VoiceGender};
    use windows::Storage::Streams::DataReader;
    use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

    use super::{Speech, Voice};

    fn init() {
        // SAFETY: initializes WinRT on this thread; "already initialized" is fine.
        let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
    }

    fn e(err: windows::core::Error) -> String {
        format!("Windows speech failed: {}", err.message())
    }

    pub fn voices() -> Result<Vec<Voice>, String> {
        init();
        let all = SpeechSynthesizer::AllVoices().map_err(e)?;
        let mut out = Vec::new();
        for v in all {
            out.push(Voice {
                id: format!("system:{}", v.Id().map_err(e)?),
                name: v.DisplayName().map_err(e)?.to_string(),
                language: v.Language().map_err(e)?.to_string(),
                gender: if v.Gender().map_err(e)? == VoiceGender::Female { "female" } else { "male" }.into(),
                engine: "system",
                languages: Vec::new(),
            });
        }
        Ok(out)
    }

    pub fn synthesize(text: &str, voice_id: Option<&str>, rate: f64) -> Result<Speech, String> {
        init();
        let synth = SpeechSynthesizer::new().map_err(e)?;
        if let Some(id) = voice_id {
            for v in SpeechSynthesizer::AllVoices().map_err(e)? {
                if v.Id().map_err(e)?.to_string() == id {
                    synth.SetVoice(&v).map_err(e)?;
                    break;
                }
            }
        }
        if (rate - 1.0).abs() > 0.01 {
            synth.Options().and_then(|o| o.SetSpeakingRate(rate)).map_err(e)?;
        }
        let stream = synth.SynthesizeTextToStreamAsync(&HSTRING::from(text)).and_then(|op| op.get()).map_err(e)?;
        let size = stream.Size().map_err(e)? as u32;
        let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0).map_err(e)?).map_err(e)?;
        reader.LoadAsync(size).and_then(|op| op.get()).map_err(e)?;
        let mut bytes = vec![0u8; size as usize];
        reader.ReadBytes(&mut bytes).map_err(e)?;
        let pcm = crate::audio::wav_decode(&bytes)?;
        Ok(Speech { rate: pcm.rate, samples: pcm.samples.iter().map(|s| crate::audio::to_i16(*s)).collect() })
    }
}

/// macOS: the system's own voices through `say`. Linux: eSpeak NG when it's
/// installed (it's a separate program, run as one; nothing is bundled).
#[cfg(not(windows))]
mod imp {
    use std::process::Command;

    use super::{Speech, Voice};

    fn tmp_wav() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("sulcusai-voice-{}.wav", uuid::Uuid::new_v4().simple()))
    }

    fn read(path: &std::path::Path) -> Result<Speech, String> {
        let bytes = std::fs::read(path).map_err(|e| e.to_string());
        std::fs::remove_file(path).ok();
        let pcm = crate::audio::wav_decode(&bytes?)?;
        Ok(Speech { rate: pcm.rate, samples: pcm.samples.iter().map(|s| crate::audio::to_i16(*s)).collect() })
    }

    #[cfg(target_os = "macos")]
    pub fn voices() -> Result<Vec<Voice>, String> {
        // "Samantha            en_US    # Hello! My name is Samantha."
        let out = Command::new("say").args(["-v", "?"]).output().map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&out.stdout);
        let line = regex::Regex::new(r"^(.+?)\s+([a-z]{2,3}_[A-Za-z0-9]+)\s+#").unwrap();
        Ok(text
            .lines()
            .filter_map(|l| line.captures(l))
            .map(|c| Voice { id: c[1].trim().to_string(), name: c[1].trim().to_string(), language: c[2].replace('_', "-"), gender: String::new(), engine: "system", languages: Vec::new() })
            .collect())
    }

    #[cfg(target_os = "macos")]
    pub fn synthesize(text: &str, voice_id: Option<&str>, rate: f64) -> Result<Speech, String> {
        let path = tmp_wav();
        let mut cmd = Command::new("say");
        if let Some(v) = voice_id {
            cmd.args(["-v", v]);
        }
        let wpm = (175.0 * rate).round() as u32;
        let status = cmd
            .args(["-r", &wpm.to_string(), "--file-format=WAVE", "--data-format=LEI16@22050", "-o"])
            .arg(&path)
            .arg("--")
            .arg(text)
            .status()
            .map_err(|e| e.to_string())?;
        if !status.success() {
            return Err("The Mac's voice couldn't read that.".into());
        }
        read(&path)
    }

    #[cfg(not(target_os = "macos"))]
    fn espeak() -> Option<std::path::PathBuf> {
        crate::engine::on_path("espeak-ng").or_else(|| crate::engine::on_path("espeak"))
    }

    #[cfg(not(target_os = "macos"))]
    pub fn voices() -> Result<Vec<Voice>, String> {
        let Some(exe) = espeak() else { return Ok(Vec::new()) };
        // " 5  en-us           --/M      English_(America)  gmw/en-US  (en 2)"
        let out = Command::new(exe).arg("--voices").output().map_err(|e| e.to_string())?;
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .skip(1)
            .filter_map(|l| {
                let cols: Vec<&str> = l.split_whitespace().collect();
                (cols.len() >= 4).then(|| Voice {
                    id: cols[1].to_string(),
                    name: format!("eSpeak {}", cols[3].replace('_', " ")),
                    language: cols[1].to_string(),
                    gender: String::new(),
                    engine: "system",
                    languages: Vec::new(),
                })
            })
            .collect())
    }

    #[cfg(not(target_os = "macos"))]
    pub fn synthesize(text: &str, voice_id: Option<&str>, rate: f64) -> Result<Speech, String> {
        let exe = espeak().ok_or("No system voice is installed. Install natural voices in Settings, or eSpeak NG from your distribution.")?;
        let path = tmp_wav();
        let mut cmd = Command::new(exe);
        if let Some(v) = voice_id {
            cmd.args(["-v", v]);
        }
        let status = cmd.args(["-s", &((175.0 * rate).round() as u32).to_string(), "-w"]).arg(&path).arg("--").arg(text).status().map_err(|e| e.to_string())?;
        if !status.success() {
            return Err("The system voice couldn't read that.".into());
        }
        read(&path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_is_read_as_plain_words() {
        let md = "## Plan\n- **Buy** milk\n- Read [the guide](https://x.y/z) and `config.toml`\n\nSee https://example.com now.";
        assert_eq!(speakable(md), "Plan Buy milk Read the guide and config.toml See a link now.");
    }

    #[test]
    fn code_blocks_are_mentioned_not_read() {
        let md = "Run this:\n```bash\nrm -rf build\n```\nThen restart.";
        assert_eq!(speakable(md), "Run this: (There's a code block here.) Then restart.");
    }

    #[test]
    fn snake_case_keeps_its_underscores() {
        assert_eq!(speakable("call read_file now"), "call read_file now");
    }

    #[test]
    fn sentences_come_out_as_they_complete() {
        let mut s = Sentences::default();
        assert!(s.push("Hello there. How").len() == 1);
        assert_eq!(s.push(" are you? I'm"), vec!["How are you?".to_string()]);
        assert!(s.push(" fine, version 3.5 works").is_empty());
        assert_eq!(s.finish(), vec!["I'm fine, version 3.5 works".to_string()]);
    }

    #[test]
    fn list_lines_are_separate_sentences_and_code_is_skipped() {
        let mut s = Sentences::default();
        let mut all = s.push("Steps:\n- one\n- two\n```\nlet x = 1;\n");
        all.extend(s.push("```\nDone."));
        all.extend(s.finish());
        assert_eq!(all, vec!["Steps:", "one", "two", "(There's a code block here.)", "Done."]);
    }

    #[test]
    fn abbreviations_do_not_end_sentences() {
        let mut s = Sentences::default();
        assert!(s.push("Use a tool, e.g. a hammer").is_empty());
    }

    #[test]
    fn voice_choice_prefers_setting_then_language() {
        let v = |id: &str, lang: &str| Voice { id: id.into(), name: id.into(), language: lang.into(), gender: "female".into(), engine: "system", languages: Vec::new() };
        let mut voices = vec![v("a", "en-US"), v("b", "de-DE")];
        assert_eq!(pick_voice(&voices, Some("b"), None).unwrap().id, "b");
        assert_eq!(pick_voice(&voices, Some("gone"), Some("de")).unwrap().id, "b");
        assert_eq!(pick_voice(&voices, None, Some("fr")).unwrap().id, "a");
        // A natural voice wins when it speaks the language.
        voices.insert(0, Voice { engine: "natural", languages: vec!["en".into(), "de".into()], ..v("n", "multi") });
        assert_eq!(pick_voice(&voices, None, Some("de")).unwrap().id, "n");
        assert_eq!(pick_voice(&voices, None, None).unwrap().id, "n");
        assert_eq!(pick_voice(&voices, None, Some("zh")).unwrap().id, "a", "no Chinese voice: falls back to the first Windows one");
    }
}
