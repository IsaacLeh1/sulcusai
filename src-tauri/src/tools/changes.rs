// SPDX-License-Identifier: AGPL-3.0-only
//! `apply_changes`: edits across many files as one unit. Every edit is
//! worked out against the current files first; only when all of them fit
//! is anything written, and a failed write puts back the files already
//! written, so a change can't stop halfway and leave a project broken.

use std::path::PathBuf;

use serde_json::{json, Value};

use super::fs::{diff, read_text, refuse_binary, saw, unchanged_since_seen, with_checks};
use super::{Ctx, Outcome, Preview};
use crate::sandbox::Sandbox;

pub fn apply_changes_params() -> Value {
    json!({ "type": "object", "properties": {
        "changes": { "type": "array", "description": "Edits, applied in order. Several may touch the same file.", "items": { "type": "object", "required": ["path"], "properties": {
            "path": { "type": "string" },
            "old_text": { "type": "string", "description": "Exact text to replace (as edit_file)." },
            "new_text": { "type": "string" },
            "replace_all": { "type": "boolean" },
            "start_line": { "type": "integer", "description": "Replace lines start_line..end_line (as read_file numbers them) with new_text." },
            "end_line": { "type": "integer" },
            "content": { "type": "string", "description": "The whole new file (creates it if missing)." },
            "delete": { "type": "boolean", "description": "Delete the file (it goes to the Recycle Bin)." } } } },
        "diff": { "type": "string", "description": "A unified diff (git diff style, --- a/path, +++ b/path, @@ hunks) instead of, or as well as, changes." },
        "dry_run": { "type": "boolean", "description": "Only show the diff of what would change; write nothing." } } })
}

pub fn undo_changes_params() -> Value {
    json!({ "type": "object", "required": ["batch"], "properties": {
        "batch": { "type": "string", "description": "The batch id apply_changes returned." } } })
}

#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    Edit { path: String, old: String, new: String, all: bool },
    Lines { path: String, start: usize, end: usize, new: String },
    Write { path: String, content: String },
    Delete { path: String },
    Patch { path: String, hunks: Vec<Hunk>, create: bool },
}

impl Change {
    fn path(&self) -> &str {
        match self {
            Change::Edit { path, .. } | Change::Lines { path, .. } | Change::Write { path, .. } | Change::Delete { path } | Change::Patch { path, .. } => path,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hunk {
    pub old_start: usize,
    /// (' ', '-', '+') and the line without its marker.
    pub lines: Vec<(char, String)>,
}

/// One file's planned outcome. `new: None` deletes it.
#[derive(Debug, Clone)]
pub struct Planned {
    pub path: PathBuf,
    pub label: String,
    pub old: Option<String>,
    pub new: Option<String>,
    pub notes: Vec<String>,
}

pub fn parse(args: &Value) -> Result<Vec<Change>, String> {
    let mut out = Vec::new();
    for (i, c) in args.get("changes").and_then(Value::as_array).into_iter().flatten().enumerate() {
        let n = i + 1;
        let path = c.get("path").and_then(Value::as_str).filter(|p| !p.trim().is_empty()).ok_or(format!("Change {n} has no path."))?.to_string();
        let s = |k: &str| c.get(k).and_then(Value::as_str).map(str::to_string);
        if c.get("delete").and_then(Value::as_bool).unwrap_or(false) {
            out.push(Change::Delete { path });
        } else if let Some(content) = s("content") {
            out.push(Change::Write { path, content });
        } else if let Some(start) = c.get("start_line").and_then(Value::as_u64) {
            let end = c.get("end_line").and_then(Value::as_u64).unwrap_or(start);
            if start == 0 || end < start {
                return Err(format!("Change {n}: start_line must be 1 or more and end_line at least start_line."));
            }
            out.push(Change::Lines { path, start: start as usize, end: end as usize, new: s("new_text").unwrap_or_default() });
        } else if let Some(old) = s("old_text") {
            let new = s("new_text").ok_or(format!("Change {n} has old_text but no new_text."))?;
            out.push(Change::Edit { path, old, new, all: c.get("replace_all").and_then(Value::as_bool).unwrap_or(false) });
        } else {
            return Err(format!("Change {n} ({path}) needs old_text and new_text, start_line and new_text, content, or delete."));
        }
    }
    if let Some(d) = args.get("diff").and_then(Value::as_str).filter(|d| !d.trim().is_empty()) {
        out.extend(parse_unified(d)?);
    }
    if out.is_empty() {
        return Err("Nothing to change: give changes, a diff, or both.".into());
    }
    Ok(out)
}

fn strip_prefix(p: &str) -> String {
    let p = p.split('\t').next().unwrap_or(p).trim();
    p.strip_prefix("a/").or_else(|| p.strip_prefix("b/")).unwrap_or(p).to_string()
}

/// Reads a unified diff into one change per file.
pub fn parse_unified(text: &str) -> Result<Vec<Change>, String> {
    let mut out = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let Some(from) = lines[i].strip_prefix("--- ") else {
            i += 1;
            continue;
        };
        let Some(to) = lines.get(i + 1).and_then(|l| l.strip_prefix("+++ ")) else {
            return Err(format!("Line {} of the diff starts a file (---) with no +++ line after it.", i + 1));
        };
        let (from, to) = (from.trim(), to.trim());
        i += 2;
        let mut hunks = Vec::new();
        while i < lines.len() && !lines[i].starts_with("--- ") {
            let l = lines[i];
            if let Some(h) = l.strip_prefix("@@") {
                let old_start = h.trim().strip_prefix('-').and_then(|r| r.split([',', ' ']).next()).and_then(|n| n.parse::<usize>().ok()).unwrap_or(1);
                hunks.push(Hunk { old_start, lines: Vec::new() });
            } else if let Some(h) = hunks.last_mut() {
                match l.chars().next() {
                    Some(m @ (' ' | '-' | '+')) => h.lines.push((m, l[1..].to_string())),
                    None => h.lines.push((' ', String::new())),
                    _ => {} // "\ No newline at end of file", "diff --git", index lines
                }
            }
            i += 1;
        }
        if to == "/dev/null" {
            out.push(Change::Delete { path: strip_prefix(from) });
        } else {
            out.push(Change::Patch { path: strip_prefix(to), hunks, create: from == "/dev/null" });
        }
    }
    if out.is_empty() {
        return Err("The diff has no files in it (expected --- a/path and +++ b/path lines).".into());
    }
    Ok(out)
}

/// Applies hunks to `text`. A hunk whose lines moved is found near where it
/// said; one whose old lines aren't there at all fails with where it was.
pub fn apply_hunks(text: &str, hunks: &[Hunk]) -> Result<String, String> {
    let trailing = text.ends_with('\n');
    let mut lines: Vec<String> = text.replace("\r\n", "\n").lines().map(str::to_string).collect();
    let mut shift: isize = 0;
    for (n, h) in hunks.iter().enumerate() {
        let old: Vec<&str> = h.lines.iter().filter(|(m, _)| *m != '+').map(|(_, l)| l.as_str()).collect();
        let new: Vec<String> = h.lines.iter().filter(|(m, _)| *m != '-').map(|(_, l)| l.clone()).collect();
        let want = (h.old_start.max(1) as isize - 1 + shift).max(0) as usize;
        let fits = |at: usize, loose: bool| {
            at + old.len() <= lines.len()
                && old.iter().enumerate().all(|(k, o)| if loose { lines[at + k].trim() == o.trim() } else { lines[at + k] == *o })
        };
        let place = if old.is_empty() {
            Some(want.min(lines.len()))
        } else {
            let mut best: Option<usize> = None;
            for loose in [false, true] {
                best = (0..=lines.len().saturating_sub(old.len())).filter(|&at| fits(at, loose)).min_by_key(|&at| at.abs_diff(want));
                if best.is_some() {
                    break;
                }
            }
            best
        };
        let Some(at) = place else {
            return Err(format!("hunk {} (at line {}) doesn't match the file: its old lines aren't there. Read the file and make the diff again.", n + 1, h.old_start));
        };
        lines.splice(at..at + old.len(), new.iter().cloned());
        shift += new.len() as isize - old.len() as isize + at as isize - want as isize;
    }
    let mut out = lines.join("\n");
    if trailing || text.is_empty() {
        out.push('\n');
    }
    Ok(out)
}

/// Keeps the file's line endings and byte-order mark as they were.
pub fn keep_style(old: &str, new: &str) -> String {
    let mut s = if old.contains("\r\n") { new.replace("\r\n", "\n").replace('\n', "\r\n") } else { new.to_string() };
    if old.starts_with('\u{feff}') && !s.starts_with('\u{feff}') {
        s.insert(0, '\u{feff}');
    }
    s
}

fn replace_lines(text: &str, start: usize, end: usize, new: &str) -> Result<String, String> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    if start > lines.len() + 1 {
        return Err(format!("the file has {} lines, so line {start} isn't there.", lines.len()));
    }
    let end = end.min(lines.len());
    let mut repl = new.to_string();
    if !repl.is_empty() && !repl.ends_with('\n') && end < lines.len() {
        repl.push('\n');
    }
    let mut out: String = lines[..start - 1].concat();
    out.push_str(&repl);
    out.push_str(&lines[end.max(start - 1)..].concat());
    Ok(out)
}

/// Works out every change against the files as they are now. Err lists
/// every change that doesn't fit, so all of them can be fixed at once.
pub fn plan(changes: &[Change], sb: &Sandbox, chat: &str) -> Result<Vec<Planned>, String> {
    let mut planned: Vec<Planned> = Vec::new();
    let mut errors = Vec::new();
    for (i, c) in changes.iter().enumerate() {
        let n = i + 1;
        let path = match sb.resolve(c.path()) {
            Ok(p) => p,
            Err(e) => {
                errors.push(format!("change {n} ({}): {e}", c.path()));
                continue;
            }
        };
        if let Err(e) = refuse_binary(&path).and_then(|_| super::locks::check(chat, &path)) {
            errors.push(format!("change {n}: {e}"));
            continue;
        }
        let label = sb.display(&path);
        let idx = match planned.iter().position(|p| p.path == path) {
            Some(i) => i,
            None => {
                let old = if path.is_file() {
                    match read_text(&path) {
                        Ok(t) => Some(t),
                        Err(e) => {
                            errors.push(format!("change {n} ({label}): {e}"));
                            continue;
                        }
                    }
                } else {
                    None
                };
                if let Some(o) = &old {
                    if !chat.is_empty() {
                        if let Err(e) = unchanged_since_seen(chat, &path, o, &label) {
                            let before = read_text(&path).unwrap_or_default();
                            errors.push(format!("change {n}: {e}\n{}", diff(&label, o, &before)));
                            continue;
                        }
                    }
                }
                planned.push(Planned { path: path.clone(), label: label.clone(), new: old.clone(), old, notes: Vec::new() });
                planned.len() - 1
            }
        };
        let p = &mut planned[idx];
        let current = p.new.clone();
        let result: Result<Option<String>, String> = match (c, current.as_deref()) {
            (Change::Delete { .. }, None) => Err("it doesn't exist.".into()),
            (Change::Delete { .. }, Some(_)) => Ok(None),
            (Change::Write { content, .. }, cur) => Ok(Some(match cur {
                Some(o) => keep_style(o, content),
                None => content.clone(),
            })),
            (Change::Patch { create: true, hunks, .. }, None) => apply_hunks("", hunks).map(Some),
            (Change::Patch { hunks, .. }, Some(t)) => apply_hunks(t, hunks).map(|s| Some(keep_style(t, &s))),
            (Change::Patch { .. }, None) => Err("it doesn't exist (a diff for a new file starts with --- /dev/null).".into()),
            (Change::Edit { .. } | Change::Lines { .. }, None) => Err("it doesn't exist; use content to create it.".into()),
            (Change::Edit { old, new, all, .. }, Some(t)) => super::fs::apply_edit(t, old, new, *all).map(|(s, _)| Some(s)),
            (Change::Lines { start, end, new, .. }, Some(t)) => replace_lines(t, *start, *end, &keep_style(t, new)).map(Some),
        };
        match result {
            Ok(next) => p.new = next,
            Err(e) => errors.push(format!("change {n} ({label}): {e}")),
        }
    }
    for p in &mut planned {
        if let (Some(o), Some(n)) = (&p.old, &p.new) {
            if o.contains("\r\n") != n.contains("\r\n") {
                p.notes.push(if o.contains("\r\n") { "line endings would change from CRLF to LF".into() } else { "line endings would change from LF to CRLF".into() });
            }
        }
    }
    if errors.is_empty() {
        Ok(planned.into_iter().filter(|p| p.old != p.new).collect())
    } else {
        Err(format!("Nothing was changed. These changes don't fit:\n- {}", errors.join("\n- ")))
    }
}

fn combined_diff(planned: &[Planned]) -> String {
    planned.iter().map(|p| diff(&p.label, p.old.as_deref().unwrap_or(""), p.new.as_deref().unwrap_or(""))).collect::<Vec<_>>().join("\n")
}

fn summary(planned: &[Planned]) -> Vec<String> {
    planned
        .iter()
        .map(|p| {
            let verb = match (&p.old, &p.new) {
                (None, _) => "created",
                (_, None) => "deleted",
                _ => "edited",
            };
            let notes = if p.notes.is_empty() { String::new() } else { format!(" (note: {})", p.notes.join("; ")) };
            format!("{}: {verb}{notes}", p.label)
        })
        .collect()
}

/// Writes the plan; on any failure puts back what was already written.
fn write_all(planned: &[Planned], ctx: &Ctx<'_>, batch: &str) -> Result<(), String> {
    let mut done: Vec<&Planned> = Vec::new();
    let undo = |done: &[&Planned]| {
        for p in done.iter().rev() {
            let _ = match &p.old {
                Some(o) => crate::storage::write_safely(&p.path, o).map_err(|e| e.to_string()),
                None => std::fs::remove_file(&p.path).map_err(|e| e.to_string()),
            };
        }
    };
    for p in planned {
        let step = (|| -> Result<(), String> {
            match &p.new {
                Some(text) => {
                    ctx.recorder.before_write_in(batch, &p.path)?;
                    if let Some(dir) = p.path.parent() {
                        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                    }
                    crate::storage::write_safely(&p.path, text).map_err(|e| format!("couldn't write {}: {e}", p.label))
                }
                None => {
                    ctx.recorder.before_delete_in(batch, &p.path)?;
                    crate::checkpoint::recycle(&p.path)
                }
            }
        })();
        if let Err(e) = step {
            undo(&done);
            return Err(format!("Nothing was changed: {e}. The files already written were put back."));
        }
        done.push(p);
    }
    Ok(())
}

pub fn preview(args: &Value, sb: &Sandbox) -> Result<Preview, String> {
    let planned = plan(&parse(args)?, sb, "")?;
    Ok(Preview {
        title: format!("Change {} file{}", planned.len(), if planned.len() == 1 { "" } else { "s" }),
        kind: "diff",
        detail: Some(combined_diff(&planned)),
        note: Some("All of these are written together, or none of them. Undo puts them all back.".into()),
    })
}

pub fn run(name: &str, args: &Value, ctx: &Ctx<'_>) -> Outcome {
    if name == "undo_changes" {
        let batch = args.get("batch").and_then(Value::as_str).unwrap_or("").trim();
        let conn = ctx.memory.db.lock().unwrap();
        if crate::db::batch_checkpoints(&conn, ctx.memory.cipher, batch).is_empty() {
            return Outcome::error("Undo changes", format!("There's nothing left to undo in batch {batch}."));
        }
        let notes = crate::checkpoint::undo_batch(&conn, ctx.memory.cipher, batch);
        let text = if notes.is_empty() { format!("Undid batch {batch}: every file is back as it was.") } else { format!("Undid batch {batch}, except: {}", notes.join("; ")) };
        return Outcome::ok(text, format!("Undid batch {batch}"), "text", None);
    }
    match apply(args, ctx) {
        Ok(o) => o,
        Err(e) => Outcome::error("Apply changes", e),
    }
}

/// Applies a list of changes as one unit; also used by `rename_symbol`.
pub fn apply(args: &Value, ctx: &Ctx<'_>) -> Result<Outcome, String> {
    let changes = parse(args)?;
    let planned = plan(&changes, ctx.sandbox, ctx.memory.chat_id)?;
    if planned.is_empty() {
        return Err("These changes leave every file as it already is.".into());
    }
    let lines = summary(&planned);
    let detail = combined_diff(&planned);
    if args.get("dry_run").and_then(Value::as_bool).unwrap_or(false) {
        return Ok(Outcome::ok(format!("Dry run, nothing written. It would change:\n{}\n\n{detail}", lines.join("\n")), format!("Would change {} files", planned.len()), "diff", Some(detail)));
    }
    let batch = format!("b{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
    write_all(&planned, ctx, &batch)?;
    let mut warnings = Vec::new();
    for p in &planned {
        if let Some(new) = &p.new {
            saw(ctx.memory.chat_id, &p.path, new);
            crate::handoff_guard::wrote(ctx.state, ctx.memory.cipher, ctx.memory.chat_id, &p.path, new);
            let checked = with_checks(Outcome::ok("", "", "text", None), &p.path, &p.label, p.old.as_deref().unwrap_or(""), new);
            if !checked.text.is_empty() {
                warnings.push(checked.text.trim().to_string());
            }
        }
    }
    let mut text = format!("Changed {} file{} as one batch ({batch}; undo_changes with this id puts them all back):\n{}", planned.len(), if planned.len() == 1 { "" } else { "s" }, lines.join("\n"));
    for w in warnings {
        text.push_str("\n\n");
        text.push_str(&w);
    }
    let mut o = Outcome::ok(text, format!("Changed {} files", planned.len()), "diff", Some(detail));
    o.meta["batch"] = json!(batch);
    o.meta["files"] = json!(planned.iter().map(|p| p.path.display().to_string()).collect::<Vec<_>>());
    Ok(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sulcus-changes-{name}-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_unified_diff_applies_even_when_lines_moved() {
        let d = "--- a/x.txt\n+++ b/x.txt\n@@ -2,3 +2,3 @@\n b\n-c\n+C\n d\n";
        let changes = parse_unified(d).unwrap();
        let Change::Patch { hunks, .. } = &changes[0] else { panic!() };
        assert_eq!(apply_hunks("a\nb\nc\nd\ne\n", hunks).unwrap(), "a\nb\nC\nd\ne\n");
        assert_eq!(apply_hunks("new\nnew\na\nb\nc\nd\n", hunks).unwrap(), "new\nnew\na\nb\nC\nd\n");
        assert!(apply_hunks("nothing here\n", hunks).is_err());
    }

    #[test]
    fn new_and_deleted_files_come_from_dev_null() {
        let d = "--- /dev/null\n+++ b/n.txt\n@@ -0,0 +1,2 @@\n+one\n+two\n--- a/old.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-x\n";
        let c = parse_unified(d).unwrap();
        assert!(matches!(&c[0], Change::Patch { create: true, path, .. } if path == "n.txt"));
        assert_eq!(c[1], Change::Delete { path: "old.txt".into() });
        let Change::Patch { hunks, .. } = &c[0] else { panic!() };
        assert_eq!(apply_hunks("", hunks).unwrap(), "one\ntwo\n");
    }

    #[test]
    fn crlf_files_stay_crlf() {
        assert_eq!(keep_style("a\r\nb\r\n", "a\nc\n"), "a\r\nc\r\n");
        assert_eq!(replace_lines("1\n2\n3\n", 2, 2, "two").unwrap(), "1\ntwo\n3\n");
        assert_eq!(replace_lines("1\n2\n3\n", 2, 3, "").unwrap(), "1\n");
    }

    #[test]
    fn one_bad_change_of_ten_leaves_all_ten_untouched() {
        let dir = tmp("ten");
        let sb = Sandbox::new(&[dir.display().to_string()]);
        let root = dir.file_name().unwrap().to_string_lossy().to_string();
        let mut list = Vec::new();
        for i in 0..10 {
            std::fs::write(dir.join(format!("f{i}.txt")), format!("value {i}\n")).unwrap();
            let old = if i == 6 { "not there".to_string() } else { format!("value {i}") };
            list.push(Change::Edit { path: format!("{root}/f{i}.txt"), old, new: "changed".into(), all: false });
        }
        let err = plan(&list, &sb, "t-ten").unwrap_err();
        assert!(err.contains("change 7"), "{err}");
        for i in 0..10 {
            assert_eq!(std::fs::read_to_string(dir.join(format!("f{i}.txt"))).unwrap(), format!("value {i}\n"));
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_change_outside_the_shared_folders_is_refused_before_anything() {
        let dir = tmp("outside");
        let sb = Sandbox::new(&[dir.display().to_string()]);
        let root = dir.file_name().unwrap().to_string_lossy().to_string();
        let list = vec![
            Change::Write { path: format!("{root}/ok.txt"), content: "x".into() },
            Change::Write { path: "C:/Windows/evil.txt".into(), content: "x".into() },
        ];
        assert!(plan(&list, &sb, "t-out").is_err());
        assert!(!dir.join("ok.txt").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn edits_to_one_file_build_on_each_other() {
        let dir = tmp("chain");
        let sb = Sandbox::new(&[dir.display().to_string()]);
        let root = dir.file_name().unwrap().to_string_lossy().to_string();
        std::fs::write(dir.join("a.rs"), "fn a() {}\r\nfn b() {}\r\n").unwrap();
        let list = vec![
            Change::Edit { path: format!("{root}/a.rs"), old: "fn a() {}".into(), new: "fn a2() {}".into(), all: false },
            Change::Lines { path: format!("{root}/a.rs"), start: 2, end: 2, new: "fn b2() {}\n".into() },
        ];
        let p = plan(&list, &sb, "t-chain").unwrap();
        assert_eq!(p[0].new.as_deref(), Some("fn a2() {}\r\nfn b2() {}\r\n"));
        assert!(p[0].notes.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
