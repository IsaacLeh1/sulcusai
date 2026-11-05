// SPDX-License-Identifier: AGPL-3.0-only
//! Certificates from Let's Encrypt for the phone bridge (`--acme <name>`),
//! by the HTTP-01 challenge on port 80. Kept in `<dir>/acme/<name>/` and
//! renewed when a third of the 90 days is left, without a restart.
//! A bare IP address works too: Let's Encrypt only signs those on its
//! short-lived profile (about 6 days), so they renew after 3.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime};

use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use instant_acme::{Account, AuthorizationStatus, ChallengeType, Identifier, LetsEncrypt, NewAccount, NewOrder, OrderStatus};

/// The only Let's Encrypt profile that signs IP addresses.
const SHORT_PROFILE: &str = "shortlived";
use tokio_rustls::TlsAcceptor;

/// Renew once the certificate is this old (Let's Encrypt gives 90 days).
const RENEW_AFTER: Duration = Duration::from_secs(60 * 86_400);
/// Short-lived certificates last about 160 hours; half leaves days to retry.
const RENEW_IP_AFTER: Duration = Duration::from_secs(3 * 86_400);
/// Often enough that a failed IP renewal gets several more tries in time.
const LOOK_EVERY: Duration = Duration::from_secs(3 * 3_600);

fn is_ip(name: &str) -> bool {
    name.parse::<IpAddr>().is_ok()
}

fn identifier(name: &str) -> Identifier {
    match name.parse::<IpAddr>() {
        Ok(ip) => Identifier::Ip(ip),
        Err(_) => Identifier::Dns(name.to_string()),
    }
}

/// How old a certificate for `name` may get before it is renewed.
pub fn renew_after(name: &str) -> Duration {
    if is_ip(name) { RENEW_IP_AFTER } else { RENEW_AFTER }
}

/// Challenge tokens and their answers while an order is open.
pub type Tokens = Arc<Mutex<HashMap<String, String>>>;

pub fn folder(dir: &Path, name: &str) -> PathBuf {
    dir.join("acme").join(name)
}

/// Whether the saved certificate is missing or old enough to renew.
pub fn due(cert: &Path, now: SystemTime, after: Duration) -> bool {
    let Ok(made) = std::fs::metadata(cert).and_then(|m| m.modified()) else { return true };
    now.duration_since(made).map_or(false, |age| age >= after)
}

/// A name Let's Encrypt can check over HTTP: letters, digits, dots and
/// dashes, or a public IP address (private ones can't be reached to check).
pub fn valid_name(name: &str) -> bool {
    if let Ok(ip) = name.parse::<IpAddr>() {
        return public_ip(ip);
    }
    if name.split('.').all(|l| l.chars().all(|c| c.is_ascii_digit())) {
        return false;
    }
    let labels: Vec<&str> = name.split('.').collect();
    labels.len() >= 2
        && name.len() <= 253
        && labels.iter().all(|l| !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-') && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let o = v.octets();
            let shared = o[0] == 100 && (o[1] & 0xc0) == 64;
            !(v.is_private() || v.is_loopback() || v.is_link_local() || v.is_unspecified() || v.is_broadcast() || v.is_documentation() || shared)
        }
        IpAddr::V6(v) => {
            let g = v.segments();
            let local = (g[0] & 0xfe00) == 0xfc00 || (g[0] & 0xffc0) == 0xfe80;
            let docs = g[0] == 0x2001 && g[1] == 0x0db8;
            !(v.is_loopback() || v.is_unspecified() || local || docs)
        }
    }
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
        Some(creds) => Account::builder().map_err(|e| e.to_string())?.from_credentials(creds).await.map_err(|e| e.to_string())?,
        None => {
            let mailto = contact.map(|c| format!("mailto:{c}"));
            let contacts: Vec<&str> = mailto.iter().map(String::as_str).collect();
            let (account, creds) = Account::builder()
                .map_err(|e| e.to_string())?
                .create(&NewAccount { contact: &contacts, terms_of_service_agreed: true, only_return_existing: false }, LetsEncrypt::Production.url().to_string(), None)
                .await
                .map_err(|e| e.to_string())?;
            std::fs::write(&account_file, serde_json::to_vec(&creds).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            account
        }
    };
    let ids = [identifier(name)];
    let new = NewOrder::new(&ids);
    let new = if is_ip(name) { new.profile(SHORT_PROFILE) } else { new };
    let mut order = account.new_order(&new).await.map_err(|e| e.to_string())?;
    let mut auths = order.authorizations();
    while let Some(authz) = auths.next().await {
        let mut authz = authz.map_err(|e| e.to_string())?;
        if authz.status == AuthorizationStatus::Valid {
            continue;
        }
        let mut ch = authz.challenge(ChallengeType::Http01).ok_or("Let's Encrypt offered no HTTP check")?;
        tokens.lock().unwrap().insert(ch.token.clone(), ch.key_authorization().as_str().to_string());
        ch.set_ready().await.map_err(|e| e.to_string())?;
    }
    drop(auths);
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
    // A fresh key for each certificate; only its signing request leaves.
    let mut params = rcgen::CertificateParams::new(vec![name.to_string()]).map_err(|e| e.to_string())?;
    params.distinguished_name = rcgen::DistinguishedName::new();
    let key_pair = rcgen::KeyPair::generate().map_err(|e| e.to_string())?;
    let csr = params.serialize_request(&key_pair).map_err(|e| e.to_string())?;
    order.finalize_csr(csr.der()).await.map_err(|e| e.to_string())?;
    let key = key_pair.serialize_pem();
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
    let after = renew_after(&name);
    if due(&cert, SystemTime::now(), after) {
        order(&name, contact.as_deref(), &folder, &tokens).await?;
    }
    let tls = Arc::new(RwLock::new(crate::phone::tls(&cert, &key)?));
    let swap = tls.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(LOOK_EVERY).await;
            if !due(&cert, SystemTime::now(), after) {
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
        assert!(due(&cert, SystemTime::now(), RENEW_AFTER), "missing");
        std::fs::write(&cert, "x").unwrap();
        assert!(!due(&cert, SystemTime::now(), RENEW_AFTER));
        assert!(due(&cert, SystemTime::now() + RENEW_AFTER, RENEW_AFTER));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn bare_ip_addresses() {
        assert!(valid_name("8.8.8.8") && valid_name("2606:4700::1111"));
        assert!(!valid_name("203.0.113.5") && !valid_name("192.168.1.10") && !valid_name("127.0.0.1") && !valid_name("100.64.0.1") && !valid_name("fd00::1") && !valid_name("1.2.3"));
        assert!(matches!(identifier("8.8.8.8"), Identifier::Ip(_)));
        assert!(matches!(identifier("relay.example.org"), Identifier::Dns(_)));
        // Short-lived certificates (~6 days) renew well before they run out.
        assert_eq!(renew_after("8.8.8.8"), RENEW_IP_AFTER);
        assert!(RENEW_IP_AFTER + 2 * LOOK_EVERY < Duration::from_secs(6 * 86_400));
        assert_eq!(renew_after("relay.example.org"), RENEW_AFTER);
        let csr = rcgen::CertificateParams::new(vec!["8.8.8.8".to_string()]).unwrap();
        assert!(matches!(csr.subject_alt_names[0], rcgen::SanType::IpAddress(_)));
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
