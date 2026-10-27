// SPDX-License-Identifier: AGPL-3.0-only
//! schedule_task: sets up a scheduled task when the user asks for something
//! to happen by itself at set times ("every morning at 8, give me the news")
//! or when something happens ("when mail from Dana comes in, sum it up"),
//! the way ChatGPT's Tasks work. The user approves it first, and it runs
//! read-only, like the tasks made under Scheduled.

use chrono::{Local, NaiveDate, NaiveTime, TimeZone};
use serde_json::{json, Value};

use super::{Ctx, Outcome, Preview};
use crate::schedule::{self, Cadence, Schedule};

pub fn schedule_task_params() -> Value {
    json!({ "type": "object", "required": ["name", "prompt", "repeat"], "properties": {
        "name": { "type": "string", "description": "A short name, e.g. \"Morning news\"." },
        "prompt": { "type": "string", "description": "What to do each time, written as the user's request, e.g. \"Give me today's top 5 news stories, a few lines each.\"" },
        "repeat": { "type": "string", "enum": ["once", "hourly", "daily", "weekdays", "weekly", "days", "dates", "on_mail", "on_event", "on_file", "on_start"],
            "description": "When it runs: at set times, or when something happens: on_mail (new mail arrives), on_event (a calendar event is about to start), on_file (a file appears in a folder), on_start (SulcusAI starts)." },
        "from": { "type": "string", "description": "For on_mail: only mail from this person (part of their name or address)." },
        "subject": { "type": "string", "description": "For on_mail: only mail with these words in the subject." },
        "minutes": { "type": "integer", "description": "For on_event: how many minutes before the event starts (default 15)." },
        "title": { "type": "string", "description": "For on_event: only events with these words in their title." },
        "folder": { "type": "string", "description": "For on_file: the folder to watch, a full path like C:\\Users\\me\\Downloads." },
        "pattern": { "type": "string", "description": "For on_file: only files with this in their name, e.g. \".pdf\" or \"invoice\"." },
        "time": { "type": "string", "description": "The time of day, e.g. \"08:00\" or \"6:30 pm\" (for once, daily, weekdays and weekly)." },
        "times": { "type": "array", "items": { "type": "string" }, "description": "For days and dates: every time of day it runs, e.g. [\"08:00\", \"18:00\"]." },
        "date": { "type": "string", "description": "For once: the day, e.g. \"2026-10-20\" (today if left out)." },
        "dates": { "type": "array", "items": { "type": "string" }, "description": "For dates: the days it runs, e.g. [\"2026-10-24\", \"2026-10-31\"]." },
        "weekday": { "type": "string", "description": "For weekly: the day, e.g. \"monday\"." },
        "weekdays": { "type": "array", "items": { "type": "string" }, "description": "For days: the days of the week it runs, e.g. [\"monday\", \"wednesday\", \"friday\"]." },
        "minute": { "type": "integer", "description": "For hourly: the minute past each hour (0-59)." },
        "actions": { "type": "array", "items": { "type": "string", "enum": ACTIONS.iter().map(|(n, _)| *n).collect::<Vec<_>>() },
            "description": "What it may do by itself when it runs, beyond looking things up, e.g. [\"message_person\"] to send a People message, [\"email_send\"], [\"calendar_create_event\"], [\"create_music\"]. The user approves these now and isn't asked again. Leave it out for tasks that only look." } } })
}

/// What a task may be approved to do by itself, and how the approval card says it.
pub const ACTIONS: [(&str, &str); 14] = [
    ("message_person", "send a People message"),
    ("ask_person", "ask someone's assistant"),
    ("email_send", "send an email"),
    ("calendar_create_event", "add a calendar event"),
    ("create_task", "add a task"),
    ("complete_task", "mark a task done"),
    ("create_note", "write a note"),
    ("create_music", "make a song"),
    ("create_image", "make a picture"),
    ("create_video", "make a video"),
    ("message_chat", "ask another chat"),
    ("write_file", "write files in its folders"),
    ("edit_file", "change files in its folders"),
    ("run_command", "run commands"),
];

/// The actions asked for, each checked.
fn actions_of(args: &Value) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for a in list(args, "actions") {
        if !ACTIONS.iter().any(|(n, _)| *n == a) {
            return Err(format!("{a:?} isn't an action a task can take; it can be one of {}.", ACTIONS.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")));
        }
        if !out.contains(&a) {
            out.push(a);
        }
    }
    Ok(out)
}

/// "send a People message and add a calendar event".
fn said(actions: &[String]) -> String {
    let words: Vec<&str> = actions.iter().filter_map(|a| ACTIONS.iter().find(|(n, _)| n == a).map(|(_, w)| *w)).collect();
    match words.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        Some((last, _)) => last.to_string(),
        None => String::new(),
    }
}

/// "8", "8am", "8:30 pm", "20:15" as "HH:MM".
fn time_of(raw: &str) -> Result<String, String> {
    let re = regex::Regex::new(r"(?i)^\s*(\d{1,2})(?::(\d{2}))?\s*([ap])\.?\s*m?\.?\s*$|^\s*(\d{1,2})(?::(\d{2}))?\s*$").unwrap();
    let c = re.captures(raw).ok_or_else(|| format!("{raw:?} isn't a time; write it like 08:00 or 6:30 pm."))?;
    let (h, m, ap) = match c.get(1) {
        Some(h) => (h.as_str(), c.get(2).map_or("0", |m| m.as_str()), c.get(3).map(|a| a.as_str().to_ascii_lowercase())),
        None => (c.get(4).map_or("0", |h| h.as_str()), c.get(5).map_or("0", |m| m.as_str()), None),
    };
    let mut h: u32 = h.parse().map_err(|_| "bad hour".to_string())?;
    let m: u32 = m.parse().map_err(|_| "bad minute".to_string())?;
    match ap.as_deref() {
        Some("p") if h < 12 => h += 12,
        Some("a") if h == 12 => h = 0,
        _ => {}
    }
    if h > 23 || m > 59 {
        return Err(format!("{raw:?} isn't a time of day."));
    }
    Ok(format!("{h:02}:{m:02}"))
}

fn weekday_of(raw: &str) -> Result<u32, String> {
    const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
    let low = raw.trim().to_ascii_lowercase();
    DAYS.iter().position(|d| low.starts_with(d)).map(|p| p as u32).ok_or_else(|| format!("{raw:?} isn't a day of the week."))
}

pub fn cadence_of(args: &Value) -> Result<Cadence, String> {
    let text = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    let time = || text("time").map(time_of).unwrap_or_else(|| Err("Say what time (time), e.g. 08:00.".into()));
    Ok(match text("repeat").unwrap_or("daily").to_ascii_lowercase().as_str() {
        "once" => {
            let t = NaiveTime::parse_from_str(&time()?, "%H:%M").map_err(|e| e.to_string())?;
            let day = match text("date") {
                Some(d) => NaiveDate::parse_from_str(d, "%Y-%m-%d").map_err(|_| format!("{d:?} isn't a date; write it like 2026-10-20."))?,
                None => Local::now().date_naive(),
            };
            let at = Local.from_local_datetime(&day.and_time(t)).earliest().ok_or("That time doesn't exist (a clock change).")?;
            Cadence::Once { at: at.timestamp_millis() }
        }
        "hourly" => Cadence::Hourly { minute: args.get("minute").and_then(Value::as_u64).unwrap_or(0).min(59) as u32 },
        "daily" => Cadence::Daily { time: time()? },
        "weekdays" => Cadence::Weekdays { time: time()? },
        "weekly" => Cadence::Weekly { weekday: weekday_of(text("weekday").ok_or("Say which day (weekday), e.g. monday.")?)?, time: time()? },
        "days" => Cadence::Days { weekdays: list(args, "weekdays").iter().map(|d| weekday_of(d)).collect::<Result<_, _>>()?, times: times(args)? },
        "dates" => {
            let dates = list(args, "dates");
            for d in &dates {
                NaiveDate::parse_from_str(d, "%Y-%m-%d").map_err(|_| format!("{d:?} isn't a date; write it like 2026-10-20."))?;
            }
            Cadence::Dates { dates, times: times(args)? }
        }
        "on_mail" => Cadence::OnMail { from: text("from").unwrap_or_default().into(), subject: text("subject").unwrap_or_default().into() },
        "on_event" => Cadence::OnEvent { minutes: args.get("minutes").and_then(Value::as_u64).unwrap_or(15).min(1440) as u32, title: text("title").unwrap_or_default().into() },
        "on_file" => Cadence::OnFile { folder: text("folder").ok_or("Say which folder to watch (folder), as a full path.")?.into(), name: text("pattern").unwrap_or_default().into() },
        "on_start" => Cadence::OnStart,
        other => return Err(format!("repeat {other:?} isn't one of once, hourly, daily, weekdays, weekly, days, dates, on_mail, on_event, on_file, on_start.")),
    })
}

/// A list argument, or one item given as text ("monday, friday" too).
fn list(args: &Value, key: &str) -> Vec<String> {
    match args.get(key) {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        Some(Value::String(s)) => s.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect(),
        _ => Vec::new(),
    }
}

/// The times of day, as "HH:MM": `times`, else the single `time`.
fn times(args: &Value) -> Result<Vec<String>, String> {
    let mut raw = list(args, "times");
    if raw.is_empty() {
        raw = list(args, "time");
    }
    if raw.is_empty() {
        return Err("Say what times of day (times), e.g. [\"08:00\", \"18:00\"].".into());
    }
    raw.iter().map(|t| time_of(t)).collect()
}

fn name_and_prompt(args: &Value) -> Result<(String, String), String> {
    let text = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let prompt = text("prompt").ok_or("Say what to do each time (prompt).")?;
    let name = text("name").unwrap_or_else(|| prompt.chars().take(40).collect());
    Ok((name.chars().take(60).collect(), prompt.chars().take(2000).collect()))
}

pub fn preview(args: &Value) -> Result<Preview, String> {
    let (name, prompt) = name_and_prompt(args)?;
    let cadence = cadence_of(args)?;
    schedule::check_trigger(&cadence)?;
    let actions = actions_of(args)?;
    let once = matches!(cadence, Cadence::Once { .. });
    let note = if actions.is_empty() {
        "Runs by itself while SulcusAI is open, with tools that only look (it changes nothing). Each run is saved as a chat. Change or stop it under Scheduled.".to_string()
    } else {
        format!(
            "When it runs, it may {} without asking you again, and nothing else that changes or sends. It needs SulcusAI open then. {} Each run is saved as a chat.",
            said(&actions),
            if once { "It's in Tasks until it's done; cancel it there." } else { "Change or stop it under Scheduled." }
        )
    };
    Ok(Preview { title: format!("Schedule “{name}”: {}", schedule::describe(&cadence)), kind: "text", detail: Some(prompt), note: Some(note) })
}

pub fn run(args: &Value, ctx: &Ctx<'_>) -> Outcome {
    let (name, prompt) = match name_and_prompt(args) {
        Ok(p) => p,
        Err(e) => return Outcome::error("Schedule a task", e),
    };
    let cadence = match cadence_of(args) {
        Ok(c) => c,
        Err(e) => return Outcome::error(format!("Schedule “{name}”"), e),
    };
    let actions = match actions_of(args) {
        Ok(a) => a,
        Err(e) => return Outcome::error(format!("Schedule “{name}”"), e),
    };
    let next = match schedule::next_run(&cadence, Local::now()) {
        Ok(Some(t)) => Some(t),
        Ok(None) if schedule::is_trigger(&cadence) => None,
        Ok(None) => return Outcome::error(format!("Schedule “{name}”"), "That time has already passed. Pick a later one."),
        Err(e) => return Outcome::error(format!("Schedule “{name}”"), e),
    };
    if let Err(e) = schedule::check_trigger(&cadence) {
        return Outcome::error(format!("Schedule “{name}”"), e);
    }
    let conn = ctx.memory.db.lock().unwrap();
    let chat = crate::db::chat(&conn, ctx.memory.cipher, ctx.memory.chat_id);
    let s = Schedule {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.clone(),
        prompt,
        cadence: cadence.clone(),
        model_id: chat.as_ref().and_then(|c| c.model_id.clone()),
        project_id: chat.as_ref().and_then(|c| c.project_id.clone()),
        allow_changes: false,
        actions: actions.clone(),
        enabled: true,
        last_run: None,
        next_run: next.map(|t| t.timestamp_millis()),
        last_chat: None,
        armed: schedule::is_trigger(&cadence).then(crate::db::now_ms),
        handled: Vec::new(),
    };
    if let Err(e) = schedule::save(&conn, ctx.memory.cipher, &s) {
        return Outcome::error(format!("Couldn't schedule “{name}”"), e);
    }
    crate::db::log_action(&conn, "agent", "Saved a scheduled task (the assistant asked)");
    let when = schedule::describe(&cadence);
    let first = match next {
        Some(t) => format!(", first on {}", t.format("%A, %B %-d at %-I:%M %p")),
        None => String::new(),
    };
    let place = if matches!(cadence, Cadence::Once { .. }) { "Tasks (it's done once it runs)" } else { "Scheduled" };
    let may = if actions.is_empty() { String::new() } else { format!(" When it runs it will {} by itself, as approved.", said(&actions)) };
    Outcome::ok(
        format!("Scheduled “{name}”: {when}{first}.{may} It runs while SulcusAI is open, and each run is saved as a chat; the user can see, change or cancel it under {place}. Tell them that in a sentence."),
        format!("Scheduled “{name}”: {when}"),
        "text",
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_and_days_people_write() {
        assert_eq!(time_of("8").unwrap(), "08:00");
        assert_eq!(time_of("8am").unwrap(), "08:00");
        assert_eq!(time_of("6:30 pm").unwrap(), "18:30");
        assert_eq!(time_of("12 am").unwrap(), "00:00");
        assert_eq!(time_of("20:15").unwrap(), "20:15");
        assert!(time_of("25:00").is_err() && time_of("noonish").is_err());
        assert_eq!(weekday_of("Monday").unwrap(), 0);
        assert_eq!(weekday_of("sun").unwrap(), 6);
        assert_eq!(cadence_of(&json!({ "repeat": "weekly", "weekday": "friday", "time": "5pm" })).unwrap(), Cadence::Weekly { weekday: 4, time: "17:00".into() });
        assert_eq!(cadence_of(&json!({ "repeat": "hourly", "minute": 15 })).unwrap(), Cadence::Hourly { minute: 15 });
        assert!(cadence_of(&json!({ "repeat": "daily" })).is_err(), "a time is needed");
        assert_eq!(cadence_of(&json!({ "repeat": "on_mail", "from": "Dana" })).unwrap(), Cadence::OnMail { from: "Dana".into(), subject: String::new() });
        assert_eq!(cadence_of(&json!({ "repeat": "on_event" })).unwrap(), Cadence::OnEvent { minutes: 15, title: String::new() });
        assert!(cadence_of(&json!({ "repeat": "on_file" })).is_err(), "a folder is needed");
        assert!(preview(&json!({ "name": "Watch", "prompt": "Sum it up", "repeat": "on_file", "folder": r"C:\no\such\folder" })).is_err());
        let p = preview(&json!({ "name": "Morning news", "prompt": "Top news", "repeat": "weekdays", "time": "08:00" })).unwrap();
        assert!(p.title.contains("Morning news"), "{}", p.title);
    }
}
