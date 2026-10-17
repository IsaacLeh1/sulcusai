// SPDX-License-Identifier: AGPL-3.0-only
//! Crash reports, only when the person turns them on (off by default), and
//! never sent by themselves. With the switch on, a failure in the core, an
//! error in the window, or the app being closed unexpectedly while Windows
//! kept running (ended in Task Manager after a freeze, say) leaves a short
//! report on this PC. At the next start the person reads it and chooses:
//! send it to the developer's inbox (see feedback.rs), copy it, or not.
//!
//! A report holds the version, Windows, the processor, memory and graphics
//! card, and where it failed. Never chats, files or anything typed; paths
//! have the user's folder taken out.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{db, AppState, AppStateRef};

const KEY: &str = "crash_reports";

static ON: AtomicBool = AtomicBool::new(false);
static DIR: OnceLock<PathBuf> = OnceLock::new();
static ABOUT: OnceLock<String> = OnceLock::new();
static STARTED: AtomicI64 = AtomicI64::new(0);
/// When the window last showed it was responsive (its ping), ms.
static WINDOW_OK: AtomicI64 = AtomicI64::new(0);
/// The app, for sending a crash in developer mode.
static STATE: std::sync::OnceLock<Arc<AppState>> = std::sync::OnceLock::new();
/// Window errors kept this run (a loop of errors makes one report, not 100).
static WINDOW_ERRORS: AtomicU32 = AtomicU32::new(0);

fn now() -> i64 {
    db::now_ms()
}

fn reports_dir() -> Option<&'static PathBuf> {
    DIR.get()
}

/// The marker a clean exit removes: while it's there, the app is running.
fn marker() -> Option<PathBuf> {
    reports_dir().map(|d| d.join("running.json"))
}

#[derive(Serialize, Deserialize)]
struct Running {
    started: i64,
    /// The last sign of life from the core, and from the window.
    alive: i64,
    window: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    id: String,
    /// "core", "window" or "closed".
    kind: String,
    at: i64,
    text: String,
}

/// The user's folder (and name) out of a text.
pub(crate) fn tidy(s: &str) -> String {
    let mut out = s.to_string();
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        let home = home.to_string_lossy().to_string();
        if home.len() > 3 {
            out = out.replace(&home, "%USERPROFILE%").replace(&home.replace('\\', "/"), "%USERPROFILE%");
        }
    }
    if let Some(user) = std::env::var_os("USERNAME").or_else(|| std::env::var_os("USER")) {
        let user = user.to_string_lossy().to_string();
        if user.len() > 2 {
            out = out.replace(&user, "<user>");
        }
    }
    out
}

fn about(state: &AppState) -> String {
    let hw = state.hardware.read().unwrap();
    let gpu = hw.best_gpu().map(|g| format!("{} ({:.0} GB)", g.name, g.vram_bytes as f64 / 1e9)).unwrap_or_else(|| "no separate graphics card".into());
    format!(
        "SulcusAI {} · {} · {} · {:.0} GB memory · {}",
        env!("CARGO_PKG_VERSION"),
        hw.os,
        hw.cpu_name,
        hw.ram_total as f64 / 1e9,
        gpu
    )
}

fn duration(ms: i64) -> String {
    let m = (ms.max(0) / 60_000) as u64;
    if m < 60 { format!("{m} min") } else { format!("{} h {} min", m / 60, m % 60) }
}

fn write(kind: &str, what: &str, details: &str) {
    // Always kept on this PC, for the person to read (Settings → Logs).
    crate::applog::crash(what, details);
    // Developer mode: to the developer at once.
    if let Some(state) = STATE.get() {
        crate::devmode::crash(state, what, details);
    }
    if !ON.load(Ordering::Relaxed) {
        return;
    }
    let Some(dir) = reports_dir() else { return };
    let at = now();
    let when = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
    let running = duration(at - STARTED.load(Ordering::Relaxed));
    let details: String = tidy(details).lines().take(60).collect::<Vec<_>>().join("\n");
    let text = format!(
        "**What happened:** {what}\n**Version:** {}\n**When:** {when} (running for {running})\n\n**Details:**\n```\n{details}\n```",
        ABOUT.get().map(String::as_str).unwrap_or(env!("CARGO_PKG_VERSION"))
    );
    let r = Report { id: format!("{at}-{kind}"), kind: kind.into(), at, text };
    if std::fs::create_dir_all(dir).is_ok() {
        if let Ok(json) = serde_json::to_string(&r) {
            let _ = crate::storage::write_safely(&dir.join(format!("{}.json", r.id)), json);
        }
    }
}

/// At startup: the switch, a failure hook, the running marker, and a report
/// if the last run ended without closing (while Windows kept running).
pub fn start(state: &Arc<AppState>) {
    let _ = STATE.set(state.clone());
    let _ = DIR.set(state.paths.data.join("crashes"));
    let _ = ABOUT.set(about(state));
    STARTED.store(now(), Ordering::Relaxed);
    WINDOW_OK.store(now(), Ordering::Relaxed);
    ON.store(db::get::<bool>(&state.db.lock().unwrap(), KEY).unwrap_or(false), Ordering::Relaxed);

    let before = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        before(info);
        let place = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let message = info.payload().downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| info.payload().downcast_ref::<String>().cloned()).unwrap_or_default();
        let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
        let trace = std::backtrace::Backtrace::force_capture().to_string();
        write("core", "Part of SulcusAI failed (a panic).", &format!("{message}\nat {place}, thread {thread}\n\n{trace}"));
    }));

    // How the last run ended.
    if let Some(m) = marker() {
        if let Some(last) = std::fs::read_to_string(&m).ok().and_then(|t| serde_json::from_str::<Running>(&t).ok()) {
            let boot = sysinfo::System::boot_time() as i64 * 1000;
            // Windows restarted since: a shutdown, not something to report.
            if boot < last.alive {
                let frozen = last.alive - last.window;
                let what = if frozen > 90_000 {
                    format!("SulcusAI was closed unexpectedly after its window had stopped responding for about {} (a freeze).", duration(frozen))
                } else {
                    "SulcusAI was closed unexpectedly (ended from Task Manager, or it crashed), while Windows kept running.".to_string()
                };
                write("closed", &what, &format!("It ran for {} before that.\nLast engine log lines:\n{}", duration(last.alive - last.started), engine_tail(&state.paths.engine_log())));
            }
        }
        let _ = std::fs::create_dir_all(m.parent().unwrap_or(Path::new(".")));
    }
    // Signs of life while it runs.
    std::thread::spawn(|| loop {
        if let Some(m) = marker() {
            let r = Running { started: STARTED.load(Ordering::Relaxed), alive: now(), window: WINDOW_OK.load(Ordering::Relaxed) };
            if let Ok(json) = serde_json::to_string(&r) {
                let _ = std::fs::write(m, json);
            }
        }
        std::thread::sleep(Duration::from_secs(30));
    });
}

/// The engine's last lines (what it was doing), without the user's paths.
fn engine_tail(log: &Path) -> String {
    let text = std::fs::read(log).map(|b| String::from_utf8_lossy(&b[b.len().saturating_sub(4000)..]).to_string()).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(15)..].join("\n")
}

/// A clean exit (or a restart for an update): nothing to report.
pub fn closing() {
    if let Some(m) = marker() {
        let _ = std::fs::remove_file(m);
    }
}

fn pending() -> Vec<Report> {
    let Some(dir) = reports_dir() else { return Vec::new() };
    let mut list: Vec<Report> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name() != "running.json")
        .filter_map(|e| serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok())
        .collect();
    list.sort_by_key(|r| r.at);
    list
}

#[derive(Serialize)]
pub struct CrashView {
    on: bool,
    pending: Vec<Report>,
}

#[tauri::command(async)]
pub fn crash_reports_view() -> CrashView {
    let on = ON.load(Ordering::Relaxed);
    CrashView { on, pending: if on { pending() } else { Vec::new() } }
}

#[tauri::command(async)]
pub fn set_crash_reports(state: AppStateRef, on: bool) -> Result<CrashView, String> {
    {
        let conn = state.db.lock().unwrap();
        db::set(&conn, KEY, &on)?;
        db::log_action(&conn, "privacy", if on { "Turned on crash reports (shown to you before anything is sent)" } else { "Turned off crash reports" });
    }
    ON.store(on, Ordering::Relaxed);
    if !on {
        dismiss_all();
    }
    Ok(crash_reports_view())
}

fn dismiss_all() {
    if let Some(dir) = reports_dir() {
        for r in pending() {
            let _ = std::fs::remove_file(dir.join(format!("{}.json", r.id)));
        }
    }
}

/// The person chose not to send them (or did): they're gone from this PC.
#[tauri::command(async)]
pub fn dismiss_crash_reports() {
    dismiss_all();
}

/// Sends a report the person read to the developer's inbox, then removes it.
#[tauri::command]
pub async fn send_crash_report(id: String) -> Result<(), String> {
    let r = pending().into_iter().find(|r| r.id == id).ok_or("That report is gone.")?;
    let text: String = r.text.chars().take(7900).collect();
    crate::feedback::send("crash", &text, None, false).await?;
    if let Some(dir) = reports_dir() {
        let _ = std::fs::remove_file(dir.join(format!("{}.json", r.id)));
    }
    Ok(())
}

/// The window is responsive (it says so every 30 seconds).
#[tauri::command(async)]
pub fn crash_ping() {
    WINDOW_OK.store(now(), Ordering::Relaxed);
}

/// An error in the window, with where it happened.
#[tauri::command(async)]
pub fn report_window_error(message: String, stack: Option<String>) {
    if WINDOW_ERRORS.fetch_add(1, Ordering::Relaxed) >= 3 {
        return;
    }
    let message: String = message.chars().take(500).collect();
    let stack: String = stack.unwrap_or_default().chars().take(3000).collect();
    write("window", "Something in SulcusAI's window failed.", &format!("{message}\n{stack}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_leaves_out_the_users_folder_and_name() {
        let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
        let user = std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_default();
        let out = tidy(&format!("failed at {home}\\AppData\\Local\\SulcusAI\\x.rs (by {user})"));
        if home.len() > 3 {
            assert!(!out.contains(&home) && out.contains("%USERPROFILE%"), "{out}");
        }
        if user.len() > 2 {
            assert!(!out.contains(&user), "{out}");
        }
        assert_eq!(duration(3 * 60_000), "3 min");
        assert_eq!(duration(125 * 60_000), "2 h 5 min");
    }
}
