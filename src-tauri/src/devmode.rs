// SPDX-License-Identifier: AGPL-3.0-only
//! Developer mode: on a PC where the person turned it on (after reading what
//! it sends), SulcusAI sends the developer reports to improve its engine:
//! crashes as they happen, and every 30 minutes the new lines of app.log
//! about the engine, pictures, downloads and errors (never what's said in
//! chats, prompts, notes, mail or files), with the user's folder and name
//! taken out. Each report goes to the developer's own inbox on the relay,
//! and a copy stays in logs\dev-sent.log, so the person can see exactly
//! what left the PC. Off by default, and off again with one switch.

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{db, AppState, AppStateRef};

const KEY: &str = "dev_mode";
/// app.log categories that go in reports. Not settings, people, security or
/// privacy entries, which can name people, phones or places.
const SENT: &[&str] = &["app", "engine", "media", "download", "crash", "agent", "updates", "network", "sync"];
/// The relay takes 8,000 characters per message.
const MAX_CHARS: usize = 7600;
const EVERY: Duration = Duration::from_secs(30 * 60);
pub const SENT_LOG: &str = "dev-sent.log";

static ON: AtomicBool = AtomicBool::new(false);
/// When a report went last (so an error doesn't send another right away).
static LAST: AtomicI64 = AtomicI64::new(0);

#[derive(Default, Clone, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    on: bool,
    /// When the person turned it on, having read what it sends.
    since: i64,
    /// How much of app.log was sent already (bytes).
    sent_upto: u64,
    reports: u32,
    last_sent: i64,
}

fn saved(state: &AppState) -> Saved {
    db::get(&state.db.lock().unwrap(), KEY).unwrap_or_default()
}

pub fn is_on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// At startup: the switch, and the half-hourly reports while it's on.
pub fn start(state: &Arc<AppState>) {
    ON.store(saved(state).on, Ordering::Relaxed);
    let st = state.clone();
    tauri::async_runtime::spawn(async move {
        // A first report a few minutes in (how the start went), then every 30 minutes.
        tokio::time::sleep(Duration::from_secs(180)).await;
        loop {
            if is_on() {
                if let Err(e) = report(&st, "Regular report").await {
                    crate::applog::line("app", &format!("Developer report not sent: {e}"));
                }
            }
            tokio::time::sleep(EVERY).await;
        }
    });
}

/// This PC, for telling reports apart: a short id (not the sync id itself)
/// and what it runs on.
fn header(state: &AppState, why: &str) -> String {
    use sha2::{Digest, Sha256};
    let id = crate::sync::device_id(&state.paths.data);
    let short = &hex::encode(Sha256::digest(format!("sulcusai-dev:{id}").as_bytes()))[..10];
    let hw = state.hardware.read().unwrap();
    let gpus: Vec<String> = hw.gpus.iter().map(|g| format!("{} ({:.1} GB{})", g.name, g.vram_bytes as f64 / 1e9, if g.integrated { ", built-in" } else { "" })).collect();
    format!(
        "Developer mode · {why}\nPC {short} · SulcusAI {} · {} · {} · {:.0} GB memory · graphics: {}",
        env!("CARGO_PKG_VERSION"),
        hw.os,
        hw.cpu_name,
        hw.ram_total as f64 / 1e9,
        if gpus.is_empty() { "none found".into() } else { gpus.join(", ") }
    )
}

/// The new lines of app.log since the last report, only the categories
/// that go, with the user's folder and name taken out.
fn new_lines(state: &AppState, from: u64) -> (String, u64) {
    let path = state.paths.logs.join(crate::applog::APP_LOG);
    let bytes = std::fs::read(&path).unwrap_or_default();
    let len = bytes.len() as u64;
    // The log started over since (it got big): from its start.
    let from = if from > len { 0 } else { from };
    let text = String::from_utf8_lossy(&bytes[from as usize..]);
    let kept: Vec<&str> = text
        .lines()
        .filter(|l| l.split_once("] ").map(|(head, _)| head).and_then(|h| h.rsplit_once('[')).is_some_and(|(_, cat)| SENT.contains(&cat)))
        .collect();
    (crate::crash::tidy(&kept.join("\n")), len)
}

/// Sends a report now: the header, then the newest lines that fit.
async fn report(state: &Arc<AppState>, why: &str) -> Result<(), String> {
    let s = saved(state);
    let (lines, upto) = new_lines(state, s.sent_upto);
    if lines.trim().is_empty() && why == "Regular report" {
        return Ok(());
    }
    let head = header(state, why);
    let room = MAX_CHARS.saturating_sub(head.chars().count() + 4);
    let body: String = {
        let chars: Vec<char> = lines.chars().collect();
        let start = chars.len().saturating_sub(room);
        chars[start..].iter().collect()
    };
    let text = format!("{head}\n\n{body}");
    send(state, &text).await?;
    let conn = state.db.lock().unwrap();
    let mut s = db::get::<Saved>(&conn, KEY).unwrap_or_default();
    s.sent_upto = upto;
    s.reports += 1;
    s.last_sent = db::now_ms();
    db::set(&conn, KEY, &s)?;
    Ok(())
}

async fn send(state: &AppState, text: &str) -> Result<(), String> {
    crate::feedback::send("dev", text, None, true).await?;
    LAST.store(db::now_ms(), Ordering::Relaxed);
    // What left the PC, for the person to read (Settings → Logs).
    let when = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let path = state.paths.logs.join(SENT_LOG);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        use std::io::Write;
        let _ = writeln!(f, "===== Sent {when} =====\n{text}\n");
    }
    Ok(())
}

/// A crash: sent at once (already tidied by the crash log), when developer mode is on.
pub fn crash(state: &Arc<AppState>, what: &str, details: &str) {
    if !is_on() {
        return;
    }
    let st = state.clone();
    let text = format!("{}\n\n{what}\n{}", header(state, "Crash"), crate::crash::tidy(details).chars().take(6000).collect::<String>());
    tauri::async_runtime::spawn(async move {
        if let Err(e) = send(&st, &text).await {
            crate::applog::line("app", &format!("Crash report to the developer not sent: {e}"));
        }
    });
}

/// Something failed (a picture, a model load): a report soon, at most one
/// every 10 minutes, so the developer sees errors without waiting.
pub fn soon(state: &Arc<AppState>) {
    if !is_on() || db::now_ms() - LAST.load(Ordering::Relaxed) < 10 * 60_000 {
        return;
    }
    LAST.store(db::now_ms(), Ordering::Relaxed);
    let st = state.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(60)).await;
        let _ = report(&st, "After an error").await;
    });
}

// ---------- commands ----------

#[derive(Serialize)]
pub struct DevModeView {
    on: bool,
    since: Option<i64>,
    reports: u32,
    last_sent: Option<i64>,
}

#[tauri::command(async)]
pub fn dev_mode_view(state: AppStateRef) -> DevModeView {
    let s = saved(&state);
    DevModeView { on: s.on, since: (s.since > 0).then_some(s.since), reports: s.reports, last_sent: (s.last_sent > 0).then_some(s.last_sent) }
}

/// On only with `understood` (the person read what it sends).
#[tauri::command(async)]
pub fn set_dev_mode(state: AppStateRef, on: bool, understood: bool) -> Result<DevModeView, String> {
    state.cipher()?;
    if on && !understood {
        return Err("Read what developer mode sends, then confirm.".into());
    }
    {
        let conn = state.db.lock().unwrap();
        let mut s = db::get::<Saved>(&conn, KEY).unwrap_or_default();
        s.on = on;
        if on {
            s.since = db::now_ms();
            // Only what happens from now on is sent.
            s.sent_upto = state.paths.logs.join(crate::applog::APP_LOG).metadata().map(|m| m.len()).unwrap_or(0);
        }
        db::set(&conn, KEY, &s)?;
        db::log_action(&conn, "privacy", if on { "Turned on developer mode (reports go to the developer to improve SulcusAI)" } else { "Turned off developer mode" });
    }
    ON.store(on, Ordering::Relaxed);
    Ok(dev_mode_view(state))
}

/// "Send a report now" (developer mode on).
#[tauri::command]
pub async fn send_dev_report(state: AppStateRef<'_>) -> Result<DevModeView, String> {
    if !is_on() {
        return Err("Developer mode is off.".into());
    }
    report(state.inner(), "Sent from Settings").await?;
    Ok(dev_mode_view(state))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_engine_side_of_the_log_goes() {
        let lines = "2026-10-17 10:20:01 [app] Started SulcusAI\n2026-10-17 10:21:00 [settings] Added Mom's iPhone to the phone page\n2026-10-17 10:22:05 [media] generate with z-image-turbo: done in 41 s\n2026-10-17 10:23:00 [people] Ben sent a message";
        let kept: Vec<&str> = lines.lines().filter(|l| l.split_once("] ").map(|(h, _)| h).and_then(|h| h.rsplit_once('[')).is_some_and(|(_, c)| SENT.contains(&c))).collect();
        assert_eq!(kept.len(), 2);
        assert!(kept[0].contains("[app]") && kept[1].contains("[media]"));
    }

    /// Sends one real report to the developer's inbox (marked as a test),
    /// with only the engine side of the log, and keeps a copy in dev-sent.log.
    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore]
    async fn a_report_reaches_the_inbox() {
        let state = crate::e2e::scratch_state();
        let log = "2026-10-17 12:00:00 [media] Test line from the 0.9.6 release checks\n2026-10-17 12:00:01 [settings] Added Mom's iPhone (must not be sent)\n";
        std::fs::write(state.paths.logs.join(crate::applog::APP_LOG), log).unwrap();
        report(&state, "Test from the 0.9.6 release checks").await.unwrap();
        let sent = std::fs::read_to_string(state.paths.logs.join(SENT_LOG)).unwrap();
        println!("{sent}");
        assert!(sent.contains("Test line from the 0.9.6") && !sent.contains("iPhone"));
        assert_eq!(saved(&state).reports, 1);
    }
}
