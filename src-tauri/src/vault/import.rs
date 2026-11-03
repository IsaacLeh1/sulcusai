//! Reading other password managers' export files. Each file is read as
//! untrusted input: anything malformed is skipped or refused, never a panic,
//! and nothing here reads another app's own password store.

use serde::Serialize;
use zeroize::Zeroize;

use super::import_formats;
use super::store::Entry;
use super::VaultError;

/// Larger than any real export; stops a wrong file from eating memory.
pub const MAX_IMPORT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Chrome and Edge write the same CSV.
    ChromeOrEdge,
    Firefox,
    KeePass,
    KeePassXc,
    Bitwarden,
    OnePassword,
    /// A CSV with a password column that matches no known layout.
    OtherCsv,
}

#[derive(Debug, Default)]
pub struct Imported {
    pub source: Option<Source>,
    pub entries: Vec<Entry>,
    /// Rows that weren't logins or had no password.
    pub skipped: usize,
}

/// Reads any supported export, telling the format from its first bytes.
pub fn import_any(bytes: &[u8]) -> Result<Imported, VaultError> {
    if bytes.len() > MAX_IMPORT_BYTES {
        return Err(VaultError::BadImport("this file is too large to be a password export".into()));
    }
    if bytes.starts_with(b"PK\x03\x04") {
        return import_formats::one_password_1pux(bytes);
    }
    if super::store::is_export(bytes) {
        return Err(VaultError::BadImport("this is a SulcusAI vault export: open it with its export password".into()));
    }
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim_start_matches('\u{feff}').trim_start();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return import_formats::bitwarden_json(trimmed);
    }
    if trimmed.starts_with('<') {
        return import_formats::keepass_xml(trimmed);
    }
    import_csv(trimmed)
}

/// RFC 4180 CSV, forgiving the ways real exports bend it: CR, LF or CRLF line
/// ends, quotes inside unquoted fields taken literally, and an unclosed quote
/// running to the end of the file.
pub(crate) fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut at_start = true;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if at_start => {
                quoted = true;
                at_start = false;
            }
            ',' => {
                row.push(std::mem::take(&mut field));
                at_start = true;
            }
            '\r' | '\n' => {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                at_start = true;
            }
            c => {
                field.push(c);
                at_start = false;
            }
        }
    }
    if !field.is_empty() || !row.is_empty() || quoted {
        row.push(field);
        rows.push(row);
    }
    rows.retain(|r| r.iter().any(|f| !f.trim().is_empty()));
    rows
}

fn wipe_rows(rows: &mut Vec<Vec<String>>) {
    for r in rows.iter_mut() {
        for f in r.iter_mut() {
            f.zeroize();
        }
    }
    rows.clear();
}

fn key(header: &str) -> String {
    header.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

fn find(headers: &[String], names: &[&str]) -> Option<usize> {
    names.iter().find_map(|n| headers.iter().position(|h| h.as_str() == *n))
}

fn detect(headers: &[String]) -> Source {
    let has = |n: &str| headers.iter().any(|h| h == n);
    if has("httprealm") || has("formactionorigin") || has("timepasswordchanged") {
        Source::Firefox
    } else if has("loginuri") || has("loginpassword") {
        Source::Bitwarden
    } else if has("loginname") && has("website") {
        Source::KeePass
    } else if has("group") && has("title") {
        Source::KeePassXc
    } else if has("otpauth") || (has("title") && has("url")) || (has("title") && has("website")) {
        Source::OnePassword
    } else if has("name") && has("url") && has("username") && has("password") {
        Source::ChromeOrEdge
    } else {
        Source::OtherCsv
    }
}

pub(crate) fn import_csv(text: &str) -> Result<Imported, VaultError> {
    let mut rows = parse_csv(text);
    if rows.is_empty() {
        return Err(VaultError::BadImport("the file is empty".into()));
    }
    let headers: Vec<String> = rows[0].iter().map(|h| key(h)).collect();
    let Some(pw) = find(&headers, &["password", "loginpassword", "pass", "passwd"]) else {
        wipe_rows(&mut rows);
        return Err(VaultError::BadImport("this file has no password column, so it isn't a password export SulcusAI knows".into()));
    };
    let url = find(&headers, &["url", "loginuri", "website", "uri", "weburl", "address", "origin", "hostname"]);
    let action = find(&headers, &["formactionorigin"]);
    let user = find(&headers, &["username", "loginusername", "loginname", "user", "login", "email", "emailaddress"]);
    let notes = find(&headers, &["note", "notes", "comments", "comment", "extra"]);
    let name = find(&headers, &["name", "title", "account"]);
    let kind = find(&headers, &["type"]);
    let created = find(&headers, &["timecreated", "created", "creationtime", "createdat"]);
    let changed = find(&headers, &["timepasswordchanged", "passwordchanged", "lastmodified", "modified", "modificationtime", "updatedat", "timelastused"]);

    let mut out = Imported { source: Some(detect(&headers)), ..Default::default() };
    for r in rows.iter().skip(1) {
        let get = |i: Option<usize>| i.and_then(|i| r.get(i)).map(|s| s.trim()).unwrap_or("");
        if kind.is_some() && !get(kind).is_empty() && !get(kind).eq_ignore_ascii_case("login") {
            out.skipped += 1;
            continue;
        }
        let password = i_get(r, pw);
        if password.is_empty() {
            out.skipped += 1;
            continue;
        }
        let mut origin = get(url);
        if origin.is_empty() {
            origin = get(action);
        }
        let mut e = Entry::login(origin, get(user), password);
        e.name = get(name).to_string();
        e.notes = i_get(r, notes.unwrap_or(usize::MAX)).to_string();
        e.created_ms = parse_time(get(created)).unwrap_or(0);
        e.changed_ms = parse_time(get(changed)).unwrap_or(0);
        out.entries.push(e);
    }
    wipe_rows(&mut rows);
    Ok(out)
}

/// Passwords keep their spaces; only the row's other fields are trimmed.
fn i_get(r: &[String], i: usize) -> &str {
    r.get(i).map(String::as_str).unwrap_or("")
}

/// Milliseconds since 1970 from the forms exports use: epoch seconds or
/// milliseconds, RFC 3339, or "YYYY-MM-DD HH:MM:SS" taken as UTC.
pub(crate) fn parse_time(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if s.bytes().all(|b| b.is_ascii_digit()) && s.len() <= 16 {
        let n: i64 = s.parse().ok()?;
        // Epoch seconds pass 1e11 only in the year 5138, so bigger is ms.
        return Some(if n > 100_000_000_000 { n } else { n.saturating_mul(1000) });
    }
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(t.timestamp_millis());
    }
    for f in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"] {
        if let Ok(t) = chrono::NaiveDateTime::parse_from_str(s, f) {
            return Some(t.and_utc().timestamp_millis());
        }
    }
    None
}
