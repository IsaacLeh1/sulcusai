// SPDX-License-Identifier: AGPL-3.0-only
//! Skills made from chats: a chat that worked out how to do something
//! becomes instructions the assistant follows next time. The model drafts
//! them, the person reads and edits the draft, and saved skills live in a
//! "My skills" plugin of their own, used like any plugin's skills.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::connectors::{Plugin, Skill};
use crate::{db, AppState, AppStateRef};

const PLUGIN_ID: &str = "own-skills";
/// The transcript the draft is made from, newest kept when it's longer.
const TRANSCRIPT_CHARS: usize = 16_000;

fn format() -> Value {
    let schema = json!({ "type": "object", "properties": {
        "name": { "type": "string" },
        "description": { "type": "string" },
        "instructions": { "type": "string" }
    }, "required": ["name", "description", "instructions"] });
    json!({ "type": "json_schema", "json_schema": { "name": "skill", "schema": schema } })
}

/// The chat as plain turns, cut from the start to fit.
fn transcript(messages: &[db::Message]) -> String {
    let mut out = String::new();
    for m in messages.iter().rev().filter(|m| (m.role == "user" || m.role == "assistant") && !m.content.trim().is_empty()) {
        let who = if m.role == "user" { "User" } else { "Assistant" };
        let piece = format!("{who}: {}\n\n", m.content.trim());
        if out.chars().count() + piece.chars().count() > TRANSCRIPT_CHARS {
            break;
        }
        out.insert_str(0, &piece);
    }
    out
}

fn ask(transcript: &str) -> Vec<Value> {
    vec![
        json!({ "role": "system", "content": "You turn a chat into a reusable skill: instructions an assistant follows the next time the user wants the same kind of thing done. Write: name (2 to 5 words), description (one sentence saying when to use it), instructions (numbered steps and rules that worked in the chat, including the user's preferences and corrections, the format they wanted and what to avoid). Leave out this chat's one-off details, names, numbers, private information and anything secret. Write the instructions to the assistant, in the second person." }),
        json!({ "role": "user", "content": format!("The chat:\n\n{transcript}") }),
    ]
}

#[derive(Deserialize)]
struct Draft {
    name: String,
    description: String,
    instructions: String,
}

fn parse(raw: &str) -> Result<Skill, String> {
    let start = raw.find('{').ok_or("The model didn't write a skill.")?;
    let end = raw.rfind('}').ok_or("The model didn't write a skill.")?;
    let d: Draft = serde_json::from_str(&raw[start..=end]).map_err(|_| "The model's skill couldn't be read. Try again.".to_string())?;
    checked(Skill { name: d.name, description: d.description, body: d.instructions, dir: None, files: Vec::new() })
}

/// Trimmed and within limits, or why not.
fn checked(mut s: Skill) -> Result<Skill, String> {
    s.name = s.name.trim().chars().take(60).collect();
    s.description = s.description.trim().chars().take(240).collect();
    s.body = s.body.trim().to_string();
    if s.name.is_empty() {
        return Err("Give the skill a name.".into());
    }
    if s.body.is_empty() {
        return Err("The skill needs instructions.".into());
    }
    if s.body.chars().count() > 8_000 {
        return Err("Keep a skill's instructions under 8,000 characters.".into());
    }
    if crate::memory::looks_secret(&s.body) {
        return Err("The instructions look like they hold a password or key. Take it out first.".into());
    }
    Ok(s)
}

/// Adds or replaces (same name) a skill in "My skills", making it when needed.
fn add(conn: &rusqlite::Connection, c: &crate::crypto::Cipher, data: &std::path::Path, skill: Skill) -> Result<Plugin, String> {
    let mut plugin = crate::connectors::plugins(conn, c).into_iter().find(|p| p.id == PLUGIN_ID).unwrap_or_else(|| Plugin {
        id: PLUGIN_ID.into(),
        name: "My skills".into(),
        version: String::new(),
        description: "Skills made from your chats.".into(),
        enabled: true,
        dir: data.join("skills").display().to_string(),
        skills: Vec::new(),
        connectors: Vec::new(),
        checks: Vec::new(),
    });
    plugin.skills.retain(|s| !s.name.eq_ignore_ascii_case(&skill.name));
    plugin.skills.push(skill);
    crate::connectors::store(conn, c, "plugins", PLUGIN_ID, &plugin)?;
    Ok(plugin)
}

/// A draft skill from a chat, for the person to read and change before saving.
#[tauri::command]
pub async fn skill_from_chat(state: AppStateRef<'_>, chat_id: String) -> Result<Skill, String> {
    let state: Arc<AppState> = state.inner().clone();
    let c = state.cipher()?;
    let messages = db::messages(&state.db.lock().unwrap(), &c, &chat_id);
    let text = transcript(&messages);
    if text.trim().is_empty() {
        return Err("This chat has nothing to learn from yet.".into());
    }
    let (ep, _spec, _hold) = crate::turns::endpoint_for(&state, None).await?;
    let raw = crate::chat::complete(&ep, ask(&text), json!({ "response_format": format(), "temperature": 0.2 }), 1200).await?;
    parse(&raw)
}

#[tauri::command(async)]
pub fn save_own_skill(state: AppStateRef, skill: Skill) -> Result<(), String> {
    let c = state.cipher()?;
    let skill = checked(skill)?;
    let name = skill.name.clone();
    add(&state.db.lock().unwrap(), &c, &state.paths.data, skill)?;
    state.log("skills", &format!("Saved the skill “{name}” from a chat"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On a real model (SULCUSAI_TEST_GGUF): a chat becomes a skill that
    /// keeps the person's corrections and drops their private details.
    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore]
    async fn e2e_a_chat_becomes_a_usable_skill() {
        let (_engine, ep, name) = crate::e2e::test_model().await;
        let msg = |role: &str, content: &str| db::Message { role: role.into(), content: content.into(), ..Default::default() };
        let chat = [
            msg("user", "Write my weekly status update for my manager Dana. This week: shipped the login page, fixed 3 bugs, next week the billing page."),
            msg("assistant", "Hi Dana,\n\nThis week I shipped the login page and fixed three bugs. Next week I'll start on the billing page.\n\nBest"),
            msg("user", "Shorter, use bullet points, and always end with a Risks line, even if it's 'none'."),
            msg("assistant", "- Shipped the login page\n- Fixed 3 bugs\n- Next: billing page\nRisks: none"),
        ];
        let raw = crate::chat::complete(&ep, ask(&transcript(&chat)), json!({ "response_format": format(), "temperature": 0.2, "chat_template_kwargs": { "enable_thinking": false } }), 1200).await.unwrap();
        let skill = parse(&raw).unwrap();
        println!("{name}: {}\n{}\n{}", skill.name, skill.description, skill.body);
        let body = skill.body.to_lowercase();
        assert!(body.contains("bullet") && body.contains("risk"), "keeps the corrections");
    }

    #[test]
    fn a_draft_is_read_checked_and_saved_in_my_skills() {
        let raw = "Here it is: {\"name\": \" Weekly report \", \"description\": \"Use for the Friday report.\", \"instructions\": \"1. Start with wins.\\n2. Keep it under 200 words.\"}";
        let s = parse(raw).unwrap();
        assert_eq!(s.name, "Weekly report");
        assert!(s.body.starts_with("1. Start with wins."));
        assert!(parse("no json here").is_err());

        let conn = rusqlite::Connection::open_in_memory().unwrap();
        db::init_for_test(&conn);
        let c = crate::crypto::Cipher::random();
        let data = std::env::temp_dir();
        add(&conn, &c, &data, s.clone()).unwrap();
        let mut again = s.clone();
        again.body = "Changed.".into();
        let plugin = add(&conn, &c, &data, again).unwrap();
        assert_eq!(plugin.skills.len(), 1, "same name replaces");
        assert_eq!(crate::connectors::active_skills(&conn, &c)[0].body, "Changed.");
    }

    #[test]
    fn the_transcript_keeps_the_newest_turns() {
        let msg = |role: &str, content: &str| db::Message { role: role.into(), content: content.into(), ..Default::default() };
        let long = "x".repeat(TRANSCRIPT_CHARS);
        let t = transcript(&[msg("user", &long), msg("assistant", "the answer"), msg("tool", "ignored")]);
        assert!(t.contains("Assistant: the answer") && !t.contains("ignored") && !t.contains(&long));
    }
}
