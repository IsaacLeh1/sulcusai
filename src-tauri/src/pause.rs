// SPDX-License-Identifier: AGPL-3.0-only
//! Pause completely and Quit completely, one click each way.
//!
//! Pausing unloads every model, closes helper programs, disconnects the
//! phone link, People, Telegram and the wake word, holds every background
//! loop (quiet.rs), hides and suspends the windows, hands memory back and
//! puts SulcusAI's processes in Windows' efficiency mode. The process stays
//! only so one click brings it all back. Quitting ends everything; with
//! its switch on, Windows' Task Scheduler starts SulcusAI again for the
//! next scheduled task, which quits again when done.
//!
//! From the tray, the status bar, the phone, and (with their switch on)
//! the Start menu's right-click entries and a hotkey.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};

use crate::instance::Ask;
use crate::{db, AppState, AppStateRef};

const KEY: &str = "pause";
/// Pausing or resuming from anywhere, with the shortcuts switch on.
pub const HOTKEY: &str = "ctrl+alt+shift+KeyP";
/// The Windows task that starts SulcusAI for its next scheduled task.
const TASK_NAME: &str = "SulcusAI\\Scheduled tasks";
/// What the Task Scheduler passes, so this start runs due tasks and quits.
pub const FOR_TASKS: &str = "--scheduled";
/// The start of an error that lists what's running, for the window to ask
/// whether to stop it and pause anyway.
const BUSY: &str = "busy:";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PauseSettings {
    /// The phone link stays connected while paused, so the phone can resume.
    pub keep_phone: bool,
    /// The Start menu's right-click entries and the hotkey.
    pub shortcuts: bool,
    /// Windows' Task Scheduler starts SulcusAI for scheduled tasks after it quits.
    pub tasks_after_quit: bool,
}

pub fn settings(conn: &rusqlite::Connection) -> PauseSettings {
    db::get(conn, KEY).unwrap_or_default()
}

static FOR_TASKS_START: AtomicBool = AtomicBool::new(false);
/// The person opened the window during a start for scheduled tasks: it stays.
static OPENED: AtomicBool = AtomicBool::new(false);

/// This start came from the Task Scheduler for a scheduled task.
pub fn started_for_tasks() -> bool {
    FOR_TASKS_START.load(Ordering::Relaxed)
}

/// What's running that pausing would stop, in words.
fn running(state: &AppState) -> Vec<&'static str> {
    let mut out = Vec::new();
    if !state.generations.lock().unwrap().is_empty() {
        out.push("a reply");
    }
    if crate::media::any_job() {
        out.push("a picture, video or music job");
    }
    if !state.installs.lock().unwrap().is_empty() {
        out.push("a download");
    }
    if crate::finetune::training() || crate::uncensor::running() {
        out.push("teaching a model");
    }
    if crate::meeting::recording() {
        out.push("a meeting recording");
    }
    if crate::voice::listening_now() {
        out.push("dictation or a voice chat");
    }
    out
}

/// Pauses completely. Something running makes it ask first (an error
/// starting with "busy:"), unless `force`, which stops it.
pub async fn pause(app: &AppHandle, state: &Arc<AppState>, force: bool) -> Result<(), String> {
    if crate::quiet::paused() {
        return Ok(());
    }
    let busy = running(state);
    if !busy.is_empty() {
        if !force {
            return Err(format!("{BUSY}{}", busy.join(", ")));
        }
        for flag in state.installs.lock().unwrap().values() {
            flag.store(true, Ordering::Relaxed);
        }
        if crate::meeting::recording() || crate::voice::listening_now() {
            crate::voice::stop_all();
            crate::meeting::stop_live();
        }
        let _ = crate::pcstatus::stop_everything(state).await;
    }
    crate::quiet::set_paused(true);
    for (_, w) in app.webview_windows() {
        let _ = w.hide();
    }
    crate::quick::show_paused(app, true);
    // Everything that runs or connects, stopped.
    state.engine.lock().await.stop().await;
    crate::speech::stop(state).await;
    crate::natural::unload();
    crate::diarize::unload();
    crate::doclib::unload().await;
    crate::media::unload_all();
    crate::connectors::stop_all(state).await;
    crate::privacy::stop_tor().await;
    crate::telegram::stop();
    crate::matrix::stop();
    crate::discord::stop();
    crate::wake::stop();
    crate::sync::people::hang_up_all();
    crate::sync::wake();
    crate::phone::apply(state.clone());
    crate::api_server::apply(state.clone());
    let _ = crate::wakeup::wake_at(None);
    crate::rest::rest_main(app);
    crate::power::eco(true);
    crate::power::trim();
    state.log("perf", "Paused completely");
    app.emit("pause:changed", json!({ "paused": true })).ok();
    Ok(())
}

/// Brings everything back as it was set.
pub async fn resume(app: &AppHandle, state: &Arc<AppState>) {
    if !crate::quiet::paused() {
        crate::quick::show_main(app);
        return;
    }
    crate::quiet::set_paused(false);
    crate::power::eco(crate::perf::out_of_the_way(state));
    crate::quick::show_paused(app, false);
    crate::telegram::start(app.clone(), state.clone());
    crate::matrix::start(app.clone(), state.clone());
    crate::discord::start(app.clone(), state.clone());
    crate::wake::start(app.clone(), state.clone());
    crate::sync::wake();
    crate::phone::apply(state.clone());
    crate::api_server::apply(state.clone());
    crate::quiet::changed();
    crate::quick::show_main(app);
    state.log("perf", "Resumed from a complete pause");
    app.emit("pause:changed", json!({ "paused": false })).ok();
}

pub async fn toggle(app: &AppHandle, state: &Arc<AppState>) {
    if crate::quiet::paused() {
        resume(app, state).await;
    } else if let Err(e) = pause(app, state, false).await {
        // Something's running: the window shows it and asks.
        crate::quick::show_main(app);
        app.emit("pause:asks", json!({ "running": e.trim_start_matches(BUSY) })).ok();
    }
}

/// Quits completely: everything SulcusAI started ends with it (its job
/// object), and the exit handler hands scheduled tasks to Windows.
pub fn quit(app: &AppHandle) {
    app.exit(0);
}

/// From the Start menu entries (a second start handing the word over).
pub fn on_ask(app: &AppHandle, ask: Ask) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<Arc<AppState>>().inner().clone();
        match ask {
            Ask::Pause if !crate::quiet::paused() => toggle(&app, &state).await,
            Ask::Pause => {}
            Ask::Resume => resume(&app, &state).await,
            Ask::Quit => quit(&app),
        }
    });
}

// ---------- shortcuts ----------

fn jump_list_titles() -> [&'static str; 3] {
    ["Pause completely", "Resume", "Quit completely"]
}

/// The Start menu entries and the hotkey, to match the switch.
pub fn apply_shortcuts(app: &AppHandle, on: bool) {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let gs = app.global_shortcut();
    if on {
        if !gs.is_registered(HOTKEY) && gs.register(HOTKEY).is_err() {
            app.emit("quick:hotkey_taken", json!({ "hotkey": crate::quick::shown(HOTKEY) })).ok();
        }
    } else {
        let _ = gs.unregister(HOTKEY);
    }
    std::thread::spawn(move || {
        if let Err(e) = crate::instance::set_jump_list(on, jump_list_titles()) {
            crate::applog::line("app", &format!("The Start menu entries couldn't be {}: {e}", if on { "added" } else { "removed" }));
        }
    });
}

pub fn is_hotkey(shortcut: &tauri_plugin_global_shortcut::Shortcut) -> bool {
    HOTKEY.parse::<tauri_plugin_global_shortcut::Shortcut>().is_ok_and(|h| h.id() == shortcut.id())
}

// ---------- scheduled tasks after quitting ----------

/// The Task Scheduler's description of the task: start SulcusAI at `at`
/// (local time) with `FOR_TASKS`, or as soon as possible if the PC was off.
fn task_xml(exe: &str, at: chrono::NaiveDateTime) -> String {
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>Starts SulcusAI for its next scheduled task; SulcusAI quits again when the task is done.</Description></RegistrationInfo>
  <Triggers><TimeTrigger><StartBoundary>{}</StartBoundary><Enabled>true</Enabled></TimeTrigger></Triggers>
  <Principals><Principal id="Author"><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <StartWhenAvailable>true</StartWhenAvailable>
    <ExecutionTimeLimit>PT4H</ExecutionTimeLimit>
    <Enabled>true</Enabled>
  </Settings>
  <Actions Context="Author"><Exec><Command>{}</Command><Arguments>{FOR_TASKS}</Arguments></Exec></Actions>
</Task>
"#,
        at.format("%Y-%m-%dT%H:%M:%S"),
        esc(exe)
    )
}

#[cfg(windows)]
fn schtasks(args: &[&str]) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("schtasks.exe").args(args).creation_flags(0x0800_0000).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(not(windows))]
fn schtasks(_args: &[&str]) -> Result<(), String> {
    Err("Scheduled tasks after quitting need Windows for now.".into())
}

/// The next time a scheduled task is due, if any (kept in the clear beside
/// each task, so this works while the app is locked).
fn next_due(conn: &rusqlite::Connection) -> Option<i64> {
    conn.query_row("SELECT MIN(next_run) FROM schedules WHERE next_run > ?1", [db::now_ms()], |r| r.get(0)).ok().flatten()
}

/// At a clean exit: hands the next scheduled task to Windows, when allowed.
pub fn on_exit(state: &AppState) {
    let (on, due) = {
        let conn = state.db.lock().unwrap();
        let on = settings(&conn).tasks_after_quit && crate::features::is_on(&conn, crate::features::Feature::Scheduled);
        (on, next_due(&conn))
    };
    let (true, Some(due)) = (on, due) else { return };
    let Some(at) = chrono::DateTime::from_timestamp_millis(due).map(|d| d.with_timezone(&chrono::Local).naive_local()) else { return };
    let Ok(exe) = std::env::current_exe() else { return };
    let file = state.paths.data.join("scheduled-task.xml");
    // UTF-16 with its mark, as the Task Scheduler reads it.
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(task_xml(&exe.to_string_lossy(), at).encode_utf16().flat_map(u16::to_le_bytes));
    if std::fs::write(&file, bytes).is_err() {
        return;
    }
    match schtasks(&["/Create", "/TN", TASK_NAME, "/XML", &file.to_string_lossy(), "/F"]) {
        Ok(()) => crate::applog::line("app", &format!("Windows will start SulcusAI for the next scheduled task at {}", at.format("%Y-%m-%d %H:%M"))),
        Err(e) => crate::applog::line("app", &format!("The next scheduled task couldn't be handed to Windows: {e}")),
    }
    let _ = std::fs::remove_file(&file);
}

/// At start: the task handed to Windows is no longer needed while SulcusAI
/// runs (its own scheduler takes over).
fn take_back_task(state: &AppState) {
    if settings(&state.db.lock().unwrap()).tasks_after_quit {
        std::thread::spawn(|| {
            let _ = schtasks(&["/Delete", "/TN", TASK_NAME, "/F"]);
        });
    }
}

/// A start for scheduled tasks quits once they're done, unless the person
/// opened the window meanwhile.
fn quit_when_done(app: AppHandle, state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        // The scheduler looks within half a minute; a task then counts as a reply.
        tokio::time::sleep(Duration::from_secs(90)).await;
        loop {
            if OPENED.load(Ordering::Relaxed) {
                return;
            }
            let busy = !running(&state).is_empty();
            let soon = next_due(&state.db.lock().unwrap()).is_some_and(|t| t - db::now_ms() < 120_000);
            if !busy && !soon {
                state.log("agent", "Quit after running the scheduled tasks it was started for");
                app.exit(0);
                return;
            }
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    });
}

/// At startup: listens for the Start menu entries, sets the shortcuts, and
/// takes back the task given to Windows.
pub fn start(app: &AppHandle, state: &Arc<AppState>, for_tasks: bool) {
    FOR_TASKS_START.store(for_tasks, Ordering::Relaxed);
    let handle = app.clone();
    crate::instance::listen(move |ask| on_ask(&handle, ask));
    let s = settings(&state.db.lock().unwrap());
    if s.shortcuts {
        apply_shortcuts(app, true);
    }
    take_back_task(state);
    if for_tasks {
        quit_when_done(app.clone(), state.clone());
    }
}

/// The main window was shown by the person.
pub fn opened() {
    OPENED.store(true, Ordering::Relaxed);
}

// ---------- commands ----------

#[derive(Serialize)]
pub struct PauseView {
    paused: bool,
    settings: PauseSettings,
    hotkey: String,
    /// When Windows would start SulcusAI for the next scheduled task (ms).
    next_task: Option<i64>,
}

fn view(state: &AppState) -> PauseView {
    let conn = state.db.lock().unwrap();
    PauseView { paused: crate::quiet::paused(), settings: settings(&conn), hotkey: crate::quick::shown(HOTKEY), next_task: next_due(&conn) }
}

#[tauri::command(async)]
pub fn pause_view(state: AppStateRef) -> PauseView {
    view(&state)
}

#[tauri::command(async)]
pub fn set_pause_settings(app: AppHandle, state: AppStateRef, settings: PauseSettings) -> Result<PauseView, String> {
    let before = {
        let conn = state.db.lock().unwrap();
        let before = self::settings(&conn);
        db::set(&conn, KEY, &settings)?;
        before
    };
    if before.shortcuts != settings.shortcuts {
        apply_shortcuts(&app, settings.shortcuts);
        state.log("settings", if settings.shortcuts { "Pause and quit added to the Start menu, with a hotkey" } else { "Pause and quit taken off the Start menu, and their hotkey" });
    }
    if before.tasks_after_quit != settings.tasks_after_quit {
        if !settings.tasks_after_quit {
            std::thread::spawn(|| {
                let _ = schtasks(&["/Delete", "/TN", TASK_NAME, "/F"]);
            });
        }
        state.log("settings", if settings.tasks_after_quit { "Scheduled tasks run after quitting (Windows' Task Scheduler starts SulcusAI)" } else { "Scheduled tasks run only while SulcusAI is open" });
    }
    if before.keep_phone != settings.keep_phone && crate::quiet::paused() {
        crate::phone::apply(state.inner().clone());
    }
    Ok(view(&state))
}

#[tauri::command]
pub async fn pause_now(app: AppHandle, state: AppStateRef<'_>, force: Option<bool>) -> Result<(), String> {
    pause(&app, state.inner(), force.unwrap_or(false)).await
}

#[tauri::command]
pub async fn resume_now(app: AppHandle, state: AppStateRef<'_>) -> Result<(), String> {
    resume(&app, state.inner()).await;
    Ok(())
}

#[tauri::command]
pub fn quit_now(app: AppHandle) {
    quit(&app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_task_for_windows_starts_this_program_at_the_time_and_catches_up() {
        let at = chrono::NaiveDate::from_ymd_opt(2026, 11, 4).unwrap().and_hms_opt(8, 30, 0).unwrap();
        let xml = task_xml(r"C:\Program Files\SulcusAI\sulcusai.exe", at);
        assert!(xml.contains("<StartBoundary>2026-11-04T08:30:00</StartBoundary>"));
        assert!(xml.contains(r"<Command>C:\Program Files\SulcusAI\sulcusai.exe</Command>"));
        assert!(xml.contains("<Arguments>--scheduled</Arguments>"));
        assert!(xml.contains("<StartWhenAvailable>true</StartWhenAvailable>"), "a task missed while the PC was off runs when it's back");
        assert!(xml.contains("<RunLevel>LeastPrivilege</RunLevel>"));
        // A path with characters XML reserves stays well formed.
        assert!(task_xml(r"C:\A&B\sulcusai.exe", at).contains(r"C:\A&amp;B\sulcusai.exe"));
    }

    #[test]
    fn nothing_running_means_pausing_asks_nothing() {
        let state = crate::testutil::scratch_state();
        assert!(running(&state).is_empty());
        state.installs.lock().unwrap().insert("m".into(), Default::default());
        assert_eq!(running(&state), vec!["a download"]);
    }

    #[test]
    fn the_next_task_is_read_without_unlocking() {
        let state = crate::testutil::scratch_state();
        let conn = state.db.lock().unwrap();
        assert_eq!(next_due(&conn), None);
        let soon = db::now_ms() + 3_600_000;
        conn.execute("INSERT INTO schedules (id, data, next_run, created_at) VALUES ('a', 'x', ?1, 0), ('b', 'x', ?2, 0), ('c', 'x', 5, 0)", [soon + 60_000, soon]).unwrap();
        assert_eq!(next_due(&conn), Some(soon), "the earliest still to come");
    }
}
