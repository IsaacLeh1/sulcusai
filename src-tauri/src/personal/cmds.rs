// SPDX-License-Identifier: AGPL-3.0-only
//! Commands that read across records: the inbox pass and the overview the
//! Personal page shows, plus the small calculators it offers.

use std::collections::HashSet;

use chrono::{Local, NaiveDate, TimeZone};
use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;

use super::{home, list, mail_records, mind, plans, require, save, Record};
use crate::crypto::Cipher;
use crate::AppStateRef;

fn ms_norm(t: i64) -> i64 {
    // Mail dates from older syncs are seconds.
    if t < 100_000_000_000 {
        t * 1000
    } else {
        t
    }
}

fn day_of(ms: i64) -> Option<NaiveDate> {
    Local.timestamp_millis_opt(ms).single().map(|d| d.date_naive())
}

fn date_field(v: &Value, k: &str) -> Option<NaiveDate> {
    v[k].as_str().and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
}

const MAIL_KINDS: &[&str] = &["package", "booking", "event", "newsletter", "birthday", "application"];

/// Reads the newest mail once: a message already turned into a record is
/// skipped by its id, so running it again adds only new ones.
pub fn scan_mail(conn: &Connection, c: &Cipher, limit: usize) -> Result<usize, String> {
    let seen: HashSet<String> = MAIL_KINDS
        .iter()
        .flat_map(|k| list(conn, c, k))
        .filter_map(|r| r.data["mail_id"].as_str().map(str::to_string))
        .collect();
    let mut added = 0;
    for m in crate::mail::list(conn, c, "", limit) {
        if seen.contains(&m.id) {
            continue;
        }
        let from = if m.data.from_name.is_empty() { m.data.from.clone() } else { format!("{} <{}>", m.data.from_name, m.data.from) };
        let body = if m.data.body.is_empty() { &m.data.snippet } else { &m.data.body };
        if let Some(mut f) = mail_records::read(&m.data.subject, &from, body, ms_norm(m.date)) {
            f.data["mail_id"] = Value::String(m.id.clone());
            save(conn, c, None, f.kind, f.at, &f.data)?;
            added += 1;
        }
    }
    Ok(added)
}

#[derive(Debug, Serialize, Default)]
pub struct Overview {
    pub deliveries: Vec<Record>,
    pub trips: Vec<plans::Trip>,
    pub birthdays: Vec<plans::Upcoming>,
    pub follow_ups: Vec<Record>,
    pub today: home::DayTotals,
    pub care_due: Vec<String>,
    pub car_due: Vec<String>,
    pub journal_streak: u32,
    pub journal_prompt: String,
    pub meditation_min_week: f64,
}

pub fn overview(conn: &Connection, c: &Cipher, now_ms: i64) -> Overview {
    let today = day_of(now_ms).unwrap_or_default();
    let packages = list(conn, c, "package");
    let bookings: Vec<Record> = list(conn, c, "booking").into_iter().filter(|r| r.at >= now_ms - 86_400_000).collect();
    // Birthdays from contacts (born = YYYY-MM-DD, year_known) and from mail
    // birthday records (date only, year unknown).
    let mut people: Vec<(String, NaiveDate, bool)> = list(conn, c, "contact")
        .iter()
        .filter_map(|r| Some((r.data["name"].as_str()?.to_string(), date_field(&r.data, "born")?, r.data["year_known"].as_bool().unwrap_or(true))))
        .collect();
    people.extend(list(conn, c, "birthday").iter().filter_map(|r| {
        Some((r.data["name"].as_str().or(r.data["subject"].as_str())?.to_string(), date_field(&r.data, "date")?, false))
    }));
    let day_entries: Vec<Value> = ["food", "workout"]
        .iter()
        .flat_map(|k| list(conn, c, k))
        .filter(|r| day_of(r.at) == Some(today))
        .map(|r| r.data)
        .collect();
    let care: Vec<(String, i64, i64)> = ["plant", "pet"]
        .iter()
        .flat_map(|k| list(conn, c, k))
        .filter_map(|r| Some((r.data["name"].as_str()?.to_string(), r.data["last_ms"].as_i64().unwrap_or(r.at), r.data["every_days"].as_i64()?)))
        .collect();
    let car_due = list(conn, c, "car")
        .iter()
        .filter(|r| {
            let n = |k: &str| r.data[k].as_f64().unwrap_or(0.0);
            home::service_due(n("last_km"), n("now_km"), n("every_km").max(1.0), r.data["last_ms"].as_i64().unwrap_or(r.at), now_ms, r.data["every_days"].as_i64().unwrap_or(365))
        })
        .filter_map(|r| r.data["name"].as_str().map(str::to_string))
        .collect();
    let journal_days: Vec<NaiveDate> = list(conn, c, "journal").iter().filter_map(|r| day_of(r.at)).collect();
    let meditation_min_week = list(conn, c, "meditation")
        .iter()
        .filter(|r| r.at > now_ms - 7 * 86_400_000)
        .map(|r| r.data["minutes"].as_f64().unwrap_or(0.0))
        .sum();
    Overview {
        deliveries: plans::open_deliveries(&packages).into_iter().cloned().collect(),
        trips: plans::trips(&bookings),
        birthdays: plans::upcoming_birthdays(&people, today, 30),
        follow_ups: plans::follow_ups(&list(conn, c, "application"), now_ms).into_iter().cloned().collect(),
        today: home::day_totals(&day_entries),
        care_due: home::due_care(&care, now_ms),
        car_due,
        journal_streak: mind::journal_streak(&journal_days, today),
        journal_prompt: mind::journal_prompt(today).into(),
        meditation_min_week,
    }
}

#[tauri::command(async)]
pub fn personal_scan_mail(state: AppStateRef, limit: Option<usize>) -> Result<usize, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    require(&conn)?;
    scan_mail(&conn, &c, limit.unwrap_or(500).min(5000))
}

#[tauri::command(async)]
pub fn personal_overview(state: AppStateRef) -> Result<Overview, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    require(&conn)?;
    Ok(overview(&conn, &c, crate::db::now_ms()))
}

#[tauri::command(async)]
pub fn personal_leave_at(start_ms: i64, travel_min: i64) -> i64 {
    plans::leave_at(start_ms, travel_min)
}

#[tauri::command(async)]
pub fn personal_rota(chores: Vec<String>, people: Vec<String>, week: u32) -> Vec<plans::Chore> {
    plans::rota(&chores, &people, week)
}

#[tauri::command(async)]
pub fn personal_scale_recipe(ingredients: Vec<home::Ingredient>, from: f64, to: f64) -> Vec<home::Ingredient> {
    home::scale(&ingredients, from, to)
}

#[tauri::command(async)]
pub fn personal_shopping_list(meals: Vec<Vec<home::Ingredient>>) -> Vec<home::Ingredient> {
    home::shopping_list(&meals)
}

#[tauri::command(async)]
pub fn personal_breathing(minutes: u32, each: u32) -> Vec<mind::Phase> {
    mind::breathing(minutes.min(120), each)
}

#[tauri::command(async)]
pub fn personal_health_trend(readings: Vec<(i64, f64)>) -> Option<f64> {
    home::weekly_change(&readings, crate::db::now_ms())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);").unwrap();
        c
    }

    #[test]
    fn overview_reads_across_records() {
        let conn = conn();
        let c = Cipher::random();
        let now = crate::db::now_ms();
        let day = 86_400_000;
        save(&conn, &c, None, "package", now + day, &json!({"status": "on_the_way"})).unwrap();
        save(&conn, &c, None, "package", now, &json!({"status": "delivered"})).unwrap();
        save(&conn, &c, None, "food", now, &json!({"calories": 400})).unwrap();
        save(&conn, &c, None, "plant", now, &json!({"name": "Fern", "every_days": 2, "last_ms": now - 3 * day})).unwrap();
        save(&conn, &c, None, "journal", now, &json!({"text": "hi"})).unwrap();
        save(&conn, &c, None, "meditation", now, &json!({"minutes": 10})).unwrap();
        let o = overview(&conn, &c, now);
        assert_eq!(o.deliveries.len(), 1);
        assert_eq!(o.today.calories, 400.0);
        assert_eq!(o.care_due, ["Fern"]);
        assert_eq!(o.journal_streak, 1);
        assert_eq!(o.meditation_min_week, 10.0);
        assert!(!o.journal_prompt.is_empty());
    }

    #[test]
    fn mail_seconds_become_ms() {
        assert_eq!(ms_norm(1_700_000_000), 1_700_000_000_000);
        assert_eq!(ms_norm(1_700_000_000_000), 1_700_000_000_000);
    }
}
