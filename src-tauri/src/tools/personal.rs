// SPDX-License-Identifier: AGPL-3.0-only
//! Personal life tools: the assistant can answer "when does my package
//! come?" or "log 30 minutes of running" from the Personal records.

use serde_json::{json, Value};

use super::{MemoryCtx, Outcome};
use crate::personal;

pub fn personal_records_params() -> Value {
    json!({ "type": "object", "required": ["kind"], "properties": {
        "kind": { "type": "string", "enum": personal::KINDS },
        "query": { "type": "string", "description": "Words to find. Empty lists the newest." } } })
}

pub fn personal_add_params() -> Value {
    json!({ "type": "object", "required": ["kind", "fields"], "properties": {
        "kind": { "type": "string", "enum": personal::KINDS },
        "date": { "type": "string", "description": "YYYY-MM-DD the record is about. Leave out for now." },
        "fields": { "type": "object", "description": "For example {\"what\": \"run\", \"minutes\": 30} or {\"name\": \"Fern\", \"every_days\": 3}." } } })
}

fn date_ms(s: &str) -> Option<i64> {
    let d = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?;
    Some(d.and_hms_opt(12, 0, 0)?.and_utc().timestamp_millis())
}

pub fn run(name: &str, args: &Value, ctx: &MemoryCtx<'_>) -> Outcome {
    let conn = ctx.db.lock().unwrap();
    if !personal::enabled(&conn) {
        return Outcome::error("Personal life is off", "Personal life is turned off. It can be turned on in the Personal page.");
    }
    let kind = args.get("kind").and_then(Value::as_str).unwrap_or("");
    match name {
        "personal_records" => {
            let q = args.get("query").and_then(Value::as_str).unwrap_or("").to_lowercase();
            let rows = personal::list(&conn, ctx.cipher, kind);
            let lines: Vec<String> = rows
                .iter()
                .filter(|r| q.is_empty() || r.data.to_string().to_lowercase().contains(&q))
                .take(20)
                .map(|r| {
                    let day = chrono::DateTime::from_timestamp_millis(r.at).map(|d| d.date_naive().to_string()).unwrap_or_default();
                    let mut data = r.data.clone();
                    if let Some(o) = data.as_object_mut() {
                        o.remove("mail_id");
                    }
                    format!("- {day}: {data}")
                })
                .collect();
            let text = if lines.is_empty() { format!("No {kind} records match.") } else { lines.join("\n") };
            Outcome::ok(text, format!("Looked at personal records ({kind})"), "text", None)
        }
        "personal_add" => {
            let fields = args.get("fields").cloned().unwrap_or_else(|| json!({}));
            if !fields.is_object() {
                return Outcome::error("Couldn't add a record", "fields must be an object.");
            }
            let at = args.get("date").and_then(Value::as_str).and_then(date_ms).unwrap_or_else(crate::db::now_ms);
            match personal::save(&conn, ctx.cipher, None, kind, at, &fields) {
                Ok(r) => Outcome::ok(format!("Added a {kind} record (id {}).", r.id), format!("Added to Personal: {kind}"), "text", None),
                Err(e) => Outcome::error("Couldn't add a record", e),
            }
        }
        _ => Outcome::error("Unknown tool", name.to_string()),
    }
}
