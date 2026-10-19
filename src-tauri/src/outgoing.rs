// SPDX-License-Identifier: AGPL-3.0-only
//! Privacy mode: before a model's tool call sends something off this PC, the
//! person sees what leaves, who receives it, what they could do with it, and
//! how personal it is on a scale of 1 to 5. Then they share it, share less
//! (some parts left out or reworded), or don't share it.
//!
//! The scale:
//! 1. nothing personal;
//! 2. a first or a last name, not both;
//! 3. their interests: what they search for, read or ask about;
//! 4. identifying: a full name, an email address;
//! 5. sensitive: keys, passwords and secrets, addresses, phone numbers, card
//!    and ID numbers.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Who the person is, to recognise them in what's sent.
#[derive(Debug, Clone, Default)]
pub struct Me {
    pub first: Option<String>,
    pub last: Option<String>,
    /// Their own addresses (mail accounts), told apart from other people's.
    pub emails: Vec<String>,
    /// Where they are, as typed in the profile ("Orem, Utah").
    pub place: Option<String>,
}

impl Me {
    /// From the profile's name ("Isaac Lehman": first and last; one word is
    /// taken as a first name) and the mail accounts' addresses.
    pub fn new(name: &str, emails: Vec<String>, place: &str) -> Me {
        let words: Vec<&str> = name.split_whitespace().filter(|w| w.chars().count() > 1).collect();
        let (first, last) = match words.as_slice() {
            [] => (None, None),
            [one] => (Some(one.to_string()), None),
            [first, .., last] => (Some(first.to_string()), Some(last.to_string())),
        };
        let place = place.split(',').next().map(str::trim).filter(|p| p.chars().count() > 2).map(str::to_string);
        Me { first, last, emails: emails.into_iter().map(|e| e.to_lowercase()).collect(), place }
    }
}

/// One thing found in what's sent, with the level it puts it at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Found {
    pub level: u8,
    /// In plain words, for the popup: "your full name", "a phone number".
    pub what: String,
}

/// A street address: a house number, a street name and a street word.
pub(crate) static STREET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b\d{1,6}\s+(?:[NSEW]\.?\s+)?(?:[A-Za-z0-9.'-]+\s+){1,4}(?:street|st|avenue|ave|road|rd|boulevard|blvd|lane|ln|drive|dr|court|ct|way|place|pl|circle|cir|parkway|pkwy|highway|hwy|terrace|ter|trail|trl)\b\.?").unwrap()
});
/// A password or key handed over in words: "password: hunter22", "my api key is …".
static SAID_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:password|passwd|passcode|pin|api[ _-]?key|secret|token|access[ _-]?key|private[ _-]?key)\b(?:\s+is)?\s*[:=]?\s*[`'\x22]?[^\s`'\x22]{6,}").unwrap()
});
/// A long run of letters and digits that looks generated (a key or token).
static TOKENISH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b[A-Za-z0-9_\-]{32,}\b").unwrap());

fn has_word(text: &str, word: &str) -> bool {
    let lower = text.to_lowercase();
    let w = word.to_lowercase();
    lower.match_indices(&w).any(|(i, _)| {
        let before = lower[..i].chars().next_back();
        let after = lower[i + w.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

fn tokenish(s: &str) -> bool {
    let digits = s.chars().filter(char::is_ascii_digit).count();
    let upper = s.chars().filter(char::is_ascii_uppercase).count();
    let lower = s.chars().filter(char::is_ascii_lowercase).count();
    // Mixed like a key, not a long word or a run of one kind.
    digits >= 4 && upper + lower >= 8 && (upper >= 2 || digits >= 8)
}

/// What in `text` is personal, highest level first. Nothing found means
/// nothing personal (level 1); interests (3) are the caller's to add, since
/// they depend on what the text is (a search, a page) more than on its words.
pub fn find(text: &str, me: &Me) -> Vec<Found> {
    let mut out: Vec<Found> = Vec::new();
    let mut add = |level: u8, what: &str| {
        if !out.iter().any(|f| f.what == what) {
            out.push(Found { level, what: what.to_string() });
        }
    };
    if crate::practice::yourcode::SECRET_TEXT.is_match(text) || SAID_SECRET.is_match(text) || TOKENISH.find_iter(text).any(|m| tokenish(m.as_str())) {
        add(5, "what looks like a password, key or secret");
    }
    for (label, re) in crate::cloud::PATTERNS.iter() {
        for m in re.find_iter(text) {
            match *label {
                "card number" if crate::cloud::luhn(m.as_str()) => add(5, "a card number"),
                "SSN" => add(5, "an ID number"),
                "phone" => add(5, "a phone number"),
                "email" if me.emails.iter().any(|e| e.eq_ignore_ascii_case(m.as_str())) => add(4, "your email address"),
                "email" => add(4, "an email address"),
                _ => {}
            }
        }
    }
    if STREET.is_match(text) {
        add(5, "a street address");
    }
    let first = me.first.as_deref().is_some_and(|n| has_word(text, n));
    let last = me.last.as_deref().is_some_and(|n| has_word(text, n));
    match (first, last) {
        (true, true) => add(4, "your full name"),
        (true, false) => add(2, "your first name"),
        (false, true) => add(2, "your last name"),
        _ => {}
    }
    if me.place.as_deref().is_some_and(|p| has_word(text, p)) {
        add(3, "where you live (your town)");
    }
    out.sort_by(|a, b| b.level.cmp(&a.level));
    out
}

/// The level for what's sent: the highest of what was found and `base` (3
/// for searches and pages, which show interests; 1 for public lookups).
pub fn level(found: &[Found], base: u8) -> u8 {
    found.iter().map(|f| f.level).chain([base]).max().unwrap_or(1).clamp(1, 5)
}

/// The level's name, as the popup and settings show it.
pub fn level_name(level: u8) -> &'static str {
    match level {
        1 => "No personal information",
        2 => "A first or last name",
        3 => "Your interests and searches",
        4 => "Identifying information",
        _ => "Sensitive information",
    }
}

// ---------- privacy mode ----------

/// The setting: whether to show what each outside call sends, and the level
/// from which to ask (1 asks about everything; levels below it are sent
/// without asking and only logged).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PrivacyMode {
    pub on: bool,
    pub ask_from: u8,
}

impl Default for PrivacyMode {
    fn default() -> Self {
        PrivacyMode { on: false, ask_from: 1 }
    }
}

pub fn mode(conn: &rusqlite::Connection) -> PrivacyMode {
    let m: PrivacyMode = crate::db::get(conn, "privacy_mode").unwrap_or_default();
    PrivacyMode { ask_from: m.ask_from.clamp(1, 5), ..m }
}

#[tauri::command]
pub fn privacy_mode(state: crate::AppStateRef) -> PrivacyMode {
    mode(&state.db.lock().unwrap())
}

#[tauri::command]
pub fn set_privacy_mode(state: crate::AppStateRef, mode: PrivacyMode) -> Result<(), String> {
    let mode = PrivacyMode { ask_from: mode.ask_from.clamp(1, 5), ..mode };
    let conn = state.db.lock().unwrap();
    crate::db::set(&conn, "privacy_mode", &mode).map_err(|e| e.to_string())?;
    crate::db::log_action(&conn, "privacy", &if mode.on { format!("Privacy mode on: asks before sending anything at level {} or above", mode.ask_from) } else { "Privacy mode off".into() });
    Ok(())
}

/// One part of what's sent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Item {
    /// Where it is in the call's arguments ("query", "searches.1"); None for
    /// what goes along without being an argument (your coordinates).
    pub key: Option<String>,
    pub label: String,
    pub value: String,
    pub level: u8,
    /// Can be reworded; `removable` ones can also be left out.
    pub editable: bool,
    pub removable: bool,
}

/// What a call sends, to whom, what they could do with it, and how personal.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Review {
    pub level: u8,
    pub level_name: String,
    /// Who receives it: "DuckDuckGo (search engine)".
    pub to: String,
    pub items: Vec<Item>,
    pub could: Vec<String>,
    pub found: Vec<Found>,
    /// A note such as "your private connection is on".
    pub note: Option<String>,
}

/// What the review needs to know beyond the call.
#[derive(Debug, Clone, Default)]
pub struct Context {
    pub me: Me,
    pub search: crate::web::Provider,
    /// Web traffic goes through Tor or a proxy.
    pub proxied: bool,
    /// The profile has coordinates, or this PC's location would be used.
    pub coords: bool,
    /// The person or connector named, for People and connector tools.
    pub person: Option<String>,
}

/// The context from what this PC knows: the profile, the mail accounts, the
/// search engine and the private connection.
pub fn context(state: &crate::AppState, cipher: &crate::crypto::Cipher) -> Context {
    let conn = state.db.lock().unwrap();
    let profile = crate::db::profile(&conn, cipher);
    let emails = crate::mail::accounts(&conn, cipher).into_iter().map(|(_, a, _)| a.email).filter(|e| !e.is_empty()).collect();
    Context {
        me: Me::new(&profile.name, emails, &profile.location),
        search: crate::web::settings(&conn).provider,
        proxied: crate::privacy::proxy_for(crate::privacy::Route::Web).is_some(),
        // "Here" uses the profile's coordinates, else its town, else this PC's location.
        coords: profile.lat.is_some() || profile.location.trim().is_empty(),
        person: None,
    }
}

pub const SEARCH_COULD: &str = "A search engine can keep your searches with your internet address and the time, and use them to build a profile of your interests for ads, or hand them over if a court asks.";
pub const RESULTS_COULD: &str = "SulcusAI also opens the top results to read them: each of those sites sees your internet address and which page was read.";
pub const SITE_COULD: &str = "The site sees your internet address and the page asked for, can keep both in its logs, and may share them with ad and analytics companies. Anything written into the address goes to it too.";
pub const WEATHER_COULD: &str = "Open-Meteo learns which place you want the weather for, and from coordinates roughly where you are. It can keep this with your internet address.";
pub const WIKI_COULD: &str = "Wikipedia keeps server logs with internet addresses for a short time. What you look up isn't made public.";
pub const QUOTE_COULD: &str = "Yahoo Finance can log which stocks, funds and coins you look up, with your internet address, and use that for ads.";
pub const PLACES_COULD: &str = "OpenStreetMap's servers receive what you're looking for and where. They keep logs with internet addresses.";
pub const BROWSER_COULD: &str = "The site sees everything opened, typed and clicked. With Browser control on, this happens in your own browser where you're signed in, so the site knows who you are.";
pub const PERSON_COULD: &str = "They receive it on their PC and can read, keep and share it. Their assistant reads it too.";
pub const MAIL_COULD: &str = "The people it's sent to get your name and email address with the message and can keep or forward it. Your mail provider keeps a copy.";
pub const CONNECTOR_COULD: &str = "Connectors are programs on this PC. SulcusAI can't see where a connector sends what it's given: it may pass it on to an online service.";
pub const PROXY_NOTE: &str = "Your private connection is on, so these services see the proxy's or Tor's address instead of yours.";

/// Tools that send something off this PC (with privacy mode, they're shown
/// first). Clicking and reading in the browser, and opening files on this
/// PC, aren't among them.
pub fn is_external(tool: &str, args: &Value) -> bool {
    match tool {
        "web_search" | "fetch_page" | "weather" | "wikipedia" | "research" | "market_quote" | "find_places" | "browser_open" | "browser_type" | "email_send"
        | "ask_person" | "work_with_person" => true,
        "open_path" => args.get("target").and_then(Value::as_str).is_some_and(|t| t.starts_with("http://") || t.starts_with("https://")),
        _ => false,
    }
}

fn text_item(key: &str, label: &str, value: &str, me: &Me, base: u8, editable: bool, removable: bool) -> Item {
    Item { key: Some(key.into()), label: label.into(), value: value.into(), level: level(&find(value, me), base), editable, removable }
}

/// What `tool` called with `args` would send, or None when it stays on this PC.
pub fn review(tool: &str, args: &Value, cx: &Context) -> Option<Review> {
    if !is_external(tool, args) {
        return None;
    }
    let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
    let me = &cx.me;
    let search_engine = match cx.search {
        crate::web::Provider::Duckduckgo => "DuckDuckGo (search engine)",
        crate::web::Provider::Brave => "Brave Search (search engine)",
        crate::web::Provider::Searxng => "Your SearXNG server (search engine)",
    };
    let host = |url: &str| reqwest::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_else(|| url.to_string());
    let mut items = Vec::new();
    let mut could: Vec<&str> = Vec::new();
    let to: String = match tool {
        "web_search" => {
            items.push(text_item("query", "Search words", &s("query"), me, 3, true, false));
            could.extend([SEARCH_COULD, RESULTS_COULD]);
            search_engine.into()
        }
        "research" => {
            items.push(text_item("question", "Question", &s("question"), me, 3, true, false));
            for (i, q) in args.get("searches").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).enumerate() {
                items.push(text_item(&format!("searches.{i}"), "Search words", q, me, 3, true, true));
            }
            could.extend([SEARCH_COULD, RESULTS_COULD]);
            format!("{search_engine} and up to 6 web pages")
        }
        "fetch_page" | "browser_open" => {
            let url = s("url");
            items.push(text_item("url", "Web address", &url, me, 3, true, false));
            could.push(if tool == "browser_open" { BROWSER_COULD } else { SITE_COULD });
            host(&url)
        }
        "open_path" => {
            let url = s("target");
            items.push(text_item("target", "Web address", &url, me, 3, true, false));
            could.push(SITE_COULD);
            format!("{} (in your browser)", host(&url))
        }
        "browser_type" => {
            items.push(text_item("text", "Typed into the page", &s("text"), me, 3, true, false));
            could.push(BROWSER_COULD);
            "The website open in the browser".into()
        }
        "weather" => {
            let place = s("place");
            if place.is_empty() || crate::location::means_here(&place) {
                if cx.coords {
                    items.push(Item { key: None, label: "Your location (exact coordinates)".into(), value: me.place.clone().unwrap_or_default(), level: 5, editable: false, removable: false });
                } else if let Some(town) = &me.place {
                    items.push(Item { key: None, label: "Your town (from your profile)".into(), value: town.clone(), level: 3, editable: false, removable: false });
                }
            } else {
                items.push(text_item("place", "Place", &place, me, 1, true, false));
            }
            could.push(WEATHER_COULD);
            "Open-Meteo (weather service)".into()
        }
        "wikipedia" => {
            items.push(text_item("query", "Looked up", &s("query"), me, 3, true, false));
            could.push(WIKI_COULD);
            "Wikipedia".into()
        }
        "market_quote" => {
            for (i, q) in args.get("symbols").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).enumerate() {
                items.push(text_item(&format!("symbols.{i}"), "Symbol or name", q, me, 1, true, true));
            }
            could.push(QUOTE_COULD);
            "Yahoo Finance".into()
        }
        "find_places" => {
            items.push(text_item("query", "Looking for", &s("query"), me, 3, true, false));
            let near = s("near");
            if near.is_empty() || crate::location::means_here(&near) {
                if cx.coords {
                    items.push(Item { key: None, label: "Your location (exact coordinates)".into(), value: me.place.clone().unwrap_or_default(), level: 5, editable: false, removable: false });
                }
            } else {
                items.push(text_item("near", "Near", &near, me, 3, true, false));
            }
            could.push(PLACES_COULD);
            "OpenStreetMap (Nominatim and Overpass)".into()
        }
        "email_send" => {
            for (k, label) in [("to", "To"), ("cc", "Cc"), ("subject", "Subject"), ("body", "Message")] {
                let v = match args.get(k) {
                    Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "),
                    Some(Value::String(v)) => v.clone(),
                    _ => String::new(),
                };
                if !v.trim().is_empty() {
                    items.push(text_item(k, label, &v, me, 3, false, false));
                }
            }
            items.push(Item { key: None, label: "Your name and email address".into(), value: me.emails.first().cloned().unwrap_or_default(), level: 4, editable: false, removable: false });
            could.push(MAIL_COULD);
            "The people it's sent to, by email".into()
        }
        "ask_person" | "work_with_person" => {
            let (k, label) = if tool == "ask_person" { ("question", "Question") } else { ("goal", "What to work out") };
            items.push(text_item(k, label, &s(k), me, 3, true, false));
            could.push(PERSON_COULD);
            cx.person.clone().unwrap_or_else(|| s("person"))
        }
        _ => return None,
    };
    let found = {
        let mut all: Vec<Found> = Vec::new();
        for i in &items {
            for f in find(&i.value, me) {
                if !all.contains(&f) {
                    all.push(f);
                }
            }
            if i.key.is_none() && i.level >= 4 && !all.iter().any(|f| f.what == i.label.to_lowercase()) {
                all.push(Found { level: i.level, what: i.label.to_lowercase() });
            }
        }
        all.sort_by(|a, b| b.level.cmp(&a.level));
        all
    };
    let top = items.iter().map(|i| i.level).max().unwrap_or(1);
    Some(Review {
        level: top,
        level_name: level_name(top).into(),
        to,
        items,
        could: could.into_iter().map(str::to_string).collect(),
        found,
        note: cx.proxied.then(|| PROXY_NOTE.to_string()),
    })
}

/// One line for an approval card that asks anyway (sending mail, asking a
/// linked person): the level, why, and who gets it.
pub fn summary(r: &Review) -> String {
    let why = if r.found.is_empty() { String::new() } else { format!(": {}", r.found.iter().map(|f| f.what.as_str()).collect::<Vec<_>>().join(", ")) };
    format!("Privacy level {} of 5 ({}{why}). Goes to {}.", r.level, r.level_name.to_lowercase(), r.to)
}

/// What the person changed, for the model: so it knows what was really sent.
pub fn changed(edits: &[Edit]) -> String {
    let parts: Vec<String> = edits
        .iter()
        .map(|e| match &e.value {
            Some(v) => format!("{} was sent as \"{v}\"", e.key),
            None => format!("{} was left out", e.key),
        })
        .collect();
    format!("Privacy mode: the user changed what was sent. {}.", parts.join("; "))
}

/// A review of a connector tool call: who knows where it goes.
pub fn review_connector(connector: &str, args: &Value, me: &Me) -> Review {
    let text = serde_json::to_string_pretty(args).unwrap_or_default();
    let found = find(&text, me);
    let lv = level(&found, 1);
    Review {
        level: lv,
        level_name: level_name(lv).into(),
        to: format!("The {connector} connector"),
        items: vec![Item { key: None, label: "What it's given".into(), value: text, level: lv, editable: false, removable: false }],
        could: vec![CONNECTOR_COULD.into()],
        found,
        note: None,
    }
}

/// The person's changes in the popup: a new value for an item, or None to
/// leave it out.
#[derive(Debug, Clone, Deserialize)]
pub struct Edit {
    pub key: String,
    pub value: Option<String>,
}

static EDITS: LazyLock<Mutex<HashMap<String, Vec<Edit>>>> = LazyLock::new(Default::default);

/// The popup's changes, kept until the reply reads them.
#[tauri::command]
pub fn privacy_share(call_id: String, edits: Vec<Edit>) {
    EDITS.lock().unwrap().insert(call_id, edits);
}

pub fn take_edits(call_id: &str) -> Vec<Edit> {
    EDITS.lock().unwrap().remove(call_id).unwrap_or_default()
}

/// `args` with the edits made: only to the keys the review offered as
/// editable (a reworded value) or removable (left out of a list).
pub fn apply(args: &Value, review: &Review, edits: &[Edit]) -> Value {
    let mut out = args.clone();
    let mut removed: Vec<(String, usize)> = Vec::new();
    for e in edits {
        let Some(item) = review.items.iter().find(|i| i.key.as_deref() == Some(e.key.as_str())) else { continue };
        match (&e.value, e.key.split_once('.')) {
            (Some(v), None) if item.editable => out[e.key.as_str()] = Value::String(v.clone()),
            (Some(v), Some((list, i))) if item.editable => {
                if let (Some(arr), Ok(i)) = (out.get_mut(list).and_then(Value::as_array_mut), i.parse::<usize>()) {
                    if let Some(slot) = arr.get_mut(i) {
                        *slot = Value::String(v.clone());
                    }
                }
            }
            (None, Some((list, i))) if item.removable => {
                if let Ok(i) = i.parse() {
                    removed.push((list.to_string(), i));
                }
            }
            _ => {}
        }
    }
    // Left out from the end first, so the numbers still point at the right ones.
    removed.sort_by(|a, b| b.1.cmp(&a.1));
    for (list, i) in removed {
        if let Some(arr) = out.get_mut(&list).and_then(Value::as_array_mut) {
            if i < arr.len() {
                arr.remove(i);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn me() -> Me {
        Me::new("Isaac Lehman", vec!["isaac38.lehman@gmail.com".into()], "Orem, Utah")
    }

    fn top(text: &str, base: u8) -> u8 {
        level(&find(text, &me()), base)
    }

    #[test]
    fn the_scale_follows_what_is_in_the_text() {
        // 1: a public lookup with nothing of theirs.
        assert_eq!(top("AAPL", 1), 1);
        // 2: one of their names.
        assert_eq!(top("restaurants Isaac likes", 1), 2);
        // 3: a search shows interests.
        assert_eq!(top("best hiking boots for wide feet", 3), 3);
        assert_eq!(top("weather in Orem", 1), 3);
        // 4: full name or an email.
        assert_eq!(top("Isaac Lehman resume", 3), 4);
        assert_eq!(top("who owns jane.doe@example.com", 3), 4);
        // 5: secrets, contact details.
        assert_eq!(top("my api key is sk-abc123DEF456ghi789JKL012mno", 3), 5);
        assert_eq!(top("password: hunter22x", 3), 5);
        assert_eq!(top("pizza near 1234 North Main Street", 3), 5);
        assert_eq!(top("call me at (801) 555-0134", 3), 5);
        assert_eq!(top("4111 1111 1111 1111 balance", 3), 5);
    }

    #[test]
    fn findings_say_what_was_seen_and_whose() {
        let f = find("email isaac38.lehman@gmail.com about Lehman family reunion", &me());
        assert_eq!(f[0], Found { level: 4, what: "your email address".into() });
        assert!(f.iter().any(|x| x.what == "your last name"));
        // Name parts only as whole words: "Isaacson" isn't Isaac.
        assert!(find("Isaacson's laws of motion", &me()).is_empty());
        // Long ordinary words and plain numbers aren't keys.
        assert!(find("electroencephalographically internationalization 2026", &me()).is_empty());
        assert!(!find("token e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855", &me()).is_empty());
    }

    fn cx() -> Context {
        Context { me: me(), ..Default::default() }
    }

    #[test]
    fn a_search_shows_its_words_who_gets_them_and_the_level() {
        let r = review("web_search", &serde_json::json!({ "query": "hiking boots for Isaac Lehman" }), &cx()).unwrap();
        assert_eq!(r.to, "DuckDuckGo (search engine)");
        assert_eq!(r.items[0].key.as_deref(), Some("query"));
        assert_eq!(r.level, 4, "full name");
        assert_eq!(r.found[0].what, "your full name");
        assert!(r.could.iter().any(|c| c == SEARCH_COULD));
        // Plain interests are 3; a public lookup with nothing personal is 1.
        assert_eq!(review("web_search", &serde_json::json!({ "query": "rust borrow checker" }), &cx()).unwrap().level, 3);
        assert_eq!(review("market_quote", &serde_json::json!({ "symbols": ["AAPL", "BTC"] }), &cx()).unwrap().level, 1);
        // Weather where you are sends coordinates: an address's worth.
        let here = review("weather", &serde_json::json!({ "place": "" }), &Context { coords: true, ..cx() }).unwrap();
        assert_eq!(here.level, 5);
        assert_eq!(review("weather", &serde_json::json!({ "place": "Paris" }), &cx()).unwrap().level, 1);
    }

    #[test]
    fn calls_that_stay_on_this_pc_have_no_review() {
        assert!(review("read_file", &serde_json::json!({ "path": "a.txt" }), &cx()).is_none());
        assert!(review("open_path", &serde_json::json!({ "target": "C:/x/index.html" }), &cx()).is_none());
        assert!(review("open_path", &serde_json::json!({ "target": "https://example.com" }), &cx()).is_some());
    }

    #[test]
    fn edits_reword_and_leave_out_only_what_was_offered() {
        let args = serde_json::json!({ "question": "Isaac Lehman's best trails", "searches": ["trails Orem", "Isaac Lehman hiking", "Utah hikes"] });
        let r = review("research", &args, &cx()).unwrap();
        let edits = vec![
            Edit { key: "question".into(), value: Some("best trails".into()) },
            Edit { key: "searches.1".into(), value: None },
            Edit { key: "searches.0".into(), value: Some("trails Utah".into()) },
            // Not offered: ignored.
            Edit { key: "count".into(), value: Some("99".into()) },
        ];
        let out = apply(&args, &r, &edits);
        assert_eq!(out["question"], "best trails");
        assert_eq!(out["searches"], serde_json::json!(["trails Utah", "Utah hikes"]));
        assert!(out.get("count").is_none());
        // The required search words can be reworded but not left out.
        let s = serde_json::json!({ "query": "my address 12 Elm Street" });
        let r = review("web_search", &s, &cx()).unwrap();
        assert_eq!(apply(&s, &r, &[Edit { key: "query".into(), value: None }]), s);
    }

    #[test]
    fn a_profile_name_splits_into_first_and_last() {
        let m = Me::new("Mary Anne Smith", vec![], "");
        assert_eq!((m.first.as_deref(), m.last.as_deref()), (Some("Mary"), Some("Smith")));
        assert_eq!(Me::new("Prince", vec![], "").last, None);
        assert_eq!(Me::new("", vec![], "Orem, Utah").place.as_deref(), Some("Orem"));
    }
}
