// SPDX-License-Identifier: AGPL-3.0-only
//! run_command: PowerShell (bash on macOS and Linux) in a shared folder,
//! hidden, with a time limit; or kept running in the background (a dev
//! server, a watcher), checked with command_output and ended with
//! stop_command. Background commands end with the app (their job object),
//! can't take more than half the PC's memory between them, and stop when the
//! disk gets nearly full (see guard).

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::Ordering;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;

use super::{arg_str, guard, Ctx, Outcome, Preview};
use crate::sandbox::Sandbox;

const DEFAULT_TIMEOUT: u64 = 120;
const MAX_TIMEOUT: u64 = 600;
const KEEP_CHARS: usize = 12_000;

pub fn run_command_params() -> Value {
    json!({ "type": "object", "required": ["command"], "properties": {
        "command": { "type": "string", "description": if cfg!(windows) { "PowerShell command(s) to run." } else { "bash command(s) to run." } },
        "folder": { "type": "string", "description": "Shared folder (or subfolder) to run in. Default: the first shared folder." },
        "timeout_seconds": { "type": "integer", "description": "Stop after this many seconds (default 120, max 600)." },
        "background": { "type": "boolean", "description": "Keep it running after this returns, for servers and watchers (e.g. npm run dev). Returns after a few seconds with the output so far; check it with command_output, end it with stop_command." } } })
}

pub fn command_output_params() -> Value {
    json!({ "type": "object", "required": ["id"], "properties": {
        "id": { "type": "string", "description": "The background command's id, from run_command." } } })
}

pub fn stop_command_params() -> Value {
    json!({ "type": "object", "required": ["id"], "properties": {
        "id": { "type": "string", "description": "The background command's id, from run_command." } } })
}

/// A command left running in the background.
struct Background {
    command: String,
    /// The folder it runs in (for the disk check).
    dir: std::path::PathBuf,
    child: tokio::process::Child,
    output: Arc<Mutex<Kept>>,
}

static BACKGROUND: LazyLock<Mutex<HashMap<String, Background>>> = LazyLock::new(Default::default);
/// Background commands stopped from the Servers panel or by the assistant,
/// newest last, so they can be started again: id, command, folder.
static STOPPED: LazyLock<Mutex<Vec<(String, String, std::path::PathBuf)>>> = LazyLock::new(Default::default);

/// A background command, for the Servers panel.
#[derive(serde::Serialize, Clone)]
pub struct ServerInfo {
    pub id: String,
    pub command: String,
    pub folder: String,
    pub running: bool,
    /// The shell's process id (its programs are below it).
    #[serde(skip)]
    pub pid: Option<u32>,
    /// The address it printed, if any.
    pub url: Option<String>,
}

/// Background commands, running or not, newest first.
pub fn servers() -> Vec<ServerInfo> {
    let mut out: Vec<ServerInfo> = BACKGROUND
        .lock()
        .unwrap()
        .iter_mut()
        .map(|(id, b)| {
            let running = matches!(b.child.try_wait(), Ok(None));
            ServerInfo { id: id.clone(), command: b.command.clone(), folder: b.dir.display().to_string(), running, pid: b.child.id(), url: first_url(&b.output.lock().unwrap().text()) }
        })
        .collect();
    for (id, command, dir) in STOPPED.lock().unwrap().iter().rev() {
        if !out.iter().any(|s| &s.id == id) {
            out.push(ServerInfo { id: id.clone(), command: command.clone(), folder: dir.display().to_string(), running: false, pid: None, url: None });
        }
    }
    out
}

/// Stops a background command and anything it started (Servers panel).
pub fn stop_server(id: &str) -> Result<(), String> {
    let mut b = BACKGROUND.lock().unwrap().remove(id).ok_or("That server isn't running.")?;
    kill_tree(b.child.id());
    let _ = b.child.start_kill();
    remember_stopped(id, &b.command, &b.dir);
    Ok(())
}

fn remember_stopped(id: &str, command: &str, dir: &std::path::Path) {
    let mut all = STOPPED.lock().unwrap();
    all.retain(|(i, ..)| i != id);
    all.push((id.to_string(), command.to_string(), dir.to_path_buf()));
    let extra = all.len().saturating_sub(10);
    all.drain(..extra);
}

/// Starts a server again, the same command in the same folder (Servers
/// panel). The person approved it when the assistant first ran it.
pub async fn start_server(id: &str) -> Result<ServerInfo, String> {
    let found = {
        let mut running = BACKGROUND.lock().unwrap();
        match running.get_mut(id) {
            Some(b) => {
                if matches!(b.child.try_wait(), Ok(None)) {
                    return Err("It's already running.".into());
                }
                Some((b.command.clone(), b.dir.clone()))
            }
            None => None,
        }
    }
    .or_else(|| STOPPED.lock().unwrap().iter().find(|(i, ..)| i == id).map(|(_, c, d)| (c.clone(), d.clone())));
    let (command, dir) = found.ok_or("SulcusAI doesn't know that server any more.")?;
    if !dir.is_dir() {
        return Err(format!("{} isn't there any more.", dir.display()));
    }
    if let Some(why) = guard::disk_too_full(&dir) {
        return Err(why);
    }
    let mut child = spawn_shell(&command, &dir, None)?;
    let output = Arc::new(Mutex::new(Kept::default()));
    if let Some(out) = child.stdout.take() {
        collect(out, output.clone());
    }
    if let Some(err) = child.stderr.take() {
        collect(err, output.clone());
    }
    // A moment to print its address, or to fail.
    for _ in 0..24 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if matches!(child.try_wait(), Ok(Some(_))) || first_url(&output.lock().unwrap().text()).is_some() {
            break;
        }
    }
    if let Ok(Some(s)) = child.try_wait() {
        let said = clip(output.lock().unwrap().text().trim());
        return Err(format!("It ended right away (exit code {}). {said}", s.code().map_or("none".into(), |c| c.to_string())));
    }
    STOPPED.lock().unwrap().retain(|(i, ..)| i != id);
    let info = ServerInfo { id: id.to_string(), command: command.clone(), folder: dir.display().to_string(), running: true, pid: child.id(), url: first_url(&output.lock().unwrap().text()) };
    BACKGROUND.lock().unwrap().insert(id.to_string(), Background { command, dir, child, output });
    watch_background();
    Ok(info)
}
/// The most output kept from a command: its start, and its latest part.
const KEEP_HEAD: usize = 50_000;
const KEEP_TAIL: usize = 150_000;

/// What a command printed, kept to its start and its latest part, so one
/// that prints without end can't fill the PC's memory.
#[derive(Default)]
struct Kept {
    head: String,
    tail: String,
    cut: usize,
}

impl Kept {
    fn push(&mut self, s: &str) {
        let mut s = s;
        if self.cut == 0 && self.tail.is_empty() && self.head.len() < KEEP_HEAD {
            let room = KEEP_HEAD - self.head.len();
            let at = if s.len() <= room { s.len() } else { (0..=room).rev().find(|i| s.is_char_boundary(*i)).unwrap_or(0) };
            self.head.push_str(&s[..at]);
            s = &s[at..];
        }
        self.tail.push_str(s);
        if self.tail.len() > KEEP_TAIL {
            let from = self.tail.len() - KEEP_TAIL / 2;
            let at = (from..self.tail.len()).find(|i| self.tail.is_char_boundary(*i)).unwrap_or(self.tail.len());
            self.cut += self.tail[..at].chars().count();
            self.tail.drain(..at);
        }
    }

    fn text(&self) -> String {
        if self.cut == 0 {
            format!("{}{}", self.head, self.tail)
        } else {
            format!("{}\n… [{} characters cut] …\n{}", self.head, self.cut, self.tail)
        }
    }
}

/// Reads a stream into `out` as it comes; the task ends with the stream.
fn collect(mut r: impl tokio::io::AsyncRead + Unpin + Send + 'static, out: Arc<Mutex<Kept>>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut buf = [0u8; 8192];
        while let Ok(n) = r.read(&mut buf).await {
            if n == 0 {
                break;
            }
            out.lock().unwrap().push(&String::from_utf8_lossy(&buf[..n]));
        }
    })
}

/// The job the assistant's commands run in (Windows): half the PC's memory
/// and 256 processes between them, at below-normal priority.
#[cfg(windows)]
fn commands_job() -> Option<&'static crate::winjob::Job> {
    static JOB: LazyLock<Option<crate::winjob::Job>> = LazyLock::new(|| {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        let half = (sys.total_memory() / 2).max(2 * 1024 * 1024 * 1024);
        crate::winjob::Job::limited(half, 256).ok()
    });
    JOB.as_ref()
}

/// While background commands run: stops them all if the disk gets nearly
/// full. Started with the first one.
fn watch_background() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        tokio::spawn(async {
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;
                let dirs: Vec<std::path::PathBuf> =
                    BACKGROUND.lock().unwrap().values_mut().filter_map(|b| matches!(b.child.try_wait(), Ok(None)).then(|| b.dir.clone())).collect();
                let Some(free) = dirs.iter().filter_map(|d| guard::least_free(d)).min() else { continue };
                if free >= guard::STOP_FREE {
                    continue;
                }
                // Kept in the list, so command_output says why it ended.
                for b in BACKGROUND.lock().unwrap().values_mut() {
                    if matches!(b.child.try_wait(), Ok(None)) {
                        kill_tree(b.child.id());
                        let _ = b.child.start_kill();
                        b.output.lock().unwrap().push(DISK_FULL_STOP);
                    }
                }
            }
        });
    });
}

const DISK_FULL_STOP: &str = "\n[SulcusAI stopped this: the disk was nearly full. Tell the user, so they can make room.]";

/// Whether a background command is still running.
pub fn running() -> bool {
    BACKGROUND.lock().unwrap().values_mut().any(|b| matches!(b.child.try_wait(), Ok(None)))
}

/// The first web address a server printed, if any ("http://localhost:5173").
pub fn first_url(text: &str) -> Option<String> {
    let start = text.find("http://").or_else(|| text.find("https://"))?;
    let url: String = text[start..].chars().take_while(|c| !c.is_whitespace() && !"\"'<>()[]".contains(*c)).collect();
    let url = url.trim_end_matches(['/', '.', ',', ';']).to_string();
    (url.len() > "https://".len()).then_some(url)
}

/// command_output: what a background command has printed, and whether it's still running.
pub fn output(args: &Value) -> Outcome {
    let id = arg_str(args, "id").unwrap_or_default().to_string();
    let mut all = BACKGROUND.lock().unwrap();
    let Some(b) = all.get_mut(&id) else {
        return Outcome::error("Check a background command", format!("There's no background command {id}. It may have been stopped."));
    };
    let ended = b.child.try_wait().ok().flatten();
    let so_far = clip(b.output.lock().unwrap().text().trim_end());
    let state = match ended {
        None => "Still running.".to_string(),
        Some(s) => format!("It ended (exit code {}).", s.code().map_or("none".into(), |c| c.to_string())),
    };
    let text = format!("{}\n{state}", if so_far.is_empty() { "(no output yet)" } else { &so_far });
    Outcome::ok(text.clone(), format!("Checked on: {}", short(&b.command)), "command", Some(text))
}

/// stop_command: ends a background command and anything it started.
pub fn stop(args: &Value) -> Outcome {
    let id = arg_str(args, "id").unwrap_or_default().to_string();
    let Some(mut b) = BACKGROUND.lock().unwrap().remove(&id) else {
        return Outcome::error("Stop a background command", format!("There's no background command {id}."));
    };
    kill_tree(b.child.id());
    let _ = b.child.start_kill();
    remember_stopped(&id, &b.command, &b.dir);
    Outcome::ok(format!("Stopped {id}."), format!("Stopped: {}", short(&b.command)), "text", None)
}

fn short(command: &str) -> String {
    let first = command.lines().next().unwrap_or_default();
    let s: String = first.chars().take(60).collect();
    if s.len() < command.len() { format!("{s}…") } else { s }
}

/// A command that keeps running: waits a few seconds (less once it prints
/// a web address, or ends), then leaves it running and says how to follow it.
async fn keep_running(mut child: tokio::process::Child, command: String, dir: std::path::PathBuf, title: String, ctx: &Ctx<'_>) -> Outcome {
    let output = Arc::new(Mutex::new(Kept::default()));
    if let Some(out) = child.stdout.take() {
        collect(out, output.clone());
    }
    if let Some(err) = child.stderr.take() {
        collect(err, output.clone());
    }
    let started = Instant::now();
    let mut ended = None;
    while started.elapsed() < Duration::from_secs(15) && !ctx.cancel.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if let Ok(Some(s)) = child.try_wait() {
            ended = Some(s);
            break;
        }
        // A server that printed its address is up (a moment more for its last lines).
        if started.elapsed() > Duration::from_secs(2) && first_url(&output.lock().unwrap().text()).is_some() {
            tokio::time::sleep(Duration::from_millis(500)).await;
            break;
        }
    }
    let so_far = clip(output.lock().unwrap().text().trim_end());
    if let Some(s) = ended {
        let code = s.code().map_or("none".into(), |c| c.to_string());
        let text = format!("{}\nIt ended right away (exit code {code}), so it isn't running.", if so_far.is_empty() { "(no output)" } else { &so_far });
        let mut o = Outcome::ok(text.clone(), title, "command", Some(format!("> {command}\n\n{text}")));
        if s.code() != Some(0) {
            o.meta["status"] = json!("error");
        }
        return o;
    }
    let id = format!("bg{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    let url = first_url(&so_far);
    let text = format!(
        "Running in the background as {id} (it keeps going after this).{}\nOutput so far:\n{}\nCheck on it with command_output, stop it with stop_command.",
        url.as_ref().map(|u| format!(" It's at {u}: the user can preview it in SulcusAI's browser from this step (or you can open it there with browser_open).")).unwrap_or_default(),
        if so_far.is_empty() { "(none yet)" } else { &so_far }
    );
    BACKGROUND.lock().unwrap().insert(id.clone(), Background { command: command.clone(), dir, child, output });
    watch_background();
    let mut o = Outcome::ok(text.clone(), format!("Started in the background: {}", short(&command)), "command", Some(format!("> {command}\n\n{text}")));
    // The step offers "Preview" for a server on this PC.
    if let Some(port) = url.as_deref().and_then(crate::ports::local_port) {
        o.meta["preview_port"] = json!(port);
        o.meta["server"] = json!(id);
    }
    o
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

/// The folder to run in, or why the command can't run (see guard).
fn checked(args: &Value, sb: &Sandbox) -> Result<std::path::PathBuf, String> {
    let dir = workdir(args, sb)?;
    let command = arg_str(args, "command")?;
    if let Some(why) = guard::refuse(command, &dir, sb).or_else(|| guard::disk_too_full(&dir)) {
        return Err(why);
    }
    Ok(dir)
}

pub fn preview(args: &Value, sb: &Sandbox) -> Result<Preview, String> {
    let dir = checked(args, sb)?;
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
    let dir = match checked(args, ctx.sandbox) {
        Ok(d) => d,
        Err(e) => return Outcome::error("Run a command", e),
    };
    let timeout = Duration::from_secs(args.get("timeout_seconds").and_then(Value::as_u64).unwrap_or(DEFAULT_TIMEOUT).clamp(1, MAX_TIMEOUT));
    let title = format!("Ran a command in {}", ctx.sandbox.display(&dir));
    let child = match spawn_shell(&command, &dir, ctx.job) {
        Ok(c) => c,
        Err(e) => return Outcome::error(title, e),
    };
    if args.get("background").and_then(Value::as_bool) == Some(true) {
        return keep_running(child, command, dir, title, ctx).await;
    }
    run_to_end(child, &command, &dir, timeout, title, ctx).await
}

/// Starts the shell running `command` in `dir`, in its own limited job.
fn spawn_shell(command: &str, dir: &std::path::Path, fallback_job: Option<&crate::engine::JobRef>) -> Result<tokio::process::Child, String> {
    let script = format!(
        "$ProgressPreference='SilentlyContinue'; [Console]::OutputEncoding=[Text.Encoding]::UTF8; $OutputEncoding=[Text.Encoding]::UTF8\n{command}"
    );

    let mut cmd = if cfg!(windows) {
        let mut c = tokio::process::Command::new("powershell.exe");
        c.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &encode(&script)]);
        c
    } else {
        let _ = &script;
        let mut c = tokio::process::Command::new(if std::path::Path::new("/bin/bash").exists() { "/bin/bash" } else { "/bin/sh" });
        c.arg("-c").arg(&command).env("LC_ALL", "C.UTF-8");
        c
    };
    cmd.current_dir(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    crate::engine::tie_to_app(&mut cmd);
    let child = cmd.spawn().map_err(|e| format!("Couldn't start the shell: {e}"))?;
    // Its own limited job (memory, processes, priority); the app's if that failed.
    #[cfg(windows)]
    if let (Some(job), Some(pid)) = (commands_job().or(fallback_job), child.id()) {
        job.assign(pid).ok();
    }
    #[cfg(not(windows))]
    let _ = fallback_job;
    Ok(child)
}

/// A command run to its end (or its time limit), with its output.
async fn run_to_end(mut child: tokio::process::Child, command: &str, dir: &std::path::Path, timeout: Duration, title: String, ctx: &Ctx<'_>) -> Outcome {
    let command = command.to_string();
    let dir = dir.to_path_buf();
    let (out, err) = (Arc::new(Mutex::new(Kept::default())), Arc::new(Mutex::new(Kept::default())));
    let read_out = collect(child.stdout.take().unwrap(), out.clone());
    let read_err = collect(child.stderr.take().unwrap(), err.clone());

    let started = Instant::now();
    let mut checked_disk = Instant::now();
    let (status, stopped) = loop {
        tokio::select! {
            s = child.wait() => break (s.ok(), None),
            _ = tokio::time::sleep(Duration::from_millis(200)) => {
                let mut reason = if ctx.cancel.load(Ordering::Relaxed) {
                    Some("was stopped")
                } else if started.elapsed() >= timeout {
                    Some("timed out")
                } else {
                    None
                };
                if reason.is_none() && checked_disk.elapsed() >= Duration::from_secs(5) {
                    checked_disk = Instant::now();
                    if guard::least_free(&dir).is_some_and(|f| f < guard::STOP_FREE) {
                        reason = Some("was stopped by SulcusAI because the disk was nearly full (tell the user, so they can make room)");
                    }
                }
                if let Some(r) = reason {
                    kill_tree(child.id());
                    let _ = child.kill().await;
                    break (None, Some(r));
                }
            }
        }
    };
    // What's left to read; something it started may hold the output open.
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        let _ = read_out.await;
        let _ = read_err.await;
    })
    .await;
    let out = out.lock().unwrap().text();
    let err = err.lock().unwrap().text();
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

    /// A command that prints without end keeps a bounded amount in memory.
    #[test]
    fn endless_output_stays_small() {
        let mut k = Kept::default();
        k.push("START\n");
        for i in 0..20_000 {
            k.push(&format!("line {i}: {}\n", "é".repeat(40)));
        }
        k.push("END");
        let t = k.text();
        assert!(t.len() <= KEEP_HEAD + KEEP_TAIL + 100, "{}", t.len());
        assert!(t.starts_with("START") && t.ends_with("END") && t.contains("characters cut"));
        let mut small = Kept::default();
        small.push("just this");
        assert_eq!(small.text(), "just this");
    }

    #[test]
    fn long_output_keeps_head_and_tail() {
        let s = format!("START{}END", "x".repeat(50_000));
        let c = clip(&s);
        assert!(c.starts_with("START") && c.ends_with("END") && c.contains("characters cut"));
        assert_eq!(clip("short"), "short");
    }

    #[test]
    fn a_server_address_is_found_in_its_output() {
        assert_eq!(first_url("  VITE v5  ready\n  ➜  Local:   http://localhost:5173/\n").as_deref(), Some("http://localhost:5173"));
        assert_eq!(first_url("Serving HTTP on 0.0.0.0 port 8000 (http://0.0.0.0:8000/) ...").as_deref(), Some("http://0.0.0.0:8000"));
        assert_eq!(first_url("no address here"), None);
    }

    #[test]
    fn encoded_command_is_utf16le_base64() {
        // "a" in UTF-16LE is 61 00.
        assert_eq!(encode("a"), "YQA=");
    }
}
