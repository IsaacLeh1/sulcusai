// SPDX-License-Identifier: AGPL-3.0-only
//! Checks a plugin runs at four points of a turn: before a tool runs, after
//! it runs, when a message is sent, and when a reply is done. A check can
//! stop what's about to happen, have the person asked first, or hand the
//! model a note (a plugin can run the tests after every file write and give
//! the failures back). The person agrees to a plugin's checks, by their
//! commands, when installing it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::sandbox::Sandbox;
use crate::tools::{guard, shell, Mode};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    BeforeTool,
    AfterTool,
    MessageSent,
    ReplyDone,
}

impl Event {
    fn name(self) -> &'static str {
        match self {
            Event::BeforeTool => "before_tool",
            Event::AfterTool => "after_tool",
            Event::MessageSent => "message_sent",
            Event::ReplyDone => "reply_done",
        }
    }
}

fn thirty() -> u64 {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Check {
    pub event: Event,
    /// Only for these tools (names, or prefixes ending in `*`); empty for all.
    #[serde(default)]
    pub tools: Vec<String>,
    pub command: String,
    /// Seconds, 1 to 120.
    #[serde(default = "thirty")]
    pub timeout: u64,
}

impl Check {
    pub fn covers(&self, tool: Option<&str>) -> bool {
        let Some(tool) = tool else { return self.tools.is_empty() };
        self.tools.is_empty() || self.tools.iter().any(|t| match t.strip_suffix('*') {
            Some(prefix) => tool.starts_with(prefix),
            None => t == tool,
        })
    }
}

/// A check with the plugin it came with, for one turn.
#[derive(Debug, Clone)]
pub struct Armed {
    pub plugin: String,
    pub dir: PathBuf,
    pub check: Check,
}

/// The checks of the enabled plugins.
pub fn armed(conn: &rusqlite::Connection, c: &crate::crypto::Cipher) -> Vec<Armed> {
    crate::connectors::plugins(conn, c)
        .into_iter()
        .filter(|p| p.enabled)
        .flat_map(|p| p.checks.iter().map(|k| Armed { plugin: p.name.clone(), dir: PathBuf::from(&p.dir), check: k.clone() }).collect::<Vec<_>>())
        .collect()
}

/// What the checks said, together.
#[derive(Debug, Default, PartialEq)]
pub struct Answer {
    /// Stop it, and why (with the plugin's name).
    pub block: Option<String>,
    /// Have the person asked first, and why.
    pub ask: Option<String>,
    /// Allowed without asking (never past what always asks).
    pub allow: bool,
    /// For the model.
    pub notes: Vec<String>,
}

/// What one run said.
#[derive(Debug, PartialEq)]
enum Said {
    Carry,
    Block(String),
    Ask(String),
    Allow,
    Note(String),
    /// It failed or ran out of time: carried on, and the plugin's page says so.
    Failed(String),
}

/// Reads a check's exit code and output.
fn said(code: Option<i32>, out: &str, err: &str) -> Vec<Said> {
    match code {
        None => vec![Said::Failed("it ran out of time and was stopped".into())],
        Some(2) => vec![Said::Block(first_lines(err).unwrap_or_else(|| "no reason given".into()))],
        Some(0) if out.trim().is_empty() => vec![Said::Carry],
        Some(0) => {
            let Ok(v) = serde_json::from_str::<Value>(out.trim()) else {
                return vec![Said::Failed("what it printed isn't the JSON a check answers with".into())];
            };
            let reason = v["reason"].as_str().unwrap_or("").trim().to_string();
            let mut all = match v["decision"].as_str() {
                Some("block") => vec![Said::Block(if reason.is_empty() { "no reason given".into() } else { reason })],
                Some("ask") => vec![Said::Ask(reason)],
                Some("allow") => vec![Said::Allow],
                _ => vec![],
            };
            if let Some(n) = v["note"].as_str().map(str::trim).filter(|n| !n.is_empty()) {
                all.push(Said::Note(n.chars().take(4000).collect()));
            }
            if all.is_empty() { vec![Said::Carry] } else { all }
        }
        Some(c) => vec![Said::Failed(format!("it ended with exit code {c}{}", first_lines(err).map(|e| format!(": {e}")).unwrap_or_default()))],
    }
}

/// What a check's errors say. PowerShell's own errors come with an account
/// of where (the script, "At line", the category); only the message is kept.
fn first_lines(s: &str) -> Option<String> {
    let (s, powershell) = match s.split_once("\n    + CategoryInfo") {
        Some((before, _)) => (before, true),
        None => (s, false),
    };
    let lines: Vec<&str> = s.trim().lines().map(str::trim).filter(|l| !l.is_empty() && !(powershell && (l.starts_with("At line:") || l.starts_with('+')))).collect();
    let t: String = match lines.iter().rposition(|l| powershell && l.contains(" : ")) {
        // "Write-Error 'x'; exit 2 : x", "Get-Item : Cannot find path …",
        // wrapped onto the next lines when it's long.
        Some(at) => {
            let joined = lines[at..].join(" ");
            joined.rsplit(" : ").next().unwrap_or(&joined).to_string()
        }
        None => lines.into_iter().take(5).collect::<Vec<_>>().join(" "),
    };
    let t: String = t.chars().take(600).collect();
    (!t.is_empty()).then_some(t)
}

/// The last problem each plugin's checks had, for its page (until the app
/// restarts or a check of it runs cleanly).
static PROBLEMS: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Default::default);

pub fn problems() -> HashMap<String, String> {
    PROBLEMS.lock().unwrap().clone()
}

/// What a check is told about the moment it runs at.
pub struct Moment<'a> {
    pub event: Event,
    pub chat_id: &'a str,
    pub tool: Option<&'a str>,
    pub arguments: Option<&'a Value>,
    pub result: Option<&'a str>,
    /// The chat's first shared folder, where checks run (else the plugin's own).
    pub folder: Option<&'a Path>,
    pub mode: Mode,
}

impl<'a> Moment<'a> {
    pub fn at(event: Event, chat_id: &'a str, folder: Option<&'a Path>, mode: Mode) -> Moment<'a> {
        Moment { event, chat_id, tool: None, arguments: None, result: None, folder, mode }
    }
}

/// Runs the checks for this moment, one after another, and puts together
/// what they said. A check the guard refuses is skipped.
pub async fn run(state: &crate::AppState, checks: &[Armed], m: &Moment<'_>, sandbox: &Sandbox) -> Answer {
    let mut answer = Answer::default();
    for a in checks.iter().filter(|a| a.check.event == m.event && a.check.covers(m.tool)) {
        let dir = m.folder.map(Path::to_path_buf).unwrap_or_else(|| a.dir.clone());
        if let Some(why) = guard::refuse(&a.check.command, &dir, sandbox) {
            state.log("agent", &format!("Skipped a check from the plugin {}: {why}", a.plugin));
            continue;
        }
        let input = json!({
            "event": m.event.name(),
            "chat_id": m.chat_id,
            "tool": m.tool,
            "arguments": m.arguments,
            "result": m.result,
            "folder": m.folder.map(|f| f.display().to_string()),
            "mode": match m.mode { Mode::Plan => "plan", Mode::Auto => "auto", Mode::Bypass => "bypass" },
        });
        let started = Instant::now();
        let timeout = Duration::from_secs(a.check.timeout.clamp(1, 120));
        let run = shell::run_check(&a.check.command, &dir, &[("SULCUSAI_EVENT", m.event.name())], &input.to_string(), timeout).await;
        let (code, all) = match run {
            Ok(r) => (r.code.map_or("none".to_string(), |c| c.to_string()), said(r.code, &r.out, &r.err)),
            Err(e) => ("none".into(), vec![Said::Failed(e)]),
        };
        state.log("agent", &format!("Ran the {} check of the plugin {}: exit {code} in {} ms", m.event.name(), a.plugin, started.elapsed().as_millis()));
        if !all.iter().any(|s| matches!(s, Said::Failed(_))) {
            PROBLEMS.lock().unwrap().remove(&a.plugin);
        }
        for s in all {
            match s {
                Said::Carry => {}
                Said::Block(r) => {
                    answer.block.get_or_insert_with(|| format!("A check from the plugin {} stopped this: {r}", a.plugin));
                }
                Said::Ask(r) => {
                    answer.ask.get_or_insert_with(|| if r.is_empty() { format!("The plugin {}'s check asked for this.", a.plugin) } else { format!("The plugin {}'s check asked for this: {r}", a.plugin) });
                }
                Said::Allow => answer.allow = true,
                Said::Note(n) => answer.notes.push(format!("[Note from the plugin {}: {n}]", a.plugin)),
                Said::Failed(why) => {
                    let line = format!("Its {} check ({}) didn't work: {why}", m.event.name(), a.check.command);
                    state.log("agent", &format!("The plugin {}: {line}", a.plugin));
                    PROBLEMS.lock().unwrap().insert(a.plugin.clone(), line);
                }
            }
        }
    }
    // A block or a question outweighs an allow.
    if answer.block.is_some() || answer.ask.is_some() {
        answer.allow = false;
    }
    answer
}

#[tauri::command(async)]
pub fn plugin_check_problems(state: crate::AppStateRef) -> Result<HashMap<String, String>, String> {
    state.cipher()?;
    Ok(problems())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_are_read_from_the_exit_code_and_json() {
        assert_eq!(said(Some(0), "", ""), [Said::Carry]);
        assert_eq!(said(Some(2), "", "lint failed\n  at x.rs"), [Said::Block("lint failed at x.rs".into())]);
        assert_eq!(said(Some(2), "", ""), [Said::Block("no reason given".into())]);
        // PowerShell's account of where an error came from is left out.
        // Wrapped at the console's width, as PowerShell writes it.
        let ps = "$ProgressPreference='SilentlyContinue';\nWrite-Error 'No new files in this folder today.'; exit 2 : No new files in this\nfolder today.\n    + CategoryInfo          : NotSpecified: (:) [Write-Error], WriteErrorException\n";
        assert_eq!(said(Some(2), "", ps), [Said::Block("No new files in this folder today.".into())]);
        let thrown = "boom here\nAt line:2 char:1\n+ throw 'boom here'\n    + CategoryInfo          : OperationStopped: (boom here:String) [], RuntimeException\n";
        assert_eq!(said(Some(2), "", thrown), [Said::Block("boom here".into())]);
        assert_eq!(said(Some(0), r#"{"decision":"block","reason":"x"}"#, ""), [Said::Block("x".into())]);
        assert_eq!(said(Some(0), r#"{"decision":"ask","reason":"big change"}"#, ""), [Said::Ask("big change".into())]);
        assert_eq!(said(Some(0), r#"{"decision":"allow","note":"checked"}"#, ""), [Said::Allow, Said::Note("checked".into())]);
        assert_eq!(said(Some(0), r#"{"note":"3 tests failed"}"#, ""), [Said::Note("3 tests failed".into())]);
        assert!(matches!(said(Some(0), "all good", "")[0], Said::Failed(_)));
        assert!(matches!(said(Some(1), "", "boom")[0], Said::Failed(ref e) if e.contains("exit code 1: boom")));
        assert!(matches!(said(None, "", "")[0], Said::Failed(ref e) if e.contains("out of time")));
    }

    #[test]
    fn checks_cover_their_tools() {
        let k = |tools: &[&str]| Check { event: Event::BeforeTool, tools: tools.iter().map(|s| s.to_string()).collect(), command: "x".into(), timeout: 30 };
        assert!(k(&[]).covers(Some("write_file")) && k(&[]).covers(None));
        assert!(k(&["write_file", "browser_*"]).covers(Some("browser_click")));
        assert!(!k(&["write_file"]).covers(Some("edit_file")) && !k(&["write_file"]).covers(None));
        let parsed: Check = serde_json::from_value(json!({ "event": "reply_done", "command": "pwsh -File t.ps1" })).unwrap();
        assert_eq!((parsed.event, parsed.timeout), (Event::ReplyDone, 30));
    }

    fn armed_with(event: Event, command: &str, timeout: u64) -> Vec<Armed> {
        vec![Armed { plugin: "tester".into(), dir: std::env::temp_dir(), check: Check { event, tools: vec![], command: command.into(), timeout } }]
    }

    /// Real checks through the shell: a block by exit code, a block by
    /// JSON, a note that reads what it was given on stdin, and one that runs
    /// too long is ended.
    #[tokio::test]
    #[cfg(windows)]
    async fn real_checks_block_note_and_time_out() {
        let state = crate::testutil::scratch_state();
        let sb = Sandbox::default();
        let m = Moment { event: Event::AfterTool, chat_id: "c1", tool: Some("write_file"), arguments: None, result: Some("ok"), folder: None, mode: Mode::Auto };
        let a = run(&state, &armed_with(Event::AfterTool, "cmd /c \"echo bad file 1>&2 & exit 2\"", 30), &m, &sb).await;
        assert_eq!(a.block.as_deref(), Some("A check from the plugin tester stopped this: bad file"));
        let a = run(&state, &armed_with(Event::AfterTool, "Write-Error 'No new files today.'; exit 2", 30), &m, &sb).await;
        assert_eq!(a.block.as_deref(), Some("A check from the plugin tester stopped this: No new files today."));
        // Checks run in PowerShell on Windows, as the assistant's commands do.
        let a = run(&state, &armed_with(Event::AfterTool, r#"Write-Output '{"decision":"block","reason":"x"}'"#, 30), &m, &sb).await;
        assert_eq!(a.block.as_deref(), Some("A check from the plugin tester stopped this: x"));
        // What it's given arrives on stdin.
        let read = "$i = [Console]::In.ReadToEnd() | ConvertFrom-Json; '{\"note\":\"' + $i.event + ' ' + $i.tool + ' in ' + $env:SULCUSAI_EVENT + '\"}'";
        let a = run(&state, &armed_with(Event::AfterTool, read, 30), &m, &sb).await;
        assert_eq!(a.notes, ["[Note from the plugin tester: after_tool write_file in after_tool]"]);
        // Another moment's check doesn't run.
        assert_eq!(run(&state, &armed_with(Event::ReplyDone, "cmd /c exit 2", 30), &m, &sb).await, Answer::default());
        let started = Instant::now();
        let a = run(&state, &armed_with(Event::AfterTool, "Start-Sleep -Seconds 30", 1), &m, &sb).await;
        assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
        assert_eq!(a, Answer::default());
        assert!(problems().get("tester").is_some_and(|p| p.contains("out of time")));
    }
}
