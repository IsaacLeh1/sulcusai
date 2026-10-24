// SPDX-License-Identifier: AGPL-3.0-only
//! Letting someone use this PC's models in the chats this user shares with
//! them (off unless turned on for that person): which models, how many
//! answers and tokens a day, how many dollars a month on cloud models, on
//! which days and hours, and how long one answer may run. Their message waits
//! in line like any other (one model runs at a time) and the answer goes into
//! the shared chat. Every use is logged here with its tokens, cost and run
//! time, so this user sees what their PC did for whom; the answers this
//! user's assistant gives theirs (`ask`) are logged too.
//!
//! - This user's "About you" never goes into these answers: the model gets
//!   the shared chat and the new message, nothing else, and no tools.

use std::sync::LazyLock;

use chrono::{Datelike, Local, NaiveTime, TimeZone, Timelike};
use rusqlite::{params, Connection};
use serde_json::{json, Value};

use super::link::{self, Frame};
use super::share::{self, Live};
use super::*;

/// What someone may use on this PC, and how much.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lend {
    pub on: bool,
    /// The models they may pick; empty: this user's default model.
    pub models: Vec<String>,
    pub answers_per_day: Option<u32>,
    /// Tokens a day, what the model read and wrote together.
    pub tokens_per_day: Option<u64>,
    /// US dollars a month on cloud models.
    pub usd_per_month: Option<f64>,
    /// Days of the week they may (1 is Monday, 7 Sunday); empty: every day.
    pub days: Vec<u8>,
    /// From and until ("HH:MM", this PC's time); both empty: any time.
    pub from: String,
    pub until: String,
    /// The longest one answer may take, in minutes.
    pub max_minutes: Option<u32>,
}

/// A model someone lets this user pick in their shared chats.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LentModel {
    /// Empty: their default model.
    pub id: String,
    pub name: String,
    pub cloud: bool,
}

/// On the watching side: what each person lends this user (while connected).
#[derive(Debug, Clone, Default, Serialize)]
pub struct Offered {
    pub models: Vec<LentModel>,
    /// Their limits (without the model list).
    pub limits: Lend,
}

static OFFERS: LazyLock<Mutex<HashMap<String, Offered>>> = LazyLock::new(Default::default);

const TABLE: &str = "CREATE TABLE IF NOT EXISTS people_runs (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  contact_id TEXT NOT NULL,
  at         INTEGER NOT NULL,
  kind       TEXT NOT NULL,
  chat_id    TEXT,
  model      TEXT NOT NULL,
  ms         INTEGER NOT NULL DEFAULT 0,
  tokens_in  INTEGER NOT NULL DEFAULT 0,
  tokens_out INTEGER NOT NULL DEFAULT 0,
  usd        REAL NOT NULL DEFAULT 0,
  outcome    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS people_runs_by_contact ON people_runs(contact_id, at);";

/// One use of this PC by someone.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Run {
    pub at: i64,
    /// "chat" (an answer in a shared chat) or "question" (their assistant asked this one).
    pub kind: String,
    pub chat_id: Option<String>,
    pub chat_title: Option<String>,
    pub model: String,
    pub ms: i64,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub usd: f64,
    /// "done", "stopped", "refused: why" or "failed: why".
    pub outcome: String,
}

/// Logs a use of this PC (the people_runs table is made the first time).
pub(crate) fn log_run(conn: &Connection, contact: &str, r: &Run) {
    let _ = conn.execute_batch(TABLE);
    let _ = conn.execute(
        "INSERT INTO people_runs (contact_id, at, kind, chat_id, model, ms, tokens_in, tokens_out, usd, outcome) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![contact, r.at, r.kind, r.chat_id, r.model, r.ms, r.tokens_in, r.tokens_out, r.usd, r.outcome.chars().take(300).collect::<String>()],
    );
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Totals {
    pub answers: i64,
    pub tokens: i64,
    pub usd: f64,
    pub ms: i64,
    /// Asked and not answered here (a limit, the hours, an error).
    pub refused: i64,
}

fn totals(conn: &Connection, contact: &str, since: i64) -> Totals {
    let _ = conn.execute_batch(TABLE);
    conn.query_row(
        "SELECT COUNT(*) FILTER (WHERE outcome IN ('done', 'stopped')), IFNULL(SUM(tokens_in + tokens_out), 0), IFNULL(SUM(usd), 0), IFNULL(SUM(ms), 0),
                COUNT(*) FILTER (WHERE outcome NOT IN ('done', 'stopped'))
         FROM people_runs WHERE contact_id = ?1 AND at >= ?2",
        params![contact, since],
        |r| Ok(Totals { answers: r.get(0)?, tokens: r.get(1)?, usd: r.get(2)?, ms: r.get(3)?, refused: r.get(4)? }),
    )
    .unwrap_or_default()
}

fn start_of_day(now: chrono::DateTime<Local>) -> i64 {
    Local.from_local_datetime(&now.date_naive().and_hms_opt(0, 0, 0).unwrap()).earliest().map_or(0, |d| d.timestamp_millis())
}

fn start_of_month(now: chrono::DateTime<Local>) -> i64 {
    let first = now.date_naive().with_day(1).unwrap_or(now.date_naive());
    Local.from_local_datetime(&first.and_hms_opt(0, 0, 0).unwrap()).earliest().map_or(0, |d| d.timestamp_millis())
}

fn time_of(s: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(s.trim(), "%H:%M").ok()
}

/// Why they can't use it right now (the day or the hours), if they can't.
pub fn outside_hours(l: &Lend, now: chrono::DateTime<Local>) -> Option<String> {
    let day = now.weekday().number_from_monday() as u8;
    if !l.days.is_empty() && !l.days.contains(&day) {
        return Some("it isn't one of the days they're allowed".into());
    }
    let (Some(from), Some(until)) = (time_of(&l.from), time_of(&l.until)) else { return None };
    let t = NaiveTime::from_hms_opt(now.hour(), now.minute(), 0).unwrap_or_default();
    // A window across midnight (22:00 to 06:00) wraps around.
    let inside = if from <= until { t >= from && t < until } else { t >= from || t < until };
    (!inside).then(|| format!("it's only allowed from {} to {}", l.from.trim(), l.until.trim()))
}

/// Why a limit stops them now, if one does; else the tokens left today.
fn over_limits(conn: &Connection, contact: &str, l: &Lend, now: chrono::DateTime<Local>) -> Result<Option<u64>, String> {
    let today = totals(conn, contact, start_of_day(now));
    if let Some(n) = l.answers_per_day {
        if today.answers >= n as i64 {
            return Err(format!("today's limit of {n} answers is used up"));
        }
    }
    let mut left = None;
    if let Some(n) = l.tokens_per_day {
        let used = today.tokens.max(0) as u64;
        if used >= n {
            return Err(format!("today's limit of {n} tokens is used up"));
        }
        left = Some(n - used);
    }
    Ok(left)
}

fn over_budget(conn: &Connection, contact: &str, l: &Lend, now: chrono::DateTime<Local>) -> Option<String> {
    let b = l.usd_per_month?;
    let spent = totals(conn, contact, start_of_month(now)).usd;
    (spent >= b).then(|| format!("this month's ${b:.2} for cloud models is used up"))
}

/// A model's name, for the log (None: the default model).
pub(crate) fn model_name(state: &AppState, id: Option<&str>) -> String {
    let id = id.map(str::to_string).or_else(|| db::settings(&state.db.lock().unwrap()).default_model).unwrap_or_default();
    if crate::cloud::is_cloud(&id) {
        return crate::cloud::choices(&state.db.lock().unwrap()).into_iter().find(|c| c.id == id).map_or(id, |c| c.name);
    }
    state.model_spec(&id).map_or(id, |s| s.name)
}

/// The models a person may pick, with their names.
fn offered(state: &AppState, l: &Lend) -> Vec<LentModel> {
    if !l.on {
        return Vec::new();
    }
    let conn = state.db.lock().unwrap();
    let name = |id: &str| -> Option<(String, bool)> {
        if crate::cloud::is_cloud(id) {
            return crate::cloud::choices(&conn).into_iter().find(|c| c.id == id).map(|c| (format!("{} ({})", c.name, c.provider), true));
        }
        db::installed_model(&conn, id)?;
        Some((state.model_spec(id).map_or_else(|| id.to_string(), |s| s.name), false))
    };
    if l.models.is_empty() {
        let default = db::settings(&conn).default_model.unwrap_or_default();
        let (n, cloud) = name(&default).unwrap_or_else(|| ("their default model".into(), false));
        return vec![LentModel { id: String::new(), name: n, cloud }];
    }
    l.models.iter().filter_map(|id| name(id).map(|(n, cloud)| LentModel { id: id.clone(), name: n, cloud })).collect()
}

/// Tells someone what this user lends them (on connecting, and on a change).
pub(super) fn connected(state: &AppState, p: &Contact) {
    let limits = Lend { models: Vec::new(), ..p.lend.clone() };
    link::send(&p.id, Frame::Lends { models: offered(state, &p.lend), limits });
}

pub(super) fn disconnected(contact: &str) {
    OFFERS.lock().unwrap().remove(contact);
}

/// The watching side: what they lend this user now.
pub(super) fn lends(state: &AppState, contact: &str, models: Vec<LentModel>, limits: Lend) {
    let models: Vec<LentModel> = models.into_iter().take(40).map(|m| LentModel { id: m.id.chars().take(200).collect(), name: m.name.chars().take(120).collect(), cloud: m.cloud }).collect();
    OFFERS.lock().unwrap().insert(contact.to_string(), Offered { models, limits });
    emit(state, "people:lends", json!({ "contact_id": contact }));
}

/// The watching side: one of their models was picked for a message here,
/// but their PC didn't answer it.
pub(super) fn refused(state: &AppState, contact: &str, chat_id: &str, why: &str) {
    let why: String = why.chars().take(400).collect();
    emit(state, "people:lend_refused", json!({ "contact_id": contact, "chat_id": chat_id, "why": why }));
}

/// Live news of the answer, to everyone watching the chat.
fn tell(chat_id: &str, kind: &str, text: &str) {
    share::tell_watchers(chat_id, Live { kind: kind.into(), text: text.into() });
}

/// Someone wrote in a chat this user shares with them and picked one of the
/// models lent to them: it's answered here, if their limits allow.
pub(super) fn borrow(state: &Arc<AppState>, p: &Contact, chat_id: &str, text: &str, model: &str) {
    let (state, p, chat_id, text, model) = (state.clone(), p.clone(), chat_id.to_string(), text.to_string(), model.to_string());
    tauri::async_runtime::spawn(async move {
        let started = db::now_ms();
        let outcome = answer(&state, &p, &chat_id, &text, &model).await;
        let mut run = match outcome {
            Ok(run) => run,
            Err((why, model)) => {
                let told = format!("{}'s PC didn't answer: {why}.", my_name(&state));
                link::send(&p.id, Frame::Refused { chat_id: chat_id.clone(), why: told });
                tell(&chat_id, "done", "");
                Run { kind: "chat".into(), model, outcome: format!("refused: {why}"), ..Default::default() }
            }
        };
        run.at = started;
        run.chat_id = Some(chat_id.clone());
        log_run(&state.db.lock().unwrap(), &p.id, &run);
        emit(&state, "people:runs", json!({ "contact_id": p.id }));
    });
}

/// Answers a lent message: the checks, a place in line, the model's answer
/// into the shared chat. Err: why it wasn't answered (for the log, and for
/// them after "Ana's PC didn't answer: "), and the model asked for.
async fn answer(state: &Arc<AppState>, p: &Contact, chat_id: &str, text: &str, model: &str) -> Result<Run, (String, String)> {
    let me = my_name(state);
    let l = contact(state, &p.id).map(|p| p.lend).unwrap_or_default();
    let label = offered(state, &l).into_iter().find(|m| m.id == model).map(|m| m.name);
    let refuse = |why: String| (why, label.clone().unwrap_or_else(|| model.to_string()));
    if !l.on {
        return Err(refuse("they don't let you use their models (anymore)".into()));
    }
    let Some(label) = label.clone() else {
        return Err(refuse("that model isn't one they let you use".into()));
    };
    let now = Local::now();
    if let Some(why) = outside_hours(&l, now) {
        return Err(refuse(why));
    }
    let left = {
        let conn = state.db.lock().unwrap();
        over_limits(&conn, &p.id, &l, now).map_err(&refuse)?
    };
    let id = if model.is_empty() { None } else { Some(model.to_string()) };
    let cloud = id.as_deref().is_some_and(crate::cloud::is_cloud) || (id.is_none() && db::settings(&state.db.lock().unwrap()).default_model.as_deref().is_some_and(crate::cloud::is_cloud));
    if cloud {
        if let Some(why) = over_budget(&state.db.lock().unwrap(), &p.id, &l, now) {
            return Err(refuse(why));
        }
    }
    let key = format!("lent:{}:{chat_id}", p.id);
    let Some((cancel, _guard)) = crate::claim(&state.generations, &key) else {
        return Err(refuse("your last message here is still being answered".into()));
    };
    // The longest one answer may take.
    if let Some(m) = l.max_minutes.filter(|m| *m > 0) {
        let c = cancel.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(m as u64 * 60)).await;
            c.store(true, std::sync::atomic::Ordering::Relaxed);
        });
    }
    tell(chat_id, "loading", "");
    let chat = chat_id.to_string();
    #[cfg(test)]
    let test = super::ask::TEST_ENDPOINT.lock().unwrap().clone();
    #[cfg(not(test))]
    let test: Option<crate::engine::Endpoint> = None;
    let (mut ep, model_name) = match test {
        Some(ep) => (ep, "Test model".to_string()),
        None => crate::llm_endpoint(state, id, false, move |w| match w {
            crate::engine::Wait::Line(place, _) => tell(&chat, "queued", &place.to_string()),
            crate::engine::Wait::Loading => tell(&chat, "loading", ""),
        })
        .await
        .map(|(ep, spec, _)| (ep, spec.name))
        .map_err(|e| refuse(e.trim_end_matches('.').to_string()))?,
    };
    if let Some(t) = &ep.cloud {
        t.for_chat(chat_id);
    }
    // Not past today's tokens.
    if let Some(left) = left {
        let most = ep.extra.get("max_tokens").and_then(Value::as_u64).unwrap_or(u64::MAX);
        ep.extra["max_tokens"] = json!(most.min(left.max(256)));
    }
    let c = state.work_cipher().map_err(&refuse)?;
    let them = p.name.clone();
    let transcript = share::transcript(&share::messages_of(state, &c, chat_id), &me, (ep.ctx as usize).saturating_mul(2));
    let today = Local::now().format("%A, %B %-d, %Y").to_string();
    // Nothing about this user: the model knows only the chat.
    let (base, _) = crate::chat::system_prompt(&db::Profile::default(), &today);
    let system = format!(
        "{base}\n\n{}\n\nYou're answering for {them} in a chat {me} shares with them, on {me}'s PC: {me} lets {them} use it. \
         The chat so far (the newest at the end):\n\n{transcript}Answer {them}'s new message below, in the context of that chat. \
         Your answer goes into the shared chat, so {me} reads it too. You have no tools here: answer from the chat and what you know.",
        crate::chat::runs_where(&ep)
    );
    let messages = vec![json!({ "role": "system", "content": system }), json!({ "role": "user", "content": text })];
    tell(chat_id, "start", "");
    let t0 = Instant::now();
    let done = crate::chat::stream(&ep, messages, None, &cancel, |d| {
        if let crate::chat::Delta::Content(t) = d {
            tell(chat_id, "delta", &t);
        }
    })
    .await;
    let ms = t0.elapsed().as_millis() as i64;
    let done = match done {
        Ok(d) => d,
        Err(e) => {
            tell(chat_id, "done", "");
            return Ok(Run { kind: "chat".into(), model: model_name, ms, outcome: format!("failed: {e}"), ..Default::default() });
        }
    };
    let (tokens_in, tokens_out) = (done.prompt_tokens.unwrap_or(0) as i64, done.completion_tokens.unwrap_or(0) as i64);
    let usd = ep.cloud.as_ref().map_or(0.0, |t| tokens_in as f64 * t.model.price_in.unwrap_or(0.0) / 1e6 + tokens_out as f64 * t.model.price_out.unwrap_or(0.0) / 1e6);
    let answer = done.content.trim().to_string();
    if !answer.is_empty() {
        // Marked as theirs: their model choice, answered on this PC.
        let id = uuid::Uuid::new_v4().simple().to_string();
        share::posted(state, &p.id, chat_id, &id, &answer, Some(&format!("{label}, on {me}'s PC")));
    }
    tell(chat_id, "done", "");
    let outcome = if done.cancelled { "stopped" } else { "done" };
    Ok(Run { kind: "chat".into(), model: model_name, ms, tokens_in, tokens_out, usd, outcome: outcome.into(), ..Default::default() })
}

// ---------- commands ----------

#[derive(Debug, Serialize)]
pub struct RunsView {
    pub today: Totals,
    pub month: Totals,
    pub all: Totals,
    /// The latest uses, newest first.
    pub runs: Vec<Run>,
    /// Answering for them right now.
    pub busy: bool,
}

/// What someone used this PC for: totals and the latest runs.
#[tauri::command(async)]
pub fn people_runs(state: AppStateRef, contact_id: String) -> Result<RunsView, String> {
    features::require(&state, Feature::People)?;
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let _ = conn.execute_batch(TABLE);
    let now = Local::now();
    let mut stmt = conn
        .prepare("SELECT at, kind, chat_id, model, ms, tokens_in, tokens_out, usd, outcome FROM people_runs WHERE contact_id = ?1 ORDER BY at DESC LIMIT 200")
        .map_err(|e| e.to_string())?;
    let mut runs: Vec<Run> = stmt
        .query_map([&contact_id], |r| {
            Ok(Run { at: r.get(0)?, kind: r.get(1)?, chat_id: r.get(2)?, chat_title: None, model: r.get(3)?, ms: r.get(4)?, tokens_in: r.get(5)?, tokens_out: r.get(6)?, usd: r.get(7)?, outcome: r.get(8)? })
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    for r in &mut runs {
        r.chat_title = r.chat_id.as_deref().and_then(|id| db::chat(&conn, &c, id)).map(|ch| ch.title);
    }
    let busy = state.generations.lock().unwrap().keys().any(|k| k.starts_with(&format!("lent:{contact_id}:")));
    Ok(RunsView { today: totals(&conn, &contact_id, start_of_day(now)), month: totals(&conn, &contact_id, start_of_month(now)), all: totals(&conn, &contact_id, 0), runs, busy })
}

/// Sets what someone may use on this PC; they hear of it at once.
#[tauri::command(async)]
pub fn people_set_lend(state: AppStateRef, contact_id: String, lend: Lend) -> Result<(), String> {
    features::require(&state, Feature::People)?;
    let mut lend = lend;
    lend.models.retain(|m| !m.trim().is_empty());
    lend.days.retain(|d| (1..=7).contains(d));
    for t in [&mut lend.from, &mut lend.until] {
        if !t.trim().is_empty() && time_of(t).is_none() {
            return Err("Write the hours as HH:MM, like 18:00.".into());
        }
    }
    let p = update(&state, |b| {
        b.contacts.iter_mut().find(|p| p.id == contact_id).map(|p| {
            p.lend = lend.clone();
            p.clone()
        })
    })?
    .ok_or("They aren't linked anymore.")?;
    // Turned off mid-answer: it stops.
    if !lend.on {
        for (k, flag) in state.generations.lock().unwrap().iter() {
            if k.starts_with(&format!("lent:{contact_id}:")) {
                flag.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
    state.log("people", &format!("{} {} use this PC's models", if lend.on { "Let" } else { "Stopped letting" }, p.name));
    connected(&state, &p);
    changed(&state);
    Ok(())
}

/// The models someone lends this user, for their shared chats' picker.
#[tauri::command(async)]
pub fn people_lent(contact_id: String) -> Offered {
    OFFERS.lock().unwrap().get(&contact_id).cloned().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(h: u32, m: u32, weekday_from_monday: u32) -> chrono::DateTime<Local> {
        // 2026-10-19 was a Monday.
        Local.with_ymd_and_hms(2026, 10, 18 + weekday_from_monday, h, m, 0).unwrap()
    }

    #[test]
    fn hours_and_days_are_kept() {
        let evenings = Lend { on: true, from: "18:00".into(), until: "22:00".into(), days: vec![1, 2, 3, 4, 5], ..Default::default() };
        assert!(outside_hours(&evenings, at(19, 30, 1)).is_none());
        assert!(outside_hours(&evenings, at(17, 59, 1)).is_some());
        assert!(outside_hours(&evenings, at(22, 0, 1)).is_some());
        assert!(outside_hours(&evenings, at(19, 0, 6)).unwrap().contains("days"), "Saturday isn't allowed");
        let nights = Lend { from: "22:00".into(), until: "06:00".into(), ..Default::default() };
        assert!(outside_hours(&nights, at(23, 0, 2)).is_none());
        assert!(outside_hours(&nights, at(5, 0, 2)).is_none());
        assert!(outside_hours(&nights, at(12, 0, 2)).is_some());
        assert!(outside_hours(&Lend::default(), at(3, 0, 7)).is_none(), "no hours set: any time");
    }

    #[test]
    fn limits_count_what_they_used() {
        let conn = Connection::open_in_memory().unwrap();
        let now = Local::now();
        let l = Lend { on: true, answers_per_day: Some(2), tokens_per_day: Some(1000), usd_per_month: Some(1.0), ..Default::default() };
        assert_eq!(over_limits(&conn, "ben", &l, now), Ok(Some(1000)));
        let run = |tokens: i64, usd: f64, outcome: &str| Run { at: db::now_ms(), kind: "chat".into(), model: "m".into(), tokens_in: tokens, usd, outcome: outcome.into(), ..Default::default() };
        log_run(&conn, "ben", &run(300, 0.4, "done"));
        log_run(&conn, "ben", &run(0, 0.0, "refused: it isn't one of the days they're allowed"));
        assert_eq!(over_limits(&conn, "ben", &l, now), Ok(Some(700)), "a refusal isn't an answer and used no tokens");
        assert!(over_budget(&conn, "ben", &l, now).is_none());
        log_run(&conn, "ben", &run(300, 0.7, "done"));
        assert!(over_limits(&conn, "ben", &l, now).unwrap_err().contains("2 answers"));
        assert!(over_budget(&conn, "ben", &l, now).unwrap().contains("$1.00"));
        assert_eq!(over_limits(&conn, "amy", &l, now), Ok(Some(1000)), "each person has their own");
        let t = totals(&conn, "ben", 0);
        assert_eq!((t.answers, t.tokens, t.refused), (2, 600, 1));
    }
}
