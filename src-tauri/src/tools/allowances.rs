// SPDX-License-Identifier: AGPL-3.0-only
//! Rules the person saves for what the assistant may do without asking: a
//! tool (or tools by prefix), optionally what it acts on (a command, a path,
//! an address), and Allow, Ask or Deny. Deny wins, then Ask, then Allow;
//! with no rule the run mode decides as before. An Allow never loosens the
//! checks that always ask (sending, deleting, anything leaving the PC) or
//! the commands the guard refuses.
//!
//! Rules belong to this PC: they don't sync, since what a path or a command
//! does differs from one PC to the next.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use regex::Regex;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::guard;
use crate::crypto::Cipher;
use crate::sandbox::Sandbox;
use crate::AppStateRef;

const TABLE: &str = "standing_permissions";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Effect {
    Allow,
    Ask,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Allowance {
    #[serde(default)]
    pub id: String,
    /// A tool name, or a prefix ending in `*` (`browser_*`).
    pub tool: String,
    /// What it acts on, with `*` (any run of characters, or within one folder
    /// for paths) and `**` (any folders). None: any use of the tool.
    #[serde(default)]
    pub pattern: Option<String>,
    pub effect: Effect,
    /// Only in chats of this project; None for everywhere.
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub created_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    Command,
    Path,
    Url,
    Text,
}

const PATH_TOOLS: &[&str] = &["read_file", "write_file", "edit_file", "delete_path", "make_folder", "list_dir", "find_files", "search_files", "preview_site"];

/// What a pattern for this tool is matched against; None when it acts on
/// nothing a pattern could name.
fn kind_of(tool: &str) -> Option<Kind> {
    match tool {
        "run_command" => Some(Kind::Command),
        "move_path" | "open_path" => Some(Kind::Path),
        "fetch_page" | "browser_open" => Some(Kind::Url),
        "email_send" => Some(Kind::Text),
        t if PATH_TOOLS.contains(&t) => Some(Kind::Path),
        _ => None,
    }
}

/// What the call acts on, each matched on its own: a command's pieces, a
/// path as it resolves (so `..` can't step around a rule), an address.
fn subjects(tool: &str, args: &Value, sandbox: &Sandbox) -> Vec<(String, Kind)> {
    let arg = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty());
    let path = |p: &str| (resolved(p, sandbox), Kind::Path);
    match tool {
        "run_command" => arg("command").map(|c| guard::pieces(c).into_iter().map(|p| (p, Kind::Command)).collect()).unwrap_or_default(),
        "move_path" => ["from", "to"].into_iter().filter_map(arg).map(path).collect(),
        "open_path" => arg("target").map(|t| if t.contains("://") { (t.to_string(), Kind::Url) } else { path(t) }).into_iter().collect(),
        "fetch_page" | "browser_open" => arg("url").map(|u| (u.to_string(), Kind::Url)).into_iter().collect(),
        "email_send" => ["to", "cc"]
            .into_iter()
            .filter_map(arg)
            .flat_map(|list| list.split([',', ';']).map(str::trim).filter(|a| !a.is_empty()).map(|a| (a.to_string(), Kind::Text)).collect::<Vec<_>>())
            .collect(),
        t if PATH_TOOLS.contains(&t) => arg("path").map(path).into_iter().collect(),
        _ => Vec::new(),
    }
}

/// A path as the file tools would reach it, with `/` between folders. One
/// outside the shared folders (which the tool refuses anyway) is still
/// read with its `..` taken out, so a Deny holds for it too.
fn resolved(input: &str, sandbox: &Sandbox) -> String {
    let p = sandbox.resolve_read(input).or_else(|_| sandbox.resolve(input)).unwrap_or_else(|_| {
        let mut out = PathBuf::new();
        for c in Path::new(input).components() {
            match c {
                Component::ParentDir => {
                    out.pop();
                }
                Component::CurDir => {}
                other => out.push(other.as_os_str()),
            }
        }
        out
    });
    p.to_string_lossy().replace('\\', "/")
}

/// A pattern as a regular expression for its kind of subject.
fn glob(pattern: &str, kind: Kind) -> Option<Regex> {
    static CACHE: OnceLock<Mutex<HashMap<(String, Kind), Option<Regex>>>> = OnceLock::new();
    let key = (pattern.to_string(), kind);
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    cache
        .entry(key)
        .or_insert_with(|| {
            let body = if kind == Kind::Path { path_regex(pattern) } else { text_regex(pattern) };
            // Folders on Windows ignore capitals; elsewhere `Project` and `project` are two.
            let fold = kind != Kind::Path || cfg!(windows);
            Regex::new(&format!("{}^{body}$", if fold { "(?i)" } else { "" })).ok()
        })
        .clone()
}

fn path_regex(pattern: &str) -> String {
    let mut p = pattern.trim().replace('\\', "/");
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = crate::sandbox::home_folder() {
            p = format!("{}/{rest}", home.to_string_lossy().replace('\\', "/"));
        }
    }
    let b = p.as_bytes();
    let rooted = p.starts_with('/') || p.starts_with("**") || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':');
    // A name or a relative path (`.env`, `src/*.rs`) can sit in any folder.
    if !rooted {
        p = format!("**/{p}");
    }
    if p.ends_with('/') {
        p.push_str("**");
    }
    let chars: Vec<char> = p.chars().collect();
    let mut re = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '*' && chars.get(i + 1) == Some(&'*') {
            if chars.get(i + 2) == Some(&'/') {
                re.push_str("(?:.*/)?");
                i += 3;
            } else {
                re.push_str(".*");
                i += 2;
            }
            continue;
        }
        match chars[i] {
            '*' => re.push_str("[^/]*"),
            c => re.push_str(&regex::escape(&c.to_string())),
        }
        i += 1;
    }
    re
}

/// Commands, addresses and text: `*` is any run of characters, and any
/// run of spaces matches any other.
fn text_regex(pattern: &str) -> String {
    pattern
        .split_whitespace()
        .map(|word| word.split('*').map(regex::escape).collect::<Vec<_>>().join(".*"))
        .collect::<Vec<_>>()
        .join(r"\s+")
}

fn names(rule: &str, tool: &str) -> bool {
    match rule.strip_suffix('*') {
        Some(prefix) => tool.to_ascii_lowercase().starts_with(&prefix.to_ascii_lowercase()),
        None => rule.eq_ignore_ascii_case(tool),
    }
}

fn fits(rule: &Allowance, (text, kind): &(String, Kind)) -> bool {
    match &rule.pattern {
        None => true,
        Some(p) => glob(p, *kind).is_some_and(|re| re.is_match(text)),
    }
}

/// What the saved rules say about a call, or None to leave it to the run
/// mode. A command is allowed only when every piece of it is, and denied
/// or asked about when any piece is.
pub fn decide(rules: &[Allowance], project: Option<&str>, tool: &str, args: &Value, sandbox: &Sandbox) -> Option<Effect> {
    let here: Vec<&Allowance> = rules.iter().filter(|r| r.project.as_deref().is_none_or(|p| project == Some(p)) && names(&r.tool, tool)).collect();
    if here.is_empty() {
        return None;
    }
    let subjects = subjects(tool, args, sandbox);
    // A rule written for a whole chain (`cd x && git push`) is matched against it too.
    let whole = (tool == "run_command").then(|| args.get("command").and_then(Value::as_str)).flatten().map(|c| (c.trim().to_string(), Kind::Command));
    let hit = |effect: Effect| here.iter().filter(|r| r.effect == effect).any(|r| r.pattern.is_none() || subjects.iter().chain(&whole).any(|s| fits(r, s)));
    if hit(Effect::Deny) {
        return Some(Effect::Deny);
    }
    if hit(Effect::Ask) {
        return Some(Effect::Ask);
    }
    let allows: Vec<&Allowance> = here.iter().copied().filter(|r| r.effect == Effect::Allow).collect();
    if allows.iter().any(|r| r.pattern.is_none()) {
        return Some(Effect::Allow);
    }
    let covered = |s: &(String, Kind)| allows.iter().any(|r| fits(r, s));
    // `ri x` also comes as `remove-item x`; a rule for the way it was written covers both.
    let as_written = |s: &(String, Kind)| s.1 == Kind::Command && subjects.iter().any(|q| q.0 != s.0 && guard::normalise(&q.0) == s.0 && covered(q));
    let all = !subjects.is_empty() && subjects.iter().all(|s| covered(s) || as_written(s));
    all.then_some(Effect::Allow)
}

/// Whether a call counts as allowed for the chat. An Allow rule counts like
/// "Allow for this chat", and neither covers a delete or a command the
/// guard flags: those ask every time.
pub fn allowed(for_chat: bool, rule: Option<Effect>, destructive: bool) -> bool {
    (for_chat || rule == Some(Effect::Allow)) && !destructive
}

/// What "Always allow…" offers for a call, for the person to change before
/// saving: the exact command, the folder a file is in (and all under it),
/// or the site an address is on. Never a bare `*`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Suggestion {
    pub tool: String,
    pub pattern: Option<String>,
    /// The chat's project, for "only in this project".
    pub project: Option<String>,
}

pub fn suggest(tool: &str, args: &Value, sandbox: &Sandbox, project: Option<&str>) -> Suggestion {
    let sep = if cfg!(windows) { "\\" } else { "/" };
    let folder = |p: &str| {
        let p = PathBuf::from(p);
        let dir = if p.is_dir() { p } else { p.parent().map(Path::to_path_buf).unwrap_or(p) };
        dir.to_string_lossy().replace('/', sep).trim_end_matches(sep).to_string()
    };
    let pattern = match kind_of(tool) {
        Some(Kind::Command) => args.get("command").and_then(Value::as_str).map(|c| c.trim().to_string()),
        Some(Kind::Path) => {
            let dirs: Vec<String> = subjects(tool, args, sandbox).iter().filter(|s| s.1 == Kind::Path).map(|s| folder(&s.0)).collect();
            common(&dirs, sep).map(|d| format!("{d}{sep}**"))
        }
        Some(Kind::Url) => subjects(tool, args, sandbox).first().and_then(|(u, _)| reqwest::Url::parse(u).ok()).and_then(|u| Some(format!("{}://{}/*", u.scheme(), u.host_str()?))),
        Some(Kind::Text) => args.get("to").and_then(Value::as_str).map(|t| t.trim().to_string()),
        None => None,
    };
    Suggestion { tool: tool.to_string(), pattern: pattern.filter(|p| !p.trim_matches(['*', '/', '\\', ' ']).is_empty()), project: project.map(str::to_string) }
}

/// The folder both paths are in (a move's from and to).
fn common(dirs: &[String], sep: &str) -> Option<String> {
    let first = dirs.first()?;
    let mut parts: Vec<&str> = first.split(sep).collect();
    for d in &dirs[1..] {
        let other: Vec<&str> = d.split(sep).collect();
        let same = parts.iter().zip(&other).take_while(|(a, b)| if cfg!(windows) { a.eq_ignore_ascii_case(b) } else { a == b }).count();
        parts.truncate(same);
    }
    let joined = parts.join(sep);
    (!joined.is_empty()).then_some(joined)
}

pub fn list(conn: &Connection, c: &Cipher) -> Vec<Allowance> {
    crate::connectors::load(conn, c, TABLE)
}

pub fn save(conn: &Connection, c: &Cipher, mut a: Allowance) -> Result<Allowance, String> {
    a.tool = a.tool.trim().to_string();
    if a.tool.is_empty() || !a.tool.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '*')) {
        return Err("Name a tool (such as run_command), or tools by the start of their names (such as browser_*).".into());
    }
    a.pattern = a.pattern.map(|p| p.trim().to_string()).filter(|p| !p.is_empty());
    if a.pattern.is_some() && !a.tool.ends_with('*') && kind_of(&a.tool).is_none() {
        return Err(format!("{} doesn't act on a command, a path or an address, so a rule for it can't have a pattern. Leave the pattern empty.", a.tool));
    }
    if a.id.is_empty() {
        a.id = uuid::Uuid::new_v4().to_string();
    }
    if a.created_at == 0 {
        a.created_at = crate::db::now_ms();
    }
    crate::connectors::store(conn, c, TABLE, &a.id, &a)?;
    Ok(a)
}

pub fn remove(conn: &Connection, id: &str) -> Result<(), String> {
    conn.execute(&format!("DELETE FROM {TABLE} WHERE id = ?1"), params![id]).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command(async)]
pub fn list_allowances(state: AppStateRef) -> Result<Vec<Allowance>, String> {
    let c = state.cipher()?;
    Ok(list(&state.db.lock().unwrap(), &c))
}

#[tauri::command(async)]
pub fn save_allowance(state: AppStateRef, allowance: Allowance) -> Result<Allowance, String> {
    let c = state.cipher()?;
    save(&state.db.lock().unwrap(), &c, allowance)
}

#[tauri::command(async)]
pub fn remove_allowance(state: AppStateRef, id: String) -> Result<(), String> {
    state.cipher()?;
    remove(&state.db.lock().unwrap(), &id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{permission, Mode, Permission, Risk};
    use serde_json::json;

    fn rule(tool: &str, pattern: Option<&str>, effect: Effect) -> Allowance {
        Allowance { id: String::new(), tool: tool.into(), pattern: pattern.map(Into::into), effect, project: None, created_at: 0 }
    }

    fn work() -> (Sandbox, PathBuf) {
        let d = std::env::temp_dir().join(format!("sulcusai-allow-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(d.join("work").join("sub")).unwrap();
        let root = dunce::canonicalize(d.join("work")).unwrap();
        (Sandbox::new(&[root.display().to_string()]), root)
    }

    #[test]
    fn one_star_stays_in_its_folder_and_two_cross_folders() {
        let one = glob("C:/work/*.rs", Kind::Path).unwrap();
        assert!(one.is_match("C:/work/main.rs"));
        assert!(!one.is_match("C:/work/src/main.rs"));
        let two = glob("C:/work/**", Kind::Path).unwrap();
        assert!(two.is_match("C:/work/src/deep/main.rs"));
        let any = glob("**/.env", Kind::Path).unwrap();
        assert!(any.is_match("C:/a/b/.env") && any.is_match(".env"));
        assert!(!any.is_match("C:/a/b/x.env"));
    }

    #[test]
    fn patterns_take_either_separator_and_mean_their_dots_and_brackets() {
        let p = glob(r"C:\work\(old)\v1.0\*", Kind::Path).unwrap();
        assert!(p.is_match("C:/work/(old)/v1.0/notes.txt"));
        assert!(!p.is_match("C:/work/(old)/v1x0/notes.txt"));
        assert!(glob("*.env", Kind::Path).unwrap().is_match("D:/x/prod.env"));
        assert!(glob("git status*", Kind::Command).unwrap().is_match("GIT   status --short"));
    }

    #[test]
    #[cfg(windows)]
    fn windows_paths_match_without_regard_to_capitals() {
        assert!(glob(r"c:\Work\**", Kind::Path).unwrap().is_match("C:/WORK/x.txt"));
    }

    #[test]
    fn deny_beats_allow_and_ask_beats_allow() {
        let (sb, _) = work();
        let args = json!({ "command": "git status" });
        let both = [rule("run_command", Some("git *"), Effect::Allow), rule("run_command", Some("git status"), Effect::Deny)];
        assert_eq!(decide(&both, None, "run_command", &args, &sb), Some(Effect::Deny));
        let ask = [rule("run_command", None, Effect::Allow), rule("run_command", Some("git status*"), Effect::Ask)];
        assert_eq!(decide(&ask, None, "run_command", &args, &sb), Some(Effect::Ask));
        assert_eq!(decide(&ask, None, "run_command", &json!({ "command": "cargo test" }), &sb), Some(Effect::Allow));
        assert_eq!(decide(&[], None, "run_command", &args, &sb), None);
    }

    #[test]
    fn a_project_rule_holds_only_in_that_project() {
        let (sb, _) = work();
        let mut r = rule("run_command", Some("cargo *"), Effect::Allow);
        r.project = Some("p1".into());
        let args = json!({ "command": "cargo build" });
        assert_eq!(decide(std::slice::from_ref(&r), Some("p1"), "run_command", &args, &sb), Some(Effect::Allow));
        assert_eq!(decide(std::slice::from_ref(&r), Some("p2"), "run_command", &args, &sb), None);
        assert_eq!(decide(&[r], None, "run_command", &args, &sb), None);
    }

    #[test]
    fn a_tool_prefix_covers_every_tool_it_starts() {
        let (sb, _) = work();
        let r = [rule("browser_*", None, Effect::Ask)];
        assert_eq!(decide(&r, None, "browser_click", &json!({}), &sb), Some(Effect::Ask));
        assert_eq!(decide(&r, None, "read_file", &json!({}), &sb), None);
    }

    #[test]
    fn paths_are_matched_where_they_really_go() {
        let (sb, root) = work();
        let allow = [rule("read_file", Some(&format!("{}{}**", root.display(), std::path::MAIN_SEPARATOR)), Effect::Allow)];
        let inside = root.join("sub").join("a.txt");
        assert_eq!(decide(&allow, None, "read_file", &json!({ "path": inside.display().to_string() }), &sb), Some(Effect::Allow));
        let out = format!("{}{s}..{s}x.txt", root.display(), s = std::path::MAIN_SEPARATOR);
        assert_eq!(decide(&allow, None, "read_file", &json!({ "path": out }), &sb), None);
        let deny = [rule("write_file", Some(r"**\.env"), Effect::Deny)];
        assert_eq!(decide(&deny, None, "write_file", &json!({ "path": "sub/../.env" }), &sb), Some(Effect::Deny));
        assert_eq!(decide(&deny, None, "write_file", &json!({ "path": "sub/notes.txt" }), &sb), None);
        // A move needs both ends allowed, and either end denied stops it.
        let mv = json!({ "from": inside.display().to_string(), "to": out });
        let both = [rule("move_path", Some(&format!("{}/**", root.display())), Effect::Allow)];
        assert_eq!(decide(&both, None, "move_path", &mv, &sb), None);
    }

    #[test]
    fn a_command_is_allowed_only_when_every_piece_is() {
        let (sb, _) = work();
        let r = [rule("run_command", Some("git status*"), Effect::Allow)];
        let cmd = |c: &str| json!({ "command": c });
        assert_eq!(decide(&r, None, "run_command", &cmd("git status"), &sb), Some(Effect::Allow));
        assert_eq!(decide(&r, None, "run_command", &cmd("git status; Remove-Item x"), &sb), None);
        assert_eq!(decide(&r, None, "run_command", &cmd("cmd /c \"git status\""), &sb), None);
        // Any piece denied denies the whole, however it's dressed.
        let d = [rule("run_command", Some("remove-item *"), Effect::Deny)];
        assert_eq!(decide(&d, None, "run_command", &cmd("echo hi; ri build"), &sb), Some(Effect::Deny));
        assert_eq!(decide(&d, None, "run_command", &cmd("cmd /c \"del build\""), &sb), Some(Effect::Deny));
        // A rule for the short name covers its long form too.
        let rm = [rule("run_command", Some("rm build/*"), Effect::Allow)];
        assert_eq!(decide(&rm, None, "run_command", &cmd("rm build/x"), &sb), Some(Effect::Allow));
    }

    /// An Allow counts like "Allow for this chat": it never gets past a
    /// send, a delete or Plan mode.
    #[test]
    fn an_allow_never_skips_what_always_asks() {
        let allow = Some(Effect::Allow);
        // Submitting, buying or sending, and a command that sends off the PC (made Submit).
        assert_eq!(permission(Mode::Auto, Risk::Submit, allowed(false, allow, false)), Permission::Ask);
        assert_eq!(permission(Mode::Bypass, Risk::Submit, allowed(false, allow, false)), Permission::Ask);
        // Deleting, or a command the guard flags.
        assert!(!allowed(true, allow, true));
        assert_eq!(permission(Mode::Auto, Risk::Write, allowed(false, allow, true)), Permission::Ask);
        assert_eq!(permission(Mode::Plan, Risk::Write, allowed(false, allow, false)), Permission::Refuse);
        // What the guard refuses is refused by the command itself, with or without a rule.
        let (sb, root) = work();
        assert!(guard::refuse("format C:", &root, &sb).is_some());
        assert_eq!(permission(Mode::Auto, Risk::Programs, allowed(false, allow, false)), Permission::Run);
    }

    #[test]
    fn suggestions_name_the_command_or_the_folder() {
        let (sb, root) = work();
        let s = suggest("run_command", &json!({ "command": " git status " }), &sb, Some("p"));
        assert_eq!(s.pattern.as_deref(), Some("git status"));
        assert_eq!(s.project.as_deref(), Some("p"));
        let file = root.join("sub").join("a.txt");
        let s = suggest("write_file", &json!({ "path": file.display().to_string() }), &sb, None);
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(s.pattern, Some(format!("{}{sep}sub{sep}**", root.display())));
        let s = suggest("fetch_page", &json!({ "url": "https://example.com/a/b?c" }), &sb, None);
        assert_eq!(s.pattern.as_deref(), Some("https://example.com/*"));
        assert_eq!(suggest("calculate", &json!({}), &sb, None).pattern, None);
    }

    #[test]
    fn rules_are_saved_encrypted_and_removed() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init_for_test(&conn);
        let c = Cipher::for_test();
        let saved = save(&conn, &c, rule(" run_command ", Some(" git status* "), Effect::Allow)).unwrap();
        assert_eq!((saved.tool.as_str(), saved.pattern.as_deref()), ("run_command", Some("git status*")));
        assert!(!saved.id.is_empty() && saved.created_at > 0);
        let raw: String = conn.query_row("SELECT data FROM standing_permissions", [], |r| r.get(0)).unwrap();
        assert!(!raw.contains("git status"));
        assert_eq!(list(&conn, &c), vec![saved.clone()]);
        assert!(save(&conn, &c, rule("calculate", Some("x"), Effect::Deny)).is_err());
        assert!(save(&conn, &c, rule("rm -rf", None, Effect::Deny)).is_err());
        remove(&conn, &saved.id).unwrap();
        assert!(list(&conn, &c).is_empty());
    }
}
