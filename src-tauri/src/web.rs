// SPDX-License-Identifier: AGPL-3.0-only
//! Web search and page reading, for chats where web access is on (the Local
//! AI + Web level, or the 🌐 toggle on one chat). The AI still runs here;
//! only the search query and page requests go out.
//!
//! Pages are untrusted: their text goes to the model marked as information,
//! never instructions, and fetches refuse local and private addresses so a
//! page can't steer the assistant into probing this PC or the home network.

use std::net::IpAddr;
use std::time::Duration;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::crypto::Cipher;
use crate::db;
use crate::net::{self, Connectivity, Purpose};
use crate::AppStateRef;

/// Most page text handed to the model.
const PAGE_CHARS: usize = 24_000;
/// Biggest page downloaded.
const PAGE_BYTES: usize = 3 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    #[default]
    Duckduckgo,
    Brave,
    Searxng,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WebSettings {
    pub provider: Provider,
    pub searxng_url: String,
}

pub fn settings(conn: &Connection) -> WebSettings {
    db::get(conn, "web").unwrap_or_default()
}

fn brave_key(conn: &Connection, c: &Cipher) -> Option<String> {
    db::get::<String>(conn, "web_brave_key").and_then(|k| c.decrypt(&k).ok()).filter(|k| !k.is_empty())
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Hit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

// ---------- address checks ----------

pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let o = v.octets();
            !(v.is_loopback()
                || v.is_private()
                || v.is_link_local()
                || v.is_unspecified()
                || v.is_multicast()
                || v.is_broadcast()
                || v.is_documentation()
                || o[0] == 0
                || (o[0] == 100 && (64..128).contains(&o[1])) // carrier-grade NAT
                || (o[0] == 198 && (o[1] == 18 || o[1] == 19)))
        }
        IpAddr::V6(v) => {
            if let Some(v4) = v.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let s = v.segments();
            !(v.is_loopback() || v.is_unspecified() || v.is_multicast() || (s[0] & 0xfe00) == 0xfc00 || (s[0] & 0xffc0) == 0xfe80)
        }
    }
}

/// Refuses anything but public http(s) addresses.
pub async fn check_url(raw: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(raw.trim()).map_err(|_| format!("That isn't a web address: {raw}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("Only http and https addresses can be opened.".into());
    }
    let host = url.host_str().ok_or("That address has no host.")?.trim_end_matches('.').to_lowercase();
    if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") || host.ends_with(".internal") || !host.contains('.') && host.parse::<IpAddr>().is_err() {
        return Err("Addresses on this PC or the local network can't be opened from the web tools.".into());
    }
    let port = url.port_or_known_default().unwrap_or(443);
    let addrs: Vec<IpAddr> = match host.trim_matches(['[', ']']).parse::<IpAddr>() {
        Ok(ip) => vec![ip],
        Err(_) => tokio::net::lookup_host((host.as_str(), port)).await.map_err(|_| format!("Couldn't find {host}."))?.map(|a| a.ip()).collect(),
    };
    if addrs.is_empty() || !addrs.iter().all(|ip| is_public(*ip)) {
        return Err("Addresses on this PC or the local network can't be opened from the web tools.".into());
    }
    Ok(url)
}

fn client(level: Connectivity, chat_web: bool) -> Result<reqwest::Client, String> {
    net::external_client(level, Purpose::Search, chat_web)?;
    reqwest::Client::builder()
        // A browser-like agent; some sites refuse unknown ones.
        .user_agent(concat!("Mozilla/5.0 (Windows NT 10.0; Win64; x64) SulcusAI/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 6 {
                return attempt.error("too many redirects");
            }
            let local = attempt.url().host_str().is_none_or(|h| {
                h == "localhost" || h.ends_with(".local") || h.trim_matches(['[', ']']).parse::<IpAddr>().is_ok_and(|ip| !is_public(ip))
            });
            if local { attempt.error("redirect to a local address") } else { attempt.follow() }
        }))
        .build()
        .map_err(|e| e.to_string())
}

// ---------- search ----------

fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&").replace("&quot;", "\"").replace("&#x27;", "'").replace("&#39;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&nbsp;", " ")
}

fn strip_tags(s: &str) -> String {
    let re = regex::Regex::new(r"<[^>]+>").unwrap();
    decode_entities(&re.replace_all(s, "")).split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Results from DuckDuckGo's plain HTML page.
pub fn parse_duckduckgo(html: &str) -> Vec<Hit> {
    let link = regex::Regex::new(r#"(?s)<a[^>]*class="result__a"[^>]*href="([^"]+)"[^>]*>(.*?)</a>"#).unwrap();
    let snippet = regex::Regex::new(r#"(?s)<a[^>]*class="result__snippet"[^>]*>(.*?)</a>"#).unwrap();
    let snippets: Vec<String> = snippet.captures_iter(html).map(|c| strip_tags(&c[1])).collect();
    let mut out = Vec::new();
    for (i, c) in link.captures_iter(html).enumerate() {
        let href = decode_entities(&c[1]);
        // Links go through a redirect: //duckduckgo.com/l/?uddg=<real url>&rut=…
        let real = reqwest::Url::parse(&if href.starts_with("//") { format!("https:{href}") } else { href.clone() })
            .ok()
            .and_then(|u| u.query_pairs().find(|(k, _)| k == "uddg").map(|(_, v)| v.to_string()))
            .unwrap_or(href);
        if real.contains("duckduckgo.com/y.js") || !real.starts_with("http") {
            continue; // ads
        }
        out.push(Hit { title: strip_tags(&c[2]), url: real, snippet: snippets.get(i).cloned().unwrap_or_default() });
    }
    out
}

pub async fn search(conn_settings: &WebSettings, key: Option<String>, client: &reqwest::Client, query: &str, count: usize) -> Result<Vec<Hit>, String> {
    let fail = |e: reqwest::Error| format!("The search didn't work: {e}");
    let hits = match conn_settings.provider {
        Provider::Duckduckgo => {
            let html = client
                .post("https://html.duckduckgo.com/html/")
                .form(&[("q", query), ("kl", "us-en")])
                .send()
                .await
                .map_err(fail)?
                .text()
                .await
                .map_err(fail)?;
            let hits = parse_duckduckgo(&html);
            if hits.is_empty() && html.contains("anomaly") {
                return Err("DuckDuckGo is limiting searches from this PC right now. Try again in a minute, or use Brave or SearXNG in Settings.".into());
            }
            hits
        }
        Provider::Brave => {
            let key = key.ok_or("Add your Brave Search API key in Settings › Connectivity.")?;
            let v: Value = client
                .get("https://api.search.brave.com/res/v1/web/search")
                .query(&[("q", query), ("count", &count.to_string())])
                .header("X-Subscription-Token", key)
                .header("Accept", "application/json")
                .send()
                .await
                .map_err(fail)?
                .error_for_status()
                .map_err(fail)?
                .json()
                .await
                .map_err(fail)?;
            v.pointer("/web/results")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|r| Hit {
                            title: strip_tags(r["title"].as_str().unwrap_or("")),
                            url: r["url"].as_str().unwrap_or("").into(),
                            snippet: strip_tags(r["description"].as_str().unwrap_or("")),
                        })
                        .collect()
                })
                .unwrap_or_default()
        }
        Provider::Searxng => {
            let base = conn_settings.searxng_url.trim().trim_end_matches('/');
            if base.is_empty() {
                return Err("Add your SearXNG address in Settings › Connectivity.".into());
            }
            let v: Value = client
                .get(format!("{base}/search"))
                .query(&[("q", query), ("format", "json")])
                .send()
                .await
                .map_err(fail)?
                .error_for_status()
                .map_err(|e| format!("SearXNG refused the search ({e}). Its JSON output must be turned on."))?
                .json()
                .await
                .map_err(fail)?;
            v["results"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|r| Hit {
                            title: r["title"].as_str().unwrap_or("").into(),
                            url: r["url"].as_str().unwrap_or("").into(),
                            snippet: r["content"].as_str().unwrap_or("").into(),
                        })
                        .collect()
                })
                .unwrap_or_default()
        }
    };
    Ok(hits.into_iter().filter(|h| !h.url.is_empty()).take(count).collect())
}

pub fn provider_name(p: Provider) -> &'static str {
    match p {
        Provider::Duckduckgo => "DuckDuckGo",
        Provider::Brave => "Brave Search",
        Provider::Searxng => "SearXNG",
    }
}

// ---------- pages ----------

#[derive(Debug, Clone, Serialize)]
pub struct Page {
    pub url: String,
    pub title: String,
    pub text: String,
    pub truncated: bool,
}

/// Readable text from HTML: scripts, styles and page chrome dropped.
pub fn html_to_text(html: &str) -> (String, String) {
    let title = regex::Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap().captures(html).map(|c| strip_tags(&c[1])).unwrap_or_default();
    let mut cleaned = html.to_string();
    for tag in ["script", "style", "noscript", "svg", "nav", "footer", "header", "form", "iframe"] {
        let re = regex::Regex::new(&format!(r"(?is)<{tag}\b.*?</{tag}\s*>")).unwrap();
        cleaned = re.replace_all(&cleaned, " ").into_owned();
    }
    let text = html2text::from_read(cleaned.as_bytes(), 100).unwrap_or_default();
    // Collapse runs of blank lines.
    let mut out = String::new();
    let mut blank = 0;
    for line in text.lines() {
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    (title, out.trim().to_string())
}

pub async fn fetch(client: &reqwest::Client, raw: &str) -> Result<Page, String> {
    let url = check_url(raw).await?;
    let resp = client.get(url.clone()).send().await.map_err(|e| format!("Couldn't open the page: {e}"))?;
    let status = resp.status();
    let final_url = resp.url().to_string();
    if !status.is_success() {
        return Err(format!("The site answered {status}."));
    }
    let kind = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_lowercase();
    if !(kind.contains("html") || kind.contains("text") || kind.contains("json") || kind.contains("xml") || kind.is_empty()) {
        return Err(format!("That page isn't text (it's {kind}). Reading PDFs and other files comes with the documents features."));
    }
    let mut body = Vec::new();
    let mut stream = resp.bytes_stream();
    use futures_util::StreamExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("The page stopped loading: {e}"))?;
        body.extend_from_slice(&chunk);
        if body.len() > PAGE_BYTES {
            break;
        }
    }
    let raw_text = String::from_utf8_lossy(&body).to_string();
    let (title, mut text) = if kind.contains("html") || raw_text.trim_start().starts_with('<') {
        html_to_text(&raw_text)
    } else {
        (String::new(), raw_text)
    };
    let truncated = text.chars().count() > PAGE_CHARS;
    if truncated {
        text = text.chars().take(PAGE_CHARS).collect();
    }
    Ok(Page { url: final_url, title, text, truncated })
}

/// The web settings a chat's tools use, and whether web is allowed for it.
/// The level, and whether search is on for this chat anyway: its globe, or
/// the "Web search" feature (search and reading pages in every chat, even
/// while Offline).
pub fn chat_access(conn: &Connection, chat_id: &str) -> (Connectivity, bool) {
    let level = db::settings(conn).connectivity;
    let chat_web: bool = conn.query_row("SELECT web FROM chats WHERE id = ?1", [chat_id], |r| r.get::<_, i64>(0)).map(|v| v != 0).unwrap_or(false);
    (level, chat_web || crate::features::is_on(conn, crate::features::Feature::WebSearch))
}

pub fn allowed_for_chat(conn: &Connection, chat_id: &str) -> bool {
    let (level, search_on) = chat_access(conn, chat_id);
    net::allowed(level, Purpose::Search, search_on)
}

/// For the tools: a client, the settings and the Brave key, if web is allowed.
pub fn prepare(conn: &Connection, c: &Cipher, chat_id: &str) -> Result<(reqwest::Client, WebSettings, Option<String>), String> {
    let (level, chat_web) = chat_access(conn, chat_id);
    Ok((client(level, chat_web)?, settings(conn), brave_key(conn, c)))
}

// ---------- excerpts ----------

/// The lines of a page that bear on a question: they share its words, or
/// carry figures (numbers with units). Small models answer far better from
/// these than from search snippets alone.
pub fn excerpts(text: &str, query: &str, max_chars: usize) -> String {
    const STOP: &[&str] = &["the", "and", "for", "what", "whats", "how", "is", "are", "today", "now", "current", "in", "of", "a", "to", "me", "my", "with", "does", "do", "on", "at", "this", "that", "give", "tell", "exact"];
    let words: Vec<String> = query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2 && !STOP.contains(w))
        .map(str::to_string)
        .collect();
    let figure = regex::Regex::new(r"(?i)\d\s*(°|º|%|\$|€|£|mph|km/?h|degrees|deg\b|f\b|c\b|in\b|mm\b|cm\b|kg\b|lbs?\b|million|billion|pm\b|am\b)|[$€£]\s*\d").unwrap();
    let mut scored: Vec<(usize, i32, String)> = text
        .split(['\n'])
        .flat_map(|l| l.split(". "))
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| l.len() >= 12)
        .enumerate()
        .map(|(i, l)| {
            let low = l.to_lowercase();
            let hits = words.iter().filter(|w| low.contains(w.as_str())).count() as i32;
            let score = hits * 2 + if figure.is_match(&l) { 3 } else if l.chars().any(|c| c.is_ascii_digit()) { 1 } else { 0 };
            (i, score, l.chars().take(300).collect())
        })
        .filter(|(_, s, _)| *s >= 3)
        .collect();
    scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut picked: Vec<(usize, String)> = Vec::new();
    let mut used = 0;
    for (i, _, l) in scored {
        if used + l.len() > max_chars || picked.iter().any(|(_, p)| p == &l) {
            continue;
        }
        used += l.len() + 1;
        picked.push((i, l));
    }
    picked.sort_by_key(|(i, _)| *i);
    picked.into_iter().map(|(_, l)| l).collect::<Vec<_>>().join("\n")
}

// ---------- weather ----------

/// What the WMO weather codes mean.
fn weather_words(code: i64) -> &'static str {
    match code {
        0 => "clear",
        1 => "mainly clear",
        2 => "partly cloudy",
        3 => "overcast",
        45 | 48 => "fog",
        51 | 53 | 55 => "drizzle",
        56 | 57 => "freezing drizzle",
        61 => "light rain",
        63 => "rain",
        65 => "heavy rain",
        66 | 67 => "freezing rain",
        71 => "light snow",
        73 => "snow",
        75 => "heavy snow",
        77 => "snow grains",
        80 | 81 => "rain showers",
        82 => "heavy rain showers",
        85 | 86 => "snow showers",
        95 => "thunderstorm",
        96 | 99 => "thunderstorm with hail",
        _ => "unsettled",
    }
}

/// Current conditions and a short forecast from Open-Meteo (no account;
/// weather data CC BY 4.0). Places like "Orem, Utah" are matched by name,
/// then by the state or country after the comma.
pub async fn weather(client: &reqwest::Client, place: &str, days: usize) -> Result<String, String> {
    let (name, region) = match place.split_once(',') {
        Some((n, r)) => (n.trim(), r.trim().to_lowercase()),
        None => (place.trim(), String::new()),
    };
    let geo: serde_json::Value = client
        .get("https://geocoding-api.open-meteo.com/v1/search")
        .query(&[("name", name), ("count", "10"), ("language", "en"), ("format", "json")])
        .send()
        .await
        .map_err(|e| format!("Couldn't reach the weather service: {e}"))?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let places = geo["results"].as_array().cloned().unwrap_or_default();
    let fits = |p: &serde_json::Value| {
        region.is_empty()
            || ["admin1", "country", "country_code"].iter().any(|k| p[*k].as_str().is_some_and(|v| {
                let v = v.to_lowercase();
                v == region || v.contains(&region) || region.contains(&v) || us_state_code(&region) == Some(v.as_str())
            }))
    };
    let p = places.iter().find(|p| fits(p)).or(places.first()).ok_or_else(|| format!("I couldn't find a place called “{place}”."))?;
    let (lat, lon) = (p["latitude"].as_f64().unwrap_or(0.0), p["longitude"].as_f64().unwrap_or(0.0));
    let us = matches!(p["country_code"].as_str(), Some("US" | "LR" | "MM"));
    let label = [p["name"].as_str(), p["admin1"].as_str(), p["country"].as_str()].into_iter().flatten().collect::<Vec<_>>().join(", ");
    let f: serde_json::Value = client
        .get("https://api.open-meteo.com/v1/forecast")
        .query(&[
            ("latitude", lat.to_string()),
            ("longitude", lon.to_string()),
            ("current", "temperature_2m,apparent_temperature,relative_humidity_2m,precipitation,weather_code,wind_speed_10m".into()),
            ("daily", "weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max".into()),
            ("temperature_unit", if us { "fahrenheit" } else { "celsius" }.into()),
            ("wind_speed_unit", if us { "mph" } else { "kmh" }.into()),
            ("timezone", "auto".into()),
            ("forecast_days", days.clamp(1, 7).to_string()),
        ])
        .send()
        .await
        .map_err(|e| format!("Couldn't reach the weather service: {e}"))?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let (t, w) = if us { ("°F", "mph") } else { ("°C", "km/h") };
    let other = |v: f64| if us { format!("{:.0}°C", (v - 32.0) * 5.0 / 9.0) } else { format!("{:.0}°F", v * 9.0 / 5.0 + 32.0) };
    let c = &f["current"];
    let num = |v: &serde_json::Value| v.as_f64().unwrap_or(f64::NAN);
    let mut out = format!(
        "Weather for {label} (Open-Meteo, local time {}):\nNow: {:.0}{t} ({}), feels like {:.0}{t}, {}, humidity {:.0}%, wind {:.0} {w}.",
        c["time"].as_str().unwrap_or("").replace('T', " "),
        num(&c["temperature_2m"]),
        other(num(&c["temperature_2m"])),
        num(&c["apparent_temperature"]),
        weather_words(c["weather_code"].as_i64().unwrap_or(-1)),
        num(&c["relative_humidity_2m"]),
        num(&c["wind_speed_10m"]),
    );
    let d = &f["daily"];
    for i in 0..d["time"].as_array().map_or(0, Vec::len) {
        let day = match i {
            0 => "Today".to_string(),
            1 => "Tomorrow".to_string(),
            _ => d["time"][i].as_str().unwrap_or("").to_string(),
        };
        out.push_str(&format!(
            "\n{day}: high {:.0}{t} ({}), low {:.0}{t} ({}), {}, chance of rain or snow {:.0}%.",
            num(&d["temperature_2m_max"][i]),
            other(num(&d["temperature_2m_max"][i])),
            num(&d["temperature_2m_min"][i]),
            other(num(&d["temperature_2m_min"][i])),
            weather_words(d["weather_code"][i].as_i64().unwrap_or(-1)),
            num(&d["precipitation_probability_max"][i]),
        ));
    }
    Ok(out)
}

/// "ut" → "utah", for matching "Orem, UT".
fn us_state_code(code: &str) -> Option<&'static str> {
    const STATES: &[(&str, &str)] = &[
        ("al", "alabama"), ("ak", "alaska"), ("az", "arizona"), ("ar", "arkansas"), ("ca", "california"), ("co", "colorado"), ("ct", "connecticut"),
        ("de", "delaware"), ("fl", "florida"), ("ga", "georgia"), ("hi", "hawaii"), ("id", "idaho"), ("il", "illinois"), ("in", "indiana"), ("ia", "iowa"),
        ("ks", "kansas"), ("ky", "kentucky"), ("la", "louisiana"), ("me", "maine"), ("md", "maryland"), ("ma", "massachusetts"), ("mi", "michigan"),
        ("mn", "minnesota"), ("ms", "mississippi"), ("mo", "missouri"), ("mt", "montana"), ("ne", "nebraska"), ("nv", "nevada"), ("nh", "new hampshire"),
        ("nj", "new jersey"), ("nm", "new mexico"), ("ny", "new york"), ("nc", "north carolina"), ("nd", "north dakota"), ("oh", "ohio"), ("ok", "oklahoma"),
        ("or", "oregon"), ("pa", "pennsylvania"), ("ri", "rhode island"), ("sc", "south carolina"), ("sd", "south dakota"), ("tn", "tennessee"), ("tx", "texas"),
        ("ut", "utah"), ("vt", "vermont"), ("va", "virginia"), ("wa", "washington"), ("wv", "west virginia"), ("wi", "wisconsin"), ("wy", "wyoming"),
    ];
    STATES.iter().find(|(c, _)| *c == code).map(|(_, n)| *n)
}

// ---------- commands ----------

#[derive(Serialize)]
pub struct WebSettingsView {
    #[serde(flatten)]
    settings: WebSettings,
    has_brave_key: bool,
}

#[tauri::command]
pub fn get_web_settings(state: AppStateRef) -> Result<WebSettingsView, String> {
    let c = state.cipher()?;
    let conn = state.db.lock().unwrap();
    Ok(WebSettingsView { settings: settings(&conn), has_brave_key: brave_key(&conn, &c).is_some() })
}

/// `brave_key`: None keeps the saved key, "" removes it.
#[tauri::command]
pub fn set_web_settings(state: AppStateRef, settings: WebSettings, brave_key: Option<String>) -> Result<(), String> {
    let c = state.cipher()?;
    let url = settings.searxng_url.trim();
    if !url.is_empty() && !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("The SearXNG address should start with http:// or https://".into());
    }
    let conn = state.db.lock().unwrap();
    db::set(&conn, "web", &settings)?;
    if let Some(k) = brave_key {
        db::set(&conn, "web_brave_key", &c.encrypt(k.trim()))?;
    }
    db::log_action(&conn, "privacy", &format!("Web search set to {}", provider_name(settings.provider)));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_and_local_addresses_are_refused() {
        for ip in ["127.0.0.1", "10.0.0.5", "192.168.1.20", "172.16.3.1", "169.254.1.1", "100.64.0.1", "0.0.0.0", "::1", "fd00::1", "fe80::1", "::ffff:192.168.0.1"] {
            assert!(!is_public(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "1.1.1.1", "2606:4700:4700::1111"] {
            assert!(is_public(ip.parse().unwrap()), "{ip}");
        }
    }

    #[tokio::test]
    async fn url_checks_need_no_network_for_obvious_cases() {
        for bad in ["file:///C:/secret.txt", "http://localhost:1430/", "http://127.0.0.1:8080/", "http://printer.local/", "http://intranet/", "http://[::1]/", "ftp://example.com/"] {
            assert!(check_url(bad).await.is_err(), "{bad}");
        }
        assert!(check_url("https://93.184.215.14/").await.is_ok());
    }

    #[test]
    fn the_web_search_feature_allows_search_while_offline() {
        let d = std::env::temp_dir().join(format!("sulcusai-web-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        let conn = db::open(&d.join("t.db")).unwrap();
        conn.execute("INSERT INTO chats (id, title, web, created_at, updated_at) VALUES ('c', 't', 0, 1, 1)", []).unwrap();
        assert_eq!(db::settings(&conn).connectivity, Connectivity::Offline);
        assert!(!allowed_for_chat(&conn, "c"));
        crate::features::set(&conn, crate::features::Feature::WebSearch, true).unwrap();
        assert!(allowed_for_chat(&conn, "c"));
        assert_eq!(db::settings(&conn).connectivity, Connectivity::Offline, "the level stays Offline");
        // The browser, email and cloud still follow the level.
        let (level, _) = chat_access(&conn, "c");
        assert!(!net::allowed(level, Purpose::Web, false) && !net::allowed(level, Purpose::Cloud, true));
    }

    #[test]
    fn excerpts_keep_the_lines_with_answers() {
        let page = "Skip to content\nSign in\nOrem, UT Weather Forecast\nCurrent conditions: 58°F, partly cloudy. Wind NW 7 mph.\nToday's high 64°F and low 41°F.\nPrivacy policy\nAdvertise with us\nOrem is a city in Utah County.";
        let e = excerpts(page, "weather in Orem Utah today", 400);
        assert!(e.contains("58°F") && e.contains("high 64°F"), "{e}");
        assert!(!e.contains("Privacy") && !e.contains("Sign in"), "{e}");
        assert!(e.find("58°F").unwrap() < e.find("64°F").unwrap(), "page order is kept");
        assert_eq!(us_state_code("ut"), Some("utah"));
        assert_eq!(weather_words(2), "partly cloudy");
    }

    #[test]
    fn duckduckgo_results_are_read_and_ads_skipped() {
        let html = r#"
        <div class="result results_links"><a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust-lang.org%2F&amp;rut=abc">Rust <b>Programming</b> Language</a>
        <a class="result__snippet" href="x">A language empowering <b>everyone</b> &amp; more.</a></div>
        <div class="result"><a class="result__a" href="https://duckduckgo.com/y.js?ad_provider=x">Ad</a><a class="result__snippet" href="y">ad text</a></div>
        <div class="result"><a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fdoc.rust-lang.org%2Fbook%2F&amp;rut=d">The Book</a></div>"#;
        let hits = parse_duckduckgo(html);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0], Hit { title: "Rust Programming Language".into(), url: "https://www.rust-lang.org/".into(), snippet: "A language empowering everyone & more.".into() });
        assert_eq!(hits[1].url, "https://doc.rust-lang.org/book/");
    }

    #[test]
    fn pages_become_readable_text() {
        let html = "<html><head><title>Hello &amp; welcome</title><style>p{color:red}</style><script>alert(1)</script></head>
            <body><nav>Home | About</nav><h1>Big news</h1><p>The launch moved to <b>Thursday</b>.</p><ul><li>One</li><li>Two</li></ul><footer>© 2026</footer></body></html>";
        let (title, text) = html_to_text(html);
        assert_eq!(title, "Hello & welcome");
        assert!(text.contains("Big news") && text.contains("Thursday") && text.contains("One"));
        assert!(!text.contains("alert") && !text.contains("color:red") && !text.contains("Home | About") && !text.contains("2026"));
    }
}
