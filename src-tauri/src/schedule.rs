// SPDX-License-Identifier: AGPL-3.0-only
//! Scheduled tasks: prompts that run on their own at set times, each run
//! saved as a chat. Nobody is there to approve changes, so tasks only look
//! (read-only tools) unless the user allowed changes when creating them.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Datelike, Local, NaiveTime, TimeZone, Timelike, Weekday};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};

use crate::crypto::Cipher;
use crate::db::{self, now_ms, Message};
use crate::{agent, chat, tools, AppState, AppStateRef};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Cadence {
    /// One run at a local date and time (ms since epoch).
    Once { at: i64 },
    /// Every hour at this minute.
    Hourly { minute: u32 },
    Daily { time: String },
    /// Monday to Friday.
    Weekdays { time: String },
    /// 0 = Monday … 6 = Sunday.
    Weekly { weekday: u32, time: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schedule {
    pub id: String,
    pub name: String,
    pub prompt: String,
    pub cadence: Cadence,
    pub model_id: Option<String>,
    pub project_id: Option<String>,
    /// Run in Bypass mode with every tool; otherwise read-only.
    pub allow_changes: bool,
    pub enabled: bool,
    pub last_run: Option<i64>,
    pub next_run: Option<i64>,
    pub last_chat: Option<String>,
}

fn parse_time(t: &str) -> Result<NaiveTime, String> {
    NaiveTime::parse_from_str(t, "%H:%M").map_err(|_| "Use a time like 08:30.".to_string())
}

fn weekday(n: u32) -> Weekday {
    match n % 7 {
        0 => Weekday::Mon,
        1 => Weekday::Tue,
        2 => Weekday::Wed,
        3 => Weekday::Thu,
        4 => Weekday::Fri,
        5 => Weekday::Sat,
        _ => Weekday::Sun,
    }
}

fn at_local(date: chrono::NaiveDate, t: NaiveTime) -> Option<DateTime<Local>> {
    // On a daylight-saving gap the time doesn't exist; earliest valid wins.
    Local.from_local_datetime(&date.and_time(t)).earliest()
}

/// The first run strictly after `after`, or None if there is none (a past one-off).
pub fn next_run(c: &Cadence, after: DateTime<Local>) -> Result<Option<DateTime<Local>>, String> {
    Ok(match c {
        Cadence::Once { at } => Local.timestamp_millis_opt(*at).single().filter(|t| *t > after),
        Cadence::Hourly { minute } => {
            if *minute > 59 {
                return Err("Pick a minute from 0 to 59.".into());
            }
            let base = after.with_second(0).and_then(|t| t.with_nanosecond(0)).ok_or("bad time")?;
            let candidate = base.with_minute(*minute).ok_or("bad time")?;
            Some(if candidate > after { candidate } else { candidate + chrono::Duration::hours(1) })
        }
        Cadence::Daily { time } | Cadence::Weekdays { time } | Cadence::Weekly { time, .. } => {
            let t = parse_time(time)?;
            let mut day = after.date_naive();
            for _ in 0..8 {
                let ok_day = match c {
                    Cadence::Weekdays { .. } => !matches!(day.weekday(), Weekday::Sat | Weekday::Sun),
                    Cadence::Weekly { weekday: w, .. } => day.weekday() == weekday(*w),
                    _ => true,
                };
                if ok_day {
                    if let Some(when) = at_local(day, t).filter(|w| *w > after) {
                        return Ok(Some(when));
                    }
                }
                day = day.succ_opt().ok_or("bad date")?;
            }
            None
        }
    })
}

/// A plain-language description, such as "Weekdays at 08:30".
pub fn describe(c: &Cadence) -> String {
    const DAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
    match c {
        Cadence::Once { at } => Local
            .timestamp_millis_opt(*at)
            .single()
            .map_or("Once".into(), |t| format!("Once, {}", t.format("%b %-d at %H:%M"))),
        Cadence::Hourly { minute } => format!("Every hour at :{minute:02}"),
        Cadence::Daily { time } => format!("Every day at {time}"),
        Cadence::Weekdays { time } => format!("Weekdays at {time}"),
        Cadence::Weekly { weekday, time } => format!("Every {} at {time}", DAYS[(*weekday % 7) as usize]),
    }
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

pub fn list(conn: &Connection, c: &Cipher) -> Vec<Schedule> {
    let Ok(mut stmt) = conn.prepare("SELECT id, data FROM schedules ORDER BY created_at") else { return Vec::new() };
    stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map(|rows| {
            rows.filter_map(Result::ok)
                .filter_map(|(_, data)| c.decrypt(&data).ok().and_then(|j| serde_json::from_str(&j).ok()))
                .collect()
        })
        .unwrap_or_default()
}

pub fn save(conn: &Connection, c: &Cipher, s: &Schedule) -> Result<(), String> {
    let data = c.encrypt(&serde_json::to_string(s).map_err(err)?);
    conn.execute(
        "INSERT INTO schedules (id, data, next_run, created_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(id) DO UPDATE SET data = excluded.data, next_run = excluded.next_run",
        params![s.id, data, s.next_run, now_ms()],
    )
    .map_err(err)?;
    Ok(())
}

pub fn delete(conn: &Connection, id: &str) -> Result<(), String> {
    conn.execute("DELETE FROM schedules WHERE id = ?1", [id]).map_err(err)?;
    Ok(())
}

/// Runs one schedule now and saves the result as a chat. Returns the chat id.
pub async fn run(app: &AppHandle, state: &Arc<AppState>, s: &Schedule) -> Result<String, String> {
    let cipher = state.cipher()?;
    let (profile, title) = {
        let conn = state.db.lock().unwrap();
        let title = format!("⏰ {} · {}", s.name, Local::now().format("%b %-d, %H:%M"));
        (db::profile(&conn, &cipher), title)
    };
    let (ep, spec, installed) = crate::llm_endpoint(state, s.model_id.clone(), || {}).await?;

    let (chat_id, turn_id) = {
        let conn = state.db.lock().unwrap();
        let chat = db::create_chat_in(&conn, &cipher, Some(installed.model_id.clone()), s.project_id.clone(), false)?;
        db::set_chat_title(&conn, &cipher, &chat.id, &title)?;
        db::set_chat_mode(&conn, &chat.id, if s.allow_changes { "bypass" } else { "plan" })?;
        let turn_id = uuid::Uuid::new_v4().to_string();
        db::add_message(&conn, &cipher, &Message {
            id: turn_id.clone(),
            chat_id: chat.id.clone(),
            role: "user".into(),
            content: s.prompt.clone(),
            created_at: now_ms(),
            ..Default::default()
        })?;
        (chat.id, turn_id)
    };
    let Some((cancel, _guard)) = crate::claim(&state.generations, &chat_id) else { return Err("Already running.".into()) };
    let today = Local::now().format("%A, %B %-d, %Y").to_string();
    let (base, about) = chat::system_prompt(&profile, &today);
    let base = format!("{base}\n\nThis is a scheduled task (\"{}\") running on its own; the user isn't watching. Do the task and reply with the result.", s.name);
    let emitter = app.clone();
    let turn = agent::Turn {
        emit: Arc::new(move |event: &str, payload: serde_json::Value| {
            emitter.emit(event, payload).ok();
        }),
        state: state.clone(),
        chat_id: chat_id.clone(),
        turn_id,
        cipher,
        ep,
        mode: if s.allow_changes { tools::Mode::Bypass } else { tools::Mode::Plan },
        use_tools: spec.tools,
        base,
        about,
        cancel,
    };
    let result = turn.run().await;
    app.emit("chat:done", json!({ "chat_id": chat_id, "error": result.as_ref().err() })).ok();
    result.map(|_| chat_id)
}

/// Checks every half minute for due tasks while the app is open and unlocked.
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let busy = AtomicBool::new(false);
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            let state = app.state::<Arc<AppState>>().inner().clone();
            let Ok(cipher) = state.cipher() else { continue };
            if !crate::features::is_on(&state.db.lock().unwrap(), crate::features::Feature::Scheduled) {
                continue;
            }
            // Don't interrupt someone mid-chat; try again next tick.
            if !state.generations.lock().unwrap().is_empty() || busy.load(std::sync::atomic::Ordering::Relaxed) {
                continue;
            }
            let now = Local::now();
            let due: Vec<Schedule> = list(&state.db.lock().unwrap(), &cipher)
                .into_iter()
                .filter(|s| s.enabled && s.next_run.is_some_and(|n| n <= now.timestamp_millis()))
                .collect();
            for mut s in due {
                busy.store(true, std::sync::atomic::Ordering::Relaxed);
                let outcome = run(&app, &state, &s).await;
                busy.store(false, std::sync::atomic::Ordering::Relaxed);
                s.last_run = Some(now_ms());
                s.next_run = next_run(&s.cadence, Local::now()).ok().flatten().map(|t| t.timestamp_millis());
                if matches!(s.cadence, Cadence::Once { .. }) {
                    s.enabled = false;
                }
                match &outcome {
                    Ok(chat_id) => {
                        s.last_chat = Some(chat_id.clone());
                        state.log("agent", "A scheduled task ran");
                        app.emit("schedule:ran", json!({ "id": s.id, "name": s.name, "chat_id": chat_id })).ok();
                    }
                    Err(e) => {
                        state.log("agent", "A scheduled task failed to run");
                        app.emit("schedule:ran", json!({ "id": s.id, "name": s.name, "error": e })).ok();
                    }
                }
                let _ = save(&state.db.lock().unwrap(), &cipher, &s);
            }
        }
    });
}

// ---------- commands ----------

#[derive(Serialize)]
pub struct ScheduleView {
    #[serde(flatten)]
    schedule: Schedule,
    when: String,
}

#[tauri::command]
pub fn list_schedules(state: AppStateRef) -> Result<Vec<ScheduleView>, String> {
    let c = state.cipher()?;
    Ok(list(&state.db.lock().unwrap(), &c).into_iter().map(|s| ScheduleView { when: describe(&s.cadence), schedule: s }).collect())
}

#[tauri::command]
pub fn save_schedule(state: AppStateRef, schedule: Schedule) -> Result<ScheduleView, String> {
    let c = state.cipher()?;
    let mut s = schedule;
    s.name = s.name.trim().to_string();
    if s.name.is_empty() || s.prompt.trim().is_empty() {
        return Err("A task needs a name and something to do.".into());
    }
    if s.id.is_empty() {
        s.id = uuid::Uuid::new_v4().to_string();
    }
    s.next_run = next_run(&s.cadence, Local::now())?.map(|t| t.timestamp_millis());
    if s.next_run.is_none() {
        return Err("That time has already passed.".into());
    }
    let conn = state.db.lock().unwrap();
    save(&conn, &c, &s)?;
    db::log_action(&conn, "agent", if s.allow_changes { "Saved a scheduled task that may change files" } else { "Saved a scheduled task" });
    Ok(ScheduleView { when: describe(&s.cadence), schedule: s })
}

#[tauri::command]
pub fn delete_schedule(state: AppStateRef, id: String) -> Result<(), String> {
    state.cipher()?;
    delete(&state.db.lock().unwrap(), &id)
}

#[tauri::command]
pub async fn run_schedule_now(app: AppHandle, state: AppStateRef<'_>, id: String) -> Result<String, String> {
    let c = state.cipher()?;
    let mut s = list(&state.db.lock().unwrap(), &c).into_iter().find(|s| s.id == id).ok_or("That task no longer exists.")?;
    let chat_id = run(&app, state.inner(), &s).await?;
    s.last_run = Some(now_ms());
    s.last_chat = Some(chat_id.clone());
    save(&state.db.lock().unwrap(), &c, &s)?;
    Ok(chat_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, m, d, h, min, 0).single().unwrap()
    }

    #[test]
    fn daily_runs_later_today_or_tomorrow() {
        let c = Cadence::Daily { time: "08:30".into() };
        assert_eq!(next_run(&c, local(2026, 10, 6, 7, 0)).unwrap().unwrap(), local(2026, 10, 6, 8, 30));
        assert_eq!(next_run(&c, local(2026, 10, 6, 8, 30)).unwrap().unwrap(), local(2026, 10, 7, 8, 30));
    }

    #[test]
    fn weekdays_skip_the_weekend() {
        // Friday Oct 9, 2026 after the time → Monday Oct 12.
        let c = Cadence::Weekdays { time: "09:00".into() };
        assert_eq!(next_run(&c, local(2026, 10, 9, 10, 0)).unwrap().unwrap(), local(2026, 10, 12, 9, 0));
    }

    #[test]
    fn weekly_finds_the_right_day() {
        // Tuesday Oct 6, 2026 → next Sunday (6) is Oct 11.
        let c = Cadence::Weekly { weekday: 6, time: "18:00".into() };
        assert_eq!(next_run(&c, local(2026, 10, 6, 12, 0)).unwrap().unwrap(), local(2026, 10, 11, 18, 0));
    }

    #[test]
    fn hourly_and_once() {
        let c = Cadence::Hourly { minute: 15 };
        assert_eq!(next_run(&c, local(2026, 10, 6, 7, 20)).unwrap().unwrap(), local(2026, 10, 6, 8, 15));
        assert!(next_run(&Cadence::Hourly { minute: 60 }, local(2026, 10, 6, 7, 0)).is_err());
        let at = local(2026, 10, 7, 9, 0).timestamp_millis();
        assert!(next_run(&Cadence::Once { at }, local(2026, 10, 6, 9, 0)).unwrap().is_some());
        assert!(next_run(&Cadence::Once { at }, local(2026, 10, 8, 9, 0)).unwrap().is_none());
        assert!(next_run(&Cadence::Daily { time: "8:3x".into() }, local(2026, 10, 6, 7, 0)).is_err());
    }

    #[test]
    fn descriptions_read_naturally() {
        assert_eq!(describe(&Cadence::Weekdays { time: "08:30".into() }), "Weekdays at 08:30");
        assert_eq!(describe(&Cadence::Weekly { weekday: 0, time: "07:00".into() }), "Every Monday at 07:00");
        assert_eq!(describe(&Cadence::Hourly { minute: 5 }), "Every hour at :05");
    }

    #[test]
    fn schedules_are_stored_encrypted() {
        use crate::crypto::{Protector, Vault};
        struct Plain;
        impl Protector for Plain {
            fn protect(&self, d: &[u8]) -> Result<Vec<u8>, String> {
                Ok(d.to_vec())
            }
            fn unprotect(&self, d: &[u8]) -> Result<Vec<u8>, String> {
                Ok(d.to_vec())
            }
        }
        let d = std::env::temp_dir().join(format!("sulcusai-sch-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        let conn = db::open(&d.join("t.db")).unwrap();
        let c = Vault::open(&d.join("keys.json"), Box::new(Plain)).unwrap().cipher().unwrap();
        let s = Schedule {
            id: "s1".into(),
            name: "Morning news digest".into(),
            prompt: "Summarize my notes folder".into(),
            cadence: Cadence::Daily { time: "07:00".into() },
            model_id: None,
            project_id: None,
            allow_changes: false,
            enabled: true,
            last_run: None,
            next_run: Some(1),
            last_chat: None,
        };
        save(&conn, &c, &s).unwrap();
        let raw: String = conn.query_row("SELECT data FROM schedules", [], |r| r.get(0)).unwrap();
        assert!(!raw.contains("Morning"));
        assert_eq!(list(&conn, &c)[0].name, "Morning news digest");
        delete(&conn, "s1").unwrap();
        assert!(list(&conn, &c).is_empty());
    }
}
