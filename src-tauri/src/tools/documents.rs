// SPDX-License-Identifier: AGPL-3.0-only
//! create_document and read_document, in the folders the user shared.

use serde_json::{json, Value};

use super::{arg_str, Ctx, Outcome, Preview};
use crate::docs::{self, Sheet, Slide};
use crate::sandbox::Sandbox;

pub fn create_document_params() -> Value {
    json!({ "type": "object", "required": ["path"], "properties": {
        "path": { "type": "string", "description": "Where to save it, ending in .docx, .pdf, .md, .xlsx or .pptx." },
        "markdown": { "type": "string", "description": "For .docx, .pdf and .md: the content in Markdown (# headings, **bold**, *italic*, - bullets, 1. lists, | tables |). For .xlsx it can be a table instead of sheets; for .pptx, # headings start slides." },
        "sheets": { "type": "array", "description": "For .xlsx: sheets, each {\"name\", \"rows\": [[cells]], \"charts\": [...]}. The first row is the header. Cells are text, numbers, or formulas starting with = such as \"=SUM(B2:B9)\". A chart is {\"type\": column|bar|line|pie|area, \"title\", \"categories\": \"A2:A9\", \"values\": [\"B2:B9\"], \"names\": [\"Sales\"]}.",
            "items": { "type": "object" } },
        "slides": { "type": "array", "description": "For .pptx: slides, each {\"title\", \"bullets\": [\"point\", \"  sub-point\"]}. A slide without bullets is a title slide and may have a \"subtitle\". Or skip this and give markdown where each # heading starts a slide.",
            "items": { "type": "object" } } } })
}

pub fn read_document_params() -> Value {
    json!({ "type": "object", "required": ["path"], "properties": {
        "path": { "type": "string", "description": "A .docx, .xlsx, .xls, .pptx, .pdf, .csv, .md or .txt file in a shared folder." } } })
}

fn kind(path: &str) -> Result<&'static str, String> {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "docx" => Ok("Word document"),
        "pdf" => Ok("PDF"),
        "md" | "markdown" => Ok("Markdown file"),
        "xlsx" => Ok("Excel spreadsheet"),
        "pptx" => Ok("PowerPoint deck"),
        other => Err(format!("I can make .docx, .pdf, .md, .xlsx and .pptx files, not .{other}.")),
    }
}

/// A list given as JSON, or as a JSON string (small models do both).
fn list(args: &Value, key: &str) -> Option<Value> {
    match args.get(key)? {
        Value::String(s) => serde_json::from_str(s).ok(),
        v @ Value::Array(a) if !a.is_empty() => Some(v.clone()),
        v @ Value::Object(_) => Some(json!([v])),
        _ => None,
    }
}

fn text_arg(args: &Value) -> Option<&str> {
    ["markdown", "content", "text", "csv"].iter().find_map(|k| args.get(*k).and_then(Value::as_str)).filter(|t| !t.trim().is_empty())
}

fn sheets(args: &Value) -> Result<Vec<Sheet>, String> {
    // Sheets with rows, else rows, else a table in the text.
    let given: Vec<Sheet> = list(args, "sheets").and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    let found: Vec<Sheet> = match given.iter().any(|s| !s.rows.is_empty()) {
        true => given,
        false => match list(args, "rows") {
            Some(Value::Array(rows)) => vec![Sheet { name: "Sheet1".into(), rows, charts: vec![] }],
            Some(_) => Vec::new(),
            None => text_arg(args).map(docs::sheets_from_text).unwrap_or_default(),
        },
    };
    if found.iter().all(|s| s.rows.is_empty()) {
        return Err("Give the spreadsheet as a simple table in markdown, one row per line, for example:\n| Item | Cost |\n| Rent | 1200 |\n| Total | =SUM(B2:B2) |".into());
    }
    Ok(found)
}

fn slides(args: &Value) -> Result<Vec<Slide>, String> {
    let found: Vec<Slide> = match list(args, "slides") {
        Some(v) => serde_json::from_value(v).map_err(|e| format!("The slides aren't in the expected shape: {e}"))?,
        None => text_arg(args).map(docs::slides_from_markdown).unwrap_or_default(),
    };
    if found.is_empty() {
        return Err("Give the slides: slides, or markdown where each # heading starts a slide and - bullets are its points.".into());
    }
    Ok(found)
}

fn markdown(args: &Value) -> Result<&str, String> {
    text_arg(args).ok_or_else(|| "Give the document's content as markdown.".to_string())
}

/// What the approval card shows: the content, in a readable form.
pub fn preview(args: &Value, sb: &Sandbox) -> Result<Preview, String> {
    let raw = arg_str(args, "path")?;
    let what = kind(raw)?;
    let path = sb.resolve(raw)?;
    let label = sb.display(&path);
    let detail = match raw.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "xlsx" => sheets(args)?
            .iter()
            .map(|s| {
                let rows: Vec<String> = s.rows.iter().take(15).map(|r| docs::row_cells(r).iter().map(|c| c.as_str().map(str::to_string).unwrap_or_else(|| c.to_string())).collect::<Vec<_>>().join(" | ")).collect();
                let charts = if s.charts.is_empty() { String::new() } else { format!("\n+ {} chart(s)", s.charts.len()) };
                format!("Sheet {}:\n{}{charts}", s.name, rows.join("\n"))
            })
            .collect::<Vec<_>>()
            .join("\n\n"),
        "pptx" => slides(args)?.iter().enumerate().map(|(i, s)| format!("{}. {}\n{}", i + 1, s.title, s.bullets.iter().map(|b| format!("   • {}", b.trim())).collect::<Vec<_>>().join("\n"))).collect::<Vec<_>>().join("\n"),
        _ => markdown(args)?.chars().take(4000).collect(),
    };
    Ok(Preview {
        title: format!("{} {label}", if path.exists() { "Replace" } else { "Create" }),
        kind: "text",
        detail: Some(detail),
        note: Some(format!("A {what}. You can undo it from the chat.")),
    })
}

pub fn run(name: &str, args: &Value, ctx: &Ctx<'_>) -> Outcome {
    let r: Result<Outcome, String> = (|| {
        let sb = ctx.sandbox;
        let raw = arg_str(args, "path")?;
        let path = sb.resolve(raw)?;
        let label = sb.display(&path);
        match name {
            "create_document" => {
                let what = kind(raw)?;
                let existed = path.exists();
                ctx.recorder.before_write(&path)?;
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                }
                match raw.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
                    "docx" => docs::write_docx(&path, markdown(args)?)?,
                    "pdf" => docs::write_pdf(&path, markdown(args)?)?,
                    "xlsx" => docs::write_xlsx(&path, &sheets(args)?)?,
                    "pptx" => docs::write_pptx(&path, &slides(args)?)?,
                    _ => std::fs::write(&path, markdown(args)?).map_err(|e| e.to_string())?,
                }
                let verb = if existed { "Replaced" } else { "Created" };
                let mut o = Outcome::ok(format!("{verb} the {what} {label}."), format!("{verb} {label}"), "text", None);
                o.meta["file"] = json!(path.display().to_string());
                Ok(o)
            }
            "read_document" => {
                let text = docs::read_text(&path)?;
                let preview: String = text.chars().take(1500).collect();
                Ok(Outcome::ok(format!("{label}:\n\n{text}"), format!("Read {label}"), "text", Some(preview)))
            }
            other => Err(format!("Unknown tool {other}.")),
        }
    })();
    r.unwrap_or_else(|e| Outcome::error(super::failed_title(name, args), e))
}
