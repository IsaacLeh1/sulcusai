// SPDX-License-Identifier: AGPL-3.0-only
//! A second, optional listener in Ollama's shape, on its port (11434), so
//! programs made for Ollama can use the models on this PC as they are.
//! Like Ollama it takes no key, so it offers only this PC's own models: no
//! cloud models (they cost money) and not the SulcusAI assistant (it works
//! with files and accounts). It listens on 127.0.0.1 only, refuses a Host
//! that isn't this PC, and refuses requests sent by web pages.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio::sync::oneshot;

use crate::catalog::ModelSpec;
use crate::{db, engine, net, AppState};

pub const DEFAULT_PORT: u16 = 11434;
/// The Ollama version reported: programs check for a recent enough one.
const LIKE_VERSION: &str = "0.12.6";

struct Running {
    port: u16,
    stop: oneshot::Sender<()>,
}

static SERVER: Mutex<Option<Running>> = Mutex::new(None);
static PROBLEM: Mutex<Option<String>> = Mutex::new(None);

#[derive(Clone)]
struct Ctx {
    state: Arc<AppState>,
    port: u16,
}

/// Starts, restarts or stops the listener (`port`, or none).
pub fn apply(state: Arc<AppState>, port: Option<u16>) {
    if let Some(r) = SERVER.lock().unwrap().take() {
        let _ = r.stop.send(());
    }
    *PROBLEM.lock().unwrap() = None;
    let Some(port) = port else { return };
    let (tx, rx) = oneshot::channel();
    *SERVER.lock().unwrap() = Some(Running { port, stop: tx });
    let ctx = Ctx { state, port };
    tauri::async_runtime::spawn(async move {
        let mut listener = None;
        for _ in 0..20 {
            match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
                Ok(l) => {
                    listener = Some(l);
                    break;
                }
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
            }
        }
        let Some(listener) = listener else {
            *PROBLEM.lock().unwrap() = Some(format!("Port {port} is in use by another program (Ollama itself, if it's running). Quit it, or pick another port."));
            return;
        };
        let _ = axum::serve(listener, router(ctx)).with_graceful_shutdown(async { rx.await.ok(); }).await;
    });
}

/// Whether it's listening on `port` now, and what went wrong if not.
pub fn status(port: u16) -> (bool, Option<String>) {
    (SERVER.lock().unwrap().as_ref().is_some_and(|r| r.port == port), PROBLEM.lock().unwrap().clone())
}

/// The routes over `state`, for tests that call them over HTTP.
#[cfg(test)]
pub(crate) fn router_for_test(state: Arc<AppState>, port: u16) -> Router {
    router(Ctx { state, port })
}

fn router(ctx: Ctx) -> Router {
    Router::new()
        .route("/", get(hello).head(hello))
        .route("/api/version", get(version))
        .route("/api/tags", get(tags))
        .route("/api/ps", get(running))
        .route("/api/show", post(show))
        .route("/api/chat", post(chat))
        .route("/api/generate", post(generate))
        .route("/api/embed", post(embed))
        .route("/api/embeddings", post(embed_one))
        // Ollama's OpenAI-shaped routes, for programs pointed at it that way.
        .route("/v1/models", get(openai_models))
        .route("/v1/chat/completions", post(openai_chat))
        .with_state(ctx)
}

fn fail(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

/// This PC only, and not a web page: a page in a browser can send requests
/// to 127.0.0.1, but it always says where it's from (Origin).
fn guard(headers: &HeaderMap, port: u16) -> Result<(), Response> {
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or_default().to_ascii_lowercase();
    let name = host.strip_suffix(&format!(":{port}")).unwrap_or(&host);
    if !["127.0.0.1", "localhost", "[::1]"].contains(&name) {
        return Err(fail(StatusCode::FORBIDDEN, "Only programs on this PC can use this server."));
    }
    if let Some(origin) = headers.get(header::ORIGIN).and_then(|h| h.to_str().ok()) {
        let o = origin.to_ascii_lowercase();
        let local = ["http://127.0.0.1", "http://localhost", "https://127.0.0.1", "https://localhost", "http://[::1]"]
            .iter()
            .any(|p| o == *p || o.strip_prefix(p).is_some_and(|rest| rest.starts_with(':')));
        if !local {
            return Err(fail(StatusCode::FORBIDDEN, "Web pages can't use SulcusAI's models through this server."));
        }
    }
    Ok(())
}

/// A model's name for Ollama programs: its name in small letters, words
/// joined by dashes ("Qwen3 VL 4B" → "qwen3-vl-4b").
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() || c == '.' || c == '_' {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

/// This PC's models: (name, id, spec, installed).
fn local_models(state: &AppState) -> Vec<(String, db::InstalledModel, Option<ModelSpec>)> {
    let installed = db::installed_models(&state.db.lock().unwrap());
    installed
        .into_iter()
        .map(|m| {
            let spec = state.model_spec(&m.model_id);
            let name = format!("{}:latest", slug(spec.as_ref().map_or(m.model_id.as_str(), |s| s.name.as_str())));
            (name, m, spec)
        })
        .collect()
}

/// The model a program names: by its Ollama name (with or without a tag)
/// or its id; none: the default model.
fn find(state: &AppState, asked: Option<&str>) -> Result<(String, String), String> {
    let models = local_models(state);
    let asked = asked.map(str::trim).filter(|a| !a.is_empty());
    let wanted = match asked {
        Some(a) => a.to_string(),
        None => db::settings(&state.db.lock().unwrap()).default_model.ok_or_else(|| "No model is installed in SulcusAI yet.".to_string())?,
    };
    let bare = wanted.strip_suffix(":latest").unwrap_or(&wanted).to_lowercase();
    models
        .iter()
        .find(|(name, m, _)| m.model_id.eq_ignore_ascii_case(&wanted) || m.model_id.eq_ignore_ascii_case(&bare) || name.trim_end_matches(":latest") == bare)
        .map(|(name, m, _)| (name.clone(), m.model_id.clone()))
        .ok_or_else(|| format!("model \"{wanted}\" not found: GET /api/tags lists the models on this PC"))
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
}

fn details(m: &db::InstalledModel, spec: Option<&ModelSpec>) -> Value {
    // The name's first word ("Qwen3 4B Instruct" → "qwen3"): ids of models
    // found in other apps say nothing ("local-…").
    let family = spec.map(|s| slug(&s.name).split('-').next().unwrap_or("").to_string()).unwrap_or_default();
    let size = spec.map(|s| s.params_b).filter(|b| *b > 0.0).map(|b| if b >= 1.0 { format!("{b:.1}B").replace(".0B", "B") } else { format!("{:.0}M", b * 1000.0) });
    json!({ "parent_model": "", "format": "gguf", "family": family, "families": [family], "parameter_size": size.unwrap_or_default(), "quantization_level": m.quant })
}

fn capabilities(spec: Option<&ModelSpec>) -> Vec<&'static str> {
    let mut c = vec!["completion"];
    if spec.is_some_and(|s| s.tools) {
        c.push("tools");
    }
    if spec.is_some_and(|s| s.vision.is_some()) {
        c.push("vision");
    }
    c
}

async fn hello(State(ctx): State<Ctx>, headers: HeaderMap) -> Response {
    if let Err(r) = guard(&headers, ctx.port) {
        return r;
    }
    "Ollama is running".into_response()
}

async fn version(State(ctx): State<Ctx>, headers: HeaderMap) -> Response {
    if let Err(r) = guard(&headers, ctx.port) {
        return r;
    }
    Json(json!({ "version": LIKE_VERSION })).into_response()
}

async fn tags(State(ctx): State<Ctx>, headers: HeaderMap) -> Response {
    if let Err(r) = guard(&headers, ctx.port) {
        return r;
    }
    let models: Vec<Value> = local_models(&ctx.state)
        .iter()
        .map(|(name, m, spec)| {
            let at = chrono::DateTime::from_timestamp_millis(m.installed_at).unwrap_or_default().to_rfc3339();
            json!({ "name": name, "model": name, "modified_at": at, "size": m.size, "digest": m.model_id, "details": details(m, spec.as_ref()) })
        })
        .collect();
    Json(json!({ "models": models })).into_response()
}

async fn running(State(ctx): State<Ctx>, headers: HeaderMap) -> Response {
    if let Err(r) = guard(&headers, ctx.port) {
        return r;
    }
    let loaded = ctx.state.engine.lock().await.status().model_id;
    let models: Vec<Value> = local_models(&ctx.state)
        .iter()
        .filter(|(_, m, _)| loaded.as_deref() == Some(m.model_id.as_str()))
        .map(|(name, m, spec)| json!({ "name": name, "model": name, "size": m.size, "size_vram": m.size, "digest": m.model_id, "details": details(m, spec.as_ref()), "expires_at": "0001-01-01T00:00:00Z" }))
        .collect();
    Json(json!({ "models": models })).into_response()
}

async fn show(State(ctx): State<Ctx>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    if let Err(r) = guard(&headers, ctx.port) {
        return r;
    }
    let asked = body.get("model").or_else(|| body.get("name")).and_then(Value::as_str);
    let (name, id) = match find(&ctx.state, asked) {
        Ok(x) => x,
        Err(e) => return fail(StatusCode::NOT_FOUND, &e),
    };
    let models = local_models(&ctx.state);
    let Some((_, m, spec)) = models.iter().find(|(n, _, _)| *n == name) else { return fail(StatusCode::NOT_FOUND, "model not found") };
    let family = details(m, spec.as_ref())["family"].as_str().unwrap_or("").to_string();
    let mut info = json!({ "general.architecture": family, "general.basename": spec.as_ref().map_or(id.clone(), |s| s.name.clone()) });
    if let Some(s) = spec {
        info[format!("{family}.context_length")] = json!(s.arch.max_ctx);
        info[format!("{family}.block_count")] = json!(s.arch.n_layer);
        info["general.parameter_count"] = json!((s.params_b * 1e9) as u64);
    }
    Json(json!({
        "license": spec.as_ref().map(|s| format!("{:?}", s.license)).unwrap_or_default(),
        "modelfile": format!("# Served by SulcusAI\nFROM {}\n", m.path),
        "parameters": "",
        "template": "{{ .Prompt }}",
        "details": details(m, spec.as_ref()),
        "model_info": info,
        "capabilities": capabilities(spec.as_ref()),
        "modified_at": chrono::DateTime::from_timestamp_millis(m.installed_at).unwrap_or_default().to_rfc3339(),
    }))
    .into_response()
}

/// The model loaded and ready, unless a chat in the app is answering with
/// another one (it isn't swapped out from under it).
async fn ready(state: &Arc<AppState>, id: &str, pictures: bool) -> Result<engine::Endpoint, Response> {
    let loaded = state.engine.lock().await.status().model_id;
    if loaded.as_deref() != Some(id) && !state.generations.lock().unwrap().is_empty() {
        return Err(fail(StatusCode::CONFLICT, "SulcusAI is busy replying with another model. Try again in a moment, or use the model that's loaded."));
    }
    match crate::llm_endpoint(state, Some(id.to_string()), pictures, |_| {}).await {
        Ok((ep, spec, _)) => {
            state.log("api", &format!("A program on this PC asked {} for a reply through the Ollama-style server", spec.name));
            Ok(ep)
        }
        Err(e) => Err(fail(StatusCode::SERVICE_UNAVAILABLE, &e)),
    }
}

/// A picture's kind from the start of its base64.
fn picture_url(b64: &str) -> String {
    let mime = if b64.starts_with("iVBOR") {
        "image/png"
    } else if b64.starts_with("UklGR") {
        "image/webp"
    } else if b64.starts_with("R0lGOD") {
        "image/gif"
    } else {
        "image/jpeg"
    };
    format!("data:{mime};base64,{b64}")
}

/// Ollama's messages in OpenAI's shape, for the engine.
pub fn openai_messages(messages: &[Value]) -> Vec<Value> {
    let mut out = Vec::new();
    for (i, m) in messages.iter().enumerate() {
        let role = m.get("role").and_then(Value::as_str).unwrap_or("user");
        let text = m.get("content").and_then(Value::as_str).unwrap_or("");
        let images: Vec<&str> = m.get("images").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        let content = if images.is_empty() {
            json!(text)
        } else {
            let mut parts = vec![json!({ "type": "text", "text": text })];
            parts.extend(images.iter().map(|b| json!({ "type": "image_url", "image_url": { "url": picture_url(b) } })));
            json!(parts)
        };
        let mut msg = json!({ "role": role, "content": content });
        if let Some(t) = m.get("thinking").and_then(Value::as_str).filter(|t| !t.is_empty()) {
            msg["reasoning_content"] = json!(t);
        }
        if let Some(calls) = m.get("tool_calls").and_then(Value::as_array) {
            let calls: Vec<Value> = calls
                .iter()
                .enumerate()
                .map(|(j, c)| {
                    let f = &c["function"];
                    let args = match &f["arguments"] {
                        Value::String(s) => s.clone(),
                        v => v.to_string(),
                    };
                    json!({ "id": c.get("id").and_then(Value::as_str).map_or(format!("call_{i}_{j}"), str::to_string), "type": "function", "function": { "name": f["name"], "arguments": args } })
                })
                .collect();
            msg["tool_calls"] = json!(calls);
        }
        if role == "tool" {
            if let Some(n) = m.get("tool_name").or_else(|| m.get("name")).and_then(Value::as_str) {
                msg["name"] = json!(n);
            }
            if let Some(id) = m.get("tool_call_id").and_then(Value::as_str) {
                msg["tool_call_id"] = json!(id);
            }
        }
        out.push(msg);
    }
    out
}

/// Ollama's `format`, `options` and `think` in OpenAI's shape, over `extra`
/// (the app's own settings for the model).
pub fn apply_options(body: &mut Value, req: &Value, extra: &Value) {
    if let Some(o) = extra.as_object() {
        for (k, v) in o {
            body[k] = v.clone();
        }
    }
    match req.get("format") {
        Some(Value::String(s)) if s == "json" => body["response_format"] = json!({ "type": "json_object" }),
        Some(schema @ Value::Object(_)) => body["response_format"] = json!({ "type": "json_schema", "json_schema": { "name": "reply", "schema": schema } }),
        _ => {}
    }
    if let Some(o) = req.get("options").and_then(Value::as_object) {
        for (from, to) in [("temperature", "temperature"), ("top_p", "top_p"), ("top_k", "top_k"), ("min_p", "min_p"), ("seed", "seed"), ("stop", "stop"), ("repeat_penalty", "repeat_penalty"), ("presence_penalty", "presence_penalty"), ("frequency_penalty", "frequency_penalty")] {
            if let Some(v) = o.get(from).filter(|v| !v.is_null()) {
                body[to] = v.clone();
            }
        }
        if let Some(n) = o.get("num_predict").and_then(Value::as_i64).filter(|n| *n > 0) {
            body["max_tokens"] = json!(n);
        }
    }
    match req.get("think") {
        Some(Value::Bool(on)) => body["chat_template_kwargs"]["enable_thinking"] = json!(on),
        Some(Value::String(level)) => {
            body["chat_template_kwargs"]["enable_thinking"] = json!(true);
            body["reasoning_effort"] = json!(level);
            body["chat_template_kwargs"]["reasoning_effort"] = json!(level);
        }
        _ => {}
    }
}

/// Ollama's closing figures from what the engine reported (OpenAI's usage,
/// llama.cpp's timings, in milliseconds).
fn closing(usage: &Value, timings: &Value, started: std::time::Instant) -> Value {
    let ns = |ms: Option<f64>| (ms.unwrap_or(0.0) * 1e6) as u64;
    let prompt = usage.get("prompt_tokens").or_else(|| timings.get("prompt_n")).and_then(Value::as_u64).unwrap_or(0);
    let eval = usage.get("completion_tokens").or_else(|| timings.get("predicted_n")).and_then(Value::as_u64).unwrap_or(0);
    json!({
        "total_duration": started.elapsed().as_nanos() as u64,
        "load_duration": 0,
        "prompt_eval_count": prompt,
        "prompt_eval_duration": ns(timings.get("prompt_ms").and_then(Value::as_f64)),
        "eval_count": eval,
        "eval_duration": ns(timings.get("predicted_ms").and_then(Value::as_f64)),
    })
}

/// OpenAI's tool calls in Ollama's shape (arguments as an object).
fn ollama_calls(calls: &[Value]) -> Vec<Value> {
    calls
        .iter()
        .map(|c| {
            let args = c["function"]["arguments"].as_str().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_else(|| json!({}));
            json!({ "function": { "name": c["function"]["name"], "arguments": args } })
        })
        .collect()
}

fn merge(a: &mut Value, b: &Value) {
    if let (Some(a), Some(b)) = (a.as_object_mut(), b.as_object()) {
        for (k, v) in b {
            a.insert(k.clone(), v.clone());
        }
    }
}

/// Lines of newline-separated JSON as a streamed reply.
fn ndjson(rx: tokio::sync::mpsc::UnboundedReceiver<Value>) -> Response {
    let body = futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|v| (Ok::<_, std::io::Error>(axum::body::Bytes::from(format!("{v}\n"))), rx)) });
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .body(Body::from_stream(body))
        .unwrap_or_else(|_| fail(StatusCode::INTERNAL_SERVER_ERROR, "Couldn't stream the reply."))
}

/// Each JSON event of the engine's server-sent events, in order.
async fn each_event(resp: reqwest::Response, mut f: impl FnMut(Value)) {
    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    while let Some(Ok(bytes)) = stream.next().await {
        buf.push_str(&String::from_utf8_lossy(&bytes));
        while let Some(at) = buf.find('\n') {
            let line: String = buf.drain(..=at).collect();
            if let Some(data) = line.trim().strip_prefix("data:") {
                if let Ok(v) = serde_json::from_str::<Value>(data.trim()) {
                    f(v);
                }
            }
        }
    }
}

/// A chat reply in Ollama's shape: what the engine writes, piece by piece
/// as `{message, done: false}` lines (or in one), then the closing figures.
/// `generate`: as /api/generate answers (`response` instead of `message`).
async fn reply(state: &Arc<AppState>, name: String, id: String, req: &Value, messages: Vec<Value>, generate: bool) -> Response {
    let pictures = messages.iter().any(|m| m["content"].is_array());
    let ep = match ready(state, &id, pictures).await {
        Ok(ep) => ep,
        Err(r) => return r,
    };
    let stream = req.get("stream").and_then(Value::as_bool).unwrap_or(true);
    let mut body = json!({ "model": id, "messages": messages, "stream": stream });
    if let Some(t) = req.get("tools").filter(|t| t.as_array().is_some_and(|a| !a.is_empty())) {
        body["tools"] = t.clone();
    }
    apply_options(&mut body, req, &ep.extra);
    if stream {
        body["stream_options"] = json!({ "include_usage": true });
    }
    let started = std::time::Instant::now();
    let resp = match net::local_client().post(ep.url("/v1/chat/completions")).bearer_auth(&ep.key).json(&body).send().await {
        Ok(r) => r,
        Err(e) => return fail(StatusCode::BAD_GATEWAY, &format!("Couldn't reach the engine: {e}")),
    };
    if !resp.status().is_success() {
        let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        let text = resp.text().await.unwrap_or_default();
        let msg = serde_json::from_str::<Value>(&text).ok().and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_string)).unwrap_or(text);
        return fail(status, &msg);
    }
    *state.engine_used.lock().unwrap() = std::time::Instant::now();
    let piece = move |name: &str, content: &str, thinking: &str, calls: Option<Vec<Value>>, done: bool| -> Value {
        let mut v = json!({ "model": name, "created_at": now(), "done": done });
        if generate {
            v["response"] = json!(content);
            if !thinking.is_empty() {
                v["thinking"] = json!(thinking);
            }
        } else {
            v["message"] = json!({ "role": "assistant", "content": content });
            if !thinking.is_empty() {
                v["message"]["thinking"] = json!(thinking);
            }
            if let Some(c) = calls {
                v["message"]["tool_calls"] = json!(c);
            }
        }
        v
    };
    if !stream {
        let out: Value = resp.json().await.unwrap_or_default();
        let msg = &out["choices"][0]["message"];
        let calls = msg["tool_calls"].as_array().filter(|c| !c.is_empty()).map(|c| ollama_calls(c));
        let mut v = piece(&name, msg["content"].as_str().unwrap_or(""), msg["reasoning_content"].as_str().unwrap_or(""), calls, true);
        v["done_reason"] = json!(if out["choices"][0]["finish_reason"] == "length" { "length" } else { "stop" });
        merge(&mut v, &closing(&out["usage"], &out["timings"], started));
        return Json(v).into_response();
    }
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        // Tool calls come in pieces by index; they're sent whole at the end.
        let mut calls: Vec<Value> = Vec::new();
        let (mut usage, mut timings, mut reason) = (Value::Null, Value::Null, "stop".to_string());
        each_event(resp, |ev| {
            if ev.get("usage").is_some_and(|u| !u.is_null()) {
                usage = ev["usage"].clone();
            }
            if ev.get("timings").is_some() {
                timings = ev["timings"].clone();
            }
            let Some(choice) = ev["choices"].get(0) else { return };
            if let Some(r) = choice["finish_reason"].as_str() {
                reason = r.to_string();
            }
            let d = &choice["delta"];
            let (content, thinking) = (d["content"].as_str().unwrap_or(""), d["reasoning_content"].as_str().unwrap_or(""));
            if !content.is_empty() || !thinking.is_empty() {
                let _ = tx.send(piece(&name, content, thinking, None, false));
            }
            for c in d["tool_calls"].as_array().into_iter().flatten() {
                let i = c["index"].as_u64().unwrap_or(0) as usize;
                while calls.len() <= i {
                    calls.push(json!({ "function": { "name": "", "arguments": "" } }));
                }
                if let Some(n) = c["function"]["name"].as_str() {
                    calls[i]["function"]["name"] = json!(format!("{}{n}", calls[i]["function"]["name"].as_str().unwrap_or("")));
                }
                if let Some(a) = c["function"]["arguments"].as_str() {
                    calls[i]["function"]["arguments"] = json!(format!("{}{a}", calls[i]["function"]["arguments"].as_str().unwrap_or("")));
                }
            }
        })
        .await;
        if !calls.is_empty() && !generate {
            let _ = tx.send(piece(&name, "", "", Some(ollama_calls(&calls)), false));
        }
        let mut last = piece(&name, "", "", None, true);
        last["done_reason"] = json!(if reason == "length" { "length" } else { "stop" });
        merge(&mut last, &closing(&usage, &timings, started));
        let _ = tx.send(last);
    });
    ndjson(rx)
}

async fn chat(State(ctx): State<Ctx>, headers: HeaderMap, Json(req): Json<Value>) -> Response {
    if let Err(r) = guard(&headers, ctx.port) {
        return r;
    }
    let (name, id) = match find(&ctx.state, req.get("model").and_then(Value::as_str)) {
        Ok(x) => x,
        Err(e) => return fail(StatusCode::NOT_FOUND, &e),
    };
    let messages = openai_messages(req.get("messages").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default());
    // No messages: Ollama loads the model and says it's done.
    if messages.is_empty() {
        if let Err(r) = ready(&ctx.state, &id, false).await {
            return r;
        }
        return Json(json!({ "model": name, "created_at": now(), "message": { "role": "assistant", "content": "" }, "done_reason": "load", "done": true })).into_response();
    }
    Box::pin(reply(&ctx.state, name, id, &req, messages, false)).await
}

async fn generate(State(ctx): State<Ctx>, headers: HeaderMap, Json(req): Json<Value>) -> Response {
    if let Err(r) = guard(&headers, ctx.port) {
        return r;
    }
    let (name, id) = match find(&ctx.state, req.get("model").and_then(Value::as_str)) {
        Ok(x) => x,
        Err(e) => return fail(StatusCode::NOT_FOUND, &e),
    };
    let prompt = req.get("prompt").and_then(Value::as_str).unwrap_or("");
    if prompt.is_empty() {
        if let Err(r) = ready(&ctx.state, &id, false).await {
            return r;
        }
        return Json(json!({ "model": name, "created_at": now(), "response": "", "done_reason": "load", "done": true })).into_response();
    }
    if req.get("raw").and_then(Value::as_bool).unwrap_or(false) || req.get("suffix").and_then(Value::as_str).is_some_and(|s| !s.is_empty()) {
        return Box::pin(complete_raw(&ctx.state, name, id, &req)).await;
    }
    let mut msgs = Vec::new();
    if let Some(s) = req.get("system").and_then(Value::as_str).filter(|s| !s.is_empty()) {
        msgs.push(json!({ "role": "system", "content": s }));
    }
    msgs.push(json!({ "role": "user", "content": prompt, "images": req.get("images").cloned().unwrap_or(json!([])) }));
    let messages = openai_messages(&msgs);
    Box::pin(reply(&ctx.state, name, id, &req, messages, true)).await
}

/// Text carried on as written (`raw`), or filled in between the prompt and
/// `suffix` (code completion): the engine's own completion, no chat template.
async fn complete_raw(state: &Arc<AppState>, name: String, id: String, req: &Value) -> Response {
    let ep = match ready(state, &id, false).await {
        Ok(ep) => ep,
        Err(r) => return r,
    };
    let stream = req.get("stream").and_then(Value::as_bool).unwrap_or(true);
    let prompt = req.get("prompt").and_then(Value::as_str).unwrap_or("");
    let suffix = req.get("suffix").and_then(Value::as_str).filter(|s| !s.is_empty());
    let mut options = json!({});
    apply_options(&mut options, req, &Value::Null);
    let mut body = match suffix {
        Some(s) => json!({ "input_prefix": prompt, "input_suffix": s, "prompt": "" }),
        None => json!({ "prompt": prompt }),
    };
    body["stream"] = json!(stream);
    for (k, to) in [("temperature", "temperature"), ("top_p", "top_p"), ("top_k", "top_k"), ("min_p", "min_p"), ("seed", "seed"), ("stop", "stop"), ("repeat_penalty", "repeat_penalty"), ("max_tokens", "n_predict")] {
        if let Some(v) = options.get(k) {
            body[to] = v.clone();
        }
    }
    let path = if suffix.is_some() { "/infill" } else { "/completion" };
    let started = std::time::Instant::now();
    let resp = match net::local_client().post(ep.url(path)).bearer_auth(&ep.key).json(&body).send().await {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => return fail(StatusCode::BAD_GATEWAY, &r.text().await.unwrap_or_default()),
        Err(e) => return fail(StatusCode::BAD_GATEWAY, &format!("Couldn't reach the engine: {e}")),
    };
    *state.engine_used.lock().unwrap() = std::time::Instant::now();
    let finish = move |v: &Value, name: &str| -> Value {
        let mut last = json!({ "model": name, "created_at": now(), "response": "", "done": true, "done_reason": if v["stopped_limit"] == true { "length" } else { "stop" } });
        let usage = json!({ "prompt_tokens": v["tokens_evaluated"], "completion_tokens": v["tokens_predicted"] });
        merge(&mut last, &closing(&usage, &v["timings"], started));
        last
    };
    if !stream {
        let v: Value = resp.json().await.unwrap_or_default();
        let mut last = finish(&v, &name);
        last["response"] = v["content"].clone();
        return Json(last).into_response();
    }
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut end = Value::Null;
        each_event(resp, |ev| {
            if let Some(t) = ev["content"].as_str().filter(|t| !t.is_empty()) {
                let _ = tx.send(json!({ "model": name, "created_at": now(), "response": t, "done": false }));
            }
            if ev["stop"] == true {
                end = ev;
            }
        })
        .await;
        let _ = tx.send(finish(&end, &name));
    });
    ndjson(rx)
}

/// Text turned into numbers, by the documents library's model (whichever
/// model the program names: it's the one here for this).
async fn embed_with(state: &Arc<AppState>, input: Vec<String>) -> Result<Vec<Value>, Response> {
    if !crate::doclib::model_ready(state) {
        return Err(fail(StatusCode::SERVICE_UNAVAILABLE, "Turn on Documents in SulcusAI and add a folder once: that downloads the model that turns text into numbers."));
    }
    let ep = crate::doclib::embeddings_endpoint(state).await.map_err(|e| fail(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    let resp = net::local_client().post(ep.url("/v1/embeddings")).bearer_auth(&ep.key).json(&json!({ "model": "library", "input": input })).send().await;
    let out: Value = match resp {
        Ok(r) => r.json().await.unwrap_or_default(),
        Err(e) => return Err(fail(StatusCode::BAD_GATEWAY, &format!("Couldn't reach the model: {e}"))),
    };
    let mut data: Vec<Value> = out["data"].as_array().cloned().unwrap_or_default();
    data.sort_by_key(|d| d["index"].as_u64().unwrap_or(0));
    Ok(data.into_iter().map(|d| d["embedding"].clone()).collect())
}

async fn embed(State(ctx): State<Ctx>, headers: HeaderMap, Json(req): Json<Value>) -> Response {
    if let Err(r) = guard(&headers, ctx.port) {
        return r;
    }
    let input: Vec<String> = match req.get("input") {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        _ => Vec::new(),
    };
    match embed_with(&ctx.state, input).await {
        Ok(v) => Json(json!({ "model": req.get("model").cloned().unwrap_or(json!("library")), "embeddings": v })).into_response(),
        Err(r) => r,
    }
}

/// The older single-text form.
async fn embed_one(State(ctx): State<Ctx>, headers: HeaderMap, Json(req): Json<Value>) -> Response {
    if let Err(r) = guard(&headers, ctx.port) {
        return r;
    }
    let prompt = req.get("prompt").and_then(Value::as_str).unwrap_or("").to_string();
    match embed_with(&ctx.state, vec![prompt]).await {
        Ok(v) => Json(json!({ "embedding": v.into_iter().next().unwrap_or(json!([])) })).into_response(),
        Err(r) => r,
    }
}

async fn openai_models(State(ctx): State<Ctx>, headers: HeaderMap) -> Response {
    if let Err(r) = guard(&headers, ctx.port) {
        return r;
    }
    let data: Vec<Value> = local_models(&ctx.state).iter().map(|(name, m, _)| json!({ "id": name, "object": "model", "created": m.installed_at / 1000, "owned_by": "library" })).collect();
    Json(json!({ "object": "list", "data": data })).into_response()
}

/// OpenAI's chat shape, passed to the engine as it is (this PC's models).
async fn openai_chat(State(ctx): State<Ctx>, headers: HeaderMap, Json(mut body): Json<Value>) -> Response {
    if let Err(r) = guard(&headers, ctx.port) {
        return r;
    }
    let (name, id) = match find(&ctx.state, body.get("model").and_then(Value::as_str)) {
        Ok(x) => x,
        Err(e) => return fail(StatusCode::NOT_FOUND, &e),
    };
    let pictures = body["messages"].as_array().is_some_and(|m| m.iter().any(|m| m["content"].is_array()));
    let ep = match ready(&ctx.state, &id, pictures).await {
        Ok(ep) => ep,
        Err(r) => return r,
    };
    body["model"] = json!(id);
    if let Some(o) = ep.extra.as_object() {
        for (k, v) in o {
            if body.get(k).is_none() {
                body[k] = v.clone();
            }
        }
    }
    let streaming = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let resp = match net::local_client().post(ep.url("/v1/chat/completions")).bearer_auth(&ep.key).json(&body).send().await {
        Ok(r) => r,
        Err(e) => return fail(StatusCode::BAD_GATEWAY, &format!("Couldn't reach the engine: {e}")),
    };
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    *ctx.state.engine_used.lock().unwrap() = std::time::Instant::now();
    if streaming && status.is_success() {
        return Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from_stream(resp.bytes_stream()))
            .unwrap_or_else(|_| fail(StatusCode::INTERNAL_SERVER_ERROR, "Couldn't stream the reply."));
    }
    let mut out: Value = resp.json().await.unwrap_or_default();
    if out.get("model").is_some() {
        out["model"] = json!(name);
    }
    (status, Json(out)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(host: &str, origin: Option<&str>) -> HeaderMap {
        let mut m = HeaderMap::new();
        m.insert(header::HOST, host.parse().unwrap());
        if let Some(o) = origin {
            m.insert(header::ORIGIN, o.parse().unwrap());
        }
        m
    }

    /// Programs on this PC, yes; web pages and other hosts, no.
    #[test]
    fn only_programs_on_this_pc() {
        assert!(guard(&h("localhost:11434", None), 11434).is_ok());
        assert!(guard(&h("127.0.0.1:11434", None), 11434).is_ok());
        assert!(guard(&h("127.0.0.1", None), 11434).is_ok());
        assert!(guard(&h("127.0.0.1:11434", Some("http://localhost:3000")), 11434).is_ok());
        assert!(guard(&h("evil.example:11434", None), 11434).is_err(), "DNS rebinding");
        assert!(guard(&h("127.0.0.1:11434", Some("https://evil.example")), 11434).is_err(), "a web page");
        assert!(guard(&h("127.0.0.1:11434", Some("http://localhost.evil.example")), 11434).is_err());
        assert!(guard(&h("127.0.0.1:11434", Some("null")), 11434).is_err(), "a sandboxed page or a file");
    }

    #[test]
    fn names_requests_and_options_translate() {
        assert_eq!(slug("Qwen3 VL 4B"), "qwen3-vl-4b");
        assert_eq!(slug("Llama 3.2 3B (uncensored)"), "llama-3.2-3b-uncensored");
        let m = openai_messages(&[
            json!({ "role": "user", "content": "what's this?", "images": ["iVBORw0KGgo="] }),
            json!({ "role": "assistant", "content": "", "tool_calls": [{ "function": { "name": "look", "arguments": { "at": 1 } } }] }),
            json!({ "role": "tool", "content": "done", "tool_name": "look" }),
        ]);
        assert_eq!(m[0]["content"][1]["image_url"]["url"], "data:image/png;base64,iVBORw0KGgo=");
        assert_eq!(m[1]["tool_calls"][0]["function"]["arguments"], "{\"at\":1}");
        assert_eq!(m[2]["name"], "look");
        let mut body = json!({});
        apply_options(&mut body, &json!({ "format": { "type": "object" }, "options": { "temperature": 0.2, "num_predict": 64, "num_ctx": 8192 }, "think": false }), &json!({ "temperature": 0.7, "top_k": 40 }));
        assert_eq!(body["response_format"]["json_schema"]["schema"]["type"], "object");
        assert_eq!(body["temperature"], 0.2, "the program's own settings win");
        assert_eq!(body["top_k"], 40);
        assert_eq!(body["max_tokens"], 64);
        assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);
        assert_eq!(ollama_calls(&[json!({ "function": { "name": "f", "arguments": "{\"a\":2}" } })])[0]["function"]["arguments"]["a"], 2);
    }
}
