// SPDX-License-Identifier: AGPL-3.0-only
//! One verdict on every tool result: complete, partial (and what's missing)
//! or failed (and why), with the next thing to try. A result cut to size, a
//! page built by scripts, search snippets or a document with unread
//! equations otherwise look complete to the model, and it answers as if it
//! had everything.

use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Complete,
    Partial,
    Failed,
}

impl Status {
    pub fn word(self) -> &'static str {
        match self {
            Status::Complete => "complete",
            Status::Partial => "partial",
            Status::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Envelope {
    pub status: Status,
    pub missing: Vec<String>,
    pub next: Option<String>,
}

/// The one line on what to try after an error, from what it says. None
/// when the error already names a next step or nothing useful fits.
pub fn next_after_error(name: &str, error: &str) -> Option<String> {
    let e = error.to_lowercase();
    if ["request_folder", "next:", "try ", "instead", "use "].iter().any(|k| e.contains(k)) {
        return None;
    }
    let say = |s: &str| Some(s.to_string());
    if e.contains("outside") && (e.contains("folder") || e.contains("shared")) || e.contains("not shared") {
        return say("Call request_folder with that folder so the user can share it, then try again.");
    }
    if e.contains("not found") || e.contains("no such file") || e.contains("cannot find") || e.contains("doesn't exist") || e.contains("does not exist") {
        return say("Check the name with list_dir or find_files, then try again with the exact path.");
    }
    if e.contains("timed out") || e.contains("timeout") || e.contains("connect") || e.contains("network") || e.contains("dns") {
        return say("Try once more; if it fails again, tell the user the site or service couldn't be reached.");
    }
    if e.contains("access is denied") || e.contains("permission denied") {
        return say("The file may be open in another program or read-only. Tell the user, or pick another file name.");
    }
    if e.contains("needs ") || e.contains("must be") || e.contains("expected") {
        return say(&format!("Fix the arguments as the error says and call {name} again."));
    }
    None
}

/// The verdict on a result, from its text and meta.
pub fn judge(name: &str, text: &str, meta: &Value) -> Envelope {
    let status = meta["status"].as_str().unwrap_or("ok");
    if status == "error" {
        let why = meta["detail"].as_str().unwrap_or(text);
        return Envelope { status: Status::Failed, missing: vec!["the whole result".into()], next: next_after_error(name, why) };
    }
    if status == "denied" {
        return Envelope { status: Status::Failed, missing: vec!["the user declined it".into()], next: None };
    }
    let mut missing = Vec::new();
    let mut next = None;
    if meta["coverage"] == "partial" || text.contains("[Coverage: PARTIAL") {
        missing.push("parts of the document the text doesn't hold (see the coverage lines)".into());
        next = Some("Call read_document with mode \"ocr\" or \"look\" and the flagged pages, or say what wasn't read.".into());
    }
    if let Some(at) = text.find("[Cut here: ") {
        let n: String = text[at + 11..].chars().take_while(char::is_ascii_digit).collect();
        missing.push(format!("{n} characters cut off to fit"));
        next.get_or_insert_with(|| match name {
            "read_file" => "Read on with start_line.".to_string(),
            "read_document" => "Read the rest with read_document pages.".to_string(),
            "run_command" | "command_output" => "Run it again with less output (filter it, or write it to a file and read parts).".to_string(),
            _ => "Ask for less at once (a narrower search or one part at a time).".to_string(),
        });
    }
    if text.contains("[The document goes on.]") && !missing.iter().any(|m| m.contains("cut off")) {
        missing.push("the rest of the document".into());
        next.get_or_insert_with(|| "Read the rest with read_document pages.".into());
    }
    match name {
        "web_search" | "research" if !text.trim().is_empty() => {
            missing.push("only short snippets, not the full pages".into());
            next.get_or_insert_with(|| "fetch_page the results that matter before relying on details.".into());
        }
        "fetch_page" | "browser_read" | "browser_text" if text.trim().chars().count() < 300 => {
            missing.push("most of the page: it may be built by scripts, behind a click or sign-in, or hold its text in pictures".into());
            next.get_or_insert_with(|| "Open it in the browser (browser_open, then browser_read) or say the page couldn't be read.".into());
        }
        "email_read" if text.contains("Attachment") || meta["attachments"].as_array().is_some_and(|a| !a.is_empty()) => {
            missing.push("attachments (not read)".into());
            next.get_or_insert_with(|| "Save the attachment and read it with read_document if it matters.".into());
        }
        _ => {}
    }
    let status = if missing.is_empty() { Status::Complete } else { Status::Partial };
    Envelope { status, missing, next }
}

/// Puts the verdict in the result: `meta.envelope` for the window and the
/// log, and one line in the text for the model when it isn't complete (a
/// coverage report already says it, so it's left alone there).
pub fn stamp(name: &str, text: &mut String, meta: &mut Value) -> Envelope {
    let env = judge(name, text, meta);
    if meta.is_object() {
        meta["envelope"] = json!({ "status": env.status.word(), "missing": env.missing, "next": env.next });
    }
    match env.status {
        Status::Complete => {}
        Status::Failed => {
            if let Some(n) = &env.next {
                text.push_str(&format!("\nNext: {n}"));
            }
        }
        Status::Partial => {
            let has_report = text.contains("[Coverage: PARTIAL");
            let mut line = format!("\n\n[Result: partial. Missing: {}.", env.missing.join("; "));
            if let Some(n) = &env.next {
                line.push_str(&format!(" Next: {n}"));
            }
            line.push(']');
            if !has_report {
                text.push_str(&line);
            }
        }
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok() -> Value {
        json!({ "status": "ok" })
    }

    #[test]
    fn a_clean_result_is_complete_and_unchanged() {
        let mut t = "hello".to_string();
        let mut m = ok();
        assert_eq!(stamp("read_file", &mut t, &mut m).status, Status::Complete);
        assert_eq!(t, "hello");
        assert_eq!(m["envelope"]["status"], "complete");
    }

    #[test]
    fn a_cut_result_says_where_it_stopped() {
        let mut t = "abc\n\n[Cut here: 5000 more characters weren't shown, to fit your memory.]".to_string();
        let mut m = ok();
        let e = stamp("read_file", &mut t, &mut m);
        assert_eq!(e.status, Status::Partial);
        assert!(t.contains("5000 characters cut off") && t.contains("start_line"), "{t}");
        let mut c = "out\n[Cut here: 20 more characters of output weren't shown]".to_string();
        assert!(stamp("run_command", &mut c, &mut ok()).next.unwrap().contains("less output"));
    }

    #[test]
    fn errors_say_what_to_do_next() {
        let mut t = "Error: That's outside the folders you've shared.".to_string();
        let mut m = json!({ "status": "error", "detail": "That's outside the folders you've shared." });
        let e = stamp("read_file", &mut t, &mut m);
        assert_eq!(e.status, Status::Failed);
        assert!(t.ends_with("then try again."), "{t}");
        assert!(next_after_error("x", "Not found. Try find_files instead.").is_none(), "already says what to do");
        assert!(next_after_error("fetch_page", "connection timed out").unwrap().contains("once more"));
        assert_eq!(judge("x", "", &json!({ "status": "denied" })).status, Status::Failed);
    }

    #[test]
    fn coverage_snippets_thin_pages_and_attachments_are_partial() {
        let mut t = "text\n\n[Coverage: PARTIAL. page 2: equations]".to_string();
        let mut m = json!({ "status": "ok", "coverage": "partial" });
        let e = stamp("read_document", &mut t, &mut m);
        assert_eq!(e.status, Status::Partial);
        assert!(!t.contains("[Result: partial"), "the coverage lines already say it");
        assert!(e.next.unwrap().contains("ocr"));
        assert_eq!(judge("web_search", "1. a site - snippet", &ok()).status, Status::Partial);
        assert_eq!(judge("web_search", "", &ok()).status, Status::Complete);
        assert_eq!(judge("fetch_page", "Loading...", &ok()).status, Status::Partial);
        assert_eq!(judge("fetch_page", &"words ".repeat(100), &ok()).status, Status::Complete);
        assert_eq!(judge("email_read", "Hi\nAttachments: a.pdf", &ok()).status, Status::Partial);
    }
}
