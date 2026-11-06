// SPDX-License-Identifier: AGPL-3.0-only
//! Link previews and GIF search for chats. Both reach the web, so each has
//! its own switch that starts off, and both go through the app's web client
//! (the private way and the local-network guard apply).

use base64::Engine;
use serde::Serialize;

use super::store::{book, update};
use crate::AppStateRef;

const PAGE_LIMIT: usize = 512 * 1024;
const IMAGE_LIMIT: usize = 1024 * 1024;
const GIF_LIMIT: usize = 3 * 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct LinkPreview {
    pub url: String,
    pub title: String,
    pub description: String,
    pub site: String,
    /// A data: URL, so the chat never loads from the site itself.
    pub image: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Gif {
    pub title: String,
    pub preview: String,
    pub url: String,
}

fn client(state: &crate::AppState) -> Result<reqwest::Client, String> {
    crate::web::client(state.settings().connectivity, false).map_err(|_| "This needs web access: switch to Local AI + Web (or Cloud).".to_string())
}

async fn get_limited(c: &reqwest::Client, url: &str, limit: usize) -> Result<(Vec<u8>, String), String> {
    let mut res = c.get(url).send().await.map_err(|e| e.to_string())?.error_for_status().map_err(|e| e.to_string())?;
    let mime = res.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let mut out = Vec::new();
    while let Some(chunk) = res.chunk().await.map_err(|e| e.to_string())? {
        out.extend_from_slice(&chunk);
        if out.len() > limit {
            return Err("That's too big to show here.".into());
        }
    }
    Ok((out, mime))
}

/// The value of `<meta property|name="key" content="...">` in a page.
pub fn meta(html: &str, key: &str) -> Option<String> {
    let re = regex::Regex::new(&format!(r#"(?is)<meta[^>]+(?:property|name)\s*=\s*["']{}["'][^>]*>"#, regex::escape(key))).ok()?;
    let tag = re.find(html)?.as_str();
    let content = regex::Regex::new(r#"(?is)content\s*=\s*["']([^"']*)["']"#).ok()?;
    Some(unescape(content.captures(tag)?.get(1)?.as_str().trim()))
}

fn unescape(s: &str) -> String {
    s.replace("&amp;", "&").replace("&quot;", "\"").replace("&#39;", "'").replace("&lt;", "<").replace("&gt;", ">")
}

pub fn parse_preview(url: &str, html: &str) -> LinkPreview {
    let title_tag = regex::Regex::new(r"(?is)<title[^>]*>(.*?)</title>").ok().and_then(|r| r.captures(html)).map(|c| unescape(c[1].trim()));
    let host = reqwest::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_default();
    LinkPreview {
        url: url.into(),
        title: meta(html, "og:title").or(title_tag).unwrap_or_default().chars().take(200).collect(),
        description: meta(html, "og:description").or_else(|| meta(html, "description")).unwrap_or_default().chars().take(300).collect(),
        site: meta(html, "og:site_name").unwrap_or(host),
        image: meta(html, "og:image"),
    }
}

fn https(url: &str) -> Result<reqwest::Url, String> {
    let u = reqwest::Url::parse(url).map_err(|_| "That isn't a web address.".to_string())?;
    if u.scheme() != "https" && u.scheme() != "http" {
        return Err("Only web addresses get previews.".into());
    }
    Ok(u)
}

#[tauri::command]
pub async fn social_link_preview(state: AppStateRef<'_>, url: String) -> Result<LinkPreview, String> {
    if !book(&state.db.lock().unwrap()).switches.link_previews {
        return Err("Link previews are off.".into());
    }
    let u = https(&url)?;
    let c = client(&state)?;
    let (body, _) = get_limited(&c, u.as_str(), PAGE_LIMIT).await?;
    let mut p = parse_preview(u.as_str(), &String::from_utf8_lossy(&body));
    p.image = match p.image.take().and_then(|i| u.join(&i).ok()) {
        Some(img) => match get_limited(&c, img.as_str(), IMAGE_LIMIT).await {
            Ok((bytes, mime)) if mime.starts_with("image/") && !mime.contains("svg") => Some(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))),
            _ => None,
        },
        None => None,
    };
    Ok(p)
}

fn giphy_host(u: &reqwest::Url) -> bool {
    u.scheme() == "https" && u.host_str().is_some_and(|h| h == "giphy.com" || h.ends_with(".giphy.com"))
}

/// GIF search through GIPHY with the user's own key. What's sent: the
/// search words and the key, nothing about the chat.
#[tauri::command]
pub async fn social_gif_search(state: AppStateRef<'_>, query: String) -> Result<Vec<Gif>, String> {
    let key = {
        let b = book(&state.db.lock().unwrap());
        if !b.switches.gif_search {
            return Err("GIF search is off.".into());
        }
        b.switches.gif_key.clone()
    };
    if key.trim().is_empty() {
        return Err("Add a GIPHY key in group settings first.".into());
    }
    let mut u = reqwest::Url::parse("https://api.giphy.com/v1/gifs/search").expect("giphy url");
    u.query_pairs_mut().append_pair("api_key", key.trim()).append_pair("q", &query).append_pair("limit", "24").append_pair("rating", "pg-13");
    let (body, _) = get_limited(&client(&state)?, u.as_str(), PAGE_LIMIT * 4).await?;
    let v: serde_json::Value = serde_json::from_slice(&body).map_err(|_| "GIPHY's answer didn't make sense.".to_string())?;
    Ok(v["data"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|g| {
                    let img = &g["images"];
                    Some(Gif { title: g["title"].as_str().unwrap_or("").into(), preview: img["fixed_width_small"]["url"].as_str()?.into(), url: img["fixed_height"]["url"].as_str()?.into() })
                })
                .collect()
        })
        .unwrap_or_default())
}

/// The GIF's bytes, so it travels inside the message and members' PCs load
/// nothing from GIPHY.
#[tauri::command]
pub async fn social_gif_fetch(state: AppStateRef<'_>, url: String) -> Result<String, String> {
    if !book(&state.db.lock().unwrap()).switches.gif_search {
        return Err("GIF search is off.".into());
    }
    let u = https(&url)?;
    if !giphy_host(&u) {
        return Err("Only GIFs from the search.".into());
    }
    let (bytes, _) = get_limited(&client(&state)?, u.as_str(), GIF_LIMIT).await?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// Remembers a site the user chose to open from a chat, so it isn't asked
/// about again.
#[tauri::command]
pub async fn social_trust_site(state: AppStateRef<'_>, host: String) -> Result<(), String> {
    update(&state, |b| {
        let h = host.to_lowercase();
        if !b.personal.known_sites.contains(&h) {
            b.personal.known_sites.push(h);
        }
        Ok(())
    })
}

#[tauri::command]
pub async fn social_known_sites(state: AppStateRef<'_>) -> Result<Vec<String>, String> {
    Ok(book(&state.db.lock().unwrap()).personal.known_sites)
}
