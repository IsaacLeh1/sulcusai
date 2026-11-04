// SPDX-License-Identifier: AGPL-3.0-only
//! The Bug / Feedback button: what the person writes opens as a new issue on
//! the project's GitHub page in their browser, filled in, for them to read
//! and send themselves. Nothing goes to a server run by the developer. Only
//! that text goes, plus a reply address and the version number if they
//! choose to add them. Crash and developer-mode reports go the same way.

const ISSUES: &str = "https://github.com/IsaacLeh1/sulcusai/issues/new";
/// Browsers and GitHub refuse very long addresses; longer text is cut.
const MAX_URL: usize = 7500;

fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The filled-in new-issue page. `kind`: "bug", "idea", "crash" or "dev".
pub fn issue_url(kind: &str, text: &str, contact: Option<&str>, version: bool) -> Result<String, String> {
    let text = crate::secret_scrub::scrub(text.trim());
    if text.is_empty() {
        return Err("Write something first.".into());
    }
    if text.chars().count() > 8000 {
        return Err("That's too long to send (8,000 characters at most).".into());
    }
    let title = format!("[{kind}] {}", text.lines().next().unwrap_or("").chars().take(70).collect::<String>());
    let mut body = text.clone();
    if let Some(c) = contact.map(str::trim).filter(|c| !c.is_empty()) {
        body.push_str(&format!("\n\nReply to: {c}"));
    }
    if version {
        body.push_str(&format!("\n\nSulcusAI {}", env!("CARGO_PKG_VERSION")));
    }
    let label = if kind == "idea" { "enhancement" } else { "bug" };
    let head = format!("{ISSUES}?labels={label}&title={}&body=", encode(&title));
    let mut chars: Vec<char> = body.chars().collect();
    let mut url = format!("{head}{}", encode(&body));
    while url.len() > MAX_URL && !chars.is_empty() {
        let keep = chars.len() * 9 / 10;
        chars.truncate(keep);
        url = format!("{head}{}", encode(&format!("{}\n\n(cut to fit)", chars.iter().collect::<String>())));
    }
    Ok(url)
}

/// Opens the filled-in issue in the browser for the person to send.
pub async fn send(kind: &str, text: &str, contact: Option<&str>, version: bool) -> Result<(), String> {
    // It's a user's own action, so it goes at every level, but not while
    // fully offline (unless the web is kept).
    crate::offline::check(crate::offline::Keep::Web)?;
    let url = issue_url(kind, text, contact, version)?;
    crate::open_with_system(std::ffi::OsStr::new(&url))
}

#[tauri::command]
pub async fn send_feedback(kind: String, text: String, contact: Option<String>, version: bool) -> Result<(), String> {
    let kind = if kind == "bug" { "bug" } else { "idea" };
    send(kind, &text, contact.as_deref(), version).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_is_filled_in_and_bounded() {
        let u = issue_url("bug", "It froze\nwhen I clicked", Some("a@b.c"), true).unwrap();
        assert!(u.starts_with("https://github.com/IsaacLeh1/sulcusai/issues/new?labels=bug&title=%5Bbug%5D%20It%20froze&body="));
        assert!(u.contains("a%40b.c"));
        assert!(issue_url("bug", "  ", None, false).is_err());
        let long = "é".repeat(7000);
        assert!(issue_url("idea", &long, None, false).unwrap().len() <= MAX_URL);
    }
}
