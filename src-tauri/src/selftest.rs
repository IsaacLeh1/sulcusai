// SPDX-License-Identifier: AGPL-3.0-only
//! The release check that a built copy's window draws
//! (`scripts/check_window.ps1`): 0.9.49 went out with a blank one. Started
//! with SULCUSAI_SELFTEST naming a result file, and SULCUSAI_DATA_DIR on a
//! scratch folder, the main window is never shown. Its text is read every
//! two seconds from outside the page, so a page whose code never loaded
//! still answers (with nothing), and the app exits 0 once the window holds
//! something other than the crash screen, or 1 after a minute and a half.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

const ENV: &str = "SULCUSAI_SELFTEST";
const WAIT: Duration = Duration::from_secs(90);

static RESULT: OnceLock<PathBuf> = OnceLock::new();
static LAST: Mutex<String> = Mutex::new(String::new());
static DONE: OnceLock<()> = OnceLock::new();

/// Starts the check when asked for; the caller shows the window otherwise.
pub fn begin(app: &AppHandle) -> Result<bool, String> {
    let Some(result) = std::env::var_os(ENV) else { return Ok(false) };
    // Startup tidies its data folder (unfinished work, empty chats), which
    // must never be one a running copy uses.
    if std::env::var_os("SULCUSAI_DATA_DIR").is_none() {
        return Err(format!("{ENV} needs SULCUSAI_DATA_DIR set to a scratch folder."));
    }
    let _ = RESULT.set(PathBuf::from(result));
    let app = app.clone();
    std::thread::spawn(move || {
        let until = Instant::now() + WAIT;
        while Instant::now() < until && DONE.get().is_none() {
            std::thread::sleep(Duration::from_secs(2));
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.eval("window.__TAURI_INTERNALS__.invoke('selftest_seen', { text: document.body ? document.body.innerText : '', crashed: !!document.querySelector('.crash-detail') })");
            }
        }
        let last = LAST.lock().unwrap().clone();
        finish(&app, false, &format!("Nothing drawn in {} seconds. Last text: {last:?}", WAIT.as_secs()));
    });
    Ok(true)
}

#[tauri::command]
pub fn selftest_seen(app: AppHandle, text: String, crashed: bool) {
    if RESULT.get().is_none() {
        return;
    }
    if crashed {
        finish(&app, false, &format!("The crash screen showed: {text}"));
    } else if text.trim().chars().count() >= 20 {
        finish(&app, true, &text);
    } else {
        *LAST.lock().unwrap() = text;
    }
}

fn finish(app: &AppHandle, drawn: bool, detail: &str) {
    if DONE.set(()).is_err() {
        return;
    }
    if let Some(path) = RESULT.get() {
        let head: String = detail.chars().take(600).collect();
        let _ = std::fs::write(path, format!("{}\n{head}\n", if drawn { "drawn" } else { "blank" }));
    }
    app.exit(if drawn { 0 } else { 1 });
}
