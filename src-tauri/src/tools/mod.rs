// SPDX-License-Identifier: AGPL-3.0-only
//! Tools the model can call, what each one risks, and how they run.

mod fs;
mod shell;

use std::sync::atomic::AtomicBool;

use serde::Serialize;
use serde_json::{json, Value};

use crate::checkpoint::Recorder;
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
        (_, Risk::Read) => Permission::Run,
        (Mode::Plan, _) => Permission::Refuse,
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

pub const TOOLS: &[ToolDef] = &[
    ToolDef { name: "list_dir", risk: Risk::Read, params: fs::list_dir_params,
        description: "List a folder's contents. With no path, lists the shared folders." },
    ToolDef { name: "read_file", risk: Risk::Read, params: fs::read_file_params,
        description: "Read a text file. Lines are numbered. Use start_line/end_line for long files." },
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
];

pub fn find(name: &str) -> Option<&'static ToolDef> {
    TOOLS.iter().find(|t| t.name == name)
}

/// OpenAI-style definitions for the tools this mode may use.
pub fn definitions(mode: Mode) -> Value {
    let list: Vec<Value> = TOOLS
        .iter()
        .filter(|t| permission(mode, t.risk, false) != Permission::Refuse)
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
        _ => {
            // File work is quick but blocking; keep it off the async threads.
            let name = name.to_string();
            let args = args.clone();
            tokio::task::block_in_place(|| fs::run(&name, &args, ctx))
        }
    }
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
        let defs = definitions(Mode::Plan);
        let names: Vec<&str> = defs.as_array().unwrap().iter().map(|d| d["function"]["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"read_file"));
        assert!(!names.contains(&"write_file") && !names.contains(&"run_command"));
        assert_eq!(definitions(Mode::Auto).as_array().unwrap().len(), TOOLS.len());
    }

    #[test]
    fn every_tool_has_an_object_schema() {
        for t in TOOLS {
            assert_eq!((t.params)()["type"], "object", "{}", t.name);
        }
    }
}
