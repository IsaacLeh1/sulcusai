// SPDX-License-Identifier: AGPL-3.0-only
//! The phone as a remote for the PC's call: mute, deafen and switch voice
//! channel while the PC is busy with a full-screen game, say.
//!
//! The microphone and speakers belong to the PC's window, so the phone
//! doesn't touch them itself: each control becomes a `remote:control` event
//! for the main window, which does what its own buttons do and reports back
//! with `remote_controls_report`. The phone reads the state with
//! `remote_controls`.
//!
//! Event payloads, for the window's call view to handle:
//! `{ "action": "mute", "on": bool }`, `{ "action": "deafen", "on": bool }`,
//! `{ "action": "switch", "group": id, "channel": id }`, `{ "action": "leave" }`.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::Emitter;

use crate::AppState;

/// What the window last reported about its own microphone and sound.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Reported {
    pub muted: bool,
    pub deafened: bool,
}

static REPORTED: Mutex<Option<Reported>> = Mutex::new(None);

fn tell_window(state: &AppState, payload: Value) -> Result<Value, String> {
    let app = state.app.get().ok_or("The app isn't ready yet.")?;
    app.emit_to("main", "remote:control", payload).map_err(|e| e.to_string())?;
    Ok(Value::Null)
}

/// The phone's remote requests, by command name. `None`: not one of them.
pub async fn handle(state: &AppState, cmd: &str, args: &Value) -> Option<Result<Value, String>> {
    let on = args["on"].as_bool().unwrap_or(true);
    let r = match cmd {
        "remote_controls" => {
            let call = crate::social::live::social_call_view().await;
            let reported = REPORTED.lock().unwrap().clone().unwrap_or_default();
            Ok(json!({ "call": call, "muted": reported.muted, "deafened": reported.deafened }))
        }
        "remote_mute" => tell_window(state, json!({ "action": "mute", "on": on })),
        "remote_deafen" => tell_window(state, json!({ "action": "deafen", "on": on })),
        "remote_leave" => tell_window(state, json!({ "action": "leave" })),
        "remote_switch_channel" => {
            let (Some(group), Some(channel)) = (args["group"].as_str(), args["channel"].as_str()) else {
                return Some(Err("Pick a voice channel.".into()));
            };
            tell_window(state, json!({ "action": "switch", "group": group, "channel": channel }))
        }
        _ => return None,
    };
    Some(r)
}

/// The window says how its microphone and sound are now (after its own
/// buttons or a remote control).
#[tauri::command(async)]
pub fn remote_controls_report(muted: bool, deafened: bool) {
    *REPORTED.lock().unwrap() = Some(Reported { muted, deafened });
}
