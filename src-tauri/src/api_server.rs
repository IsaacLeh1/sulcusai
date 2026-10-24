// SPDX-License-Identifier: AGPL-3.0-only
//! The opt-in local API server: OpenAI-compatible, for other programs on
//! this PC (editors, scripts, other chat apps). It listens on 127.0.0.1
//! only, every request needs the app's key, and requests whose Host isn't
//! this PC are refused, so a web page can't reach it either.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::oneshot;

use crate::{db, net, AppState, AppStateRef};

const KEY: &str = "api_server";
pub const DEFAULT_PORT: u16 = 7340;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ApiSettings {
    pub enabled: bool,
    pub port: u16,
    pub key: String,
}

impl Default for ApiSettings {
    fn default() -> Self {
        ApiSettings { enabled: false, port: DEFAULT_PORT, key: String::new() }
    }
}

fn new_key() -> String {
    format!("sk-sulcus-{}", uuid::Uuid::new_v4().simple())
}

/// The settings, with a key made on first use.
fn settings(conn: &Connection) -> ApiSettings {
    let mut s: ApiSettings = db::get(conn, KEY).unwrap_or_default();
    s.key = crate::crypto::local_open(&s.key);
    if s.key.is_empty() {
        s.key = new_key();
        let _ = save(conn, &s);
    }
    s
}

/// Saves the settings with the key sealed to this Windows account.
fn save(conn: &Connection, s: &ApiSettings) -> Result<(), String> {
    db::set(conn, KEY, &ApiSettings { key: crate::crypto::local_seal(&s.key), ..s.clone() })
}

struct Running {
    port: u16,
    stop: oneshot::Sender<()>,
}

static SERVER: Mutex<Option<Running>> = Mutex::new(None);
static PROBLEM: Mutex<Option<String>> = Mutex::new(None);

#[derive(Clone)]
struct Ctx {
    state: Arc<AppState>,
    key: String,
    port: u16,
}

/// Starts, restarts or stops the server to match the settings.
pub fn apply(state: Arc<AppState>) {
    let s = settings(&state.db.lock().unwrap());
    if let Some(r) = SERVER.lock().unwrap().take() {
        let _ = r.stop.send(());
    }
    *PROBLEM.lock().unwrap() = None;
    if !s.enabled {
        return;
    }
    let (tx, rx) = oneshot::channel();
    *SERVER.lock().unwrap() = Some(Running { port: s.port, stop: tx });
    let ctx = Ctx { state, key: s.key, port: s.port };
    tauri::async_runtime::spawn(async move {
        // The old server lets go of the port as it shuts down.
        let mut listener = None;
        for _ in 0..20 {
            match tokio::net::TcpListener::bind(("127.0.0.1", ctx.port)).await {
                Ok(l) => {
                    listener = Some(l);
                    break;
                }
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
            }
        }
        let Some(listener) = listener else {
            *PROBLEM.lock().unwrap() = Some(format!("Port {} is in use by another program. Pick another port.", ctx.port));
            return;
        };
        let app = router(ctx);
        let _ = axum::serve(listener, app).with_graceful_shutdown(async { rx.await.ok(); }).await;
    });
}

fn router(ctx: Ctx) -> Router {
    Router::new()
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(chat))
        .route("/v1/sulcusai", get(about))
        .with_state(ctx)
}

/// The model name for the SulcusAI assistant itself ("sulcusai", or
/// "sulcusai:<model>" for it on a given model): the chat runs in the app,
/// with its tools, memory, folders, accounts and the web as the app allows.
const ASSISTANT: &str = "sulcusai";

/// Some(None): the assistant on the default model; Some(Some(m)): on `m`.
fn assistant_model(asked: Option<&str>) -> Option<Option<&str>> {
    let a = asked?.trim();
    if a.eq_ignore_ascii_case(ASSISTANT) {
        return Some(None);
    }
    a.strip_prefix("sulcusai:").map(|m| Some(m.trim()))
}

fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": { "message": message, "type": "sulcusai_error" } }))).into_response()
}

/// Same length and bytes, compared without stopping early.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Only this PC, only with the key.
fn check(headers: &HeaderMap, key: &str, port: u16) -> Result<(), Response> {
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or_default().to_ascii_lowercase();
    let local = [format!("127.0.0.1:{port}"), format!("localhost:{port}"), "127.0.0.1".into(), "localhost".into()];
    if !local.contains(&host) {
        return Err(error(StatusCode::FORBIDDEN, "Only programs on this PC can use this server."));
    }
    let given = headers.get(header::AUTHORIZATION).and_then(|h| h.to_str().ok()).and_then(|h| h.strip_prefix("Bearer ")).unwrap_or_default();
    if !same(given.trim(), key) {
        return Err(error(StatusCode::UNAUTHORIZED, "Missing or wrong API key. Copy it from SulcusAI → Settings → Local API server."));
    }
    Ok(())
}

/// Every model a program can use: the SulcusAI assistant, this PC's models
/// and the cloud models turned on, each with what it can do.
fn model_list(state: &AppState) -> Vec<Value> {
    let (installed, cloud, level, default) = {
        let conn = state.db.lock().unwrap();
        let s = db::settings(&conn);
        (db::installed_models(&conn), crate::cloud::choices(&conn), s.connectivity, s.default_model)
    };
    let mut out = vec![json!({
        "id": ASSISTANT, "object": "model", "owned_by": "sulcusai", "name": "SulcusAI assistant",
        "description": "The SulcusAI assistant itself, on your default model: it uses the app's tools, memory, your folders and connected accounts, and the web as the app allows. Use \"sulcusai:<model id>\" for another model.",
        "default_model": default,
    })];
    for m in &installed {
        let spec = state.model_spec(&m.model_id);
        out.push(json!({
            "id": m.model_id, "object": "model", "owned_by": "sulcusai", "created": m.installed_at / 1000,
            "name": spec.as_ref().map_or(m.model_id.clone(), |s| s.name.clone()),
            "runs": "this PC", "quant": m.quant,
            "context_window": spec.as_ref().map(|s| s.arch.max_ctx),
            "tools": spec.as_ref().map(|s| s.tools), "vision": spec.as_ref().map(|s| s.vision.is_some()),
            "words_per_second": m.tps.map(|t| (t * 0.75).round()),
        }));
    }
    for c in &cloud {
        out.push(json!({
            "id": c.id, "object": "model", "owned_by": c.provider, "name": c.name, "runs": c.provider,
            "vision": c.vision, "usd_per_million_tokens": { "in": c.price_in, "out": c.price_out },
            "usable_now": level == net::Connectivity::Cloud,
        }));
    }
    out
}

async fn models(State(ctx): State<Ctx>, headers: HeaderMap) -> Response {
    if let Err(r) = check(&headers, &ctx.key, ctx.port) {
        return r;
    }
    Json(json!({ "object": "list", "data": model_list(&ctx.state) })).into_response()
}

/// What SulcusAI is and can do here, for a program (or its model) to use
/// it fully: the app, what's connected, the features on, the assistant's
/// tools, the models, and how to ask.
async fn about(State(ctx): State<Ctx>, headers: HeaderMap) -> Response {
    if let Err(r) = check(&headers, &ctx.key, ctx.port) {
        return r;
    }
    let state = &ctx.state;
    let Ok(c) = state.cipher() else { return error(StatusCode::SERVICE_UNAVAILABLE, "SulcusAI is locked. Unlock it, then try again.") };
    let (on, level) = {
        let conn = state.db.lock().unwrap();
        (crate::features::enabled(&conn), db::settings(&conn).connectivity)
    };
    let features: Vec<String> = on.iter().map(|f| format!("{f:?}")).collect();
    let tools: Vec<Value> = crate::tools::TOOLS
        .iter()
        .filter(|t| tool_on(t.name, &on))
        .map(|t| json!({ "name": t.name, "description": t.description, "asks_the_user_first": t.risk != crate::tools::Risk::Read }))
        .collect();
    Json(json!({
        "app": "SulcusAI",
        "version": env!("CARGO_PKG_VERSION"),
        "about": crate::tools::app_note(state, &c),
        "internet": format!("{level:?}"),
        "features_on": features,
        "assistant_tools": tools,
        "models": model_list(state),
        "how_to": {
            "assistant": "POST /v1/chat/completions with model \"sulcusai\" (or \"sulcusai:<model id>\"): the SulcusAI assistant answers your newest user message with its tools, memory, folders, connected accounts and the web, as in the app. Earlier messages you send become the chat so far; system messages are passed on as the program's instructions. The chat appears in SulcusAI, and the reply's sulcusai.chat_id continues it (send it back in the X-SulcusAI-Chat header). Anything that needs the user's approval waits for it in the app.",
            "plain": "POST /v1/chat/completions with any other id from /v1/models: a reply from that model alone (on this PC, or a cloud model turned on in SulcusAI, at the Cloud level), with your own tools if you send them. stream: true works for both.",
        },
    }))
    .into_response()
}

/// Whether a tool works with the features turned on.
fn tool_on(name: &str, on: &std::collections::BTreeSet<crate::features::Feature>) -> bool {
    use crate::features::Feature as F;
    let has = |f: F| on.contains(&f);
    match crate::tools::group(name) {
        "memory" => has(F::Memory),
        "meetings" => has(F::Meetings),
        "notes" => has(F::Notes),
        "tasks" => has(F::Tasks),
        "web" => has(F::WebSearch),
        "browser" => has(F::BrowserControl),
        "email" => has(F::Email),
        "calendar" => has(F::Calendar),
        "documents" => has(F::Documents),
        "media" => has(F::Images) || has(F::Video) || has(F::Music),
        "people" => has(F::People),
        "chats" => has(F::ChatTeam),
        "skills" => has(F::Connectors),
        "app" | "compute" => true,
        _ => has(F::Files),
    }
}

/// A model a request names (by id or name): installed here, or a cloud
/// model turned on; none or "default": the default model.
fn pick_model(state: &AppState, asked: Option<&str>) -> Result<String, String> {
    let conn = state.db.lock().unwrap();
    let asked = asked.map(str::trim).filter(|a| !a.is_empty() && *a != "default");
    let Some(asked) = asked else {
        return db::settings(&conn).default_model.ok_or_else(|| "No model is installed in SulcusAI yet.".to_string());
    };
    if crate::model_known(&conn, asked) {
        return Ok(asked.to_string());
    }
    let local = db::installed_models(&conn).into_iter().find(|m| state.model_spec(&m.model_id).is_some_and(|s| s.name.eq_ignore_ascii_case(asked))).map(|m| m.model_id);
    let cloud = crate::cloud::choices(&conn).into_iter().find(|c| c.name.eq_ignore_ascii_case(asked)).map(|c| c.id);
    local.or(cloud).ok_or_else(|| format!("No model is called “{asked}”. GET /v1/models lists them."))
}

/// The text of a message's content (a string, or OpenAI's list of parts).
fn content_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

fn sse(v: &Value) -> axum::body::Bytes {
    axum::body::Bytes::from(format!("data: {v}\n\n"))
}

/// A stream of server-sent events from a channel, ending with [DONE].
fn event_stream(rx: tokio::sync::mpsc::UnboundedReceiver<axum::body::Bytes>) -> Response {
    let body = futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|b| (Ok::<_, std::io::Error>(b), rx)) });
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(body))
        .unwrap_or_else(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "Couldn't stream the reply."))
}

fn chunk(id: &str, model: &str, delta: Value, finish: Option<&str>) -> Value {
    json!({ "id": id, "object": "chat.completion.chunk", "created": db::now_ms() / 1000, "model": model, "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }] })
}

async fn chat(State(ctx): State<Ctx>, headers: HeaderMap, Json(mut body): Json<Value>) -> Response {
    if let Err(r) = check(&headers, &ctx.key, ctx.port) {
        return r;
    }
    let state = &ctx.state;
    let asked = body.get("model").and_then(Value::as_str).map(str::to_string);
    if let Some(on) = assistant_model(asked.as_deref()) {
        let on = on.map(str::to_string);
        return Box::pin(assistant(&ctx, &headers, &body, on)).await;
    }
    let id = match pick_model(state, asked.as_deref()) {
        Ok(id) => id,
        Err(e) => return error(StatusCode::NOT_FOUND, &e),
    };
    if crate::cloud::is_cloud(&id) {
        return cloud_reply(state, &id, &body).await;
    }
    // Don't swap the model out from under a chat that's replying in the app.
    let loaded = state.engine.lock().await.status().model_id;
    if loaded.as_deref() != Some(id.as_str()) && !state.generations.lock().unwrap().is_empty() {
        return error(StatusCode::CONFLICT, "SulcusAI is busy replying with another model. Try again in a moment, or use the model that's loaded.");
    }
    let (ep, spec, _) = match crate::llm_endpoint(state, Some(id.clone()), false, |_| {}).await {
        Ok(x) => x,
        Err(e) => return error(StatusCode::SERVICE_UNAVAILABLE, &e),
    };
    let streaming = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    body["model"] = json!(id);
    state.log("api", &format!("A program on this PC asked {} for a reply through the local API", spec.name));
    let resp = match net::local_client().post(ep.url("/v1/chat/completions")).bearer_auth(&ep.key).json(&body).send().await {
        Ok(r) => r,
        Err(e) => return error(StatusCode::BAD_GATEWAY, &format!("Couldn't reach the engine: {e}")),
    };
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    if streaming && status.is_success() {
        *state.engine_used.lock().unwrap() = std::time::Instant::now();
        return Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .header(header::CACHE_CONTROL, "no-cache")
            .body(Body::from_stream(resp.bytes_stream()))
            .unwrap_or_else(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "Couldn't stream the reply."));
    }
    let text = resp.text().await.unwrap_or_default();
    let mut out: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({ "error": { "message": text } }));
    if out.get("model").is_some() {
        out["model"] = json!(id);
    }
    (status, Json(out)).into_response()
}

/// A cloud model turned on in SulcusAI: the request goes to its provider
/// with the key kept here (the program never sees it), counted in the budget.
async fn cloud_reply(state: &Arc<AppState>, id: &str, body: &Value) -> Response {
    let (ep, spec, _) = match crate::llm_endpoint(state, Some(id.to_string()), false, |_| {}).await {
        Ok(x) => x,
        Err(e) => return error(StatusCode::SERVICE_UNAVAILABLE, &e),
    };
    state.log("api", &format!("A program on this PC asked {} for a reply through the local API", spec.name));
    let messages = body.get("messages").and_then(Value::as_array).cloned().unwrap_or_default();
    let tools = body.get("tools").cloned().filter(|t| t.as_array().is_some_and(|a| !a.is_empty()));
    let reply_id = format!("chatcmpl-{}", uuid::Uuid::new_v4().simple());
    let model = id.to_string();
    let calls = |f: &crate::chat::Finished| -> Vec<Value> {
        f.tool_calls.iter().map(|c| json!({ "id": c.id, "type": "function", "function": { "name": c.name, "arguments": c.arguments } })).collect()
    };
    if body.get("stream").and_then(Value::as_bool).unwrap_or(false) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            let cancel = std::sync::atomic::AtomicBool::new(false);
            let send = tx.clone();
            let (rid, m) = (reply_id.clone(), model.clone());
            let done = crate::chat::stream(&ep, messages, tools.as_ref(), &cancel, move |d| {
                let delta = match d {
                    crate::chat::Delta::Content(s) => json!({ "content": s }),
                    crate::chat::Delta::Thinking(s) => json!({ "reasoning_content": s }),
                };
                let _ = send.send(sse(&chunk(&rid, &m, delta, None)));
            })
            .await;
            match done {
                Ok(f) if !f.tool_calls.is_empty() => {
                    let _ = tx.send(sse(&chunk(&reply_id, &model, json!({ "tool_calls": calls(&f) }), Some("tool_calls"))));
                }
                Ok(_) => {
                    let _ = tx.send(sse(&chunk(&reply_id, &model, json!({}), Some("stop"))));
                }
                Err(e) => {
                    let _ = tx.send(sse(&json!({ "error": { "message": e } })));
                }
            }
            let _ = tx.send(axum::body::Bytes::from_static(b"data: [DONE]\n\n"));
        });
        return event_stream(rx);
    }
    let cancel = std::sync::atomic::AtomicBool::new(false);
    match crate::chat::stream(&ep, messages, tools.as_ref(), &cancel, |_| {}).await {
        Ok(f) => {
            let tool_calls = calls(&f);
            let mut message = json!({ "role": "assistant", "content": f.content });
            if !f.thinking.is_empty() {
                message["reasoning_content"] = json!(f.thinking);
            }
            if !tool_calls.is_empty() {
                message["tool_calls"] = json!(tool_calls);
            }
            Json(json!({
                "id": reply_id, "object": "chat.completion", "created": db::now_ms() / 1000, "model": model,
                "choices": [{ "index": 0, "message": message, "finish_reason": if tool_calls.is_empty() { "stop" } else { "tool_calls" } }],
                "usage": { "prompt_tokens": f.prompt_tokens, "completion_tokens": f.completion_tokens },
            }))
            .into_response()
        }
        Err(e) => error(StatusCode::BAD_GATEWAY, &e),
    }
}

/// The SulcusAI assistant answering a program: the newest user message
/// runs as a turn in a chat in the app (the one named by X-SulcusAI-Chat,
/// or a new one holding the earlier messages), with everything the app's
/// assistant has. What needs the user's approval waits for it in the app.
async fn assistant(ctx: &Ctx, headers: &HeaderMap, body: &Value, on: Option<String>) -> Response {
    let state = &ctx.state;
    let Some(app) = state.app.get().cloned() else { return error(StatusCode::SERVICE_UNAVAILABLE, "SulcusAI isn't ready yet.") };
    let Ok(c) = state.cipher() else { return error(StatusCode::SERVICE_UNAVAILABLE, "SulcusAI is locked. Unlock it, then try again.") };
    let model = match on.as_deref().map(|m| pick_model(state, Some(m))).transpose() {
        Ok(m) => m,
        Err(e) => return error(StatusCode::NOT_FOUND, &e),
    };
    let messages = body.get("messages").and_then(Value::as_array).cloned().unwrap_or_default();
    let Some(last) = messages.iter().rposition(|m| m.get("role").and_then(Value::as_str) == Some("user")) else {
        return error(StatusCode::BAD_REQUEST, "Send at least one message with role \"user\".");
    };
    let mut text = content_text(&messages[last]["content"]);
    let instructions: Vec<String> = messages
        .iter()
        .filter(|m| matches!(m.get("role").and_then(Value::as_str), Some("system" | "developer")))
        .map(|m| content_text(&m["content"]))
        .filter(|s| !s.trim().is_empty())
        .collect();
    if !instructions.is_empty() {
        text = format!("Instructions from the program that sent this:\n{}\n\n{text}", instructions.join("\n"));
    }
    let named = headers.get("x-sulcusai-chat").and_then(|h| h.to_str().ok()).map(str::to_string).or_else(|| body.get("chat_id").and_then(Value::as_str).map(str::to_string));
    let chat_id = {
        let conn = state.db.lock().unwrap();
        match named {
            Some(id) => match db::chat(&conn, &c, &id) {
                Some(ch) => ch.id,
                None => return error(StatusCode::NOT_FOUND, "There's no chat with that id in SulcusAI."),
            },
            None => {
                let chat = match db::create_chat(&conn, &c, model.clone()) {
                    Ok(ch) => ch,
                    Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, &e),
                };
                // The conversation so far, as the program sent it.
                for m in &messages[..last] {
                    let role = m.get("role").and_then(Value::as_str).unwrap_or_default();
                    let said = content_text(&m["content"]);
                    if matches!(role, "user" | "assistant") && !said.trim().is_empty() {
                        let _ = db::add_message(&conn, &c, &db::Message { id: uuid::Uuid::new_v4().to_string(), chat_id: chat.id.clone(), role: role.into(), content: said, created_at: db::now_ms(), ..Default::default() });
                    }
                }
                chat.id
            }
        }
    };
    if let Some(m) = &model {
        let _ = db::set_chat_model(&state.db.lock().unwrap(), &chat_id, m);
    }
    {
        use tauri::Emitter;
        app.emit("chat:changed", json!({ "chat_id": chat_id })).ok();
    }
    state.log("api", "A program on this PC asked the SulcusAI assistant something through the local API");
    let reply_id = format!("chatcmpl-{}", uuid::Uuid::new_v4().simple());
    let name = on.map_or(ASSISTANT.to_string(), |m| format!("{ASSISTANT}:{m}"));
    let used: Arc<Mutex<Vec<String>>> = Default::default();
    let seen = used.clone();
    let streaming = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<axum::body::Bytes>();
    let (rid, m, out) = (reply_id.clone(), name.clone(), tx.clone());
    // Every event of the turn: what it writes streams out; the tools it uses are listed.
    let tee: crate::Tee = Arc::new(move |event: &str, p: &Value| match event {
        "chat:delta" if streaming => {
            let mut delta = json!({});
            if let Some(s) = p.get("content").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                delta["content"] = json!(s);
            }
            if let Some(s) = p.get("thinking").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                delta["reasoning_content"] = json!(s);
            }
            if delta.as_object().is_some_and(|o| !o.is_empty()) {
                let _ = out.send(sse(&chunk(&rid, &m, delta, None)));
            }
        }
        "agent:tool_start" => {
            if let Some(t) = p.get("tool").and_then(Value::as_str) {
                seen.lock().unwrap().push(t.to_string());
            }
        }
        _ => {}
    });
    let state2 = state.clone();
    let chat2 = chat_id.clone();
    let turn = tokio::spawn(async move { Box::pin(crate::run_turn(&app, &state2, chat2, text, Vec::new(), Some(tee))).await });
    let extra = |used: &Arc<Mutex<Vec<String>>>| json!({ "chat_id": chat_id, "tools_used": used.lock().unwrap().clone() });
    if streaming {
        // After what it writes: why it stopped, and the chat to continue in.
        let (rid, m, info) = (reply_id.clone(), name.clone(), extra(&used));
        let used = used.clone();
        tokio::spawn(async move {
            let failed = match turn.await {
                Ok(Ok(_)) => None,
                Ok(Err(e)) => Some(e),
                Err(e) => Some(e.to_string()),
            };
            if let Some(e) = failed {
                let _ = tx.send(sse(&json!({ "error": { "message": e } })));
            }
            let mut last = chunk(&rid, &m, json!({}), Some("stop"));
            last["sulcusai"] = json!({ "chat_id": info["chat_id"], "tools_used": used.lock().unwrap().clone() });
            let _ = tx.send(sse(&last));
            let _ = tx.send(axum::body::Bytes::from_static(b"data: [DONE]\n\n"));
        });
        return event_stream(rx);
    }
    drop(rx);
    match turn.await {
        Ok(Ok(last)) => {
            let content = last.as_ref().map(|m| m.content.clone()).unwrap_or_default();
            let mut message = json!({ "role": "assistant", "content": content });
            if let Some(t) = last.as_ref().and_then(|m| m.thinking.clone()).filter(|t| !t.is_empty()) {
                message["reasoning_content"] = json!(t);
            }
            Json(json!({
                "id": reply_id, "object": "chat.completion", "created": db::now_ms() / 1000, "model": name,
                "choices": [{ "index": 0, "message": message, "finish_reason": "stop" }],
                "sulcusai": extra(&used),
            }))
            .into_response()
        }
        Ok(Err(e)) => error(StatusCode::BAD_GATEWAY, &e),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

// ---------- settings commands ----------

#[derive(Serialize)]
pub struct ApiServerView {
    enabled: bool,
    port: u16,
    key: String,
    url: String,
    running: bool,
    problem: Option<String>,
}

fn view(state: &AppState) -> ApiServerView {
    let s = settings(&state.db.lock().unwrap());
    let running = SERVER.lock().unwrap().as_ref().is_some_and(|r| r.port == s.port);
    ApiServerView { url: format!("http://127.0.0.1:{}/v1", s.port), enabled: s.enabled, port: s.port, key: s.key, running, problem: PROBLEM.lock().unwrap().clone() }
}

#[tauri::command(async)]
pub fn api_server_view(state: AppStateRef) -> ApiServerView {
    view(&state)
}

#[tauri::command]
pub async fn set_api_server(state: AppStateRef<'_>, enabled: bool, port: u16) -> Result<ApiServerView, String> {
    if port < 1024 {
        return Err("Use a port from 1024 to 65535.".into());
    }
    {
        let conn = state.db.lock().unwrap();
        let mut s = settings(&conn);
        let was = s.enabled;
        s.enabled = enabled;
        s.port = port;
        save(&conn, &s)?;
        drop(conn);
        if was != enabled {
            let note = if enabled { format!("Turned on the local API server (127.0.0.1:{port})") } else { "Turned off the local API server".into() };
            state.log("settings", &note);
        }
    }
    apply(state.inner().clone());
    // Give it a moment to start (or report the port in use).
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    Ok(view(&state))
}

/// A new key; programs using the old one stop working.
#[tauri::command(async)]
pub fn new_api_key(state: AppStateRef) -> Result<ApiServerView, String> {
    {
        let conn = state.db.lock().unwrap();
        let mut s = settings(&conn);
        s.key = new_key();
        save(&conn, &s)?;
    }
    state.log("settings", "Made a new key for the local API server");
    apply(state.inner().clone());
    Ok(view(&state))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(host: &str, auth: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, host.parse().unwrap());
        if let Some(a) = auth {
            h.insert(header::AUTHORIZATION, a.parse().unwrap());
        }
        h
    }

    #[test]
    fn only_this_pc_with_the_key() {
        let key = "sk-sulcus-abc";
        assert!(check(&headers("127.0.0.1:7340", Some("Bearer sk-sulcus-abc")), key, 7340).is_ok());
        assert!(check(&headers("localhost:7340", Some("Bearer sk-sulcus-abc")), key, 7340).is_ok());
        let status = |r: Result<(), Response>| r.err().map(|r| r.status());
        assert_eq!(status(check(&headers("127.0.0.1:7340", None), key, 7340)), Some(StatusCode::UNAUTHORIZED));
        assert_eq!(status(check(&headers("127.0.0.1:7340", Some("Bearer sk-sulcus-abd")), key, 7340)), Some(StatusCode::UNAUTHORIZED));
        // A web page that points its own domain at 127.0.0.1 (DNS rebinding).
        assert_eq!(status(check(&headers("evil.example:7340", Some("Bearer sk-sulcus-abc")), key, 7340)), Some(StatusCode::FORBIDDEN));
    }

    #[test]
    fn the_assistant_is_asked_for_by_name() {
        assert_eq!(assistant_model(Some("sulcusai")), Some(None));
        assert_eq!(assistant_model(Some("SulcusAI")), Some(None));
        assert_eq!(assistant_model(Some("sulcusai:qwen3-8b")), Some(Some("qwen3-8b")));
        assert_eq!(assistant_model(Some("sulcusai:cloud:openai:gpt-5")), Some(Some("cloud:openai:gpt-5")));
        assert_eq!(assistant_model(Some("qwen3-8b")), None);
        assert_eq!(assistant_model(None), None);
        // A message's text, plain or in parts.
        assert_eq!(content_text(&json!("hi")), "hi");
        assert_eq!(content_text(&json!([{ "type": "text", "text": "a" }, { "type": "image_url" }, { "type": "text", "text": "b" }])), "a\nb");
    }
}
