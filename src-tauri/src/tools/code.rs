// SPDX-License-Identifier: AGPL-3.0-only
//! Code-aware tools: where a name is defined and used (whole words, outside
//! comments and strings), a rename that changes exactly those places as one
//! `apply_changes` batch, what imports a file, and compiler and type
//! checker errors as file, line and message, so a model knows what it broke
//! without reading a whole build log.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::{arg_str, Ctx, Outcome};

const CODE_EXT: &[&str] = &["rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "go", "java", "kt", "cs", "c", "h", "cpp", "hpp", "swift", "rb", "php", "vue", "svelte"];
const MAX_FILES: usize = 20_000;
const MAX_REFS: usize = 300;

pub fn find_references_params() -> Value {
    json!({ "type": "object", "required": ["name"], "properties": {
        "name": { "type": "string", "description": "A function, type, variable or other identifier." },
        "path": { "type": "string", "description": "Folder to look in (default: the shared folders)." } } })
}
pub fn rename_symbol_params() -> Value {
    json!({ "type": "object", "required": ["name", "new_name"], "properties": {
        "name": { "type": "string" }, "new_name": { "type": "string" },
        "path": { "type": "string" },
        "dry_run": { "type": "boolean", "description": "Show the diff only." } } })
}
pub fn find_importers_params() -> Value {
    json!({ "type": "object", "required": ["path"], "properties": { "path": { "type": "string", "description": "The file whose importers to list." } } })
}
pub fn file_tree_params() -> Value {
    json!({ "type": "object", "properties": {
        "path": { "type": "string", "description": "The folder (default: the shared folders)." },
        "depth": { "type": "integer", "description": "Levels to show (1 to 6, default 3)." } } })
}
pub fn list_errors_params() -> Value {
    json!({ "type": "object", "required": ["path"], "properties": {
        "path": { "type": "string", "description": "The project folder." },
        "kind": { "type": "string", "enum": ["auto", "rust", "typescript"], "description": "Which checker (auto: by the project's files)." } } })
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

fn valid_ident(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_ident) && !s.chars().next().unwrap().is_ascii_digit()
}

/// The code part of each line: comments and string contents blanked to
/// spaces (columns stay put), carried across lines for block comments.
pub fn code_only(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_block = false;
    for line in text.lines() {
        let chars: Vec<char> = line.chars().collect();
        let mut s = String::with_capacity(line.len());
        let mut i = 0;
        let mut quote: Option<char> = None;
        while i < chars.len() {
            let c = chars[i];
            let next = chars.get(i + 1).copied();
            if in_block {
                if c == '*' && next == Some('/') {
                    in_block = false;
                    s.push_str("  ");
                    i += 2;
                } else {
                    s.push(' ');
                    i += 1;
                }
                continue;
            }
            if let Some(q) = quote {
                if c == '\\' {
                    s.push_str("  ");
                    i += 2;
                    continue;
                }
                if c == q {
                    quote = None;
                    s.push(c);
                } else {
                    s.push(' ');
                }
                i += 1;
                continue;
            }
            match (c, next) {
                ('/', Some('/')) => {
                    s.extend(std::iter::repeat_n(' ', chars.len() - i));
                    break;
                }
                ('/', Some('*')) => {
                    in_block = true;
                    s.push_str("  ");
                    i += 2;
                    continue;
                }
                ('"' | '`', _) => quote = Some(c),
                // A Rust lifetime ('a) isn't a quote; a char literal is short.
                ('\'', _) if chars.get(i + 2) == Some(&'\'') || (next == Some('\\')) => quote = Some(c),
                _ => {}
            }
            s.push(c);
            i += 1;
        }
        out.push(s);
    }
    out
}

/// Columns (in chars) where `name` stands as a whole word.
fn word_at(line: &str, name: &str) -> Vec<usize> {
    let chars: Vec<char> = line.chars().collect();
    let n: Vec<char> = name.chars().collect();
    let mut hits = Vec::new();
    let mut i = 0;
    while i + n.len() <= chars.len() {
        if chars[i..i + n.len()] == n[..] && (i == 0 || !is_ident(chars[i - 1])) && chars.get(i + n.len()).is_none_or(|c| !is_ident(*c)) {
            hits.push(i);
            i += n.len();
        } else {
            i += 1;
        }
    }
    hits
}

fn looks_like_definition(line: &str, col: usize) -> bool {
    let before: String = line.chars().take(col).collect();
    let b = before.trim_end();
    ["fn", "struct", "enum", "trait", "type", "const", "static", "let", "mod", "function", "class", "interface", "def", "var", "impl"].iter().any(|k| b.ends_with(k) && b[..b.len() - k.len()].chars().last().is_none_or(|c| !is_ident(c)))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reference {
    pub file: PathBuf,
    pub line: usize,
    pub col: usize,
    pub text: String,
    pub definition: bool,
}

pub fn references_in(file: &Path, text: &str, name: &str) -> Vec<Reference> {
    let code = code_only(text);
    let raw: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    for (i, line) in code.iter().enumerate() {
        for col in word_at(line, name) {
            out.push(Reference { file: file.to_path_buf(), line: i + 1, col: col + 1, text: raw.get(i).unwrap_or(&"").trim().to_string(), definition: looks_like_definition(line, col) });
        }
    }
    out
}

fn code_files(ctx: &Ctx<'_>, path: Option<&str>) -> Result<Vec<PathBuf>, String> {
    let roots = match path {
        Some(p) => vec![ctx.sandbox.resolve_read(p)?],
        None => ctx.sandbox.roots().to_vec(),
    };
    let files = super::fs::walk_files(&roots, None, MAX_FILES)?;
    Ok(files.into_iter().filter(|f| f.extension().and_then(|e| e.to_str()).is_some_and(|e| CODE_EXT.contains(&e))).collect())
}

/// `a/b/../c` as `a/c`, without touching the disk.
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Renames `name` to `new_name` at every code reference: the changes, one per place.
pub fn rename_changes(files: &[(PathBuf, String)], name: &str, new_name: &str) -> Vec<(PathBuf, String, String)> {
    let mut out = Vec::new();
    for (path, text) in files {
        let refs = references_in(path, text, name);
        if refs.is_empty() {
            continue;
        }
        let mut lines: Vec<String> = text.split_inclusive('\n').map(str::to_string).collect();
        for r in refs.iter().rev() {
            let l = &mut lines[r.line - 1];
            let chars: Vec<char> = l.chars().collect();
            let mut s: String = chars[..r.col - 1].iter().collect();
            s.push_str(new_name);
            s.extend(&chars[r.col - 1 + name.chars().count()..]);
            *l = s;
        }
        out.push((path.clone(), text.clone(), lines.concat()));
    }
    out
}

/// One error or warning from a checker.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Problem {
    pub file: String,
    pub line: usize,
    pub col: usize,
    pub severity: String,
    pub message: String,
}

/// Reads rustc's short format (`src/a.rs:3:5: error[E0425]: ...`), tsc's
/// (`src/a.ts(3,5): error TS2304: ...`) and the common `file:line:col: msg`.
pub fn parse_problems(out: &str) -> Vec<Problem> {
    let tsc = regex::Regex::new(r"^(?P<f>[^\s(][^(]*)\((?P<l>\d+),(?P<c>\d+)\): (?P<s>error|warning) (?P<m>.+)$").unwrap();
    let colon = regex::Regex::new(r"^(?P<f>(?:[A-Za-z]:)?[^:\s][^:]*):(?P<l>\d+):(?P<c>\d+):\s*(?P<s>error|warning)?(?:\[[^\]]*\])?:?\s*(?P<m>.+)$").unwrap();
    let mut seen = std::collections::HashSet::new();
    out.lines()
        .filter_map(|l| tsc.captures(l.trim()).or_else(|| colon.captures(l.trim())))
        .map(|c| Problem {
            file: c["f"].trim().to_string(),
            line: c["l"].parse().unwrap_or(0),
            col: c["c"].parse().unwrap_or(0),
            severity: c.name("s").map_or("error", |m| m.as_str()).to_string(),
            message: c["m"].trim().to_string(),
        })
        .filter(|p| seen.insert((p.file.clone(), p.line, p.message.clone())))
        .collect()
}

/// The checker command for a project folder.
pub fn checker(dir: &Path, kind: &str) -> Option<(&'static str, String)> {
    let rust = || dir.join("Cargo.toml").exists();
    let ts = || dir.join("tsconfig.json").exists();
    match kind {
        "rust" => Some(("rust", "cargo check --message-format=short --quiet".into())),
        "typescript" => Some(("typescript", "node node_modules/typescript/bin/tsc --noEmit --pretty false -p .".into())),
        _ if ts() => Some(("typescript", "node node_modules/typescript/bin/tsc --noEmit --pretty false -p .".into())),
        _ if rust() => Some(("rust", "cargo check --message-format=short --quiet".into())),
        _ => None,
    }
}

pub fn problems_text(problems: &[Problem], limit: usize) -> String {
    let errors = problems.iter().filter(|p| p.severity == "error").count();
    let mut s = format!("{errors} error{}, {} warning{}.", if errors == 1 { "" } else { "s" }, problems.len() - errors, if problems.len() - errors == 1 { "" } else { "s" });
    let mut sorted: Vec<&Problem> = problems.iter().collect();
    sorted.sort_by_key(|p| p.severity != "error");
    for p in sorted.iter().take(limit) {
        s.push_str(&format!("\n{}:{}:{}: {}: {}", p.file, p.line, p.col, p.severity, p.message));
    }
    if problems.len() > limit {
        s.push_str(&format!("\n… {} more not shown.", problems.len() - limit));
    }
    s
}

pub async fn run(name: &str, args: &Value, ctx: &Ctx<'_>) -> Outcome {
    if name == "list_errors" {
        return match list_errors(args, ctx).await {
            Ok(o) => o,
            Err(e) => Outcome::error("List errors", e),
        };
    }
    let (n, a) = (name.to_string(), args.clone());
    tokio::task::block_in_place(|| run_sync(&n, &a, ctx)).unwrap_or_else(|e| Outcome::error(super::failed_title(name, args), e))
}

async fn list_errors(args: &Value, ctx: &Ctx<'_>) -> Result<Outcome, String> {
    let dir = ctx.sandbox.resolve(arg_str(args, "path")?)?;
    let kind = args.get("kind").and_then(Value::as_str).unwrap_or("auto");
    let (which, cmd) = checker(&dir, kind).ok_or("No Cargo.toml or tsconfig.json there; say kind, or run the project's check with run_command.")?;
    let run = super::shell::run_check(&cmd, &dir, &[], "", std::time::Duration::from_secs(900)).await?;
    let all = format!("{}\n{}", run.out, run.err);
    let problems = parse_problems(&all);
    let text = if problems.is_empty() && run.code == Some(0) {
        format!("The {which} check passed: no errors.")
    } else if problems.is_empty() {
        format!("The {which} check failed, but its output had no file:line errors:\n{}", super::git::page(&all, 1, 60))
    } else {
        problems_text(&problems, 40)
    };
    let mut o = Outcome::ok(text, format!("Checked {} ({which})", ctx.sandbox.display(&dir)), "text", None);
    o.meta["problems"] = json!(problems);
    Ok(o)
}

fn run_sync(name: &str, args: &Value, ctx: &Ctx<'_>) -> Result<Outcome, String> {
    let path = args.get("path").and_then(Value::as_str).filter(|p| !p.trim().is_empty());
    match name {
        "find_references" => {
            let sym = arg_str(args, "name")?.trim();
            if !valid_ident(sym) {
                return Err(format!("{sym:?} isn't a single identifier."));
            }
            let mut refs = Vec::new();
            for f in code_files(ctx, path)? {
                if let Ok(text) = std::fs::read_to_string(&f) {
                    refs.extend(references_in(&f, &text, sym));
                }
            }
            if refs.is_empty() {
                return Ok(Outcome::ok(format!("{sym} isn't used in code here (comments and strings don't count)."), format!("References to {sym}"), "text", None));
            }
            refs.sort_by_key(|r| !r.definition);
            let files: std::collections::HashSet<_> = refs.iter().map(|r| r.file.clone()).collect();
            let mut s = format!("{} references in {} files", refs.len(), files.len());
            for r in refs.iter().take(MAX_REFS) {
                s.push_str(&format!("\n{}:{}:{}{} {}", ctx.sandbox.display(&r.file), r.line, r.col, if r.definition { " (definition)" } else { "" }, r.text));
            }
            if refs.len() > MAX_REFS {
                s.push_str(&format!("\n… cut off after {MAX_REFS}; give a path to narrow it."));
            }
            Ok(Outcome::ok(s, format!("References to {sym}"), "text", None))
        }
        "rename_symbol" => {
            let (from, to) = (arg_str(args, "name")?.trim(), arg_str(args, "new_name")?.trim());
            if !valid_ident(from) || !valid_ident(to) {
                return Err("name and new_name must each be one identifier.".into());
            }
            let texts: Vec<(PathBuf, String)> = code_files(ctx, path)?.into_iter().filter_map(|f| std::fs::read_to_string(&f).ok().map(|t| (f, t))).collect();
            let clash: Vec<String> = texts.iter().filter(|(f, t)| !references_in(f, t, to).is_empty()).map(|(f, _)| ctx.sandbox.display(f)).take(5).collect();
            if !clash.is_empty() {
                return Err(format!("{to} is already used in {}; pick a name that isn't.", clash.join(", ")));
            }
            let changes: Vec<Value> = rename_changes(&texts, from, to).into_iter().map(|(p, _, new)| json!({ "path": p.display().to_string(), "content": new })).collect();
            if changes.is_empty() {
                return Err(format!("{from} isn't used in code here."));
            }
            let mut a = json!({ "changes": changes });
            a["dry_run"] = args.get("dry_run").cloned().unwrap_or(json!(false));
            super::changes::apply(&a, ctx)
        }
        "find_importers" => {
            let target = ctx.sandbox.resolve_read(arg_str(args, "path")?)?;
            let stem = target.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
            let root = ctx.sandbox.roots().iter().find(|r| target.starts_with(r)).cloned().unwrap_or_else(|| target.parent().unwrap_or(&target).to_path_buf());
            let import = regex::Regex::new(r#"^\s*(import\b.*from\s+['"](?P<a>[^'"]+)['"]|import\s+['"](?P<b>[^'"]+)['"]|(?:export\s+.*from\s+)['"](?P<c>[^'"]+)['"]|.*require\(['"](?P<d>[^'"]+)['"]\)|(pub\s+)?mod\s+(?P<m>\w+)\s*;|use\s+(?:crate|super|self)::(?P<u>[\w:]+))"#).unwrap();
            let mut hits = Vec::new();
            for f in code_files(ctx, Some(&root.display().to_string()))? {
                if f == target {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&f) else { continue };
                for (i, line) in text.lines().enumerate() {
                    let Some(c) = import.captures(line) else { continue };
                    let spec = ["a", "b", "c", "d"].iter().find_map(|k| c.name(k)).map(|m| m.as_str());
                    let found = match spec {
                        Some(s) if s.starts_with('.') => {
                            let base = f.parent().unwrap_or(&f).join(s);
                            let n = |p: &Path| p.to_string_lossy().replace('\\', "/").to_lowercase();
                            let norm = n(&normalize(&base));
                            let t = n(&target);
                            t == norm || t.starts_with(&format!("{norm}.")) || t == format!("{norm}/index.ts") || t == format!("{norm}/index.tsx")
                        }
                        Some(_) => false,
                        None => c.name("m").is_some_and(|m| m.as_str() == stem && f.parent() == target.parent().or(target.parent()))
                            || c.name("u").is_some_and(|u| u.as_str().split("::").any(|p| p == stem)),
                    };
                    if found {
                        hits.push(format!("{}:{}: {}", ctx.sandbox.display(&f), i + 1, line.trim()));
                    }
                }
            }
            let text = if hits.is_empty() { format!("Nothing here imports {}.", ctx.sandbox.display(&target)) } else { format!("{} imports:\n{}", hits.len(), hits.join("\n")) };
            Ok(Outcome::ok(text, format!("What imports {}", ctx.sandbox.display(&target)), "text", None))
        }
        "file_tree" => {
            let depth = args.get("depth").and_then(Value::as_u64).unwrap_or(3).clamp(1, 6) as usize;
            let roots = match path {
                Some(p) => vec![ctx.sandbox.resolve_read(p)?],
                None => ctx.sandbox.roots().to_vec(),
            };
            let mut out = String::new();
            for r in &roots {
                let files = super::fs::walk_files(std::slice::from_ref(r), None, MAX_FILES)?;
                out.push_str(&tree_text(r, &files, depth, &ctx.sandbox.display(r)));
                if files.len() >= MAX_FILES {
                    out.push_str(&format!("… cut off at {MAX_FILES} files.
"));
                }
            }
            Ok(Outcome::ok(out, "File overview", "text", None))
        }
        other => Err(format!("Unknown tool {other}.")),
    }
}

/// Folders a few levels deep with how many files each holds (all levels
/// below counted), and the files at the shallow levels.
pub fn tree_text(root: &Path, files: &[PathBuf], depth: usize, label: &str) -> String {
    use std::collections::BTreeMap;
    let mut dirs: BTreeMap<Vec<String>, usize> = BTreeMap::new();
    let mut shown: Vec<Vec<String>> = Vec::new();
    for f in files {
        let Ok(rel) = f.strip_prefix(root) else { continue };
        let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        for k in 1..parts.len() {
            if k <= depth {
                *dirs.entry(parts[..k].to_vec()).or_default() += 1;
            }
        }
        if parts.len() <= depth {
            shown.push(parts);
        }
    }
    let mut entries: Vec<(Vec<String>, Option<usize>)> = dirs.into_iter().map(|(k, n)| (k, Some(n))).chain(shown.into_iter().map(|k| (k, None))).collect();
    entries.sort();
    let mut out = format!("{label}/ ({} files)
", files.len());
    let mut lines = 0;
    for (k, n) in entries {
        lines += 1;
        if lines > 400 {
            out.push_str("… cut off at 400 lines; ask for a smaller depth or a subfolder.
");
            break;
        }
        let indent = "  ".repeat(k.len());
        match n {
            Some(n) => out.push_str(&format!("{indent}{}/ ({n})
", k.last().unwrap())),
            None => out.push_str(&format!("{indent}{}
", k.last().unwrap())),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tree_counts_files_per_folder() {
        let root = Path::new("/p");
        let files: Vec<PathBuf> = ["a.txt", "src/b.rs", "src/c/d.rs", "src/c/e/f.rs"].iter().map(|f| root.join(f)).collect();
        let t = tree_text(root, &files, 2, "p");
        assert!(t.starts_with("p/ (4 files)
"));
        assert!(t.contains("  src/ (3)
") && t.contains("    c/ (2)
") && t.contains("    b.rs
") && !t.contains("d.rs"), "{t}");
    }

    #[test]
    fn references_skip_comments_strings_and_longer_names() {
        let text = "fn parse() {}\n// parse is here\nlet s = \"parse\";\nparse(); parse_all();\n/* parse\n parse */ x.parse()\n";
        let refs = references_in(Path::new("a.rs"), text, "parse");
        let at: Vec<(usize, bool)> = refs.iter().map(|r| (r.line, r.definition)).collect();
        assert_eq!(at, vec![(1, true), (4, false), (6, false)]);
    }

    #[test]
    fn a_rename_used_in_twelve_files_touches_exactly_its_references() {
        let files: Vec<(PathBuf, String)> = (0..12)
            .map(|i| (PathBuf::from(format!("f{i}.rs")), format!("use crate::old_name;\nfn f{i}() {{ old_name(); old_name_two(); }}\n// old_name\n")))
            .chain(std::iter::once((PathBuf::from("none.rs"), "fn other() {}\n".to_string())))
            .collect();
        let changes = rename_changes(&files, "old_name", "new_name");
        assert_eq!(changes.len(), 12);
        assert_eq!(changes[0].2, "use crate::new_name;\nfn f0() { new_name(); old_name_two(); }\n// old_name\n");
    }

    #[test]
    fn the_first_type_error_comes_with_file_and_line() {
        let tsc = "src/a.ts(3,5): error TS2304: Cannot find name 'x'.\nsrc/b.tsx(10,1): warning TS6133: unused\n";
        let p = parse_problems(tsc);
        assert_eq!(p[0], Problem { file: "src/a.ts".into(), line: 3, col: 5, severity: "error".into(), message: "TS2304: Cannot find name 'x'.".into() });
        let rust = "src\\lib.rs:12:9: error[E0425]: cannot find value `y` in this scope\nsrc\\lib.rs:2:1: warning: unused import\n";
        let r = parse_problems(rust);
        assert_eq!((r[0].file.as_str(), r[0].line, r[0].severity.as_str()), ("src\\lib.rs", 12, "error"));
        assert!(r[0].message.contains("cannot find value"));
        assert!(problems_text(&r, 1).starts_with("1 error, 1 warning.\nsrc\\lib.rs:12:9"));
    }
}
