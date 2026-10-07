// SPDX-License-Identifier: AGPL-3.0-only
//! Note and task tools: the assistant can jot notes, find them, and keep
//! the user's to-do list.

use chrono::Local;
use serde_json::{json, Value};

use super::{arg_str, MemoryCtx, Outcome, Preview};
use crate::notes::{self, NoteBody};

pub fn create_note_params() -> Value {
    json!({ "type": "object", "required": ["body"], "properties": {
        "title": { "type": "string" },
        "body": { "type": "string", "description": "Markdown." },
        "folder": { "type": "string" },
        "tags": { "type": "array", "items": { "type": "string" } } } })
}

pub fn search_notes_params() -> Value {
    json!({ "type": "object", "required": ["query"], "properties": { "query": { "type": "string", "description": "Words to find. Empty lists recent notes." } } })
}

pub fn read_note_params() -> Value {
    json!({ "type": "object", "required": ["note_id"], "properties": { "note_id": { "type": "string" } } })
}

pub fn update_note_params() -> Value {
    json!({ "type": "object", "required": ["note_id", "body"], "properties": {
        "note_id": { "type": "string" },
        "title": { "type": "string", "description": "Leave out to keep the title." },
        "body": { "type": "string", "description": "The whole new text (Markdown)." } } })
}

pub fn create_task_params() -> Value {
    json!({ "type": "object", "required": ["title"], "properties": {
        "title": { "type": "string" },
        "due": { "type": "string", "description": "As the user said it, e.g. \"Friday\", \"tomorrow 3pm\", \"2026-11-02\"." },
        "priority": { "type": "string", "enum": ["none", "low", "medium", "high"] },
        "notes": { "type": "string" },
        "remind": { "type": "boolean", "description": "Remind the user when it's due." } } })
}

pub fn list_tasks_params() -> Value {
    json!({ "type": "object", "properties": { "include_done": { "type": "boolean" } } })
}

pub fn complete_task_params() -> Value {
    json!({ "type": "object", "required": ["task_id"], "properties": { "task_id": { "type": "string" } } })
}

fn when(ms: i64, with_time: bool) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.with_timezone(&Local).format(if with_time { "%a %b %-d, %-I:%M %p" } else { "%a %b %-d" }).to_string())
        .unwrap_or_default()
}

pub fn preview(name: &str, args: &Value, ctx_note: Option<notes::Note>) -> Result<Preview, String> {
    match name {
        "update_note" => {
            let n = ctx_note.ok_or("No note has that id. Use search_notes to find it.")?;
            let new = args.get("body").and_then(Value::as_str).unwrap_or("");
            Ok(Preview {
                title: format!("Rewrite the note “{}”", n.data.title),
                kind: "diff",
                detail: Some(crate::tools::text_diff(&n.data.body, new)),
                note: None,
            })
        }
        _ => Err("No preview for this tool.".into()),
    }
}

pub fn run(name: &str, args: &Value, ctx: &MemoryCtx<'_>) -> Outcome {
    let conn = ctx.db.lock().unwrap();
    let c = ctx.cipher;
    match name {
        "create_note" => {
            let body = NoteBody {
                title: args.get("title").and_then(Value::as_str).unwrap_or("").into(),
                body: args.get("body").and_then(Value::as_str).unwrap_or("").into(),
                folder: args.get("folder").and_then(Value::as_str).unwrap_or("").into(),
                tags: args.get("tags").and_then(|t| serde_json::from_value(t.clone()).ok()).unwrap_or_default(),
            };
            match notes::save_note(&conn, c, None, body) {
                Ok(n) => {
                    let mut o = Outcome::ok(format!("Saved the note “{}” (id {}).", n.data.title, n.id), format!("Saved a note: {}", n.data.title), "text", None);
                    o.meta["note_id"] = json!(n.id);
                    o
                }
                Err(e) => Outcome::error("Couldn't save a note", e),
            }
        }
        "search_notes" => {
            let q = args.get("query").and_then(Value::as_str).unwrap_or("");
            let hits = notes::search_notes(&conn, c, q);
            let text = if hits.is_empty() {
                "No notes match.".to_string()
            } else {
                hits.iter()
                    .take(10)
                    .map(|n| {
                        let preview: String = n.data.body.chars().take(160).collect::<String>().replace('\n', " ");
                        format!("- {} (id {}){}: {preview}", n.data.title, n.id, if n.data.folder.is_empty() { String::new() } else { format!(" in {}", n.data.folder) })
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            Outcome::ok(text, format!("Searched notes for “{q}”"), "text", None)
        }
        "read_note" => {
            let id = args.get("note_id").and_then(Value::as_str).unwrap_or("");
            match notes::get_note(&conn, c, id) {
                Some(n) => Outcome::ok(format!("# {}\n\n{}", n.data.title, n.data.body), format!("Read the note “{}”", n.data.title), "text", None),
                None => Outcome::error("Couldn't read a note", "No note has that id. Use search_notes to find it."),
            }
        }
        "update_note" => {
            let id = match arg_str(args, "note_id") {
                Ok(i) => i,
                Err(e) => return Outcome::error("Couldn't change a note", e),
            };
            let Some(old) = notes::get_note(&conn, c, id) else {
                return Outcome::error("Couldn't change a note", "No note has that id.");
            };
            let mut body = old.data.clone();
            body.body = args.get("body").and_then(Value::as_str).unwrap_or("").into();
            if let Some(t) = args.get("title").and_then(Value::as_str).filter(|t| !t.trim().is_empty()) {
                body.title = t.into();
            }
            match notes::save_note(&conn, c, Some(id), body) {
                Ok(n) => Outcome::ok(format!("Updated the note “{}”.", n.data.title), format!("Changed the note “{}”", n.data.title), "diff", Some(crate::tools::text_diff(&old.data.body, &n.data.body))),
                Err(e) => Outcome::error("Couldn't change a note", e),
            }
        }
        "create_task" => {
            let title = args.get("title").and_then(Value::as_str).unwrap_or("");
            let mut t = notes::new_task(title);
            t.data.notes = args.get("notes").and_then(Value::as_str).unwrap_or("").into();
            t.priority = match args.get("priority").and_then(Value::as_str) {
                Some("low") => 1,
                Some("medium") => 2,
                Some("high") => 3,
                _ => 0,
            };
            let due_text = args.get("due").and_then(Value::as_str).unwrap_or("");
            if let Some((d, tm)) = notes::parse_due(due_text, Local::now().date_naive()) {
                t.due = notes::to_ms(d, tm);
                t.due_has_time = tm.is_some();
                if args.get("remind").and_then(Value::as_bool).unwrap_or(false) {
                    // Date-only reminders go off at 9 in the morning.
                    t.remind_at = notes::to_ms(d, Some(tm.unwrap_or(chrono::NaiveTime::from_hms_opt(9, 0, 0).unwrap())));
                }
            } else if !due_text.is_empty() {
                t.data.notes = format!("{}\nDue: {due_text}", t.data.notes).trim().to_string();
            }
            match notes::save_task(&conn, c, t) {
                Ok(t) => {
                    let due = t.due.map(|d| format!(", due {}", when(d, t.due_has_time))).unwrap_or_default();
                    Outcome::ok(format!("Added the task “{}”{due} (id {}).", t.data.title, t.id), format!("Added a task: {}{due}", t.data.title), "text", None)
                }
                Err(e) => Outcome::error("Couldn't add a task", e),
            }
        }
        "list_tasks" => {
            let all = args.get("include_done").and_then(Value::as_bool).unwrap_or(false);
            let list: Vec<notes::Task> = notes::list_tasks(&conn, c).into_iter().filter(|t| all || t.done_at.is_none()).collect();
            let text = if list.is_empty() {
                "No tasks.".to_string()
            } else {
                list.iter()
                    .take(50)
                    .map(|t| {
                        let mut s = format!("- [{}] {} (id {})", if t.done_at.is_some() { "x" } else { " " }, t.data.title, t.id);
                        if let Some(d) = t.due {
                            s.push_str(&format!(", due {}", when(d, t.due_has_time)));
                        }
                        if t.priority > 0 {
                            s.push_str(&format!(", {} priority", ["", "low", "medium", "high"][t.priority as usize]));
                        }
                        s
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            Outcome::ok(text, "Looked at the task list", "text", None)
        }
        "complete_task" => {
            let id = args.get("task_id").and_then(Value::as_str).unwrap_or("");
            let Some(mut t) = notes::get_task(&conn, c, id) else {
                return Outcome::error("Couldn't complete a task", "No task has that id. Use list_tasks to find it.");
            };
            t.done_at = Some(crate::db::now_ms());
            match notes::save_task(&conn, c, t) {
                Ok(t) => Outcome::ok(format!("Marked “{}” done.", t.data.title), format!("Completed: {}", t.data.title), "text", None),
                Err(e) => Outcome::error("Couldn't complete a task", e),
            }
        }
        other => Outcome::error(format!("Couldn't use {other}"), "Unknown notes tool."),
    }
}
