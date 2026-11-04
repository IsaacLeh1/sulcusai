// SPDX-License-Identifier: AGPL-3.0-only
//! Chat tools for Language and writing: writing checks, the scam check, PC
//! health and scoring a practice answer. Each is offered only while its
//! switch is on. Posting to social sites has no tool on purpose: only the
//! Post button sends.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::{arg_str, MemoryCtx, Outcome};
use crate::writing::{self, health, help, partner, scam};

pub fn check_writing_params() -> Value {
    json!({ "type": "object", "required": ["text"], "properties": {
        "text": { "type": "string", "description": "The text to check: reading ease, tone and grammar slips." } } })
}

pub fn check_scam_params() -> Value {
    json!({ "type": "object", "required": ["text"], "properties": {
        "text": { "type": "string", "description": "The message, text or link the user is unsure about." },
        "sender": { "type": "string", "description": "The sender's address, when known." } } })
}

pub fn pc_health_params() -> Value {
    json!({ "type": "object", "properties": {} })
}

pub fn score_practice_params() -> Value {
    json!({ "type": "object", "required": ["language", "reference", "answer"], "properties": {
        "language": { "type": "string", "description": "The language being practised, e.g. \"Spanish\"." },
        "reference": { "type": "string", "description": "A natural correct translation you wrote." },
        "answer": { "type": "string", "description": "The user's answer, exactly as typed." } } })
}

fn switch_for(name: &str) -> &'static str {
    match name {
        "check_writing" => "writing_help",
        "check_scam" => "scam_check",
        "pc_health" => "pc_health",
        _ => "partner",
    }
}

pub fn run(name: &str, args: &Value, m: &MemoryCtx<'_>) -> Outcome {
    let title = match name {
        "check_writing" => "Checked the writing",
        "check_scam" => "Checked for scam signs",
        "pc_health" => "Checked this PC",
        _ => "Scored the answer",
    };
    // The switch can be turned off mid-chat; check again at the call.
    if let Err(e) = writing::require(&m.db.lock().unwrap(), switch_for(name)) {
        return Outcome::error(title, e);
    }
    match name {
        "check_writing" => match arg_str(args, "text") {
            Ok(t) => Outcome::ok(writing_text(&help::check(t)), title, "text", None),
            Err(e) => Outcome::error(title, e),
        },
        "check_scam" => match arg_str(args, "text") {
            Ok(t) => {
                let sender = args.get("sender").and_then(Value::as_str).filter(|s| !s.trim().is_empty());
                Outcome::ok(scam_text(&scam::check(t, sender)), title, "text", None)
            }
            Err(e) => Outcome::error(title, e),
        },
        "pc_health" => {
            let h = health::judge(health::take());
            let lines: Vec<String> = h.findings.iter().map(|f| format!("- {} ({}): {}", f.area, f.level, f.text)).collect();
            Outcome::ok(format!("Overall: {}\n{}", h.overall, lines.join("\n")), title, "text", None)
        }
        _ => {
            let (language, reference, answer) = match (arg_str(args, "language"), arg_str(args, "reference"), arg_str(args, "answer")) {
                (Ok(l), Ok(r), Ok(a)) => (l.trim().to_string(), r, a),
                (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return Outcome::error(title, e),
            };
            let s = partner::score(reference, answer);
            let conn = m.db.lock().unwrap();
            let mut all: BTreeMap<String, partner::Progress> = writing::load(&conn, m.cipher, writing::PROGRESS);
            let p = all.entry(language.clone()).or_insert_with(|| partner::Progress { language: language.clone(), ..Default::default() });
            partner::record(p, s.score);
            let (rounds, streak) = (p.rounds, p.streak);
            if let Err(e) = writing::save(&conn, m.cipher, writing::PROGRESS, &all) {
                return Outcome::error(title, e);
            }
            Outcome::ok(practice_text(&s, rounds, streak), title, "text", None)
        }
    }
}

fn writing_text(r: &help::Report) -> String {
    let mut out = format!(
        "{} words, {} sentences (about {} words each), reading ease {} of 100, school grade {}, tone {}.",
        r.words, r.sentences, r.avg_sentence, r.ease, r.grade, r.tone
    );
    if r.long_sentences > 0 {
        out.push_str(&format!(" {} sentences are over 25 words.", r.long_sentences));
    }
    if r.issues.is_empty() {
        out.push_str("\nNo grammar slips found by the rules.");
    } else {
        out.push_str("\nSlips:");
        for i in r.issues.iter().take(30) {
            out.push_str(&format!("\n- {}{}", i.message, i.fix.as_deref().map(|f| format!(" (use \"{f}\")")).unwrap_or_default()));
        }
    }
    out
}

fn scam_text(v: &scam::Verdict) -> String {
    let head = match v.level {
        "likely_scam" => "Likely a scam",
        "careful" => "Some signs of a scam",
        _ => "No common signs found (that doesn't prove it's safe)",
    };
    let signs: Vec<String> = v.signs.iter().map(|s| format!("- {}", s.why)).collect();
    format!("{head} (score {} of 100).\n{}", v.score, signs.join("\n"))
}

fn practice_text(s: &partner::Score, rounds: u32, streak: u32) -> String {
    let marks: Vec<String> = s.words.iter().map(|w| format!("{} [{}]", w.word, w.mark)).collect();
    let missing = if s.missing.is_empty() { String::new() } else { format!("\nLeft out: {}", s.missing.join(", ")) };
    format!("Score {} of 100. Words: {}{missing}\nRounds so far: {rounds}, passes in a row: {streak}.", s.score, marks.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_read_plainly() {
        assert!(writing_text(&help::check("i went home.")).contains("Slips"));
        assert!(scam_text(&scam::check("Buy a gift card now, urgent", None)).starts_with("Some signs") || scam_text(&scam::check("Buy a gift card now, urgent", None)).starts_with("Likely"));
        let s = partner::score("hola amigo", "hola");
        assert!(practice_text(&s, 1, 0).contains("Left out: amigo"));
    }

    #[test]
    fn every_tool_has_a_switch() {
        assert_eq!(switch_for("check_writing"), "writing_help");
        assert_eq!(switch_for("check_scam"), "scam_check");
        assert_eq!(switch_for("pc_health"), "pc_health");
        assert_eq!(switch_for("score_practice"), "partner");
    }
}
