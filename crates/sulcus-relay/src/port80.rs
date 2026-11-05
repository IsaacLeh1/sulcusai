// SPDX-License-Identifier: AGPL-3.0-only
//! Port 80 for Let's Encrypt's HTTP-01 check. It is open to the whole
//! internet, so it is a few lines of fixed answers instead of a web server:
//! the request line is read (never a body), compared byte for byte with the
//! challenges open right now, and anything else gets a bare redirect to
//! HTTPS. It is only listening while an order is open.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::acme::Tokens;

pub const PREFIX: &[u8] = b"/.well-known/acme-challenge/";

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// The whole request head; Let's Encrypt sends a few hundred bytes.
    pub head_max: usize,
    /// From connecting to the end of the head (slow clients are cut off).
    pub read_time: Duration,
    pub per_ip: usize,
    pub total: usize,
    /// New connections per address per minute.
    pub per_minute: u32,
}

impl Default for Limits {
    fn default() -> Self {
        // Let's Encrypt checks from several places, a few times per order.
        Limits { head_max: 2048, read_time: Duration::from_secs(5), per_ip: 4, total: 64, per_minute: 30 }
    }
}

/// The answer for one request head: the challenge answer, or the redirect.
pub fn respond(head: &[u8], tokens: &Tokens, name: &str) -> Vec<u8> {
    let line = head.split(|b| *b == b'\n').next().unwrap_or_default();
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let mut parts = line.split(|b| *b == b' ');
    let (method, target, version) = (parts.next(), parts.next(), parts.next());
    let wellformed = parts.next().is_none() && matches!(version, Some(b"HTTP/1.1") | Some(b"HTTP/1.0"));
    if wellformed && matches!(method, Some(b"GET")) {
        // No decoding at all: `%2F`, `..` and the like can never equal a token.
        if let Some(token) = target.and_then(|t| t.strip_prefix(PREFIX)) {
            if !token.is_empty() && token.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-' || *c == b'_') {
                let token = std::str::from_utf8(token).unwrap_or_default();
                if let Some(answer) = tokens.lock().unwrap().get(token) {
                    let mut r = format!("HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", answer.len()).into_bytes();
                    r.extend_from_slice(answer.as_bytes());
                    return r;
                }
            }
        }
    }
    // A fixed place: never the asked-for Host or path, so nothing is echoed.
    format!("HTTP/1.1 301 Moved Permanently\r\nLocation: https://{}/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", host(name)).into_bytes()
}

fn host(name: &str) -> String {
    match name.parse::<IpAddr>() {
        Ok(IpAddr::V6(v)) => format!("[{v}]"),
        _ => name.to_string(),
    }
}

const TOO_BIG: &[u8] = b"HTTP/1.1 431 Request Header Fields Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

/// Reads one request head within the limits; Err(true) when it was too big.
async fn read_head(s: &mut TcpStream, limits: &Limits) -> Result<Vec<u8>, bool> {
    let mut buf = vec![0u8; limits.head_max];
    let mut got = 0;
    let read = async {
        loop {
            if got == buf.len() {
                return Err(true);
            }
            let n = s.read(&mut buf[got..]).await.map_err(|_| false)?;
            if n == 0 {
                return Err(false);
            }
            got += n;
            if buf[..got].windows(4).any(|w| w == b"\r\n\r\n") || buf[..got].windows(2).any(|w| w == b"\n\n") {
                return Ok(());
            }
        }
    };
    tokio::time::timeout(limits.read_time, read).await.map_err(|_| false)??;
    buf.truncate(got);
    Ok(buf)
}

#[derive(Default)]
pub(crate) struct Counts {
    open: HashMap<IpAddr, usize>,
    total: usize,
    recent: HashMap<IpAddr, (Instant, u32)>,
}

/// Admits a connection or says no; the slot is given back on drop.
pub(crate) struct Slot {
    counts: Arc<Mutex<Counts>>,
    ip: IpAddr,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut c = self.counts.lock().unwrap();
        c.total -= 1;
        if let Some(n) = c.open.get_mut(&self.ip) {
            *n -= 1;
            if *n == 0 {
                c.open.remove(&self.ip);
            }
        }
    }
}

pub(crate) fn admit(counts: &Arc<Mutex<Counts>>, ip: IpAddr, limits: &Limits, now: Instant) -> Option<Slot> {
    let mut c = counts.lock().unwrap();
    if c.recent.len() > 10_000 {
        c.recent.retain(|_, (start, _)| now.duration_since(*start) < Duration::from_secs(60));
    }
    let r = c.recent.entry(ip).or_insert((now, 0));
    if now.duration_since(r.0) >= Duration::from_secs(60) {
        *r = (now, 0);
    }
    r.1 += 1;
    if r.1 > limits.per_minute || c.total >= limits.total || c.open.get(&ip).copied().unwrap_or(0) >= limits.per_ip {
        return None;
    }
    c.total += 1;
    *c.open.entry(ip).or_insert(0) += 1;
    Some(Slot { counts: counts.clone(), ip })
}

/// Serves until the task is dropped (the order is done).
pub async fn serve(listener: TcpListener, tokens: Tokens, name: String, limits: Limits) {
    let counts: Arc<Mutex<Counts>> = Default::default();
    let name: Arc<str> = name.into();
    loop {
        let Ok((mut s, addr)) = listener.accept().await else {
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        };
        // An over-limit client is closed at once, without a byte sent.
        let Some(slot) = admit(&counts, addr.ip(), &limits, Instant::now()) else { continue };
        let (tokens, name) = (tokens.clone(), name.clone());
        tokio::spawn(async move {
            let _slot = slot;
            let answer = match read_head(&mut s, &limits).await {
                Ok(head) => respond(&head, &tokens, &name),
                Err(true) => TOO_BIG.to_vec(),
                Err(false) => return,
            };
            let _ = tokio::time::timeout(limits.read_time, s.write_all(&answer)).await;
            let _ = s.shutdown().await;
        });
    }
}

/// Port 80 is listening for as long as this lives.
pub struct Open(Vec<tokio::task::JoinHandle<()>>);

impl Drop for Open {
    fn drop(&mut self) {
        self.0.iter().for_each(|h| h.abort());
    }
}

/// IPv6 and IPv4. Where `::` already takes both (Linux), the second bind
/// fails and is not needed.
pub async fn open(port: u16, tokens: Tokens, name: String) -> Result<Open, String> {
    let v6 = TcpListener::bind(("::", port)).await;
    let v4 = TcpListener::bind(("0.0.0.0", port)).await;
    let all: Vec<TcpListener> = [v6.ok(), v4.ok()].into_iter().flatten().collect();
    if all.is_empty() {
        return Err(format!("port {port} (needed for Let's Encrypt) is in use or not allowed"));
    }
    Ok(Open(all.into_iter().map(|l| tokio::spawn(serve(l, tokens.clone(), name.clone(), Limits::default()))).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens() -> Tokens {
        let t: Tokens = Default::default();
        t.lock().unwrap().insert("Ab_9-z".into(), "Ab_9-z.thumb".into());
        t
    }

    fn body(r: &[u8]) -> String {
        String::from_utf8_lossy(r).into_owned()
    }

    #[test]
    fn only_open_tokens_exactly() {
        let t = tokens();
        let ok = body(&respond(b"GET /.well-known/acme-challenge/Ab_9-z HTTP/1.1\r\nHost: x\r\n\r\n", &t, "relay.example.org"));
        assert!(ok.starts_with("HTTP/1.1 200") && ok.ends_with("\r\n\r\nAb_9-z.thumb"));
        assert!(!ok.to_ascii_lowercase().contains("server:") && !ok.to_ascii_lowercase().contains("date:"));
        for bad in [
            "GET /.well-known/acme-challenge/other HTTP/1.1",
            "GET /.well-known/acme-challenge/Ab_9-z/ HTTP/1.1",
            "GET /.well-known/acme-challenge/Ab_9-z?x HTTP/1.1",
            "GET /.well-known/acme-challenge/ HTTP/1.1",
            "GET /.well-known/acme-challenge/../acme-challenge/Ab_9-z HTTP/1.1",
            "GET /.well-known/acme-challenge/%41b_9-z HTTP/1.1",
            "GET /.well-known%2Facme-challenge%2FAb_9-z HTTP/1.1",
            "GET /.well-known/acme-challenge/x/..%2FAb_9-z HTTP/1.1",
            "GET //.well-known/acme-challenge/Ab_9-z HTTP/1.1",
            "GET http://evil/.well-known/acme-challenge/Ab_9-z HTTP/1.1",
            "POST /.well-known/acme-challenge/Ab_9-z HTTP/1.1",
            "GET /.well-known/acme-challenge/Ab_9-z HTTP/2",
            "GET /.well-known/acme-challenge/Ab_9-z HTTP/1.1 extra",
            "GET /admin HTTP/1.1",
            "garbage",
        ] {
            let r = body(&respond(format!("{bad}\r\nHost: evil.example\r\n\r\n").as_bytes(), &t, "relay.example.org"));
            assert_eq!(r, "HTTP/1.1 301 Moved Permanently\r\nLocation: https://relay.example.org/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", "{bad}");
        }
        assert!(body(&respond(b"GET / HTTP/1.1\r\n\r\n", &t, "2606:4700::1111")).contains("Location: https://[2606:4700::1111]/\r\n"));
        t.lock().unwrap().clear();
        assert!(body(&respond(b"GET /.well-known/acme-challenge/Ab_9-z HTTP/1.1\r\n\r\n", &t, "r.example.org")).starts_with("HTTP/1.1 301"), "closed orders answer nothing");
    }

    #[test]
    fn rate_and_connection_limits() {
        let counts: Arc<Mutex<Counts>> = Default::default();
        let l = Limits { per_ip: 2, total: 3, per_minute: 5, ..Limits::default() };
        let (a, b, c): (IpAddr, IpAddr, IpAddr) = ("1.1.1.1".parse().unwrap(), "2.2.2.2".parse().unwrap(), "3.3.3.3".parse().unwrap());
        let now = Instant::now();
        let s1 = admit(&counts, a, &l, now).unwrap();
        let _s2 = admit(&counts, a, &l, now).unwrap();
        assert!(admit(&counts, a, &l, now).is_none(), "per address");
        let _s3 = admit(&counts, b, &l, now).unwrap();
        assert!(admit(&counts, c, &l, now).is_none(), "in all");
        drop(s1);
        let _s4 = admit(&counts, c, &l, now).unwrap();
        let counts: Arc<Mutex<Counts>> = Default::default();
        for _ in 0..5 {
            drop(admit(&counts, a, &l, now).unwrap());
        }
        assert!(admit(&counts, a, &l, now).is_none(), "per minute");
        assert!(admit(&counts, a, &l, now + Duration::from_secs(61)).is_some(), "a new minute");
    }

    async fn server(limits: Limits) -> std::net::SocketAddr {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(serve(l, tokens(), "relay.example.org".into(), limits));
        addr
    }

    async fn reply(s: &mut TcpStream) -> String {
        let mut out = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut out)).await;
        body(&out)
    }

    #[tokio::test]
    async fn over_the_wire() {
        let addr = server(Limits { read_time: Duration::from_millis(300), per_minute: 1000, ..Limits::default() }).await;
        let mut s = TcpStream::connect(addr).await.unwrap();
        s.write_all(b"GET /.well-known/acme-challenge/Ab_9-z HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
        assert!(reply(&mut s).await.ends_with("Ab_9-z.thumb"));

        // Oversized head: 431, and no more is read.
        let mut s = TcpStream::connect(addr).await.unwrap();
        let big = format!("GET / HTTP/1.1\r\nX: {}\r\n\r\n", "a".repeat(4000));
        let _ = s.write_all(big.as_bytes()).await;
        assert!(reply(&mut s).await.starts_with("HTTP/1.1 431"));

        // A body is never read: the answer comes right after the head.
        let mut s = TcpStream::connect(addr).await.unwrap();
        s.write_all(b"POST / HTTP/1.1\r\nContent-Length: 1000000\r\n\r\nxx").await.unwrap();
        assert!(reply(&mut s).await.starts_with("HTTP/1.1 301"));

        // Slow client: dripped bytes, cut off at the deadline with nothing sent.
        let mut s = TcpStream::connect(addr).await.unwrap();
        let start = Instant::now();
        for b in b"GET / HT" {
            if s.write_all(&[*b]).await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(60)).await;
        }
        assert_eq!(reply(&mut s).await, "");
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    #[tokio::test]
    async fn floods_are_turned_away() {
        let addr = server(Limits { read_time: Duration::from_secs(2), per_ip: 3, per_minute: 1000, ..Limits::default() }).await;
        // Three idle connections fill this address's share...
        let mut idle = Vec::new();
        for _ in 0..3 {
            idle.push(TcpStream::connect(addr).await.unwrap());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut s = TcpStream::connect(addr).await.unwrap();
        let _ = s.write_all(b"GET /.well-known/acme-challenge/Ab_9-z HTTP/1.1\r\n\r\n").await;
        assert_eq!(reply(&mut s).await, "", "closed at once");
        drop(idle);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut s = TcpStream::connect(addr).await.unwrap();
        s.write_all(b"GET /.well-known/acme-challenge/Ab_9-z HTTP/1.1\r\n\r\n").await.unwrap();
        assert!(reply(&mut s).await.ends_with("Ab_9-z.thumb"), "room again");

        // A burst past the per-minute rate is cut off.
        let addr = server(Limits { per_minute: 5, ..Limits::default() }).await;
        let mut answered = 0;
        for _ in 0..20 {
            let mut s = TcpStream::connect(addr).await.unwrap();
            let _ = s.write_all(b"GET / HTTP/1.1\r\n\r\n").await;
            if !reply(&mut s).await.is_empty() {
                answered += 1;
            }
        }
        assert_eq!(answered, 5);
    }
}
