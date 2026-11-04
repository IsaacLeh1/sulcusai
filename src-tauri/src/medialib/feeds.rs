// SPDX-License-Identifier: AGPL-3.0-only
//! RSS 2.0 and Atom feeds: podcasts (their audio files) and news for the
//! morning brief. Fetching goes through the app's usual network asks; this
//! only reads what came back.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub title: String,
    pub link: String,
    pub published: String,
    pub summary: String,
    /// The podcast episode's audio, when there is one.
    pub audio: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Feed {
    pub title: String,
    pub items: Vec<Item>,
}

pub(crate) fn entity(name: &str) -> String {
    match name {
        "amp" => "&".into(),
        "lt" => "<".into(),
        "gt" => ">".into(),
        "quot" => "\"".into(),
        "apos" => "'".into(),
        o => o
            .strip_prefix("#x")
            .and_then(|h| u32::from_str_radix(h, 16).ok())
            .or_else(|| o.strip_prefix('#').and_then(|d| d.parse().ok()))
            .and_then(char::from_u32)
            .map(String::from)
            .unwrap_or_default(),
    }
}

fn local(n: &[u8]) -> String {
    let s = String::from_utf8_lossy(n);
    s.rsplit(':').next().unwrap_or(&s).to_ascii_lowercase()
}

pub fn parse(xml: &str) -> Feed {
    use quick_xml::events::{BytesStart, Event as X};
    let mut r = quick_xml::Reader::from_str(xml);
    let mut feed = Feed::default();
    let mut stack: Vec<String> = Vec::new();
    let mut cur: Option<Item> = None;
    let mut text = String::new();
    let attr = |e: &BytesStart, key: &str| -> Option<String> {
        e.attributes().flatten().find(|a| local(a.key.as_ref()) == key).map(|a| String::from_utf8_lossy(&a.value).to_string())
    };
    let link_attr = |e: &BytesStart, cur: &mut Option<Item>| {
        let name = local(e.name().as_ref());
        let Some(it) = cur.as_mut() else { return };
        if name == "enclosure" && attr(e, "type").is_none_or(|t| t.starts_with("audio")) {
            it.audio = attr(e, "url");
        } else if name == "link" {
            if let Some(h) = attr(e, "href") {
                match attr(e, "rel").as_deref() {
                    Some("enclosure") => it.audio = Some(h),
                    None | Some("alternate") if it.link.is_empty() => it.link = h,
                    _ => {}
                }
            }
        }
    };
    loop {
        match r.read_event() {
            Ok(X::Start(e)) => {
                let name = local(e.name().as_ref());
                if name == "item" || name == "entry" {
                    cur = Some(Item::default());
                }
                link_attr(&e, &mut cur);
                stack.push(name);
                text.clear();
            }
            Ok(X::Empty(e)) => link_attr(&e, &mut cur),
            Ok(X::Text(t)) => text.push_str(&t.decode().map(|s| s.to_string()).unwrap_or_default()),
            Ok(X::CData(t)) => text.push_str(&String::from_utf8_lossy(&t)),
            Ok(X::GeneralRef(g)) => text.push_str(&entity(&g.decode().unwrap_or_default())),
            Ok(X::End(_)) => {
                let name = stack.pop().unwrap_or_default();
                let t = text.trim().to_string();
                text.clear();
                match cur.as_mut() {
                    Some(it) => match name.as_str() {
                        "title" => it.title = t,
                        "link" if !t.is_empty() => it.link = t,
                        "pubdate" | "published" | "updated" if it.published.is_empty() => it.published = t,
                        "description" | "summary" | "content" if it.summary.is_empty() => it.summary = plain(&t),
                        "item" | "entry" => feed.items.extend(cur.take()),
                        _ => {}
                    },
                    None if name == "title" && feed.title.is_empty() => feed.title = t,
                    None => {}
                }
            }
            Ok(X::Eof) | Err(_) => break,
            _ => {}
        }
    }
    feed
}

/// Descriptions often carry HTML; the brief wants words, kept short.
fn plain(s: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for c in s.chars() {
        match c {
            '<' => inside = true,
            '>' if inside => {
                inside = false;
                out.push(' ');
            }
            _ if !inside => out.push(c),
            _ => {}
        }
    }
    let joined = out.split_whitespace().collect::<Vec<_>>().join(" ");
    joined.chars().take(400).collect()
}

/// News for the brief: newest first across feeds, the same link once, at
/// most `n` items.
pub fn for_brief(feeds: &[Feed], n: usize) -> Vec<Item> {
    let mut all: Vec<(i64, Item)> = feeds.iter().flat_map(|f| f.items.iter().cloned()).map(|i| (when(&i.published), i)).collect();
    all.sort_by(|a, b| b.0.cmp(&a.0));
    let mut seen = std::collections::HashSet::new();
    all.into_iter().map(|(_, i)| i).filter(|i| seen.insert(if i.link.is_empty() { i.title.clone() } else { i.link.clone() })).take(n).collect()
}

fn when(s: &str) -> i64 {
    chrono::DateTime::parse_from_rfc2822(s).or_else(|_| chrono::DateTime::parse_from_rfc3339(s)).map(|d| d.timestamp()).unwrap_or(0)
}

// ---------- saved feeds ----------

const SAVED: &str = "media.feeds";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    pub url: String,
    pub title: String,
    /// A podcast when its items carry audio; news otherwise.
    pub podcast: bool,
}

pub fn saved(conn: &rusqlite::Connection) -> Vec<Saved> {
    crate::db::get(conn, SAVED).unwrap_or_default()
}

pub fn save(conn: &rusqlite::Connection, s: Saved) -> Result<Vec<Saved>, String> {
    let mut all = saved(conn);
    all.retain(|x| x.url != s.url);
    all.push(s);
    crate::db::set(conn, SAVED, &all)?;
    Ok(all)
}

pub fn forget(conn: &rusqlite::Connection, url: &str) -> Result<Vec<Saved>, String> {
    let mut all = saved(conn);
    all.retain(|x| x.url != url);
    crate::db::set(conn, SAVED, &all)?;
    Ok(all)
}

/// Only web addresses: a feed pointing at a file or the local network
/// could read what it shouldn't.
pub fn check_url(url: &str) -> Result<String, String> {
    let u = reqwest::Url::parse(url.trim()).map_err(|_| "That isn't a web address.".to_string())?;
    if !matches!(u.scheme(), "https" | "http") {
        return Err("Feeds have to be web addresses (https://...).".into());
    }
    let host = u.host_str().unwrap_or("");
    let local = host == "localhost" || host.ends_with(".local") || host.parse::<std::net::IpAddr>().is_ok_and(|ip| !crate::web::is_public(ip));
    if local {
        return Err("Feeds on this PC or the home network aren't read.".into());
    }
    Ok(u.to_string())
}

pub async fn fetch(client: &reqwest::Client, url: &str) -> Result<Feed, String> {
    let url = check_url(url)?;
    let resp = client.get(&url).timeout(std::time::Duration::from_secs(20)).send().await.map_err(|e| format!("Couldn't reach the feed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("The feed answered {}.", resp.status()));
    }
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err("That feed is too big to read.".into());
    }
    let feed = parse(&String::from_utf8_lossy(&bytes));
    if feed.title.is_empty() && feed.items.is_empty() {
        return Err("That address isn't a news or podcast feed.".into());
    }
    Ok(feed)
}

/// The news part of the brief: the newest items across the saved news
/// feeds, as short lines.
pub async fn brief_text(client: &reqwest::Client, subs: &[Saved], n: usize) -> String {
    let mut feeds = Vec::new();
    for s in subs.iter().filter(|s| !s.podcast).take(20) {
        if let Ok(f) = fetch(client, &s.url).await {
            feeds.push(Feed { title: s.title.clone(), items: f.items });
        }
    }
    let items = for_brief(&feeds, n);
    if items.is_empty() {
        return "No news from the saved feeds.".into();
    }
    items.iter().map(|i| format!("- {} ({}){}", i.title, i.link, if i.summary.is_empty() { String::new() } else { format!(": {}", i.summary.chars().take(200).collect::<String>()) })).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_feeds_and_addresses() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        c.execute("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)", []).unwrap();
        save(&c, Saved { url: "https://a.org/rss".into(), title: "A".into(), podcast: false }).unwrap();
        save(&c, Saved { url: "https://a.org/rss".into(), title: "A2".into(), podcast: true }).unwrap();
        assert_eq!(saved(&c).len(), 1);
        assert!(saved(&c)[0].podcast);
        assert!(forget(&c, "https://a.org/rss").unwrap().is_empty());
        assert!(check_url("file:///C:/x").is_err());
        assert!(check_url("http://192.168.1.2/feed").is_err());
        assert!(check_url("http://localhost/feed").is_err());
        assert!(check_url("https://news.org/feed").is_ok());
    }

    const RSS: &str = r#"<?xml version="1.0"?><rss version="2.0"><channel><title>Talk &amp; Tea</title>
<item><title>Ep 1</title><link>https://ex.org/1</link><pubDate>Mon, 02 Nov 2026 08:00:00 +0000</pubDate>
<description><![CDATA[<p>First <b>one</b></p>]]></description>
<enclosure url="https://ex.org/1.mp3" type="audio/mpeg" length="1"/></item>
<item><title>Ep 2</title><link>https://ex.org/2</link><pubDate>Tue, 03 Nov 2026 08:00:00 +0000</pubDate></item>
</channel></rss>"#;

    const ATOM: &str = r#"<feed xmlns="http://www.w3.org/2005/Atom"><title>News</title>
<entry><title>Big &lt;day&gt;</title><link href="https://n.org/a"/><updated>2026-11-04T09:00:00Z</updated><summary>Text</summary></entry>
<entry><title>Dup</title><link rel="alternate" href="https://ex.org/1"/><updated>2026-10-01T09:00:00Z</updated></entry></feed>"#;

    #[test]
    fn reads_rss_podcasts() {
        let f = parse(RSS);
        assert_eq!(f.title, "Talk & Tea");
        assert_eq!(f.items.len(), 2);
        assert_eq!(f.items[0].audio.as_deref(), Some("https://ex.org/1.mp3"));
        assert_eq!(f.items[0].summary, "First one");
        assert_eq!(f.items[1].audio, None);
    }

    #[test]
    fn reads_atom_and_builds_brief() {
        let a = parse(ATOM);
        assert_eq!(a.title, "News");
        assert_eq!(a.items[0].title, "Big <day>");
        assert_eq!(a.items[0].link, "https://n.org/a");
        let b = for_brief(&[parse(RSS), a], 10);
        let titles: Vec<_> = b.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, vec!["Big <day>", "Ep 2", "Ep 1"]);
        assert_eq!(for_brief(&[parse(RSS)], 1).len(), 1);
    }
}
