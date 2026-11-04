// SPDX-License-Identifier: AGPL-3.0-only
//! Azure OpenAI / AI Foundry and Google Vertex AI, with the sign-ins they
//! use: a Microsoft Entra ID device code, a Google service account or
//! Google's application default credentials. Bedrock is in bedrock.rs.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{settings, save, CloudModel, Failed, Kind, Provider, Target};
use crate::AppState;

/// Azure's GA api-version for deployment addresses, used when none is set.
pub const AZURE_VERSION: &str = "2024-10-21";
const ENTRA: &str = "https://login.microsoftonline.com";
const ENTRA_SCOPE: &str = "https://cognitiveservices.azure.com/.default offline_access";
const GOOGLE_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
const GOOGLE_TOKEN: &str = "https://oauth2.googleapis.com/token";

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Auth {
    /// The provider's key (Azure's api-key, AWS access keys or a Bedrock API
    /// key, a Google service-account JSON), sealed like every other key.
    #[default]
    Key,
    /// Microsoft Entra ID, signed in with a device code.
    Entra,
    /// A profile in the AWS files under the user's home folder.
    Profile,
    /// gcloud's application default credentials.
    Adc,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Ent {
    pub auth: Auth,
    /// Azure deployment addresses only (empty: AZURE_VERSION).
    pub api_version: String,
    /// Bedrock's AWS region, or Vertex's location ("global", "us-east5").
    pub region: String,
    /// Vertex's Google Cloud project id.
    pub project: String,
    /// The AWS profile ("default" when empty).
    pub profile: String,
    /// Entra directory (tenant) and the company's app registration.
    pub tenant: String,
    pub client_id: String,
    /// Entra's sign-in address for a national cloud (empty: the public one).
    pub authority: String,
    /// Entra's refresh token, sealed.
    pub token: String,
    /// Who signed in with Entra, shown in Settings.
    pub account: String,
}

impl Ent {
    pub fn keyless(&self, kind: Kind) -> bool {
        match (kind, self.auth) {
            (Kind::Azure, Auth::Entra) => !self.token.is_empty(),
            (Kind::Bedrock, Auth::Profile) | (Kind::Vertex, Auth::Adc) => true,
            _ => false,
        }
    }

    fn authority(&self) -> String {
        let a = self.authority.trim().trim_end_matches('/');
        let a = if a.is_empty() { ENTRA } else { a };
        let tenant = if self.tenant.trim().is_empty() { "organizations" } else { self.tenant.trim() };
        format!("{a}/{tenant}/oauth2/v2.0")
    }
}

/// Names that go into addresses: letters, digits, dots, dashes and
/// underscores only, so a typed value can't change where a request goes.
pub fn plain_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// What's missing or wrong before a provider of these kinds can be used.
pub fn problem(p: &Provider) -> Option<String> {
    let e = &p.ent;
    match p.kind {
        Kind::Azure if p.base_url.is_empty() => Some("Add the resource's address, like https://NAME.openai.azure.com.".into()),
        Kind::Azure if e.auth == Auth::Entra && !plain_name(&e.client_id) => Some("Entra sign-in needs the app registration's client id from your IT team.".into()),
        Kind::Azure if !e.api_version.is_empty() && !plain_name(&e.api_version) => Some("That api-version isn't one Azure uses (like 2024-10-21).".into()),
        Kind::Bedrock if !e.region.is_empty() && !plain_name(&e.region) => Some("Write the AWS region like us-east-1.".into()),
        Kind::Vertex if !plain_name(&e.project) => Some("Add the Google Cloud project id.".into()),
        Kind::Vertex if !plain_name(&vertex_location(e)) => Some("Write the location like us-east5 or global.".into()),
        _ => None,
    }
}

fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub(super) fn path_part(s: &str) -> String {
    enc(s)
}

// ---------- Azure ----------

/// Foundry's and Azure OpenAI's v1 API takes the deployment as the model
/// and needs no api-version; older resources name it in the address.
fn azure_v1(base: &str) -> bool {
    base.trim_end_matches('/').ends_with("/openai/v1")
}

pub fn azure_chat_url(base: &str, deployment: &str, version: &str) -> String {
    let base = base.trim_end_matches('/');
    if azure_v1(base) {
        format!("{base}/chat/completions")
    } else {
        let v = if version.is_empty() { AZURE_VERSION } else { version };
        format!("{base}/openai/deployments/{}/chat/completions?api-version={}", enc(deployment), enc(v))
    }
}

// ---------- Vertex ----------

fn vertex_location(e: &Ent) -> String {
    if e.region.trim().is_empty() { "global".into() } else { e.region.trim().to_string() }
}

fn vertex_root(base: &str, e: &Ent) -> String {
    let loc = vertex_location(e);
    let host = match base.trim_end_matches('/') {
        "" if loc == "global" => "https://aiplatform.googleapis.com".to_string(),
        "" => format!("https://{loc}-aiplatform.googleapis.com"),
        b => b.to_string(),
    };
    format!("{host}/v1/projects/{}/locations/{}", enc(e.project.trim()), enc(&loc))
}

pub fn vertex_claude(model: &str) -> bool {
    model.starts_with("claude")
}

pub(super) fn vertex_claude_url(t: &Target) -> String {
    format!("{}/publishers/anthropic/models/{}:streamRawPredict", vertex_root(&t.base_url, &t.ent), enc(&t.model.id))
}

/// Vertex's OpenAI-compatible endpoint names Gemini "google/<model>".
pub(super) fn openai_model(t: &Target) -> String {
    match t.kind {
        Kind::Vertex if !t.model.id.contains('/') => format!("google/{}", t.model.id),
        _ => t.model.id.clone(),
    }
}

/// Models Vertex offers that people usually want; Vertex has no list that
/// covers both publishers, and the ids can be changed in Settings.
pub fn vertex_suggestions() -> Vec<CloudModel> {
    let m = |id: &str, name: &str, ctx: u32, out: u32, pin: f64, pout: f64| CloudModel { id: id.into(), name: name.into(), ctx, max_output: out, vision: true, price_in: Some(pin), price_out: Some(pout) };
    vec![
        m("gemini-2.5-pro", "Gemini 2.5 Pro (Vertex)", 1_048_576, 65_536, 1.25, 10.0),
        m("gemini-2.5-flash", "Gemini 2.5 Flash (Vertex)", 1_048_576, 65_536, 0.3, 2.5),
        m("claude-sonnet-4-6", "Claude Sonnet 4.6 (Vertex)", 200_000, 64_000, 3.0, 15.0),
        m("claude-opus-4-6", "Claude Opus 4.6 (Vertex)", 200_000, 32_000, 5.0, 25.0),
        m("claude-haiku-4-5", "Claude Haiku 4.5 (Vertex)", 200_000, 64_000, 1.0, 5.0),
    ]
}

// ---------- requests ----------

/// The chat completions request for OpenAI-style kinds, with their auth.
pub(super) async fn openai_request(client: &reqwest::Client, t: &Target) -> Result<reqwest::RequestBuilder, Failed> {
    Ok(match t.kind {
        Kind::Azure => {
            let req = client.post(azure_chat_url(&t.base_url, &t.model.id, &t.ent.api_version));
            if t.ent.auth == Auth::Entra { req.bearer_auth(bearer(client, t).await?) } else { req.header("api-key", &t.key) }
        }
        Kind::Vertex => client.post(format!("{}/endpoints/openapi/chat/completions", vertex_root(&t.base_url, &t.ent))).bearer_auth(bearer(client, t).await?),
        _ => client.post(format!("{}/chat/completions", t.base_url)).bearer_auth(&t.key),
    })
}

pub(super) async fn bearer(client: &reqwest::Client, t: &Target) -> Result<String, Failed> {
    token(&t.state, client, &t.provider_id, t.kind, &t.ent, &t.key).await.map_err(Failed::fatal)
}

/// Access tokens by provider, until a minute before they run out.
static TOKENS: LazyLock<Mutex<HashMap<String, (String, Instant)>>> = LazyLock::new(Default::default);

/// A bearer token for Entra or Google, fetched again when it's close to
/// running out.
pub async fn token(state: &AppState, client: &reqwest::Client, pid: &str, kind: Kind, e: &Ent, key: &str) -> Result<String, String> {
    if let Some((t, until)) = TOKENS.lock().unwrap().get(pid) {
        if Instant::now() < *until {
            return Ok(t.clone());
        }
    }
    let (access, secs) = match (kind, e.auth) {
        (Kind::Azure, Auth::Entra) => {
            let c = state.cipher()?;
            let refresh = if e.token.is_empty() { String::new() } else { c.decrypt(&e.token)? };
            if refresh.is_empty() {
                return Err("Sign in with Microsoft Entra ID in Settings → Cloud models first.".into());
            }
            let v = entra_refresh(client, e, &refresh).await?;
            if let Some(new) = v["refresh_token"].as_str().filter(|n| *n != refresh) {
                let conn = state.db.lock().unwrap();
                let mut s = settings(&conn);
                if let Some(p) = s.providers.iter_mut().find(|p| p.id == pid) {
                    p.ent.token = c.encrypt(new);
                    save(&conn, &s)?;
                }
            }
            access_of(&v)?
        }
        (Kind::Vertex, Auth::Adc) => google_token(client, &adc_file()?).await?,
        (Kind::Vertex, _) => google_token(client, key).await?,
        _ => return Ok(key.to_string()),
    };
    let until = Instant::now() + Duration::from_secs(secs.saturating_sub(60).max(30));
    TOKENS.lock().unwrap().insert(pid.to_string(), (access.clone(), until));
    Ok(access)
}

/// Forgets a provider's token (signed out, or its settings changed).
pub fn forget(pid: &str) {
    TOKENS.lock().unwrap().remove(pid);
}

fn access_of(v: &Value) -> Result<(String, u64), String> {
    let t = v["access_token"].as_str().filter(|t| !t.is_empty()).ok_or("The sign-in service gave no token.")?;
    Ok((t.to_string(), v["expires_in"].as_u64().or_else(|| v["expires_in"].as_str().and_then(|s| s.parse().ok())).unwrap_or(3600)))
}

async fn post_form(client: &reqwest::Client, url: &str, form: &[(&str, &str)]) -> Result<Value, String> {
    let resp = client.post(url).form(form).timeout(Duration::from_secs(60)).send().await.map_err(|e| format!("Couldn't reach the sign-in service: {e}"))?;
    let ok = resp.status().is_success();
    let v: Value = resp.json().await.map_err(|e| format!("The sign-in service answered oddly: {e}"))?;
    if ok {
        Ok(v)
    } else {
        let why = v["error_description"].as_str().or(v["error"].as_str()).unwrap_or("refused").lines().next().unwrap_or("").to_string();
        Err(format!("Sign-in failed: {why}"))
    }
}

// ---------- Entra device code ----------

#[derive(Debug, Clone, Serialize)]
pub struct DeviceCode {
    pub user_code: String,
    pub verification_uri: String,
    pub message: String,
    #[serde(skip)]
    pub device_code: String,
    #[serde(skip)]
    pub interval: u64,
    pub expires_in: u64,
}

pub async fn entra_start(client: &reqwest::Client, e: &Ent) -> Result<DeviceCode, String> {
    let v = post_form(client, &format!("{}/devicecode", e.authority()), &[("client_id", e.client_id.trim()), ("scope", ENTRA_SCOPE)]).await?;
    let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
    if s("device_code").is_empty() {
        return Err("Entra didn't give a sign-in code.".into());
    }
    Ok(DeviceCode { user_code: s("user_code"), verification_uri: s("verification_uri"), message: s("message"), device_code: s("device_code"), interval: v["interval"].as_u64().unwrap_or(5).max(1), expires_in: v["expires_in"].as_u64().unwrap_or(900) })
}

pub enum Poll {
    Waiting,
    SlowDown,
    Done { refresh: String, access: String, secs: u64, account: String },
}

pub async fn entra_poll(client: &reqwest::Client, e: &Ent, device_code: &str) -> Result<Poll, String> {
    let resp = client
        .post(format!("{}/token", e.authority()))
        .form(&[("grant_type", "urn:ietf:params:oauth:grant-type:device_code"), ("client_id", e.client_id.trim()), ("device_code", device_code)])
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| format!("Couldn't reach Entra: {e}"))?;
    let v: Value = resp.json().await.map_err(|e| e.to_string())?;
    match v["error"].as_str() {
        Some("authorization_pending") => return Ok(Poll::Waiting),
        Some("slow_down") => return Ok(Poll::SlowDown),
        Some(other) => return Err(format!("Sign-in stopped: {}", v["error_description"].as_str().unwrap_or(other).lines().next().unwrap_or(other))),
        None => {}
    }
    let (access, secs) = access_of(&v)?;
    let refresh = v["refresh_token"].as_str().ok_or("Entra gave no refresh token; ask IT to allow offline_access.")?.to_string();
    Ok(Poll::Done { refresh, access, secs, account: account_of(v["id_token"].as_str().unwrap_or("")) })
}

async fn entra_refresh(client: &reqwest::Client, e: &Ent, refresh: &str) -> Result<Value, String> {
    post_form(client, &format!("{}/token", e.authority()), &[("grant_type", "refresh_token"), ("client_id", e.client_id.trim()), ("refresh_token", refresh), ("scope", ENTRA_SCOPE)]).await
}

/// The signed-in name from an id token (read only for display; Entra
/// already checked it, and nothing is decided on it).
fn account_of(id_token: &str) -> String {
    let Some(body) = id_token.split('.').nth(1) else { return String::new() };
    let Ok(raw) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(body.trim_end_matches('=')) else { return String::new() };
    let v: Value = serde_json::from_slice(&raw).unwrap_or_default();
    v["preferred_username"].as_str().or(v["upn"].as_str()).or(v["name"].as_str()).unwrap_or_default().to_string()
}

pub fn remember(pid: &str, access: String, secs: u64) {
    TOKENS.lock().unwrap().insert(pid.to_string(), (access, Instant::now() + Duration::from_secs(secs.saturating_sub(60).max(30))));
}

// ---------- Google ----------

/// gcloud's application default credentials file.
pub fn adc_path() -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os("GOOGLE_APPLICATION_CREDENTIALS").filter(|p| !p.is_empty()) {
        return Some(p.into());
    }
    let base = if cfg!(windows) { std::env::var_os("APPDATA").map(std::path::PathBuf::from) } else { std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")) };
    base.map(|b| b.join("gcloud").join("application_default_credentials.json"))
}

fn adc_file() -> Result<String, String> {
    let path = adc_path().ok_or("Couldn't find gcloud's settings folder.")?;
    std::fs::read_to_string(&path).map_err(|_| "No Google application default credentials on this PC. Run `gcloud auth application-default login`, or paste a service-account key instead.".into())
}

/// A token from a service-account key or gcloud's authorized-user file.
pub async fn google_token(client: &reqwest::Client, creds: &str) -> Result<(String, u64), String> {
    let v: Value = serde_json::from_str(creds).map_err(|_| "The Google credentials aren't a JSON key file.".to_string())?;
    let uri = v["token_uri"].as_str().unwrap_or(GOOGLE_TOKEN);
    match v["type"].as_str() {
        Some("service_account") => {
            let jwt = service_jwt(&v, chrono::Utc::now().timestamp())?;
            access_of(&post_form(client, uri, &[("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"), ("assertion", &jwt)]).await?)
        }
        Some("authorized_user") => {
            let s = |k: &str| v[k].as_str().unwrap_or_default();
            access_of(&post_form(client, uri, &[("grant_type", "refresh_token"), ("client_id", s("client_id")), ("client_secret", s("client_secret")), ("refresh_token", s("refresh_token"))]).await?)
        }
        _ => Err("Use a service-account key, or gcloud's own sign-in (application default credentials).".into()),
    }
}

pub(super) fn jwt_claims(v: &Value, now: i64) -> Result<(String, String), String> {
    let b64 = |x: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(x);
    let email = v["client_email"].as_str().ok_or("The key file has no client_email.")?;
    let head = json!({ "alg": "RS256", "typ": "JWT", "kid": v["private_key_id"] });
    let claims = json!({ "iss": email, "scope": GOOGLE_SCOPE, "aud": v["token_uri"].as_str().unwrap_or(GOOGLE_TOKEN), "iat": now, "exp": now + 3600 });
    Ok((format!("{}.{}", b64(head.to_string().as_bytes()), b64(claims.to_string().as_bytes())), email.to_string()))
}

fn service_jwt(v: &Value, now: i64) -> Result<String, String> {
    use ring::signature::{RsaKeyPair, RSA_PKCS1_SHA256};
    let (unsigned, _) = jwt_claims(v, now)?;
    let pem = v["private_key"].as_str().ok_or("The key file has no private_key.")?;
    let body: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
    let der = base64::engine::general_purpose::STANDARD.decode(body.trim()).map_err(|_| "The key file's private key couldn't be read.".to_string())?;
    let pair = RsaKeyPair::from_pkcs8(&der).map_err(|_| "The key file's private key isn't an RSA key.".to_string())?;
    let mut sig = vec![0; pair.public().modulus_len()];
    pair.sign(&RSA_PKCS1_SHA256, &ring::rand::SystemRandom::new(), unsigned.as_bytes(), &mut sig).map_err(|_| "Couldn't sign the Google sign-in.".to_string())?;
    Ok(format!("{unsigned}.{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sig)))
}

// ---------- model lists ----------

pub(super) async fn list_models(state: Option<&AppState>, p: &Provider, key: &str, client: &reqwest::Client, w: &super::Wire) -> Result<Vec<CloudModel>, String> {
    let base = p.base_url.trim_end_matches('/');
    match p.kind {
        Kind::Vertex => Ok(vertex_suggestions()),
        Kind::Bedrock => super::bedrock::list_models(p, key, client, w).await,
        _ => {
            let url = if azure_v1(base) { format!("{base}/models") } else { format!("{base}/openai/deployments?api-version=2022-12-01") };
            let mut req = w.add(client.get(url));
            req = match (p.ent.auth, state) {
                (Auth::Entra, Some(state)) => req.bearer_auth(token(state, client, &p.id, p.kind, &p.ent, key).await?),
                (Auth::Entra, None) => return Err("Sign in with Entra in the app first.".into()),
                _ => req.header("api-key", key),
            };
            let none = || format!("{} didn't list its deployments. Type each deployment's name under Models instead.", p.name);
            let resp = req.send().await.map_err(|e| e.to_string())?;
            if !resp.status().is_success() {
                return Err(none());
            }
            let v: Value = resp.json().await.map_err(|_| none())?;
            let out: Vec<CloudModel> = v["data"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|m| {
                    let id = m["id"].as_str()?.to_string();
                    let name = m["model"].as_str().map(|base| format!("{id} ({base})")).unwrap_or_else(|| id.clone());
                    Some(CloudModel { id, name, ctx: 128_000, max_output: 16_000, vision: true, price_in: None, price_out: None })
                })
                .collect();
            if out.is_empty() { Err(none()) } else { Ok(out) }
        }
    }
}
