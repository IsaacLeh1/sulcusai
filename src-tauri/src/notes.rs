// SPDX-License-Identifier: AGPL-3.0-only
//! Notes (Markdown, folders, tags, pins) and tasks (due dates, priorities,
//! subtasks, reminders). Titles and text are encrypted; due dates, priority
//! and done state are kept plain so lists sort and reminders fire.

use std::sync::Arc;
use std::time::Duration;

use chrono::{Datelike, Local, NaiveDate, NaiveTime, TimeZone, Weekday};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};

use crate::crypto::Cipher;
use crate::db;
use crate::features::{self, Feature};
use crate::{AppState, AppStateRef};

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

// ---------- notes ----------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct NoteBody {
    pub title: String,
    pub body: String,
    pub folder: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Note {
    pub id: String,
    #[serde(flatten)]
    pub data: NoteBody,
    pub pinned: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

pub fn list_notes(conn: &Connection, c: &Cipher) -> Vec<Note> {
    let Ok(mut stmt) = conn.prepare("SELECT id, data, pinned, created_at, updated_at FROM notes ORDER BY pinned DESC, updated_at DESC") else {
        return Vec::new();
    };
    stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get(3)?, r.get(4)?)))
        .map(|rows| {
            rows.filter_map(Result::ok)
                .map(|(id, data, pinned, created_at, updated_at)| Note {
                    id,
                    data: c.decrypt(&data).ok().and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default(),
                    pinned: pinned != 0,
                    created_at,
                    updated_at,
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn get_note(conn: &Connection, c: &Cipher, id: &str) -> Option<Note> {
    list_notes(conn, c).into_iter().find(|n| n.id == id)
}

fn clean_note(mut n: NoteBody) -> NoteBody {
    n.title = n.title.trim().chars().take(200).collect();
    if n.title.is_empty() {
        n.title = n.body.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("Untitled").trim_start_matches('#').trim().chars().take(80).collect();
    }
    n.folder = n.folder.trim().trim_matches('/').to_string();
    let mut tags: Vec<String> = n.tags.iter().map(|t| t.trim().trim_start_matches('#').to_lowercase()).filter(|t| !t.is_empty()).collect();
    tags.sort();
    tags.dedup();
    n.tags = tags;
    n
}

pub fn save_note(conn: &Connection, c: &Cipher, id: Option<&str>, body: NoteBody) -> Result<Note, String> {
    let body = clean_note(body);
    let now = db::now_ms();
    let data = c.encrypt(&serde_json::to_string(&body).map_err(err)?);
    let id = match id {
        Some(id) => {
            let n = conn.execute("UPDATE notes SET data = ?2, updated_at = ?3 WHERE id = ?1", params![id, data, now]).map_err(err)?;
            if n == 0 {
                return Err("That note no longer exists.".into());
            }
            id.to_string()
        }
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO notes (id, data, pinned, created_at, updated_at) VALUES (?1, ?2, 0, ?3, ?3)",
                params![id, data, now],
            )
            .map_err(err)?;
            id
        }
    };
    get_note(conn, c, &id).ok_or_else(|| "Couldn't save the note.".into())
}

/// Notes containing every word of the query (title, body, folder or tags).
pub fn search_notes(conn: &Connection, c: &Cipher, query: &str) -> Vec<Note> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut hits: Vec<(usize, Note)> = list_notes(conn, c)
        .into_iter()
        .filter_map(|n| {
            let title = n.data.title.to_lowercase();
            let hay = format!("{} {} {} {}", title, n.data.body.to_lowercase(), n.data.folder.to_lowercase(), n.data.tags.join(" "));
            if !words.iter().all(|w| hay.contains(w.as_str())) {
                return None;
            }
            let score = words.iter().filter(|w| title.contains(w.as_str())).count();
            Some((score, n))
        })
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.updated_at.cmp(&a.1.updated_at)));
    hits.into_iter().map(|(_, n)| n).collect()
}

// ---------- tasks ----------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Subtask {
    pub title: String,
    #[serde(default)]
    pub done: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TaskBody {
    pub title: String,
    pub notes: String,
    pub subtasks: Vec<Subtask>,
    /// Where it came from, e.g. a meeting.
    pub source: Option<TaskSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSource {
    pub kind: String,
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    #[serde(flatten)]
    pub data: TaskBody,
    /// Due date, ms since the epoch (local midnight for date-only).
    pub due: Option<i64>,
    /// The due time was given, not just the date.
    #[serde(default)]
    pub due_has_time: bool,
    /// 0 none, 1 low, 2 medium, 3 high.
    pub priority: u8,
    pub remind_at: Option<i64>,
    pub done_at: Option<i64>,
    #[serde(default)]
    pub created_at: i64,
}

pub fn list_tasks(conn: &Connection, c: &Cipher) -> Vec<Task> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT id, data, due, due_has_time, priority, remind_at, done_at, created_at FROM tasks
         ORDER BY done_at IS NOT NULL, due IS NULL, due, priority DESC, created_at",
    ) else {
        return Vec::new();
    };
    stmt.query_map([], |r| {
        Ok(Task {
            id: r.get(0)?,
            data: c.decrypt(&r.get::<_, String>(1)?).ok().and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default(),
            due: r.get(2)?,
            due_has_time: r.get::<_, i64>(3)? != 0,
            priority: r.get::<_, i64>(4)? as u8,
            remind_at: r.get(5)?,
            done_at: r.get(6)?,
            created_at: r.get(7)?,
        })
    })
    .map(|rows| rows.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

pub fn get_task(conn: &Connection, c: &Cipher, id: &str) -> Option<Task> {
    list_tasks(conn, c).into_iter().find(|t| t.id == id)
}

/// Creates (empty id) or updates a task.
pub fn save_task(conn: &Connection, c: &Cipher, mut t: Task) -> Result<Task, String> {
    t.data.title = t.data.title.trim().chars().take(300).collect();
    if t.data.title.is_empty() {
        return Err("A task needs a title.".into());
    }
    t.priority = t.priority.min(3);
    let data = c.encrypt(&serde_json::to_string(&t.data).map_err(err)?);
    if t.id.is_empty() {
        t.id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO tasks (id, data, due, due_has_time, priority, remind_at, reminded, done_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?8)",
            params![t.id, data, t.due, t.due_has_time as i64, t.priority as i64, t.remind_at, t.done_at, db::now_ms()],
        )
        .map_err(err)?;
    } else {
        // A changed reminder time can fire again.
        let n = conn
            .execute(
                "UPDATE tasks SET data = ?2, due = ?3, due_has_time = ?4, priority = ?5,
                 reminded = CASE WHEN remind_at IS ?6 THEN reminded ELSE 0 END, remind_at = ?6, done_at = ?7 WHERE id = ?1",
                params![t.id, data, t.due, t.due_has_time as i64, t.priority as i64, t.remind_at, t.done_at],
            )
            .map_err(err)?;
        if n == 0 {
            return Err("That task no longer exists.".into());
        }
    }
    get_task(conn, c, &t.id).ok_or_else(|| "Couldn't save the task.".into())
}

pub fn new_task(title: &str) -> Task {
    Task {
        id: String::new(),
        data: TaskBody { title: title.into(), ..Default::default() },
        due: None,
        due_has_time: false,
        priority: 0,
        remind_at: None,
        done_at: None,
        created_at: 0,
    }
}

// ---------- dates ----------

fn weekday(s: &str) -> Option<Weekday> {
    Some(match s.get(..3)? {
        "mon" => Weekday::Mon,
        "tue" => Weekday::Tue,
        "wed" => Weekday::Wed,
        "thu" => Weekday::Thu,
        "fri" => Weekday::Fri,
        "sat" => Weekday::Sat,
        "sun" => Weekday::Sun,
        _ => return None,
    })
}

fn month(s: &str) -> Option<u32> {
    const M: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    M.iter().position(|m| s.starts_with(m)).map(|i| i as u32 + 1)
}

/// Reads a time like "3pm", "3:30 pm", "15:00" or "noon".
fn time_of(s: &str) -> Option<NaiveTime> {
    let s = s.trim().trim_start_matches("at ").trim().to_lowercase();
    if s == "noon" || s == "midday" {
        return NaiveTime::from_hms_opt(12, 0, 0);
    }
    let (num, pm, am) = if let Some(x) = s.strip_suffix("pm") {
        (x.trim(), true, false)
    } else if let Some(x) = s.strip_suffix("am") {
        (x.trim(), false, true)
    } else {
        (s.as_str(), false, false)
    };
    let (h, m) = match num.split_once(':') {
        Some((h, m)) => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        None => (num.parse::<u32>().ok()?, 0),
    };
    if !(pm || am || num.contains(':')) {
        return None;
    }
    let h = match (h, pm, am) {
        (12, false, true) => 0,
        (h, true, _) if h < 12 => h + 12,
        (h, _, _) => h,
    };
    NaiveTime::from_hms_opt(h, m, 0)
}

/// Turns "Friday", "tomorrow 3pm", "next week", "Oct 12" or "2026-10-12"
/// into a date (and maybe a time), counting from `from`.
pub fn parse_due(text: &str, from: NaiveDate) -> Option<(NaiveDate, Option<NaiveTime>)> {
    let t = text.trim().to_lowercase();
    let t = t.trim_start_matches("by ").trim_start_matches("due ").trim_start_matches("on ").trim();
    if t.is_empty() {
        return None;
    }
    // Split off a trailing time ("friday 3pm", "tomorrow at 9:30").
    let words: Vec<&str> = t.split_whitespace().collect();
    let mut time = None;
    let mut date_words = words.clone();
    for k in (1..words.len()).rev() {
        if let Some(tm) = time_of(&words[k..].join(" ")) {
            time = Some(tm);
            date_words = words[..k].to_vec();
            break;
        }
    }
    if let Some(tm) = time_of(t) {
        return Some((from, Some(tm)));
    }
    let d = date_words.join(" ");
    let d = d.trim_end_matches(',').trim_end_matches(" at").trim_end_matches(" afternoon").trim_end_matches(" morning").trim_end_matches(" evening").trim();
    let date = match d {
        "today" | "tonight" | "end of day" | "eod" => from,
        "tomorrow" => from.succ_opt()?,
        "next week" => from + chrono::Duration::days(7),
        "end of the week" | "end of week" | "this week" => {
            let ahead = (Weekday::Fri.num_days_from_monday() as i64 - from.weekday().num_days_from_monday() as i64).rem_euclid(7);
            from + chrono::Duration::days(ahead)
        }
        _ => {
            if let Ok(iso) = NaiveDate::parse_from_str(d, "%Y-%m-%d") {
                iso
            } else if let Some(wd) = weekday(d.trim_start_matches("next ").trim_start_matches("this ")) {
                let mut ahead = (wd.num_days_from_monday() as i64 - from.weekday().num_days_from_monday() as i64).rem_euclid(7);
                // The same weekday as today means next week's.
                if ahead == 0 {
                    ahead = 7;
                }
                from + chrono::Duration::days(ahead)
            } else {
                // "oct 12", "12 october", "october 12, 2027"
                let parts: Vec<&str> = d.split(|c: char| c == ' ' || c == ',').filter(|p| !p.is_empty()).collect();
                let (mut mon, mut day, mut year) = (None, None, None);
                for p in parts {
                    let p = p.trim_end_matches("st").trim_end_matches("nd").trim_end_matches("rd").trim_end_matches("th");
                    if let Some(m) = month(p) {
                        mon = Some(m);
                    } else if let Ok(n) = p.parse::<i32>() {
                        if n > 31 { year = Some(n) } else { day = Some(n as u32) }
                    }
                }
                let (m, dd) = (mon?, day?);
                let mut date = NaiveDate::from_ymd_opt(year.unwrap_or(from.year()), m, dd)?;
                if year.is_none() && date < from {
                    date = NaiveDate::from_ymd_opt(from.year() + 1, m, dd)?;
                }
                date
            }
        }
    };
    Some((date, time))
}

/// ms since the epoch for a local date (and time, else midnight).
pub fn to_ms(date: NaiveDate, time: Option<NaiveTime>) -> Option<i64> {
    let dt = date.and_time(time.unwrap_or(NaiveTime::MIN));
    Local.from_local_datetime(&dt).earliest().map(|d| d.timestamp_millis())
}

// ---------- reminders ----------

/// Every 30 s: due reminders become a notification and an in-app toast.
pub fn start_reminders(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            let state = app.state::<Arc<AppState>>().inner().clone();
            let Ok(cipher) = state.cipher() else { continue };
            let due: Vec<Task> = {
                let conn = state.db.lock().unwrap();
                if !features::is_on(&conn, Feature::Tasks) {
                    continue;
                }
                let now = db::now_ms();
                let ids: Vec<String> = conn
                    .prepare("SELECT id FROM tasks WHERE remind_at IS NOT NULL AND remind_at <= ?1 AND reminded = 0 AND done_at IS NULL")
                    .and_then(|mut s| s.query_map([now], |r| r.get(0)).map(|r| r.filter_map(Result::ok).collect()))
                    .unwrap_or_default();
                for id in &ids {
                    let _ = conn.execute("UPDATE tasks SET reminded = 1 WHERE id = ?1", [id]);
                }
                ids.iter().filter_map(|id| get_task(&conn, &cipher, id)).collect()
            };
            for t in due {
                crate::notify::show(&app, "Reminder", &t.data.title);
                app.emit("task:reminder", json!({ "id": t.id, "title": t.data.title })).ok();
            }
        }
    });
}

// ---------- commands ----------

#[tauri::command]
pub fn notes(state: AppStateRef) -> Result<Vec<Note>, String> {
    let c = state.cipher()?;
    Ok(list_notes(&state.db.lock().unwrap(), &c))
}

#[tauri::command]
pub fn save_note_cmd(state: AppStateRef, id: Option<String>, note: NoteBody) -> Result<Note, String> {
    let c = state.cipher()?;
    features::require(&state, Feature::Notes)?;
    save_note(&state.db.lock().unwrap(), &c, id.as_deref(), note)
}

#[tauri::command]
pub fn pin_note(state: AppStateRef, id: String, pinned: bool) -> Result<(), String> {
    state.cipher()?;
    state.db.lock().unwrap().execute("UPDATE notes SET pinned = ?2 WHERE id = ?1", params![id, pinned as i64]).map_err(err)?;
    Ok(())
}

#[tauri::command]
pub fn delete_note(state: AppStateRef, id: String) -> Result<(), String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    conn.execute("DELETE FROM notes WHERE id = ?1", [&id]).map_err(err)?;
    db::log_action(&conn, "notes", "Deleted a note");
    Ok(())
}

#[tauri::command]
pub fn tasks(state: AppStateRef) -> Result<Vec<Task>, String> {
    let c = state.cipher()?;
    Ok(list_tasks(&state.db.lock().unwrap(), &c))
}

#[tauri::command]
pub fn save_task_cmd(state: AppStateRef, task: Task) -> Result<Task, String> {
    let c = state.cipher()?;
    features::require(&state, Feature::Tasks)?;
    save_task(&state.db.lock().unwrap(), &c, task)
}

#[tauri::command]
pub fn delete_task(state: AppStateRef, id: String) -> Result<(), String> {
    state.cipher()?;
    state.db.lock().unwrap().execute("DELETE FROM tasks WHERE id = ?1", [&id]).map_err(err)?;
    Ok(())
}

/// Reads a typed due date, for the task editor's date box.
#[tauri::command]
pub fn parse_due_text(text: String) -> Option<serde_json::Value> {
    let (d, t) = parse_due(&text, Local::now().date_naive())?;
    Some(json!({ "due": to_ms(d, t), "has_time": t.is_some() }))
}

/// Adds a meeting's action items as tasks; due words such as "Friday"
/// count from the meeting's date. Returns how many were added.
#[tauri::command]
pub fn tasks_from_meeting(state: AppStateRef, meeting_id: String, indexes: Vec<usize>) -> Result<usize, String> {
    let c = state.cipher()?;
    features::require(&state, Feature::Tasks)?;
    let conn = state.db.lock().unwrap();
    let m = crate::meeting::list(&conn, &c).into_iter().find(|m| m.id == meeting_id).ok_or("That meeting no longer exists.")?;
    let notes = m.data.notes.as_ref().ok_or("This meeting has no notes yet.")?;
    let from = chrono::DateTime::from_timestamp_millis(m.started_at).map_or(Local::now().date_naive(), |d| d.with_timezone(&Local).date_naive());
    let already: Vec<String> = list_tasks(&conn, &c)
        .into_iter()
        .filter(|t| t.data.source.as_ref().is_some_and(|s| s.id == meeting_id))
        .map(|t| t.data.title)
        .collect();
    let mut added = 0;
    for i in indexes {
        let Some(a) = notes.action_items.get(i) else { continue };
        if already.contains(&a.task) {
            continue;
        }
        let mut t = new_task(&a.task);
        if let Some((d, tm)) = parse_due(&a.due, from) {
            t.due = to_ms(d, tm);
            t.due_has_time = tm.is_some();
        }
        let mut extra = Vec::new();
        if !a.owner.is_empty() {
            extra.push(format!("Owner: {}", a.owner));
        }
        if !a.due.is_empty() && t.due.is_none() {
            extra.push(format!("Due: {}", a.due));
        }
        t.data.notes = extra.join("\n");
        t.data.source = Some(TaskSource { kind: "meeting".into(), id: meeting_id.clone(), label: m.data.title.clone() });
        save_task(&conn, &c, t)?;
        added += 1;
    }
    db::log_action(&conn, "tasks", &format!("Added {added} tasks from a meeting"));
    Ok(added)
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn setup() -> (Connection, Cipher) {
        let d = std::env::temp_dir().join(format!("sulcusai-nt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        (db::open(&d.join("t.db")).unwrap(), Vault::open(&d.join("keys.json"), Box::new(Plain)).unwrap().cipher().unwrap())
    }

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn due_words_become_dates() {
        let tue = day(2026, 10, 6); // a Tuesday
        assert_eq!(parse_due("Friday", tue), Some((day(2026, 10, 9), None)));
        assert_eq!(parse_due("by Friday afternoon", tue).unwrap().0, day(2026, 10, 9));
        assert_eq!(parse_due("tomorrow", tue), Some((day(2026, 10, 7), None)));
        assert_eq!(parse_due("Tuesday", tue).unwrap().0, day(2026, 10, 13), "the same weekday means next week");
        assert_eq!(parse_due("next week", tue).unwrap().0, day(2026, 10, 13));
        assert_eq!(parse_due("Oct 12", tue).unwrap().0, day(2026, 10, 12));
        assert_eq!(parse_due("March 3rd", tue).unwrap().0, day(2027, 3, 3), "a past month rolls to next year");
        assert_eq!(parse_due("2026-12-01", tue).unwrap().0, day(2026, 12, 1));
        assert_eq!(parse_due("end of the week", tue).unwrap().0, day(2026, 10, 9));
        let (d, t) = parse_due("tomorrow at 3:30pm", tue).unwrap();
        assert_eq!((d, t), (day(2026, 10, 7), NaiveTime::from_hms_opt(15, 30, 0)));
        assert_eq!(parse_due("noon", tue).unwrap().1, NaiveTime::from_hms_opt(12, 0, 0));
        assert_eq!(parse_due("soon", tue), None);
        assert_eq!(parse_due("", tue), None);
    }

    #[test]
    fn notes_are_encrypted_titled_and_searchable() {
        let (conn, c) = setup();
        let n = save_note(&conn, &c, None, NoteBody { body: "# Trip ideas\nKyoto in spring".into(), tags: vec!["#Travel".into(), "travel".into()], ..Default::default() }).unwrap();
        assert_eq!(n.data.title, "Trip ideas");
        assert_eq!(n.data.tags, vec!["travel"]);
        let raw: String = conn.query_row("SELECT data FROM notes", [], |r| r.get(0)).unwrap();
        assert!(!raw.contains("Kyoto"));
        assert_eq!(search_notes(&conn, &c, "kyoto spring").len(), 1);
        assert!(search_notes(&conn, &c, "kyoto winter").is_empty());
        let n2 = save_note(&conn, &c, Some(&n.id), NoteBody { title: "Japan".into(), body: "Kyoto".into(), ..Default::default() }).unwrap();
        assert_eq!(n2.id, n.id);
        assert_eq!(list_notes(&conn, &c).len(), 1);
    }

    #[test]
    fn tasks_sort_open_first_by_due_date() {
        let (conn, c) = setup();
        let mut a = new_task("later");
        a.due = Some(2_000);
        let mut b = new_task("sooner");
        b.due = Some(1_000);
        let mut d = new_task("done");
        d.due = Some(500);
        d.done_at = Some(5);
        let none = new_task("whenever");
        for t in [a, b, d, none] {
            save_task(&conn, &c, t).unwrap();
        }
        let titles: Vec<String> = list_tasks(&conn, &c).into_iter().map(|t| t.data.title).collect();
        assert_eq!(titles, vec!["sooner", "later", "whenever", "done"]);
        assert!(save_task(&conn, &c, new_task("  ")).is_err());
    }

    #[test]
    fn a_moved_reminder_can_fire_again() {
        let (conn, c) = setup();
        let mut t = new_task("call");
        t.remind_at = Some(100);
        let t = save_task(&conn, &c, t).unwrap();
        conn.execute("UPDATE tasks SET reminded = 1", []).unwrap();
        let mut moved = t.clone();
        moved.remind_at = Some(200);
        save_task(&conn, &c, moved).unwrap();
        let r: i64 = conn.query_row("SELECT reminded FROM tasks", [], |r| r.get(0)).unwrap();
        assert_eq!(r, 0);
    }
}
