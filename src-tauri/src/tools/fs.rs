// SPDX-License-Identifier: AGPL-3.0-only
//! File tools. Every path goes through the sandbox; every change is
//! recorded for undo before it happens.

use std::fmt::Write as _;
use std::path::Path;

use serde_json::{json, Value};
use similar::TextDiff;

use super::{arg_str, Ctx, Outcome, Preview};
use crate::sandbox::Sandbox;

const MAX_READ_BYTES: u64 = 2 * 1024 * 1024;
const MAX_READ_CHARS: usize = 60_000;
const DEFAULT_LINES: usize = 600;
const MAX_LIST: usize = 400;
const MAX_MATCHES: usize = 150;
const MAX_DIFF_LINES: usize = 400;

pub fn list_dir_params() -> Value {
    json!({ "type": "object", "properties": { "path": { "type": "string", "description": "Folder to list. Leave out to list the shared folders." } } })
}
pub fn read_file_params() -> Value {
    json!({ "type": "object", "required": ["path"], "properties": {
        "path": { "type": "string" },
        "start_line": { "type": "integer", "description": "First line to read (1-based)." },
        "end_line": { "type": "integer", "description": "Last line to read." } } })
}
pub fn find_files_params() -> Value {
    json!({ "type": "object", "required": ["pattern"], "properties": {
        "pattern": { "type": "string", "description": "Glob, for example **/*.py or src/**/test_*" },
        "path": { "type": "string", "description": "Folder to search in (default: all shared folders)." } } })
}
pub fn search_files_params() -> Value {
    json!({ "type": "object", "required": ["pattern"], "properties": {
        "pattern": { "type": "string", "description": "Regular expression to look for." },
        "path": { "type": "string", "description": "Folder or file to search (default: all shared folders)." },
        "glob": { "type": "string", "description": "Only search files matching this glob, for example *.ts" },
        "ignore_case": { "type": "boolean" } } })
}
pub fn write_file_params() -> Value {
    json!({ "type": "object", "required": ["path", "content"], "properties": {
        "path": { "type": "string" }, "content": { "type": "string", "description": "The complete new file contents." } } })
}
/// A stand-in some models write instead of a file's contents: one short
/// bracketed or "…"-style line ("[corrected content]", "[insert code here]",
/// "... rest of the code ...").
fn is_stand_in(content: &str) -> bool {
    let t = content.trim();
    if t.is_empty() || t.lines().count() > 1 || t.chars().count() > 80 {
        return false;
    }
    let bracketed = (t.starts_with('[') && t.ends_with(']')) || (t.starts_with('<') && t.ends_with('>') && !t.contains("</")) || (t.starts_with('{') && t.ends_with('}') && !t.contains(':'));
    let filler = t.starts_with("...") || t.starts_with('…') || t.to_lowercase().contains("rest of the");
    // Words in brackets, not data (["a", "b"]) or markup.
    let words = t.chars().any(char::is_alphabetic) && t.contains(' ') && !t.contains('"') && !t.contains('\'') && !t.contains('=');
    (bracketed && words) || filler
}

#[cfg(test)]
#[test]
fn edits_match_when_only_the_spacing_differs() {
    let js = "document.addEventListener('DOMContentLoaded', () => {\n    const buy = document.getElementById('buy');\n    buy.addEventListener('click', go);\n});\n";
    // Copied without its indentation, with a doubled space.
    let (out, n) = apply_edit(js, "const buy = document.getElementById('buy');\nbuy.addEventListener('click',  go);", "const buy = document.getElementById('buy-button');\nbuy.addEventListener('click', go);", false).unwrap();
    assert_eq!(n, 1);
    assert_eq!(out, "document.addEventListener('DOMContentLoaded', () => {\n    const buy = document.getElementById('buy-button');\n    buy.addEventListener('click', go);\n});\n");
    // CRLF files keep CRLF; over-indented old text is shifted back.
    let crlf = "a {\r\n  color: red;\r\n}\r\n";
    let (out, _) = apply_edit(crlf, "        color: red;", "        color: blue;\n        margin: 0;", false).unwrap();
    assert_eq!(out, "a {\r\n  color: blue;\r\n  margin: 0;\r\n}\r\n");
    // Two places that match loosely: still refused.
    assert!(apply_edit("x = 1;\ny = 2;\n  x = 1;\n", "x  =  1;", "x = 3;", false).is_err());
}

#[cfg(test)]
#[test]
fn stand_ins_arent_written_as_files() {
    for s in ["[corrected content]", "[insert code here]", "  [written to the file; read_file shows it]\n", "... rest of the code ...", "<your HTML here>", "{ full contents }"] {
        assert!(is_stand_in(s), "{s}");
    }
    for s in ["<p>Hi</p>", "[1, 2, 3]", "[\"a\", \"b\"]", "{\"a\": 1}", "body { margin: 0 }", "console.log('hi');", "", "<meta charset=\"utf-8\">", "line one\n[two words]"] {
        assert!(!is_stand_in(s), "{s}");
    }
}

pub fn edit_file_params() -> Value {
    json!({ "type": "object", "required": ["path", "old_text", "new_text"], "properties": {
        "path": { "type": "string" },
        "old_text": { "type": "string", "description": "Exact text to replace, copied from the file. Empty: new_text is added at the end of the file." },
        "new_text": { "type": "string" },
        "replace_all": { "type": "boolean", "description": "Replace every occurrence instead of exactly one." } } })
}
pub fn move_path_params() -> Value {
    json!({ "type": "object", "required": ["from", "to"], "properties": { "from": { "type": "string" }, "to": { "type": "string" } } })
}
pub fn make_folder_params() -> Value {
    json!({ "type": "object", "required": ["path"], "properties": { "path": { "type": "string" } } })
}
pub fn delete_path_params() -> Value {
    json!({ "type": "object", "required": ["path"], "properties": { "path": { "type": "string" } } })
}

/// Office files and PDFs are zipped or binary: writing text into one breaks it.
fn refuse_binary(path: &Path) -> Result<(), String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if matches!(ext.as_str(), "docx" | "xlsx" | "xls" | "pptx" | "pdf" | "doc" | "ppt" | "odt" | "ods") {
        return Err(format!("A .{ext} file can't be written as text. Use create_document for Word, Excel, PowerPoint and PDF files."));
    }
    Ok(())
}

fn read_text(path: &Path) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|_| format!("{} doesn't exist.", path.display()))?;
    if meta.is_dir() {
        return Err("That's a folder; use list_dir.".into());
    }
    if meta.len() > MAX_READ_BYTES {
        return Err(format!("The file is {} MB, too large to read whole. Use search_files to find the part you need.", meta.len() / 1_048_576));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.iter().take(8192).any(|&b| b == 0) {
        return Err(format!("This looks like a binary file ({} bytes), not text.", bytes.len()));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// A unified diff for previews, trimmed for display.
pub fn diff(label: &str, old: &str, new: &str) -> String {
    // Compare without carriage returns so Windows files display line by line.
    let (old, new) = (old.replace("\r\n", "\n"), new.replace("\r\n", "\n"));
    let text = TextDiff::from_lines(&old, &new).unified_diff().context_radius(3).header(label, label).to_string();
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() > MAX_DIFF_LINES {
        format!("{}\n… {} more lines", lines[..MAX_DIFF_LINES].join("\n"), lines.len() - MAX_DIFF_LINES)
    } else {
        text
    }
}

fn line_count(s: &str) -> usize {
    if s.is_empty() { 0 } else { s.lines().count() }
}

/// Models sometimes copy read_file's line numbers into what they write.
/// If every line carries consecutive numbers in read_file's format (or a
/// common look-alike), returns the text without them.
pub fn strip_line_numbers(s: &str) -> Option<String> {
    let lines: Vec<&str> = s.lines().collect();
    if lines.len() < 2 {
        return None;
    }
    let mut expected: Option<u64> = None;
    let mut out = Vec::with_capacity(lines.len());
    for line in &lines {
        let trimmed = line.trim_start();
        let digits: String = trimmed.chars().take_while(|c| c.is_ascii_digit()).collect();
        let n: u64 = digits.parse().ok()?;
        if expected.is_some_and(|e| e != n) {
            return None;
        }
        expected = Some(n + 1);
        let rest = &trimmed[digits.len()..];
        let body = ["\t", ": ", ":", "│ ", "| ", "→"].iter().find_map(|sep| rest.strip_prefix(sep))?;
        out.push(body);
    }
    let mut text = out.join("\n");
    if s.ends_with('\n') {
        text.push('\n');
    }
    Some(text)
}

/// Applies an edit_file request to text, or explains why it can't.
/// A line with its runs of spaces and tabs made one space, and none at the ends.
fn squash(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `old` found line by line with spacing and indentation not counting (in
/// exactly one place), replaced by `new` indented to fit where it goes.
fn loose_edit(text: &str, old: &str, new: &str) -> Option<String> {
    let given: Vec<&str> = old.lines().collect();
    let first = given.iter().position(|l| !l.trim().is_empty())?;
    let last = given.iter().rposition(|l| !l.trim().is_empty())?;
    let want: Vec<String> = given[first..=last].iter().map(|l| squash(l)).collect();
    let lines: Vec<&str> = text.split('\n').collect();
    let mut starts = Vec::with_capacity(lines.len());
    let mut at = 0;
    for l in &lines {
        starts.push(at);
        at += l.len() + 1;
    }
    let mut found = None;
    for i in 0..lines.len().saturating_sub(want.len() - 1) {
        if (0..want.len()).all(|k| squash(lines[i + k]) == want[k]) {
            if found.is_some() {
                return None;
            }
            found = Some(i);
        }
    }
    let i = found?;
    let end_line = i + want.len() - 1;
    let start = starts[i];
    let end = starts[end_line] + lines[end_line].trim_end_matches('\r').len();
    // Shift the new lines by the difference between the file's indentation and the model's.
    let indent = |l: &str| l.len() - l.trim_start().len();
    let have = indent(lines[i]);
    let gave = indent(given[first]);
    let pad = &lines[i][..have];
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let replaced: Vec<String> = new
        .lines()
        .map(|l| {
            if l.trim().is_empty() {
                String::new()
            } else if have > gave {
                format!("{}{l}", &pad[..have - gave])
            } else {
                let cut = (gave - have).min(indent(l));
                l[cut..].to_string()
            }
        })
        .collect();
    Some(format!("{}{}{}", &text[..start], replaced.join(eol), &text[end..]))
}

/// read_file's line numbers taken off the lines that carry them, when some
/// do and some don't (a model copied a few lines with their numbers). None
/// when no line has one.
fn strip_some_line_numbers(s: &str) -> Option<String> {
    let numbered = |l: &str| {
        let t = l.trim_start_matches(' ');
        let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
        (digits > 0 && l.len() - t.len() <= 6 && t[digits..].starts_with('\t')).then(|| t[digits + 1..].to_string())
    };
    let lines: Vec<&str> = s.split('\n').collect();
    if !lines.iter().any(|l| numbered(l).is_some()) {
        return None;
    }
    Some(lines.iter().map(|l| numbered(l).unwrap_or_else(|| l.to_string())).collect::<Vec<_>>().join("\n"))
}

/// Where the file comes closest to `old`: the most of its lines (spacing
/// not counting) that match in a row, where, and the first that doesn't.
/// For an error that shows the model what to copy instead of re-reading.
fn closest(text: &str, old: &str) -> Option<String> {
    let want: Vec<String> = old.lines().map(squash).filter(|l| !l.is_empty()).collect();
    let lines: Vec<&str> = text.lines().collect();
    let mut best: Option<(usize, usize, usize)> = None; // (matched, file line, first old line)
    for (o, first) in want.iter().enumerate() {
        for (i, l) in lines.iter().enumerate() {
            if squash(l) != *first {
                continue;
            }
            let mut k = 0;
            let mut j = i;
            while o + k < want.len() && j < lines.len() {
                if squash(lines[j]).is_empty() {
                    j += 1;
                    continue;
                }
                if squash(lines[j]) != want[o + k] {
                    break;
                }
                k += 1;
                j += 1;
            }
            if best.is_none_or(|b| k > b.0) {
                best = Some((k, i, o));
            }
        }
        if best.is_some() {
            break;
        }
    }
    let (matched, at, from) = best?;
    let next = want.get(from + matched);
    let shown = lines[at..lines.len().min(at + matched + 3)].join("\n");
    Some(match next {
        Some(n) => format!(
            "The closest place is line {}: {matched} line(s) match, then your old_text has `{n}`, which isn't there. The file has:\n{shown}\n",
            at + 1
        ),
        None => format!("The closest place is line {}:\n{shown}\n", at + 1),
    })
}

/// Where the `{` already open at the start of `s` closes (`}`'s byte
/// index), skipping strings and line comments; None if it doesn't.
fn closing_brace(s: &str) -> Option<usize> {
    let mut depth = 1i32;
    let mut quote: Option<char> = None;
    let mut chars = s.char_indices().peekable();
    while let Some((i, ch)) = chars.next() {
        match quote {
            Some(q) => {
                if ch == '\\' {
                    chars.next();
                } else if ch == q {
                    quote = None;
                }
            }
            None => match ch {
                '"' | '\'' | '`' => quote = Some(ch),
                '/' if chars.peek().is_some_and(|(_, c)| *c == '/') => {
                    while chars.peek().is_some_and(|(_, c)| *c != '\n') {
                        chars.next();
                    }
                }
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            },
        }
    }
    None
}

/// The one place `old` is, replaced with `new`. A small model often sends
/// a block's first line as old_text and the whole block, changed, as
/// new_text (gpt-oss 20B adding a command, which left `money` twice and the
/// file broken): when new_text starts with old_text, what the file already
/// has after it is replaced, not kept twice. With old_text opening a block
/// (`{`) that new_text closes, the file's block goes; otherwise the lines
/// after it that new_text repeats do.
fn replace_once(text: &str, old: &str, new: &str) -> String {
    let Some(at) = text.find(old) else { return text.to_string() };
    let (before, after) = (&text[..at], &text[at + old.len()..]);
    if let Some(rest) = new.strip_prefix(old) {
        if old.trim_end().ends_with('{') && closing_brace(rest).is_some() {
            if let Some(end) = closing_brace(after) {
                return format!("{before}{new}{}", &after[end + 1..]);
            }
        }
        // Lines after old_text in the file that new_text starts by repeating.
        let file_lines: Vec<&str> = after.split_inclusive('\n').collect();
        let new_lines: Vec<&str> = rest.split_inclusive('\n').collect();
        let same = file_lines.iter().zip(&new_lines).take_while(|(f, n)| f.trim_end() == n.trim_end()).count();
        // Whole lines only (new_text's last one may run on), and at least one with something on it.
        let same = if same == new_lines.len() && !rest.ends_with('\n') { same.saturating_sub(1) } else { same };
        if file_lines[..same].iter().any(|l| !l.trim().is_empty()) {
            let covered: usize = file_lines[..same].iter().map(|l| l.len()).sum();
            let gap = if new.ends_with('\n') { "" } else { "\n" };
            return format!("{before}{new}{gap}{}", &after[covered..]);
        }
    }
    format!("{before}{new}{after}")
}

/// Why a code file no longer parses after a change, if it doesn't: JSON
/// here, JavaScript with Node (when it's installed). Said with the edit, so
/// a broken file is fixed at once rather than found steps later.
pub fn syntax_problem(path: &std::path::Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "json" => {
            let text = std::fs::read_to_string(path).ok()?;
            serde_json::from_str::<Value>(&text).err().map(|e| e.to_string())
        }
        "js" | "cjs" | "mjs" => {
            let mut cmd = std::process::Command::new("node");
            cmd.arg("--check").arg(path).stdin(std::process::Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0800_0000);
            }
            let out = cmd.output().ok()?;
            if out.status.success() {
                return None;
            }
            let err = String::from_utf8_lossy(&out.stderr);
            // "file.js:10" then the line, the caret and "SyntaxError: …".
            let line = err.lines().find(|l| l.contains(".js:") || l.contains(".mjs:") || l.contains(".cjs:")).and_then(|l| l.rsplit(':').next()).unwrap_or("").trim().to_string();
            let what = err.lines().find(|l| l.contains("Error")).unwrap_or("it doesn't parse").trim().to_string();
            Some(if line.is_empty() { what } else { format!("{what} (line {line})") })
        }
        _ => None,
    }
}

/// Unfinished parts a change added ("TODO", "not implemented"): lines in
/// `new` that `old` didn't have, with their line numbers. gpt-oss 20B's
/// budget app had an Edit button whose code was `// TODO: allow editing`.
pub fn added_stubs(old: &str, new: &str) -> Vec<(usize, String)> {
    static STUB: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)\b(todo|fixme|not (yet )?implemented|implement (this|later|me)|coming soon)\b").unwrap());
    let before: std::collections::HashSet<&str> = old.lines().map(str::trim).collect();
    new.lines()
        .enumerate()
        .filter(|(_, l)| STUB.is_match(l) && !before.contains(l.trim()))
        .map(|(i, l)| (i + 1, l.trim().chars().take(120).collect()))
        .take(5)
        .collect()
}

/// The change's words, with a warning when the file it left doesn't parse,
/// and a note of unfinished parts it added.
fn with_checks(mut o: Outcome, path: &std::path::Path, label: &str, old: &str, new: &str) -> Outcome {
    if let Some(problem) = syntax_problem(path) {
        o.text.push_str(&format!("\n\nWarning: {label} doesn't parse now: {problem}. Read it and fix that before anything else."));
        o.meta["broken"] = serde_json::json!(true);
    }
    let stubs = added_stubs(old, new);
    if !stubs.is_empty() {
        let list: Vec<String> = stubs.iter().map(|(n, l)| format!("line {n}: {l}")).collect();
        o.text.push_str(&format!("\n\nNote: {label} has unfinished parts: {}. Finish them if they're part of what was asked.", list.join("; ")));
        o.meta["unfinished"] = serde_json::json!({ "path": path.display().to_string(), "lines": stubs.iter().map(|(_, l)| l.clone()).collect::<Vec<_>>() });
    }
    o
}

pub fn apply_edit(text: &str, old: &str, new: &str, replace_all: bool) -> Result<(String, usize), String> {
    // Nothing to replace: added at the end (a model appending a test).
    if old.is_empty() {
        let gap = if text.is_empty() || text.ends_with('\n') { "" } else { "\n" };
        return Ok((format!("{text}{gap}{new}"), 1));
    }
    if !text.contains(old) {
        // Retry without copied line numbers (on every line, or on some) before giving up.
        let found = |o: &String| text.contains(o.as_str()) || text.contains(&o.replace('\n', "\r\n"));
        if let Some(stripped_old) = strip_line_numbers(old).filter(found).or_else(|| strip_some_line_numbers(old).filter(found)) {
            let stripped_new = strip_line_numbers(new).or_else(|| strip_some_line_numbers(new)).unwrap_or_else(|| new.to_string());
            return apply_edit(text, &stripped_old, &stripped_new, replace_all);
        }
    }
    let count = text.matches(old).count();
    match count {
        0 => {
            // Windows files often use CRLF; accept LF-only old_text for them.
            let crlf_old = old.replace("\r\n", "\n").replace('\n', "\r\n");
            if text.contains("\r\n") && text.matches(crlf_old.as_str()).count() > 0 {
                let new = new.replace("\r\n", "\n").replace('\n', "\r\n");
                return apply_edit(text, &crlf_old, &new, replace_all);
            }
            // Small models rarely copy indentation and spacing exactly: the one
            // place that matches when spaces don't count is the place meant.
            if let Some(t) = loose_edit(text, old, new) {
                return Ok((t, 1));
            }
            let stripped = strip_some_line_numbers(old);
            if let Some(t) = stripped.as_deref().and_then(|o| loose_edit(text, o, &strip_some_line_numbers(new).unwrap_or_else(|| new.to_string()))) {
                return Ok((t, 1));
            }
            let near = closest(text, stripped.as_deref().unwrap_or(old)).unwrap_or_default();
            Err(format!("old_text wasn't found. {near}Copy the text exactly from the file (or send the whole fixed file with write_file)."))
        }
        1 => Ok((replace_once(text, old, new), 1)),
        n if replace_all => Ok((text.replace(old, new), n)),
        n => Err(format!("old_text appears {n} times. Include more surrounding lines so it matches exactly one place, or set replace_all.")),
    }
}

pub fn preview(name: &str, args: &Value, sb: &Sandbox) -> Result<Preview, String> {
    let p = |key: &str| -> Result<std::path::PathBuf, String> { sb.resolve(arg_str(args, key)?) };
    Ok(match name {
        "write_file" => {
            let path = p("path")?;
            refuse_binary(&path)?;
            let label = sb.display(&path);
            let raw = arg_str(args, "content")?;
            let stripped = strip_line_numbers(raw);
            let new = stripped.as_deref().unwrap_or(raw);
            let old = if path.exists() { read_text(&path)? } else { String::new() };
            Preview {
                title: if path.exists() { format!("Replace {label}") } else { format!("Create {label}") },
                kind: "diff",
                detail: Some(diff(&label, &old, new)),
                note: None,
            }
        }
        "edit_file" => {
            let path = p("path")?;
            let label = sb.display(&path);
            let old = read_text(&path)?;
            let replace_all = args.get("replace_all").and_then(Value::as_bool).unwrap_or(false);
            let (new, _) = apply_edit(&old, arg_str(args, "old_text")?, arg_str(args, "new_text")?, replace_all)?;
            Preview { title: format!("Edit {label}"), kind: "diff", detail: Some(diff(&label, &old, &new)), note: None }
        }
        "move_path" => {
            let (from, to) = (p("from")?, p("to")?);
            Preview { title: format!("Move {} → {}", sb.display(&from), sb.display(&to)), kind: "text", detail: None, note: None }
        }
        "make_folder" => Preview { title: format!("Create folder {}", sb.display(&p("path")?)), kind: "text", detail: None, note: None },
        "delete_path" => {
            let path = p("path")?;
            Preview {
                title: format!("Delete {}", sb.display(&path)),
                kind: "text",
                detail: None,
                note: Some("It goes to the Recycle Bin, and Undo can restore it.".into()),
            }
        }
        // Reads ask only when a saved rule or Settings › Advanced says so.
        "read_file" | "list_dir" => {
            let path = sb.resolve_read(arg_str(args, "path")?)?;
            let verb = if name == "read_file" { "Read" } else { "List" };
            Preview { title: format!("{verb} {}", sb.display(&path)), kind: "text", detail: None, note: None }
        }
        "find_files" | "search_files" => {
            let pattern = arg_str(args, "pattern")?;
            Preview { title: format!("Search for {pattern}"), kind: "text", detail: args.get("path").and_then(Value::as_str).map(str::to_string), note: None }
        }
        other => Preview { title: other.replace('_', " "), kind: "text", detail: Some(serde_json::to_string_pretty(args).unwrap_or_default()), note: None },
    })
}

pub fn run(name: &str, args: &Value, ctx: &Ctx<'_>) -> Outcome {
    let sb = ctx.sandbox;
    let result: Result<Outcome, String> = (|| {
        match name {
            "list_dir" => list_dir(sb, args.get("path").and_then(Value::as_str)),
            "read_file" => {
                let path = sb.resolve_read(arg_str(args, "path")?)?;
                let text = read_text(&path)?;
                let lines: Vec<&str> = text.lines().collect();
                let start = args.get("start_line").and_then(Value::as_u64).map_or(1, |n| n.max(1) as usize);
                if start > lines.len() && !lines.is_empty() {
                    return Err(format!("The file has {} lines, so there's nothing from line {start} on.", lines.len()));
                }
                let end = args.get("end_line").and_then(Value::as_u64).map_or(start + DEFAULT_LINES - 1, |n| n as usize).min(lines.len());
                let mut out = String::new();
                for (i, line) in lines.iter().enumerate().take(end).skip(start - 1) {
                    let _ = writeln!(out, "{:>5}\t{line}", i + 1);
                    if out.len() > MAX_READ_CHARS {
                        let _ = writeln!(out, "… stopped at line {} (output limit). Use start_line to read further.", i + 1);
                        break;
                    }
                }
                if end < lines.len() && out.len() <= MAX_READ_CHARS {
                    let _ = writeln!(out, "… {} more lines. Use start_line={} to continue.", lines.len() - end, end + 1);
                }
                if lines.is_empty() {
                    out = "(empty file)".into();
                }
                let label = sb.display(&path);
                Ok(Outcome::ok(out, format!("Read {label}"), "text", None))
            }
            "find_files" => find_files(sb, arg_str(args, "pattern")?, args.get("path").and_then(Value::as_str)),
            "search_files" => search_files(sb, args, ctx),
            "write_file" => {
                let path = sb.resolve(arg_str(args, "path")?)?;
                refuse_binary(&path)?;
                let raw = arg_str(args, "content")?;
                let stripped = strip_line_numbers(raw);
                let content = stripped.as_deref().unwrap_or(raw);
                if is_stand_in(content) {
                    return Err(format!(
                        "{content:?} is a placeholder, not the file's contents. Send the whole file, every line of it, as content."
                    ));
                }
                let label = sb.display(&path);
                let old = if path.exists() { read_text(&path)? } else { String::new() };
                let existed = path.exists();
                ctx.recorder.before_write(&path)?;
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                }
                crate::storage::write_safely(&path, content).map_err(|e| format!("Couldn't write {label}: {e}"))?;
                crate::handoff_guard::wrote(ctx.state, ctx.memory.cipher, ctx.memory.chat_id, &path, content);
                let verb = if existed { "Replaced" } else { "Created" };
                let mut o = Outcome::ok(
                    format!("{verb} {label} ({} lines).", line_count(content)),
                    format!("{verb} {label}"),
                    "diff",
                    Some(diff(&label, &old, content)),
                );
                // Listed in the chat's Files, with Open and Show in folder.
                o.meta["file"] = serde_json::json!(path.display().to_string());
                Ok(with_checks(o, &path, &label, &old, content))
            }
            "edit_file" => {
                let path = sb.resolve(arg_str(args, "path")?)?;
                let label = sb.display(&path);
                let old = read_text(&path)?;
                let replace_all = args.get("replace_all").and_then(Value::as_bool).unwrap_or(false);
                let (new, n) = apply_edit(&old, arg_str(args, "old_text")?, arg_str(args, "new_text")?, replace_all)?;
                // gpt-oss 20B sent the same edit, old and new alike, 30 times over.
                if new == old {
                    return Err("old_text and new_text are the same, so this changes nothing. If something should be different, put the changed text in new_text; if it's right as it is, move on to the next step.".into());
                }
                ctx.recorder.before_write(&path)?;
                crate::storage::write_safely(&path, &new).map_err(|e| format!("Couldn't write {label}: {e}"))?;
                let o = Outcome::ok(
                    format!("Edited {label} ({n} replacement{}).", if n == 1 { "" } else { "s" }),
                    format!("Edited {label}"),
                    "diff",
                    Some(diff(&label, &old, &new)),
                );
                Ok(with_checks(o, &path, &label, &old, &new))
            }
            "move_path" => {
                let (from, to) = (sb.resolve(arg_str(args, "from")?)?, sb.resolve(arg_str(args, "to")?)?);
                if !from.exists() {
                    return Err(format!("{} doesn't exist.", sb.display(&from)));
                }
                if to.exists() {
                    return Err(format!("{} already exists; pick another name or delete it first.", sb.display(&to)));
                }
                if let Some(dir) = to.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                }
                std::fs::rename(&from, &to).map_err(|e| format!("Couldn't move it: {e}"))?;
                ctx.recorder.after_move(&from, &to)?;
                let title = format!("Moved {} → {}", sb.display(&from), sb.display(&to));
                Ok(Outcome::ok(format!("{title}."), title, "text", None))
            }
            "make_folder" => {
                let path = sb.resolve(arg_str(args, "path")?)?;
                let label = sb.display(&path);
                if path.exists() {
                    return Ok(Outcome::ok(format!("{label} already exists."), format!("Folder {label} exists"), "text", None));
                }
                std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
                ctx.recorder.after_mkdir(&path)?;
                Ok(Outcome::ok(format!("Created folder {label}."), format!("Created folder {label}"), "text", None))
            }
            "delete_path" => {
                let path = sb.resolve(arg_str(args, "path")?)?;
                let label = sb.display(&path);
                if sb.roots().iter().any(|r| r == &path) {
                    return Err("A shared folder itself can't be deleted from a chat.".into());
                }
                if !path.exists() {
                    return Err(format!("{label} doesn't exist."));
                }
                ctx.recorder.before_delete(&path)?;
                crate::checkpoint::recycle(&path)?;
                Ok(Outcome::ok(format!("Moved {label} to the Recycle Bin."), format!("Deleted {label}"), "text", None))
            }
            other => Err(format!("Unknown tool {other}.")),
        }
    })();
    result.unwrap_or_else(|e| Outcome::error(super::failed_title(name, args), e))
}

fn list_dir(sb: &Sandbox, path: Option<&str>) -> Result<Outcome, String> {
    let Some(path) = path.filter(|p| !p.trim().is_empty() && p.trim() != ".") else {
        let mut lines: Vec<String> = sb.roots().iter().map(|r| format!("{}/ (shared: {})", sb.display(r), r.display())).collect();
        if let Some(h) = sb.home() {
            lines.push(format!("{} (the home folder: search and read only)", h.display()));
        }
        if lines.is_empty() {
            return Err("No folders are shared yet.".into());
        }
        return Ok(Outcome::ok(format!("Folders you can use:\n{}", lines.join("\n")), "Listed folders", "text", None));
    };
    let dir = sb.resolve_read(path)?;
    let label = sb.display(&dir);
    let mut entries: Vec<(bool, String, u64)> = std::fs::read_dir(&dir)
        .map_err(|_| format!("{label} isn't a folder that can be read."))?
        .flatten()
        .map(|e| {
            let meta = e.metadata().ok();
            (meta.as_ref().is_some_and(|m| m.is_dir()), e.file_name().to_string_lossy().to_string(), meta.map_or(0, |m| m.len()))
        })
        .collect();
    entries.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.to_lowercase().cmp(&b.1.to_lowercase())));
    let total = entries.len();
    let mut out = format!("{label}/ ({total} items)\n");
    for (is_dir, name, size) in entries.into_iter().take(MAX_LIST) {
        if is_dir {
            let _ = writeln!(out, "  {name}/");
        } else {
            let _ = writeln!(out, "  {name}  ({})", human(size));
        }
    }
    if total > MAX_LIST {
        let _ = writeln!(out, "  … {} more", total - MAX_LIST);
    }
    Ok(Outcome::ok(out, format!("Listed {label}"), "text", None))
}

fn human(n: u64) -> String {
    match n {
        n if n >= 1 << 30 => format!("{:.1} GB", n as f64 / (1u64 << 30) as f64),
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1u64 << 20) as f64),
        n if n >= 1 << 10 => format!("{:.1} KB", n as f64 / 1024.0),
        n => format!("{n} B"),
    }
}

fn search_roots(sb: &Sandbox, path: Option<&str>) -> Result<Vec<std::path::PathBuf>, String> {
    match path.filter(|p| !p.trim().is_empty()) {
        Some(p) => Ok(vec![sb.resolve_read(p)?]),
        None if !sb.is_empty() => Ok(sb.roots().to_vec()),
        None => sb.home().map(|h| vec![h.clone()]).ok_or_else(|| "No folders are shared yet.".to_string()),
    }
}

/// Files under each root, honoring .gitignore and skipping build/dependency
/// folders. Globs are matched relative to each root, so each gets its own walk.
pub(crate) fn walk_files(roots: &[std::path::PathBuf], glob: Option<&str>, limit: usize) -> Result<Vec<std::path::PathBuf>, String> {
    let mut files = Vec::new();
    for root in roots {
        let mut ov = ignore::overrides::OverrideBuilder::new(root);
        // "*report*" finds Report.pdf: names are matched without regard to capitals.
        ov.case_insensitive(true).map_err(|e| e.to_string())?;
        // Never descend into these, even without a .gitignore.
        for skip in ["!.git/", "!node_modules/", "!target/", "!.venv/", "!__pycache__/"] {
            ov.add(skip).map_err(|e| e.to_string())?;
        }
        // The home folder: not its app data and hidden folders (huge, and
        // where keys and saved logins live).
        let home = crate::sandbox::home_folder().is_some_and(|h| &h == root);
        if home {
            for skip in ["!/AppData/", "!/.*/"] {
                ov.add(skip).map_err(|e| e.to_string())?;
            }
        }
        if let Some(g) = glob {
            let g = g.replace('\\', "/");
            let g = g.trim_start_matches("./").trim_start_matches('/');
            // "proj/src/*.rs" names the shared folder itself, like other paths do.
            let name = root.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            let g = match g.split_once('/') {
                Some((head, rest)) if head.to_lowercase() == name => rest.to_string(),
                _ => g.to_string(),
            };
            let g = if g.contains('/') { g } else { format!("**/{g}") };
            ov.add(&g).map_err(|e| format!("That glob isn't valid: {e}"))?;
        }
        let walk = ignore::WalkBuilder::new(root)
            .max_depth(if home { Some(8) } else { None })
            .hidden(false)
            .git_ignore(true)
            .git_exclude(true)
            .parents(true)
            .require_git(false)
            .overrides(ov.build().map_err(|e| e.to_string())?)
            .build();
        for entry in walk.flatten() {
            if entry.file_type().is_some_and(|t| t.is_file()) {
                files.push(entry.into_path());
                if files.len() >= limit {
                    return Ok(files);
                }
            }
        }
    }
    Ok(files)
}

fn find_files(sb: &Sandbox, pattern: &str, path: Option<&str>) -> Result<Outcome, String> {
    let roots = search_roots(sb, path)?;
    let mut found: Vec<std::path::PathBuf> = walk_files(&roots, Some(pattern), MAX_LIST)?;
    let mut searched: Vec<String> = roots.iter().map(|r| r.display().to_string()).collect();
    // Nothing in the shared folders: the home folder next, before giving up.
    let mut from_home = false;
    if found.is_empty() && path.is_none_or(|p| p.trim().is_empty()) {
        if let Some(h) = sb.home().filter(|h| !roots.contains(h)) {
            found = walk_files(std::slice::from_ref(h), Some(pattern), MAX_LIST)?;
            searched.push(h.display().to_string());
            from_home = !found.is_empty();
        }
    }
    // For an online model, secrets in the home folder aren't listed (they can't be read either).
    found.retain(|p| !sb.secrets_closed() || sb.holds(p) || !crate::sandbox::secret(p));
    let shown: Vec<String> = found.iter().map(|p| if sb.holds(p) { sb.display(p) } else { p.display().to_string() }).collect();
    let text = if shown.is_empty() {
        let stem = pattern.trim_matches('*').split('.').next().unwrap_or(pattern);
        format!(
            "Not found: no file matches {pattern} in {}. Try a broader pattern (part of the name between stars, like *{stem}*), another folder with path, or search_files to look inside files.",
            searched.join(" or ")
        )
    } else if from_home {
        format!("Nothing in the shared folders; found in the home folder:\n{}", shown.join("\n"))
    } else {
        shown.join("\n")
    };
    Ok(Outcome::ok(text, format!("Found {} file{} matching {pattern}", shown.len(), if shown.len() == 1 { "" } else { "s" }), "text", None))
}

fn search_files(sb: &Sandbox, args: &Value, ctx: &Ctx<'_>) -> Result<Outcome, String> {
    // Chats not working together: other chats' handoff notes stay out of the results.
    let together = crate::features::is_on(&ctx.state.db.lock().unwrap(), crate::features::Feature::ChatTeam);
    let pattern = arg_str(args, "pattern")?;
    let ignore_case = args.get("ignore_case").and_then(Value::as_bool).unwrap_or(false);
    let re = regex::RegexBuilder::new(pattern)
        .case_insensitive(ignore_case)
        .size_limit(1 << 20)
        .build()
        .map_err(|e| format!("That pattern isn't a valid regular expression: {e}"))?;
    let roots = search_roots(sb, args.get("path").and_then(Value::as_str))?;
    let glob = args.get("glob").and_then(Value::as_str);
    let mut out = String::new();
    let mut hits = 0;
    let single_file = roots.len() == 1 && roots[0].is_file();
    let files: Vec<std::path::PathBuf> = if single_file { roots.clone() } else { walk_files(&roots, glob, 20_000)? };
    'files: for file in files {
        if sb.secrets_closed() && !sb.holds(&file) && crate::sandbox::secret(&file) {
            continue;
        }
        let Ok(text) = read_text(&file) else { continue };
        if !together && crate::handoff_guard::looks_like_handoff(&file, &text) && crate::handoff_guard::guarded(ctx.state, ctx.memory.cipher, ctx.memory.chat_id, &file).is_some() {
            continue;
        }
        for (i, line) in text.lines().enumerate() {
            if re.is_match(line) {
                let shown: String = line.trim().chars().take(240).collect();
                let _ = writeln!(out, "{}:{}: {shown}", sb.display(&file), i + 1);
                hits += 1;
                if hits >= MAX_MATCHES {
                    let _ = writeln!(out, "… stopped at {MAX_MATCHES} matches. Narrow the pattern or path.");
                    break 'files;
                }
            }
        }
    }
    if hits == 0 {
        out = format!("No matches for {pattern}.");
    }
    Ok(Outcome::ok(out, format!("Searched for “{pattern}” ({hits} match{})", if hits == 1 { "" } else { "es" }), "text", None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_sent_whole_from_its_first_line_replaces_the_block() {
        // gpt-oss 20B adding pnl: old_text a function's first line, new_text
        // the function again and the new one after it.
        let file = "function money(n) {\n  return \"$\" + n;\n}\n\nfunction report() {}\n";
        let new = "function money(n) {\n  return \"$\" + n;\n}\n\nfunction pnl() {}";
        assert_eq!(apply_edit(file, "function money(n) {", new, false).unwrap().0, "function money(n) {\n  return \"$\" + n;\n}\n\nfunction pnl() {}\n\nfunction report() {}\n");
        // The block changed on the way: the old body goes (a `}` in a string doesn't count).
        let changed = "function money(n) {\n  return \"}\" + n.toFixed(2);\n}";
        assert_eq!(apply_edit(file, "function money(n) {", changed, false).unwrap().0, "function money(n) {\n  return \"}\" + n.toFixed(2);\n}\n\nfunction report() {}\n");
        // A line put at the top of the body (the block isn't closed): only added.
        assert_eq!(apply_edit(file, "function money(n) {", "function money(n) {\n  log(n);", false).unwrap().0, "function money(n) {\n  log(n);\n  return \"$\" + n;\n}\n\nfunction report() {}\n");
        // No braces (Python): the lines it repeats aren't kept twice.
        let py = "def a():\n    return 1\n\ndef b():\n    pass\n";
        assert_eq!(apply_edit(py, "def a():", "def a():\n    return 1\n\ndef c():\n    pass", false).unwrap().0, "def a():\n    return 1\n\ndef c():\n    pass\ndef b():\n    pass\n");
        // An ordinary edit is left alone.
        assert_eq!(apply_edit(py, "return 1", "return 2", false).unwrap().0, "def a():\n    return 2\n\ndef b():\n    pass\n");
    }

    #[test]
    fn unfinished_parts_a_change_adds_are_noted() {
        let old = "function edit(id) {\n  // TODO from the user: keep\n}\n";
        let new = "function edit(id) {\n  // TODO from the user: keep\n  // TODO: load into form, allow editing\n  console.log('edit', id);\n}\nfunction undo() { throw new Error('Not implemented'); }\n";
        let got = added_stubs(old, new);
        assert_eq!(got, vec![(3, "// TODO: load into form, allow editing".to_string()), (6, "function undo() { throw new Error('Not implemented'); }".to_string())]);
        // An input's placeholder text isn't one.
        assert!(added_stubs("", "<input placeholder=\"Amount\">").is_empty());
    }

    #[test]
    fn a_broken_code_file_is_said_at_once() {
        let dir = std::env::temp_dir().join(format!("sulcus-syntax-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let (good, bad, json) = (dir.join("ok.js"), dir.join("bad.js"), dir.join("p.json"));
        std::fs::write(&good, "function a() { return 1; }\n").unwrap();
        std::fs::write(&bad, "function a() {\n  return 1;\n}\n}\n").unwrap();
        std::fs::write(&json, "{\"a\": 1,}").unwrap();
        assert_eq!(syntax_problem(&good), None);
        assert!(syntax_problem(&json).is_some());
        // Node may not be installed where the tests run.
        if std::process::Command::new("node").arg("--version").output().is_ok() {
            let p = syntax_problem(&bad).expect("a stray brace is found");
            assert!(p.contains("SyntaxError") && p.contains("line 4"), "{p}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn edits_must_match_exactly_once_unless_replace_all() {
        assert_eq!(apply_edit("a b a", "b", "c", false).unwrap(), ("a c a".into(), 1));
        assert!(apply_edit("a b a", "a", "c", false).unwrap_err().contains("2 times"));
        assert_eq!(apply_edit("a b a", "a", "c", true).unwrap(), ("c b c".into(), 2));
        assert!(apply_edit("abc", "zzz", "y", false).is_err());
        // Nothing to replace: added at the end.
        assert_eq!(apply_edit("abc", "", "y", false).unwrap(), ("abc\ny".into(), 1));
        assert_eq!(apply_edit("abc\n", "", "y\n", false).unwrap(), ("abc\ny\n".into(), 1));
    }

    /// What a gpt-oss-20b wrote, adding a feature to a small app: line
    /// numbers on some lines only, and an extra closing brace.
    #[test]
    fn edits_from_a_small_model_apply_or_say_where_they_went_wrong() {
        let file = "test(\"a\", () => {\n  assert.strictEqual(p[0].realized, 200);\n});\n";
        let (new, _) = apply_edit(file, "  assert.strictEqual(p[0].realized, 200);\n   32\t});\n", "  assert.strictEqual(p[0].realized, 200);\n   32\t});\n\ntest(\"b\", () => {});\n", false).unwrap();
        assert_eq!(new, "test(\"a\", () => {\n  assert.strictEqual(p[0].realized, 200);\n});\n\ntest(\"b\", () => {});\n");

        let file = "function report(p) {\n  const rows = p.map((x) => x);\n  return rows;\n}\n\nmodule.exports = { report };\n";
        let err = apply_edit(file, "function report(p) {\n    const rows = p.map((x) => x);\n    return rows;\n    }\n\n    module.exports = { report };\n    }", "x", false).unwrap_err();
        assert!(err.contains("The closest place is line 1: 5 line(s) match, then your old_text has `}`, which isn't there."), "{err}");
        assert!(err.contains("module.exports = { report };"), "shows the file's lines: {err}");
    }

    #[test]
    fn lf_edits_apply_to_crlf_files() {
        let text = "fn main() {\r\n    old();\r\n}\r\n";
        let (new, n) = apply_edit(text, "fn main() {\n    old();", "fn main() {\n    new();", false).unwrap();
        assert_eq!(n, 1);
        assert_eq!(new, "fn main() {\r\n    new();\r\n}\r\n");
    }

    #[test]
    fn copied_line_numbers_are_removed() {
        assert_eq!(strip_line_numbers("1: # Title\n2: Body\n3: End").unwrap(), "# Title\nBody\nEnd");
        assert_eq!(strip_line_numbers("    1\tfn main() {\n    2\t}\n").unwrap(), "fn main() {\n}\n");
        assert!(strip_line_numbers("1: one\n3: three").is_none(), "not consecutive");
        assert!(strip_line_numbers("10 apples\n11 pears").is_none(), "no separator: real content");
        assert!(strip_line_numbers("single line").is_none());
        let (out, _) = apply_edit("a\nb\nc\n", "2: b\n3: c", "2: B\n3: c", false).unwrap();
        assert_eq!(out, "a\nB\nc\n");
    }

    #[test]
    fn globs_may_start_with_the_shared_folder_name() {
        let d = std::env::temp_dir().join(format!("sulcusai-glob-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(d.join("proj").join("src")).unwrap();
        std::fs::write(d.join("proj").join("src").join("a.rs"), "fn a() {}").unwrap();
        std::fs::write(d.join("proj").join("notes.md"), "x").unwrap();
        let sb = Sandbox::new(&[d.join("proj").display().to_string()]);
        for pattern in ["proj/src/*.rs", "src/*.rs", "*.rs", "proj/**/*.rs"] {
            let out = find_files(&sb, pattern, None).unwrap();
            assert!(out.text.contains("proj/src/a.rs"), "{pattern}: {}", out.text);
            assert!(!out.text.contains("notes.md"), "{pattern}");
        }
    }

    #[test]
    fn crlf_files_diff_line_by_line() {
        let d = diff("f", "one\r\ntwo\r\n", "one\r\nthree\r\n");
        assert!(!d.contains('\r'));
        assert!(d.contains("-two\n") && d.contains("+three"));
    }

    #[test]
    fn diff_shows_changed_lines() {
        let d = diff("f.txt", "one\ntwo\n", "one\nthree\n");
        assert!(d.contains("-two") && d.contains("+three"));
    }
}
