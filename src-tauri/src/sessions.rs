// SPDX-License-Identifier: AGPL-3.0-only
//! Sessions that start and manage sessions of their own (off by default:
//! `Feature::Sessions`). A child is a real chat with a written brief. It
//! works only in its own folder or git worktree, and only its parent merges
//! its work. Limits keep a tree from running away: depth, sessions at once,
//! a token budget per tree, a repeat check, and a reason for every spawn.
//! A local child starts only when the graphics card has room; a cloud child
//! doesn't need the card.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::tools::{arg_str, Ctx, Outcome};

pub const MAX_DEPTH: u8 = 2;
pub const TREE_TOKENS: u64 = 2_000_000;
pub const EFFORTS: &[&str] = &["low", "medium", "high"];

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Brief {
    pub goal: String,
    #[serde(default)]
    pub scope: Vec<String>,
    #[serde(default)]
    pub done: Vec<String>,
    #[serde(default)]
    pub files: Vec<String>,
}

impl Brief {
    pub fn text(&self, folder: &str) -> String {
        let list = |t: &str, v: &[String]| if v.is_empty() { String::new() } else { format!("\n{t}:\n- {}", v.join("\n- ")) };
        format!(
            "You are a session started by another session to do one part of a job.\nGoal: {}\nWork only in {folder}; you can't change anything outside it.{}{}{}\nWhen done, reply with a short report: what changed (files), whether each done check passed, and anything the parent must decide. Report problems; no status chatter.",
            self.goal,
            list("Scope", &self.scope),
            list("Done when", &self.done),
            list("Files and notes you may use", &self.files)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Queued,
    Running,
    Paused,
    Done,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    /// The session's chat id.
    pub id: String,
    pub parent: String,
    pub root: String,
    pub depth: u8,
    pub title: String,
    pub brief: Brief,
    pub reason: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub state: State,
    pub folder: String,
    pub branch: Option<String>,
    pub tokens: u64,
    pub started_at: i64,
    pub report: String,
    pub log: Vec<String>,
}

impl Node {
    pub fn live(&self) -> bool {
        matches!(self.state, State::Queued | State::Running | State::Paused)
    }
}

static TREE: Mutex<Option<Vec<Node>>> = Mutex::new(None);

fn file(data: &Path) -> PathBuf {
    data.join("sessions").join("tree.json")
}

fn with_tree<T>(data: &Path, f: impl FnOnce(&mut Vec<Node>) -> T) -> T {
    let mut g = TREE.lock().unwrap();
    let tree = g.get_or_insert_with(|| std::fs::read_to_string(file(data)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default());
    let out = f(tree);
    let _ = std::fs::create_dir_all(data.join("sessions"));
    let _ = std::fs::write(file(data), serde_json::to_string_pretty(tree).unwrap_or_default());
    out
}

pub fn node(data: &Path, id: &str) -> Option<Node> {
    with_tree(data, |t| t.iter().find(|n| n.id == id).cloned())
}

/// A session's chat works only in its own folder (`agent.rs` uses this).
pub fn own_folder(data: &Path, chat_id: &str) -> Option<String> {
    node(data, chat_id).map(|n| n.folder)
}

/// Everything under the tree `chat_id` belongs to (its root's tree).
pub fn tree_of(data: &Path, chat_id: &str) -> Vec<Node> {
    with_tree(data, |t| {
        let root = t.iter().find(|n| n.id == chat_id).map_or(chat_id.to_string(), |n| n.root.clone());
        t.iter().filter(|n| n.root == root).cloned().collect()
    })
}

/// The most sessions at once: one per 8 GB of RAM, between 2 and 8.
pub fn max_live(ram_bytes: u64) -> usize {
    ((ram_bytes / (8 << 30)) as usize).clamp(2, 8)
}

/// Whether `parent` may start another child. Err says which limit holds.
pub fn can_spawn(tree: &[Node], parent: &str, goal: &str, reason: &str, live_cap: usize) -> Result<u8, String> {
    if reason.trim().len() < 10 {
        return Err("Say why this needs its own session (reason): splitting work costs many times the tokens.".into());
    }
    let depth = tree.iter().find(|n| n.id == parent).map_or(0, |n| n.depth) + 1;
    if depth > MAX_DEPTH {
        return Err(format!("Sessions go at most {MAX_DEPTH} levels deep; do this part here."));
    }
    let root = tree.iter().find(|n| n.id == parent).map_or(parent, |n| n.root.as_str());
    let mine: Vec<&Node> = tree.iter().filter(|n| n.root == root).collect();
    if mine.iter().filter(|n| n.live()).count() >= live_cap {
        return Err(format!("{live_cap} sessions are already working; wait for one to finish."));
    }
    let tokens: u64 = mine.iter().map(|n| n.tokens).sum();
    if tokens >= TREE_TOKENS {
        return Err(format!("This tree has used its budget of {TREE_TOKENS} tokens."));
    }
    let g = goal.trim().to_lowercase();
    if mine.iter().any(|n| n.brief.goal.trim().to_lowercase() == g) {
        return Err("A session with this same goal was already started; message it instead.".into());
    }
    Ok(depth)
}

/// Where a session's model can run.
#[derive(Debug, PartialEq)]
pub enum Placement {
    /// A cloud model: the card isn't involved.
    Cloud,
    /// Already on the card for another session: shared, not loaded twice.
    Share,
    Fits,
    /// No room: wait, or take one of these instead.
    NoRoom { smaller: Option<String> },
}

/// `need` is the model's memory on the card, `free` what's left after the
/// models already loaded, `smaller` the local models (with needs) to offer.
pub fn place(model: &str, need: Option<u64>, free: u64, loaded: &[String], smaller: &[(String, u64)]) -> Placement {
    if crate::cloud::is_cloud(model) {
        return Placement::Cloud;
    }
    if loaded.iter().any(|m| m == model) {
        return Placement::Share;
    }
    match need {
        Some(n) if n <= free => Placement::Fits,
        _ => Placement::NoRoom { smaller: smaller.iter().filter(|(_, n)| *n <= free).max_by_key(|(_, n)| *n).map(|(m, _)| m.clone()) },
    }
}

/// A model change is allowed only to a model the person has installed or
/// connected, and to a cloud one only when cloud chats are allowed.
pub fn check_model(model: &str, installed: &[String], cloud_ok: bool) -> Result<(), String> {
    if crate::cloud::is_cloud(model) {
        if !cloud_ok {
            return Err("Cloud models aren't allowed here (the cloud level is off or policy forbids it).".into());
        }
        return Ok(());
    }
    if installed.iter().any(|m| m == model) {
        Ok(())
    } else {
        Err(format!("{model} isn't installed; pick one of: {}", installed.join(", ")))
    }
}

fn model_files(state: &crate::AppState) -> Vec<(String, u64)> {
    let dir = state.paths.models.clone();
    crate::tools::fs::walk_files(&[dir], Some("**/*.gguf"), 2000)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| Some((p.file_stem()?.to_string_lossy().into_owned(), std::fs::metadata(&p).ok()?.len())))
        .collect()
}

/// The card's memory a local model needs: its file and a fifth for the context.
fn need_of(files: &[(String, u64)], model: &str) -> Option<u64> {
    files.iter().find(|(m, _)| model.contains(m.as_str()) || m.contains(model)).map(|(_, n)| n + n / 5)
}

fn cloud_ok(state: &crate::AppState) -> bool {
    let conn = state.db.lock().unwrap();
    !crate::cloud::settings(&conn).providers.is_empty() && !crate::outgoing::mode(&conn).on
}

fn log(data: &Path, id: &str, line: String) {
    with_tree(data, |t| {
        if let Some(n) = t.iter_mut().find(|n| n.id == id) {
            n.log.push(format!("{} {line}", chrono::Local::now().format("%H:%M")));
        }
    });
}

fn set_state(data: &Path, id: &str, s: State, report: Option<String>) {
    with_tree(data, |t| {
        if let Some(n) = t.iter_mut().find(|n| n.id == id) {
            n.state = s;
            if let Some(r) = report {
                n.tokens += (r.len() / 4) as u64;
                n.report = r;
            }
        }
    });
}

pub const TOOLS: &[&str] = &["start_session", "message_session", "set_session_model", "set_session_effort", "stop_session", "list_sessions", "stop_all_sessions"];

pub fn start_session_params() -> Value {
    json!({ "type": "object", "required": ["goal", "folder", "reason"], "properties": {
        "goal": { "type": "string" },
        "reason": { "type": "string", "description": "Why this part needs its own session." },
        "folder": { "type": "string", "description": "The folder it works in. In a git repository it gets its own worktree of it." },
        "scope": { "type": "array", "items": { "type": "string" } },
        "done": { "type": "array", "items": { "type": "string" }, "description": "Checks that tell it's done." },
        "files": { "type": "array", "items": { "type": "string" } },
        "model": { "type": "string", "description": "Default: this chat's model. Hard steps to a cloud model when allowed, routine ones to a small local one." },
        "effort": { "type": "string", "enum": EFFORTS } } })
}
pub fn session_params() -> Value {
    json!({ "type": "object", "required": ["session"], "properties": { "session": { "type": "string", "description": "The session's id from list_sessions." }, "pause": { "type": "boolean", "description": "stop_session: pause instead (message_session resumes it)." } } })
}
pub fn message_session_params() -> Value {
    json!({ "type": "object", "required": ["session", "message"], "properties": { "session": { "type": "string" }, "message": { "type": "string" } } })
}
pub fn set_model_params() -> Value {
    json!({ "type": "object", "required": ["session", "model", "reason"], "properties": { "session": { "type": "string" }, "model": { "type": "string" }, "reason": { "type": "string" } } })
}
pub fn set_effort_params() -> Value {
    json!({ "type": "object", "required": ["session", "effort", "reason"], "properties": { "session": { "type": "string" }, "effort": { "type": "string", "enum": EFFORTS }, "reason": { "type": "string" } } })
}
pub fn no_params() -> Value {
    json!({ "type": "object", "properties": {} })
}

fn child_of(data: &Path, parent: &str, id: &str) -> Result<Node, String> {
    let n = node(data, id.trim()).ok_or(format!("There's no session {id}; list_sessions shows them."))?;
    if n.parent != parent {
        return Err("Only the session that started it can manage it.".into());
    }
    Ok(n)
}

/// Runs a message in a session's chat in the background; its reply becomes the report.
fn run_in(state: Arc<crate::AppState>, id: String, text: String) {
    let Some(app) = state.app.get().cloned() else { return };
    let data = state.paths.data.clone();
    set_state(&data, &id, State::Running, None);
    tauri::async_runtime::spawn(async move {
        let r = crate::chat_cmds::run_turn(&app, &state, id.clone(), text, Vec::new(), None).await;
        let still = node(&data, &id).is_some_and(|n| n.state == State::Running);
        if still {
            match r {
                Ok(Some(m)) => set_state(&data, &id, State::Done, Some(m.content)),
                Ok(None) => set_state(&data, &id, State::Stopped, None),
                Err(e) => set_state(&data, &id, State::Failed, Some(e)),
            }
        }
        if node(&data, &id).is_some_and(|n| !n.live()) {
            crate::tools::locks::release(&id);
        }
        use tauri::Emitter;
        app.emit("sessions:changed", json!({ "id": id })).ok();
    });
}

fn stop_one(state: &crate::AppState, id: &str, to: State) {
    if let Some(flag) = state.generations.lock().unwrap().get(id) {
        flag.store(true, Ordering::Relaxed);
    }
    set_state(&state.paths.data, id, to, None);
    if to != State::Paused {
        crate::tools::locks::release(id);
    }
}

/// Stops every session in the tree `chat_id` belongs to, then frees the card.
pub async fn stop_all(state: &Arc<crate::AppState>, chat_id: &str) -> usize {
    let live: Vec<String> = tree_of(&state.paths.data, chat_id).into_iter().filter(Node::live).map(|n| n.id).collect();
    for id in &live {
        stop_one(state, id, State::Stopped);
    }
    if !live.is_empty() {
        state.engine.lock().await.stop().await;
    }
    live.len()
}

pub async fn tool(name: &str, args: &Value, ctx: &Ctx<'_>) -> Outcome {
    match run(name, args, ctx).await {
        Ok(o) => o,
        Err(e) => Outcome::error(name.replace('_', " "), e),
    }
}

async fn run(name: &str, args: &Value, ctx: &Ctx<'_>) -> Result<Outcome, String> {
    let state = ctx.state;
    let data = state.paths.data.clone();
    let me = ctx.memory.chat_id;
    match name {
        "list_sessions" => {
            let tree = tree_of(&data, me);
            if tree.is_empty() {
                return Ok(Outcome::ok("No sessions started yet.", "Sessions", "text", None));
            }
            let lines: Vec<String> = tree
                .iter()
                .map(|n| format!("{} [{}] depth {} {:?} model {} effort {} tokens~{}: {}{}", n.id, n.title, n.depth, n.state, n.model.as_deref().unwrap_or("same"), n.effort.as_deref().unwrap_or("default"), n.tokens, n.brief.goal, if n.report.is_empty() { String::new() } else { format!("\n  report: {}", n.report.chars().take(600).collect::<String>()) }))
                .collect();
            Ok(Outcome::ok(lines.join("\n"), format!("{} sessions", tree.len()), "text", None))
        }
        "stop_all_sessions" => {
            let n = stop_all(state, me).await;
            Ok(Outcome::ok(format!("Stopped {n} sessions and freed the graphics card."), "Stopped all sessions", "text", None))
        }
        "start_session" => {
            let goal = arg_str(args, "goal")?.trim().to_string();
            let reason = arg_str(args, "reason")?.to_string();
            let strings = |k: &str| args.get(k).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>();
            let brief = Brief { goal: goal.clone(), scope: strings("scope"), done: strings("done"), files: strings("files") };
            let folder = ctx.sandbox.resolve(arg_str(args, "folder")?)?;
            let (parent, ram) = {
                let conn = state.db.lock().unwrap();
                (crate::db::chat(&conn, ctx.memory.cipher, me).ok_or("This chat is gone.")?, crate::hardware::detect(&state.paths.models).ram_total)
            };
            let tree = tree_of(&data, me);
            let depth = can_spawn(&tree, me, &goal, &reason, max_live(ram))?;
            let model = args.get("model").and_then(Value::as_str).map(str::to_string).or(parent.model_id.clone()).ok_or("Pick a model for it.")?;
            let files = model_files(state);
            check_model(&model, &files.iter().map(|f| f.0.clone()).collect::<Vec<_>>(), cloud_ok(state)).or_else(|e| if Some(&model) == parent.model_id.as_ref() { Ok(()) } else { Err(e) })?;
            let mut loaded: Vec<String> = tree.iter().filter(|n| n.live()).filter_map(|n| n.model.clone()).collect();
            loaded.extend(parent.model_id.clone());
            let vram = crate::hardware::dedicated().map_or(0, |g| g.vram_bytes);
            let used: u64 = { let mut l = loaded.clone(); l.sort(); l.dedup(); l.iter().filter(|m| !crate::cloud::is_cloud(m)).filter_map(|m| need_of(&files, m)).sum() };
            let placement = place(&model, need_of(&files, &model), vram.saturating_sub(used), &loaded, &files.iter().map(|(m, n)| (m.clone(), n + n / 5)).collect::<Vec<_>>());
            let start_now = !matches!(placement, Placement::NoRoom { .. });
            // Its own worktree in a repository, else the folder itself.
            let (work, branch) = match crate::tools::git::repo_root(&folder) {
                Ok(root) => {
                    let name = format!("s-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
                    let wt = crate::tools::git::add_worktree(&root, &crate::tools::git::worktrees_dir(state), &name)?;
                    (wt.join(folder.strip_prefix(&root).unwrap_or(Path::new(""))), Some(format!("work/{name}")))
                }
                Err(_) => (folder.clone(), None),
            };
            let child = {
                let conn = state.db.lock().unwrap();
                let c = crate::db::create_chat_in(&conn, ctx.memory.cipher, Some(model.clone()), parent.project_id.clone(), false)?;
                let title = format!("Session: {}", goal.chars().take(50).collect::<String>());
                crate::db::set_chat_title(&conn, ctx.memory.cipher, &c.id, &title)?;
                // Never more freedom than the chat that started it.
                crate::db::set_chat_mode(&conn, &c.id, &parent.mode)?;
                crate::db::add_chat_folder(&conn, &c.id, &work.display().to_string())?;
                (c.id, title)
            };
            if branch.is_none() {
                crate::tools::locks::claim(&child.0, &work)?;
            }
            let effort = args.get("effort").and_then(Value::as_str).filter(|e| EFFORTS.contains(e)).map(str::to_string);
            let root = tree.iter().find(|n| n.id == me).map_or(me.to_string(), |n| n.root.clone());
            let n = Node {
                id: child.0.clone(),
                parent: me.to_string(),
                root,
                depth,
                title: child.1.clone(),
                brief: brief.clone(),
                reason,
                model: Some(model.clone()),
                effort,
                state: State::Queued,
                folder: work.display().to_string(),
                branch: branch.clone(),
                tokens: 0,
                started_at: chrono::Utc::now().timestamp_millis(),
                report: String::new(),
                log: vec![format!("started by {me} with {model}")],
            };
            with_tree(&data, |t| t.push(n));
            if let Some(app) = state.app.get() {
                use tauri::Emitter;
                app.emit("sessions:changed", json!({ "id": child.0 })).ok();
            }
            let note = match &placement {
                Placement::NoRoom { smaller } => format!(
                    " It is queued: {model} doesn't fit on the graphics card now. Wait for a session to finish, or set_session_model to {}{}.",
                    smaller.as_deref().map_or("a smaller local model".into(), |s| format!("{s} (fits)")),
                    if cloud_ok(state) { " or a cloud model" } else { "" }
                ),
                Placement::Share => " It shares the model already on the card.".into(),
                _ => String::new(),
            };
            if start_now {
                run_in(state.clone(), child.0.clone(), brief.text(&work.display().to_string()));
            }
            let merge = branch.map(|b| format!(" Its work lands on branch {b}; merge it with git_merge when its report is in.")).unwrap_or_default();
            let mut o = Outcome::ok(format!("Started session {} ({}).{note}{merge}", child.0, child.1), child.1.clone(), "text", None);
            o.meta["open_chat"] = json!(child.0);
            o.meta["open_title"] = json!(child.1);
            Ok(o)
        }
        "message_session" => {
            let n = child_of(&data, me, arg_str(args, "session")?)?;
            if n.state == State::Running {
                return Err("It's still working; wait for its report, or stop it first.".into());
            }
            log(&data, &n.id, "message from the parent".into());
            run_in(state.clone(), n.id.clone(), arg_str(args, "message")?.to_string());
            Ok(Outcome::ok(format!("Sent to {}; it is working on it.", n.title), format!("Messaged {}", n.title), "text", None))
        }
        "stop_session" => {
            let n = child_of(&data, me, arg_str(args, "session")?)?;
            let pause = args.get("pause").and_then(Value::as_bool).unwrap_or(false);
            stop_one(state, &n.id, if pause { State::Paused } else { State::Stopped });
            Ok(Outcome::ok(format!("{} {}.", if pause { "Paused" } else { "Stopped" }, n.title), format!("Stopped {}", n.title), "text", None))
        }
        "set_session_model" => {
            let n = child_of(&data, me, arg_str(args, "session")?)?;
            let model = arg_str(args, "model")?.trim().to_string();
            let files = model_files(state);
            check_model(&model, &files.iter().map(|f| f.0.clone()).collect::<Vec<_>>(), cloud_ok(state))?;
            crate::db::set_chat_model(&state.db.lock().unwrap(), &n.id, &model)?;
            with_tree(&data, |t| t.iter_mut().filter(|x| x.id == n.id).for_each(|x| x.model = Some(model.clone())));
            log(&data, &n.id, format!("model {} -> {model}: {}", n.model.unwrap_or_default(), arg_str(args, "reason")?));
            Ok(Outcome::ok(format!("{} now uses {model} from its next message.", n.title), format!("Model for {}", n.title), "text", None))
        }
        "set_session_effort" => {
            let n = child_of(&data, me, arg_str(args, "session")?)?;
            let effort = arg_str(args, "effort")?.to_string();
            if !EFFORTS.contains(&effort.as_str()) {
                return Err("effort is low, medium or high.".into());
            }
            with_tree(&data, |t| t.iter_mut().filter(|x| x.id == n.id).for_each(|x| x.effort = Some(effort.clone())));
            log(&data, &n.id, format!("effort -> {effort}: {}", arg_str(args, "reason")?));
            Ok(Outcome::ok(format!("{} works at {effort} effort from its next message.", n.title), format!("Effort for {}", n.title), "text", None))
        }
        other => Err(format!("Unknown tool {other}.")),
    }
}

/// The plan window's view of a tree.
#[tauri::command(async)]
pub fn sessions_tree(state: crate::AppStateRef, chat_id: String) -> Result<Vec<Node>, String> {
    state.cipher()?;
    Ok(tree_of(&state.paths.data, &chat_id))
}

#[tauri::command]
pub async fn sessions_stop_all(state: crate::AppStateRef<'_>, chat_id: String) -> Result<usize, String> {
    state.cipher()?;
    Ok(stop_all(&state, &chat_id).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(id: &str, parent: &str, depth: u8, state: State, goal: &str) -> Node {
        Node { id: id.into(), parent: parent.into(), root: "root".into(), depth, title: id.into(), brief: Brief { goal: goal.into(), ..Default::default() }, reason: String::new(), model: None, effort: None, state, folder: String::new(), branch: None, tokens: 0, started_at: 0, report: String::new(), log: vec![] }
    }

    #[test]
    fn depth_count_reason_and_repeats_are_capped() {
        let why = "it is a separate part of the job";
        let tree = vec![n("a", "root", 1, State::Running, "parse"), n("b", "a", 2, State::Running, "lex")];
        assert_eq!(can_spawn(&tree, "root", "docs", why, 4), Ok(1));
        assert_eq!(can_spawn(&tree, "a", "docs", why, 4), Ok(2));
        assert!(can_spawn(&tree, "b", "docs", why, 4).unwrap_err().contains("levels deep"));
        assert!(can_spawn(&tree, "root", "docs", why, 2).unwrap_err().contains("already working"));
        assert!(can_spawn(&tree, "root", "Parse", why, 4).unwrap_err().contains("same goal"));
        assert!(can_spawn(&tree, "root", "docs", "", 4).is_err());
    }

    #[test]
    fn a_second_local_child_starts_only_when_it_fits_and_cloud_ignores_the_card() {
        const GB: u64 = 1 << 30;
        let small = vec![("qwen-4b".to_string(), 3 * GB)];
        // 12 GB card, an 8 GB model already loaded: 4 GB left.
        assert_eq!(place("qwen-14b", Some(9 * GB), 4 * GB, &["qwen-8b".into()], &small), Placement::NoRoom { smaller: Some("qwen-4b".into()) });
        assert_eq!(place("qwen-4b", Some(3 * GB), 4 * GB, &["qwen-8b".into()], &small), Placement::Fits);
        assert_eq!(place("qwen-8b", Some(8 * GB), 0, &["qwen-8b".into()], &small), Placement::Share);
        assert_eq!(place("cloud:openai/gpt", None, 0, &[], &small), Placement::Cloud);
    }

    #[test]
    fn model_changes_stay_inside_what_the_person_has() {
        let installed = vec!["qwen-4b".to_string()];
        assert!(check_model("qwen-4b", &installed, false).is_ok());
        assert!(check_model("other", &installed, true).is_err());
        assert!(check_model("cloud:x/y", &installed, false).is_err());
        assert!(check_model("cloud:x/y", &installed, true).is_ok());
    }

    #[test]
    fn the_brief_names_goal_folder_and_checks() {
        let b = Brief { goal: "Fix the parser".into(), done: vec!["cargo test parser".into()], ..Default::default() };
        let t = b.text("C:/w/s-1");
        assert!(t.contains("Goal: Fix the parser") && t.contains("Work only in C:/w/s-1") && t.contains("- cargo test parser"));
    }
}
