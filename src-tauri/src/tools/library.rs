// SPDX-License-Identifier: AGPL-3.0-only
//! search_documents: passages from the person's documents library, each
//! with a link the chat shows as its source (`doc:<file id>`), which opens
//! the file.

use serde_json::{json, Value};

use super::{Ctx, Outcome};

pub fn search_params() -> Value {
    json!({
        "type": "object",
        "properties": {
            "query": { "type": "string", "description": "What to find: the question, or its key words and names." },
            "limit": { "type": "integer", "description": "How many passages (default 6, at most 12)." }
        },
        "required": ["query"]
    })
}

/// "Lease.pdf, page 3".
fn source_name(name: &str, place: Option<&str>) -> String {
    match place {
        Some(p) => format!("{name}, {p}"),
        None => name.to_string(),
    }
}

pub async fn search(args: &Value, ctx: &Ctx<'_>) -> Outcome {
    let query = args.get("query").and_then(Value::as_str).unwrap_or("").trim();
    if query.is_empty() {
        return Outcome::error("Search your documents", "Say what to look for.");
    }
    let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(6).clamp(1, 12) as usize;
    let title = format!("Searched your documents for “{}”", query.chars().take(60).collect::<String>());
    match crate::doclib::search(ctx.state, query, limit).await {
        Ok(hits) if hits.is_empty() => Outcome::ok(
            "Nothing in the documents library matches that. Say so plainly; don't guess what the documents say. Suggest other words, or ask which document it's in.",
            title,
            "text",
            Some("Nothing matched.".into()),
        ),
        Ok(hits) => {
            let mut text = String::from("Passages from the user's documents, best first. Answer from these only, and after each fact put its source link exactly as given:\n\n");
            let mut sources = Vec::new();
            for (i, h) in hits.iter().enumerate() {
                let name = source_name(&h.name, h.place.as_deref());
                let link = format!("[{name}](doc:{})", h.file_id);
                text.push_str(&format!("[{}] {link}\n{}\n\n", i + 1, h.text));
                sources.push(json!({ "title": name, "url": format!("doc:{}", h.file_id), "snippet": h.text.chars().take(160).collect::<String>() }));
            }
            let detail = hits.iter().map(|h| source_name(&h.name, h.place.as_deref())).collect::<Vec<_>>().join("\n");
            let mut o = Outcome::ok(text, title, "text", Some(detail));
            o.meta["sources"] = json!(sources);
            o
        }
        Err(e) => Outcome::error("Couldn't search your documents", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_name_their_place() {
        assert_eq!(source_name("Lease.pdf", Some("page 3")), "Lease.pdf, page 3");
        assert_eq!(source_name("notes.md", None), "notes.md");
    }
}
