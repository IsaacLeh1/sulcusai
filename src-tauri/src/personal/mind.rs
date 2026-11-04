// SPDX-License-Identifier: AGPL-3.0-only
//! Mind: journal streaks and prompts, meditation timing, tasks pulled out of
//! rambling text, and the profile coach for resumes and profiles.

use chrono::{Datelike, Duration, NaiveDate, Weekday};
use serde::Serialize;

/// Days in a row with an entry, counting back from today (or yesterday, so
/// the streak isn't lost before today's entry is written).
pub fn journal_streak(days: &[NaiveDate], today: NaiveDate) -> u32 {
    let mut d = if days.contains(&today) { today } else { today - Duration::days(1) };
    let mut n = 0;
    while days.contains(&d) {
        n += 1;
        d -= Duration::days(1);
    }
    n
}

const PROMPTS: &[&str] = &[
    "What went well today?",
    "What is on your mind right now?",
    "Who made your day better, and how?",
    "What would make tomorrow good?",
    "What did you learn this week?",
    "What are you putting off, and why?",
    "What are you grateful for?",
];

/// One prompt per day, the same all day.
pub fn journal_prompt(day: NaiveDate) -> &'static str {
    PROMPTS[day.num_days_from_ce() as usize % PROMPTS.len()]
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Phase {
    pub name: &'static str,
    pub seconds: u32,
}

/// Box breathing: in, hold, out, hold, repeated to fill the minutes.
pub fn breathing(minutes: u32, each: u32) -> Vec<Phase> {
    let each = each.max(1);
    let rounds = (minutes * 60) / (each * 4);
    (0..rounds).flat_map(|_| ["Breathe in", "Hold", "Breathe out", "Hold"].map(|name| Phase { name, seconds: each })).collect()
}

#[derive(Debug, Serialize, PartialEq)]
pub struct LooseTask {
    pub title: String,
    #[serde(serialize_with = "super::opt_date_text")]
    pub due: Option<NaiveDate>,
}

const STARTERS: &[&str] = &["i need to ", "i have to ", "i should ", "i must ", "need to ", "have to ", "remember to ", "don't forget to ", "gotta ", "i want to "];

fn due_in(lower: &str, today: NaiveDate) -> Option<NaiveDate> {
    if lower.contains("today") || lower.contains("tonight") {
        return Some(today);
    }
    if lower.contains("tomorrow") {
        return Some(today + Duration::days(1));
    }
    if lower.contains("next week") {
        return Some(today + Duration::days(7));
    }
    let days = [
        ("monday", Weekday::Mon), ("tuesday", Weekday::Tue), ("wednesday", Weekday::Wed), ("thursday", Weekday::Thu),
        ("friday", Weekday::Fri), ("saturday", Weekday::Sat), ("sunday", Weekday::Sun),
    ];
    days.iter().find(|(w, _)| lower.contains(w)).map(|(_, wd)| {
        let ahead = (wd.num_days_from_monday() as i64 - today.weekday().num_days_from_monday() as i64).rem_euclid(7);
        today + Duration::days(if ahead == 0 { 7 } else { ahead })
    })
}

fn strip_when(s: &str) -> String {
    let mut t = s.to_string();
    for w in ["by tomorrow", "tomorrow", "today", "tonight", "next week", "on monday", "on tuesday", "on wednesday", "on thursday", "on friday", "on saturday", "on sunday", "by friday", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"] {
        if let Some(p) = t.to_lowercase().find(w) {
            t.replace_range(p..p + w.len(), "");
        }
    }
    t.split_whitespace().collect::<Vec<_>>().join(" ").trim_end_matches([',', ' ']).to_string()
}

/// Tasks from a stream of thought: each sentence or "and"-joined clause that
/// starts like a to-do becomes one, with a due day when one is named.
pub fn tasks_from_rambling(text: &str, today: NaiveDate) -> Vec<LooseTask> {
    let mut out = Vec::new();
    for sentence in text.split(['.', '!', '?', '\n', ';']) {
        let mut carry: Option<NaiveDate> = None;
        for (i, clause) in sentence.split(" and ").enumerate() {
            let c = clause.trim();
            let lower = c.to_lowercase();
            let rest = STARTERS.iter().find_map(|s| lower.find(s).map(|p| &c[p + s.len()..]));
            let rest = match (rest, i) {
                (Some(r), _) => r,
                // "I need to call Bo and book the vet": the second clause
                // keeps the first one's intent.
                (None, n) if n > 0 && !out.is_empty() && lower.split_whitespace().count() <= 6 => c,
                _ => continue,
            };
            let due = due_in(&lower, today).or(carry);
            carry = due;
            let title = strip_when(rest);
            if !title.is_empty() {
                let mut chars = title.chars();
                let title = chars.next().unwrap().to_uppercase().collect::<String>() + chars.as_str();
                out.push(LooseTask { title, due });
            }
        }
    }
    out
}

/// Plain checks on a resume or profile text. Each tip is one thing to fix.
pub fn profile_tips(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let words = text.split_whitespace().count();
    let mut tips = Vec::new();
    if words < 150 {
        tips.push("It's short. Add a line on what you did and what came of it for each role.".into());
    }
    if words > 900 {
        tips.push("It's long. Keep it to what matters for the job you want.".into());
    }
    if !text.chars().any(|c| c.is_ascii_digit()) {
        tips.push("No numbers yet. Results with numbers (time saved, people helped, sales) stand out.".into());
    }
    if !lower.contains('@') {
        tips.push("No email address found. Add a way to reach you.".into());
    }
    let weak = ["responsible for", "duties included", "worked on", "helped with"];
    if let Some(w) = weak.iter().find(|w| lower.contains(*w)) {
        tips.push(format!("\"{w}\" says little. Start lines with what you did: led, built, cut, grew."));
    }
    if lower.matches(" i ").count() > 5 {
        tips.push("Many lines start with \"I\". Resumes usually leave it out.".into());
    }
    tips
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn streak_counts_back() {
        let today = d(2026, 11, 4);
        assert_eq!(journal_streak(&[d(2026, 11, 3), d(2026, 11, 2), d(2026, 10, 30)], today), 2);
        assert_eq!(journal_streak(&[today], today), 1);
        assert_eq!(journal_streak(&[], today), 0);
        assert_eq!(journal_prompt(today), journal_prompt(today));
    }

    #[test]
    fn breathing_fills_minutes() {
        let p = breathing(1, 4);
        assert_eq!(p.len(), 12);
        assert_eq!(p[0], Phase { name: "Breathe in", seconds: 4 });
    }

    #[test]
    fn tasks_from_a_ramble() {
        let today = d(2026, 11, 4); // a Wednesday
        let t = tasks_from_rambling("Ugh, long day. I need to call the dentist tomorrow and book the vet. Remember to pay rent on Friday! The weather was nice.", today);
        assert_eq!(
            t,
            vec![
                LooseTask { title: "Call the dentist".into(), due: Some(d(2026, 11, 5)) },
                LooseTask { title: "Book the vet".into(), due: Some(d(2026, 11, 5)) },
                LooseTask { title: "Pay rent".into(), due: Some(d(2026, 11, 6)) },
            ]
        );
        assert!(tasks_from_rambling("Nothing to do here.", today).is_empty());
    }

    #[test]
    fn profile_coach_points_at_fixes() {
        let tips = profile_tips("Responsible for things at a shop");
        assert!(tips.iter().any(|t| t.contains("short")));
        assert!(tips.iter().any(|t| t.contains("numbers")));
        assert!(tips.iter().any(|t| t.contains("responsible for")));
        let good = format!("ana@mail.com Led 4 people, cut costs 20%. {}", "Built tools that shipped. ".repeat(40));
        assert!(profile_tips(&good).is_empty());
    }
}
