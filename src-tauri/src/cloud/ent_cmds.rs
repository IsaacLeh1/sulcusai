// SPDX-License-Identifier: AGPL-3.0-only
//! The window's commands for enterprise providers and gateways: Azure,
//! Bedrock and Vertex settings, Entra sign-in, headers and a company CA.

use super::enterprise::{self, Auth, Ent};
use super::*;

/// What the window shows of a provider's enterprise settings (no secrets).
#[derive(Serialize)]
pub struct EntView {
    auth: Auth,
    api_version: String,
    region: String,
    project: String,
    profile: String,
    tenant: String,
    client_id: String,
    authority: String,
    signed_in: bool,
    account: String,
    /// The names of its extra headers (values stay sealed).
    header_names: Vec<String>,
    has_ca: bool,
    /// What's still needed before it can be used.
    problem: Option<String>,
    /// Set by the company policy file; only the key and models can change.
    locked: bool,
}

pub(super) fn ent_view(p: &Provider, c: Option<&Cipher>) -> EntView {
    let e = &p.ent;
    let header_names = c
        .filter(|_| !p.headers.is_empty())
        .and_then(|c| c.decrypt(&p.headers).ok())
        .and_then(|j| serde_json::from_str::<serde_json::Map<String, Value>>(&j).ok())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    EntView {
        auth: e.auth,
        api_version: e.api_version.clone(),
        region: e.region.clone(),
        project: e.project.clone(),
        profile: e.profile.clone(),
        tenant: e.tenant.clone(),
        client_id: e.client_id.clone(),
        authority: e.authority.clone(),
        signed_in: !e.token.is_empty(),
        account: e.account.clone(),
        header_names,
        has_ca: !p.ca_cert.trim().is_empty(),
        problem: enterprise::problem(p),
        locked: p.id == crate::policy::GATEWAY_ID,
    }
}

#[derive(Deserialize)]
pub struct EntInput {
    #[serde(default)]
    base_url: Option<String>,
    auth: Auth,
    #[serde(default)]
    api_version: String,
    #[serde(default)]
    region: String,
    #[serde(default)]
    project: String,
    #[serde(default)]
    profile: String,
    #[serde(default)]
    tenant: String,
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    authority: String,
}

fn allowed_auth(kind: Kind, auth: Auth) -> bool {
    matches!((kind, auth), (_, Auth::Key) | (Kind::Azure, Auth::Entra) | (Kind::Bedrock, Auth::Profile) | (Kind::Vertex, Auth::Adc))
}

/// Applies typed settings to a provider, checking each.
pub fn apply(p: &mut Provider, i: EntInput) -> Result<(), String> {
    if !allowed_auth(p.kind, i.auth) {
        return Err(format!("{} doesn't sign in that way.", p.name));
    }
    if p.id == crate::policy::GATEWAY_ID {
        return Err("Your organization set up this gateway; only its key and models can change here.".into());
    }
    if let Some(url) = i.base_url {
        let url = clean_base(&url);
        if !url.is_empty() && !net::private_ok(&url) {
            return Err("Use an https:// address (or a local one).".into());
        }
        p.base_url = url;
    }
    let authority = i.authority.trim().trim_end_matches('/').to_string();
    if !authority.is_empty() && !authority.starts_with("https://") && !net::private_ok(&authority) {
        return Err("The Entra sign-in address must start with https://.".into());
    }
    let changed_sign_in = p.ent.auth != i.auth || p.ent.tenant != i.tenant.trim() || p.ent.client_id != i.client_id.trim() || p.ent.authority != authority;
    let (token, account) = if changed_sign_in { (String::new(), String::new()) } else { (p.ent.token.clone(), p.ent.account.clone()) };
    p.ent = Ent {
        auth: i.auth,
        api_version: i.api_version.trim().into(),
        region: i.region.trim().into(),
        project: i.project.trim().into(),
        profile: i.profile.trim().into(),
        tenant: i.tenant.trim().into(),
        client_id: i.client_id.trim().into(),
        authority,
        token,
        account,
    };
    for (v, what) in [(&p.ent.tenant, "tenant"), (&p.ent.profile, "profile")] {
        if !v.is_empty() && !enterprise::plain_name(v) {
            return Err(format!("That {what} name has characters it can't have."));
        }
    }
    enterprise::forget(&p.id);
    Ok(())
}

#[tauri::command(async)]
pub fn set_cloud_enterprise(state: AppStateRef, id: String, settings_in: EntInput) -> Result<CloudView, String> {
    let name = {
        let conn = state.db.lock().unwrap();
        let mut s = settings(&conn);
        let p = s.providers.iter_mut().find(|p| p.id == id).ok_or("Unknown provider")?;
        apply(p, settings_in)?;
        if !p.has_credentials() {
            p.enabled = false;
        }
        let name = p.name.clone();
        save(&conn, &s)?;
        name
    };
    state.log("settings", &format!("Changed how {name} is reached"));
    Ok(cloud_view(state))
}

/// Starts an Entra device-code sign-in: the window shows the code and the
/// address to enter it at, and `cloud:signed_in` follows once it's done.
#[tauri::command]
pub async fn cloud_entra_start(app: tauri::AppHandle, state: AppStateRef<'_>, id: String) -> Result<enterprise::DeviceCode, String> {
    let c = state.cipher()?;
    let p = settings(&state.db.lock().unwrap()).providers.into_iter().find(|p| p.id == id).ok_or("Unknown provider")?;
    if p.kind != Kind::Azure || p.ent.auth != Auth::Entra {
        return Err("Choose Microsoft Entra ID sign-in for this provider first.".into());
    }
    if let Some(why) = enterprise::problem(&p) {
        return Err(why);
    }
    let client = via(&state, &wire_of(&p, &c))?;
    let code = enterprise::entra_start(&client, &p.ent).await?;
    state.log("network", &format!("Started a Microsoft Entra sign-in for {}", p.name));
    let state = state.inner().clone();
    let started = code.clone();
    tauri::async_runtime::spawn(async move {
        use tauri::Emitter;
        let r = wait_for_entra(&state, &client, &p, &started).await;
        match r {
            Ok(account) => {
                state.log("settings", &format!("Signed in to {} with Microsoft Entra ID; the sign-in is kept encrypted", p.name));
                app.emit("cloud:signed_in", json!({ "id": p.id, "name": p.name, "account": account })).ok();
            }
            Err(e) => {
                app.emit("cloud:sign_in_failed", json!({ "id": p.id, "error": e })).ok();
            }
        }
    });
    Ok(code)
}

pub(super) async fn wait_for_entra(state: &AppState, client: &reqwest::Client, p: &Provider, code: &enterprise::DeviceCode) -> Result<String, String> {
    let until = std::time::Instant::now() + Duration::from_secs(code.expires_in.min(1800));
    let mut every = code.interval;
    while std::time::Instant::now() < until {
        tokio::time::sleep(Duration::from_secs(every)).await;
        match enterprise::entra_poll(client, &p.ent, &code.device_code).await? {
            enterprise::Poll::Waiting => {}
            enterprise::Poll::SlowDown => every += 5,
            enterprise::Poll::Done { refresh, access, secs, account } => {
                let c = state.cipher()?;
                let conn = state.db.lock().unwrap();
                let mut s = settings(&conn);
                let x = s.providers.iter_mut().find(|x| x.id == p.id).ok_or("The provider was removed.")?;
                x.ent.token = c.encrypt(&refresh);
                x.ent.account = account.clone();
                x.enabled = true;
                save(&conn, &s)?;
                enterprise::remember(&p.id, access, secs);
                return Ok(account);
            }
        }
    }
    Err("The sign-in code ran out. Start again.".into())
}

#[tauri::command(async)]
pub fn cloud_entra_sign_out(state: AppStateRef, id: String) -> Result<CloudView, String> {
    {
        let conn = state.db.lock().unwrap();
        let mut s = settings(&conn);
        let p = s.providers.iter_mut().find(|p| p.id == id).ok_or("Unknown provider")?;
        p.ent.token.clear();
        p.ent.account.clear();
        p.enabled = p.enabled && p.has_credentials();
        save(&conn, &s)?;
    }
    enterprise::forget(&id);
    state.log("settings", "Signed out of Microsoft Entra ID for a cloud provider");
    Ok(cloud_view(state))
}

/// Header names a gateway may get; ones that carry the connection itself
/// or the provider's own auth are kept out so a typo can't break either.
pub fn header_ok(name: &str) -> bool {
    let n = name.trim().to_ascii_lowercase();
    !n.is_empty()
        && n.len() <= 64
        && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        && !matches!(n.as_str(), "host" | "content-length" | "content-type" | "transfer-encoding" | "connection" | "authorization" | "api-key" | "x-api-key" | "cookie" | "x-amz-date" | "x-amz-security-token")
}

/// Extra headers for a provider: sent with each request, sealed like keys.
/// Values left empty keep what was saved (the window never sees them).
#[tauri::command(async)]
pub fn set_cloud_headers(state: AppStateRef, id: String, headers: Vec<(String, String)>) -> Result<CloudView, String> {
    let c = state.cipher()?;
    let name = {
        let conn = state.db.lock().unwrap();
        let mut s = settings(&conn);
        let p = s.providers.iter_mut().find(|p| p.id == id).ok_or("Unknown provider")?;
        let old: serde_json::Map<String, Value> = if p.headers.is_empty() { Default::default() } else { serde_json::from_str(&c.decrypt(&p.headers)?).unwrap_or_default() };
        let mut map = serde_json::Map::new();
        for (k, v) in headers {
            let k = k.trim().to_string();
            if !header_ok(&k) {
                return Err(format!("\"{k}\" can't be used as an extra header."));
            }
            if v.contains(['\r', '\n']) {
                return Err("A header value must be on one line.".into());
            }
            let v = if v.is_empty() { old.get(&k).cloned().unwrap_or(Value::String(String::new())) } else { Value::String(v.trim().to_string()) };
            map.insert(k, v);
        }
        p.headers = if map.is_empty() { String::new() } else { c.encrypt(&Value::Object(map).to_string()) };
        let name = p.name.clone();
        save(&conn, &s)?;
        name
    };
    state.log("settings", &format!("Changed the extra headers sent to {name}"));
    Ok(cloud_view(state))
}

/// A company CA certificate (PEM) trusted for one provider ("" removes it).
#[tauri::command(async)]
pub fn set_cloud_ca(state: AppStateRef, id: String, pem: String) -> Result<CloudView, String> {
    let pem = pem.trim().to_string();
    if !pem.is_empty() {
        net::ca_certs(&pem)?;
    }
    let name = {
        let conn = state.db.lock().unwrap();
        let mut s = settings(&conn);
        let p = s.providers.iter_mut().find(|p| p.id == id).ok_or("Unknown provider")?;
        if p.id == crate::policy::GATEWAY_ID {
            return Err("Your organization set this gateway's certificate.".into());
        }
        p.ca_cert = pem.clone();
        let name = p.name.clone();
        save(&conn, &s)?;
        name
    };
    state.log("settings", &format!("{name}: {}", if pem.is_empty() { "no company certificate" } else { "trusts a company certificate" }));
    Ok(cloud_view(state))
}
