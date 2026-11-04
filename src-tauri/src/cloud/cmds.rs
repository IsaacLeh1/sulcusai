// SPDX-License-Identifier: AGPL-3.0-only
//! The window's commands for cloud providers.

use super::*;


#[derive(Serialize)]
pub struct ProviderView {
    id: String,
    name: String,
    kind: Kind,
    base_url: String,
    enabled: bool,
    has_key: bool,
    models: Vec<CloudModel>,
    media: Vec<CloudMedia>,
    /// Picture, video and voice models this provider offers.
    media_suggestions: Vec<CloudMedia>,
    share_pcs: bool,
    share_people: Vec<String>,
    /// Its requests go through a proxy of their own.
    has_proxy: bool,
}

#[derive(Serialize)]
pub struct CloudView {
    providers: Vec<ProviderView>,
    presets: Vec<ProviderView>,
    budget: Option<f64>,
    redact: bool,
    spend: Spend,
}

pub(super) fn pview(p: &Provider) -> ProviderView {
    ProviderView {
        id: p.id.clone(),
        name: p.name.clone(),
        kind: p.kind,
        base_url: p.base_url.clone(),
        enabled: p.enabled,
        has_key: !p.key.is_empty(),
        models: p.models.clone(),
        media: p.media.clone(),
        media_suggestions: media_suggestions(&p.id),
        share_pcs: p.share_pcs,
        share_people: p.share_people.clone(),
        has_proxy: !p.proxy.is_empty(),
    }
}

#[tauri::command(async)]
pub fn cloud_view(state: AppStateRef) -> CloudView {
    let conn = state.db.lock().unwrap();
    let s = settings(&conn);
    let presets = presets().iter().filter(|p| !s.providers.iter().any(|q| q.id == p.id)).map(pview).collect();
    CloudView { providers: s.providers.iter().map(pview).collect(), presets, budget: s.budget, redact: s.redact, spend: spend(&conn) }
}

/// Adds a preset provider, or a custom OpenAI-compatible one.
#[tauri::command(async)]
pub fn add_cloud_provider(state: AppStateRef, preset: Option<String>, name: Option<String>, base_url: Option<String>) -> Result<CloudView, String> {
    state.cipher()?;
    let p = match preset {
        Some(id) => presets().into_iter().find(|p| p.id == id).ok_or("Unknown provider")?,
        None => {
            let url = clean_base(&base_url.unwrap_or_default());
            if !net::private_ok(&url) {
                return Err("Use an https:// address (or a local one).".into());
            }
            let name = name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "Custom provider".into());
            // The same server gets the same id on every PC, so a chat synced
            // from another PC finds its model here. The id stays in this
            // app (and its synced chats); it's never sent to the server.
            Provider { id: custom_id(&url), name, kind: Kind::Openai, base_url: url, enabled: false, key: String::new(), models: Vec::new(), media: Vec::new(), cookies: String::new(), share_pcs: false, share_people: Vec::new(), proxy: String::new(), ..Default::default() }
        }
    };
    let mut p = p;
    {
        let conn = state.db.lock().unwrap();
        let mut s = settings(&conn);
        // The same server again (another key): an id of its own.
        if p.id.starts_with("custom-") {
            let mut n = 2;
            while s.providers.iter().any(|q| q.id == p.id) {
                p.id = custom_id(&format!("{}#{n}", p.base_url));
                n += 1;
            }
        }
        if !s.providers.iter().any(|q| q.id == p.id) {
            s.providers.push(p.clone());
        }
        save(&conn, &s)?;
    }
    state.log("settings", &format!("Added the cloud provider {}", p.name));
    Ok(cloud_view(state))
}

#[tauri::command(async)]
pub fn remove_cloud_provider(state: AppStateRef, id: String) -> Result<CloudView, String> {
    {
        let conn = state.db.lock().unwrap();
        let mut s = settings(&conn);
        s.providers.retain(|p| p.id != id);
        save(&conn, &s)?;
    }
    state.log("settings", "Removed a cloud provider and its key");
    Ok(cloud_view(state))
}

/// Sets the key (empty clears it) and on/off for a provider.
#[tauri::command(async)]
pub fn set_cloud_provider(state: AppStateRef, id: String, enabled: bool, key: Option<String>) -> Result<CloudView, String> {
    let c = state.cipher()?;
    let name = {
        let conn = state.db.lock().unwrap();
        let mut s = settings(&conn);
        let p = s.providers.iter_mut().find(|p| p.id == id).ok_or("Unknown provider")?;
        if let Some(k) = key {
            p.key = if k.trim().is_empty() { String::new() } else { c.encrypt(k.trim()) };
        }
        p.enabled = enabled && !p.key.is_empty();
        let name = p.name.clone();
        save(&conn, &s)?;
        name
    };
    state.log("settings", &format!("Updated the cloud provider {name}"));
    Ok(cloud_view(state))
}

/// The models a provider offers (asks the provider).
#[tauri::command]
pub async fn cloud_models_available(state: AppStateRef<'_>, id: String) -> Result<Vec<CloudModel>, String> {
    let c = state.cipher()?;
    let p = settings(&state.db.lock().unwrap()).providers.into_iter().find(|p| p.id == id).ok_or("Unknown provider")?;
    if !p.has_credentials() {
        return Err(format!("Add your {} API key first.", p.name));
    }
    if let Some(why) = enterprise::problem(&p) {
        return Err(why);
    }
    let key = if p.key.is_empty() { String::new() } else { c.decrypt(&p.key)? };
    let cookies = if p.cookies.is_empty() { String::new() } else { c.decrypt(&p.cookies).unwrap_or_default() };
    let w = wire_of(&p, &c);
    let client = via(&state, &w)?;
    state.log("network", &format!("Asked {} which models it offers", p.name));
    list_models(Some(&state), &p, &key, &cookies, &client, &w).await
}

/// Where a server's pages are, from its API address: Open WebUI's API is
/// under /api, LiteLLM's under /v1.
pub(super) fn site_of(base_url: &str) -> Result<String, String> {
    let b = base_url.trim().trim_end_matches('/');
    let b = b.strip_suffix("/api").or_else(|| b.strip_suffix("/v1")).unwrap_or(b);
    let u = reqwest::Url::parse(&format!("{b}/")).map_err(|_| "That server's address isn't a web address.".to_string())?;
    if !matches!(u.scheme(), "http" | "https") {
        return Err("That server's address isn't a web address.".into());
    }
    Ok(u.to_string())
}

/// Signs in to a server in the built-in browser (Open WebUI, or one
/// behind a sign-in page): its pages open there, and once the user has
/// signed in, its session is used: Open WebUI's sign-in becomes the key,
/// and the site's cookies go with its API calls. Watches for ten minutes.
#[tauri::command(async)]
pub fn cloud_sign_in(app: tauri::AppHandle, state: AppStateRef, id: String) -> Result<String, String> {
    use tauri::Emitter;
    state.cipher()?;
    let p = settings(&state.db.lock().unwrap()).providers.into_iter().find(|p| p.id == id).ok_or("Unknown provider")?;
    if p.kind != Kind::Openai || presets().iter().any(|x| x.id == p.id) {
        return Err(format!("{} uses an API key: press Get a key… to sign in to your account and make one.", p.name));
    }
    let site = site_of(&p.base_url)?;
    app.emit("viewer:open", json!({ "url": site })).map_err(|e| e.to_string())?;
    state.log("settings", &format!("Opened {} in the built-in browser to sign in", p.name));
    let state = state.inner().clone();
    let at = site.clone();
    tauri::async_runtime::spawn(async move { watch_sign_in(app, state, id, at).await });
    Ok(site)
}

/// Waits for the sign-in in the viewer, trying the session each time the
/// site's cookies change, until its model list answers.
pub(super) async fn watch_sign_in(app: tauri::AppHandle, state: Arc<AppState>, id: String, site: String) {
    use tauri::Emitter;
    let Ok(url) = tauri::Url::parse(&site) else { return };
    let until = std::time::Instant::now() + Duration::from_secs(600);
    let mut tried = String::new();
    while std::time::Instant::now() < until {
        tokio::time::sleep(Duration::from_secs(2)).await;
        // Any web viewer: they share one profile, so one sign-in.
        let Some(w) = crate::viewer::all(&app).into_iter().next() else { continue };
        let u = url.clone();
        // Never on the main thread: reading cookies there waits forever (WebView2).
        let Ok(Ok(cookies)) = tokio::task::spawn_blocking(move || w.cookies_for_url(u)).await else { continue };
        let header = cookies.iter().map(|c| format!("{}={}", c.name(), c.value())).collect::<Vec<_>>().join("; ");
        if header.is_empty() || header == tried {
            continue;
        }
        tried = header.clone();
        let (Ok(c), Some(p)) = (state.cipher(), settings(&state.db.lock().unwrap()).providers.into_iter().find(|p| p.id == id)) else { return };
        // Open WebUI keeps its sign-in in "token"; otherwise the key saved before.
        let token = cookies.iter().find(|c| c.name() == "token").map(|c| c.value().to_string());
        let key = match token.clone() {
            Some(t) => t,
            None if !p.key.is_empty() => c.decrypt(&p.key).unwrap_or_default(),
            None => continue,
        };
        let w = wire_of(&p, &c);
        let Ok(http) = via(&state, &w) else { return };
        if list_models(Some(&state), &p, &key, &header, &http, &w).await.is_err() {
            continue;
        }
        let saved = {
            let conn = state.db.lock().unwrap();
            let mut s = settings(&conn);
            match s.providers.iter_mut().find(|x| x.id == id) {
                Some(x) => {
                    x.key = c.encrypt(&key);
                    // Its token alone is enough for Open WebUI.
                    x.cookies = if token.is_some() { String::new() } else { c.encrypt(&header) };
                    x.enabled = true;
                    save(&conn, &s).is_ok()
                }
                None => false,
            }
        };
        if saved {
            state.log("settings", &format!("Signed in to {} in the built-in browser; its session is kept encrypted", p.name));
            app.emit("cloud:signed_in", json!({ "id": id, "name": p.name })).ok();
        }
        return;
    }
}

/// What a chat has cost on cloud models, with this month's total and the budget.
#[derive(Serialize)]
pub struct ChatSpend {
    chat: Spend,
    month: Spend,
    budget: Option<f64>,
}

#[tauri::command(async)]
pub fn chat_cloud_spend(state: AppStateRef, chat_id: String) -> ChatSpend {
    let conn = state.db.lock().unwrap();
    ChatSpend { chat: db::get(&conn, &chat_spend_key(&chat_id)).unwrap_or_default(), month: spend(&conn), budget: settings(&conn).budget }
}

/// Which of the provider's models appear in the model lists.
#[tauri::command(async)]
pub fn set_cloud_models(state: AppStateRef, id: String, models: Vec<CloudModel>) -> Result<CloudView, String> {
    {
        let conn = state.db.lock().unwrap();
        let mut s = settings(&conn);
        let p = s.providers.iter_mut().find(|p| p.id == id).ok_or("Unknown provider")?;
        p.models = models;
        save(&conn, &s)?;
    }
    Ok(cloud_view(state))
}

/// The picture, video and voice models turned on for a provider.
#[tauri::command(async)]
pub fn set_cloud_media(state: AppStateRef, id: String, media: Vec<CloudMedia>) -> Result<CloudView, String> {
    {
        let conn = state.db.lock().unwrap();
        let mut s = settings(&conn);
        let p = s.providers.iter_mut().find(|p| p.id == id).ok_or("Unknown provider")?;
        p.media = media.into_iter().filter(|m| !m.id.trim().is_empty()).collect();
        save(&conn, &s)?;
    }
    Ok(cloud_view(state))
}

#[tauri::command(async)]
pub fn set_cloud_options(state: AppStateRef, budget: Option<f64>, redact: bool) -> Result<CloudView, String> {
    {
        let conn = state.db.lock().unwrap();
        let mut s = settings(&conn);
        s.budget = budget.filter(|b| *b > 0.0);
        s.redact = redact;
        save(&conn, &s)?;
    }
    state.log("settings", "Changed the cloud budget or privacy options");
    Ok(cloud_view(state))
}

/// The cloud models people can pick for a chat (enabled providers only).
#[derive(Serialize)]
pub struct CloudChoice {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub vision: bool,
    /// US dollars per million tokens in and out, if known.
    pub price_in: Option<f64>,
    pub price_out: Option<f64>,
}

#[tauri::command(async)]
pub fn cloud_choices(state: AppStateRef) -> Vec<CloudChoice> {
    choices(&state.db.lock().unwrap())
}

/// A server's provider id, from its address (the same on every PC).
pub fn custom_id(base_url: &str) -> String {
    use sha2::{Digest, Sha256};
    let url = base_url.trim().trim_end_matches('/').to_lowercase();
    format!("custom-{}", &hex::encode(Sha256::digest(url.as_bytes()))[..8])
}

/// Which provider here a cloud model id is from: the same id, or (a chat
/// synced from another PC) one offering the same model.
pub fn provider_of<'a>(s: &'a CloudSettings, id: &str) -> Option<&'a Provider> {
    let (pid, mid) = parse_ref(id)?;
    s.providers
        .iter()
        .find(|p| p.id == pid && p.models.iter().any(|m| m.id == mid))
        .or_else(|| s.providers.iter().find(|p| p.enabled && !p.key.is_empty() && p.models.iter().any(|m| m.id == mid)))
}

/// A fingerprint of a provider's address and key: two PCs with the same one
/// use the same account. It can't be turned back into the key, and it's
/// only told to this user's own paired PCs, inside their encrypted link.
pub fn key_print(c: &Cipher, p: &Provider) -> Option<String> {
    use sha2::{Digest, Sha256};
    let key = c.decrypt(&p.key).ok().filter(|k| !k.trim().is_empty())?;
    let url = p.base_url.trim().trim_end_matches('/').to_lowercase();
    let mut h = Sha256::new();
    h.update(b"sulcusai-key-print\0");
    h.update(url.as_bytes());
    h.update(b"\0");
    h.update(key.trim().as_bytes());
    Some(hex::encode(h.finalize())[..20].to_string())
}

/// The fingerprints of the providers whose chats go to this user's other PCs.
pub fn shared_prints(conn: &Connection, c: &Cipher) -> Vec<String> {
    settings(conn).providers.iter().filter(|p| p.share_pcs && p.enabled).filter_map(|p| key_print(c, p)).collect()
}

/// Chats made with a cloud model that another PC (with these fingerprints)
/// doesn't get: its provider's chats stay on this PC, or that PC doesn't
/// use the same key.
pub fn withheld_chats(conn: &Connection, c: &Cipher, theirs: &[String]) -> std::collections::HashSet<String> {
    let s = settings(conn);
    let Ok(mut stmt) = conn.prepare("SELECT id, model_id FROM chats WHERE model_id LIKE 'cloud:%'") else { return Default::default() };
    let rows: Vec<(String, String)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map(|it| it.filter_map(Result::ok).collect()).unwrap_or_default();
    rows.into_iter()
        .filter(|(_, model)| match provider_of(&s, model) {
            Some(p) => !(p.share_pcs && key_print(c, p).is_some_and(|k| theirs.contains(&k))),
            None => true,
        })
        .map(|(id, _)| id)
        .collect()
}

/// The chats made with a provider's models.
pub fn chats_of(conn: &Connection, provider: &Provider) -> Vec<String> {
    let s = CloudSettings { providers: vec![provider.clone()], ..Default::default() };
    let Ok(mut stmt) = conn.prepare("SELECT id, model_id FROM chats WHERE model_id LIKE 'cloud:%'") else { return Vec::new() };
    let rows: Vec<(String, String)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map(|it| it.filter_map(Result::ok).collect()).unwrap_or_default();
    rows.into_iter().filter(|(_, m)| provider_of(&s, m).is_some_and(|p| p.id == provider.id)).map(|(id, _)| id).collect()
}

/// The people a chat goes to because of its cloud model's provider.
pub fn people_for(conn: &Connection, model_id: Option<&str>) -> Vec<String> {
    let Some(m) = model_id.filter(|m| is_cloud(m)) else { return Vec::new() };
    provider_of(&settings(conn), m).map(|p| p.share_people.clone()).unwrap_or_default()
}

/// What a proxy address must look like: socks5://, socks5h:// (names looked
/// up through the proxy, needed for names only the VPN knows), http:// or
/// https://, with a host and a port.
pub fn proxy_problem(proxy: &str) -> Option<&'static str> {
    let Ok(u) = reqwest::Url::parse(proxy.trim()) else { return Some("Write the proxy like socks5h://127.0.0.1:1080.") };
    if !matches!(u.scheme(), "socks5" | "socks5h" | "http" | "https") {
        return Some("The proxy must start with socks5h://, socks5://, http:// or https://.");
    }
    if u.host_str().is_none_or(str::is_empty) || u.port().is_none() {
        return Some("Give the proxy's address and port, like socks5h://127.0.0.1:1080.");
    }
    None
}

/// Sends one provider's requests through a proxy (empty: directly).
#[tauri::command(async)]
pub fn set_cloud_proxy(state: AppStateRef, id: String, proxy: String) -> Result<CloudView, String> {
    let c = state.cipher()?;
    let proxy = proxy.trim().to_string();
    if !proxy.is_empty() {
        if let Some(why) = proxy_problem(&proxy) {
            return Err(why.into());
        }
    }
    let name = {
        let conn = state.db.lock().unwrap();
        let mut s = settings(&conn);
        let p = s.providers.iter_mut().find(|p| p.id == id).ok_or("Unknown provider")?;
        p.proxy = if proxy.is_empty() { String::new() } else { c.encrypt(&proxy) };
        let name = p.name.clone();
        save(&conn, &s)?;
        name
    };
    state.log("settings", &format!("{name}: requests {}", if proxy.is_empty() { "go directly" } else { "go through its own proxy" }));
    Ok(cloud_view(state))
}

/// Where a provider's chats go: this user's other PCs with the same key,
/// and people linked in People.
#[tauri::command(async)]
pub fn set_cloud_sharing(state: AppStateRef, id: String, share_pcs: bool, share_people: Vec<String>) -> Result<CloudView, String> {
    let c = state.cipher()?;
    let (p, chats, was) = {
        let conn = state.db.lock().unwrap();
        let mut s = settings(&conn);
        let p = s.providers.iter_mut().find(|p| p.id == id).ok_or("Unknown provider")?;
        let was = p.share_pcs;
        p.share_pcs = share_pcs;
        p.share_people = share_people;
        let p = p.clone();
        save(&conn, &s)?;
        let chats = chats_of(&conn, &p);
        (p, chats, was)
    };
    // Turned on: chats held back from the other PCs go at the next sync.
    if share_pcs && !was {
        let _ = crate::sync::track::resend_chats(&state.db.lock().unwrap(), &chats);
        crate::sync::wake();
    }
    for chat in &chats {
        crate::sync::people::share::follow_provider(&state, &c, chat);
    }
    state.log(
        "settings",
        &format!("{}: chats {} the other PCs with its key{}", p.name, if share_pcs { "go to" } else { "stay off" }, if p.share_people.is_empty() { String::new() } else { format!(", and are shared with {} people", p.share_people.len()) }),
    );
    Ok(cloud_view(state))
}

/// The model a cloud id names, here: the same one, or the same model from a
/// provider this PC has under another id (a chat synced from another PC).
pub fn resolve(conn: &Connection, id: &str) -> Option<String> {
    let all = choices(conn);
    if all.iter().any(|c| c.id == id) {
        return Some(id.to_string());
    }
    let (_, model) = parse_ref(id)?;
    all.into_iter().find(|c| parse_ref(&c.id).is_some_and(|(_, m)| m == model)).map(|c| c.id)
}

pub fn choices(conn: &Connection) -> Vec<CloudChoice> {
    settings(conn)
        .providers
        .iter()
        .filter(|p| p.enabled && !p.key.is_empty())
        .flat_map(|p| {
            p.models.iter().map(move |m| CloudChoice { id: model_ref(&p.id, &m.id), name: m.name.clone(), provider: p.name.clone(), vision: m.vision, price_in: m.price_in, price_out: m.price_out })
        })
        .collect()
}

/// For a test against a real provider set up in the app: the models it
/// offers, and a one-word reply from each model picked. Nothing secret
/// (key, cookies, proxy, address) goes in the report.
#[cfg(test)]
pub(crate) async fn check_provider(p: &Provider, c: &Cipher) -> String {
    let mut report = format!("{} ({}, {}): ", p.name, if p.base_url.starts_with("https://") { "https" } else { "plain http" }, if p.proxy.is_empty() { "direct" } else { "through its proxy" });
    let key = match c.decrypt(&p.key) {
        Ok(k) => k,
        Err(_) => return report + "its key couldn't be opened",
    };
    let cookies = if p.cookies.is_empty() { String::new() } else { c.decrypt(&p.cookies).unwrap_or_default() };
    let w = wire_of(p, c);
    let client = match net::external_client_ca(Connectivity::Cloud, Purpose::Cloud, Some(w.proxy.as_str()).filter(|x| !x.is_empty()), Some(w.ca.as_str()).filter(|x| !x.is_empty())) {
        Ok(c) => c,
        Err(e) => return report + &e,
    };
    let started = std::time::Instant::now();
    let offered = match list_models(None, p, &key, &cookies, &client, &w).await {
        Ok(models) => {
            report += &format!("offers {} models in {} ms: {}; ", models.len(), started.elapsed().as_millis(), models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>().join(", "));
            models
        }
        Err(e) => return report + &format!("listing its models failed after {} ms: {e}", started.elapsed().as_millis()),
    };
    if p.kind != Kind::Openai {
        return report;
    }
    let base = p.base_url.trim_end_matches('/');
    // The models picked here, or (none picked yet) the first two it offers
    // that aren't for pictures, speech or embeddings.
    let tried: Vec<CloudModel> = if p.models.is_empty() {
        offered.into_iter().filter(|m| !["embed", "tts", "whisper", "image", "dall", "rerank", "moderation"].iter().any(|w| m.id.to_lowercase().contains(w))).take(2).collect()
    } else {
        p.models.clone()
    };
    for m in &tried {
        let t = std::time::Instant::now();
        let body = json!({ "model": m.id, "messages": [{ "role": "user", "content": "Reply with only the word OK." }], "max_tokens": 200, "stream": false });
        let mut req = client.post(format!("{base}/chat/completions")).bearer_auth(&key).json(&body).timeout(Duration::from_secs(120));
        if !cookies.is_empty() {
            req = req.header("cookie", &cookies);
        }
        let answer = match req.send().await {
            Ok(r) if r.status().is_success() => match r.json::<Value>().await {
                Ok(v) => format!("\u{201c}{}\u{201d}", v.pointer("/choices/0/message/content").and_then(Value::as_str).unwrap_or("(empty)").trim().chars().take(60).collect::<String>()),
                Err(e) => format!("unreadable reply ({e})"),
            },
            Ok(r) => format!("error {}", r.status()),
            Err(e) => format!("failed ({e})"),
        };
        report += &format!("\n  {}: {answer} in {} ms", m.name, t.elapsed().as_millis());
    }
    report
}
