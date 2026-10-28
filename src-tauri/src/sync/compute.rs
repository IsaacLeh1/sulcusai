// SPDX-License-Identifier: AGPL-3.0-only
//! A compute PC: one of the user's paired PCs (a server built for AI) does
//! the heavy work while this one keeps its chats, files and screen.
//!
//! - Chats: the compute PC's models are offered here as "compute:<id>".
//!   Picking one starts a small relay on this PC (127.0.0.1, with a key of
//!   its own) that the chat talks to as it would to a model here; each
//!   request goes over the paired, encrypted link, the compute PC runs it on
//!   its own engine (waiting its turn like its own chats) and the answer
//!   streams back. Tools, files and memories stay on this PC.
//! - Pictures, video, songs and narration: with "make them on the compute
//!   PC" on, a job that needs nothing kept only here is sent there; its
//!   progress shows here and what it makes is saved here, then removed there.
//! - Its health: temperatures, use, memory, every drive's space, how long
//!   it's been up, what it's doing; Stop all, pause and resume, unloading
//!   the model and its performance mode, from here.
//!
//! Only a paired PC can ask any of this, and only while the compute PC is
//! unlocked.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

use super::wire::{self, Sealer};
use super::Peer;
use crate::{db, AppState, AppStateRef};

/// Model ids run on the compute PC.
pub const PREFIX: &str = "compute:";
const KEY: &str = "compute";
/// Media bytes go in pieces of this size.
const PIECE: usize = 4 << 20;

pub fn is_compute(id: &str) -> bool {
    id.starts_with(PREFIX)
}

/// Which paired PC does the work, and what it does.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub peer: Option<String>,
    /// Pictures, video, songs and narration are made there.
    pub media: bool,
}

fn settings(conn: &rusqlite::Connection) -> Settings {
    db::get(conn, KEY).unwrap_or_default()
}

fn the_peer(state: &AppState) -> Result<(Peer, SocketAddr), String> {
    let id = settings(&state.db.lock().unwrap()).peer.ok_or("No compute PC is linked: pick one in Settings › Your devices.")?;
    let p = super::peer(state, &id).ok_or("The compute PC isn't paired anymore.")?;
    let addr = super::seen_addr(&p.id).or_else(|| p.addr.as_deref()?.parse().ok()).ok_or_else(|| format!("{} isn't on this network right now.", p.name))?;
    Ok((p, addr))
}

// ---------- the compute PC's side ----------

static PAUSED: AtomicBool = AtomicBool::new(false);
static STARTED: LazyLock<Instant> = LazyLock::new(Instant::now);

/// New AI work waits while the person paused this PC from their other one.
pub fn paused() -> bool {
    PAUSED.load(Ordering::Relaxed)
}

/// Marks when the app started (for its uptime).
pub fn started() {
    LazyLock::force(&STARTED);
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Drive {
    pub name: String,
    pub mount: String,
    pub total: u64,
    pub free: u64,
    /// Where SulcusAI keeps its models.
    pub models: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RemoteModel {
    pub id: String,
    pub name: String,
    pub ctx: u32,
    pub tools: bool,
    pub vision: bool,
}

/// How the compute PC is doing.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Status {
    pub name: String,
    pub version: String,
    /// The PC tab's reading: processor, memory, graphics card, temperatures, jobs.
    pub pc: Value,
    pub drives: Vec<Drive>,
    pub uptime_s: u64,
    pub app_uptime_s: u64,
    pub paused: bool,
    pub models: Vec<RemoteModel>,
    pub default_model: Option<String>,
    /// Media models installed there, by name.
    pub media: Vec<String>,
    /// Its performance mode ("cool", "balanced", "turbo").
    pub perf: String,
}

fn drives(state: &AppState) -> Vec<Drive> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let models = state.paths.models.to_string_lossy().to_lowercase();
    let mut out: Vec<Drive> = disks
        .list()
        .iter()
        .filter(|d| d.total_space() > 0)
        .map(|d| {
            let mount = d.mount_point().to_string_lossy().to_string();
            Drive { name: d.name().to_string_lossy().to_string(), models: models.starts_with(&mount.to_lowercase()), mount, total: d.total_space(), free: d.available_space() }
        })
        .collect();
    out.sort_by(|a, b| a.mount.cmp(&b.mount));
    out.dedup_by(|a, b| a.mount == b.mount);
    out
}

pub(super) async fn status(state: &Arc<AppState>) -> Result<Status, String> {
    let pc = serde_json::to_value(crate::pcstatus::status(state).await?).map_err(|e| e.to_string())?;
    let (installed, default, perf) = {
        let conn = state.db.lock().unwrap();
        (db::installed_models(&conn), db::settings(&conn).default_model, crate::perf::settings(&conn).mode)
    };
    let models = installed
        .iter()
        .filter_map(|i| {
            let m = state.catalog.model(&i.model_id)?;
            Some(RemoteModel { id: m.id.clone(), name: m.name.clone(), ctx: m.default_ctx.max(4096), tools: m.tools, vision: m.vision.is_some() })
        })
        .collect();
    let media = crate::media::installed(&state.db.lock().unwrap()).into_iter().filter_map(|i| crate::media::models(state).into_iter().find(|m| m.id == i.model_id).map(|m| m.name)).collect();
    Ok(Status {
        name: std::env::var("COMPUTERNAME").unwrap_or_default(),
        version: env!("CARGO_PKG_VERSION").into(),
        pc,
        drives: drives(state),
        uptime_s: sysinfo::System::uptime(),
        app_uptime_s: STARTED.elapsed().as_secs(),
        paused: paused(),
        models,
        default_model: default.filter(|d| !crate::cloud::is_cloud(d)),
        media,
        perf: serde_json::to_value(perf).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default(),
    })
}

/// Does what the other PC asked: stop, pause, resume, unload, a mode.
pub(super) async fn control(state: &Arc<AppState>, ask: &Value) -> Result<String, String> {
    let action = ask.get("action").and_then(Value::as_str).unwrap_or("");
    let value = ask.get("value").and_then(Value::as_str).unwrap_or("");
    let said = match action {
        "stop_all" => format!("Stopped {} things and freed the graphics card.", crate::pcstatus::stop_everything(state).await?),
        "pause" => {
            PAUSED.store(true, Ordering::Relaxed);
            "Paused: new work waits until it's resumed.".into()
        }
        "resume" => {
            PAUSED.store(false, Ordering::Relaxed);
            "Resumed.".into()
        }
        "unload" => {
            state.engine.lock().await.stop().await;
            "The chat model was unloaded.".into()
        }
        "perf" => {
            let mode: crate::perf::Mode = serde_json::from_value(json!(value)).map_err(|_| "Unknown mode.".to_string())?;
            let conn = state.db.lock().unwrap();
            let mut s = crate::perf::settings(&conn);
            s.mode = mode;
            db::set(&conn, "perf", &s)?;
            format!("Performance set to {value}.")
        }
        _ => return Err("Unknown request.".into()),
    };
    state.log("compute", &format!("Asked from a linked PC: {action}"));
    Ok(said)
}

/// Runs a chat request from the other PC on this one's engine, streaming
/// the answer back as it comes.
pub(super) async fn serve_chat(state: &Arc<AppState>, ask: Value, w: &mut OwnedWriteHalf, out: &mut Sealer) -> Result<(), String> {
    let mut body = ask.get("body").cloned().unwrap_or(Value::Null);
    let model = ask.get("model").and_then(Value::as_str).filter(|m| !m.is_empty()).map(str::to_string);
    while paused() {
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let pictures = body.get("messages").and_then(Value::as_array).is_some_and(|m| m.iter().any(|x| x.get("content").and_then(Value::as_array).is_some_and(|c| c.iter().any(|p| p.get("type").and_then(Value::as_str) == Some("image_url")))));
    let started = crate::llm_endpoint(state, model, pictures, |_| {}).await;
    let (ep, spec, _) = match started {
        Ok(e) if e.0.cloud.is_none() => e,
        Ok(_) => return send_error(w, out, "The compute PC's model is a cloud model; pick one of its own.").await,
        Err(e) => return send_error(w, out, &e).await,
    };
    let _hold = crate::engine::Hold::new(&ep.key);
    if let Some(b) = body.as_object_mut() {
        b.remove("model");
    }
    let resp = crate::net::local_client().post(ep.url("/v1/chat/completions")).bearer_auth(&ep.key).json(&body).send().await.map_err(|e| e.to_string())?;
    let head = json!({ "status": resp.status().as_u16(), "ctx": ep.ctx, "model": spec.name });
    wire::write_frame(w, &out.seal(&serde_json::to_vec(&head).map_err(|e| e.to_string())?)).await.map_err(|e| e.to_string())?;
    let mut resp = resp;
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        if chunk.is_empty() {
            continue;
        }
        // The other PC went away: the answer stops with it.
        wire::write_frame(w, &out.seal(&chunk)).await.map_err(|e| e.to_string())?;
    }
    wire::write_frame(w, &out.seal(&[])).await.map_err(|e| e.to_string())?;
    Ok(())
}

async fn send_error(w: &mut OwnedWriteHalf, out: &mut Sealer, why: &str) -> Result<(), String> {
    let head = json!({ "status": 503, "error": why });
    wire::write_frame(w, &out.seal(&serde_json::to_vec(&head).map_err(|e| e.to_string())?)).await.map_err(|e| e.to_string())?;
    wire::write_frame(w, &out.seal(&[])).await.map_err(|e| e.to_string())
}

/// What the other PC sends with a job: the request, and the item it works
/// on (a picture to edit) when there is one.
#[derive(Debug, Serialize, Deserialize)]
struct MediaAsk {
    request: crate::media::Request,
    source: Option<SourceItem>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SourceItem {
    item: crate::media::MediaItem,
    /// Base64.
    data: String,
}

async fn send_json(w: &mut OwnedWriteHalf, out: &mut Sealer, v: &Value) -> Result<(), String> {
    wire::write_frame(w, &out.seal(&serde_json::to_vec(v).map_err(|e| e.to_string())?)).await.map_err(|e| e.to_string())
}

async fn send_bytes(w: &mut OwnedWriteHalf, out: &mut Sealer, data: &[u8]) -> Result<(), String> {
    for piece in data.chunks(PIECE) {
        wire::write_frame(w, &out.seal(piece)).await.map_err(|e| e.to_string())?;
    }
    wire::write_frame(w, &out.seal(&[])).await.map_err(|e| e.to_string())
}

/// Runs a job for the other PC: its progress goes back as it changes, then
/// each thing made with its bytes; nothing of it is kept here.
pub(super) async fn serve_media(state: &Arc<AppState>, ask: Value, w: &mut OwnedWriteHalf, out: &mut Sealer) -> Result<(), String> {
    let ask: MediaAsk = serde_json::from_value(ask).map_err(|e| e.to_string())?;
    let c = state.work_cipher()?;
    let mut req = ask.request;
    req.chat_id = None;
    let mut temp = Vec::new();
    if let Some(src) = ask.source {
        let bytes = base64::engine::general_purpose::STANDARD.decode(src.data).map_err(|e| e.to_string())?;
        let item = crate::media::MediaItem { id: String::new(), hidden: true, chat_id: None, parent: None, ..src.item };
        let saved = crate::media::store::save(&state.db.lock().unwrap(), &state.paths, &c, item, &bytes, None)?;
        req.source = Some(saved.id.clone());
        temp.push(saved.id);
    }
    while paused() {
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let id = crate::media::store::new_id();
    let st = state.clone();
    let job_id = id.clone();
    let mut job = tokio::spawn(async move { crate::media::run(&st, job_id, req).await });
    let mut last = String::new();
    let result = loop {
        tokio::select! {
            r = &mut job => break r.map_err(|e| e.to_string()).and_then(|r| r),
            _ = tokio::time::sleep(Duration::from_millis(500)) => {
                if let Some(j) = crate::media::list_jobs().into_iter().find(|j| j.id == id) {
                    let now = format!("{}|{:.3}", j.stage, j.fraction);
                    if now != last {
                        last = now;
                        let sent = send_json(w, out, &json!({ "t": "progress", "stage": j.stage, "fraction": j.fraction, "eta": j.eta_secs, "device": j.device })).await;
                        // The other PC gave up on it: so does this one.
                        if sent.is_err() {
                            crate::media::cancel(&id);
                        }
                    }
                }
            }
        }
    };
    let outcome = match result {
        Ok(items) => {
            for item in &items {
                let data = crate::media::store::read(&state.paths, &c, &item.id)?;
                let thumb = crate::media::store::read_thumb(&state.paths, &c, &item.id).ok();
                send_json(w, out, &json!({ "t": "item", "item": item, "thumb": thumb.is_some() })).await?;
                send_bytes(w, out, &data).await?;
                if let Some(t) = thumb {
                    send_bytes(w, out, &t).await?;
                }
                temp.push(item.id.clone());
            }
            send_json(w, out, &json!({ "t": "done" })).await
        }
        Err(e) => send_json(w, out, &json!({ "t": "error", "error": e })).await,
    };
    let conn = state.db.lock().unwrap();
    for t in temp {
        let _ = crate::media::store::delete(&conn, &state.paths, &t);
    }
    outcome
}

// ---------- this PC's side ----------

async fn ask_json(state: &Arc<AppState>, want: &str, ask: &Value) -> Result<Value, String> {
    let (p, addr) = the_peer(state)?;
    let (mut r, _w, mut inn, _out) = super::request(state, &p, addr, want, ask).await?;
    let frame = tokio::time::timeout(Duration::from_secs(30), wire::read_frame(&mut r)).await.map_err(|_| format!("{} didn't answer.", p.name))?.map_err(|e| e.to_string())?;
    serde_json::from_slice(&inn.open(&frame)?).map_err(|e| e.to_string())
}

/// The last status seen, for model sizes between asks.
static LAST: Mutex<Option<Status>> = Mutex::new(None);

pub async fn remote_status(state: &Arc<AppState>) -> Result<Status, String> {
    let v = ask_json(state, "compute.status", &Value::Null).await?;
    if let Some(e) = v.get("error").and_then(Value::as_str) {
        return Err(e.to_string());
    }
    let s: Status = serde_json::from_value(v).map_err(|e| e.to_string())?;
    *LAST.lock().unwrap() = Some(s.clone());
    Ok(s)
}

/// The chat relay for one of the compute PC's models: its port and key.
struct Relay {
    port: u16,
    key: String,
}

static RELAYS: LazyLock<Mutex<HashMap<String, Relay>>> = LazyLock::new(Default::default);

#[derive(Clone)]
struct RelayState {
    app: Arc<AppState>,
    model: String,
    key: String,
}

async fn relay_health() -> &'static str {
    "{\"status\":\"ok\"}"
}

async fn relay_chat(State(rs): State<RelayState>, headers: HeaderMap, body: Bytes) -> Response {
    let ok = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).is_some_and(|v| v == format!("Bearer {}", rs.key));
    if !ok {
        return (StatusCode::UNAUTHORIZED, "").into_response();
    }
    let Ok(body) = serde_json::from_slice::<Value>(&body) else { return (StatusCode::BAD_REQUEST, "").into_response() };
    let streaming = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let opened = async {
        let (p, addr) = the_peer(&rs.app)?;
        let (mut r, w, mut inn, out) = super::request(&rs.app, &p, addr, "compute.chat", &json!({ "model": rs.model, "body": body })).await?;
        let head: Value = serde_json::from_slice(&inn.open(&wire::read_frame(&mut r).await.map_err(|e| e.to_string())?)?).map_err(|e| e.to_string())?;
        Ok::<_, String>((r, w, inn, out, head))
    }
    .await;
    let (r, w, inn, out, head) = match opened {
        Ok(o) => o,
        Err(e) => return (StatusCode::SERVICE_UNAVAILABLE, json!({ "error": { "message": e } }).to_string()).into_response(),
    };
    if let Some(e) = head.get("error").and_then(Value::as_str) {
        return (StatusCode::SERVICE_UNAVAILABLE, json!({ "error": { "message": e } }).to_string()).into_response();
    }
    let code = StatusCode::from_u16(head.get("status").and_then(Value::as_u64).unwrap_or(200) as u16).unwrap_or(StatusCode::OK);
    // The answer's pieces, as the compute PC sends them; the link's writing
    // half is kept with it so the other PC sees this side still listening.
    let stream = futures_util::stream::unfold((r, inn, w, out, false), |(mut r, mut inn, w, out, done)| async move {
        if done {
            return None;
        }
        let frame = wire::read_frame(&mut r).await.ok()?;
        let piece = inn.open(&frame).ok()?;
        if piece.is_empty() {
            return None;
        }
        Some((Ok::<_, std::io::Error>(Bytes::from(piece)), (r, inn, w, out, false)))
    });
    Response::builder()
        .status(code)
        .header(header::CONTENT_TYPE, if streaming { "text/event-stream" } else { "application/json" })
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// The relay for `model` (a compute PC's model id), started on first use.
async fn relay(state: &Arc<AppState>, model: &str) -> Result<(u16, String), String> {
    if let Some(r) = RELAYS.lock().unwrap().get(model) {
        return Ok((r.port, r.key.clone()));
    }
    let key = uuid::Uuid::new_v4().simple().to_string();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let rs = RelayState { app: state.clone(), model: model.to_string(), key: key.clone() };
    let router = Router::new().route("/health", get(relay_health)).route("/v1/chat/completions", post(relay_chat)).with_state(rs);
    tauri::async_runtime::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    RELAYS.lock().unwrap().insert(model.to_string(), Relay { port, key: key.clone() });
    Ok((port, key))
}

/// A chat endpoint for a compute PC's model ("compute:<id>"): this PC's relay to it.
pub async fn endpoint(state: &Arc<AppState>, id: &str) -> Result<(crate::engine::Endpoint, crate::catalog::ModelSpec, db::InstalledModel), String> {
    let model = id.strip_prefix(PREFIX).ok_or("Not a compute PC's model.")?;
    let cached = LAST.lock().unwrap().clone();
    let status = match cached {
        Some(s) => s,
        None => remote_status(state).await?,
    };
    let known = status.models.iter().find(|m| m.id == model).cloned().ok_or_else(|| format!("{} doesn't have that model anymore.", status.name))?;
    let (port, key) = relay(state, model).await?;
    let ep = crate::engine::Endpoint { port, key, ctx: known.ctx, extra: crate::advanced::get(state).request_extra(), cloud: None };
    let mut spec = state.catalog.model(model).cloned().ok_or("That model isn't one SulcusAI knows.")?;
    spec.id = id.to_string();
    spec.name = format!("{} (on {})", known.name, status.name);
    spec.default_ctx = known.ctx;
    let installed = db::InstalledModel { model_id: id.to_string(), quant: "compute".into(), path: String::new(), size: 0, installed_at: 0, tps: None, prompt_tps: None };
    Ok((ep, spec, installed))
}

/// Whether a Studio job goes to the compute PC: it's on for media, and the
/// job needs nothing that's only on this PC (recorded voices, files picked
/// here, songs to work on, custom models).
pub fn media_here(state: &AppState, req: &crate::media::Request) -> bool {
    let s = settings(&state.db.lock().unwrap());
    if !s.media || s.peer.is_none() {
        return false;
    }
    let music_local = req.music.as_ref().is_some_and(|m| m.audio.is_some() || m.own_voice.is_some() || m.custom.as_deref().is_some_and(|c| !c.is_empty()) || m.persona.is_some() || m.task.as_deref().is_some_and(|t| t != "new"));
    let own_voice = req.voice.as_deref().is_some_and(|v| v.starts_with("own:")) || req.talk.as_ref().is_some_and(|t| t.speakers.iter().any(|s| s.voice.as_deref().is_some_and(|v| v.starts_with("own:"))) || !t.files.is_empty());
    let ok_op = matches!(req.op.as_str(), "generate" | "edit" | "fill" | "extend" | "restyle" | "upscale" | "remove_background" | "video" | "music" | "sound" | "narrate" | "podcast" | "dialogue" | "explainer");
    ok_op && !music_local && !own_voice && req.examples.is_empty() && req.train.is_none() && req.wallpaper.is_none()
}

/// A Studio job made on the compute PC: `progress` as it goes, then what it
/// made (item, bytes, thumbnail) to keep here.
pub async fn media(state: &Arc<AppState>, req: &crate::media::Request, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64, Option<String>) + Sync)) -> Result<Vec<(crate::media::MediaItem, Vec<u8>, Option<Vec<u8>>)>, String> {
    let (p, addr) = the_peer(state)?;
    let c = state.work_cipher()?;
    let source = match req.source.as_deref() {
        Some(id) => {
            let item = crate::media::store::get(&state.db.lock().unwrap(), &c, id).ok_or("The item to work on is gone.")?;
            let data = crate::media::store::read(&state.paths, &c, id)?;
            Some(SourceItem { item, data: base64::engine::general_purpose::STANDARD.encode(data) })
        }
        None => None,
    };
    let ask = serde_json::to_value(MediaAsk { request: req.clone(), source }).map_err(|e| e.to_string())?;
    let (mut r, _w, mut inn, _out) = super::request(state, &p, addr, "compute.media", &ask).await?;
    let mut made = Vec::new();
    loop {
        let frame = tokio::select! {
            f = wire::read_frame(&mut r) => f.map_err(|_| format!("The link to {} was lost.", p.name))?,
            _ = async { while !cancel.load(Ordering::Relaxed) { tokio::time::sleep(Duration::from_millis(250)).await } } => {
                // Dropping the link makes the compute PC stop it.
                return Err(crate::download::CANCELLED.into());
            }
        };
        let msg: Value = serde_json::from_slice(&inn.open(&frame)?).map_err(|e| e.to_string())?;
        match msg.get("t").and_then(Value::as_str) {
            Some("progress") => progress(msg["stage"].as_str().unwrap_or(""), msg["fraction"].as_f64().unwrap_or(0.0), msg["device"].as_str().map(str::to_string)),
            Some("item") => {
                let item: crate::media::MediaItem = serde_json::from_value(msg["item"].clone()).map_err(|e| e.to_string())?;
                let data = read_bytes(&mut r, &mut inn).await?;
                let thumb = if msg["thumb"].as_bool() == Some(true) { Some(read_bytes(&mut r, &mut inn).await?) } else { None };
                made.push((item, data, thumb));
            }
            Some("done") => return Ok(made),
            Some("error") => return Err(msg["error"].as_str().unwrap_or("The compute PC couldn't make it.").to_string()),
            _ => return Err("The compute PC sent something unexpected.".into()),
        }
    }
}

async fn read_bytes(r: &mut OwnedReadHalf, inn: &mut Sealer) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let piece = inn.open(&wire::read_frame(r).await.map_err(|e| e.to_string())?)?;
        if piece.is_empty() {
            return Ok(out);
        }
        out.extend(piece);
    }
}

/// The compute PC's name, for showing where a job runs.
pub fn peer_name(state: &AppState) -> Option<String> {
    the_peer(state).ok().map(|(p, _)| p.name)
}

// ---------- commands ----------

#[derive(Serialize)]
pub struct ComputeView {
    settings: Settings,
    /// Paired PCs that could be it: (id, name).
    peers: Vec<(String, String)>,
}

#[tauri::command]
pub fn compute_settings(state: AppStateRef<'_>) -> ComputeView {
    let conn = state.db.lock().unwrap();
    ComputeView { settings: settings(&conn), peers: super::config(&conn).peers.into_iter().map(|p| (p.id, p.name)).collect() }
}

#[tauri::command]
pub fn compute_set(state: AppStateRef<'_>, settings: Settings) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    if let Some(id) = &settings.peer {
        if !super::config(&conn).peers.iter().any(|p| &p.id == id) {
            return Err("That PC isn't paired.".into());
        }
    }
    db::set(&conn, KEY, &settings)?;
    db::log_action(&conn, "compute", if settings.peer.is_some() { "Linked a compute PC" } else { "Unlinked the compute PC" });
    drop(conn);
    *LAST.lock().unwrap() = None;
    Ok(())
}

#[tauri::command]
pub async fn compute_status(state: AppStateRef<'_>) -> Result<Status, String> {
    let state = state.inner().clone();
    remote_status(&state).await
}

#[tauri::command]
pub async fn compute_control(state: AppStateRef<'_>, action: String, value: Option<String>) -> Result<String, String> {
    let state = state.inner().clone();
    let v = ask_json(&state, "compute.control", &json!({ "action": action, "value": value })).await?;
    match v.get("error").and_then(Value::as_str) {
        Some(e) => Err(e.to_string()),
        None => Ok(v.get("said").and_then(Value::as_str).unwrap_or("Done.").to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two PCs on this machine: A, the compute PC, has a real model
    /// (SULCUSAI_TEST_MODEL_ID, its file SULCUSAI_TEST_GGUF); B pairs with it,
    /// reads its health, pauses and resumes it, and chats through it.
    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore]
    async fn e2e_compute_pc() {
        use std::net::Ipv4Addr;
        let a = crate::testutil::temp_state(None);
        let b = crate::testutil::scratch_state();
        super::super::update(&a, |c| c.name = "SERVER".into()).unwrap();
        let model = std::env::var("SULCUSAI_TEST_MODEL_ID").unwrap_or_else(|_| "qwen3-4b-2507".into());
        let gguf = std::path::PathBuf::from(std::env::var("SULCUSAI_TEST_GGUF").expect("SULCUSAI_TEST_GGUF"));
        {
            let conn = a.db.lock().unwrap();
            db::save_installed(&conn, &db::InstalledModel { model_id: model.clone(), quant: "Q4_K_M".into(), path: gguf.display().to_string(), size: std::fs::metadata(&gguf).unwrap().len(), installed_at: db::now_ms(), tps: None, prompt_tps: None }).unwrap();
        }
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let sa = a.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let s = sa.clone();
                tokio::spawn(async move {
                    if let Err(e) = super::super::answer(&s, stream).await {
                        eprintln!("the compute PC answered: {e}");
                    }
                });
            }
        });
        *super::super::PAIRING.lock().unwrap() = Some(super::super::Pairing { code: "K7Q29XMB".into(), until: Instant::now() + super::super::CODE_FOR, tries: 0 });
        assert_eq!(super::super::pair_call(&b, "K7Q29XMB", addr).await.unwrap(), "SERVER");
        let a_id = super::super::device_id(&a.paths.data);
        db::set(&b.db.lock().unwrap(), KEY, &Settings { peer: Some(a_id), media: false }).unwrap();

        let s = remote_status(&b).await.unwrap();
        println!("{} ({}): up {} s, {} drives, models {:?}, cpu {:?}", s.name, s.version, s.uptime_s, s.drives.len(), s.models.iter().map(|m| &m.id).collect::<Vec<_>>(), s.pc.get("cpu"));
        assert!(!s.drives.is_empty() && s.drives.iter().all(|d| d.total >= d.free));
        assert!(s.models.iter().any(|m| m.id == model));

        let v = ask_json(&b, "compute.control", &json!({ "action": "pause" })).await.unwrap();
        println!("pause: {v}");
        assert!(remote_status(&b).await.unwrap().paused);
        ask_json(&b, "compute.control", &json!({ "action": "resume" })).await.unwrap();
        assert!(!remote_status(&b).await.unwrap().paused);

        let started = Instant::now();
        let (ep, spec, _) = crate::llm_endpoint(&b, Some(format!("{PREFIX}{model}")), false, |_| {}).await.unwrap();
        let said = crate::chat::complete(&ep, vec![json!({"role": "user", "content": "What is 12 plus 30? Reply with just the number."})], json!({"temperature": 0}), 20).await.unwrap();
        println!("{} said “{said}” in {:.1} s (loading included)", spec.name, started.elapsed().as_secs_f64());
        assert!(said.contains("42"));
        // Streaming, as chats ask.
        let body = json!({ "messages": [{"role": "user", "content": "Count from 1 to 5, separated by commas."}], "stream": true, "max_tokens": 40, "temperature": 0 });
        let resp = crate::net::local_client().post(ep.url("/v1/chat/completions")).bearer_auth(&ep.key).json(&body).send().await.unwrap();
        let text = resp.text().await.unwrap();
        println!("streamed {} bytes, {} events", text.len(), text.matches("data:").count());
        assert!(text.matches("data:").count() > 3 && text.contains("[DONE]"));
        a.engine.lock().await.stop().await;
    }

    #[test]
    fn only_jobs_that_need_nothing_from_here_go() {
        let state = crate::testutil::temp_state(None);
        let req = crate::media::Request { op: "generate".into(), prompt: "a cat".into(), ..Default::default() };
        assert!(!media_here(&state, &req), "off by default");
        db::set(&state.db.lock().unwrap(), KEY, &Settings { peer: Some("p".into()), media: true }).unwrap();
        assert!(media_here(&state, &req));
        let own = crate::media::Request { op: "narrate".into(), voice: Some("own:abc".into()), ..Default::default() };
        assert!(!media_here(&state, &own), "a recorded voice is only here");
        let examples = crate::media::Request { examples: vec!["x".into()], ..req.clone() };
        assert!(!media_here(&state, &examples));
        assert!(!media_here(&state, &crate::media::Request { op: "train".into(), ..Default::default() }));
        assert!(is_compute("compute:qwen3-8b") && !is_compute("qwen3-8b"));
    }
}
