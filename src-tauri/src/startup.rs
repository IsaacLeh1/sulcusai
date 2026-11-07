// SPDX-License-Identifier: AGPL-3.0-only
//! How the app starts: safe mode and low-end mode.
//!
//! Safe mode is one start without extras (the chat bridges, clipboard and
//! screen timeline, wake word, quick ask, the browser bridge), plugins and
//! custom themes, for when one of them stops the app starting or makes it
//! misbehave. It comes from "Restart in safe mode" (a marker read once) or
//! `--safe-mode` on the command line; the next normal start is normal again.
//!
//! Low-end mode is a switch (off by default) for PCs with little memory or
//! an old processor: extras start later, the screen timeline stays off and
//! the window drops animations and blur.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tauri::AppHandle;

use crate::{db, AppState, AppStateRef};

const MARKER: &str = ".safe-mode-once";
const LOW_END: &str = "low_end_mode";

static SAFE: AtomicBool = AtomicBool::new(false);

/// At startup, before anything else starts: whether this start is safe mode.
/// The marker is removed at once, so a crash in safe mode can't trap the app
/// in it.
pub fn begin(data: &Path) {
    let marker = data.join(MARKER);
    let asked = std::env::args().any(|a| a == "--safe-mode");
    let once = marker.exists();
    if once {
        let _ = std::fs::remove_file(&marker);
    }
    if asked || once {
        SAFE.store(true, Ordering::Relaxed);
        crate::applog::line("app", "Started in safe mode: no extras, plugins or custom themes");
    }
}

pub fn safe_mode() -> bool {
    SAFE.load(Ordering::Relaxed)
}

pub fn low_end_mode(state: &AppState) -> bool {
    db::get::<bool>(&state.db.lock().unwrap(), LOW_END).unwrap_or(false)
}

#[derive(Serialize)]
pub struct StartupView {
    /// This start is safe mode: the window skips custom themes and plugins
    /// and shows a line saying so, with "Restart normally".
    safe_mode: bool,
    low_end_mode: bool,
    /// Memory under 8 GB or 4 threads or fewer: where the switch is offered
    /// first.
    suggest_low_end: bool,
}

#[tauri::command(async)]
pub fn startup_view(state: AppStateRef) -> StartupView {
    let suggest_low_end = {
        let hw = state.hardware.read().unwrap();
        hw.ram_total < 8_000_000_000 || hw.cpu_threads <= 4
    };
    StartupView { safe_mode: safe_mode(), low_end_mode: low_end_mode(&state), suggest_low_end }
}

#[tauri::command(async)]
pub fn set_low_end_mode(state: AppStateRef, on: bool) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    db::log_action(&conn, "settings", if on { "Turned on low-end mode" } else { "Turned off low-end mode" });
    db::set(&conn, LOW_END, &on)
}

/// Restarts once in safe mode; `on: false` restarts normally.
#[tauri::command]
pub fn restart_in_safe_mode(app: AppHandle, state: AppStateRef, on: bool) -> Result<(), String> {
    let marker = state.paths.data.join(MARKER);
    if on {
        std::fs::write(&marker, b"1").map_err(|e| e.to_string())?;
    } else {
        let _ = std::fs::remove_file(&marker);
    }
    state.log("app", if on { "Restarting in safe mode" } else { "Restarting normally" });
    crate::crash::closing();
    app.restart();
}
