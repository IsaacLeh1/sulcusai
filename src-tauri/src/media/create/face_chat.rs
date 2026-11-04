// SPDX-License-Identifier: AGPL-3.0-only
//! Voice chat with a face: each spoken reply is read aloud and a picture is
//! moved to it (Ditto, quick talk). Replies are cut at sentence ends so the
//! face starts moving on the first sentence instead of after the whole reply.

use serde::{Deserialize, Serialize};

use super::consent::{Claim, Likeness};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Settings {
    /// Off until turned on.
    #[serde(default)]
    pub on: bool,
    pub picture: Option<String>,
    #[serde(default)]
    pub claim: Claim,
}

/// What's needed before a face can talk; `ditto` is whether it's installed.
pub fn ready(s: &Settings, ditto: bool, notice_ok: bool) -> Result<(), String> {
    if !s.on {
        return Err("Voice chat with a face is off.".into());
    }
    let pic = s.picture.as_deref().ok_or("Choose a picture for the face first.")?;
    if pic.is_empty() {
        return Err("Choose a picture for the face first.".into());
    }
    if !ditto {
        return Err("A talking face needs Ditto. Install it from the Studio's Models list.".into());
    }
    // A face nobody said who it is counts as a real person's.
    let mut c = s.claim.clone();
    c.face.get_or_insert(Likeness::Someone);
    super::consent::check(&c, notice_ok)
}

/// Splits streamed text into pieces to speak: whole sentences, at least
/// `min` characters, the leftover kept for the next call.
pub fn take_sentences(buf: &mut String, min: usize) -> Vec<String> {
    let mut out = Vec::new();
    loop {
        let cut = buf.char_indices().find(|&(i, c)| {
            matches!(c, '.' | '!' | '?' | '\n') && i + 1 >= min && buf[i + c.len_utf8()..].starts_with(|n: char| n.is_whitespace())
        });
        let Some((i, c)) = cut else { break };
        let end = i + c.len_utf8();
        let s = buf[..end].trim().to_string();
        *buf = buf[end..].trim_start().to_string();
        if !s.is_empty() {
            out.push(s);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn needs_on_picture_ditto_and_consent() {
        let mut s = Settings::default();
        assert!(ready(&s, true, true).is_err());
        s.on = true;
        s.picture = Some("me.png".into());
        assert!(ready(&s, false, true).unwrap_err().contains("Ditto"));
        assert!(ready(&s, true, true).unwrap_err().contains("permission"));
        s.claim.face = Some(Likeness::Nobody);
        assert!(ready(&s, true, false).is_ok());
        s.claim.face = Some(Likeness::Myself);
        assert!(ready(&s, true, false).is_err());
    }
    #[test]
    fn sentences_stream() {
        let mut b = String::from("Hi. This is a test! And more");
        assert_eq!(take_sentences(&mut b, 8), vec!["Hi. This is a test!"]);
        assert_eq!(b, "And more");
        b.push_str(" text. ");
        assert_eq!(take_sentences(&mut b, 1), vec!["And more text."]);
        let mut d = String::from("3.5 is fine");
        assert!(take_sentences(&mut d, 1).is_empty());
    }
}
