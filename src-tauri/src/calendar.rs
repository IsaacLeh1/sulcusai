// SPDX-License-Identifier: AGPL-3.0-only
//! Calendar: events on this PC (they work Offline), plus CalDAV accounts
//! (iCloud, Fastmail, Nextcloud and others) copied in and written back.
//!
//! - Titles, places, notes and attendees are encrypted; times are not, so
//!   ranges can be queried.
//! - Syncing needs Local AI + Web. The server expands repeating events, so
//!   each occurrence is stored as its own event.
//! - Inviting people always needs the user's go-ahead (the server may email
//!   them).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};

use crate::crypto::Cipher;
use crate::features::Feature;
use crate::net::{self, Purpose};
use crate::{db, AppState, AppStateRef};

/// Synced window: from 30 days back to 180 days ahead.
const PAST_DAYS: i64 = 30;
const FUTURE_DAYS: i64 = 180;
const DAY_MS: i64 = 86_400_000;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct EventData {
    pub title: String,
    pub location: String,
    pub notes: String,
    pub attendees: Vec<String>,
    pub uid: String,
    /// On the server (CalDAV): the event's address and version.
    pub href: Option<String>,
    pub etag: Option<String>,
    pub recurring: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub id: String,
    pub account_id: Option<String>,
    pub calendar: Option<String>,
    pub start: i64,
    pub end: i64,
    pub all_day: bool,
    #[serde(flatten)]
    pub data: EventData,
}

const COLS: &str = "id, account_id, calendar, start, end, all_day, data";

fn from_row(c: &Cipher) -> impl Fn(&rusqlite::Row) -> rusqlite::Result<Event> + '_ {
    move |r| {
        Ok(Event {
            id: r.get(0)?,
            account_id: r.get(1)?,
            calendar: r.get(2)?,
            start: r.get(3)?,
            end: r.get(4)?,
            all_day: r.get::<_, i64>(5)? != 0,
            data: c.decrypt(&r.get::<_, String>(6)?).ok().and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default(),
        })
    }
}

/// Events overlapping [from, to).
pub fn events(conn: &Connection, c: &Cipher, from: i64, to: i64) -> Vec<Event> {
    let sql = format!("SELECT {COLS} FROM events WHERE start < ?2 AND end > ?1 ORDER BY start");
    let Ok(mut stmt) = conn.prepare(&sql) else { return Vec::new() };
    stmt.query_map(params![from, to], from_row(c)).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

pub fn event(conn: &Connection, c: &Cipher, id: &str) -> Option<Event> {
    let sql = format!("SELECT {COLS} FROM events WHERE id = ?1");
    conn.query_row(&sql, [id], from_row(c)).optional().ok().flatten()
}

fn insert(conn: &Connection, c: &Cipher, e: &Event) -> Result<(), String> {
    conn.execute(
        "INSERT INTO events (id, account_id, calendar, start, end, all_day, data, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(id) DO UPDATE SET start = excluded.start, end = excluded.end, all_day = excluded.all_day, data = excluded.data, updated_at = excluded.updated_at",
        params![e.id, e.account_id, e.calendar, e.start, e.end, e.all_day as i64, c.encrypt(&serde_json::to_string(&e.data).map_err(|x| x.to_string())?), db::now_ms()],
    )
    .map_err(|x| x.to_string())?;
    Ok(())
}

// ---------- free time ----------

/// Gaps of at least `minutes` between `from` and `to`, within working hours
/// (`day_start`..`day_end`, local time) on each day. All-day events don't
/// block time.
pub fn free_slots(busy: &[(i64, i64)], from: i64, to: i64, minutes: i64, day_start: u32, day_end: u32) -> Vec<(i64, i64)> {
    let mut busy: Vec<(i64, i64)> = busy.to_vec();
    busy.sort();
    let mut out = Vec::new();
    let Some(mut day) = Local.timestamp_millis_opt(from).earliest().map(|d| d.date_naive()) else { return out };
    let last = Local.timestamp_millis_opt(to).earliest().map(|d| d.date_naive()).unwrap_or(day);
    while day <= last {
        let at = |h: u32| Local.from_local_datetime(&day.and_time(NaiveTime::from_hms_opt(h, 0, 0).unwrap())).earliest().map(|d| d.timestamp_millis());
        if let (Some(ds), Some(de)) = (at(day_start), at(day_end)) {
            let mut cursor = ds.max(from);
            let end = de.min(to);
            for &(s, e) in &busy {
                if e <= cursor || s >= end {
                    continue;
                }
                if s - cursor >= minutes * 60_000 {
                    out.push((cursor, s));
                }
                cursor = cursor.max(e);
            }
            if end - cursor >= minutes * 60_000 {
                out.push((cursor, end));
            }
        }
        day = day.succ_opt().unwrap_or(day);
        if day == NaiveDate::MAX {
            break;
        }
    }
    out
}

// ---------- iCalendar ----------

/// Unfolds lines and splits each into (name, params, value).
fn ical_lines(text: &str) -> Vec<(String, HashMap<String, String>, String)> {
    let mut unfolded: Vec<String> = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if (line.starts_with(' ') || line.starts_with('\t')) && !unfolded.is_empty() {
            unfolded.last_mut().unwrap().push_str(&line[1..]);
        } else if !line.is_empty() {
            unfolded.push(line.to_string());
        }
    }
    unfolded
        .into_iter()
        .filter_map(|l| {
            // The value starts at the first colon outside quotes.
            let mut quoted = false;
            let split = l.char_indices().find(|(_, ch)| {
                if *ch == '"' {
                    quoted = !quoted;
                }
                *ch == ':' && !quoted
            })?;
            let (head, value) = (&l[..split.0], &l[split.0 + 1..]);
            let mut parts = head.split(';');
            let name = parts.next()?.to_ascii_uppercase();
            let params = parts.filter_map(|p| p.split_once('=')).map(|(k, v)| (k.to_ascii_uppercase(), v.trim_matches('"').to_string())).collect();
            Some((name, params, value.to_string()))
        })
        .collect()
}

fn unescape(v: &str) -> String {
    v.replace("\\n", "\n").replace("\\N", "\n").replace("\\,", ",").replace("\\;", ";").replace("\\\\", "\\")
}

fn escape(v: &str) -> String {
    v.replace('\\', "\\\\").replace('\n', "\\n").replace(',', "\\,").replace(';', "\\;")
}

/// A DTSTART/DTEND value as milliseconds, and whether it's a whole day.
fn ical_time(value: &str, params: &HashMap<String, String>) -> Option<(i64, bool)> {
    if params.get("VALUE").is_some_and(|v| v.eq_ignore_ascii_case("DATE")) || value.len() == 8 {
        let d = NaiveDate::parse_from_str(value, "%Y%m%d").ok()?;
        return Some((crate::notes::to_ms(d, None)?, true));
    }
    if let Some(utc) = value.strip_suffix('Z') {
        let t = NaiveDateTime::parse_from_str(utc, "%Y%m%dT%H%M%S").ok()?;
        return Some((Utc.from_utc_datetime(&t).timestamp_millis(), false));
    }
    let t = NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S").ok()?;
    if let Some(tz) = params.get("TZID").and_then(|z| z.parse::<chrono_tz::Tz>().ok()) {
        return Some((tz.from_local_datetime(&t).earliest()?.timestamp_millis(), false));
    }
    // Floating time, or a zone name we don't know: this PC's time.
    Some((Local.from_local_datetime(&t).earliest()?.timestamp_millis(), false))
}

/// ISO 8601 durations as used in DURATION (P1D, PT1H30M, P1W).
fn ical_duration(v: &str) -> Option<i64> {
    let v = v.trim_start_matches(['+', 'P']);
    let (mut ms, mut num, mut in_time) = (0i64, String::new(), false);
    for ch in v.chars() {
        match ch {
            'T' => in_time = true,
            '0'..='9' => num.push(ch),
            unit => {
                let n: i64 = num.parse().ok()?;
                num.clear();
                ms += n * match (unit, in_time) {
                    ('W', _) => 7 * DAY_MS,
                    ('D', _) => DAY_MS,
                    ('H', true) => 3_600_000,
                    ('M', true) => 60_000,
                    ('S', true) => 1_000,
                    _ => return None,
                };
            }
        }
    }
    Some(ms)
}

/// The events in an iCalendar text (cancelled ones are skipped).
pub fn parse_ics(text: &str) -> Vec<(i64, i64, bool, EventData)> {
    let mut out = Vec::new();
    let mut cur: Option<(Option<(i64, bool)>, Option<i64>, Option<i64>, EventData, bool)> = None;
    for (name, params, value) in ical_lines(text) {
        match name.as_str() {
            "BEGIN" if value.eq_ignore_ascii_case("VEVENT") => cur = Some((None, None, None, EventData::default(), false)),
            "END" if value.eq_ignore_ascii_case("VEVENT") => {
                if let Some((Some((start, all_day)), end, dur, data, cancelled)) = cur.take() {
                    if !cancelled {
                        let end = end.or(dur.map(|d| start + d)).unwrap_or(if all_day { start + DAY_MS } else { start + 3_600_000 });
                        out.push((start, end.max(start), all_day, data));
                    }
                }
            }
            _ => {
                let Some((start, end, dur, data, cancelled)) = cur.as_mut() else { continue };
                match name.as_str() {
                    "UID" => data.uid = value,
                    "SUMMARY" => data.title = unescape(&value),
                    "LOCATION" => data.location = unescape(&value),
                    "DESCRIPTION" => data.notes = unescape(&value),
                    "DTSTART" => *start = ical_time(&value, &params),
                    "DTEND" => *end = ical_time(&value, &params).map(|t| t.0),
                    "DURATION" => *dur = ical_duration(&value),
                    "RRULE" | "RECURRENCE-ID" => data.recurring = true,
                    "STATUS" => *cancelled = value.eq_ignore_ascii_case("CANCELLED"),
                    "ATTENDEE" => {
                        let addr = value.trim_start_matches("mailto:").trim_start_matches("MAILTO:").to_string();
                        if !addr.is_empty() {
                            data.attendees.push(addr);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    out
}

fn ics_stamp(ms: i64) -> String {
    Utc.timestamp_millis_opt(ms).single().unwrap_or_default().format("%Y%m%dT%H%M%SZ").to_string()
}

/// One event as an iCalendar file.
pub fn to_ics(e: &Event, organizer: Option<&str>) -> String {
    let mut lines = vec![
        "BEGIN:VCALENDAR".to_string(),
        "VERSION:2.0".into(),
        "PRODID:-//SulcusAI//Calendar//EN".into(),
        "BEGIN:VEVENT".into(),
        format!("UID:{}", e.data.uid),
        format!("DTSTAMP:{}", ics_stamp(db::now_ms())),
    ];
    if e.all_day {
        let day = |ms: i64| Local.timestamp_millis_opt(ms).earliest().unwrap_or_default().format("%Y%m%d").to_string();
        lines.push(format!("DTSTART;VALUE=DATE:{}", day(e.start)));
        lines.push(format!("DTEND;VALUE=DATE:{}", day(e.end)));
    } else {
        lines.push(format!("DTSTART:{}", ics_stamp(e.start)));
        lines.push(format!("DTEND:{}", ics_stamp(e.end)));
    }
    lines.push(format!("SUMMARY:{}", escape(&e.data.title)));
    if !e.data.location.is_empty() {
        lines.push(format!("LOCATION:{}", escape(&e.data.location)));
    }
    if !e.data.notes.is_empty() {
        lines.push(format!("DESCRIPTION:{}", escape(&e.data.notes)));
    }
    if !e.data.attendees.is_empty() {
        if let Some(o) = organizer {
            lines.push(format!("ORGANIZER:mailto:{o}"));
        }
        for a in &e.data.attendees {
            lines.push(format!("ATTENDEE;RSVP=TRUE;PARTSTAT=NEEDS-ACTION:mailto:{a}"));
        }
    }
    lines.push("END:VEVENT".into());
    lines.push("END:VCALENDAR".into());
    lines.join("\r\n") + "\r\n"
}

// ---------- CalDAV ----------

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CalInfo {
    pub href: String,
    pub name: String,
    pub color: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CalAccount {
    pub name: String,
    pub url: String,
    pub username: String,
    pub password: String,
    pub calendars: Vec<CalInfo>,
}

/// Starting addresses for well-known providers.
pub fn presets() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("iCloud", "https://caldav.icloud.com/", "Use your Apple ID and an app-specific password (appleid.apple.com › Sign-In and Security)."),
        ("Fastmail", "https://caldav.fastmail.com/", "Use an app password from Settings › Privacy & Security."),
        ("Nextcloud", "https://YOUR-SERVER/remote.php/dav/", "Use an app password from Settings › Security."),
        ("Other (CalDAV)", "", "The CalDAV address your provider gives you."),
    ]
}

/// One <response> from a WebDAV multistatus.
#[derive(Debug, Default, Clone)]
pub struct DavResponse {
    pub href: String,
    /// Element name → text (the first one seen).
    pub props: HashMap<String, String>,
    /// Hrefs nested in a property, by property name.
    pub hrefs: HashMap<String, String>,
    /// Child element names inside <resourcetype>.
    pub types: Vec<String>,
    pub ok: bool,
}

pub fn parse_multistatus(xml: &str) -> Vec<DavResponse> {
    use quick_xml::events::Event as X;
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut out = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut cur: Option<DavResponse> = None;
    let mut status_ok = true;
    let local = |n: &[u8]| -> String {
        let s = String::from_utf8_lossy(n).to_string();
        s.rsplit(':').next().unwrap_or(&s).to_ascii_lowercase()
    };
    loop {
        match reader.read_event() {
            Ok(X::Start(e)) => {
                let name = local(e.name().as_ref());
                if name == "response" {
                    cur = Some(DavResponse { ok: true, ..Default::default() });
                }
                if name == "propstat" {
                    status_ok = true;
                }
                if stack.last().is_some_and(|p| p == "resourcetype") {
                    if let Some(r) = cur.as_mut() {
                        r.types.push(name.clone());
                    }
                }
                stack.push(name);
            }
            Ok(X::Empty(e)) => {
                let name = local(e.name().as_ref());
                if stack.last().is_some_and(|p| p == "resourcetype") {
                    if let Some(r) = cur.as_mut() {
                        r.types.push(name);
                    }
                }
            }
            Ok(X::Text(t)) => {
                let text = t.decode().map(|s| s.to_string()).unwrap_or_default();
                let text = quick_xml::escape::unescape(&text).map(|s| s.to_string()).unwrap_or(text);
                add_text(&mut cur, &stack, &text, &mut status_ok);
            }
            Ok(X::CData(t)) => {
                let text = String::from_utf8_lossy(&t).to_string();
                add_text(&mut cur, &stack, &text, &mut status_ok);
            }
            Ok(X::GeneralRef(r)) => {
                // Entities split text events (&amp; and friends).
                let text = match r.decode().unwrap_or_default().as_ref() {
                    "amp" => "&".to_string(),
                    "lt" => "<".into(),
                    "gt" => ">".into(),
                    "quot" => "\"".into(),
                    "apos" => "'".into(),
                    other => other.strip_prefix("#x").and_then(|h| u32::from_str_radix(h, 16).ok()).or_else(|| other.strip_prefix('#').and_then(|d| d.parse().ok())).and_then(char::from_u32).map(String::from).unwrap_or_default(),
                };
                add_text(&mut cur, &stack, &text, &mut status_ok);
            }
            Ok(X::End(e)) => {
                let name = local(e.name().as_ref());
                stack.pop();
                if name == "response" {
                    if let Some(r) = cur.take() {
                        out.push(r);
                    }
                }
            }
            Ok(X::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

fn add_text(cur: &mut Option<DavResponse>, stack: &[String], text: &str, status_ok: &mut bool) {
    let Some(r) = cur.as_mut() else { return };
    let Some(el) = stack.last() else { return };
    let parent = stack.len().checked_sub(2).map(|i| stack[i].as_str()).unwrap_or("");
    match el.as_str() {
        "href" if parent == "response" => r.href.push_str(text.trim()),
        "href" => {
            r.hrefs.entry(parent.to_string()).or_default().push_str(text.trim());
        }
        "status" => {
            let ok = text.contains(" 200");
            if parent == "propstat" {
                *status_ok = ok;
            } else {
                r.ok = ok;
            }
        }
        _ if *status_ok => {
            // calendar-data arrives in pieces around entities: keep appending.
            r.props.entry(el.clone()).or_default().push_str(text);
        }
        _ => {}
    }
}

fn join(base: &str, href: &str) -> String {
    reqwest::Url::parse(base).and_then(|b| b.join(href)).map(|u| u.to_string()).unwrap_or_else(|_| href.to_string())
}

struct Dav {
    client: reqwest::Client,
    user: String,
    pass: String,
}

impl Dav {
    fn new(a: &CalAccount) -> Result<Dav, String> {
        let client = reqwest::Client::builder()
            .user_agent(concat!("SulcusAI/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Dav { client, user: a.username.clone(), pass: a.password.clone() })
    }

    async fn call(&self, method: &str, url: &str, depth: &str, body: String) -> Result<(u16, String), String> {
        let m = reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| e.to_string())?;
        let resp = self
            .client
            .request(m, url)
            .basic_auth(&self.user, Some(&self.pass))
            .header("Depth", depth)
            .header("Content-Type", "application/xml; charset=utf-8")
            .body(body)
            .send()
            .await
            .map_err(|e| format!("Couldn't reach the calendar server: {e}"))?;
        let status = resp.status().as_u16();
        if status == 401 || status == 403 {
            return Err("The calendar server didn't accept the sign-in. Check the user name and (app) password.".into());
        }
        Ok((status, resp.text().await.unwrap_or_default()))
    }

    async fn propfind(&self, url: &str, depth: &str, props: &str) -> Result<Vec<DavResponse>, String> {
        let body = format!(r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:a="http://apple.com/ns/ical/"><d:prop>{props}</d:prop></d:propfind>"#);
        let (status, text) = self.call("PROPFIND", url, depth, body).await?;
        if status != 207 {
            return Err(format!("The calendar server answered {status} at {url}."));
        }
        Ok(parse_multistatus(&text))
    }

    /// The account's calendars (collections that hold events).
    async fn discover(&self, url: &str) -> Result<Vec<CalInfo>, String> {
        let url = if url.ends_with('/') { url.to_string() } else { format!("{url}/") };
        // The principal, from the address given or the well-known one.
        let mut principal = None;
        for start in [url.clone(), join(&url, "/.well-known/caldav")] {
            if let Ok(r) = self.propfind(&start, "0", "<d:current-user-principal/>").await {
                if let Some(h) = r.iter().find_map(|x| x.hrefs.get("current-user-principal").filter(|h| !h.is_empty())) {
                    principal = Some(join(&start, h));
                    break;
                }
            }
        }
        let home = match principal {
            Some(p) => {
                let r = self.propfind(&p, "0", "<c:calendar-home-set/>").await?;
                r.iter().find_map(|x| x.hrefs.get("calendar-home-set").cloned()).map(|h| join(&p, &h)).unwrap_or(url.clone())
            }
            None => url.clone(),
        };
        let list = self
            .propfind(&home, "1", "<d:resourcetype/><d:displayname/><a:calendar-color/><c:supported-calendar-component-set/>")
            .await?;
        let cals: Vec<CalInfo> = list
            .into_iter()
            .filter(|r| r.types.iter().any(|t| t == "calendar"))
            .map(|r| CalInfo {
                name: r.props.get("displayname").map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| "Calendar".into()),
                color: r.props.get("calendar-color").map(|s| s.trim().chars().take(7).collect()).unwrap_or_default(),
                href: join(&home, &r.href),
            })
            .collect();
        if cals.is_empty() {
            return Err("No calendars were found at that address.".into());
        }
        Ok(cals)
    }

    /// Events in [from, to), with repeating events expanded by the server.
    async fn events(&self, cal: &str, from: i64, to: i64) -> Result<Vec<(String, Option<String>, String)>, String> {
        let (s, e) = (ics_stamp(from), ics_stamp(to));
        let body = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:getetag/><c:calendar-data><c:expand start="{s}" end="{e}"/></c:calendar-data></d:prop><c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VEVENT"><c:time-range start="{s}" end="{e}"/></c:comp-filter></c:comp-filter></c:filter></c:calendar-query>"#
        );
        let (status, text) = self.call("REPORT", cal, "1", body).await?;
        if status != 207 {
            return Err(format!("The calendar server answered {status} when asked for events."));
        }
        Ok(parse_multistatus(&text)
            .into_iter()
            .filter_map(|r| Some((join(cal, &r.href), r.props.get("getetag").map(|t| t.trim().to_string()), r.props.get("calendar-data")?.clone())))
            .collect())
    }

    async fn put(&self, url: &str, ics: String, etag: Option<&str>) -> Result<Option<String>, String> {
        let mut req = self.client.put(url).basic_auth(&self.user, Some(&self.pass)).header("Content-Type", "text/calendar; charset=utf-8").body(ics);
        req = match etag {
            Some(t) => req.header("If-Match", t),
            None => req.header("If-None-Match", "*"),
        };
        let resp = req.send().await.map_err(|e| format!("Couldn't reach the calendar server: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("The calendar server refused the event ({}).", resp.status()));
        }
        Ok(resp.headers().get("etag").and_then(|v| v.to_str().ok()).map(str::to_string))
    }

    async fn delete(&self, url: &str, etag: Option<&str>) -> Result<(), String> {
        let mut req = self.client.delete(url).basic_auth(&self.user, Some(&self.pass));
        if let Some(t) = etag {
            req = req.header("If-Match", t);
        }
        let resp = req.send().await.map_err(|e| format!("Couldn't reach the calendar server: {e}"))?;
        if !resp.status().is_success() && resp.status().as_u16() != 404 {
            return Err(format!("The calendar server didn't delete the event ({}).", resp.status()));
        }
        Ok(())
    }
}

pub fn web_allowed(state: &AppState) -> Result<(), String> {
    if net::allowed(state.settings().connectivity, Purpose::Web, false) {
        Ok(())
    } else {
        Err("Calendar accounts need Local AI + Web to sync. Events on this PC work Offline.".into())
    }
}

pub fn account(conn: &Connection, c: &Cipher, id: &str) -> Option<CalAccount> {
    let data: String = conn.query_row("SELECT data FROM cal_accounts WHERE id = ?1", [id], |r| r.get(0)).optional().ok().flatten()?;
    c.decrypt(&data).ok().and_then(|j| serde_json::from_str(&j).ok())
}

pub fn accounts(conn: &Connection, c: &Cipher) -> Vec<(String, CalAccount, Option<i64>)> {
    let Ok(mut stmt) = conn.prepare("SELECT id, data, synced_at FROM cal_accounts ORDER BY created_at") else { return Vec::new() };
    stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<i64>>(2)?)))
        .map(|rows| {
            rows.filter_map(Result::ok)
                .filter_map(|(id, d, at)| Some((id, serde_json::from_str(&c.decrypt(&d).ok()?).ok()?, at)))
                .collect()
        })
        .unwrap_or_default()
}

fn store_account(conn: &Connection, c: &Cipher, id: &str, a: &CalAccount) -> Result<(), String> {
    conn.execute(
        "INSERT INTO cal_accounts (id, data, created_at) VALUES (?1, ?2, ?3) ON CONFLICT(id) DO UPDATE SET data = excluded.data",
        params![id, c.encrypt(&serde_json::to_string(a).map_err(|e| e.to_string())?), db::now_ms()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Replaces the account's events in the synced window with the server's.
pub async fn sync_account(state: &AppState, c: &Cipher, id: &str) -> Result<usize, String> {
    web_allowed(state)?;
    let a = account(&state.db.lock().unwrap(), c, id).ok_or("That calendar account no longer exists.")?;
    let dav = Dav::new(&a)?;
    let now = db::now_ms();
    let (from, to) = (now - PAST_DAYS * DAY_MS, now + FUTURE_DAYS * DAY_MS);
    let mut found = Vec::new();
    for cal in &a.calendars {
        for (href, etag, ics) in dav.events(&cal.href, from, to).await? {
            for (i, (start, end, all_day, mut data)) in parse_ics(&ics).into_iter().enumerate() {
                data.href = Some(href.clone());
                data.etag = etag.clone();
                // Occurrences of a repeating event share a UID; the start tells them apart.
                let key = format!("{id}|{}|{start}|{i}", data.uid);
                let event_id = format!("{:x}", sha2::Digest::finalize(<sha2::Sha256 as sha2::Digest>::new_with_prefix(key.as_bytes())))[..32].to_string();
                found.push(Event { id: event_id, account_id: Some(id.to_string()), calendar: Some(cal.href.clone()), start, end, all_day, data });
            }
        }
    }
    let conn = state.db.lock().unwrap();
    conn.execute("DELETE FROM events WHERE account_id = ?1", [id]).map_err(|e| e.to_string())?;
    for e in &found {
        insert(&conn, c, e)?;
    }
    conn.execute("UPDATE cal_accounts SET synced_at = ?2 WHERE id = ?1", params![id, db::now_ms()]).map_err(|e| e.to_string())?;
    Ok(found.len())
}

pub async fn sync_all(state: &AppState, c: &Cipher) -> Result<usize, String> {
    let ids: Vec<String> = accounts(&state.db.lock().unwrap(), c).into_iter().map(|(id, _, _)| id).collect();
    let mut total = 0;
    for id in ids {
        total += sync_account(state, c, &id).await?;
    }
    Ok(total)
}

// ---------- creating ----------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NewEvent {
    pub title: String,
    pub start: i64,
    pub end: i64,
    pub all_day: bool,
    pub location: String,
    pub notes: String,
    pub attendees: Vec<String>,
    /// A CalDAV calendar's address; None = on this PC.
    pub calendar: Option<String>,
}

/// Saves an event on this PC, or on the account that owns the calendar.
pub async fn create(state: &AppState, c: &Cipher, n: NewEvent) -> Result<Event, String> {
    if n.title.trim().is_empty() {
        return Err("Give the event a title.".into());
    }
    let end = if n.end > n.start { n.end } else if n.all_day { n.start + DAY_MS } else { n.start + 3_600_000 };
    let uid = format!("{}@sulcusai", uuid::Uuid::new_v4());
    let mut e = Event {
        id: uuid::Uuid::new_v4().simple().to_string(),
        account_id: None,
        calendar: None,
        start: n.start,
        end,
        all_day: n.all_day,
        data: EventData { title: n.title.trim().into(), location: n.location, notes: n.notes, attendees: n.attendees, uid, ..Default::default() },
    };
    if let Some(cal) = n.calendar.filter(|c| !c.is_empty()) {
        web_allowed(state)?;
        let (id, a) = accounts(&state.db.lock().unwrap(), c)
            .into_iter()
            .find(|(_, a, _)| a.calendars.iter().any(|x| x.href == cal))
            .map(|(id, a, _)| (id, a))
            .ok_or("That calendar isn't connected anymore.")?;
        let url = format!("{}{}.ics", if cal.ends_with('/') { cal.clone() } else { format!("{cal}/") }, e.data.uid.replace('@', "-"));
        let organizer = if a.username.contains('@') { Some(a.username.as_str()) } else { None };
        let etag = Dav::new(&a)?.put(&url, to_ics(&e, organizer), None).await?;
        e.account_id = Some(id);
        e.calendar = Some(cal);
        e.data.href = Some(url);
        e.data.etag = etag;
    }
    let conn = state.db.lock().unwrap();
    insert(&conn, c, &e)?;
    let who = if e.data.attendees.is_empty() { String::new() } else { format!(" and invited {}", e.data.attendees.join(", ")) };
    db::log_action(&conn, "calendar", &format!("Added “{}” to the calendar{who}", e.data.title));
    Ok(e)
}

pub async fn delete(state: &AppState, c: &Cipher, id: &str) -> Result<(), String> {
    let e = event(&state.db.lock().unwrap(), c, id).ok_or("That event no longer exists.")?;
    if let (Some(acc), Some(href)) = (&e.account_id, &e.data.href) {
        web_allowed(state)?;
        if e.data.recurring {
            return Err("This is one of a repeating series; change it in the calendar it came from for now.".into());
        }
        let a = account(&state.db.lock().unwrap(), c, acc).ok_or("That calendar isn't connected anymore.")?;
        Dav::new(&a)?.delete(href, e.data.etag.as_deref()).await?;
    }
    let conn = state.db.lock().unwrap();
    conn.execute("DELETE FROM events WHERE id = ?1", [id]).map_err(|e| e.to_string())?;
    db::log_action(&conn, "calendar", &format!("Deleted “{}” from the calendar", e.data.title));
    Ok(())
}

// ---------- background ----------

pub fn start_sync(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<Arc<AppState>>().inner().clone();
        loop {
            tokio::time::sleep(Duration::from_secs(900)).await;
            let ready = {
                let conn = state.db.lock().unwrap();
                crate::features::is_on(&conn, Feature::Calendar) && conn.query_row("SELECT COUNT(*) FROM cal_accounts", [], |r| r.get::<_, i64>(0)).unwrap_or(0) > 0
            };
            let Ok(c) = state.cipher() else { continue };
            if !ready || web_allowed(&state).is_err() {
                continue;
            }
            if sync_all(&state, &c).await.is_ok() {
                app.emit("calendar:synced", json!({})).ok();
            }
        }
    });
}

// ---------- commands ----------

fn ensure_on(state: &AppState) -> Result<(), String> {
    crate::features::require(state, Feature::Calendar)
}

#[derive(Serialize)]
pub struct AccountView {
    id: String,
    name: String,
    url: String,
    calendars: Vec<CalInfo>,
    synced_at: Option<i64>,
}

#[tauri::command]
pub fn calendar_presets() -> Vec<serde_json::Value> {
    presets().into_iter().map(|(name, url, note)| json!({ "name": name, "url": url, "note": note })).collect()
}

#[tauri::command]
pub fn calendar_accounts(state: AppStateRef) -> Result<Vec<AccountView>, String> {
    let c = state.cipher()?;
    Ok(accounts(&state.db.lock().unwrap(), &c)
        .into_iter()
        .map(|(id, a, synced_at)| AccountView { id, name: a.name, url: a.url, calendars: a.calendars, synced_at })
        .collect())
}

#[tauri::command]
pub async fn add_calendar_account(state: AppStateRef<'_>, account: CalAccount) -> Result<String, String> {
    ensure_on(&state)?;
    let c = state.cipher()?;
    web_allowed(&state)?;
    let mut a = account;
    a.url = a.url.trim().to_string();
    if !a.url.starts_with("https://") && !reqwest::Url::parse(&a.url).ok().and_then(|u| u.host_str().map(|h| matches!(h, "127.0.0.1" | "localhost"))).unwrap_or(false) {
        return Err("Use the https:// address your provider gives you.".into());
    }
    a.calendars = Dav::new(&a)?.discover(&a.url).await?;
    if a.name.trim().is_empty() {
        a.name = reqwest::Url::parse(&a.url).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_else(|| "Calendar".into());
    }
    let id = uuid::Uuid::new_v4().to_string();
    {
        let conn = state.db.lock().unwrap();
        store_account(&conn, &c, &id, &a)?;
        db::log_action(&conn, "calendar", &format!("Connected the calendar account {} ({} calendars)", a.name, a.calendars.len()));
    }
    sync_account(&state, &c, &id).await?;
    Ok(id)
}

#[tauri::command]
pub fn remove_calendar_account(state: AppStateRef, id: String) -> Result<(), String> {
    state.cipher()?;
    let conn = state.db.lock().unwrap();
    conn.execute("DELETE FROM events WHERE account_id = ?1", [&id]).map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM cal_accounts WHERE id = ?1", [&id]).map_err(|e| e.to_string())?;
    db::log_action(&conn, "calendar", "Removed a calendar account and its copied events");
    Ok(())
}

#[tauri::command]
pub async fn sync_calendars(state: AppStateRef<'_>) -> Result<usize, String> {
    ensure_on(&state)?;
    let c = state.cipher()?;
    sync_all(&state, &c).await
}

#[tauri::command]
pub fn calendar_events(state: AppStateRef, from: i64, to: i64) -> Result<Vec<Event>, String> {
    ensure_on(&state)?;
    let c = state.cipher()?;
    Ok(events(&state.db.lock().unwrap(), &c, from, to))
}

#[tauri::command]
pub async fn create_event(state: AppStateRef<'_>, event: NewEvent) -> Result<Event, String> {
    ensure_on(&state)?;
    let c = state.cipher()?;
    create(&state, &c, event).await
}

#[tauri::command]
pub async fn delete_event(state: AppStateRef<'_>, id: String) -> Result<(), String> {
    ensure_on(&state)?;
    let c = state.cipher()?;
    delete(&state, &c, &id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    const ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:abc@example.com\r\nSUMMARY:Budget review\\, Q4\r\nDTSTART;TZID=America/Denver:20261009T140000\r\nDTEND;TZID=America/Denver:20261009T150000\r\nLOCATION:Room 2\r\nDESCRIPTION:Bring the\\n numbers\r\nATTENDEE;CN=Ana:mailto:ana@example.com\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:day@x\r\nSUMMARY:Holiday\r\nDTSTART;VALUE=DATE:20261012\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:gone@x\r\nSUMMARY:Cancelled thing\r\nSTATUS:CANCELLED\r\nDTSTART:20261009T180000Z\r\nDURATION:PT30M\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:long@x\r\nSUMMARY:A very long title that the ser\r\n ver folded\r\nDTSTART:20261010T170000Z\r\nDURATION:PT1H30M\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn icalendar_is_read() {
        let ev = parse_ics(ICS);
        assert_eq!(ev.len(), 3, "the cancelled one is skipped");
        let (s, e, all_day, d) = &ev[0];
        assert_eq!(d.title, "Budget review, Q4");
        assert_eq!(d.notes, "Bring the\n numbers");
        assert_eq!(d.attendees, vec!["ana@example.com"]);
        // 2pm in Denver (MDT, UTC-6) is 20:00 UTC.
        assert_eq!(*s, Utc.with_ymd_and_hms(2026, 10, 9, 20, 0, 0).unwrap().timestamp_millis());
        assert_eq!(e - s, 3_600_000);
        assert!(!all_day);
        assert!(ev[1].2 && ev[1].1 - ev[1].0 == DAY_MS, "all-day, one day long");
        assert_eq!(ev[2].3.title, "A very long title that the server folded");
        assert_eq!(ev[2].1 - ev[2].0, 90 * 60_000);
    }

    #[test]
    fn events_round_trip_through_ics() {
        let e = Event {
            id: "x".into(),
            account_id: None,
            calendar: None,
            start: Utc.with_ymd_and_hms(2026, 10, 9, 20, 0, 0).unwrap().timestamp_millis(),
            end: Utc.with_ymd_and_hms(2026, 10, 9, 21, 0, 0).unwrap().timestamp_millis(),
            all_day: false,
            data: EventData { title: "Lunch; with Bo, maybe".into(), notes: "line1\nline2".into(), attendees: vec!["bo@x.com".into()], uid: "u1@sulcusai".into(), ..Default::default() },
        };
        let ics = to_ics(&e, Some("me@x.com"));
        let back = parse_ics(&ics);
        assert_eq!(back.len(), 1);
        assert_eq!((back[0].0, back[0].1), (e.start, e.end));
        assert_eq!(back[0].3.title, e.data.title);
        assert_eq!(back[0].3.notes, e.data.notes);
        assert_eq!(back[0].3.attendees, vec!["bo@x.com"]);
        assert!(ics.contains("ORGANIZER:mailto:me@x.com"));
    }

    #[test]
    fn free_time_skips_busy_blocks() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 9).unwrap();
        let at = |h: u32, m: u32| Local.from_local_datetime(&day.and_hms_opt(h, m, 0).unwrap()).unwrap().timestamp_millis();
        let busy = [(at(10, 0), at(11, 0)), (at(10, 30), at(12, 0)), (at(15, 0), at(15, 20))];
        let slots = free_slots(&busy, at(0, 0), at(23, 59), 30, 9, 17);
        assert_eq!(slots, vec![(at(9, 0), at(10, 0)), (at(12, 0), at(15, 0)), (at(15, 20), at(17, 0))]);
        let long = free_slots(&busy, at(0, 0), at(23, 59), 120, 9, 17);
        assert_eq!(long, vec![(at(12, 0), at(15, 0))]);
    }

    #[test]
    fn webdav_answers_are_read() {
        let xml = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
 <d:response><d:href>/dav/calendars/user/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
 <d:response><d:href>/dav/calendars/user/work/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><c:calendar/></d:resourcetype><d:displayname>Work &amp; school</d:displayname></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
  <d:propstat><d:prop><x:calendar-color xmlns:x="http://apple.com/ns/ical/"/></d:prop><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat></d:response>
 <d:response><d:href>/p/</d:href><d:propstat><d:prop><d:current-user-principal><d:href>/principals/user/</d:href></d:current-user-principal></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
</d:multistatus>"#;
        let r = parse_multistatus(xml);
        assert_eq!(r.len(), 3);
        assert!(!r[0].types.contains(&"calendar".to_string()));
        assert!(r[1].types.contains(&"calendar".to_string()));
        assert_eq!(r[1].props.get("displayname").map(String::as_str), Some("Work & school"));
        assert_eq!(r[2].hrefs.get("current-user-principal").map(String::as_str), Some("/principals/user/"));
    }
}

/// Against a local Radicale on 5232 (user demo / demopass):
/// `cargo test --lib caldav_round_trip -- --ignored --nocapture`
#[cfg(test)]
mod live {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn caldav_round_trip() {
        let a = CalAccount { name: "Test".into(), url: "http://127.0.0.1:5232/".into(), username: "demo".into(), password: "demopass".into(), calendars: vec![] };
        let dav = Dav::new(&a).unwrap();
        // A fresh calendar to work in.
        let cal = format!("http://127.0.0.1:5232/demo/t{}/", uuid::Uuid::new_v4().simple());
        let body = r#"<?xml version="1.0"?><c:mkcalendar xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:set><d:prop><d:displayname>Work</d:displayname></d:prop></d:set></c:mkcalendar>"#;
        let (status, _) = dav.call("MKCALENDAR", &cal, "0", body.into()).await.unwrap();
        assert_eq!(status, 201);

        let wrong = Dav::new(&CalAccount { password: "nope".into(), ..a.clone() }).unwrap();
        let e = wrong.discover(&a.url).await.unwrap_err();
        println!("wrong password: {e}");

        let cals = dav.discover(&a.url).await.unwrap();
        println!("calendars: {cals:?}");
        assert!(cals.iter().any(|c| c.href == cal && c.name == "Work"));

        let now = db::now_ms();
        let start = (now / 3_600_000 + 24) * 3_600_000;
        let one = Event {
            id: "x".into(),
            account_id: None,
            calendar: None,
            start,
            end: start + 3_600_000,
            all_day: false,
            data: EventData { title: "Dentist, then lunch".into(), location: "Main St".into(), uid: "one@sulcusai".into(), ..Default::default() },
        };
        let etag = dav.put(&format!("{cal}one.ics"), to_ics(&one, None), None).await.unwrap();
        println!("created, etag {etag:?}");
        // A weekly event, three times.
        let weekly = to_ics(&Event { data: EventData { title: "Standup".into(), uid: "weekly@sulcusai".into(), ..Default::default() }, start: start + 7_200_000, end: start + 9_000_000, ..one.clone() }, None)
            .replace("END:VEVENT", "RRULE:FREQ=WEEKLY;COUNT=3\r\nEND:VEVENT");
        dav.put(&format!("{cal}weekly.ics"), weekly, None).await.unwrap();

        let got = dav.events(&cal, now - DAY_MS, now + 60 * DAY_MS).await.unwrap();
        let parsed: Vec<_> = got.iter().flat_map(|(_, _, ics)| parse_ics(ics)).collect();
        for (s, _, _, d) in &parsed {
            println!("  {} at {} (recurring {})", d.title, Local.timestamp_millis_opt(*s).unwrap(), d.recurring);
        }
        assert_eq!(parsed.iter().filter(|p| p.3.title == "Standup").count(), 3, "expanded by the server");
        let dentist = parsed.iter().find(|p| p.3.title == "Dentist, then lunch").unwrap();
        assert_eq!((dentist.0, dentist.1), (one.start, one.end));
        assert_eq!(dentist.3.location, "Main St");

        dav.delete(&format!("{cal}one.ics"), etag.as_deref()).await.unwrap();
        let after = dav.events(&cal, now - DAY_MS, now + 60 * DAY_MS).await.unwrap();
        assert!(!after.iter().any(|(_, _, ics)| ics.contains("Dentist")));
        dav.call("DELETE", &cal, "0", String::new()).await.unwrap();
    }
}
