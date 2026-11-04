// SPDX-License-Identifier: AGPL-3.0-only
//! The phone bridge: serves `https://<this relay>/l/<link>/...` and passes
//! each request to the member PC that claimed the link, over
//! `/sulcus/phone/1`. Same paths as the Cloudflare link, so the phone page
//! works unchanged. What passes is sealed with the phone's key.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::body::{to_bytes, Body};
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use sulcus_net::phone_wire::{self, Links};
use sulcus_net::NodeHandle;
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::rustls;
use tokio_rustls::server::TlsStream;
use tokio_rustls::TlsAcceptor;

/// What a phone may send in one request, as on the Cloudflare link.
const MAX_BODY: usize = 30_000_000;
/// Headers the PC may set on what it sends the phone.
const PASSED: [&str; 6] = ["content-type", "cache-control", "content-security-policy", "x-frame-options", "referrer-policy", "x-content-type-options"];

#[derive(Clone)]
pub struct Bridge {
    pub node: Arc<NodeHandle>,
    pub links: Arc<Mutex<Links>>,
}

fn away(path: &str) -> Response {
    let html = path == "/" || path.is_empty() || path.ends_with(".html");
    let body = if html {
        "<!doctype html><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>SulcusAI</title><h1>Your PC isn't reachable</h1><p>Make sure your PC is on, SulcusAI is open on it, and Network is on.</p>"
    } else {
        "Your PC isn't reachable right now."
    };
    let mut r = (StatusCode::SERVICE_UNAVAILABLE, body).into_response();
    r.headers_mut().insert("content-type", HeaderValue::from_static(if html { "text/html; charset=utf-8" } else { "text/plain" }));
    r.headers_mut().insert("cache-control", HeaderValue::from_static("no-store"));
    r
}

/// `/l/<link>/<rest>` into the link and the rest (with its leading slash).
fn split(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix("/l/")?;
    let (link, rest) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    (link.len() == 32 && link.chars().all(|c| c.is_ascii_hexdigit())).then_some((link, rest))
}

async fn handle(State(b): State<Bridge>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    let Some((link, rest)) = split(&path) else { return StatusCode::NOT_FOUND.into_response() };
    if rest.is_empty() {
        let mut r = StatusCode::FOUND.into_response();
        if let Ok(v) = HeaderValue::from_str(&format!("{path}/")) {
            r.headers_mut().insert("location", v);
        }
        return r;
    }
    let Some(pc) = b.links.lock().unwrap().pc(link) else { return away(rest) };
    let (link, rest) = (link.to_string(), rest.to_string());
    let ask = if req.method() == Method::POST && rest == "/api" {
        let phone: String = req.headers().get("x-phone").and_then(|v| v.to_str().ok()).unwrap_or_default().chars().take(64).collect();
        let Ok(body) = to_bytes(req.into_body(), MAX_BODY).await else { return (StatusCode::PAYLOAD_TOO_LARGE, "Too big.").into_response() };
        let Ok(body) = String::from_utf8(body.to_vec()) else { return StatusCode::BAD_REQUEST.into_response() };
        phone_wire::Request::Api { link, phone, body }
    } else if req.method() == Method::GET {
        phone_wire::Request::Get { link, path: rest.chars().take(300).collect() }
    } else {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    };
    let Ok(a) = phone_wire::ask(&b.node, pc, &ask).await else { return away(&rest) };
    let mut r = Response::new(Body::from(a.body));
    *r.status_mut() = StatusCode::from_u16(a.status).unwrap_or(StatusCode::BAD_GATEWAY);
    for (k, v) in a.headers {
        let k = k.to_ascii_lowercase();
        if let (Some(name), Ok(v)) = (PASSED.iter().find(|p| **p == k), HeaderValue::from_str(&v)) {
            r.headers_mut().insert(*name, v);
        }
    }
    r
}

pub fn router(b: Bridge) -> axum::Router {
    axum::Router::new().fallback(handle).with_state(b)
}

/// The certificate and key (PEM files, for example from Let's Encrypt).
pub fn tls(cert: &Path, key: &Path) -> Result<TlsAcceptor, String> {
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(cert).map_err(|e| format!("{}: {e}", cert.display()))?.collect::<Result<_, _>>().map_err(|e| format!("{}: {e}", cert.display()))?;
    let key = PrivateKeyDer::from_pem_file(key).map_err(|e| format!("{}: {e}", key.display()))?;
    let cfg = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| e.to_string())?;
    Ok(TlsAcceptor::from(Arc::new(cfg)))
}

/// A TLS listener for `axum::serve`; a failed handshake is skipped.
pub struct TlsListener {
    pub tcp: TcpListener,
    pub tls: TlsAcceptor,
}

impl axum::serve::Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let Ok((tcp, addr)) = self.tcp.accept().await else { continue };
            let accepting = self.tls.accept(tcp);
            if let Ok(Ok(s)) = tokio::time::timeout(std::time::Duration::from_secs(10), accepting).await {
                return (s, addr);
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.tcp.local_addr()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sulcus_net::libp2p::multiaddr::Protocol;
    use sulcus_net::phone_wire::Answer;
    use sulcus_net::{identity, Node, NodeConfig};

    #[test]
    fn only_link_paths() {
        let id = "0123456789abcdef0123456789abcdef";
        assert_eq!(split(&format!("/l/{id}/x.js")), Some((id, "/x.js")));
        assert_eq!(split(&format!("/l/{id}")), Some((id, "")));
        assert_eq!(split("/l/short/"), None);
        assert_eq!(split("/other"), None);
    }

    /// A phone's request over HTTP reaches the PC that claimed the link,
    /// all on 127.0.0.1 (plain HTTP here; TLS is the listener's job).
    #[tokio::test(flavor = "multi_thread")]
    async fn the_phone_reaches_its_pc_through_the_bridge() {
        let link = "0123456789abcdef0123456789abcdef";
        let relay = Arc::new(Node::start(NodeConfig::loopback(identity::generate())).unwrap());
        let pc = Node::start(NodeConfig::loopback(identity::generate())).unwrap();
        let links = Arc::new(Mutex::new(Links::default()));
        let member = pc.peer_id();
        phone_wire::serve_claims(&relay, links.clone(), move |p| *p == member);
        let rid = relay.peer_id();
        phone_wire::serve(&pc, move || Some((rid, link.to_string())), |req| async move {
            match req {
                phone_wire::Request::Get { path, .. } => Answer { status: 200, headers: vec![("content-type".into(), "text/html".into()), ("set-cookie".into(), "x".into())], body: path.into_bytes() },
                phone_wire::Request::Api { phone, body, .. } => Answer { status: 200, headers: Vec::new(), body: format!("{phone}:{body}").into_bytes() },
            }
        });
        let mut ev = relay.subscribe();
        while !relay.listening().iter().any(|a| a.iter().any(|p| matches!(p, Protocol::Tcp(_)))) {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(10), ev.recv()).await;
        }
        let addr = relay.listening().into_iter().find(|a| a.iter().any(|p| matches!(p, Protocol::Tcp(_)))).unwrap().with(Protocol::P2p(rid));
        let mut pev = pc.subscribe();
        pc.dial(addr).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            while !pc.connected().contains(&rid) {
                let _ = pev.recv().await;
            }
        })
        .await
        .unwrap();

        let http = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", http.local_addr().unwrap());
        let app = router(Bridge { node: relay.clone(), links: links.clone() });
        tokio::spawn(async move { axum::serve(http, app).await.unwrap() });
        let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();

        let r = client.get(format!("{base}/l/{link}/index.html")).send().await.unwrap();
        assert_eq!(r.status(), 503, "nobody claimed it yet");
        phone_wire::claim(&pc, rid, link).await.unwrap();
        let r = client.get(format!("{base}/l/{link}/index.html")).send().await.unwrap();
        assert_eq!(r.status(), 200);
        assert!(r.headers().get("set-cookie").is_none(), "only the passed headers");
        assert_eq!(r.text().await.unwrap(), "/index.html");
        let r = client.post(format!("{base}/l/{link}/api")).header("x-phone", "p1").body("sealed").send().await.unwrap();
        assert_eq!(r.text().await.unwrap(), "p1:sealed");
        assert_eq!(client.get(format!("{base}/l/{link}")).send().await.unwrap().status(), 302);
        relay.stop().await;
        pc.stop().await;
    }
}
