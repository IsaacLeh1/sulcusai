// SPDX-License-Identifier: AGPL-3.0-only
//! Model Context Protocol client: for local servers it starts the program
//! as a hidden process and speaks JSON-RPC over its stdin/stdout; for
//! servers at an address it speaks Streamable HTTP (each message POSTed,
//! the answer as JSON or as server-sent events, with the session id the
//! server gives). Either way it lists the server's tools and calls them.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::oneshot;

use crate::engine::JobRef;

const PROTOCOL: &str = "2025-06-18";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_RESULT_CHARS: usize = 40_000;

/// How to reach a server: a program to start (`command`), or an address
/// (`url`, with `headers` such as a key, as other apps write them).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ServerSpec {
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,
}

impl ServerSpec {
    pub fn remote(&self) -> bool {
        !self.url.trim().is_empty()
    }
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

/// A server's program running on this PC.
struct Local {
    child: Child,
    stdin: ChildStdin,
    pending: Pending,
    stderr_tail: Arc<Mutex<Vec<String>>>,
}

/// A server at an address.
struct Remote {
    http: reqwest::Client,
    url: String,
    headers: HashMap<String, String>,
    /// Given by the server when the session starts; sent with every message after.
    session: Option<String>,
    /// The protocol version agreed on (sent after the handshake).
    protocol: Option<String>,
}

enum Link {
    Local(Local),
    Remote(Remote),
}

pub struct Client {
    link: Link,
    next_id: AtomicU64,
    pub server_name: String,
}

/// Whether an address is this PC (reached directly, not through the web rules).
fn on_this_pc(url: &str) -> bool {
    reqwest::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_ascii_lowercase)).is_some_and(|h| h == "localhost" || h == "127.0.0.1" || h == "[::1]" || h == "::1")
}

/// A JSON-RPC answer's result, or its error.
fn answer(msg: &Value) -> Result<Value, String> {
    match msg.get("error") {
        Some(e) => Err(e.get("message").and_then(Value::as_str).unwrap_or("The connector reported an error.").to_string()),
        None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
    }
}

impl Client {
    /// Starts the server (or reaches it) and completes the MCP handshake.
    /// `level`: what may go online, for servers at an address.
    pub async fn start(spec: &ServerSpec, job: Option<&JobRef>, level: crate::net::Connectivity) -> Result<Client, String> {
        let link = if spec.remote() {
            let url = spec.url.trim().to_string();
            if !url.starts_with("https://") && !(url.starts_with("http://") && on_this_pc(&url)) {
                return Err("A connector's address needs https:// (plain http only for this PC).".into());
            }
            let http = if on_this_pc(&url) { crate::net::local_client() } else { crate::net::service_client(level)? };
            Link::Remote(Remote { http, url, headers: spec.headers.clone(), session: None, protocol: None })
        } else {
            Link::Local(Self::spawn(spec, job)?)
        };
        let mut client = Client { link, next_id: AtomicU64::new(1), server_name: String::new() };
        let init = client
            .request(
                "initialize",
                json!({ "protocolVersion": PROTOCOL, "capabilities": {}, "clientInfo": { "name": "SulcusAI", "version": env!("CARGO_PKG_VERSION") } }),
            )
            .await
            .map_err(|e| client.explain(&e))?;
        client.server_name = init.pointer("/serverInfo/name").and_then(Value::as_str).unwrap_or("").to_string();
        if let Link::Remote(r) = &mut client.link {
            r.protocol = Some(init.get("protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL).to_string());
        }
        client.notify("notifications/initialized", json!({})).await?;
        Ok(client)
    }

    fn spawn(spec: &ServerSpec, job: Option<&JobRef>) -> Result<Local, String> {
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
        crate::engine::tie_to_app(&mut cmd);
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
                    let _ = tx.send(answer(&msg));
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
        Ok(Local { child, stdin, pending, stderr_tail })
    }

    fn explain(&self, e: &str) -> String {
        let Link::Local(l) = &self.link else { return e.to_string() };
        let tail = l.stderr_tail.lock().unwrap().join("\n");
        if tail.trim().is_empty() {
            e.to_string()
        } else {
            format!("{e}\n{tail}")
        }
    }

    async fn send(stdin: &mut ChildStdin, msg: Value) -> Result<(), String> {
        let mut line = msg.to_string();
        line.push('\n');
        stdin.write_all(line.as_bytes()).await.map_err(|_| "The connector isn't running.".to_string())?;
        stdin.flush().await.map_err(|e| e.to_string())
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        let msg = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        match &mut self.link {
            Link::Local(l) => Self::send(&mut l.stdin, msg).await,
            Link::Remote(r) => r.post(&msg, None).await.map(|_| ()),
        }
    }

    pub async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let l = match &mut self.link {
            Link::Remote(r) => {
                return match tokio::time::timeout(REQUEST_TIMEOUT, r.post(&msg, Some(id))).await {
                    Ok(Ok(Some(reply))) => answer(&reply),
                    Ok(Ok(None)) => Err("The connector didn't answer.".into()),
                    Ok(Err(e)) => Err(e),
                    Err(_) => Err("The connector didn't answer in time.".into()),
                };
            }
            Link::Local(l) => l,
        };
        let (tx, rx) = oneshot::channel();
        l.pending.lock().unwrap().insert(id, tx);
        Self::send(&mut l.stdin, msg).await?;
        match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err("The connector stopped.".into()),
            Err(_) => {
                l.pending.lock().unwrap().remove(&id);
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
        match &mut self.link {
            Link::Local(l) => matches!(l.child.try_wait(), Ok(None)),
            // Each message is its own request; a gone session shows on the next one.
            Link::Remote(_) => true,
        }
    }

    pub async fn stop(self) {
        match self.link {
            Link::Local(mut l) => {
                let _ = l.child.start_kill();
                let _ = tokio::time::timeout(Duration::from_secs(3), l.child.wait()).await;
            }
            // The session ends (servers that keep one let it go).
            Link::Remote(r) => {
                if let Some(s) = &r.session {
                    let mut req = r.http.delete(&r.url).header("Mcp-Session-Id", s);
                    for (k, v) in &r.headers {
                        req = req.header(k, v);
                    }
                    let _ = tokio::time::timeout(Duration::from_secs(3), req.send()).await;
                }
            }
        }
    }
}

impl Remote {
    /// Posts one message. For a request (`id`), the answer: the JSON body,
    /// or the event in the stream that answers it.
    async fn post(&mut self, msg: &Value, id: Option<u64>) -> Result<Option<Value>, String> {
        crate::offline::check(crate::offline::Keep::Connectors)?;
        let mut req = self.http.post(&self.url).header("Accept", "application/json, text/event-stream").json(msg);
        for (k, v) in &self.headers {
            req = req.header(k, v);
        }
        if let Some(s) = &self.session {
            req = req.header("Mcp-Session-Id", s);
        }
        if let Some(p) = &self.protocol {
            req = req.header("MCP-Protocol-Version", p);
        }
        let resp = req.send().await.map_err(|e| format!("Couldn't reach the connector at {}: {e}", crate::host_of(&self.url)))?;
        if let Some(s) = resp.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()) {
            self.session = Some(s.to_string());
        }
        let status = resp.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(format!("The connector at {} turned SulcusAI away ({status}). It may need a key or token in its headers.", crate::host_of(&self.url)));
        }
        if status.as_u16() == 404 && self.session.is_some() && id.is_some() {
            return Err("The connector's session ended. Try again.".into());
        }
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("The connector answered {status}: {}", text.chars().take(300).collect::<String>()));
        }
        let Some(id) = id else { return Ok(None) };
        let events = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).is_some_and(|t| t.starts_with("text/event-stream"));
        if !events {
            let body: Value = resp.json().await.map_err(|e| format!("The connector's answer isn't JSON: {e}"))?;
            // A batch answer, or one message.
            let one = match body {
                Value::Array(all) => all.into_iter().find(|m| m.get("id").and_then(Value::as_u64) == Some(id)),
                m => Some(m),
            };
            return Ok(one);
        }
        // Events until the one answering this request (others, like
        // progress notes or the server's own requests, are passed over).
        let mut stream = resp.bytes_stream();
        let (mut buf, mut data) = (String::new(), String::new());
        while let Some(chunk) = stream.next().await {
            buf.push_str(&String::from_utf8_lossy(&chunk.map_err(|e| e.to_string())?));
            while let Some(at) = buf.find('\n') {
                let line: String = buf.drain(..=at).collect();
                let line = line.trim_end_matches(['\r', '\n']);
                if let Some(d) = line.strip_prefix("data:") {
                    data.push_str(d.trim_start());
                } else if line.is_empty() && !data.is_empty() {
                    let event = std::mem::take(&mut data);
                    if let Ok(m) = serde_json::from_str::<Value>(&event) {
                        if m.get("id").and_then(Value::as_u64) == Some(id) && m.get("method").is_none() {
                            return Ok(Some(m));
                        }
                    }
                }
            }
        }
        Ok(None)
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
        let mut c = match Client::start(&echo_spec(), None, crate::net::Connectivity::Offline).await {
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

    /// A server at an address (Streamable HTTP): the session id it gives
    /// comes back on every message, the key goes in the headers, and
    /// answers arrive as JSON or as events (after a progress note).
    #[tokio::test]
    async fn talks_to_a_server_at_an_address() {
        use axum::http::{HeaderMap, StatusCode};
        use axum::response::IntoResponse;
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let log = seen.clone();
        let app = axum::Router::new().route(
            "/mcp",
            axum::routing::post(move |h: HeaderMap, axum::Json(m): axum::Json<Value>| {
                let log = log.clone();
                async move {
                    if h.get("authorization").and_then(|v| v.to_str().ok()) != Some("Bearer t0ken") {
                        return StatusCode::UNAUTHORIZED.into_response();
                    }
                    let method = m["method"].as_str().unwrap_or("").to_string();
                    let session = h.get("mcp-session-id").and_then(|v| v.to_str().ok()).unwrap_or("-").to_string();
                    log.lock().unwrap().push(format!("{method} {session}"));
                    let reply = |r: Value| json!({ "jsonrpc": "2.0", "id": m["id"], "result": r });
                    match method.as_str() {
                        "initialize" => ([("mcp-session-id", "s1")], axum::Json(reply(json!({ "protocolVersion": "2025-06-18", "serverInfo": { "name": "web-test" } })))).into_response(),
                        "notifications/initialized" => StatusCode::ACCEPTED.into_response(),
                        "tools/list" => {
                            let note = json!({ "jsonrpc": "2.0", "method": "notifications/progress", "params": {} });
                            let body = format!("event: message\ndata: {note}\n\ndata: {}\n\n", reply(json!({ "tools": [{ "name": "add", "inputSchema": { "type": "object" } }] })));
                            ([("content-type", "text/event-stream")], body).into_response()
                        }
                        "tools/call" => axum::Json(reply(json!({ "content": [{ "type": "text", "text": format!("{}", m["params"]["arguments"]["a"].as_i64().unwrap_or(0) + 2) }] }))).into_response(),
                        _ => StatusCode::NOT_FOUND.into_response(),
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await.ok() });
        let mut spec = ServerSpec { url: format!("http://127.0.0.1:{port}/mcp"), ..Default::default() };
        assert!(Client::start(&spec, None, crate::net::Connectivity::Offline).await.err().unwrap().contains("key or token"));
        spec.headers.insert("Authorization".into(), "Bearer t0ken".into());
        let mut c = Client::start(&spec, None, crate::net::Connectivity::Offline).await.unwrap();
        assert_eq!(c.server_name, "web-test");
        let tools = c.list_tools().await.unwrap();
        assert_eq!(tools[0].name, "add");
        assert_eq!(c.call("add", json!({ "a": 40 })).await.unwrap(), (false, "42".to_string()));
        assert_eq!(*seen.lock().unwrap(), ["initialize -", "notifications/initialized s1", "tools/list s1", "tools/call s1"]);
        // Plain http only for this PC.
        let far = ServerSpec { url: "http://example.com/mcp".into(), ..Default::default() };
        assert!(Client::start(&far, None, crate::net::Connectivity::Cloud).await.err().unwrap().contains("https://"));
        c.stop().await;
    }

    #[tokio::test]
    async fn a_missing_program_gives_a_clear_error() {
        let spec = ServerSpec { command: "definitely-not-a-real-program-xyz".into(), ..Default::default() };
        assert!(Client::start(&spec, None, crate::net::Connectivity::Offline).await.err().unwrap().contains("Couldn't start"));
    }
}
