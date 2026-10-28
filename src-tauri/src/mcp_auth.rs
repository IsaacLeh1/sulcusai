// SPDX-License-Identifier: AGPL-3.0-only
//! Signing in to a connector at an address, as the MCP specification
//! (2025-06-18) asks: the connector names its sign-in server (RFC 9728),
//! that server describes itself (RFC 8414), SulcusAI registers itself there
//! (RFC 7591), and the browser sign-in uses PKCE with the connector's
//! address as the resource it's for (RFC 8707). A sign-in only starts from
//! the person's own button; a model's call that needs one fails instead.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::oauth;

/// What's kept (encrypted, with the connector) after signing in.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Auth {
    pub issuer: String,
    pub client_id: String,
    pub token_endpoint: String,
    pub access_token: String,
    pub refresh_token: String,
    /// When the access token runs out (ms); 0 when the server didn't say.
    pub expires_at: i64,
    /// Who signed in, when the sign-in server says.
    pub account: String,
}

impl Auth {
    /// The sign-in without its tokens, for the window.
    pub fn shown(&self) -> Auth {
        Auth { access_token: String::new(), refresh_token: String::new(), ..self.clone() }
    }

    fn running_out(&self) -> bool {
        self.expires_at != 0 && self.expires_at < crate::db::now_ms() + 60_000
    }
}

/// A sign-in server, from its metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct Server {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: Option<String>,
    pub scope: Option<String>,
}

/// `resource_metadata="…"` from a 401's WWW-Authenticate.
pub fn resource_metadata(header: &str) -> Option<String> {
    let at = header.find("resource_metadata=")? + "resource_metadata=".len();
    let rest = &header[at..];
    let value = match rest.strip_prefix('"') {
        Some(quoted) => quoted.split('"').next()?,
        None => rest.split([',', ' ']).next()?,
    };
    (!value.is_empty()).then(|| value.to_string())
}

fn origin(url: &str) -> Result<(String, String), String> {
    let u = reqwest::Url::parse(url).map_err(|_| format!("{url} isn't a web address."))?;
    let host = u.host_str().ok_or_else(|| format!("{url} has no host."))?;
    let origin = match u.port() {
        Some(p) => format!("{}://{host}:{p}", u.scheme()),
        None => format!("{}://{host}", u.scheme()),
    };
    Ok((origin, u.path().trim_end_matches('/').to_string()))
}

/// Where a well-known document for `base` may be: inserted before the
/// path (RFC 8414 and 9728), then at the root, then (for OpenID
/// Connect) after the path.
fn well_known(base: &str, name: &str) -> Result<Vec<String>, String> {
    let (origin, path) = origin(base)?;
    let mut all = Vec::new();
    if !path.is_empty() {
        all.push(format!("{origin}/.well-known/{name}{path}"));
    }
    all.push(format!("{origin}/.well-known/{name}"));
    if !path.is_empty() && name == "openid-configuration" {
        all.push(format!("{origin}{path}/.well-known/{name}"));
    }
    Ok(all)
}

async fn get_json(http: &reqwest::Client, url: &str) -> Option<Value> {
    let resp = tokio::time::timeout(Duration::from_secs(20), http.get(url).header("Accept", "application/json").send()).await.ok()?.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json().await.ok()
}

/// The connector's sign-in server: from the 401 it answers with, else the
/// well-known places, else (servers from before resource metadata) its
/// own origin.
pub async fn discover(http: &reqwest::Client, url: &str, headers: &HashMap<String, String>) -> Result<Server, String> {
    let mut probe = http.post(url).header("Accept", "application/json, text/event-stream").json(&json!({
        "jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "SulcusAI", "version": env!("CARGO_PKG_VERSION") } }
    }));
    for (k, v) in headers {
        probe = probe.header(k, v);
    }
    let named = match tokio::time::timeout(Duration::from_secs(20), probe.send()).await {
        Ok(Ok(r)) if r.status().as_u16() == 401 => r.headers().get("www-authenticate").and_then(|v| v.to_str().ok()).and_then(resource_metadata),
        Ok(Ok(_)) => None,
        Ok(Err(e)) => return Err(format!("Couldn't reach the connector at {}: {e}", crate::host_of(url))),
        Err(_) => return Err(format!("The connector at {} didn't answer.", crate::host_of(url))),
    };
    let candidates = match named {
        Some(m) => vec![m],
        None => well_known(url, "oauth-protected-resource")?,
    };
    let mut resource = None;
    for c in candidates {
        if let Some(v) = get_json(http, &c).await {
            resource = Some(v);
            break;
        }
    }
    let issuer = match resource.as_ref().and_then(|r| r["authorization_servers"][0].as_str()) {
        Some(i) => i.trim_end_matches('/').to_string(),
        None => origin(url)?.0,
    };
    let scope = resource
        .as_ref()
        .and_then(|r| r["scopes_supported"].as_array())
        .map(|s| s.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" "))
        .filter(|s| !s.is_empty());
    let mut places = well_known(&issuer, "oauth-authorization-server")?;
    places.extend(well_known(&issuer, "openid-configuration")?);
    for place in places {
        let Some(m) = get_json(http, &place).await else { continue };
        let (Some(auth), Some(token)) = (m["authorization_endpoint"].as_str(), m["token_endpoint"].as_str()) else { continue };
        // PKCE with S256 is required; a server that says it can't do it is refused.
        if let Some(methods) = m["code_challenge_methods_supported"].as_array() {
            if !methods.iter().any(|x| x == "S256") {
                return Err(format!("{issuer} doesn't support the safe sign-in SulcusAI uses (PKCE with S256)."));
            }
        }
        return Ok(Server {
            issuer: m["issuer"].as_str().unwrap_or(&issuer).trim_end_matches('/').to_string(),
            authorization_endpoint: auth.to_string(),
            token_endpoint: token.to_string(),
            registration_endpoint: m["registration_endpoint"].as_str().map(str::to_string),
            scope: scope.clone(),
        });
    }
    Err(format!("The connector at {} didn't say where to sign in.", crate::host_of(url)))
}

/// Registers SulcusAI with the sign-in server; returns its client id.
pub async fn register(http: &reqwest::Client, server: &Server, redirect: &str) -> Result<String, String> {
    let endpoint = server
        .registration_endpoint
        .as_deref()
        .ok_or_else(|| format!("{} doesn't let apps register themselves, so SulcusAI can't sign in to it.", server.issuer))?;
    let body = json!({
        "client_name": "SulcusAI",
        "redirect_uris": [redirect],
        "token_endpoint_auth_method": "none",
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
    });
    let resp = http.post(endpoint).json(&body).send().await.map_err(|e| format!("Couldn't register with {}: {e}", server.issuer))?;
    let status = resp.status();
    let v: Value = resp.json().await.unwrap_or_default();
    match v["client_id"].as_str() {
        Some(id) if status.is_success() => Ok(id.to_string()),
        _ => Err(format!("{} turned down SulcusAI's registration ({status}): {}", server.issuer, v["error_description"].as_str().or(v["error"].as_str()).unwrap_or(""))),
    }
}

pub fn authorize_url(server: &Server, client_id: &str, redirect: &str, challenge: &str, state: &str, resource: &str) -> String {
    let mut pairs = vec![
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", redirect),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        ("state", state),
        ("resource", resource),
    ];
    if let Some(s) = &server.scope {
        pairs.push(("scope", s));
    }
    let sep = if server.authorization_endpoint.contains('?') { '&' } else { '?' };
    format!("{}{sep}{}", server.authorization_endpoint, oauth::query(&pairs))
}

fn auth_from(v: &Value, server: &Server, client_id: &str, old_refresh: &str) -> Auth {
    Auth {
        issuer: server.issuer.clone(),
        client_id: client_id.to_string(),
        token_endpoint: server.token_endpoint.clone(),
        access_token: v["access_token"].as_str().unwrap_or("").to_string(),
        refresh_token: v["refresh_token"].as_str().unwrap_or(old_refresh).to_string(),
        expires_at: v["expires_in"].as_i64().map_or(0, |s| crate::db::now_ms() + s * 1000),
        account: v["id_token"].as_str().and_then(oauth::email_from_id_token).unwrap_or_default(),
    }
}

/// The whole sign-in, in the person's browser. Each sign-in registers
/// afresh: the address the browser comes back to has a new port each time,
/// and sign-in servers hold a registration to its exact address.
pub async fn sign_in(state: &crate::AppState, url: &str, headers: &HashMap<String, String>) -> Result<Auth, String> {
    let http = crate::net::service_client(state.settings().connectivity)?;
    let server = discover(&http, url, headers).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect = format!("http://127.0.0.1:{port}/callback");
    let client_id = register(&http, &server, &redirect).await?;
    let (verifier, challenge) = oauth::pkce();
    let st = uuid::Uuid::new_v4().simple().to_string();
    oauth::open_in_browser(&state.db.lock().unwrap(), &authorize_url(&server, &client_id, &redirect, &challenge, &st, url))?;
    state.log("network", &format!("Opened the sign-in for the connector at {} in the browser", crate::host_of(url)));
    let code = oauth::wait_for_code(listener, &st, Duration::from_secs(300)).await?;
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", redirect.as_str()),
        ("client_id", client_id.as_str()),
        ("code_verifier", verifier.as_str()),
        ("resource", url),
    ];
    let v = oauth::token_request(&http, &server.token_endpoint, &form).await?;
    state.log("privacy", &format!("Signed in to the connector at {}", crate::host_of(url)));
    Ok(auth_from(&v, &server, &client_id, ""))
}

/// A new access token from the refresh token.
pub async fn refresh(http: &reqwest::Client, url: &str, a: &Auth) -> Result<Auth, String> {
    if a.refresh_token.is_empty() {
        return Err("The sign-in can't be renewed.".into());
    }
    let form = [("grant_type", "refresh_token"), ("refresh_token", a.refresh_token.as_str()), ("client_id", a.client_id.as_str()), ("resource", url)];
    let v = oauth::token_request(http, &a.token_endpoint, &form).await?;
    let server = Server { issuer: a.issuer.clone(), authorization_endpoint: String::new(), token_endpoint: a.token_endpoint.clone(), registration_endpoint: None, scope: None };
    let mut fresh = auth_from(&v, &server, &a.client_id, &a.refresh_token);
    if fresh.account.is_empty() {
        fresh.account = a.account.clone();
    }
    Ok(fresh)
}

/// Renewed when it's about to run out; None when it's still good.
pub async fn renewed_if_due(state: &crate::AppState, url: &str, a: &Auth) -> Result<Option<Auth>, String> {
    if !a.running_out() {
        return Ok(None);
    }
    let http = crate::net::service_client(state.settings().connectivity)?;
    refresh(&http, url, a).await.map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_metadata_address_is_read_from_the_header() {
        assert_eq!(resource_metadata(r#"Bearer realm="x", resource_metadata="https://a.com/.well-known/oauth-protected-resource/mcp""#).as_deref(), Some("https://a.com/.well-known/oauth-protected-resource/mcp"));
        assert_eq!(resource_metadata("Bearer resource_metadata=https://a.com/m, error=\"invalid_token\"").as_deref(), Some("https://a.com/m"));
        assert_eq!(resource_metadata("Bearer realm=\"x\""), None);
    }

    #[test]
    fn well_known_places_go_before_the_path_first() {
        assert_eq!(
            well_known("https://auth.example.com/tenant1", "oauth-authorization-server").unwrap(),
            ["https://auth.example.com/.well-known/oauth-authorization-server/tenant1", "https://auth.example.com/.well-known/oauth-authorization-server"]
        );
        assert_eq!(well_known("https://auth.example.com", "openid-configuration").unwrap(), ["https://auth.example.com/.well-known/openid-configuration"]);
    }

    /// The whole chain against a local server, up to where the browser
    /// would open: the 401 names the metadata, the metadata names the
    /// sign-in server, it describes itself under its path, registration
    /// gives a client id, and a refresh token gives a new access token.
    #[tokio::test]
    async fn discovery_registration_and_refresh() {
        use axum::http::{HeaderMap, StatusCode};
        use axum::response::IntoResponse;
        use axum::routing::{get, post};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let b = base.clone();
        let app = axum::Router::new()
            .route(
                "/mcp",
                post(move |_: HeaderMap| {
                    let b = b.clone();
                    async move { (StatusCode::UNAUTHORIZED, [("www-authenticate", format!(r#"Bearer resource_metadata="{b}/.well-known/oauth-protected-resource/mcp""#))]).into_response() }
                }),
            )
            .route("/.well-known/oauth-protected-resource/mcp", get({
                let b = base.clone();
                move || async move { axum::Json(json!({ "resource": format!("{b}/mcp"), "authorization_servers": [format!("{b}/auth")], "scopes_supported": ["read", "write"] })) }
            }))
            .route("/.well-known/oauth-authorization-server/auth", get({
                let b = base.clone();
                move || async move {
                    axum::Json(json!({ "issuer": format!("{b}/auth"), "authorization_endpoint": format!("{b}/auth/authorize"), "token_endpoint": format!("{b}/auth/token"),
                        "registration_endpoint": format!("{b}/auth/register"), "code_challenge_methods_supported": ["S256"] }))
                }
            }))
            .route("/auth/register", post(|axum::Json(r): axum::Json<Value>| async move {
                assert_eq!(r["token_endpoint_auth_method"], "none");
                axum::Json(json!({ "client_id": "c-123" }))
            }))
            .route("/auth/token", post(|body: String| async move {
                assert!(body.contains("grant_type=refresh_token") && body.contains("client_id=c-123") && body.contains("resource="), "{body}");
                axum::Json(json!({ "access_token": "new", "expires_in": 3600 }))
            }));
        tokio::spawn(async move { axum::serve(listener, app).await.ok() });
        let http = crate::net::local_client();
        let url = format!("{base}/mcp");
        let server = discover(&http, &url, &HashMap::new()).await.unwrap();
        assert_eq!(server.issuer, format!("{base}/auth"));
        assert_eq!(server.token_endpoint, format!("{base}/auth/token"));
        assert_eq!(server.scope.as_deref(), Some("read write"));
        assert_eq!(register(&http, &server, "http://127.0.0.1:5/callback").await.unwrap(), "c-123");
        let link = authorize_url(&server, "c-123", "http://127.0.0.1:5/callback", "ch", "st", &url);
        assert!(link.starts_with(&format!("{base}/auth/authorize?")) && link.contains("code_challenge_method=S256") && link.contains("resource=http%3A%2F%2F127.0.0.1"), "{link}");
        let old = Auth { issuer: server.issuer.clone(), client_id: "c-123".into(), token_endpoint: server.token_endpoint.clone(), refresh_token: "r1".into(), account: "me@x.com".into(), ..Default::default() };
        let fresh = refresh(&http, &url, &old).await.unwrap();
        assert_eq!((fresh.access_token.as_str(), fresh.refresh_token.as_str(), fresh.account.as_str()), ("new", "r1", "me@x.com"));
        assert!(fresh.expires_at > crate::db::now_ms());
    }
}
