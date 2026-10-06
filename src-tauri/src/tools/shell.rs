// SPDX-License-Identifier: AGPL-3.0-only
//! run_command: PowerShell in a shared folder, hidden, with a time limit.

use std::process::Stdio;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;

use super::{arg_str, Ctx, Outcome, Preview};
use crate::sandbox::Sandbox;

const DEFAULT_TIMEOUT: u64 = 120;
const MAX_TIMEOUT: u64 = 600;
const KEEP_CHARS: usize = 12_000;

pub fn run_command_params() -> Value {
    json!({ "type": "object", "required": ["command"], "properties": {
        "command": { "type": "string", "description": "PowerShell command(s) to run." },
        "folder": { "type": "string", "description": "Shared folder (or subfolder) to run in. Default: the first shared folder." },
        "timeout_seconds": { "type": "integer", "description": "Stop after this many seconds (default 120, max 600)." } } })
}

fn workdir(args: &Value, sb: &Sandbox) -> Result<std::path::PathBuf, String> {
    let dir = match args.get("folder").and_then(Value::as_str).filter(|f| !f.trim().is_empty()) {
        Some(f) => sb.resolve(f)?,
        None => sb.roots().first().cloned().ok_or("No folders are shared yet.")?,
    };
    if !dir.is_dir() {
        return Err(format!("{} isn't a folder.", sb.display(&dir)));
    }
    Ok(dir)
}

pub fn preview(args: &Value, sb: &Sandbox) -> Result<Preview, String> {
    let dir = workdir(args, sb)?;
    Ok(Preview {
        title: format!("Run a command in {}", sb.display(&dir)),
        kind: "command",
        detail: Some(arg_str(args, "command")?.to_string()),
        note: Some("Commands can change things that Undo can't put back.".into()),
    })
}

/// Keeps the start and end of long output, where errors usually are.
pub fn clip(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= KEEP_CHARS * 2 {
        return s.to_string();
    }
    let head: String = chars[..KEEP_CHARS].iter().collect();
    let tail: String = chars[chars.len() - KEEP_CHARS..].iter().collect();
    format!("{head}\n… [{} characters cut] …\n{tail}", chars.len() - KEEP_CHARS * 2)
}

/// PowerShell's -EncodedCommand takes base64 of UTF-16LE, which sidesteps quoting.
fn encode(script: &str) -> String {
    let wide: Vec<u8> = script.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    B64.encode(wide)
}

pub async fn run(args: &Value, ctx: &Ctx<'_>) -> Outcome {
    let command = match arg_str(args, "command") {
        Ok(c) => c.to_string(),
        Err(e) => return Outcome::error("Run a command", e),
    };
    let dir = match workdir(args, ctx.sandbox) {
        Ok(d) => d,
        Err(e) => return Outcome::error("Run a command", e),
    };
    let timeout = Duration::from_secs(args.get("timeout_seconds").and_then(Value::as_u64).unwrap_or(DEFAULT_TIMEOUT).clamp(1, MAX_TIMEOUT));
    let title = format!("Ran a command in {}", ctx.sandbox.display(&dir));
    let script = format!(
        "$ProgressPreference='SilentlyContinue'; [Console]::OutputEncoding=[Text.Encoding]::UTF8; $OutputEncoding=[Text.Encoding]::UTF8\n{command}"
    );

    let mut cmd = tokio::process::Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &encode(&script)])
        .current_dir(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Outcome::error(title, format!("Couldn't start PowerShell: {e}")),
    };
    #[cfg(windows)]
    if let (Some(job), Some(pid)) = (ctx.job, child.id()) {
        job.assign(pid).ok();
    }

    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let read_out = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf).await;
        buf
    });
    let read_err = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf).await;
        buf
    });

    let started = Instant::now();
    let (status, stopped) = loop {
        tokio::select! {
            s = child.wait() => break (s.ok(), None),
            _ = tokio::time::sleep(Duration::from_millis(200)) => {
                let reason = if ctx.cancel.load(Ordering::Relaxed) {
                    Some("stopped")
                } else if started.elapsed() >= timeout {
                    Some("timed out")
                } else {
                    None
                };
                if let Some(r) = reason {
                    kill_tree(child.id());
                    let _ = child.kill().await;
                    break (None, Some(r));
                }
            }
        }
    };
    let out = String::from_utf8_lossy(&read_out.await.unwrap_or_default()).into_owned();
    let err = String::from_utf8_lossy(&read_err.await.unwrap_or_default()).into_owned();
    let mut combined = out.trim_end().to_string();
    if !err.trim().is_empty() {
        if !combined.is_empty() {
            combined.push_str("\n");
        }
        combined.push_str(&format!("[stderr]\n{}", err.trim_end()));
    }
    let combined = clip(&combined);
    let code = status.and_then(|s| s.code());
    let footer = match (stopped, code) {
        (Some(r), _) => format!("The command {r} after {}s.", started.elapsed().as_secs()),
        (None, Some(c)) => format!("Exit code {c}."),
        (None, None) => "The command ended without an exit code.".into(),
    };
    let text = if combined.is_empty() { format!("(no output)\n{footer}") } else { format!("{combined}\n{footer}") };
    let ok = stopped.is_none() && code == Some(0);
    let detail = format!("> {command}\n\n{text}");
    let mut outcome = Outcome::ok(text, title, "command", Some(detail));
    if !ok {
        outcome.meta["status"] = json!("error");
    }
    outcome
}

/// Ends the command and anything it started.
fn kill_tree(pid: Option<u32>) {
    #[cfg(windows)]
    if let Some(pid) = pid {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status();
    }
    #[cfg(not(windows))]
    let _ = pid;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_output_keeps_head_and_tail() {
        let s = format!("START{}END", "x".repeat(50_000));
        let c = clip(&s);
        assert!(c.starts_with("START") && c.ends_with("END") && c.contains("characters cut"));
        assert_eq!(clip("short"), "short");
    }

    #[test]
    fn encoded_command_is_utf16le_base64() {
        // "a" in UTF-16LE is 61 00.
        assert_eq!(encode("a"), "YQA=");
    }
}
