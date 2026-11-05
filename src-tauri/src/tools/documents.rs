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
        "path": { "type": "string", "description": "A .docx, .xlsx, .xls, .pptx, .pdf, .csv, .md or .txt file in a shared folder (or a .png/.jpg for ocr and look)." },
        "mode": { "type": "string", "enum": ["text", "structure", "ocr", "look", "picture"],
            "description": "text (default): the text, with a coverage report of what it doesn't hold. structure: pages, headings, figure/table/equation mentions and flagged pages, cheap, for a first look. ocr: read flagged or chosen pages as pictures with Windows' text reader (scans). look: a picture-reading model on this PC reads chosen pages, equations as LaTeX. picture: the page itself as a picture for you to see." },
        "pages": { "type": "string", "description": "PDF pages, like 3, 2-4 or 1,3,5 (at most 6 for ocr, look and picture). Without it, ocr and look take the flagged pages." } } })
}

/// What a model that sees pictures is asked about a page.
const LOOK_ASK: &str = "Transcribe this document page exactly. Write every equation in LaTeX between $ signs, keep the reading order, \
describe each figure or chart in one line in [brackets] with any numbers it shows, and write tables as Markdown tables. Don't summarize.";

/// The pages a picture read takes: the ones asked for, else the flagged
/// ones (pages the text doesn't hold), else the first.
fn picked(args: &Value, count: u32, flagged: &[u32], most: usize) -> Result<Vec<u32>, String> {
    match args.get("pages").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
        Some(spec) => crate::pages::parse_pages(spec, count, most),
        None if !flagged.is_empty() => Ok(flagged.iter().copied().take(most).collect()),
        None => Ok(vec![1]),
    }
}

/// A picture file or rendered PDF pages, as (page, PNG/JPEG bytes).
fn page_pictures(path: &std::path::Path, ext: &str, args: &Value, most: usize) -> Result<Vec<(u32, Vec<u8>)>, String> {
    if matches!(ext, "png" | "jpg" | "jpeg" | "bmp" | "tif" | "tiff" | "gif") {
        return Ok(vec![(1, std::fs::read(path).map_err(|e| e.to_string())?)]);
    }
    if ext != "pdf" {
        return Err("ocr, look and picture work on PDFs and pictures. For Word, PowerPoint or Excel, use mode text (Word equations come back as LaTeX), or save it as a PDF first.".into());
    }
    let count = crate::pages::pdf_page_count(path).ok_or("Windows couldn't open this PDF to draw its pages. Use mode text, and say which parts couldn't be read.")?;
    let flagged: Vec<u32> = crate::coverage::pdf(path, &docs::pdf_pages(path).unwrap_or_default()).iter().filter_map(|g| g.page).collect();
    picked(args, count, &flagged, most)?.into_iter().map(|p| crate::pages::render_pdf_page(path, p, 1600).map(|png| (p, png))).collect()
}

fn kind(path: &str) -> Result<&'static str, String> {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "docx" => Ok("Word document"),
        "pdf" => Ok("PDF"),
        "md" | "markdown" => Ok("Markdown file"),
        "xlsx" => Ok("Excel spreadsheet"),
        "pptx" => Ok("PowerPoint deck"),
        _ => Err(format!(
            "create_document makes Word, PDF, Markdown, Excel and PowerPoint files (.docx, .pdf, .md, .xlsx, .pptx), and “{path}” isn't one. \
             For a website, an app or code (.html, .css, .js, .py…), use write_file instead, one file at a time (for example site/index.html)."
        )),
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
                // Windows says only "Access is denied", which sends a small
                // model off asking for another folder.
                if path.is_dir() {
                    return Err(format!("{label} is a folder, not a file. Pick a file name inside it, such as {label}/{}.", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
                }
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
                let ext0 = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
                match args.get("mode").and_then(Value::as_str).unwrap_or("text") {
                    "text" | "" => {}
                    "structure" => return structure(&path, &ext0, &label),
                    "ocr" => {
                        let mut out = Vec::new();
                        for (p, pic) in page_pictures(&path, &ext0, args, 6)? {
                            let t = crate::pages::ocr(&pic)?;
                            out.push(format!("--- page {p} (text read from the picture; equations and symbols may be wrong, use mode look for those) ---\n{}", if t.trim().is_empty() { "[no text found]" } else { t.trim() }));
                        }
                        let text = out.join("\n\n");
                        let mut o = Outcome::ok(format!("{label}, read as pictures:\n\n{text}"), format!("Read {label} as pictures"), "text", Some(text.chars().take(1500).collect()));
                        o.meta["coverage"] = json!("complete");
                        return Ok(o);
                    }
                    "look" => {
                        let pics = page_pictures(&path, &ext0, args, 4)?;
                        let images: Vec<image::DynamicImage> = pics.iter().filter_map(|(_, b)| image::load_from_memory(b).ok()).collect();
                        let pages: Vec<String> = pics.iter().map(|(p, _)| p.to_string()).collect();
                        // Local readers only: a page goes to a cloud model only with the person's yes, and this call doesn't ask.
                        let read = tokio::runtime::Handle::current().block_on(crate::vision::read(ctx.state, &images, LOOK_ASK, None, 3000));
                        let text = match read {
                            Ok(t) => format!("{label}, pages {} read by a picture-reading model on this PC:\n\n{t}", pages.join(", ")),
                            Err(e) => {
                                let ocr: Vec<String> = pics.iter().map(|(p, b)| format!("--- page {p} ---\n{}", crate::pages::ocr(b).unwrap_or_default())).collect();
                                format!(
                                    "No picture-reading model could read it ({e}). Windows' text reader got this instead; equations may be missing or wrong, so say so if they matter:\n\n{}",
                                    ocr.join("\n\n")
                                )
                            }
                        };
                        return Ok(Outcome::ok(text.clone(), format!("Looked at {label}"), "text", Some(text.chars().take(1500).collect())));
                    }
                    "picture" => {
                        let pics = page_pictures(&path, &ext0, args, 2)?;
                        let urls: Vec<String> = pics.iter().filter_map(|(_, b)| image::load_from_memory(b).ok()).filter_map(|i| crate::vision::data_url(&i).ok()).collect();
                        let pages: Vec<String> = pics.iter().map(|(p, _)| p.to_string()).collect();
                        let mut o = Outcome::ok(
                            format!("Page(s) {} of {label} follow as a picture. If you can't see it, use mode look instead.", pages.join(", ")),
                            format!("Showed {label} page {}", pages.join(", ")),
                            "text",
                            None,
                        );
                        o.meta["tool_images"] = json!(urls);
                        return Ok(o);
                    }
                    other => return Err(format!("mode “{other}” isn't one of text, structure, ocr, look or picture.")),
                }
                let mut text = match (ext0.as_str(), args.get("pages").and_then(Value::as_str).filter(|s| !s.trim().is_empty())) {
                    ("pdf", Some(spec)) => {
                        let all = docs::pdf_pages(&path)?;
                        let chosen = crate::pages::parse_pages(spec, all.len() as u32, 200)?;
                        chosen.iter().map(|p| format!("--- page {p} ---\n{}", all[*p as usize - 1].trim())).collect::<Vec<_>>().join("\n\n")
                    }
                    _ => docs::read_text(&path)?,
                };
                if ext0 == "docx" {
                    // Word equations aren't in the plain text (hypotheses, formulas).
                    let eqs: Vec<String> = word_xml(&path).map(|x| crate::coverage::office_math_latex(&x)).unwrap_or_default().into_iter().filter(|e| !e.trim().is_empty()).collect();
                    if !eqs.is_empty() {
                        text.push_str("\n\n[Equations in this document (Word equations as LaTeX, in order; each sits where its line was left blank above):");
                        for (i, e) in eqs.iter().enumerate() {
                            text.push_str(&format!("\n{}. {e}", i + 1));
                        }
                        text.push(']');
                    }
                }
                let preview: String = text.chars().take(1500).collect();
                // What the text doesn't hold (equations, figures, scans), so
                // the model knows its read was partial.
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
                let gaps = match ext.as_str() {
                    "pdf" => crate::coverage::pdf(&path, &docs::pdf_pages(&path).unwrap_or_default()),
                    "docx" => word_xml(&path).map(|x| crate::coverage::docx(&x)).unwrap_or_default(),
                    _ => Vec::new(),
                };
                let cut = text.ends_with("[The document goes on.]").then_some(docs::MAX_READ);
                let report = crate::coverage::report(&gaps, cut);
                let mut o = Outcome::ok(format!("{label}:\n\n{text}{}", report.as_deref().unwrap_or("")), format!("Read {label}"), "text", Some(preview));
                o.meta["coverage"] = json!(if report.is_some() { "partial" } else { "complete" });
                Ok(o)
            }
            other => Err(format!("Unknown tool {other}.")),
        }
    })();
    r.unwrap_or_else(|e| Outcome::error(super::failed_title(name, args), e))
}

/// The document's shape before a full read: pages, headings, mentions of
/// figures, tables and equations, and the parts the text doesn't hold.
fn structure(path: &std::path::Path, ext: &str, label: &str) -> Result<Outcome, String> {
    let (parts, gaps) = match ext {
        "pdf" => {
            let pages = docs::pdf_pages(path)?;
            let gaps = crate::coverage::pdf(path, &pages);
            (pages, gaps)
        }
        "docx" => (vec![docs::read_text(path)?], word_xml(path).map(|x| crate::coverage::docx(&x)).unwrap_or_default()),
        _ => (vec![docs::read_text(path)?], Vec::new()),
    };
    let mut lines = vec![format!("{label}: {} page(s) or part(s)", parts.len())];
    lines.extend(crate::pages::outline(&parts).into_iter().take(200));
    if gaps.is_empty() {
        lines.push("Nothing found that the text doesn't hold: mode text reads it all.".into());
    } else {
        lines.push("Not in the text (read these with mode ocr or look and pages):".into());
        lines.extend(gaps.iter().map(|g| match g.page {
            Some(p) => format!("- page {p}: {}", g.what),
            None => format!("- {}", g.what),
        }));
    }
    let text = lines.join("\n");
    Ok(Outcome::ok(text.clone(), format!("Looked over {label}"), "text", Some(text.chars().take(1500).collect())))
}

/// A Word file's main XML, for what a text read leaves out.
fn word_xml(path: &std::path::Path) -> Option<String> {
    use std::io::Read;
    let f = std::fs::File::open(path).ok()?;
    let mut zip = zip::ZipArchive::new(f).ok()?;
    let entry = zip.by_name("word/document.xml").ok()?;
    let mut s = String::new();
    entry.take(64 << 20).read_to_string(&mut s).ok()?;
    Some(s)
}
