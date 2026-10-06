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
pub fn edit_file_params() -> Value {
    json!({ "type": "object", "required": ["path", "old_text", "new_text"], "properties": {
        "path": { "type": "string" },
        "old_text": { "type": "string", "description": "Exact text to replace, copied from the file." },
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
pub fn apply_edit(text: &str, old: &str, new: &str, replace_all: bool) -> Result<(String, usize), String> {
    if old.is_empty() {
        return Err("old_text is empty. To create or replace a whole file, use write_file.".into());
    }
    if !text.contains(old) {
        // Retry without copied line numbers before giving up.
        if let Some(stripped_old) = strip_line_numbers(old).filter(|o| text.contains(o.as_str()) || text.contains(&o.replace('\n', "\r\n"))) {
            let stripped_new = strip_line_numbers(new).unwrap_or_else(|| new.to_string());
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
            Err("old_text wasn't found. Read the file again and copy the text exactly, including spaces and indentation.".into())
        }
        1 => Ok((text.replacen(old, new, 1), 1)),
        n if replace_all => Ok((text.replace(old, new), n)),
        n => Err(format!("old_text appears {n} times. Include more surrounding lines so it matches exactly one place, or set replace_all.")),
    }
}

pub fn preview(name: &str, args: &Value, sb: &Sandbox) -> Result<Preview, String> {
    let p = |key: &str| -> Result<std::path::PathBuf, String> { sb.resolve(arg_str(args, key)?) };
    Ok(match name {
        "write_file" => {
            let path = p("path")?;
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
        other => Preview { title: other.to_string(), kind: "text", detail: None, note: None },
    })
}

pub fn run(name: &str, args: &Value, ctx: &Ctx<'_>) -> Outcome {
    let sb = ctx.sandbox;
    let result: Result<Outcome, String> = (|| {
        match name {
            "list_dir" => list_dir(sb, args.get("path").and_then(Value::as_str)),
            "read_file" => {
                let path = sb.resolve(arg_str(args, "path")?)?;
                let text = read_text(&path)?;
                let lines: Vec<&str> = text.lines().collect();
                let start = args.get("start_line").and_then(Value::as_u64).map_or(1, |n| n.max(1) as usize);
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
            "search_files" => search_files(sb, args),
            "write_file" => {
                let path = sb.resolve(arg_str(args, "path")?)?;
                let raw = arg_str(args, "content")?;
                let stripped = strip_line_numbers(raw);
                let content = stripped.as_deref().unwrap_or(raw);
                let label = sb.display(&path);
                let old = if path.exists() { read_text(&path)? } else { String::new() };
                let existed = path.exists();
                ctx.recorder.before_write(&path)?;
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                }
                std::fs::write(&path, content).map_err(|e| format!("Couldn't write {label}: {e}"))?;
                let verb = if existed { "Replaced" } else { "Created" };
                Ok(Outcome::ok(
                    format!("{verb} {label} ({} lines).", line_count(content)),
                    format!("{verb} {label}"),
                    "diff",
                    Some(diff(&label, &old, content)),
                ))
            }
            "edit_file" => {
                let path = sb.resolve(arg_str(args, "path")?)?;
                let label = sb.display(&path);
                let old = read_text(&path)?;
                let replace_all = args.get("replace_all").and_then(Value::as_bool).unwrap_or(false);
                let (new, n) = apply_edit(&old, arg_str(args, "old_text")?, arg_str(args, "new_text")?, replace_all)?;
                ctx.recorder.before_write(&path)?;
                std::fs::write(&path, &new).map_err(|e| format!("Couldn't write {label}: {e}"))?;
                Ok(Outcome::ok(
                    format!("Edited {label} ({n} replacement{}).", if n == 1 { "" } else { "s" }),
                    format!("Edited {label}"),
                    "diff",
                    Some(diff(&label, &old, &new)),
                ))
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
        if sb.is_empty() {
            return Err("No folders are shared yet.".into());
        }
        let names: Vec<String> = sb.roots().iter().map(|r| format!("{}/", sb.display(r))).collect();
        return Ok(Outcome::ok(format!("Shared folders:\n{}", names.join("\n")), "Listed shared folders", "text", None));
    };
    let dir = sb.resolve(path)?;
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
        Some(p) => Ok(vec![sb.resolve(p)?]),
        None if sb.is_empty() => Err("No folders are shared yet.".into()),
        None => Ok(sb.roots().to_vec()),
    }
}

/// Files under each root, honoring .gitignore and skipping build/dependency
/// folders. Globs are matched relative to each root, so each gets its own walk.
fn walk_files(roots: &[std::path::PathBuf], glob: Option<&str>, limit: usize) -> Result<Vec<std::path::PathBuf>, String> {
    let mut files = Vec::new();
    for root in roots {
        let mut ov = ignore::overrides::OverrideBuilder::new(root);
        // Never descend into these, even without a .gitignore.
        for skip in ["!.git/", "!node_modules/", "!target/", "!.venv/", "!__pycache__/"] {
            ov.add(skip).map_err(|e| e.to_string())?;
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
    let found: Vec<String> = walk_files(&roots, Some(pattern), MAX_LIST)?.iter().map(|p| sb.display(p)).collect();
    let text = if found.is_empty() { format!("No files match {pattern}.") } else { found.join("\n") };
    Ok(Outcome::ok(text, format!("Found {} file{} matching {pattern}", found.len(), if found.len() == 1 { "" } else { "s" }), "text", None))
}

fn search_files(sb: &Sandbox, args: &Value) -> Result<Outcome, String> {
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
        let Ok(text) = read_text(&file) else { continue };
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
    fn edits_must_match_exactly_once_unless_replace_all() {
        assert_eq!(apply_edit("a b a", "b", "c", false).unwrap(), ("a c a".into(), 1));
        assert!(apply_edit("a b a", "a", "c", false).unwrap_err().contains("2 times"));
        assert_eq!(apply_edit("a b a", "a", "c", true).unwrap(), ("c b c".into(), 2));
        assert!(apply_edit("abc", "zzz", "y", false).is_err());
        assert!(apply_edit("abc", "", "y", false).is_err());
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
