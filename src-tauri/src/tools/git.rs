// SPDX-License-Identifier: AGPL-3.0-only
//! Git as tools rather than raw commands: short structured answers, a
//! commit that takes only the files this chat changed (the tree often holds
//! someone else's work), and worktrees for child sessions with a merge that
//! stops on a conflict and shows both sides instead of picking one.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use serde_json::{json, Value};

use super::{arg_str, Ctx, Outcome, Preview};
use crate::sandbox::Sandbox;

const MAX_DIFF_LINES: usize = 400;
const MAX_STATUS: usize = 200;

fn path_param() -> Value {
    json!({ "type": "string", "description": "A folder inside the repository (a shared folder)." })
}

pub fn git_status_params() -> Value {
    json!({ "type": "object", "required": ["path"], "properties": { "path": path_param() } })
}
pub fn git_diff_params() -> Value {
    json!({ "type": "object", "required": ["path"], "properties": { "path": path_param(),
        "file": { "type": "string", "description": "Only this file." },
        "staged": { "type": "boolean" },
        "start_line": { "type": "integer", "description": "Page on from this line of the diff." } } })
}
pub fn git_commit_params() -> Value {
    json!({ "type": "object", "required": ["path", "message", "files"], "properties": { "path": path_param(),
        "message": { "type": "string" },
        "files": { "type": "array", "items": { "type": "string" }, "description": "Exactly the files to commit. Only files this chat changed can be named." } } })
}
pub fn git_branch_params() -> Value {
    json!({ "type": "object", "required": ["path", "action"], "properties": { "path": path_param(),
        "action": { "type": "string", "enum": ["list", "create", "switch"] },
        "name": { "type": "string" } } })
}
pub fn git_worktree_params() -> Value {
    json!({ "type": "object", "required": ["path", "action"], "properties": { "path": path_param(),
        "action": { "type": "string", "enum": ["list", "add", "remove"] },
        "name": { "type": "string", "description": "The worktree's name (add, remove). Its branch is work/<name>." } } })
}
pub fn git_merge_params() -> Value {
    json!({ "type": "object", "required": ["path", "branch"], "properties": { "path": path_param(),
        "branch": { "type": "string", "description": "The branch to merge into the current one (a child's work/<name>)." } } })
}

pub fn git(dir: &Path, args: &[&str]) -> Result<(bool, String, String), String> {
    let mut c = Command::new("git");
    c.args(args).current_dir(dir).env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    let out = c.output().map_err(|e| format!("Couldn't run git: {e}. Is Git installed?"))?;
    Ok((out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned()))
}

fn ok_git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let (ok, out, err) = git(dir, args)?;
    if ok { Ok(out) } else { Err(format!("git {} failed: {}", args.first().unwrap_or(&""), err.trim())) }
}

/// The repository's top folder for a path inside it.
pub fn repo_root(dir: &Path) -> Result<PathBuf, String> {
    let out = ok_git(dir, &["rev-parse", "--show-toplevel"]).map_err(|_| format!("{} isn't inside a git repository.", dir.display()))?;
    Ok(PathBuf::from(out.trim()))
}

/// Worktrees made by `git_worktree add`, and the build folder each shares.
static TARGETS: Mutex<Vec<(PathBuf, PathBuf)>> = Mutex::new(Vec::new());

/// The shared build folder for a command run inside a worktree made here.
pub fn shared_target(dir: &Path) -> Option<PathBuf> {
    TARGETS.lock().unwrap().iter().find(|(w, _)| dir.starts_with(w)).map(|(_, t)| t.clone())
}

pub fn worktrees_dir(state: &crate::AppState) -> PathBuf {
    state.paths.data.join("worktrees")
}

/// Makes a worktree on a new branch `work/<name>`, outside the repository
/// (never inside it, where its files would show as changes).
pub fn add_worktree(repo: &Path, base: &Path, name: &str) -> Result<PathBuf, String> {
    let name: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' }).collect();
    if name.trim_matches('-').is_empty() {
        return Err("Give the worktree a name.".into());
    }
    let root = repo_root(repo)?;
    let dir = base.join(&name);
    if dir.exists() {
        return Err(format!("A worktree called {name} already exists."));
    }
    std::fs::create_dir_all(base).map_err(|e| e.to_string())?;
    let branch = format!("work/{name}");
    ok_git(&root, &["worktree", "add", "-b", &branch, &dir.display().to_string()])?;
    let target = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| root.join("target").join("src-tauri"));
    TARGETS.lock().unwrap().push((dir.clone(), target));
    Ok(dir)
}

pub fn remove_worktree(repo: &Path, dir: &Path) -> Result<(), String> {
    let root = repo_root(repo)?;
    ok_git(&root, &["worktree", "remove", "--force", &dir.display().to_string()])?;
    TARGETS.lock().unwrap().retain(|(w, _)| w != dir);
    Ok(())
}

/// What a merge found: done, or stopped on files both sides changed.
#[derive(Debug, PartialEq)]
pub enum Merged {
    Clean(String),
    Conflict(Vec<(String, String, String)>),
}

/// Merges `branch` into the checked-out branch. On a conflict it collects
/// both sides of each file, then aborts, so the tree is as it was.
pub fn merge(dir: &Path, branch: &str) -> Result<Merged, String> {
    let root = repo_root(dir)?;
    let (ok, out, err) = git(&root, &["merge", "--no-ff", "--no-edit", branch])?;
    if ok {
        return Ok(Merged::Clean(out.trim().to_string()));
    }
    let conflicted = ok_git(&root, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
    let files: Vec<String> = conflicted.lines().map(str::to_string).filter(|l| !l.is_empty()).collect();
    if files.is_empty() {
        let _ = git(&root, &["merge", "--abort"]);
        return Err(format!("The merge failed: {}", if err.trim().is_empty() { out.trim() } else { err.trim() }));
    }
    let side = |n: u8, f: &str| git(&root, &["show", &format!(":{n}:{f}")]).map(|(_, o, _)| o).unwrap_or_default();
    let both = files.iter().map(|f| (f.clone(), side(2, f), side(3, f))).collect();
    let _ = git(&root, &["merge", "--abort"]);
    Ok(Merged::Conflict(both))
}

/// Files this chat wrote, created or deleted (not undone), by full path.
fn chat_files(ctx: &Ctx<'_>) -> Vec<PathBuf> {
    let conn = ctx.memory.db.lock().unwrap();
    crate::db::chat_changes(&conn, ctx.memory.cipher, ctx.memory.chat_id)
        .into_iter()
        .filter(|c| !c.undone)
        .flat_map(|c| std::iter::once(PathBuf::from(c.path)).chain(c.other.map(PathBuf::from)))
        .collect()
}

fn same(a: &Path, b: &Path) -> bool {
    let n = |p: &Path| p.to_string_lossy().replace('\\', "/").to_lowercase();
    n(a) == n(b)
}

fn dir_of(args: &Value, sb: &Sandbox) -> Result<PathBuf, String> {
    let p = sb.resolve_read(arg_str(args, "path")?)?;
    Ok(if p.is_file() { p.parent().map(Path::to_path_buf).unwrap_or(p) } else { p })
}

/// Cuts long text to a page, saying where it stopped.
pub fn page(text: &str, start: usize, max: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = start.max(1);
    let end = (start - 1 + max).min(lines.len());
    let mut out = lines.get(start - 1..end).map(|l| l.join("\n")).unwrap_or_default();
    if end < lines.len() {
        out.push_str(&format!("\n… cut off at line {end} of {}. Ask again with start_line={} for the rest.", lines.len(), end + 1));
    }
    out
}

pub fn preview(name: &str, args: &Value, sb: &Sandbox) -> Result<Preview, String> {
    let dir = dir_of(args, sb)?;
    let label = sb.display(&dir);
    Ok(match name {
        "git_commit" => {
            let files: Vec<String> = args.get("files").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
            Preview { title: format!("Commit {} file{} in {label}", files.len(), if files.len() == 1 { "" } else { "s" }), kind: "text", detail: Some(format!("{}\n\n{}", arg_str(args, "message")?, files.join("\n"))), note: Some("Only these files are committed; nothing else in the folder is staged.".into()) }
        }
        "git_merge" => Preview { title: format!("Merge {} in {label}", arg_str(args, "branch")?), kind: "text", detail: None, note: Some("Stops and shows both sides if the same lines changed on both branches.".into()) },
        "git_worktree" if args.get("action").and_then(Value::as_str) == Some("remove") => Preview { title: format!("Remove the worktree {}", args.get("name").and_then(Value::as_str).unwrap_or("")), kind: "text", detail: None, note: Some("Its folder is deleted. Commits on its branch stay.".into()) },
        _ => Preview { title: format!("{} in {label}", name.replace('_', " ")), kind: "text", detail: args.get("name").and_then(Value::as_str).map(str::to_string), note: None },
    })
}

pub fn run(name: &str, args: &Value, ctx: &Ctx<'_>) -> Outcome {
    let r = (|| -> Result<Outcome, String> {
        let dir = dir_of(args, ctx.sandbox)?;
        let root = repo_root(&dir)?;
        match name {
            "git_status" => {
                let out = ok_git(&root, &["status", "--porcelain=v1", "-b", "-uall"])?;
                let mut lines = out.lines();
                let branch = lines.next().unwrap_or("").trim_start_matches("## ").to_string();
                let (mut staged, mut changed, mut untracked) = (Vec::new(), Vec::new(), Vec::new());
                for l in lines {
                    let (x, y, f) = (l.chars().next().unwrap_or(' '), l.chars().nth(1).unwrap_or(' '), l.get(3..).unwrap_or(""));
                    if x == '?' {
                        untracked.push(f.to_string());
                        continue;
                    }
                    if x != ' ' {
                        staged.push(format!("{x} {f}"));
                    }
                    if y != ' ' {
                        changed.push(format!("{y} {f}"));
                    }
                }
                let list = |title: &str, v: &[String]| {
                    if v.is_empty() {
                        return String::new();
                    }
                    let more = if v.len() > MAX_STATUS { format!("\n  … and {} more", v.len() - MAX_STATUS) } else { String::new() };
                    format!("\n{title} ({}):\n  {}{more}", v.len(), v.iter().take(MAX_STATUS).cloned().collect::<Vec<_>>().join("\n  "))
                };
                let text = format!("Branch {branch}{}{}{}", list("Staged", &staged), list("Changed, not staged", &changed), list("Untracked", &untracked));
                let mut o = Outcome::ok(text, format!("Git status in {}", ctx.sandbox.display(&dir)), "text", None);
                o.meta["git"] = json!({ "branch": branch, "staged": staged.len(), "changed": changed.len(), "untracked": untracked.len() });
                Ok(o)
            }
            "git_diff" => {
                let mut a = vec!["diff", "--no-color"];
                if args.get("staged").and_then(Value::as_bool).unwrap_or(false) {
                    a.push("--cached");
                }
                let file = args.get("file").and_then(Value::as_str).map(|f| ctx.sandbox.resolve_read(f)).transpose()?;
                let fs = file.as_ref().map(|f| f.display().to_string());
                if let Some(f) = &fs {
                    a.extend(["--", f.as_str()]);
                }
                let out = ok_git(&root, &a)?;
                let start = args.get("start_line").and_then(Value::as_u64).unwrap_or(1) as usize;
                let text = if out.trim().is_empty() { "No differences.".to_string() } else { page(&out, start, MAX_DIFF_LINES) };
                Ok(Outcome::ok(text.clone(), "Git diff", "diff", Some(text)))
            }
            "git_commit" => {
                let message = arg_str(args, "message")?.trim().to_string();
                if message.is_empty() {
                    return Err("Write a commit message.".into());
                }
                let mine = chat_files(ctx);
                let mut files = Vec::new();
                for f in args.get("files").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
                    let p = ctx.sandbox.resolve(f)?;
                    if !mine.iter().any(|m| same(m, &p)) {
                        return Err(format!("{f} wasn't changed in this chat, so it isn't committed from here (it may be someone else's work). Name only files this chat changed."));
                    }
                    files.push(p);
                }
                if files.is_empty() {
                    return Err("Name the files to commit.".into());
                }
                let mut problems = super::checks::rule_problems("commit message", &message);
                for f in &files {
                    if let Ok(text) = std::fs::read_to_string(f) {
                        problems.extend(super::checks::added_rule_problems(&root, f, &text));
                    }
                }
                if !problems.is_empty() {
                    return Err(format!("Not committed, the repository's rules found:\n- {}", problems.join("\n- ")));
                }
                let names: Vec<String> = files.iter().map(|p| p.display().to_string()).collect();
                let mut add = vec!["add", "-A", "--"];
                add.extend(names.iter().map(String::as_str));
                ok_git(&root, &add)?;
                let mut commit = vec!["commit", "-m", message.as_str(), "--only", "--"];
                commit.extend(names.iter().map(String::as_str));
                let out = ok_git(&root, &commit)?;
                let head = ok_git(&root, &["rev-parse", "--short", "HEAD"]).unwrap_or_default();
                Ok(Outcome::ok(format!("Committed {} ({} files): {}", head.trim(), files.len(), out.lines().next().unwrap_or("")), format!("Committed {} files", files.len()), "text", None))
            }
            "git_branch" => {
                let action = arg_str(args, "action")?;
                let name = args.get("name").and_then(Value::as_str).unwrap_or("").trim();
                let out = match action {
                    "list" => ok_git(&root, &["branch", "--list", "--format=%(HEAD) %(refname:short)"])?,
                    "create" if !name.is_empty() => ok_git(&root, &["branch", name]).map(|_| format!("Made branch {name}."))?,
                    "switch" if !name.is_empty() => ok_git(&root, &["switch", name]).map(|_| format!("Switched to {name}."))?,
                    _ => return Err("Give a name for create or switch.".into()),
                };
                Ok(Outcome::ok(out.clone(), format!("Git branch {action}"), "text", None))
            }
            "git_worktree" => {
                let action = arg_str(args, "action")?;
                let name = args.get("name").and_then(Value::as_str).unwrap_or("").trim();
                let base = worktrees_dir(ctx.state);
                match action {
                    "list" => Ok(Outcome::ok(ok_git(&root, &["worktree", "list"])?, "Git worktrees", "text", None)),
                    "add" => {
                        let d = add_worktree(&root, &base, name)?;
                        Ok(Outcome::ok(format!("Made worktree {} on branch work/{name}. Builds there share the repository's build folder.", d.display()), format!("Made worktree {name}"), "text", None))
                    }
                    "remove" => {
                        remove_worktree(&root, &base.join(name))?;
                        Ok(Outcome::ok(format!("Removed worktree {name}."), format!("Removed worktree {name}"), "text", None))
                    }
                    _ => Err("action is list, add or remove.".into()),
                }
            }
            "git_merge" => {
                let branch = arg_str(args, "branch")?;
                match merge(&root, branch)? {
                    Merged::Clean(out) => Ok(Outcome::ok(format!("Merged {branch}. {out}"), format!("Merged {branch}"), "text", None)),
                    Merged::Conflict(files) => {
                        let shown: Vec<String> = files.iter().map(|(f, ours, theirs)| format!("{f}\n--- this branch ---\n{}\n--- {branch} ---\n{}", page(ours, 1, 80), page(theirs, 1, 80))).collect();
                        Err(format!("Stopped: {} file{} changed on both sides, so nothing was merged (the tree is as it was). Decide with the person which to keep:\n\n{}", files.len(), if files.len() == 1 { "" } else { "s" }, shown.join("\n\n")))
                    }
                }
            }
            other => Err(format!("Unknown tool {other}.")),
        }
    })();
    r.unwrap_or_else(|e| Outcome::error(super::failed_title(name, args), e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> PathBuf {
        let d = std::env::temp_dir().join(format!("sulcus-git-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&d).unwrap();
        for a in [&["init", "-q", "-b", "main"][..], &["config", "user.email", "t@example.com"], &["config", "user.name", "T"], &["config", "core.autocrlf", "false"]] {
            ok_git(&d, a).unwrap();
        }
        std::fs::write(d.join("a.txt"), "one\n").unwrap();
        ok_git(&d, &["add", "."]).unwrap();
        ok_git(&d, &["commit", "-qm", "start"]).unwrap();
        d
    }

    #[test]
    fn a_conflict_at_merge_shows_both_sides_and_leaves_the_tree() {
        if git(Path::new("."), &["--version"]).is_err() {
            return;
        }
        let d = repo();
        let base = d.with_extension("wt");
        let wt = add_worktree(&d, &base, "child").unwrap();
        assert_eq!(shared_target(&wt.join("src")).is_some(), true);
        std::fs::write(wt.join("a.txt"), "child\n").unwrap();
        ok_git(&wt, &["commit", "-qam", "child"]).unwrap();
        std::fs::write(d.join("a.txt"), "parent\n").unwrap();
        ok_git(&d, &["commit", "-qam", "parent"]).unwrap();
        match merge(&d, "work/child").unwrap() {
            Merged::Conflict(f) => {
                assert_eq!(f[0].0, "a.txt");
                assert_eq!((f[0].1.as_str(), f[0].2.as_str()), ("parent\n", "child\n"));
            }
            m => panic!("{m:?}"),
        }
        assert_eq!(std::fs::read_to_string(d.join("a.txt")).unwrap(), "parent\n");
        remove_worktree(&d, &wt).unwrap();
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn long_output_says_where_it_was_cut() {
        let text: String = (1..=20_000).map(|i| format!("line {i}\n")).collect();
        let p = page(&text, 1, 400);
        assert!(p.ends_with("Ask again with start_line=401 for the rest."), "{}", &p[p.len() - 80..]);
        assert!(p.contains("cut off at line 400 of 20000"));
    }
}
