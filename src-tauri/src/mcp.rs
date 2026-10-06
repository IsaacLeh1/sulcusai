// SPDX-License-Identifier: AGPL-3.0-only
//! Model Context Protocol client for local (stdio) servers: starts the
//! server as a hidden process, speaks JSON-RPC over its stdin/stdout, lists
//! its tools and calls them.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::oneshot;

use crate::engine::JobRef;

const PROTOCOL: &str = "2025-06-18";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_RESULT_CHARS: usize = 40_000;

/// How to start a local server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ServerSpec {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "empty_schema", rename = "inputSchema")]
    pub input_schema: Value,
    /// The server says the tool doesn't change anything.
    #[serde(default)]
    pub read_only: bool,
}

fn empty_schema() -> Value {
    json!({ "type": "object", "properties": {} })
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

pub struct Client {
    child: Child,
    stdin: ChildStdin,
    pending: Pending,
    next_id: AtomicU64,
    pub server_name: String,
    stderr_tail: Arc<Mutex<Vec<String>>>,
}

impl Client {
    /// Starts the server and completes the MCP handshake.
    pub async fn start(spec: &ServerSpec, job: Option<&JobRef>) -> Result<Client, String> {
        if spec.command.trim().is_empty() {
            return Err("The connector has no command to run.".into());
        }
        let mut cmd = Command::new(&spec.command);
        cmd.args(&spec.args)
            .envs(&spec.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(dir) = spec.cwd.as_deref().filter(|d| !d.is_empty()) {
            cmd.current_dir(dir);
        }
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn().map_err(|e| format!("Couldn't start {}: {e}", spec.command))?;
        #[cfg(windows)]
        if let (Some(job), Some(pid)) = (job, child.id()) {
            job.assign(pid).ok();
        }
        #[cfg(not(windows))]
        let _ = job;

        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stderr = child.stderr.take().ok_or("no stderr")?;
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let stderr_tail = Arc::new(Mutex::new(Vec::new()));

        // Responses arrive on stdout, one JSON message per line.
        let p = pending.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
                let Some(id) = msg.get("id").and_then(Value::as_u64) else { continue };
                if msg.get("method").is_some() {
                    continue; // a request from the server; not supported
                }
                if let Some(tx) = p.lock().unwrap().remove(&id) {
                    let result = match msg.get("error") {
                        Some(e) => Err(e.get("message").and_then(Value::as_str).unwrap_or("The connector reported an error.").to_string()),
                        None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                    };
                    let _ = tx.send(result);
                }
            }
            // The server exited: fail anything still waiting.
            for (_, tx) in p.lock().unwrap().drain() {
                let _ = tx.send(Err("The connector stopped.".into()));
            }
        });
        let tail = stderr_tail.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut t = tail.lock().unwrap();
                t.push(line);
                if t.len() > 20 {
                    t.remove(0);
                }
            }
        });

        let mut client = Client { child, stdin, pending, next_id: AtomicU64::new(1), server_name: String::new(), stderr_tail };
        let init = client
            .request(
                "initialize",
                json!({ "protocolVersion": PROTOCOL, "capabilities": {}, "clientInfo": { "name": "SulcusAI", "version": env!("CARGO_PKG_VERSION") } }),
            )
            .await
            .map_err(|e| client.explain(&e))?;
        client.server_name = init.pointer("/serverInfo/name").and_then(Value::as_str).unwrap_or("").to_string();
        client.notify("notifications/initialized", json!({})).await?;
        Ok(client)
    }

    fn explain(&self, e: &str) -> String {
        let tail = self.stderr_tail.lock().unwrap().join("\n");
        if tail.trim().is_empty() {
            e.to_string()
        } else {
            format!("{e}\n{tail}")
        }
    }

    async fn send(&mut self, msg: Value) -> Result<(), String> {
        let mut line = msg.to_string();
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await.map_err(|_| "The connector isn't running.".to_string())?;
        self.stdin.flush().await.map_err(|e| e.to_string())
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.send(json!({ "jsonrpc": "2.0", "method": method, "params": params })).await
    }

    pub async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })).await?;
        match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err("The connector stopped.".into()),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err("The connector didn't answer in time.".into())
            }
        }
    }

    pub async fn list_tools(&mut self) -> Result<Vec<McpTool>, String> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..20 {
            let params = match &cursor {
                Some(c) => json!({ "cursor": c }),
                None => json!({}),
            };
            let r = self.request("tools/list", params).await?;
            for t in r.get("tools").and_then(Value::as_array).into_iter().flatten() {
                if let Ok(mut tool) = serde_json::from_value::<McpTool>(t.clone()) {
                    tool.read_only = t.pointer("/annotations/readOnlyHint").and_then(Value::as_bool).unwrap_or(false);
                    out.push(tool);
                }
            }
            cursor = r.get("nextCursor").and_then(Value::as_str).map(str::to_string);
            if cursor.is_none() {
                break;
            }
        }
        Ok(out)
    }

    /// Calls a tool and flattens its result to text for the model.
    pub async fn call(&mut self, name: &str, args: Value) -> Result<(bool, String), String> {
        let r = self.request("tools/call", json!({ "name": name, "arguments": args })).await?;
        let is_error = r.get("isError").and_then(Value::as_bool).unwrap_or(false);
        let mut text = String::new();
        for part in r.get("content").and_then(Value::as_array).into_iter().flatten() {
            match part.get("type").and_then(Value::as_str) {
                Some("text") => text.push_str(part.get("text").and_then(Value::as_str).unwrap_or("")),
                Some("resource") => text.push_str(part.pointer("/resource/text").and_then(Value::as_str).unwrap_or("[resource]")),
                Some(other) => text.push_str(&format!("[{other} content not shown]")),
                None => {}
            }
            text.push('\n');
        }
        if text.trim().is_empty() {
            if let Some(s) = r.get("structuredContent") {
                text = s.to_string();
            }
        }
        if text.chars().count() > MAX_RESULT_CHARS {
            text = text.chars().take(MAX_RESULT_CHARS).collect::<String>() + "\n… (cut short)";
        }
        Ok((is_error, text.trim_end().to_string()))
    }

    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub async fn stop(mut self) {
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(3), self.child.wait()).await;
    }
}

/// Model-facing tool name: `mcp__server__tool`, limited to the characters
/// and length OpenAI-style tool names allow.
pub fn tool_name(server: &str, tool: &str) -> String {
    let clean = |s: &str| -> String {
        s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect()
    };
    let mut name = format!("mcp__{}__{}", clean(server), clean(tool));
    name.truncate(64);
    name
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A minimal MCP server in Python, for testing the client end to end.
    pub const ECHO_SERVER: &str = r#"
import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    if "id" not in msg:
        continue
    m = msg["method"]
    if m == "initialize":
        r = {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}, "serverInfo": {"name": "echo-test", "version": "1"}}
    elif m == "tools/list":
        r = {"tools": [
            {"name": "echo", "description": "Echo text back", "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}, "annotations": {"readOnlyHint": True}},
            {"name": "shout", "description": "Upper-case text", "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}}]}
    elif m == "tools/call":
        a = msg["params"]["arguments"]
        if msg["params"]["name"] == "echo":
            r = {"content": [{"type": "text", "text": "echo: " + a.get("text", "")}]}
        else:
            r = {"content": [{"type": "text", "text": a.get("text", "").upper()}], "isError": False}
    else:
        sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": msg["id"], "error": {"code": -32601, "message": "no such method"}}) + "\n")
        sys.stdout.flush()
        continue
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": msg["id"], "result": r}) + "\n")
    sys.stdout.flush()
"#;

    pub fn echo_spec() -> ServerSpec {
        let dir = std::env::temp_dir().join(format!("sulcusai-mcp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("echo_server.py");
        std::fs::write(&script, ECHO_SERVER).unwrap();
        ServerSpec { command: "python".into(), args: vec![script.display().to_string()], ..Default::default() }
    }

    #[tokio::test]
    async fn talks_to_a_real_stdio_server() {
        let mut c = match Client::start(&echo_spec(), None).await {
            Ok(c) => c,
            Err(e) if e.contains("Couldn't start python") => return, // no Python on this machine
            Err(e) => panic!("{e}"),
        };
        assert_eq!(c.server_name, "echo-test");
        let tools = c.list_tools().await.unwrap();
        assert_eq!(tools.len(), 2);
        assert!(tools[0].read_only && !tools[1].read_only);
        assert_eq!(c.call("echo", json!({ "text": "hi" })).await.unwrap(), (false, "echo: hi".to_string()));
        assert_eq!(c.call("shout", json!({ "text": "hi" })).await.unwrap().1, "HI");
        assert!(c.request("nope", json!({})).await.unwrap_err().contains("no such method"));
        assert!(c.alive());
        c.stop().await;
    }

    #[test]
    fn tool_names_are_safe_for_the_model() {
        assert_eq!(tool_name("my files", "read.file"), "mcp__my_files__read_file");
        assert!(tool_name(&"x".repeat(80), "y").len() <= 64);
    }

    #[tokio::test]
    async fn a_missing_program_gives_a_clear_error() {
        let spec = ServerSpec { command: "definitely-not-a-real-program-xyz".into(), ..Default::default() };
        assert!(Client::start(&spec, None).await.err().unwrap().contains("Couldn't start"));
    }
}
