// SPDX-License-Identifier: AGPL-3.0-only
//! programs_list, program_open, program_read, program_click, program_type,
//! program_key, program_scroll: the assistant works other programs on this
//! PC (`crate::programs`). The first use in a chat asks; pressing anything
//! that sends, buys or deletes asks every time; password fields are the
//! person's. While it works, a strip at the top of the screen says so, with
//! Stop.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use super::{Outcome, Preview};
use crate::programs::{self, Action, Pick};

pub fn programs_list_params() -> Value {
    json!({ "type": "object", "properties": {} })
}

pub fn program_open_params() -> Value {
    json!({ "type": "object", "required": ["name"], "properties": { "name": { "type": "string", "description": "The program's name as in the Start menu, e.g. Notepad, Calculator, Excel." } } })
}

pub fn program_read_params() -> Value {
    json!({ "type": "object", "properties": { "window": { "type": "string", "description": "The window's number from programs_list, or part of its title or program name. Leave out for the one used last." } } })
}

pub fn program_click_params() -> Value {
    json!({ "type": "object", "required": ["ref"], "properties": { "ref": { "type": "integer", "description": "The element's number from the window." } } })
}

pub fn program_type_params() -> Value {
    json!({ "type": "object", "required": ["ref", "text"], "properties": {
        "ref": { "type": "integer", "description": "The field's number from the window." },
        "text": { "type": "string", "description": "What to type." },
        "replace": { "type": "boolean", "description": "Put it in place of what's there. Fields are replaced unless this is false; documents get the text added at the end unless this is true (replacing a document's text asks the user)." },
        "submit": { "type": "boolean", "description": "Press Enter afterwards." } } })
}

pub fn program_key_params() -> Value {
    json!({ "type": "object", "required": ["keys"], "properties": { "keys": { "type": "string", "description": "Keys to press together in the window used last, e.g. ctrl+s, enter, escape, tab, alt+f, f5, pagedown." } } })
}

pub fn use_screen_params() -> Value {
    json!({ "type": "object", "required": ["task"], "properties": { "task": { "type": "string", "description": "The whole task in words, as the user would put it, e.g. \"In the game's menu, turn the music volume down to half.\"" } } })
}

pub fn program_scroll_params() -> Value {
    json!({ "type": "object", "properties": {
        "ref": { "type": "integer", "description": "Scroll this element (or what holds it); leave out for the window." },
        "direction": { "type": "string", "enum": ["down", "up"] },
        "screens": { "type": "integer", "description": "How far, in screens (1 unless given)." } } })
}

fn element(args: &Value) -> Result<u32, String> {
    args.get("ref")
        .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim_matches(['[', ']']).parse().ok())))
        .map(|n| n as u32)
        .ok_or_else(|| "Give the element's number from the window as ref.".into())
}

fn pick(args: &Value) -> Pick {
    match args.get("window") {
        Some(Value::Number(n)) => n.as_i64().map_or(Pick::Last, Pick::Id),
        Some(Value::String(s)) if !s.trim().is_empty() => s.trim().parse::<i64>().map_or_else(|_| Pick::Named(s.trim().to_string()), Pick::Id),
        _ => Pick::Last,
    }
}

/// A press, keys or Enter that sends, buys, deletes or closes: asked first,
/// every time. Typing into a password field is refused before it's asked.
pub async fn assess(chat_id: &str, name: &str, args: &Value) -> Result<Option<Preview>, String> {
    let card = |title: String, detail: String| Preview { title, kind: "text", detail: Some(detail), note: Some("Asking because this could send, buy, delete or close something in another program.".into()) };
    match name {
        "program_click" => {
            let t = programs::target(chat_id, element(args)?).await?;
            Ok(programs::risky(&t.name).then(|| card(format!("Press “{}” in {}", t.name, t.program), format!("{} “{}”", t.kind, t.name))))
        }
        "program_type" => {
            let t = programs::target(chat_id, element(args)?).await?;
            if t.password {
                return Err("This is a password field. Ask the user to type it in the program.".into());
            }
            let text = args.get("text").and_then(Value::as_str).unwrap_or_default();
            let submit = args.get("submit").and_then(Value::as_bool).unwrap_or(false);
            // A file of theirs open in it (the title names one): asked first,
            // even to add to it. A small model typed into a README Notepad had
            // reopened, though it was asked for a new document.
            if let Some(file) = t.file.as_ref().filter(|_| t.kind == "document") {
                return Ok(Some(Preview {
                    title: format!("Type into {file}"),
                    kind: "text",
                    detail: Some(text.to_string()),
                    note: Some(format!("“{file}” is a file of yours open in {}, not a new document. Allow only if you meant to change that file.", t.program)),
                }));
            }
            // A document's own text (often a file of theirs) is only replaced when they say so.
            if t.kind == "document" && args.get("replace").and_then(Value::as_bool) == Some(true) {
                let name = if t.name.is_empty() { t.program.clone() } else { t.name.clone() };
                return Ok(Some(Preview {
                    title: format!("Replace all the text in {name}"),
                    kind: "text",
                    detail: Some(text.to_string()),
                    note: Some(format!("Everything in this document in {} would be replaced with the text above.", t.program)),
                }));
            }
            Ok((submit && programs::enter_sends(t.kind, &t.name)).then(|| card(format!("Type and press Enter in {}", t.program), format!("Into {} “{}”:\n{text}", t.kind, t.name))))
        }
        "program_key" => {
            let keys = args.get("keys").and_then(Value::as_str).unwrap_or_default();
            Ok(programs::risky_keys(keys).then(|| card(format!("Press {keys}"), format!("In the window used last: {keys}"))))
        }
        _ => Ok(None),
    }
}

fn reading_outcome(r: programs::Reading, title: String) -> Outcome {
    let shown = r.window.label();
    Outcome::ok(r.text, title, "text", Some(shown))
}

pub async fn run(name: &str, args: &Value, app: Option<&AppHandle>, chat_id: &str, log: impl Fn(&str)) -> Outcome {
    let result: Result<Outcome, String> = async {
        match name {
            "programs_list" => {
                let list = programs::list().await?;
                if list.is_empty() {
                    return Ok(Outcome::ok("No program windows are open.", "Listed open windows", "text", None));
                }
                let text = list.iter().map(|w| format!("window {}: {} “{}”", w.id, w.program, w.title)).collect::<Vec<_>>().join("\n");
                Ok(Outcome::ok(format!("{}\n\n{text}", programs::UNTRUSTED), format!("Listed {} open windows", list.len()), "text", None))
            }
            "program_open" => {
                let what = super::arg_str(args, "name")?;
                banner(app, chat_id, &format!("Opening {what}"));
                let r = programs::open(chat_id, what).await?;
                log(&format!("Opened {} in another program", r.window.program));
                banner(app, chat_id, &format!("Working in {}", r.window.program));
                Ok(reading_outcome(r.clone(), format!("Opened {}", r.window.program)))
            }
            "program_read" => {
                let r = programs::read(chat_id, pick(args)).await?;
                banner(app, chat_id, &format!("Working in {}", r.window.program));
                Ok(reading_outcome(r.clone(), format!("Read {}", r.window.program)))
            }
            "program_click" | "program_type" | "program_key" | "program_scroll" => {
                let action = match name {
                    "program_click" => Action::Click(element(args)?),
                    "program_type" => Action::Type {
                        r: element(args)?,
                        text: super::arg_str(args, "text")?.to_string(),
                        submit: args.get("submit").and_then(Value::as_bool).unwrap_or(false),
                        replace: args.get("replace").and_then(Value::as_bool),
                    },
                    "program_key" => Action::Key(super::arg_str(args, "keys")?.to_string()),
                    _ => Action::Scroll {
                        r: args.get("ref").filter(|v| !v.is_null()).map(|_| element(args)).transpose()?,
                        down: args.get("direction").and_then(Value::as_str) != Some("up"),
                        screens: args.get("screens").and_then(Value::as_u64).unwrap_or(1) as u32,
                    },
                };
                banner(app, chat_id, "Working in another program");
                let (done, r) = programs::act(chat_id, action).await?;
                banner(app, chat_id, &format!("Working in {}", r.window.program));
                log(&format!("{done} in {}", r.window.program));
                Ok(reading_outcome(r, done))
            }
            other => Err(format!("Unknown program tool {other}.")),
        }
    }
    .await;
    result.unwrap_or_else(|e| Outcome::error("Couldn't use the program", e))
}

// ---------- the working strip ----------

const BANNER: &str = "working";

/// What the strip says, and for which chat (its Stop stops that reply).
fn shown() -> &'static Mutex<Value> {
    static SHOWN: std::sync::OnceLock<Mutex<Value>> = std::sync::OnceLock::new();
    SHOWN.get_or_init(|| Mutex::new(Value::Null))
}

/// Each update; the strip goes 20 s after the last one.
static LAST: AtomicU64 = AtomicU64::new(0);

/// Shows the strip at the top of the screen (on top of everything, left out
/// of screenshots) saying what the assistant is doing.
pub(crate) fn banner(app: Option<&AppHandle>, chat_id: &str, what: &str) {
    let Some(app) = app else { return };
    let payload = json!({ "chat_id": chat_id, "what": what });
    *shown().lock().unwrap() = payload.clone();
    let n = LAST.fetch_add(1, Ordering::SeqCst) + 1;
    match app.get_webview_window(BANNER) {
        Some(w) => {
            let _ = w.show();
            let _ = app.emit_to(BANNER, "programs:working", payload);
        }
        None => {
            let (width, height) = (520.0, 46.0);
            let x = app.primary_monitor().ok().flatten().map(|m| (m.size().width as f64 / m.scale_factor() - width) / 2.0).unwrap_or(400.0);
            let built = WebviewWindowBuilder::new(app, BANNER, WebviewUrl::App("index.html".into()))
                .title("SulcusAI is working")
                .inner_size(width, height)
                .position(x.max(0.0), 8.0)
                .decorations(false)
                .resizable(false)
                .always_on_top(true)
                .skip_taskbar(true)
                .focused(false)
                .shadow(true)
                .build();
            if let Ok(w) = built {
                programs::banner_ready(&w);
            }
        }
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(20)).await;
        if LAST.load(Ordering::SeqCst) == n {
            hide(&app);
        }
    });
}

pub(crate) fn hide(app: &AppHandle) {
    *shown().lock().unwrap() = Value::Null;
    if let Some(w) = app.get_webview_window(BANNER) {
        let _ = w.hide();
    }
}

/// What the strip shows now (it asks when it opens).
#[tauri::command]
pub fn programs_banner() -> Value {
    shown().lock().unwrap().clone()
}

/// The strip's Stop: the reply that's working the program stops.
#[tauri::command]
pub fn programs_banner_stop(app: AppHandle, state: tauri::State<'_, Arc<crate::AppState>>) {
    let chat = shown().lock().unwrap().get("chat_id").and_then(Value::as_str).map(str::to_string);
    // Dictating into another app: its Stop stops listening.
    if chat.as_deref() == Some(crate::voice::ANYWHERE) {
        crate::voice::stop_anywhere();
    }
    if let Some(flag) = chat.and_then(|c| state.generations.lock().unwrap().get(&c).cloned()) {
        flag.store(true, Ordering::Relaxed);
    }
    hide(&app);
}
