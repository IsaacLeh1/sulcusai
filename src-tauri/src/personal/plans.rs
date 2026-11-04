// SPDX-License-Identifier: AGPL-3.0-only
//! Plans and people: deliveries, trips, leave on time, the family organizer,
//! birthdays, and job applications. Pure functions over records so the
//! window and the assistant share one answer.

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};

use super::Record;

const DAY: i64 = 86_400_000;

/// Packages not yet delivered, soonest first.
pub fn open_deliveries(packages: &[Record]) -> Vec<&Record> {
    let mut v: Vec<&Record> = packages.iter().filter(|r| r.data["status"] != "delivered").collect();
    v.sort_by_key(|r| r.at);
    v
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Trip {
    pub start: i64,
    pub end: i64,
    pub bookings: Vec<String>,
}

/// Bookings no more than two days apart belong to one trip.
pub fn trips(bookings: &[Record]) -> Vec<Trip> {
    let mut v: Vec<&Record> = bookings.iter().collect();
    v.sort_by_key(|r| r.at);
    let mut out: Vec<Trip> = Vec::new();
    for r in v {
        match out.last_mut() {
            Some(t) if r.at - t.end <= 2 * DAY => {
                t.end = r.at;
                t.bookings.push(r.id.clone());
            }
            _ => out.push(Trip { start: r.at, end: r.at, bookings: vec![r.id.clone()] }),
        }
    }
    out
}

/// When to leave: the start, minus travel, minus a buffer that grows with
/// longer trips (traffic varies more on long drives).
pub fn leave_at(start_ms: i64, travel_min: i64) -> i64 {
    let buffer = (travel_min / 5).clamp(5, 30);
    start_ms - (travel_min + buffer) * 60_000
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub struct Chore {
    pub chore: String,
    pub who: String,
}

/// Chores rotate one place each week so nobody keeps the same one.
pub fn rota(chores: &[String], people: &[String], week: u32) -> Vec<Chore> {
    if people.is_empty() {
        return Vec::new();
    }
    chores
        .iter()
        .enumerate()
        .map(|(i, c)| Chore { chore: c.clone(), who: people[(i + week as usize) % people.len()].clone() })
        .collect()
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Upcoming {
    pub name: String,
    #[serde(serialize_with = "super::date_text")]
    pub date: NaiveDate,
    pub days: i64,
    /// The age they turn, when the birth year is known.
    pub turns: Option<i32>,
}

/// Birthdays in the next `within` days. A 29 February birthday falls on
/// 28 February in other years.
pub fn upcoming_birthdays(people: &[(String, NaiveDate, bool)], today: NaiveDate, within: i64) -> Vec<Upcoming> {
    let mut v: Vec<Upcoming> = people
        .iter()
        .filter_map(|(name, born, year_known)| {
            let on = |y: i32| NaiveDate::from_ymd_opt(y, born.month(), born.day()).or_else(|| NaiveDate::from_ymd_opt(y, 2, 28));
            let mut next = on(today.year())?;
            if next < today {
                next = on(today.year() + 1)?;
            }
            let days = (next - today).num_days();
            (days <= within).then(|| Upcoming { name: name.clone(), date: next, days, turns: year_known.then(|| next.year() - born.year()) })
        })
        .collect();
    v.sort_by_key(|u| u.days);
    v
}

/// Applications still waiting with no news for a week: time to follow up.
pub fn follow_ups(applications: &[Record], now_ms: i64) -> Vec<&Record> {
    applications.iter().filter(|r| r.data["stage"] == "applied" && now_ms - r.updated_at >= 7 * DAY).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rec(id: &str, at: i64, data: serde_json::Value) -> Record {
        Record { id: id.into(), kind: "x".into(), at, data, updated_at: at }
    }

    #[test]
    fn deliveries_and_trips() {
        let p = [rec("a", 3, json!({"status": "delivered"})), rec("b", 2, json!({"status": "on_the_way"}))];
        assert_eq!(open_deliveries(&p).iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["b"]);
        let b = [rec("f", 0, json!({})), rec("h", DAY, json!({})), rec("g", 10 * DAY, json!({}))];
        let t = trips(&b);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].bookings, ["f", "h"]);
    }

    #[test]
    fn leave_on_time_buffer() {
        assert_eq!(leave_at(60 * 60_000, 10), (60 - 15) * 60_000);
        assert_eq!(leave_at(0, 300), -(300 + 30) * 60_000);
    }

    #[test]
    fn chores_rotate() {
        let c = vec!["dishes".to_string(), "trash".to_string()];
        let p = vec!["Ana".to_string(), "Bo".to_string()];
        assert_eq!(rota(&c, &p, 0)[0].who, "Ana");
        assert_eq!(rota(&c, &p, 1)[0].who, "Bo");
        assert!(rota(&c, &[], 0).is_empty());
    }

    #[test]
    fn birthdays_soon_with_age_and_leap_day() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 4).unwrap();
        let people = vec![
            ("Ana".to_string(), NaiveDate::from_ymd_opt(1990, 11, 10).unwrap(), true),
            ("Bo".to_string(), NaiveDate::from_ymd_opt(2000, 2, 29).unwrap(), false),
            ("Cy".to_string(), NaiveDate::from_ymd_opt(1980, 11, 1).unwrap(), true),
        ];
        let u = upcoming_birthdays(&people, today, 120);
        assert_eq!(u[0], Upcoming { name: "Ana".into(), date: NaiveDate::from_ymd_opt(2026, 11, 10).unwrap(), days: 6, turns: Some(36) });
        assert_eq!(u[1].date, NaiveDate::from_ymd_opt(2027, 2, 28).unwrap());
        assert_eq!(u[1].turns, None);
        assert_eq!(u.len(), 2);
    }

    #[test]
    fn job_follow_ups_after_a_week() {
        let a = [rec("a", 0, json!({"stage": "applied"})), rec("b", 0, json!({"stage": "interview"}))];
        assert_eq!(follow_ups(&a, 7 * DAY).len(), 1);
        assert!(follow_ups(&a, DAY).is_empty());
    }
}
