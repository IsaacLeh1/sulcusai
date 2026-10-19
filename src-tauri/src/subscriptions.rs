// SPDX-License-Identifier: AGPL-3.0-only
//! Subscriptions: the mailing lists and bulk senders in the inbox (from
//! their List-Unsubscribe and List-Id headers), and leaving them.
//!
//! Unsubscribing is done the most automatic way the list allows:
//! - One-click (RFC 8058): a POST to the list's https address, with the web
//!   tools' guards (public addresses only, no redirects into this PC or the
//!   local network, the private way when that's on).
//! - Else the unsubscribe email the list asks for, from the account it
//!   writes to. Sending goes through the user: it's sent only when they
//!   click Unsubscribe, and the window shows its address and subject first.
//! - Else the list's unsubscribe page, opened in the user's browser.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::features::Feature;
use crate::mail::{self, Draft, MailData, MailItem};
use crate::net::{self, Purpose};
use crate::{db, AppState, AppStateRef};

/// Lists left, by key (setting).
const KEY: &str = "mail_unsubscribed";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Unsubscribed {
    pub at: i64,
    /// "one_click", "email" or "link" (the page was opened).
    pub method: String,
}

/// How a list can be left.
#[derive(Debug, Clone, PartialEq)]
pub enum Method {
    OneClick(String),
    Email(Draft),
    Link(String),
    None,
}

impl Method {
    fn code(&self) -> &'static str {
        match self {
            Method::OneClick(_) => "one_click",
            Method::Email(_) => "email",
            Method::Link(_) => "link",
            Method::None => "none",
        }
    }
}

fn https(d: &MailData) -> Option<String> {
    d.unsubscribe.iter().find(|u| u.to_ascii_lowercase().starts_with("https://")).cloned()
}

/// The most automatic way out a message's headers offer.
pub fn method(d: &MailData) -> Method {
    let link = https(d);
    if d.one_click {
        if let Some(u) = &link {
            return Method::OneClick(u.clone());
        }
    }
    if let Some(draft) = d.unsubscribe.iter().find_map(|u| mailto_draft(u)) {
        return Method::Email(draft);
    }
    link.map(Method::Link).unwrap_or(Method::None)
}

/// Header text from an address: no line breaks or other control characters.
fn one_line(s: &str, max: usize) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ").chars().take(max).collect()
}

/// The email a mailto: unsubscribe address asks for (RFC 6068): its
/// addresses, and its subject and body (else "unsubscribe"), %-decoded.
pub fn mailto_draft(url: &str) -> Option<Draft> {
    let rest = url.get(7..).filter(|_| url.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("mailto:")))?;
    let (addrs, query) = rest.split_once('?').unwrap_or((rest, ""));
    let address = |a: &str| Some(one_line(&net::percent_decode(a), 320)).filter(|a| a.contains('@') && !a.contains(' '));
    let mut to: Vec<String> = addrs.split(',').filter_map(address).collect();
    let (mut subject, mut body) = (String::new(), String::new());
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        match k.to_ascii_lowercase().as_str() {
            "subject" => subject = one_line(&net::percent_decode(v), 200),
            "body" => body = net::percent_decode(v).replace("\r\n", "\n").trim().chars().take(2000).collect(),
            "to" => to.extend(net::percent_decode(v).split(',').filter_map(address)),
            _ => {}
        }
    }
    to.dedup();
    to.truncate(3);
    if to.is_empty() {
        return None;
    }
    let or_unsubscribe = |s: String| if s.trim().is_empty() { "unsubscribe".to_string() } else { s };
    Some(Draft { to, subject: or_unsubscribe(subject), body: or_unsubscribe(body), ..Default::default() })
}

/// Which subscription a message belongs to: its account and list (List-Id),
/// else its sender. Mail without list headers isn't from a subscription.
pub fn key_of(m: &MailItem) -> Option<String> {
    let d = &m.data;
    if d.unsubscribe.is_empty() && d.list_id.is_empty() {
        return None;
    }
    let who = if d.list_id.is_empty() { d.from.trim().to_lowercase() } else { d.list_id.clone() };
    (!who.is_empty()).then(|| format!("{}|{who}", m.account_id))
}

/// How to leave a subscription, as the window shows it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Unsub {
    pub key: String,
    /// "one_click", "email", "link" or "none".
    pub method: String,
    /// For "email": where it goes and its subject.
    pub email_to: String,
    pub email_subject: String,
    pub unsubscribed: Option<Unsubscribed>,
}

/// The way out for one message's subscription (None when it isn't one).
pub fn unsub_of(m: &MailItem, done: &HashMap<String, Unsubscribed>) -> Option<Unsub> {
    let key = key_of(m)?;
    let how = method(&m.data);
    let (email_to, email_subject) = match &how {
        Method::Email(d) => (d.to.join(", "), d.subject.clone()),
        _ => Default::default(),
    };
    Some(Unsub { unsubscribed: done.get(&key).cloned(), key, method: how.code().into(), email_to, email_subject })
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Subscription {
    pub account_id: String,
    pub name: String,
    pub address: String,
    pub list_id: String,
    pub count: usize,
    pub latest: i64,
    /// Its mail still in the inbox (to archive after leaving).
    pub inbox_ids: Vec<String>,
    /// When the user last opened one of its emails here.
    pub opened_at: Option<i64>,
    /// A list the user never opens: how many of its latest emails in a row
    /// went unopened (0 when it isn't one; see `never_opened`).
    pub unopened: usize,
    #[serde(flatten)]
    pub how: Unsub,
}

/// Opened within this long: still read.
const READ_WITHIN_MS: i64 = 60 * 86_400_000;
/// Unopened emails in a row that make a list one the user never opens.
const UNOPENED_AT_LEAST: usize = 3;
/// When SulcusAI started noting which emails are opened (setting).
const OPENS_SINCE_KEY: &str = "mail_opens_since";

/// When opens started being noted: mail from before can't count as unread
/// (the user may have read it before, or elsewhere). Set the first time.
pub fn opens_since(conn: &rusqlite::Connection) -> i64 {
    db::get::<i64>(conn, OPENS_SINCE_KEY).unwrap_or_else(|| {
        let now = db::now_ms();
        db::set(conn, OPENS_SINCE_KEY, &now).ok();
        now
    })
}

/// A list the user never opens, from its emails' (date, opened_at): none
/// opened in the last 60 days, and at least 3 received unopened since the
/// newest one opened (counting mail since opens were noted, `since`).
/// Returns how many in a row went unopened, else 0.
pub fn never_opened(mail: &[(i64, Option<i64>)], now: i64, since: i64) -> usize {
    if mail.iter().any(|(_, o)| o.is_some_and(|o| o >= now - READ_WITHIN_MS)) {
        return 0;
    }
    // Emails after the newest one opened.
    let after = mail.iter().filter(|(_, o)| o.is_some()).map(|(d, _)| *d).max().unwrap_or(i64::MIN);
    let n = mail.iter().filter(|(d, o)| o.is_none() && *d > after && *d >= since).count();
    if n >= UNOPENED_AT_LEAST { n } else { 0 }
}

/// Inbox (and archived) mail grouped by list or sender: the ones still
/// subscribed first, those never opened first among them, then the most
/// mail. `since`: when opens started being noted.
pub fn group(items: &[MailItem], done: &HashMap<String, Unsubscribed>, now: i64, since: i64) -> Vec<Subscription> {
    let mut sorted: Vec<&MailItem> = items.iter().filter(|m| m.folder.eq_ignore_ascii_case("INBOX") || m.folder == mail::ARCHIVE).collect();
    sorted.sort_by(|a, b| b.date.cmp(&a.date));
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Subscription> = HashMap::new();
    let mut opens: HashMap<String, Vec<(i64, Option<i64>)>> = HashMap::new();
    for m in sorted {
        let Some(key) = key_of(m) else { continue };
        let d = &m.data;
        let g = groups.entry(key.clone()).or_insert_with(|| {
            order.push(key.clone());
            let name = [&d.from_name, &d.list_name, &d.from].into_iter().map(|s| s.trim()).find(|s| !s.is_empty()).unwrap_or("").to_string();
            let how = Unsub { key: key.clone(), method: "none".into(), email_to: String::new(), email_subject: String::new(), unsubscribed: done.get(&key).cloned() };
            Subscription { account_id: m.account_id.clone(), name, address: d.from.clone(), list_id: d.list_id.clone(), count: 0, latest: m.date, inbox_ids: Vec::new(), opened_at: None, unopened: 0, how }
        });
        g.count += 1;
        g.opened_at = g.opened_at.max(m.opened_at);
        opens.entry(key.clone()).or_default().push((m.date, m.opened_at));
        if m.folder.eq_ignore_ascii_case("INBOX") {
            g.inbox_ids.push(m.id.clone());
        }
        // The newest message that says how to leave decides how.
        if g.how.method == "none" && !d.unsubscribe.is_empty() {
            if let Some(u) = unsub_of(m, done) {
                g.how = u;
            }
        }
    }
    let mut out: Vec<Subscription> = order.into_iter().filter_map(|k| groups.remove(&k)).collect();
    for s in &mut out {
        if s.how.unsubscribed.is_none() {
            s.unopened = never_opened(opens.get(&s.how.key).map(Vec::as_slice).unwrap_or(&[]), now, since);
        }
    }
    out.sort_by(|a, b| {
        a.how.unsubscribed.is_some().cmp(&b.how.unsubscribed.is_some()).then((b.unopened > 0).cmp(&(a.unopened > 0))).then(b.count.cmp(&a.count)).then(b.latest.cmp(&a.latest))
    });
    out
}

pub fn done(conn: &rusqlite::Connection) -> HashMap<String, Unsubscribed> {
    db::get(conn, KEY).unwrap_or_default()
}

fn record(conn: &rusqlite::Connection, key: &str, method: &str) -> Result<Unsubscribed, String> {
    let mut all = done(conn);
    let u = Unsubscribed { at: db::now_ms(), method: method.into() };
    all.insert(key.to_string(), u.clone());
    db::set(conn, KEY, &all)?;
    Ok(u)
}

/// RFC 8058: a POST of "List-Unsubscribe=One-Click" to the list's address.
/// The address comes from someone else's email: only public https
/// addresses, and no redirects to this PC or the local network.
async fn one_click(state: &AppState, url: &str) -> Result<(), String> {
    net::external_client(state.settings().connectivity, Purpose::Web, false)?;
    let url = crate::web::check_url(url).await?;
    if url.scheme() != "https" {
        return Err("The list's unsubscribe address isn't secure (https).".into());
    }
    let b = crate::web::builder()?;
    let b = if crate::privacy::proxy_for(net::Route::Web).is_some() { b } else { b.dns_resolver(Arc::new(crate::web::PublicOnly)) };
    let client = b.build().map_err(|e| e.to_string())?;
    let r = client
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body("List-Unsubscribe=One-Click")
        .send()
        .await
        .map_err(|e| format!("Couldn't reach the list's server: {e}"))?;
    if r.status().is_success() {
        Ok(())
    } else {
        Err(format!("The list's server answered {}.", r.status().as_u16()))
    }
}

// ---------- commands ----------

#[tauri::command(async)]
pub fn mail_subscriptions(state: AppStateRef) -> Result<Vec<Subscription>, String> {
    crate::features::require(&state, Feature::Email)?;
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    let items = mail::list(&conn, &c, "", usize::MAX);
    Ok(group(&items, &done(&conn), db::now_ms(), opens_since(&conn)))
}

#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    pub key: String,
    /// "one_click", "email" or "link".
    pub method: String,
    pub at: i64,
    /// For "link": the page to open in the browser.
    pub url: String,
    /// For "link" after a one-click request failed: why.
    pub problem: String,
    /// For "email": where it went and its subject.
    pub to: String,
    pub subject: String,
}

/// Leaves a subscription (the user clicked Unsubscribe): one-click, else
/// the unsubscribe email, else the page for the browser (returned in `url`).
#[tauri::command]
pub async fn mail_unsubscribe(state: AppStateRef<'_>, key: String) -> Result<Outcome, String> {
    crate::features::require(&state, Feature::Email)?;
    let c = state.cipher()?;
    let newest: Option<MailItem> = {
        let conn = state.db.lock().unwrap();
        mail::list(&conn, &c, "", usize::MAX)
            .into_iter()
            .filter(|m| m.folder.eq_ignore_ascii_case("INBOX") || m.folder == mail::ARCHIVE)
            .find(|m| !m.data.unsubscribe.is_empty() && key_of(m).as_deref() == Some(key.as_str()))
    };
    let m = newest.ok_or("This sender doesn't say how to unsubscribe. Archive its mail instead, or block the sender in your mail account.")?;
    let mut out = Outcome { key: key.clone(), method: String::new(), at: 0, url: String::new(), problem: String::new(), to: String::new(), subject: String::new() };
    match method(&m.data) {
        Method::OneClick(url) => match one_click(&state, &url).await {
            Ok(()) => out.method = "one_click".into(),
            // Not from here: the list's page, in the browser.
            Err(e) => {
                out.method = "link".into();
                out.url = url;
                out.problem = e;
            }
        },
        Method::Email(d) => {
            mail::send(&state, &c, &m.account_id, &d).await?;
            out.method = "email".into();
            out.to = d.to.join(", ");
            out.subject = d.subject;
        }
        Method::Link(url) => {
            out.method = "link".into();
            out.url = url;
        }
        Method::None => return Err("This sender doesn't say how to unsubscribe. Archive its mail instead, or block the sender in your mail account.".into()),
    }
    let conn = state.db.lock().unwrap();
    out.at = record(&conn, &key, &out.method)?.at;
    let who = if m.data.from_name.is_empty() { m.data.from.clone() } else { m.data.from_name.clone() };
    let what = match out.method.as_str() {
        "link" => format!("Opened the unsubscribe page for {who}"),
        _ => format!("Unsubscribed from {who}"),
    };
    db::log_action_enc(&conn, &c, "email", &what);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, account: &str, folder: &str, date: i64, d: MailData) -> MailItem {
        MailItem { id: id.into(), account_id: account.into(), folder: folder.into(), date, seen: false, marked_at: None, opened_at: None, data: d }
    }

    #[test]
    fn mailto_unsubscribe_is_read_and_decoded() {
        let d = mailto_draft("mailto:leave-123@lists.example.com?subject=Unsubscribe%20me%2C%20please&body=remove%20ana%2Btag%40example.org").unwrap();
        assert_eq!(d.to, vec!["leave-123@lists.example.com"]);
        assert_eq!(d.subject, "Unsubscribe me, please");
        assert_eq!(d.body, "remove ana+tag@example.org", "%2B is a plus, and a plain + stays one");
        assert!(d.reply_to.is_none() && d.cc.is_empty());
        // Nothing given: "unsubscribe" for both.
        let bare = mailto_draft("MAILTO:off%40news.example").unwrap();
        assert_eq!((bare.to, bare.subject.as_str(), bare.body.as_str()), (vec!["off@news.example".to_string()], "unsubscribe", "unsubscribe"));
        // A subject can't add header lines.
        let sneaky = mailto_draft("mailto:a@b.example?subject=hi%0D%0ABcc:%20x@evil.example").unwrap();
        assert!(!sneaky.subject.contains('\n') && !sneaky.subject.contains('\r'), "{}", sneaky.subject);
        assert!(mailto_draft("mailto:?subject=x").is_none(), "no address");
        assert!(mailto_draft("https://example.com/u").is_none());
    }

    #[test]
    fn the_most_automatic_way_out_is_chosen() {
        let both = MailData { unsubscribe: vec!["mailto:u@l.example?subject=stop".into(), "https://l.example/u/1".into()], one_click: true, ..Default::default() };
        assert_eq!(method(&both), Method::OneClick("https://l.example/u/1".into()));
        let no_post = MailData { one_click: false, ..both.clone() };
        assert!(matches!(method(&no_post), Method::Email(d) if d.subject == "stop"));
        let link = MailData { unsubscribe: vec!["https://l.example/u/1".into()], ..Default::default() };
        assert_eq!(method(&link), Method::Link("https://l.example/u/1".into()));
        assert_eq!(method(&MailData::default()), Method::None);
    }

    #[test]
    fn subscriptions_group_by_list_then_sender() {
        let news = |subject: &str| MailData {
            from: "news@paper.example".into(),
            from_name: "The Paper".into(),
            subject: subject.into(),
            unsubscribe: vec!["https://paper.example/u".into()],
            one_click: true,
            list_id: "morning.paper.example".into(),
            list_name: "Morning".into(),
            list_checked: true,
            ..Default::default()
        };
        let shop = MailData { from: "Deals@Shop.example".into(), unsubscribe: vec!["mailto:out@shop.example".into()], list_checked: true, ..Default::default() };
        let person = MailData { from: "ana@example.com".into(), from_name: "Ana".into(), list_checked: true, ..Default::default() };
        let items = vec![
            item("n1", "acc", "INBOX", 100, news("Monday")),
            item("n2", "acc", "INBOX", 300, news("Wednesday")),
            item("n3", "acc", mail::ARCHIVE, 200, news("Tuesday")),
            item("s1", "acc", "INBOX", 250, shop.clone()),
            item("s2", "acc", "INBOX", 50, MailData { from: "deals@shop.example".into(), ..shop.clone() }),
            item("p1", "acc", "INBOX", 400, person),
            // The user's own sent mail isn't a subscription.
            item("x1", "acc", "Sent", 500, news("Sent copy")),
            // Another account gets the same list: its own subscription.
            item("o1", "other", "INBOX", 120, news("Monday")),
        ];
        let mut done = HashMap::new();
        // Opens noted only after all of this mail: none of it counts as unread.
        let group = |items: &[MailItem], done: &HashMap<String, Unsubscribed>| group(items, done, 10_000, 1_000);
        let subs = group(&items, &done);
        assert_eq!(subs.len(), 3, "{subs:#?}");
        assert!(subs.iter().all(|s| s.unopened == 0));
        let paper = &subs[0];
        assert_eq!((paper.name.as_str(), paper.count, paper.latest, paper.account_id.as_str()), ("The Paper", 3, 300, "acc"));
        assert_eq!(paper.inbox_ids, vec!["n2", "n1"], "archived mail counts but isn't archived again");
        assert_eq!((paper.how.key.as_str(), paper.how.method.as_str()), ("acc|morning.paper.example", "one_click"));
        let deals = &subs[1];
        assert_eq!((deals.count, deals.name.as_str(), deals.how.method.as_str(), deals.how.email_to.as_str(), deals.how.email_subject.as_str()), (2, "Deals@Shop.example", "email", "out@shop.example", "unsubscribe"));
        assert_eq!(subs[2].how.key, "other|morning.paper.example");
        // Left lists go last, with when.
        done.insert("acc|morning.paper.example".to_string(), Unsubscribed { at: 999, method: "one_click".into() });
        let subs = group(&items, &done);
        assert_eq!(subs.last().unwrap().how.key, "acc|morning.paper.example");
        assert_eq!(subs.last().unwrap().how.unsubscribed.as_ref().unwrap().at, 999);
    }

    #[test]
    fn lists_never_opened_are_60_days_unopened_with_at_least_3() {
        const DAY: i64 = 86_400_000;
        let now = 400 * DAY;
        let since = 100 * DAY;
        let days_ago = |n: i64| now - n * DAY;
        // Three since opens were noted, none opened: never opened.
        assert_eq!(never_opened(&[(days_ago(3), None), (days_ago(2), None), (days_ago(1), None)], now, since), 3);
        // Only two: not enough to say.
        assert_eq!(never_opened(&[(days_ago(2), None), (days_ago(1), None)], now, since), 0);
        // One opened in the last 60 days: still read.
        assert_eq!(never_opened(&[(days_ago(70), Some(days_ago(59))), (days_ago(3), None), (days_ago(2), None), (days_ago(1), None)], now, since), 0);
        // Last opened 61 days ago: the ones after it count, not the one opened.
        let old_open = [(days_ago(90), Some(days_ago(61))), (days_ago(30), None), (days_ago(20), None), (days_ago(10), None), (days_ago(5), None)];
        assert_eq!(never_opened(&old_open, now, since), 4);
        // Mail from before opens were noted doesn't count (it may have been read).
        let before = [(since - DAY, None), (since - 2 * DAY, None), (days_ago(1), None), (days_ago(2), None)];
        assert_eq!(never_opened(&before, now, since), 0);

        // In the list: shown first among those still subscribed, with the count.
        let news = |n: i64| MailData { from: "news@paper.example".into(), list_id: "paper.example".into(), unsubscribe: vec!["https://paper.example/u".into()], subject: format!("Issue {n}"), ..Default::default() };
        let shop = MailData { from: "deals@shop.example".into(), unsubscribe: vec!["https://shop.example/u".into()], ..Default::default() };
        let mut items: Vec<MailItem> = (1..=3).map(|n| item(&format!("n{n}"), "acc", "INBOX", days_ago(n), news(n))).collect();
        for n in 1..=5 {
            let mut m = item(&format!("s{n}"), "acc", "INBOX", days_ago(n), shop.clone());
            if n == 2 {
                m.opened_at = Some(days_ago(1));
            }
            items.push(m);
        }
        let subs = group(&items, &HashMap::new(), now, since);
        assert_eq!((subs[0].name.as_str(), subs[0].unopened), ("news@paper.example", 3), "{subs:#?}");
        assert_eq!((subs[1].count, subs[1].unopened, subs[1].opened_at), (5, 0, Some(days_ago(1))), "opened lately, though it sends more");
    }
}
