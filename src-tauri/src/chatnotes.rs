// SPDX-License-Identifier: AGPL-3.0-only
//! Chat notes: a chat's own memory, kept apart from its messages. What the
//! user asked for and what's still to do, what the chat knows about the
//! project, and its plan, step by step ("- [ ] step", details under it),
//! checked off as it goes. The model reads them every turn and keeps them
//! up to date with its tools; when a long chat is compacted, the summary of
//! the earlier part goes here too, so what was behind it isn't lost. The
//! user opens them from the chat's menu (Notes…) and can edit them.

use rusqlite::Connection;
use serde_json::{json, Value};

use crate::crypto::Cipher;
use crate::db;
use crate::tools::{Ctx, Outcome};
use crate::AppStateRef;

const PREFIX: &str = "chat_notes:";
/// What notes may hold (characters); past it the oldest parts of a section go.
const MOST: usize = 16_000;
/// The section compaction keeps its summary in.
pub const EARLIER: &str = "Earlier in this chat";

fn key(chat_id: &str) -> String {
    format!("{PREFIX}{chat_id}")
}

pub fn get(conn: &Connection, c: &Cipher, chat_id: &str) -> String {
    db::get::<String>(conn, &key(chat_id)).map(|s| c.decrypt_or(&s, "")).unwrap_or_default()
}

pub fn set(conn: &Connection, c: &Cipher, chat_id: &str, text: &str) -> Result<(), String> {
    let text: String = text.trim().chars().take(MOST).collect();
    if text.is_empty() {
        conn.execute("DELETE FROM settings WHERE key = ?1", [key(chat_id)]).map_err(|e| e.to_string())?;
        return Ok(());
    }
    db::set(conn, &key(chat_id), &c.encrypt(&text))
}

/// Forgets a deleted chat's notes.
pub fn forget(conn: &Connection, chat_id: &str) {
    let _ = conn.execute("DELETE FROM settings WHERE key = ?1", [key(chat_id)]);
}

/// The notes with `## heading`'s section replaced by `body` (added at the
/// end when there isn't one), or with `body` added to its end (`add`).
pub fn with_section(notes: &str, heading: &str, body: &str, add: bool) -> String {
    let lines: Vec<&str> = notes.lines().collect();
    let is_heading = |l: &str| l.trim_start().starts_with("## ");
    let wanted = heading.trim().trim_start_matches('#').trim().to_lowercase();
    let start = lines.iter().position(|l| is_heading(l) && l.trim_start().trim_start_matches('#').trim().to_lowercase() == wanted);
    let body = body.trim();
    match start {
        Some(s) => {
            let end = lines[s + 1..].iter().position(|l| is_heading(l)).map_or(lines.len(), |e| s + 1 + e);
            let old = lines[s + 1..end].join("\n");
            let new_body = if add && !old.trim().is_empty() { format!("{}\n{body}", old.trim_end()) } else { body.to_string() };
            let mut out: Vec<String> = lines[..=s].iter().map(|l| l.to_string()).collect();
            out.push(new_body);
            if end < lines.len() {
                out.push(String::new());
                out.extend(lines[end..].iter().map(|l| l.to_string()));
            }
            out.join("\n").trim().to_string()
        }
        None => {
            let gap = if notes.trim().is_empty() { "" } else { "\n\n" };
            format!("{}{gap}## {}\n{body}", notes.trim_end(), heading.trim().trim_start_matches('#').trim())
        }
    }
}

/// A plan step checked off (or not): by its number among the steps ("2")
/// or by words in it. The notes changed, and the step's text.
pub fn check(notes: &str, step: &str, done: bool) -> Result<(String, String), String> {
    let mut lines: Vec<String> = notes.lines().map(str::to_string).collect();
    let is_step = |l: &str| {
        let t = l.trim_start();
        t.starts_with("- [ ]") || t.starts_with("- [x]") || t.starts_with("- [X]") || t.starts_with("* [ ]") || t.starts_with("* [x]")
    };
    let steps: Vec<usize> = (0..lines.len()).filter(|&i| is_step(&lines[i])).collect();
    if steps.is_empty() {
        return Err("The notes have no plan steps yet. Add a plan first with update_chat_notes (lines like \"- [ ] step\").".into());
    }
    let want = step.trim().trim_end_matches('.').to_lowercase();
    let at = match want.parse::<usize>() {
        Ok(n) if n >= 1 && n <= steps.len() => steps[n - 1],
        Ok(n) => return Err(format!("There's no step {n}: the plan has {} steps.", steps.len())),
        Err(_) => {
            let words: Vec<&str> = want.split_whitespace().filter(|w| w.len() > 2).collect();
            let score = |i: usize| {
                let l = lines[i].to_lowercase();
                words.iter().filter(|w| l.contains(*w)).count()
            };
            let best = steps.iter().copied().max_by_key(|&i| score(i)).filter(|&i| score(i) > 0).ok_or_else(|| format!("No plan step matches \"{step}\". Use its number."))?;
            best
        }
    };
    let line = &lines[at];
    let indent = line.len() - line.trim_start().len();
    let text = line.trim_start()[5..].trim().to_string();
    lines[at] = format!("{}- [{}] {text}", &line[..indent], if done { "x" } else { " " });
    Ok((lines.join("\n"), text))
}

/// The notes without a section (its heading and what's under it).
fn without_section(notes: &str, heading: &str) -> String {
    let lines: Vec<&str> = notes.lines().collect();
    let is_heading = |l: &str| l.trim_start().starts_with("## ");
    let Some(s) = lines.iter().position(|l| is_heading(l) && l.trim_start().trim_start_matches('#').trim().eq_ignore_ascii_case(heading)) else {
        return notes.to_string();
    };
    let end = lines[s + 1..].iter().position(|l| is_heading(l)).map_or(lines.len(), |e| s + 1 + e);
    [&lines[..s], &lines[end..]].concat().join("\n").trim().to_string()
}

/// For the model, with the newest message: the notes, and what they're for.
/// The summary of earlier messages is left out (the instructions carry it).
pub fn prompt_section(notes: &str) -> String {
    let notes = without_section(notes, EARLIER);
    if notes.trim().is_empty() {
        return "\n\n[This chat's notes are empty so far. Keep in them what the user asks for and what's left, what you learn about the \
                project, and for bigger work a plan of steps (update_chat_notes; check_plan_step as steps get done).]"
            .into();
    }
    let body = notes.trim().to_string();
    format!(
        "\n\n## This chat's notes\nThe chat's own memory, which you and the user keep: what the user asked for and what's still to do, what you \
         know about the project, and for bigger work a plan of steps (\"- [ ] step\", with what it involves under each). Keep them up to date \
         with update_chat_notes as you learn things or the user asks for something, and check steps off with check_plan_step as you finish \
         them. They stay when older messages are summarized.\n{body}"
    )
}

// ---------- tools ----------

pub fn update_params() -> Value {
    json!({ "type": "object", "required": ["text"], "properties": {
        "section": { "type": "string", "description": "The heading to write under, e.g. \"Asked for\", \"About the project\", \"Plan\" (made if it isn't there). Leave it out to replace all the notes." },
        "text": { "type": "string", "description": "What goes there, as Markdown. A plan is a list of steps like \"- [ ] Add the pnl command\", each with indented lines saying what it involves." },
        "add": { "type": "boolean", "description": "Add to the end of the section instead of replacing it." } } })
}

pub fn check_params() -> Value {
    json!({ "type": "object", "required": ["step"], "properties": {
        "step": { "type": "string", "description": "The step's number in the plan (\"2\") or words from it." },
        "done": { "type": "boolean", "description": "true (the default) checks it off; false unchecks it." } } })
}

fn changed(ctx: &Ctx<'_>) {
    if let Some(app) = ctx.app {
        use tauri::Emitter;
        app.emit("chat:notes", json!({ "chat_id": ctx.memory.chat_id })).ok();
    }
}

pub fn read(ctx: &Ctx<'_>) -> Outcome {
    let notes = get(&ctx.memory.db.lock().unwrap(), ctx.memory.cipher, ctx.memory.chat_id);
    let text = if notes.trim().is_empty() { "This chat has no notes yet.".to_string() } else { notes };
    Outcome::ok(text, "Read the chat's notes", "text", None)
}

pub fn update(args: &Value, ctx: &Ctx<'_>) -> Outcome {
    let title = "Updated the chat's notes";
    let text = args.get("text").and_then(Value::as_str).unwrap_or_default();
    if text.trim().is_empty() {
        return Outcome::error(title, "Write what goes in the notes (text).");
    }
    let conn = ctx.memory.db.lock().unwrap();
    let old = get(&conn, ctx.memory.cipher, ctx.memory.chat_id);
    let new = match args.get("section").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()) {
        Some(section) => with_section(&old, section, text, args.get("add").and_then(Value::as_bool).unwrap_or(false)),
        None if args.get("add").and_then(Value::as_bool).unwrap_or(false) => format!("{}\n\n{}", old.trim_end(), text.trim()).trim().to_string(),
        None => text.trim().to_string(),
    };
    if let Err(e) = set(&conn, ctx.memory.cipher, ctx.memory.chat_id, &new) {
        return Outcome::error(title, e);
    }
    drop(conn);
    changed(ctx);
    let section = args.get("section").and_then(Value::as_str).map(|s| format!(" ({s})")).unwrap_or_default();
    Outcome::ok(format!("Saved in the chat's notes{section}."), format!("{title}{section}"), "text", None)
}

pub fn check_step(args: &Value, ctx: &Ctx<'_>) -> Outcome {
    let step = args.get("step").map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).unwrap_or_default();
    let done = args.get("done").and_then(Value::as_bool).unwrap_or(true);
    let conn = ctx.memory.db.lock().unwrap();
    let old = get(&conn, ctx.memory.cipher, ctx.memory.chat_id);
    match check(&old, &step, done) {
        Ok((new, text)) => {
            if let Err(e) = set(&conn, ctx.memory.cipher, ctx.memory.chat_id, &new) {
                return Outcome::error("Check off a step", e);
            }
            drop(conn);
            changed(ctx);
            let verb = if done { "Checked off" } else { "Unchecked" };
            Outcome::ok(format!("{verb}: {text}."), format!("{verb} “{text}”"), "text", None)
        }
        Err(e) => Outcome::error("Check off a step", e),
    }
}

// ---------- commands ----------

#[tauri::command(async)]
pub fn chat_notes(state: AppStateRef, chat_id: String) -> Result<String, String> {
    let c = state.cipher()?;
    Ok(get(&state.db.lock().unwrap(), &c, &chat_id))
}

/// The user's edit of a chat's notes.
#[tauri::command(async)]
pub fn set_chat_notes(state: AppStateRef, chat_id: String, text: String) -> Result<String, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    set(&conn, &c, &chat_id, &text)?;
    db::log_action(&conn, "chat", "Edited a chat's notes");
    Ok(get(&conn, &c, &chat_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_are_replaced_or_added() {
        let notes = "## Asked for\n- a pnl command\n\n## Plan\n- [ ] Read the code\n- [ ] Add pnl";
        let more = with_section(notes, "Asked for", "- a test for it", true);
        assert_eq!(more, "## Asked for\n- a pnl command\n- a test for it\n\n## Plan\n- [ ] Read the code\n- [ ] Add pnl");
        let replaced = with_section(notes, "plan", "- [ ] Write the test", false);
        assert!(replaced.ends_with("## Plan\n- [ ] Write the test") && replaced.contains("a pnl command"), "{replaced}");
        let new = with_section(notes, "About the project", "Node 24, no dependencies.", false);
        assert!(new.ends_with("## About the project\nNode 24, no dependencies."), "{new}");
        assert_eq!(with_section("", "Plan", "- [ ] One", false), "## Plan\n- [ ] One");
    }

    #[test]
    fn plan_steps_are_checked_off_by_number_or_words() {
        let notes = "## Plan\n- [ ] Read the code\n  - index.js and report.js\n- [ ] Add the pnl command\n- [ ] Write its test";
        let (n, t) = check(notes, "2", true).unwrap();
        assert_eq!(t, "Add the pnl command");
        assert!(n.contains("- [x] Add the pnl command") && n.contains("- [ ] Read the code") && n.contains("  - index.js"), "{n}");
        let (n, _) = check(&n, "the test", true).unwrap();
        assert!(n.contains("- [x] Write its test"));
        let (n, _) = check(&n, "2", false).unwrap();
        assert!(n.contains("- [ ] Add the pnl command"));
        assert!(check(notes, "9", true).is_err() && check("no plan here", "1", true).is_err());
    }
}
