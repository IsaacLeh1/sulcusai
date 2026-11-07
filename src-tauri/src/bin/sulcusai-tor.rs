// SPDX-License-Identifier: AGPL-3.0-only
// Release builds have no console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! SulcusAI's private connection, as a program of its own so the app
//! itself stays small: Tor (Arti) behind a SOCKS5 server on 127.0.0.1,
//! downloaded the first time the private connection is used.
//!
//! - Argument: the folder Tor keeps its state and cache in.
//! - Standard input: the first line is "<user> <password>" for the SOCKS
//!   server; a later "start" line begins connecting to the Tor network (so
//!   does the first connection). The program ends when standard input closes,
//!   so it never outlives SulcusAI.
//! - SOCKS user name: the plain name, or "<name>-<generation>-<site tag>"
//!   from the address shield, which gets one circuit per site and
//!   generation (Arti's stream isolation). Besides CONNECT, RESOLVE (0xF0,
//!   as Tor's own SOCKS port has it) looks a name up through Tor.
//! - Standard output: `{"port": n, "isolation": true}` once listening, then status lines:
//!   `{"state": "starting" | "ready" | "stuck", "progress": 0-100,
//!   "blocked": "offline" | "filtering" | "clock" | "other"}`.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use arti_client::config::TorClientConfigBuilder;
use arti_client::{IsolationToken, StreamPrefs, TorClient};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

// Shared with the app, which builds the user names this reads.
#[allow(dead_code)]
#[path = "../shield/isolation.rs"]
mod isolation;

type Tor = TorClient<tor_rtcompat::PreferredRuntime>;

/// While stuck this long, connections fail at once instead of waiting.
const STUCK_GRACE: Duration = Duration::from_secs(20);

fn say(line: serde_json::Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

fn blocked_code(kind: arti_client::status::BlockageKind) -> &'static str {
    use arti_client::status::BlockageKind as K;
    match kind {
        K::Offline => "offline",
        K::Filtering | K::CantReachTor => "filtering",
        K::ClockSkewed => "clock",
        _ => "other",
    }
}

struct Shared {
    tor: Arc<Tor>,
    started: AtomicBool,
    stuck_since: std::sync::Mutex<Option<Instant>>,
    user: String,
    pass: String,
    circuits: std::sync::Mutex<isolation::IsolationTable<IsolationToken>>,
}

impl Shared {
    fn start(self: &Arc<Self>) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        say(serde_json::json!({ "state": "starting", "progress": 0 }));
        let s = self.clone();
        tokio::spawn(async move {
            if let Err(e) = s.tor.bootstrap().await {
                *s.stuck_since.lock().unwrap() = Some(Instant::now());
                say(serde_json::json!({ "state": "stuck", "progress": 0, "blocked": "other", "error": e.to_string() }));
            }
        });
    }

    fn stuck(&self) -> bool {
        self.stuck_since.lock().unwrap().is_some_and(|t| t.elapsed() >= STUCK_GRACE)
    }
}

#[tokio::main]
async fn main() {
    let Some(dir) = std::env::args_os().nth(1).map(std::path::PathBuf::from) else {
        eprintln!("usage: sulcusai-tor <folder>");
        std::process::exit(2);
    };
    // Both of rustls's providers end up in the build, so it can't pick one by itself.
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    if let Ok(filter) = std::env::var("SULCUSAI_TOR_LOG") {
        let _ = tracing_subscriber::fmt().with_writer(std::io::stderr).with_ansi(false).with_env_filter(tracing_subscriber::EnvFilter::new(filter)).try_init();
    }
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let Ok(Some(first)) = lines.next_line().await else { return };
    let mut parts = first.split_whitespace();
    let (Some(user), Some(pass)) = (parts.next(), parts.next()) else {
        eprintln!("the first line must be: <user> <password>");
        std::process::exit(2);
    };
    let config = match TorClientConfigBuilder::from_directories(dir.join("state"), dir.join("cache")).build() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let tor = match TorClient::builder().config(config).create_unbootstrapped() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let shared = Arc::new(Shared { tor: tor.clone(), started: AtomicBool::new(false), stuck_since: Default::default(), user: user.into(), pass: pass.into(), circuits: Default::default() });

    // Progress and problems as Arti reports them.
    {
        let s = shared.clone();
        tokio::spawn(async move {
            use futures_util::StreamExt;
            let mut events = s.tor.bootstrap_events();
            while let Some(e) = events.next().await {
                let progress = (e.as_frac() * 100.0).round().clamp(0.0, 100.0) as u8;
                if e.ready_for_traffic() {
                    *s.stuck_since.lock().unwrap() = None;
                    say(serde_json::json!({ "state": "ready", "progress": 100 }));
                } else if let Some(b) = e.blocked() {
                    s.stuck_since.lock().unwrap().get_or_insert_with(Instant::now);
                    say(serde_json::json!({ "state": "stuck", "progress": progress, "blocked": blocked_code(b.kind()) }));
                } else {
                    *s.stuck_since.lock().unwrap() = None;
                    say(serde_json::json!({ "state": "starting", "progress": progress }));
                }
            }
        });
    }

    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    say(serde_json::json!({ "port": port, "isolation": true }));
    {
        let s = shared.clone();
        tokio::spawn(async move {
            while let Ok((sock, _)) = listener.accept().await {
                s.start();
                let s = s.clone();
                tokio::spawn(async move {
                    if let Err(e) = serve(sock, &s).await {
                        eprintln!("{e}");
                    }
                });
            }
        });
    }
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim() == "start" {
            shared.start();
        }
    }
}

/// One SOCKS5 connection (RFC 1928 + 1929): user name and password, then
/// CONNECT to a name or address, carried through Tor.
async fn serve(mut s: TcpStream, shared: &Shared) -> Result<(), String> {
    let e = |e: std::io::Error| e.to_string();
    let mut head = [0u8; 2];
    s.read_exact(&mut head).await.map_err(e)?;
    let mut methods = vec![0u8; head[1] as usize];
    s.read_exact(&mut methods).await.map_err(e)?;
    if head[0] != 5 || !methods.contains(&2) {
        s.write_all(&[5, 0xFF]).await.ok();
        return Err("no password method".into());
    }
    s.write_all(&[5, 2]).await.map_err(e)?;
    let mut v = [0u8; 2];
    s.read_exact(&mut v).await.map_err(e)?;
    let mut u = vec![0u8; v[1] as usize];
    s.read_exact(&mut u).await.map_err(e)?;
    let mut pl = [0u8; 1];
    s.read_exact(&mut pl).await.map_err(e)?;
    let mut p = vec![0u8; pl[0] as usize];
    s.read_exact(&mut p).await.map_err(e)?;
    let asked = std::str::from_utf8(&u).ok().and_then(|u| isolation::parse_socks_user(&shared.user, u));
    let (Some(asked), true) = (asked, p == shared.pass.as_bytes()) else {
        s.write_all(&[1, 1]).await.ok();
        return Err("wrong password".into());
    };
    let mut prefs = StreamPrefs::new();
    if let Some((generation, tag)) = asked {
        let token = shared.circuits.lock().unwrap().token(generation, &tag, IsolationToken::new);
        prefs.set_isolation(token);
    }
    s.write_all(&[1, 0]).await.map_err(e)?;
    let mut req = [0u8; 4];
    s.read_exact(&mut req).await.map_err(e)?;
    let resolve = req[1] == 0xF0;
    if req[0] != 5 || !(req[1] == 1 || resolve) {
        s.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0]).await.ok();
        return Err("only CONNECT".into());
    }
    let host = match req[3] {
        1 => {
            let mut a = [0u8; 4];
            s.read_exact(&mut a).await.map_err(e)?;
            std::net::Ipv4Addr::from(a).to_string()
        }
        3 => {
            let mut l = [0u8; 1];
            s.read_exact(&mut l).await.map_err(e)?;
            let mut name = vec![0u8; l[0] as usize];
            s.read_exact(&mut name).await.map_err(e)?;
            String::from_utf8(name).map_err(|e| e.to_string())?
        }
        4 => {
            let mut a = [0u8; 16];
            s.read_exact(&mut a).await.map_err(e)?;
            std::net::Ipv6Addr::from(a).to_string()
        }
        _ => return Err("bad address".into()),
    };
    let mut pb = [0u8; 2];
    s.read_exact(&mut pb).await.map_err(e)?;
    let port = u16::from_be_bytes(pb);
    // Waits while Tor is still starting, but not once it's been stuck a while.
    if shared.stuck() {
        s.write_all(&[5, 1, 0, 1, 0, 0, 0, 0, 0, 0]).await.ok();
        return Err("Tor is stuck".into());
    }
    if resolve {
        return match tokio::time::timeout(Duration::from_secs(60), shared.tor.resolve_with_prefs(&host, &prefs)).await {
            Ok(Ok(found)) => {
                let mut out = vec![5, 0, 0];
                match found.first() {
                    Some(std::net::IpAddr::V4(a)) => {
                        out.push(1);
                        out.extend_from_slice(&a.octets());
                    }
                    Some(std::net::IpAddr::V6(a)) => {
                        out.push(4);
                        out.extend_from_slice(&a.octets());
                    }
                    None => {
                        s.write_all(&[5, 4, 0, 1, 0, 0, 0, 0, 0, 0]).await.ok();
                        return Err("not found".into());
                    }
                }
                out.extend_from_slice(&[0, 0]);
                s.write_all(&out).await.map_err(e)
            }
            Ok(Err(err)) => {
                s.write_all(&[5, 4, 0, 1, 0, 0, 0, 0, 0, 0]).await.ok();
                Err(err.to_string())
            }
            Err(_) => {
                s.write_all(&[5, 6, 0, 1, 0, 0, 0, 0, 0, 0]).await.ok();
                Err("timed out".into())
            }
        };
    }
    match tokio::time::timeout(Duration::from_secs(120), shared.tor.connect_with_prefs((host.as_str(), port), &prefs)).await {
        Ok(Ok(mut stream)) => {
            s.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await.map_err(e)?;
            tokio::io::copy_bidirectional(&mut s, &mut stream).await.map_err(e)?;
            Ok(())
        }
        Ok(Err(err)) => {
            s.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).await.ok();
            Err(err.to_string())
        }
        Err(_) => {
            s.write_all(&[5, 6, 0, 1, 0, 0, 0, 0, 0, 0]).await.ok();
            Err("timed out".into())
        }
    }
}
