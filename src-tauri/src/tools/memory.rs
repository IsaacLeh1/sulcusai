// SPDX-License-Identifier: AGPL-3.0-only
//! remember and search_memory: the model's side of cross-chat memory.

use serde_json::{json, Value};

use super::{arg_str, MemoryCtx, Outcome};
use crate::memory;

pub fn remember_params() -> Value {
    json!({ "type": "object", "required": ["fact"], "properties": {
        "fact": { "type": "string", "description": "One short sentence, for example: The user prefers metric units." },
        "scope": { "type": "string", "enum": ["project", "everywhere"], "description": "In a project chat, 'project' keeps it to this project (default); 'everywhere' is for facts about the user." } } })
}

pub fn search_memory_params() -> Value {
    json!({ "type": "object", "required": ["query"], "properties": { "query": { "type": "string" } } })
}

pub fn run(name: &str, args: &Value, ctx: &MemoryCtx<'_>) -> Outcome {
    match name {
        "remember" => {
            let fact = match arg_str(args, "fact") {
                Ok(f) => f,
                Err(e) => return Outcome::error("Couldn't save a memory", e),
            };
            let everywhere = args.get("scope").and_then(Value::as_str) == Some("everywhere");
            let project = if everywhere { None } else { ctx.project_id };
            let conn = ctx.db.lock().unwrap();
            match memory::add(&conn, ctx.cipher, fact, project, Some(ctx.chat_id)) {
                Ok((m, new)) => {
                    let verb = if new { "Remembered" } else { "Updated memory" };
                    let mut o = Outcome::ok(format!("{verb}: {}", m.content), format!("{verb}: {}", m.content), "text", None);
                    o.meta["memory_id"] = json!(m.id);
                    o
                }
                Err(e) => Outcome::error("Couldn't save a memory", e),
            }
        }
        "search_memory" => {
            let query = args.get("query").and_then(Value::as_str).unwrap_or("");
            let conn = ctx.db.lock().unwrap();
            let all = memory::list(&conn, ctx.cipher, ctx.project_id);
            let hits = memory::relevant(&all, query);
            let text = if hits.is_empty() {
                "Nothing remembered yet.".to_string()
            } else {
                hits.iter().map(|m| format!("- {}", m.content)).collect::<Vec<_>>().join("\n")
            };
            Outcome::ok(text, format!("Searched memory for “{query}”"), "text", None)
        }
        other => Outcome::error(format!("Couldn't use {other}"), "Unknown memory tool."),
    }
}
