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
    if s.key.is_empty() {
        s.key = new_key();
        let _ = db::set(conn, KEY, &s);
    }
    s
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
    Router::new().route("/v1/models", get(models)).route("/v1/chat/completions", post(chat)).with_state(ctx)
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

async fn models(State(ctx): State<Ctx>, headers: HeaderMap) -> Response {
    if let Err(r) = check(&headers, &ctx.key, ctx.port) {
        return r;
    }
    let installed = db::installed_models(&ctx.state.db.lock().unwrap());
    let data: Vec<Value> = installed
        .iter()
        .map(|m| {
            let name = ctx.state.model_spec(&m.model_id).map_or(m.model_id.clone(), |s| s.name);
            json!({ "id": m.model_id, "object": "model", "owned_by": "sulcusai", "name": name, "created": m.installed_at / 1000 })
        })
        .collect();
    Json(json!({ "object": "list", "data": data })).into_response()
}

/// The installed model a request names (by id or name), or the default.
fn pick_model(state: &AppState, asked: Option<&str>) -> Result<String, String> {
    let installed = db::installed_models(&state.db.lock().unwrap());
    let asked = asked.map(str::trim).filter(|a| !a.is_empty() && *a != "default");
    let Some(asked) = asked else {
        return db::settings(&state.db.lock().unwrap()).default_model.ok_or_else(|| "No model is installed in SulcusAI yet.".to_string());
    };
    installed
        .iter()
        .find(|m| m.model_id == asked || state.model_spec(&m.model_id).is_some_and(|s| s.name.eq_ignore_ascii_case(asked)))
        .map(|m| m.model_id.clone())
        .ok_or_else(|| format!("No installed model is called “{asked}”. GET /v1/models lists them."))
}

async fn chat(State(ctx): State<Ctx>, headers: HeaderMap, Json(mut body): Json<Value>) -> Response {
    if let Err(r) = check(&headers, &ctx.key, ctx.port) {
        return r;
    }
    let state = &ctx.state;
    let id = match pick_model(state, body.get("model").and_then(Value::as_str)) {
        Ok(id) => id,
        Err(e) => return error(StatusCode::NOT_FOUND, &e),
    };
    // Don't swap the model out from under a chat that's replying in the app.
    let loaded = state.engine.lock().await.status().model_id;
    if loaded.as_deref() != Some(id.as_str()) && !state.generations.lock().unwrap().is_empty() {
        return error(StatusCode::CONFLICT, "SulcusAI is busy replying with another model. Try again in a moment, or use the model that's loaded.");
    }
    let (ep, spec, _) = match crate::llm_endpoint(state, Some(id.clone()), false, || {}).await {
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

#[tauri::command]
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
        db::set(&conn, KEY, &s)?;
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
#[tauri::command]
pub fn new_api_key(state: AppStateRef) -> Result<ApiServerView, String> {
    {
        let conn = state.db.lock().unwrap();
        let mut s = settings(&conn);
        s.key = new_key();
        db::set(&conn, KEY, &s)?;
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
}
