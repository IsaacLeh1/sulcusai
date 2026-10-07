// SPDX-License-Identifier: AGPL-3.0-only
//! Translation with the local model: text, text-based documents (keeping
//! their formatting), and live meeting captions.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{json, Value};

use crate::chat;
use crate::engine::Endpoint;
use crate::AppStateRef;

/// Languages offered in the app (any language the model knows works).
pub const LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"), ("es", "Spanish"), ("fr", "French"), ("de", "German"), ("it", "Italian"),
    ("pt", "Portuguese"), ("nl", "Dutch"), ("pl", "Polish"), ("ru", "Russian"), ("uk", "Ukrainian"),
    ("tr", "Turkish"), ("ar", "Arabic"), ("he", "Hebrew"), ("hi", "Hindi"), ("zh", "Chinese (Simplified)"),
    ("zh-TW", "Chinese (Traditional)"), ("ja", "Japanese"), ("ko", "Korean"), ("vi", "Vietnamese"),
    ("th", "Thai"), ("id", "Indonesian"), ("sv", "Swedish"), ("nb", "Norwegian"), ("da", "Danish"),
    ("fi", "Finnish"), ("cs", "Czech"), ("el", "Greek"), ("ro", "Romanian"), ("hu", "Hungarian"),
];

pub fn language_name(code: &str) -> String {
    LANGUAGES.iter().find(|(c, n)| c.eq_ignore_ascii_case(code) || n.eq_ignore_ascii_case(code)).map_or(code.to_string(), |(_, n)| n.to_string())
}

fn system(to: &str) -> String {
    format!(
        "You are a professional translator. Translate the user's text into {}. Keep the meaning, tone, names, numbers \
         and formatting (Markdown, line breaks, lists). If it is already in {0}, return it unchanged. Reply with only \
         the translation: no notes, no quotes.",
        language_name(to)
    )
}

/// Splits long text at paragraph breaks into pieces of about `max` chars.
fn pieces(text: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for para in text.split_inclusive("\n\n") {
        if cur.len() + para.len() > max && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        cur.push_str(para);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

pub async fn translate_text(ep: &Endpoint, text: &str, to: &str) -> Result<String, String> {
    if text.trim().is_empty() {
        return Ok(String::new());
    }
    // Room for the piece twice (in and out) within the window, ~3.5 chars a token.
    let max_chars = ((ep.ctx as usize).saturating_sub(800) / 2 * 3).max(1500);
    let mut out = String::new();
    for piece in pieces(text, max_chars) {
        let trailing: String = piece.chars().rev().take_while(|c| c.is_whitespace()).collect::<Vec<_>>().into_iter().rev().collect();
        let messages = vec![json!({ "role": "system", "content": system(to) }), json!({ "role": "user", "content": piece.trim() })];
        let max_tokens = (piece.len() as u32 / 2).clamp(256, 6000);
        out.push_str(&chat::complete(ep, messages, json!({ "temperature": 0.2 }), max_tokens).await?);
        out.push_str(&trailing);
    }
    Ok(out.trim_end().to_string() + if text.ends_with('\n') { "\n" } else { "" })
}

/// Translates short lines one-for-one; the schema makes the model return
/// exactly as many lines as it was given.
pub async fn translate_lines(ep: &Endpoint, lines: &[String], to: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::with_capacity(lines.len());
    for batch in lines.chunks(20) {
        let n = batch.len();
        let schema = json!({ "type": "object", "required": ["lines"], "properties": { "lines": {
            "type": "array", "minItems": n, "maxItems": n, "items": { "type": "string" } } } });
        let messages = vec![
            json!({ "role": "system", "content": system(to) }),
            json!({ "role": "user", "content": format!(
                "Translate each line. Answer in JSON as {{\"lines\": [...]}} with exactly {n} lines, in the same order.\n\n{}",
                serde_json::to_string(batch).unwrap_or_default()) }),
        ];
        let extra = json!({ "temperature": 0.2, "response_format": { "type": "json_schema", "json_schema": { "name": "lines", "schema": schema } } });
        let reply = chat::complete(ep, messages, extra, 3000).await?;
        let parsed: Option<Vec<String>> = reply
            .find('{')
            .and_then(|s| reply.rfind('}').map(|e| &reply[s..=e]))
            .and_then(|j| serde_json::from_str::<Value>(j).ok())
            .and_then(|v| serde_json::from_value(v["lines"].clone()).ok())
            .filter(|l: &Vec<String>| l.len() == n);
        match parsed {
            Some(l) => out.extend(l),
            None => {
                // Fall back to one line at a time.
                for line in batch {
                    out.push(translate_text(ep, line, to).await?.replace('\n', " "));
                }
            }
        }
    }
    Ok(out)
}

pub async fn detect_language(ep: &Endpoint, text: &str) -> Result<String, String> {
    let sample: String = text.chars().take(600).collect();
    let messages = vec![
        json!({ "role": "system", "content": "Identify the language of the user's text. Answer with only the language's name in English." }),
        json!({ "role": "user", "content": sample }),
    ];
    let reply = chat::complete(ep, messages, json!({ "temperature": 0.0 }), 20).await?;
    Ok(reply.trim().trim_end_matches('.').to_string())
}

/// Subtitle files: cue numbers and timings stay as they are; only the words
/// are translated.
fn is_cue_meta(line: &str) -> bool {
    let t = line.trim();
    t.is_empty() || t.chars().all(|c| c.is_ascii_digit()) || t.contains("-->") || t == "WEBVTT" || t.starts_with("NOTE")
}

async fn translate_subtitles(ep: &Endpoint, text: &str, to: &str) -> Result<String, String> {
    let lines: Vec<&str> = text.lines().collect();
    let idx: Vec<usize> = (0..lines.len()).filter(|i| !is_cue_meta(lines[*i])).collect();
    let words: Vec<String> = idx.iter().map(|i| lines[*i].to_string()).collect();
    let translated = translate_lines(ep, &words, to).await?;
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    for (k, i) in idx.iter().enumerate() {
        out[*i] = translated[k].clone();
    }
    Ok(out.join("\n") + "\n")
}

/// `name.ext` → `name.<lang>.ext` beside it, never replacing a file.
fn output_path(input: &Path, to: &str) -> PathBuf {
    let stem = input.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "translation".into());
    let ext = input.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_else(|| "txt".into());
    let dir = input.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut path = dir.join(format!("{stem}.{to}.{ext}"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem}.{to} ({n}).{ext}"));
        n += 1;
    }
    path
}

const FILE_TYPES: &[&str] = &["txt", "md", "markdown", "srt", "vtt", "csv", "html", "htm", "json"];

// ---------- commands ----------

#[derive(Serialize)]
pub struct Translation {
    text: String,
    detected: Option<String>,
}

#[tauri::command]
pub async fn translate(state: AppStateRef<'_>, text: String, to: String, detect: bool) -> Result<Translation, String> {
    state.cipher()?;
    crate::features::require(&state, crate::features::Feature::Translate)?;
    let (ep, _) = crate::background_endpoint(state.inner()).await?;
    let detected = if detect { detect_language(&ep, &text).await.ok() } else { None };
    let out = translate_text(&ep, &text, &to).await?;
    state.log("translate", &format!("Translated text into {} on this PC", language_name(&to)));
    Ok(Translation { text: out, detected })
}

#[tauri::command]
pub fn translation_languages() -> Vec<Value> {
    LANGUAGES.iter().map(|(c, n)| json!({ "code": c, "name": n })).collect()
}

/// Translates a text document the user picked and saves the result beside
/// it. Returns the new file's path.
#[tauri::command]
pub async fn translate_file(state: AppStateRef<'_>, path: String, to: String) -> Result<String, String> {
    state.cipher()?;
    crate::features::require(&state, crate::features::Feature::Translate)?;
    let input = PathBuf::from(&path);
    let ext = input.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if !FILE_TYPES.contains(&ext.as_str()) {
        return Err(format!(
            "Translating .{ext} files isn't supported yet. Text, Markdown, subtitle (.srt, .vtt), CSV, HTML and JSON files work; \
             Word, PowerPoint and PDF come with the documents features."
        ));
    }
    let meta = std::fs::metadata(&input).map_err(|e| format!("Couldn't open the file: {e}"))?;
    if meta.len() > 2 * 1024 * 1024 {
        return Err("That file is over 2 MB, which is too long to translate in one go.".into());
    }
    let text = std::fs::read_to_string(&input).map_err(|_| "That file isn't readable text (UTF-8).".to_string())?;
    let (ep, _) = crate::background_endpoint(state.inner()).await?;
    let out = match ext.as_str() {
        "srt" | "vtt" => translate_subtitles(&ep, &text, &to).await?,
        _ => translate_text(&ep, &text, &to).await?,
    };
    let dest = output_path(&input, &to);
    std::fs::write(&dest, out).map_err(|e| format!("Couldn't save the translation: {e}"))?;
    state.log("translate", &format!("Translated a .{ext} file into {} and saved it beside the original", language_name(&to)));
    Ok(dest.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_resolve_from_codes_and_names() {
        assert_eq!(language_name("de"), "German");
        assert_eq!(language_name("japanese"), "Japanese");
        assert_eq!(language_name("tlh"), "tlh");
    }

    #[test]
    fn long_text_splits_at_paragraphs() {
        let text = "a".repeat(800) + "\n\n" + &"b".repeat(800) + "\n\n" + &"c".repeat(100);
        let p = pieces(&text, 1000);
        assert_eq!(p.len(), 2);
        assert!(p[0].starts_with('a') && p[0].ends_with("\n\n"));
        assert_eq!(p.concat(), text);
    }

    #[test]
    fn subtitle_timing_lines_are_kept() {
        assert!(is_cue_meta("12"));
        assert!(is_cue_meta("00:00:01,000 --> 00:00:02,500"));
        assert!(is_cue_meta("WEBVTT"));
        assert!(!is_cue_meta("Hello there."));
    }

    #[test]
    fn output_never_replaces_a_file() {
        let d = std::env::temp_dir().join(format!("sulcusai-tr-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        let input = d.join("notes.md");
        assert_eq!(output_path(&input, "de"), d.join("notes.de.md"));
        std::fs::write(d.join("notes.de.md"), "x").unwrap();
        assert_eq!(output_path(&input, "de"), d.join("notes.de (2).md"));
    }
}
