// SPDX-License-Identifier: AGPL-3.0-only
//! Character cards (the PNG and JSON files character chat apps share)
//! imported as assistants: the card's description, personality, scenario,
//! example dialogue and first message become the assistant's instructions.
//! Cards are read as data only; nothing in them runs.

use base64::Engine as _;
use serde_json::Value;

use crate::assistants::Assistant;

/// Cards larger than this aren't character cards.
const MAX_FILE: u64 = 20 * 1024 * 1024;
const MAX_INSTRUCTIONS: usize = 12_000;

/// The card JSON inside a PNG: a tEXt (or uncompressed iTXt) chunk named
/// "chara" (version 2) or "ccv3" (version 3), base64 encoded.
pub fn card_from_png(bytes: &[u8]) -> Option<Value> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    if !bytes.starts_with(SIGNATURE) {
        return None;
    }
    let mut at = SIGNATURE.len();
    let mut found: Option<(bool, Value)> = None;
    while at + 8 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[at..at + 4].try_into().ok()?) as usize;
        let kind = &bytes[at + 4..at + 8];
        let start = at + 8;
        let end = start.checked_add(len)?;
        if end + 4 > bytes.len() {
            break;
        }
        let data = &bytes[start..end];
        let text = match kind {
            b"tEXt" => Some(data),
            // iTXt: keyword, NUL, compression flag, method, language NUL, translated NUL, text.
            b"iTXt" => {
                let nul = data.iter().position(|b| *b == 0)?;
                let rest = data.get(nul + 1..)?;
                if rest.first() == Some(&0) {
                    let after = rest.get(2..)?;
                    let lang_end = after.iter().position(|b| *b == 0)?;
                    let after = after.get(lang_end + 1..)?;
                    let tr_end = after.iter().position(|b| *b == 0)?;
                    let mut out = data[..nul].to_vec();
                    out.push(0);
                    out.extend_from_slice(after.get(tr_end + 1..)?);
                    found_text(&out, &mut found);
                }
                None
            }
            b"IEND" => break,
            _ => None,
        };
        if let Some(t) = text {
            found_text(t, &mut found);
        }
        at = end + 4;
    }
    found.map(|(_, v)| v)
}

/// A "keyword NUL base64" chunk; version 3 wins over 2 when a card has both.
fn found_text(chunk: &[u8], found: &mut Option<(bool, Value)>) {
    let Some(nul) = chunk.iter().position(|b| *b == 0) else { return };
    let keyword = &chunk[..nul];
    let v3 = keyword.eq_ignore_ascii_case(b"ccv3");
    if !(v3 || keyword.eq_ignore_ascii_case(b"chara")) || found.as_ref().is_some_and(|(was_v3, _)| *was_v3 && !v3) {
        return;
    }
    let text: String = String::from_utf8_lossy(&chunk[nul + 1..]).chars().filter(|c| !c.is_whitespace()).collect();
    let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(text.as_bytes()) else { return };
    if let Ok(v) = serde_json::from_slice::<Value>(&raw) {
        *found = Some((v3, v));
    }
}

fn field<'a>(card: &'a Value, name: &str) -> &'a str {
    let data = if card.get("data").is_some_and(Value::is_object) { &card["data"] } else { card };
    data.get(name).and_then(Value::as_str).map(str::trim).unwrap_or("")
}

/// The card as an assistant (not yet saved).
pub fn assistant_from_card(card: &Value) -> Result<Assistant, String> {
    let name: String = field(card, "name").chars().take(60).collect();
    if name.is_empty() {
        return Err("That card has no character name.".into());
    }
    let fill = |s: &str| s.replace("{{char}}", &name).replace("{{user}}", "the user").replace("<USER>", "the user").replace("<BOT>", &name);
    let mut parts = vec![format!("Play the character {name} in this chat. Stay in character, write {name}'s replies only (never the user's), and keep the story consistent.")];
    for (label, key) in [("Who they are", "description"), ("Personality", "personality"), ("Setting", "scenario"), ("How they talk (examples)", "mes_example"), ("Also", "system_prompt"), ("After each reply", "post_history_instructions")] {
        let text = fill(field(card, key));
        if !text.is_empty() {
            parts.push(format!("{label}:\n{text}"));
        }
    }
    let first = fill(field(card, "first_mes"));
    if !first.is_empty() {
        parts.push(format!("When the chat begins, open with this (in your own words if it doesn't fit what the user said):\n{first}"));
    }
    let instructions: String = parts.join("\n\n").chars().take(MAX_INSTRUCTIONS).collect();
    let about = fill(field(card, "creator_notes"));
    let description: String = if about.is_empty() { fill(field(card, "description")) } else { about }.lines().next().unwrap_or("").chars().take(140).collect();
    let lore = lore_of(card, &fill);
    Ok(Assistant { id: String::new(), name, description, instructions, model: None, starters: vec!["Hello!".into()], lore })
}

/// The card's own lorebook (character_book), enabled entries only.
fn lore_of(card: &Value, fill: &dyn Fn(&str) -> String) -> Vec<crate::assistants::LoreEntry> {
    let data = if card.get("data").is_some_and(Value::is_object) { &card["data"] } else { card };
    data["character_book"]["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| e["enabled"].as_bool() != Some(false))
        .filter_map(|e| {
            let keys: Vec<String> = e["keys"].as_array()?.iter().filter_map(|k| k.as_str().map(str::to_string)).collect();
            let content = fill(e["content"].as_str()?.trim());
            (!keys.is_empty() && !content.is_empty()).then(|| crate::assistants::LoreEntry { keys, content: content.chars().take(2000).collect() })
        })
        .take(200)
        .collect()
}

/// Reads a .png or .json card from disk.
pub fn read_card(path: &std::path::Path) -> Result<Value, String> {
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > MAX_FILE {
        return Err("That file is too big to be a character card.".into());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if let Some(card) = card_from_png(&bytes) {
        return Ok(card);
    }
    serde_json::from_slice(&bytes).map_err(|_| "That file isn't a character card (a PNG with a card inside, or a card's JSON).".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn png_with(keyword: &str, card: &Value) -> Vec<u8> {
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut chunk = |kind: &[u8], data: &[u8]| {
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            out.extend_from_slice(kind);
            out.extend_from_slice(data);
            out.extend_from_slice(&[0, 0, 0, 0]);
        };
        chunk(b"IHDR", &[0; 13]);
        let mut text = keyword.as_bytes().to_vec();
        text.push(0);
        text.extend_from_slice(base64::engine::general_purpose::STANDARD.encode(card.to_string()).as_bytes());
        chunk(b"tEXt", &text);
        chunk(b"IEND", &[]);
        out
    }

    #[test]
    fn a_png_card_becomes_an_assistant() {
        let card = json!({ "spec": "chara_card_v2", "data": { "name": "Captain Vale", "description": "A retired sky-pirate who runs a tea shop.", "personality": "Gruff, kind, loves puns.", "scenario": "{{user}} walks into {{char}}'s shop.", "first_mes": "*looks up* Welcome, traveler.", "mes_example": "" } });
        let got = card_from_png(&png_with("chara", &card)).unwrap();
        let a = assistant_from_card(&got).unwrap();
        assert_eq!(a.name, "Captain Vale");
        assert!(a.instructions.contains("the user walks into Captain Vale's shop"));
        assert!(a.instructions.contains("Welcome, traveler.") && !a.instructions.contains("How they talk"));
        assert_eq!(a.description, "A retired sky-pirate who runs a tea shop.");
    }

    #[test]
    fn a_lorebook_comes_in_only_when_its_words_come_up() {
        let card = json!({ "data": { "name": "Vale", "character_book": { "entries": [
            { "keys": ["Skyport", "the port"], "content": "Skyport floats above the clouds; {{char}} was born there.", "enabled": true },
            { "keys": ["dragon"], "content": "Dragons are extinct.", "enabled": false },
            { "keys": [], "content": "No keys, never matched." }
        ] } } });
        let a = assistant_from_card(&card).unwrap();
        assert_eq!(a.lore.len(), 1, "disabled and keyless entries are left out");
        assert!(crate::assistants::lore_for(&a, "Tell me about skyport").contains("Vale was born there"));
        assert_eq!(crate::assistants::lore_for(&a, "What's for dinner?"), "");
    }

    #[test]
    fn version_three_wins_and_old_cards_still_read() {
        let mut png = png_with("chara", &json!({ "data": { "name": "Old" } }));
        let v3 = png_with("ccv3", &json!({ "spec": "chara_card_v3", "data": { "name": "New" } }));
        // Both chunks in one file: v3's tEXt goes before IEND.
        let iend = png.len() - 12;
        png.splice(iend..iend, v3[8 + 25..v3.len() - 12].iter().cloned());
        assert_eq!(field(&card_from_png(&png).unwrap(), "name"), "New");
        assert_eq!(assistant_from_card(&json!({ "name": "Flat", "description": "v1 card" })).unwrap().name, "Flat");
        assert!(assistant_from_card(&json!({ "data": {} })).is_err());
        assert!(card_from_png(b"not a png").is_none());
    }
}
