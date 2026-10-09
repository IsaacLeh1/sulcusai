// SPDX-License-Identifier: AGPL-3.0-only
//! Browser control: the assistant using tabs in the user's own Chrome or
//! Edge, through the SulcusAI extension (the Browser control feature).
//!
//! extension ⇄ (native messaging) ⇄ this program in connector mode ⇄
//! (named pipe) ⇄ the running app.
//!
//! - No debugging port: the browser starts the connector itself, and only
//!   for our extension's ID (the host manifest's allowed_origins).
//! - The pipe has a random name, refuses other computers, and serves only a
//!   connector that sends the per-run token from a file in the user's own
//!   folder.
//! - The page rules match the built-in browser: confirm before submitting,
//!   buying or sending; never type passwords or card numbers.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

use crate::features::Feature;
use crate::{AppState, AppStateRef};

pub const HOST_NAME: &str = "app.sulcusai.bridge";
/// Fixed by the public key in the extension's manifest.
pub const EXTENSION_ID: &str = "inkpcpcjmnholkpjadbgaiilchjfjopg";

const FILES: &[(&str, &[u8])] = &[
    ("manifest.json", include_bytes!("../../extension/manifest.json")),
    ("background.js", include_bytes!("../../extension/background.js")),
    ("page.js", include_bytes!("../../extension/page.js")),
    ("icon.png", include_bytes!("../../extension/icon.png")),
];

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

struct Conn {
    tx: mpsc::UnboundedSender<String>,
    pending: Pending,
    serial: u64,
}

/// The app's end of the connection.
pub struct Bridge {
    conn: Mutex<Option<Conn>>,
    next: AtomicU64,
    serving: AtomicBool,
    version: Mutex<Option<String>>,
    token: String,
    pipe: String,
}

impl Default for Bridge {
    fn default() -> Self {
        Bridge {
            conn: Mutex::new(None),
            next: AtomicU64::new(1),
            serving: AtomicBool::new(false),
            version: Mutex::new(None),
            token: uuid::Uuid::new_v4().simple().to_string() + &uuid::Uuid::new_v4().simple().to_string(),
            pipe: format!(r"\\.\pipe\sulcusai-{}", uuid::Uuid::new_v4().simple()),
        }
    }
}

impl Bridge {
    /// The extension has said hello on the current connection.
    pub fn connected(&self) -> bool {
        self.conn.lock().unwrap().is_some() && self.version.lock().unwrap().is_some()
    }

    /// Sends a request to the extension and waits for its answer.
    pub async fn call(&self, mut req: Value) -> Result<Value, String> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        req["id"] = json!(id);
        let (tx, rx) = oneshot::channel();
        {
            let conn = self.conn.lock().unwrap();
            let c = conn.as_ref().ok_or("The SulcusAI extension isn't connected. Open Chrome or Edge with the extension turned on.")?;
            c.pending.lock().unwrap().insert(id, tx);
            c.tx.send(req.to_string()).map_err(|_| "The extension disconnected.".to_string())?;
        }
        tokio::time::timeout(Duration::from_secs(40), rx)
            .await
            .map_err(|_| "The browser didn't answer.".to_string())?
            .map_err(|_| "The extension disconnected.".to_string())?
    }
}

/// Where the connector looks for the pipe and token, and where the host
/// manifest lives: a per-user folder both sides can find.
fn bridge_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("SulcusAI-bridge")
}

// ---------- installing ----------

#[cfg(windows)]
fn set_registry(enable: bool, manifest: &std::path::Path) -> Result<(), String> {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::*;
    for browser in [r"Software\Google\Chrome", r"Software\Microsoft\Edge", r"Software\Chromium", r"Software\BraveSoftware\Brave-Browser"] {
        let key = format!(r"{browser}\NativeMessagingHosts\{HOST_NAME}");
        // SAFETY: plain registry calls on the current user's hive.
        unsafe {
            if enable {
                let value: Vec<u16> = manifest.display().to_string().encode_utf16().chain(Some(0)).collect();
                let bytes = std::slice::from_raw_parts(value.as_ptr().cast::<u8>(), value.len() * 2);
                RegSetKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(key.as_str()), None, REG_SZ.0, Some(bytes.as_ptr().cast()), bytes.len() as u32)
                    .ok()
                    .map_err(|e| format!("Couldn't register the extension's connector: {e}"))?;
            } else {
                let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(key.as_str()));
            }
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn set_registry(_enable: bool, _manifest: &std::path::Path) -> Result<(), String> {
    Err("Browser control needs Windows for now.".into())
}

/// Writes the extension folder, the connector's manifest and registration,
/// and this run's pipe name and token. Returns the extension folder.
pub fn install(state: &AppState) -> Result<PathBuf, String> {
    install_with(state, &std::env::current_exe().map_err(|e| e.to_string())?)
}

/// `exe` is the program the browser starts as the connector.
pub fn install_with(state: &AppState, exe: &std::path::Path) -> Result<PathBuf, String> {
    let ext = state.paths.data.join("extension");
    std::fs::create_dir_all(&ext).map_err(|e| e.to_string())?;
    for (name, bytes) in FILES {
        std::fs::write(ext.join(name), bytes).map_err(|e| e.to_string())?;
    }
    let dir = bridge_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let manifest = dir.join(format!("{HOST_NAME}.json"));
    let m = json!({
        "name": HOST_NAME,
        "description": "SulcusAI browser control",
        "path": exe.display().to_string(),
        "type": "stdio",
        "allowed_origins": [format!("chrome-extension://{EXTENSION_ID}/")],
    });
    std::fs::write(&manifest, serde_json::to_vec_pretty(&m).unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("connection.json"), json!({ "pipe": state.bridge.pipe, "token": state.bridge.token }).to_string()).map_err(|e| e.to_string())?;
    set_registry(true, &manifest)?;
    Ok(ext)
}

pub fn uninstall() {
    let dir = bridge_dir();
    let _ = set_registry(false, &dir.join(format!("{HOST_NAME}.json")));
    let _ = std::fs::remove_file(dir.join("connection.json"));
    let _ = std::fs::remove_file(dir.join(format!("{HOST_NAME}.json")));
}

pub fn installed() -> bool {
    bridge_dir().join("connection.json").exists()
}

// ---------- the app's pipe server ----------

#[cfg(windows)]
pub fn start(app: &AppHandle) {
    let state = app.state::<Arc<AppState>>().inner().clone();
    serve_pipe(state, Some(app.clone()));
}

/// Accepts connectors on this run's pipe (`app` gets status events).
#[cfg(windows)]
pub fn serve_pipe(state: Arc<AppState>, app: Option<AppHandle>) {
    if state.bridge.serving.swap(true, Ordering::SeqCst) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        use tokio::net::windows::named_pipe::ServerOptions;
        let mut first = true;
        loop {
            if !state.bridge.serving.load(Ordering::SeqCst) {
                break;
            }
            let server = match ServerOptions::new().first_pipe_instance(first).reject_remote_clients(true).create(&state.bridge.pipe) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("bridge pipe: {e}");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
            };
            first = false;
            if server.connect().await.is_err() {
                continue;
            }
            let (state, app) = (state.clone(), app.clone());
            tauri::async_runtime::spawn(async move { serve(state, app, server).await });
        }
    });
}

#[cfg(not(windows))]
pub fn start(_app: &AppHandle) {}

pub fn stop(state: &AppState) {
    state.bridge.serving.store(false, Ordering::SeqCst);
    *state.bridge.conn.lock().unwrap() = None;
    *state.bridge.version.lock().unwrap() = None;
}

#[cfg(windows)]
async fn serve(state: Arc<AppState>, app: Option<AppHandle>, pipe: tokio::net::windows::named_pipe::NamedPipeServer) {
    let (r, mut w) = tokio::io::split(pipe);
    let mut lines = tokio::io::BufReader::new(r).lines();
    // The connector proves it read this run's token from the user's folder.
    let Ok(Ok(Some(first))) = tokio::time::timeout(Duration::from_secs(5), lines.next_line()).await else { return };
    let token = serde_json::from_str::<Value>(&first).ok().and_then(|v| v["token"].as_str().map(str::to_string)).unwrap_or_default();
    if token.len() != state.bridge.token.len() || !token.bytes().zip(state.bridge.token.bytes()).fold(true, |ok, (a, b)| ok & (a == b)) {
        return;
    }
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let pending: Pending = Default::default();
    let serial = state.bridge.next.fetch_add(1, Ordering::Relaxed);
    *state.bridge.conn.lock().unwrap() = Some(Conn { tx, pending: pending.clone(), serial });
    let writer = tokio::spawn(async move {
        while let Some(line) = rx.recv().await {
            if w.write_all(format!("{line}\n").as_bytes()).await.is_err() {
                break;
            }
        }
    });
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        if msg["type"] == "hello" {
            *state.bridge.version.lock().unwrap() = Some(msg["version"].as_str().unwrap_or("?").to_string());
            crate::db::log_action(&state.db.lock().unwrap(), "browser", "The SulcusAI extension connected");
            if let Some(app) = &app {
                app.emit("bridge:status", json!({ "connected": true })).ok();
            }
            continue;
        }
        if let Some(id) = msg["id"].as_u64() {
            if let Some(tx) = pending.lock().unwrap().remove(&id) {
                let r = if msg["ok"] == true { Ok(msg["result"].clone()) } else { Err(msg["error"].as_str().unwrap_or("The browser couldn't do that.").to_string()) };
                let _ = tx.send(r);
            }
        }
    }
    writer.abort();
    let mut conn = state.bridge.conn.lock().unwrap();
    if conn.as_ref().is_some_and(|c| c.serial == serial) {
        *conn = None;
        *state.bridge.version.lock().unwrap() = None;
        if let Some(app) = &app {
            app.emit("bridge:status", json!({ "connected": false })).ok();
        }
    }
}

// ---------- connector mode (started by the browser) ----------

fn read_native(input: &mut impl Read) -> Option<Vec<u8>> {
    let mut len = [0u8; 4];
    input.read_exact(&mut len).ok()?;
    let n = u32::from_le_bytes(len) as usize;
    if n > 8 * 1024 * 1024 {
        return None;
    }
    let mut buf = vec![0u8; n];
    input.read_exact(&mut buf).ok()?;
    Some(buf)
}

fn write_native(out: &Mutex<std::io::Stdout>, msg: &[u8]) {
    let mut o = out.lock().unwrap();
    let _ = o.write_all(&(msg.len() as u32).to_le_bytes());
    let _ = o.write_all(msg);
    let _ = o.flush();
}

/// Relays between the browser (stdin/stdout) and the running app (pipe).
/// Exits when the browser closes the connection.
pub fn run_host() {
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("runtime");
    rt.block_on(host_main());
}

#[cfg(windows)]
async fn host_main() {
    use tokio::net::windows::named_pipe::ClientOptions;
    let out = Arc::new(Mutex::new(std::io::stdout()));
    let (to_app, mut from_browser) = mpsc::unbounded_channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        while let Some(m) = read_native(&mut stdin) {
            if to_app.send(m).is_err() {
                break;
            }
        }
        // The browser went away.
        std::process::exit(0);
    });
    let mut told_offline = false;
    loop {
        let conn: Option<Value> = std::fs::read_to_string(bridge_dir().join("connection.json")).ok().and_then(|s| serde_json::from_str(&s).ok());
        let pipe = conn.as_ref().and_then(|c| c["pipe"].as_str().map(str::to_string));
        // Overlapped pipe I/O: a pending read mustn't hold up writes.
        let client = pipe.and_then(|p| ClientOptions::new().open(p).ok());
        let Some(client) = client else {
            if !told_offline {
                write_native(&out, br#"{"type":"status","app":false}"#);
                told_offline = true;
            }
            // Drop what the extension sent while the app was closed.
            while from_browser.try_recv().is_ok() {}
            tokio::time::sleep(Duration::from_secs(3)).await;
            continue;
        };
        told_offline = false;
        let (r, mut w) = tokio::io::split(client);
        let token = conn.as_ref().and_then(|c| c["token"].as_str()).unwrap_or("").to_string();
        if w.write_all(format!("{}\n", json!({ "token": token })).as_bytes()).await.is_err() {
            continue;
        }
        write_native(&out, br#"{"type":"status","app":true}"#);
        let reader_out = out.clone();
        let mut reader = tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(r).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                write_native(&reader_out, line.as_bytes());
            }
        });
        loop {
            tokio::select! {
                m = from_browser.recv() => match m {
                    Some(mut line) => {
                        line.push(b'\n');
                        if w.write_all(&line).await.is_err() {
                            break;
                        }
                    }
                    None => std::process::exit(0),
                },
                _ = &mut reader => break,
            }
        }
        reader.abort();
        write_native(&out, br#"{"type":"status","app":false}"#);
        told_offline = true;
    }
}

#[cfg(not(windows))]
async fn host_main() {}

/// Turns browser control on or off with the feature.
pub fn apply(app: &AppHandle) {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let on = crate::features::is_on(&state.db.lock().unwrap(), Feature::BrowserControl);
    if on {
        match install(&state) {
            Ok(_) => start(app),
            Err(e) => eprintln!("browser control: {e}"),
        }
    } else {
        uninstall();
        stop(&state);
    }
}

// ---------- commands ----------

#[derive(Serialize)]
pub struct BridgeStatus {
    installed: bool,
    connected: bool,
    version: Option<String>,
    extension_dir: String,
    extension_id: &'static str,
}

#[tauri::command]
pub fn bridge_status(state: AppStateRef) -> BridgeStatus {
    BridgeStatus {
        installed: installed(),
        connected: state.bridge.connected(),
        version: state.bridge.version.lock().unwrap().clone(),
        extension_dir: state.paths.data.join("extension").display().to_string(),
        extension_id: EXTENSION_ID,
    }
}

/// Shows the extension folder (for Load unpacked).
#[tauri::command]
pub fn show_extension_folder(state: AppStateRef) -> Result<(), String> {
    let dir = state.paths.data.join("extension");
    if !dir.exists() {
        return Err("Turn on Browser control first.".into());
    }
    crate::open_with_system(dir.as_os_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_messages_are_length_prefixed() {
        let mut data = Vec::new();
        data.extend_from_slice(&7u32.to_le_bytes());
        data.extend_from_slice(br#"{"a":1}"#);
        let mut cur = std::io::Cursor::new(data);
        assert_eq!(read_native(&mut cur).unwrap(), br#"{"a":1}"#);
        assert!(read_native(&mut cur).is_none(), "end of input");
        let mut huge = std::io::Cursor::new(u32::MAX.to_le_bytes().to_vec());
        assert!(read_native(&mut huge).is_none(), "oversized messages are refused");
    }

    #[test]
    fn the_extension_id_matches_its_key() {
        let manifest: Value = serde_json::from_slice(FILES[0].1).unwrap();
        use base64::Engine;
        let der = base64::engine::general_purpose::STANDARD.decode(manifest["key"].as_str().unwrap()).unwrap();
        let hash = sha2::Digest::finalize(<sha2::Sha256 as sha2::Digest>::new_with_prefix(&der));
        let id: String = hash.iter().take(16).flat_map(|b| [b >> 4, b & 15]).map(|n| (b'a' + n) as char).collect();
        assert_eq!(id, EXTENSION_ID);
    }
}
