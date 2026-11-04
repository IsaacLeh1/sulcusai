// SPDX-License-Identifier: AGPL-3.0-only
//! Chat tools over the Find index and PDF page citations. Hits outside the
//! chat's shared folders are left out, the same as every other file tool.

use serde_json::{json, Value};

use super::{arg_str, Ctx, Outcome};

pub fn find_indexed_params() -> Value {
    json!({ "type": "object", "required": ["query"], "properties": {
        "query": { "type": "string", "description": "Words to look for in file names, document text and picture descriptions." },
        "kind": { "type": "string", "enum": ["Pictures", "Documents", "Music", "Videos", "Archives", "Installers", "Other"], "description": "Only files of this kind." } } })
}

pub fn cite_pdf_params() -> Value {
    json!({ "type": "object", "required": ["path", "question"], "properties": {
        "path": { "type": "string", "description": "The PDF, starting with the folder name." },
        "question": { "type": "string", "description": "What to find in it." } } })
}

pub fn run(name: &str, args: &Value, ctx: &Ctx<'_>) -> Outcome {
    let sb = ctx.sandbox;
    let result: Result<Outcome, String> = (|| match name {
        "find_indexed" => {
            let q = arg_str(args, "query")?;
            let kind = args.get("kind").and_then(Value::as_str);
            let hits = crate::files::search_kind(&ctx.state.paths.data, q, kind, 60);
            let lines: Vec<String> = hits
                .into_iter()
                .filter(|h| sb.resolve_read(&h.path).is_ok())
                .take(20)
                .map(|h| format!("{} ({}): {}", sb.display(std::path::Path::new(&h.path)), crate::size_label(h.size), h.snippet))
                .collect();
            let text = if lines.is_empty() { "Nothing found in the Find index (only folders added under Settings, Find, and shared with this chat are searched).".to_string() } else { lines.join("\n") };
            Ok(Outcome::ok(text, format!("Searched the Find index for {q}"), "text", None))
        }
        "cite_pdf" => {
            let path = sb.resolve_read(arg_str(args, "path")?)?;
            let found = crate::files::pdf::passages(&crate::docs::pdf_pages(&path)?, arg_str(args, "question")?, 8);
            let text = if found.is_empty() { "No passage in this PDF matches.".to_string() } else { format!("{}\n\nName the pages you use, as (page n).", crate::files::pdf::cited_context(&found)) };
            Ok(Outcome::ok(text, format!("Read {}", sb.display(&path)), "text", None))
        }
        _ => Err(format!("No tool {name}")),
    })();
    result.unwrap_or_else(|e| Outcome::error(super::failed_title(name, args), e))
}
