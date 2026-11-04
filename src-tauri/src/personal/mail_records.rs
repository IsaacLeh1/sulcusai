// SPDX-License-Identifier: AGPL-3.0-only
//! Mail to records without a model: plain patterns for the common shapes of
//! shipping notices, bookings, invitations, newsletters, birthdays and job
//! applications. A model pass can refine these later; patterns keep the
//! first pass instant and fully offline.

use std::sync::OnceLock;

use chrono::{Datelike, NaiveDate, TimeZone, Utc};
use regex::Regex;
use serde_json::{json, Value};

pub struct Found {
    pub kind: &'static str,
    pub at: i64,
    pub data: Value,
}

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).unwrap())
}

/// Carrier and tracking number, checked most specific first so a UPS
/// number isn't read as a generic one.
pub fn tracking(text: &str) -> Option<(&'static str, String)> {
    static UPS: OnceLock<Regex> = OnceLock::new();
    static USPS: OnceLock<Regex> = OnceLock::new();
    static FEDEX: OnceLock<Regex> = OnceLock::new();
    static AMZN: OnceLock<Regex> = OnceLock::new();
    if let Some(m) = re(&UPS, r"\b1Z[0-9A-Z]{16}\b").find(text) {
        return Some(("UPS", m.as_str().into()));
    }
    if let Some(m) = re(&AMZN, r"\bTBA\d{12}\b").find(text) {
        return Some(("Amazon", m.as_str().into()));
    }
    if let Some(m) = re(&USPS, r"\b9[2-5]\d{20}\b").find(text) {
        return Some(("USPS", m.as_str().into()));
    }
    let lower = text.to_lowercase();
    if lower.contains("fedex") {
        if let Some(m) = re(&FEDEX, r"\b\d{12}\b").find(text) {
            return Some(("FedEx", m.as_str().into()));
        }
    }
    None
}

const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

/// The first date in the text: 2026-11-20, 11/20/2026, or "November 20"
/// (taken in the year of `received`, or the next one if already past).
pub fn first_date(text: &str, received: NaiveDate) -> Option<NaiveDate> {
    static ISO: OnceLock<Regex> = OnceLock::new();
    static US: OnceLock<Regex> = OnceLock::new();
    static WORD: OnceLock<Regex> = OnceLock::new();
    let mut best: Option<(usize, NaiveDate)> = None;
    let mut keep = |pos: usize, d: Option<NaiveDate>| {
        if let Some(d) = d {
            if best.map_or(true, |(p, _)| pos < p) {
                best = Some((pos, d));
            }
        }
    };
    if let Some(c) = re(&ISO, r"\b(\d{4})-(\d{2})-(\d{2})\b").captures(text) {
        keep(c.get(0).unwrap().start(), NaiveDate::from_ymd_opt(c[1].parse().ok()?, c[2].parse().ok()?, c[3].parse().ok()?));
    }
    if let Some(c) = re(&US, r"\b(\d{1,2})/(\d{1,2})/(\d{4})\b").captures(text) {
        keep(c.get(0).unwrap().start(), NaiveDate::from_ymd_opt(c[3].parse().ok()?, c[1].parse().ok()?, c[2].parse().ok()?));
    }
    if let Some(c) = re(&WORD, r"(?i)\b(jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)[a-z]*\.?\s+(\d{1,2})\b").captures(text) {
        let m = MONTHS.iter().position(|x| c[1].to_lowercase() == *x).unwrap() as u32 + 1;
        let day: u32 = c[2].parse().ok()?;
        let d = NaiveDate::from_ymd_opt(received.year(), m, day)
            .and_then(|d| if d < received { NaiveDate::from_ymd_opt(received.year() + 1, m, day) } else { Some(d) });
        keep(c.get(0).unwrap().start(), d);
    }
    best.map(|(_, d)| d)
}

fn ms(d: NaiveDate) -> i64 {
    Utc.from_utc_datetime(&d.and_hms_opt(12, 0, 0).unwrap()).timestamp_millis()
}

fn code(text: &str) -> Option<String> {
    static CODE: OnceLock<Regex> = OnceLock::new();
    re(&CODE, r"(?i)\b(?:confirmation|booking|reservation|record locator|pnr)(?:\s+(?:number|code|no\.?|#))?\s*[:#]?\s*([A-Z0-9]{5,10})\b")
        .captures(text)
        .map(|c| c[1].to_string())
}

fn has(lower: &str, words: &[&str]) -> bool {
    words.iter().any(|w| lower.contains(w))
}

pub fn read(subject: &str, from: &str, body: &str, received_ms: i64) -> Option<Found> {
    let text = format!("{subject}\n{body}");
    let lower = text.to_lowercase();
    let received = Utc.timestamp_millis_opt(received_ms).single()?.date_naive();
    let date = first_date(&text, received);
    let at = date.map(ms).unwrap_or(received_ms);
    let base = json!({ "subject": subject, "from": from, "date": date.map(|d| d.to_string()) });
    let with = |extra: Value| {
        let mut v = base.clone();
        if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
            o.extend(e.clone());
        }
        v
    };

    if let Some((carrier, number)) = tracking(&text) {
        let status = if has(&lower, &["delivered"]) {
            "delivered"
        } else if has(&lower, &["out for delivery"]) {
            "out_for_delivery"
        } else {
            "on_the_way"
        };
        return Some(Found { kind: "package", at, data: with(json!({ "carrier": carrier, "tracking": number, "status": status })) });
    }
    if has(&lower, &["flight", "hotel", "reservation", "check-in", "boarding", "itinerary", "car rental"]) && (code(&text).is_some() || date.is_some()) {
        let what = if has(&lower, &["flight", "boarding"]) {
            "flight"
        } else if has(&lower, &["hotel", "check-in"]) {
            "stay"
        } else if lower.contains("car rental") {
            "car"
        } else {
            "other"
        };
        return Some(Found { kind: "booking", at, data: with(json!({ "what": what, "code": code(&text) })) });
    }
    if has(&lower, &["birthday"]) {
        return Some(Found { kind: "birthday", at, data: with(json!({})) });
    }
    if has(&lower, &["your application", "applying to", "application received", "thank you for applying", "interview"]) {
        let stage = if lower.contains("interview") {
            "interview"
        } else if has(&lower, &["unfortunately", "not moving forward", "other candidates"]) {
            "declined"
        } else if lower.contains("offer") {
            "offer"
        } else {
            "applied"
        };
        return Some(Found { kind: "application", at, data: with(json!({ "stage": stage })) });
    }
    if has(&lower, &["invited", "invitation", "rsvp", "you're invited", "join us"]) && date.is_some() {
        return Some(Found { kind: "event", at, data: with(json!({})) });
    }
    if has(&lower, &["unsubscribe", "newsletter", "view in browser"]) {
        return Some(Found { kind: "newsletter", at: received_ms, data: with(json!({})) });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOV4: i64 = 1_793_793_600_000; // 2026-11-04 12:00 UTC

    #[test]
    fn packages_with_carrier_and_status() {
        let f = read("Your package is out for delivery", "ups@ups.com", "Tracking 1Z999AA10123456784", NOV4).unwrap();
        assert_eq!(f.kind, "package");
        assert_eq!(f.data["carrier"], "UPS");
        assert_eq!(f.data["status"], "out_for_delivery");
        assert_eq!(tracking("USPS 9400111899223817444444").unwrap().0, "USPS");
        assert_eq!(tracking("FedEx tracking 123456789012").unwrap().0, "FedEx");
        assert!(tracking("call 123456789012").is_none());
    }

    #[test]
    fn bookings_events_birthdays_applications_newsletters() {
        let b = read("Flight confirmation", "air", "Confirmation code: QX7P2L. Departs Nov 20", NOV4).unwrap();
        assert_eq!((b.kind, b.data["what"].as_str(), b.data["code"].as_str()), ("booking", Some("flight"), Some("QX7P2L")));
        assert_eq!(b.data["date"], "2026-11-20");
        let e = read("You're invited", "sam", "Party on 2026-12-01, RSVP please", NOV4).unwrap();
        assert_eq!(e.kind, "event");
        assert_eq!(read("Birthday reminder", "x", "Ana's birthday is March 3", NOV4).unwrap().data["date"], "2027-03-03");
        let a = read("Thank you for applying", "jobs", "Unfortunately we chose other candidates", NOV4).unwrap();
        assert_eq!(a.data["stage"], "declined");
        assert_eq!(read("Weekly digest", "news", "click unsubscribe", NOV4).unwrap().kind, "newsletter");
        assert!(read("lunch?", "bo", "want to grab lunch", NOV4).is_none());
    }

    #[test]
    fn dates_in_three_shapes() {
        let d = NaiveDate::from_ymd_opt(2026, 11, 4).unwrap();
        assert_eq!(first_date("on 12/25/2026", d), NaiveDate::from_ymd_opt(2026, 12, 25));
        assert_eq!(first_date("Jan 5 then 2026-12-01", d), NaiveDate::from_ymd_opt(2027, 1, 5));
        assert_eq!(first_date("no date", d), None);
    }
}
