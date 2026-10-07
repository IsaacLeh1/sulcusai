// SPDX-License-Identifier: AGPL-3.0-only
//! Tools the model can call, what each one risks, and how they run.

pub mod browser;
mod fs;
mod meetings;
mod memory;
mod notes;
mod shell;
mod web;

use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use rusqlite::Connection;

use serde::Serialize;
use serde_json::{json, Value};

use crate::checkpoint::Recorder;
use crate::crypto::Cipher;
use crate::engine::JobRef;
use crate::sandbox::Sandbox;

/// How much a tool can affect. Decides when the user is asked first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// Looks at files; changes nothing.
    Read,
    /// Creates, changes, moves or deletes files (undoable).
    Write,
    /// Runs a program; effects can't be undone automatically.
    Execute,
    /// Saves to the assistant's own memory, which the user can review and edit.
    Memory,
    /// A tool from a connector (MCP server); what it does is up to that program.
    Connector,
    /// Submits a form, buys, sends, posts or deletes on a website. Always
    /// asks, in every mode, and is never allowed for a whole chat.
    Submit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Plan,
    Auto,
    Bypass,
}

impl Mode {
    pub fn parse(s: &str) -> Mode {
        match s {
            "plan" => Mode::Plan,
            "bypass" => Mode::Bypass,
            _ => Mode::Auto,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    Run,
    Ask,
    Refuse,
}

/// The run-mode rules. `allowed` holds risks the user already approved for this chat.
pub fn permission(mode: Mode, risk: Risk, allowed_for_chat: bool) -> Permission {
    match (mode, risk) {
        (_, Risk::Read | Risk::Memory) => Permission::Run,
        (Mode::Plan, _) => Permission::Refuse,
        (_, Risk::Submit) => Permission::Ask,
        (Mode::Bypass, _) => Permission::Run,
        (Mode::Auto, _) if allowed_for_chat => Permission::Run,
        (Mode::Auto, _) => Permission::Ask,
    }
}

pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub risk: Risk,
    pub params: fn() -> Value,
}

/// Which group a tool belongs to; groups are offered only when available.
fn group(name: &str) -> &'static str {
    match name {
        "remember" | "search_memory" => "memory",
        "load_skill" => "skills",
        "search_meetings" | "read_meeting" => "meetings",
        "create_note" | "search_notes" | "read_note" | "update_note" => "notes",
        "create_task" | "list_tasks" | "complete_task" => "tasks",
        "web_search" | "fetch_page" | "request_web" => "web",
        n if n.starts_with("browser_") => "browser",
        _ => "files",
    }
}

pub const TOOLS: &[ToolDef] = &[
    ToolDef { name: "list_dir", risk: Risk::Read, params: fs::list_dir_params,
        description: "List a folder's contents. With no path, lists the shared folders." },
    ToolDef { name: "read_file", risk: Risk::Read, params: fs::read_file_params,
        description: "Read a text file. Each line starts with its number and a tab; the numbers are not part of the file, so never copy them into edits. Use start_line/end_line for long files." },
    ToolDef { name: "find_files", risk: Risk::Read, params: fs::find_files_params,
        description: "Find files by name with a glob such as **/*.rs or *report*. Respects .gitignore." },
    ToolDef { name: "search_files", risk: Risk::Read, params: fs::search_files_params,
        description: "Search file contents with a regular expression. Returns path:line: text matches. Respects .gitignore." },
    ToolDef { name: "write_file", risk: Risk::Write, params: fs::write_file_params,
        description: "Create a file, or replace a file's entire contents. Prefer edit_file for changes to existing files." },
    ToolDef { name: "edit_file", risk: Risk::Write, params: fs::edit_file_params,
        description: "Replace exact text in a file. old_text must match exactly (including spaces) and be unique unless replace_all is true. Read the file first." },
    ToolDef { name: "move_path", risk: Risk::Write, params: fs::move_path_params,
        description: "Move or rename a file or folder." },
    ToolDef { name: "make_folder", risk: Risk::Write, params: fs::make_folder_params,
        description: "Create a folder (and any missing parent folders)." },
    ToolDef { name: "delete_path", risk: Risk::Write, params: fs::delete_path_params,
        description: "Move a file or folder to the Recycle Bin." },
    ToolDef { name: "run_command", risk: Risk::Execute, params: shell::run_command_params,
        description: "Run a PowerShell command in a shared folder and return its output, for example to build, test, use git or run scripts. Runs hidden; no interactive input." },
    ToolDef { name: "remember", risk: Risk::Memory, params: memory::remember_params,
        description: "Save one lasting fact about the user, their work or their preferences, to recall in future chats. One short sentence. Never save secrets." },
    ToolDef { name: "search_memory", risk: Risk::Read, params: memory::search_memory_params,
        description: "Search what you remember from earlier chats." },
    ToolDef { name: "search_meetings", risk: Risk::Read, params: meetings::search_meetings_params,
        description: "Search the user's recorded meetings (titles, notes and transcripts) by words. Returns matching meetings with ids." },
    ToolDef { name: "read_meeting", risk: Risk::Read, params: meetings::read_meeting_params,
        description: "Read a recorded meeting's notes (summary, decisions, action items) or its full transcript, by id." },
    ToolDef { name: "create_note", risk: Risk::Memory, params: notes::create_note_params,
        description: "Save a note in the user's Notes (Markdown). Use when they ask you to note, jot down or save something." },
    ToolDef { name: "search_notes", risk: Risk::Read, params: notes::search_notes_params,
        description: "Find the user's notes by words. Returns titles, ids and a preview." },
    ToolDef { name: "read_note", risk: Risk::Read, params: notes::read_note_params,
        description: "Read one of the user's notes in full, by id." },
    ToolDef { name: "update_note", risk: Risk::Write, params: notes::update_note_params,
        description: "Replace a note's text (read it first, then send the whole new text)." },
    ToolDef { name: "create_task", risk: Risk::Memory, params: notes::create_task_params,
        description: "Add a task to the user's to-do list, with an optional due date, priority and reminder." },
    ToolDef { name: "list_tasks", risk: Risk::Read, params: notes::list_tasks_params,
        description: "List the user's tasks with their ids and due dates." },
    ToolDef { name: "complete_task", risk: Risk::Memory, params: notes::complete_task_params,
        description: "Mark one of the user's tasks done, by id." },
    ToolDef { name: "web_search", risk: Risk::Read, params: web::web_search_params,
        description: "Search the web for current information. Returns titles, addresses and snippets to cite." },
    ToolDef { name: "fetch_page", risk: Risk::Read, params: web::fetch_page_params,
        description: "Read a web page as text, by its address. Use it on search results to get the details." },
    ToolDef { name: "request_web", risk: Risk::Read, params: web::request_web_params,
        description: "Web access is off in this chat. If answering needs current information from the internet, call this to ask the user to turn it on, then stop." },
    ToolDef { name: "browser_open", risk: Risk::Read, params: browser::browser_open_params,
        description: "Open a website in the built-in browser, which the user can watch. Use it to use a site (fill in a search, click through pages); for just reading, web_search and fetch_page are quicker. Returns the page's text and its links, buttons and fields, numbered." },
    ToolDef { name: "browser_read", risk: Risk::Read, params: browser::browser_read_params,
        description: "Read the browser's current page again: its text and its numbered links, buttons and fields." },
    ToolDef { name: "browser_click", risk: Risk::Read, params: browser::browser_click_params,
        description: "Click a link or button in the browser, by its number from the page. Anything that submits a form, buys, sends, posts or deletes asks the user first." },
    ToolDef { name: "browser_type", risk: Risk::Read, params: browser::browser_type_params,
        description: "Type into a field in the browser by its number (replacing what's there), or pick a dropdown choice. Never for passwords or payment details: ask the user to type those in the browser window." },
    ToolDef { name: "browser_back", risk: Risk::Read, params: browser::browser_back_params,
        description: "Go back to the previous page in the browser." },
    ToolDef { name: "load_skill", risk: Risk::Read, params: load_skill_params,
        description: "Read the instructions for one of your skills, by name." },
    ToolDef { name: "delegate", risk: Risk::Read, params: delegate_params,
        description: "Hand a self-contained research task to a helper agent, such as finding where something is defined or summarizing a set of files. The helper has its own fresh context and read-only tools, and returns a short report. Use it for digging that would otherwise fill this conversation." },
];

fn load_skill_params() -> Value {
    json!({ "type": "object", "required": ["name"], "properties": { "name": { "type": "string" } } })
}

fn delegate_params() -> Value {
    json!({ "type": "object", "required": ["task"], "properties": {
        "task": { "type": "string", "description": "What the helper should find out, with any details it needs. It can't see this conversation." } } })
}

/// Tools a helper agent may use: read-only, and no further helpers.
pub fn helper_definitions(files: bool) -> Value {
    let mut defs = definitions(Mode::Plan, files, false);
    if let Some(list) = defs.as_array_mut() {
        list.retain(|d| d["function"]["name"] != "delegate");
    }
    defs
}

pub fn find(name: &str) -> Option<&'static ToolDef> {
    TOOLS.iter().find(|t| t.name == name)
}

/// OpenAI-style definitions for the tools this mode may use. File tools
/// need a shared folder; memory tools need memory turned on.
pub fn definitions(mode: Mode, files: bool, memory: bool) -> Value {
    let list: Vec<Value> = TOOLS
        .iter()
        .filter(|t| permission(mode, t.risk, false) != Permission::Refuse)
        .filter(|t| match group(t.name) {
            "memory" => memory,
            // Added by the agent when a plugin provides skills, or when there are meetings.
            "skills" | "meetings" | "notes" | "tasks" | "web" | "browser" => false,
            _ => files,
        })
        .map(|t| json!({ "type": "function", "function": { "name": t.name, "description": t.description, "parameters": (t.params)() } }))
        .collect();
    Value::Array(list)
}

/// What the approval card shows before a risky tool runs.
#[derive(Debug, Clone, Serialize)]
pub struct Preview {
    pub title: String,
    /// "diff", "command" or "text".
    pub kind: &'static str,
    pub detail: Option<String>,
    pub note: Option<String>,
}

/// The result: text for the model, plus what the window shows.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub text: String,
    pub meta: Value,
}

impl Outcome {
    pub fn ok(text: impl Into<String>, title: impl Into<String>, kind: &str, detail: Option<String>) -> Outcome {
        Outcome { text: text.into(), meta: json!({ "title": title.into(), "status": "ok", "kind": kind, "detail": detail }) }
    }

    pub fn error(title: impl Into<String>, message: impl Into<String>) -> Outcome {
        let message = message.into();
        Outcome {
            text: format!("Error: {message}"),
            meta: json!({ "title": title.into(), "status": "error", "kind": "text", "detail": message }),
        }
    }

    pub fn denied(title: impl Into<String>) -> Outcome {
        Outcome {
            text: "The user declined this action. Don't try it again unless they ask; suggest another approach or ask what they'd like.".into(),
            meta: json!({ "title": title.into(), "status": "denied", "kind": "text" }),
        }
    }
}

pub struct Ctx<'a> {
    pub sandbox: &'a Sandbox,
    pub recorder: &'a Recorder<'a>,
    pub cancel: &'a AtomicBool,
    pub job: Option<&'a JobRef>,
    pub memory: MemoryCtx<'a>,
    /// For the browser tools.
    pub app: Option<&'a tauri::AppHandle>,
}

pub struct MemoryCtx<'a> {
    pub db: &'a Mutex<Connection>,
    pub cipher: &'a Cipher,
    pub chat_id: &'a str,
    pub project_id: Option<&'a str>,
}

/// A unified diff of two texts, as approval cards show it.
pub fn text_diff(old: &str, new: &str) -> String {
    fs::diff("note", old, new)
}

/// The approval preview for rewriting a note (it needs the stored note).
pub fn notes_preview(name: &str, args: &Value, db: &Mutex<Connection>, cipher: &Cipher) -> Result<Preview, String> {
    let id = args.get("note_id").and_then(Value::as_str).unwrap_or("");
    let note = crate::notes::get_note(&db.lock().unwrap(), cipher, id);
    notes::preview(name, args, note)
}

pub fn preview(name: &str, args: &Value, sandbox: &Sandbox) -> Result<Preview, String> {
    match name {
        "run_command" => shell::preview(args, sandbox),
        _ => fs::preview(name, args, sandbox),
    }
}

pub async fn run(name: &str, args: &Value, ctx: &Ctx<'_>) -> Outcome {
    match name {
        "run_command" => shell::run(args, ctx).await,
        "remember" | "search_memory" => memory::run(name, args, &ctx.memory),
        "search_meetings" | "read_meeting" => meetings::run(name, args, &ctx.memory),
        "create_note" | "search_notes" | "read_note" | "update_note" | "create_task" | "list_tasks" | "complete_task" => {
            notes::run(name, args, &ctx.memory)
        }
        "web_search" | "fetch_page" | "request_web" => web::run(name, args, &ctx.memory).await,
        n if n.starts_with("browser_") => {
            let db = ctx.memory.db;
            browser::run(name, args, ctx.app, |summary| crate::db::log_action(&db.lock().unwrap(), "network", summary)).await
        }
        _ => {
            // File work is quick but blocking; keep it off the async threads.
            let name = name.to_string();
            let args = args.clone();
            tokio::task::block_in_place(|| fs::run(&name, &args, ctx))
        }
    }
}

/// A readable title for a step that failed before or while running.
pub fn failed_title(name: &str, args: &Value) -> String {
    let target = ["path", "from", "pattern", "command"]
        .iter()
        .find_map(|k| args.get(*k).and_then(Value::as_str))
        .map(|t| t.chars().take(60).collect::<String>())
        .unwrap_or_default();
    let verb = match name {
        "list_dir" => "list",
        "read_file" => "read",
        "find_files" | "search_files" => "search for",
        "remember" => return "Couldn't save a memory".into(),
        "delegate" => return "A helper couldn't finish".into(),
        "search_memory" => return "Couldn't search memory".into(),
        "search_meetings" | "read_meeting" => return "Couldn't look up a meeting".into(),
        "create_note" | "search_notes" | "read_note" | "update_note" => return "Couldn't use the notes".into(),
        "create_task" | "list_tasks" | "complete_task" => return "Couldn't use the task list".into(),
        "web_search" => return "Couldn't search the web".into(),
        "fetch_page" => return "Couldn't open a web page".into(),
        n if n.starts_with("browser_") => return "Couldn't use the browser".into(),
        "write_file" => "write",
        "edit_file" => "edit",
        "move_path" => "move",
        "make_folder" => "create folder",
        "delete_path" => "delete",
        "run_command" => "run",
        _ => return format!("Couldn't use {name}"),
    };
    format!("Couldn't {verb} {target}").trim_end().to_string()
}

pub fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key).and_then(Value::as_str).ok_or_else(|| format!("Missing \"{key}\"."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_modes_follow_the_design() {
        use Permission::*;
        assert_eq!(permission(Mode::Plan, Risk::Read, false), Run);
        assert_eq!(permission(Mode::Plan, Risk::Write, true), Refuse, "plan never changes anything");
        assert_eq!(permission(Mode::Auto, Risk::Write, false), Ask);
        assert_eq!(permission(Mode::Auto, Risk::Execute, true), Run, "approved for this chat");
        assert_eq!(permission(Mode::Bypass, Risk::Execute, false), Run);
    }

    #[test]
    fn plan_mode_only_offers_read_tools() {
        let defs = definitions(Mode::Plan, true, true);
        let names: Vec<&str> = defs.as_array().unwrap().iter().map(|d| d["function"]["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"read_file"));
        assert!(!names.contains(&"write_file") && !names.contains(&"run_command"));
        assert!(names.contains(&"remember"), "saving a memory is allowed while planning");
        // Everything except load_skill and the meeting, note, task, web and browser tools, which the agent adds when they apply.
        assert_eq!(definitions(Mode::Auto, true, true).as_array().unwrap().len(), TOOLS.len() - 18);
    }

    #[test]
    fn tool_groups_depend_on_folders_and_memory() {
        let names = |d: Value| -> Vec<String> { d.as_array().unwrap().iter().map(|x| x["function"]["name"].as_str().unwrap().to_string()).collect() };
        let only_memory = names(definitions(Mode::Auto, false, true));
        assert_eq!(only_memory, vec!["remember", "search_memory"]);
        assert!(!names(definitions(Mode::Auto, true, false)).iter().any(|n| n == "remember"));
        assert!(definitions(Mode::Auto, false, false).as_array().unwrap().is_empty());
    }

    #[test]
    fn helpers_get_read_tools_but_no_helpers_of_their_own() {
        let defs = helper_definitions(true);
        let names: Vec<&str> = defs.as_array().unwrap().iter().map(|d| d["function"]["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"read_file") && names.contains(&"search_files"));
        assert!(!names.contains(&"delegate") && !names.contains(&"write_file") && !names.contains(&"remember"));
        let main: Vec<String> = definitions(Mode::Auto, true, true).as_array().unwrap().iter().map(|d| d["function"]["name"].as_str().unwrap().to_string()).collect();
        assert!(main.iter().any(|n| n == "delegate"));
    }

    #[test]
    fn every_tool_has_an_object_schema() {
        for t in TOOLS {
            assert_eq!((t.params)()["type"], "object", "{}", t.name);
        }
    }
}
