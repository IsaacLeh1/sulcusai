// SPDX-License-Identifier: AGPL-3.0-only
//! search_meetings and read_meeting: the assistant can look up what was
//! said in the user's recorded meetings.

use serde_json::{json, Value};

use super::{MemoryCtx, Outcome};
use crate::meeting;

/// The most transcript text returned at once.
const PAGE: usize = 12_000;

pub fn search_meetings_params() -> Value {
    json!({ "type": "object", "required": ["query"], "properties": {
        "query": { "type": "string", "description": "Words to look for, e.g. a topic, a name or a project. Empty lists recent meetings." } } })
}

pub fn read_meeting_params() -> Value {
    json!({ "type": "object", "required": ["meeting_id"], "properties": {
        "meeting_id": { "type": "string", "description": "The id from search_meetings." },
        "part": { "type": "string", "enum": ["notes", "transcript"], "description": "notes (default) or the full transcript." },
        "offset": { "type": "integer", "description": "For long transcripts: where to continue, from the previous result." } } })
}

fn date(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.with_timezone(&chrono::Local).format("%a %b %-d, %Y %-I:%M %p").to_string())
        .unwrap_or_default()
}

pub fn run(name: &str, args: &Value, ctx: &MemoryCtx<'_>) -> Outcome {
    let conn = ctx.db.lock().unwrap();
    match name {
        "search_meetings" => {
            let query = args.get("query").and_then(Value::as_str).unwrap_or("");
            let hits = meeting::search(&conn, ctx.cipher, query, 8);
            if hits.is_empty() {
                return Outcome::ok("No meetings match.", format!("Searched meetings for “{query}”"), "text", None);
            }
            let text = hits
                .iter()
                .map(|(m, lines)| {
                    let mut s = format!("- {} ({}) id: {}", m.data.title, date(m.started_at), m.id);
                    if let Some(n) = &m.data.notes {
                        s.push_str(&format!("\n  Summary: {}", n.summary));
                    }
                    for l in lines {
                        s.push_str(&format!("\n  {l}"));
                    }
                    s
                })
                .collect::<Vec<_>>()
                .join("\n");
            Outcome::ok(text, format!("Searched meetings for “{query}”"), "text", None)
        }
        "read_meeting" => {
            let id = args.get("meeting_id").and_then(Value::as_str).unwrap_or("");
            let Some(m) = meeting::list(&conn, ctx.cipher).into_iter().find(|m| m.id == id) else {
                return Outcome::error("Couldn't read a meeting", "No meeting has that id. Use search_meetings to find it.");
            };
            let title = format!("Read “{}”", m.data.title);
            if args.get("part").and_then(Value::as_str) == Some("transcript") {
                let segs = meeting::segments(&conn, ctx.cipher, &m.id);
                let full = meeting::transcript_text(&segs);
                let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
                let start = full.char_indices().map(|(i, _)| i).find(|i| *i >= offset).unwrap_or(full.len());
                let end = full[start..].char_indices().map(|(i, _)| start + i).find(|i| *i >= start + PAGE).unwrap_or(full.len());
                let mut text = format!("Transcript of {} ({}):\n{}", m.data.title, date(m.started_at), &full[start..end]);
                if end < full.len() {
                    text.push_str(&format!("\n[More follows. Call read_meeting again with offset {end}.]"));
                }
                return Outcome::ok(text, title, "text", None);
            }
            let notes = meeting::notes_markdown(&m, None);
            Outcome::ok(notes, title, "text", None)
        }
        other => Outcome::error(format!("Couldn't use {other}"), "Unknown meeting tool."),
    }
}
