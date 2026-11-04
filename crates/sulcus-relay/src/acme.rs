// SPDX-License-Identifier: AGPL-3.0-only
//! Certificates from Let's Encrypt for the phone bridge (`--acme <name>`),
//! by the HTTP-01 challenge on port 80. Kept in `<dir>/acme/<name>/` and
//! renewed when a third of the 90 days is left, without a restart.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime};

use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use instant_acme::{Account, AuthorizationStatus, ChallengeType, Identifier, LetsEncrypt, NewAccount, NewOrder, OrderStatus};
use tokio_rustls::TlsAcceptor;

/// Renew once the certificate is this old (Let's Encrypt gives 90 days).
const RENEW_AFTER: Duration = Duration::from_secs(60 * 86_400);
const LOOK_EVERY: Duration = Duration::from_secs(12 * 3_600);

/// Challenge tokens and their answers while an order is open.
pub type Tokens = Arc<Mutex<HashMap<String, String>>>;

pub fn folder(dir: &Path, name: &str) -> PathBuf {
    dir.join("acme").join(name)
}

/// Whether the saved certificate is missing or old enough to renew.
pub fn due(cert: &Path, now: SystemTime) -> bool {
    let Ok(made) = std::fs::metadata(cert).and_then(|m| m.modified()) else { return true };
    now.duration_since(made).map_or(false, |age| age >= RENEW_AFTER)
}

/// A name Let's Encrypt can check over HTTP: letters, digits, dots and dashes.
pub fn valid_name(name: &str) -> bool {
    let labels: Vec<&str> = name.split('.').collect();
    labels.len() >= 2
        && name.len() <= 253
        && labels.iter().all(|l| !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-') && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
}

async fn answer(State(tokens): State<Tokens>, UrlPath(token): UrlPath<String>) -> impl IntoResponse {
    match tokens.lock().unwrap().get(&token) {
        Some(a) => (StatusCode::OK, a.clone()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Port 80: challenge answers only; everything else goes to HTTPS.
pub fn challenge_router(tokens: Tokens) -> axum::Router {
    axum::Router::new()
        .route("/.well-known/acme-challenge/{token}", axum::routing::get(answer))
        .fallback(|| async { (StatusCode::NOT_FOUND, "Use https://") })
        .with_state(tokens)
}

/// One order: proves the name, then saves `cert.pem` and `key.pem`.
async fn order(name: &str, contact: Option<&str>, folder: &Path, tokens: &Tokens) -> Result<(), String> {
    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    let account_file = folder.join("account.json");
    let account = match std::fs::read(&account_file).ok().and_then(|b| serde_json::from_slice(&b).ok()) {
        Some(creds) => Account::from_credentials(creds).await.map_err(|e| e.to_string())?,
        None => {
            let mailto = contact.map(|c| format!("mailto:{c}"));
            let contacts: Vec<&str> = mailto.iter().map(String::as_str).collect();
            let (account, creds) = Account::create(&NewAccount { contact: &contacts, terms_of_service_agreed: true, only_return_existing: false }, LetsEncrypt::Production.url(), None)
                .await
                .map_err(|e| e.to_string())?;
            std::fs::write(&account_file, serde_json::to_vec(&creds).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            account
        }
    };
    let ids = [Identifier::Dns(name.to_string())];
    let mut order = account.new_order(&NewOrder { identifiers: &ids }).await.map_err(|e| e.to_string())?;
    let mut ready = Vec::new();
    for authz in order.authorizations().await.map_err(|e| e.to_string())? {
        if authz.status == AuthorizationStatus::Valid {
            continue;
        }
        let ch = authz.challenges.iter().find(|c| c.r#type == ChallengeType::Http01).ok_or("Let's Encrypt offered no HTTP check")?;
        tokens.lock().unwrap().insert(ch.token.clone(), order.key_authorization(ch).as_str().to_string());
        ready.push(ch.url.clone());
    }
    for url in &ready {
        order.set_challenge_ready(url).await.map_err(|e| e.to_string())?;
    }
    let mut wait = Duration::from_secs(1);
    let status = loop {
        tokio::time::sleep(wait).await;
        let state = order.refresh().await.map_err(|e| e.to_string())?;
        if matches!(state.status, OrderStatus::Ready | OrderStatus::Invalid | OrderStatus::Valid) || wait > Duration::from_secs(60) {
            break state.status;
        }
        wait *= 2;
    };
    tokens.lock().unwrap().clear();
    if status == OrderStatus::Invalid {
        return Err(format!("Let's Encrypt couldn't check {name}: port 80 must reach this relay from the internet"));
    }
    let key = order.finalize().await.map_err(|e| e.to_string())?;
    let chain = loop {
        match order.certificate().await.map_err(|e| e.to_string())? {
            Some(c) => break c,
            None => tokio::time::sleep(Duration::from_secs(1)).await,
        }
    };
    // Key first: a certificate without its key would look valid but fail.
    std::fs::write(folder.join("key.pem"), key).map_err(|e| e.to_string())?;
    std::fs::write(folder.join("cert.pem"), chain).map_err(|e| e.to_string())?;
    Ok(())
}

/// Gets the certificate if needed (waiting for it the first time), then
/// keeps renewing it in the background and swaps it into `tls`.
pub async fn keep(name: String, contact: Option<String>, dir: PathBuf, tokens: Tokens) -> Result<Arc<RwLock<TlsAcceptor>>, String> {
    let folder = folder(&dir, &name);
    let (cert, key) = (folder.join("cert.pem"), folder.join("key.pem"));
    if due(&cert, SystemTime::now()) {
        order(&name, contact.as_deref(), &folder, &tokens).await?;
    }
    let tls = Arc::new(RwLock::new(crate::phone::tls(&cert, &key)?));
    let swap = tls.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(LOOK_EVERY).await;
            if !due(&cert, SystemTime::now()) {
                continue;
            }
            match order(&name, contact.as_deref(), &folder, &tokens).await.and_then(|_| crate::phone::tls(&cert, &key)) {
                Ok(new) => {
                    *swap.write().unwrap() = new;
                    println!("renewed the certificate for {name}");
                }
                Err(e) => eprintln!("couldn't renew the certificate for {name}: {e}"),
            }
        }
    });
    Ok(tls)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_renewal() {
        assert!(valid_name("relay.example.org"));
        assert!(!valid_name("localhost") && !valid_name("-x.example") && !valid_name("a..b") && !valid_name("1.2.3.4/x"));
        let dir = std::env::temp_dir().join(format!("sulcus-acme-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cert = dir.join("cert.pem");
        assert!(due(&cert, SystemTime::now()), "missing");
        std::fs::write(&cert, "x").unwrap();
        assert!(!due(&cert, SystemTime::now()));
        assert!(due(&cert, SystemTime::now() + RENEW_AFTER));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn port_80_answers_only_open_challenges() {
        let tokens: Tokens = Default::default();
        tokens.lock().unwrap().insert("tok".into(), "tok.thumb".into());
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, challenge_router(tokens)).await.unwrap() });
        let r = reqwest::get(format!("{base}/.well-known/acme-challenge/tok")).await.unwrap();
        assert_eq!(r.text().await.unwrap(), "tok.thumb");
        assert_eq!(reqwest::get(format!("{base}/.well-known/acme-challenge/other")).await.unwrap().status(), 404);
        assert_eq!(reqwest::get(format!("{base}/")).await.unwrap().status(), 404);
    }
}
