// SPDX-License-Identifier: AGPL-3.0-only
//! The phone bridge: serves `https://<this relay>/l/<link>/...` and passes
//! each request to the member PC that claimed the link, over
//! `/sulcus/phone/1`. Same paths as the Cloudflare link, so the phone page
//! works unchanged. What passes is sealed with the phone's key.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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

/// The bridge is only ever reached over HTTPS (here or at the proxy), so
/// phones are told never to try plain HTTP for it.
async fn hsts(req: Request, next: axum::middleware::Next) -> Response {
    let mut r = next.run(req).await;
    r.headers_mut().insert("strict-transport-security", HeaderValue::from_static("max-age=31536000"));
    r
}

pub fn router(b: Bridge) -> axum::Router {
    axum::Router::new().fallback(handle).layer(axum::middleware::from_fn(hsts)).with_state(b)
}

/// The certificate and key (PEM files, for example from Let's Encrypt).
pub fn tls(cert: &Path, key: &Path) -> Result<TlsAcceptor, String> {
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(cert).map_err(|e| format!("{}: {e}", cert.display()))?.collect::<Result<_, _>>().map_err(|e| format!("{}: {e}", cert.display()))?;
    let key = PrivateKeyDer::from_pem_file(key).map_err(|e| format!("{}: {e}", key.display()))?;
    let cfg = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| e.to_string())?;
    Ok(TlsAcceptor::from(Arc::new(cfg)))
}

/// Limits for the HTTPS port. Phones behind one carrier address share it,
/// so the share per address is generous; the server sets no header or idle
/// deadline of its own, hence IDLE.
pub fn limits() -> crate::port80::Limits {
    crate::port80::Limits { head_max: 0, read_time: Duration::from_secs(10), per_ip: 32, total: 1024, per_minute: 600 }
}
/// A connection with nothing read or written for this long is closed.
const IDLE: Duration = Duration::from_secs(60);

/// A TLS listener for `axum::serve`. Handshakes run side by side, so one
/// slow client never holds up the rest; a failed one is dropped.
pub struct TlsListener {
    ready: tokio::sync::mpsc::Receiver<(Conn, SocketAddr)>,
    addr: SocketAddr,
}

impl TlsListener {
    /// `tls` is swapped when the certificate is renewed.
    pub fn new(tcp: TcpListener, tls: Arc<std::sync::RwLock<TlsAcceptor>>, limits: crate::port80::Limits) -> std::io::Result<Self> {
        let addr = tcp.local_addr()?;
        let (tx, ready) = tokio::sync::mpsc::channel(64);
        tokio::spawn(async move {
            let counts: Arc<Mutex<crate::port80::Counts>> = Default::default();
            loop {
                let Ok((tcp, addr)) = tcp.accept().await else {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                };
                let Some(slot) = crate::port80::admit(&counts, addr.ip(), &limits, std::time::Instant::now()) else { continue };
                let acceptor = tls.read().unwrap().clone();
                let next = tx.clone();
                tokio::spawn(async move {
                    let tx = next;
                    if let Ok(Ok(s)) = tokio::time::timeout(limits.read_time, acceptor.accept(tcp)).await {
                        let conn = Conn { s, _slot: slot, idle: Box::pin(tokio::time::sleep(IDLE)) };
                        let _ = tx.send((conn, addr)).await;
                    }
                });
                if tx.is_closed() {
                    return;
                }
            }
        });
        Ok(TlsListener { ready, addr })
    }
}

impl axum::serve::Listener for TlsListener {
    type Io = Conn;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.ready.recv().await {
            Some(c) => c,
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(self.addr)
    }
}

/// One HTTPS connection: holds its place in the limits until dropped and
/// fails once idle too long.
pub struct Conn {
    s: TlsStream<TcpStream>,
    _slot: crate::port80::Slot,
    idle: std::pin::Pin<Box<tokio::time::Sleep>>,
}

impl Conn {
    fn moved(&mut self) {
        self.idle.as_mut().reset(tokio::time::Instant::now() + IDLE);
    }

    fn check(&mut self, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Error> {
        self.idle.as_mut().poll(cx).map(|_| std::io::ErrorKind::TimedOut.into())
    }
}

use std::future::Future;
use std::task::Poll;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

impl AsyncRead for Conn {
    fn poll_read(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        match std::pin::Pin::new(&mut self.s).poll_read(cx, buf) {
            Poll::Ready(r) => {
                self.moved();
                Poll::Ready(r)
            }
            Poll::Pending => self.check(cx).map(Err),
        }
    }
}

impl AsyncWrite for Conn {
    fn poll_write(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        match std::pin::Pin::new(&mut self.s).poll_write(cx, buf) {
            Poll::Ready(r) => {
                self.moved();
                Poll::Ready(r)
            }
            Poll::Pending => self.check(cx).map(Err),
        }
    }

    fn poll_flush(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.s).poll_flush(cx)
    }

    fn poll_shutdown(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.s).poll_shutdown(cx)
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
