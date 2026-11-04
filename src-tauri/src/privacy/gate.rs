// SPDX-License-Identifier: AGPL-3.0-only
//! The address shield's gateway: while the shield is on, every kind of
//! traffic the private connection carries comes here (a SOCKS5 server on
//! 127.0.0.1), and this is where it gets its way out:
//!
//! - through Tor, with the site and rotation generation in the user name
//!   given to the Tor helper, which keeps one circuit per site and
//!   generation, so sites can't be linked by sharing an exit;
//! - or through the person's own list, the proxy picked per site or in
//!   turn, a failing one set aside.
//!
//! Doing it here, not in each client, covers every caller (HTTP clients,
//! mail, update checks) with one picture of what's open, so a rotation can
//! close the connections that would otherwise keep the old address. The
//! site is the registrable domain of the server asked for, which for the
//! app's own requests is the site of the address requested.
//!
//! The built-in browser can't send a SOCKS user name, so it has a second
//! entry without a password. Any program on this PC could use that entry
//! while the shield is on; it only ever leads into Tor or the person's own
//! proxies, never out directly.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::Duration;

use tauri::AppHandle;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::{helper_port, start_tor, Route, GATEWAY, HELPER_ISOLATES};
use crate::net::Stream;
use crate::shield::{isolation, live, Path};
use crate::AppState;

struct Gate {
    port: u16,
    open_port: u16,
    user: String,
    pass: String,
}

static GATE: OnceLock<Gate> = OnceLock::new();
/// The app and its handle, kept for lookups that reach the Tor helper
/// from a resolver that has neither.
static APP: OnceLock<(Arc<AppState>, Option<AppHandle>)> = OnceLock::new();

/// What a connection carries, told apart by the user name each kind of
/// traffic is given, so a rotation can leave downloads alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Web,
    Updates,
    Mail,
    Models,
    Browser,
}

fn route_name(r: Route) -> &'static str {
    match r {
        Route::Web => "web",
        Route::Updates => "updates",
        Route::Mail => "mail",
        Route::Models => "models",
    }
}

fn kind_of(base: &str, user: &str) -> Option<Kind> {
    match user.strip_prefix(base)?.strip_prefix('-')? {
        "web" => Some(Kind::Web),
        "updates" => Some(Kind::Updates),
        "mail" => Some(Kind::Mail),
        "models" => Some(Kind::Models),
        _ => None,
    }
}

/// Open connections pooled by an HTTP client sit idle between requests;
/// this long without a byte counts as idle.
const IDLE_SECS: u64 = 5;

/// Whether a rotation closes this connection. A timed one closes only idle
/// pooled web connections (so the next request takes the new address) and
/// leaves mail waiting for messages alone; "New address now" closes all but
/// downloads, which finish where they started.
fn should_close(kind: Kind, idle: bool, manual: bool) -> bool {
    if manual {
        !matches!(kind, Kind::Models | Kind::Updates)
    } else {
        idle && matches!(kind, Kind::Web | Kind::Browser)
    }
}

struct Open {
    kind: Kind,
    generation: u64,
    last: Arc<AtomicU64>,
    task: tokio::task::AbortHandle,
}

static OPEN: LazyLock<Mutex<HashMap<u64, Open>>> = LazyLock::new(Default::default);
static NEXT: AtomicU64 = AtomicU64::new(0);

/// A rotation to `generation` happened: close what would keep an old address.
pub(crate) fn rotated(generation: u64, manual: bool) {
    let now = live::now();
    let mut open = OPEN.lock().unwrap();
    open.retain(|_, o| !o.task.is_finished());
    for o in open.values().filter(|o| o.generation < generation) {
        if should_close(o.kind, now.saturating_sub(o.last.load(Ordering::Relaxed)) >= IDLE_SECS, manual) {
            o.task.abort();
        }
    }
}

/// Starts both entries (once).
pub(super) async fn start(state: Arc<AppState>, app: Option<AppHandle>) -> Result<(), String> {
    let _ = APP.set((state.clone(), app.clone()));
    if GATE.get().is_some() {
        return Ok(());
    }
    let bind = || TcpListener::bind((Ipv4Addr::LOCALHOST, 0));
    let (with_password, open) = (bind().await, bind().await);
    let (with_password, open) = (with_password.map_err(|e| format!("Couldn't start the address shield: {e}"))?, open.map_err(|e| format!("Couldn't start the address shield: {e}"))?);
    let port = with_password.local_addr().map_err(|e| e.to_string())?.port();
    let open_port = open.local_addr().map_err(|e| e.to_string())?.port();
    let token = || uuid::Uuid::new_v4().simple().to_string();
    if GATE.set(Gate { port, open_port, user: token(), pass: token() }).is_err() {
        return Ok(());
    }
    for (listener, browser) in [(with_password, false), (open, true)] {
        let (state, app) = (state.clone(), app.clone());
        tokio::spawn(async move {
            while let Ok((sock, _)) = listener.accept().await {
                let (state, app) = (state.clone(), app.clone());
                tokio::spawn(async move {
                    let _ = serve(sock, browser, &state, app).await;
                });
            }
        });
    }
    Ok(())
}

/// The address a kind of traffic uses while the shield is on.
pub(super) fn address(route: Route) -> Option<String> {
    GATE.get().map(|g| format!("socks5h://{}-{}:{}@127.0.0.1:{}", g.user, route_name(route), g.pass, g.port))
}

/// The browser's entry (no password: Chromium can't send one).
pub(crate) fn browser_address() -> Option<String> {
    GATE.get().map(|g| format!("socks5://127.0.0.1:{}", g.open_port))
}

async fn serve(mut s: TcpStream, browser: bool, state: &Arc<AppState>, app: Option<AppHandle>) -> Result<(), String> {
    let g = GATE.get().ok_or("not started")?;
    let kind = if browser { Some(Kind::Browser) } else { None };
    let kind = handshake(&mut s, kind, &g.user, &g.pass).await?;
    let (host, port) = read_connect(&mut s).await?;
    let settings = live::current();
    let generation = live::generation();
    let out = match crate::shield::path(&settings) {
        Ok(Path::Tor) => through_tor(&host, port, settings.per_site, generation, state, app).await,
        Ok(Path::OwnProxy) => through_list(&host, port, settings.per_site).await,
        // Turned off a moment ago: the routes are being put back; never out directly from here.
        Ok(Path::Direct) => Err("The address shield is off.".into()),
        Err(e) => Err(e),
    };
    let out = match out {
        Ok(o) => o,
        Err(e) => {
            s.write_all(&reply(5)).await.ok();
            return Err(e);
        }
    };
    s.write_all(&reply(0)).await.map_err(|e| e.to_string())?;
    let last = Arc::new(AtomicU64::new(live::now()));
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let task = tokio::spawn(pipe(s, out, last.clone()));
    {
        let mut open = OPEN.lock().unwrap();
        open.retain(|_, o| !o.task.is_finished());
        open.insert(id, Open { kind, generation, last, task: task.abort_handle() });
    }
    let _ = task.await;
    OPEN.lock().unwrap().remove(&id);
    Ok(())
}

/// A SOCKS5 reply with an empty bound address.
fn reply(code: u8) -> [u8; 10] {
    [5, code, 0, 1, 0, 0, 0, 0, 0, 0]
}

/// The greeting and, on the password entry, the user name and password
/// (RFC 1929). `fixed` is the kind for the browser's entry.
async fn handshake<S: AsyncRead + AsyncWrite + Unpin>(s: &mut S, fixed: Option<Kind>, base: &str, pass: &str) -> Result<Kind, String> {
    let e = |e: std::io::Error| e.to_string();
    let mut head = [0u8; 2];
    s.read_exact(&mut head).await.map_err(e)?;
    let mut methods = vec![0u8; head[1] as usize];
    s.read_exact(&mut methods).await.map_err(e)?;
    let want = if fixed.is_some() { 0 } else { 2 };
    if head[0] != 5 || !methods.contains(&want) {
        s.write_all(&[5, 0xFF]).await.ok();
        return Err("no acceptable method".into());
    }
    s.write_all(&[5, want]).await.map_err(e)?;
    if let Some(k) = fixed {
        return Ok(k);
    }
    let mut v = [0u8; 2];
    s.read_exact(&mut v).await.map_err(e)?;
    let mut u = vec![0u8; v[1] as usize];
    s.read_exact(&mut u).await.map_err(e)?;
    let mut pl = [0u8; 1];
    s.read_exact(&mut pl).await.map_err(e)?;
    let mut p = vec![0u8; pl[0] as usize];
    s.read_exact(&mut p).await.map_err(e)?;
    let kind = std::str::from_utf8(&u).ok().and_then(|u| kind_of(base, u));
    match kind {
        Some(k) if p == pass.as_bytes() => {
            s.write_all(&[1, 0]).await.map_err(e)?;
            Ok(k)
        }
        _ => {
            s.write_all(&[1, 1]).await.ok();
            Err("wrong password".into())
        }
    }
}

/// The CONNECT request: the host (a name or address) and port.
async fn read_connect<S: AsyncRead + AsyncWrite + Unpin>(s: &mut S) -> Result<(String, u16), String> {
    let e = |e: std::io::Error| e.to_string();
    let mut req = [0u8; 4];
    s.read_exact(&mut req).await.map_err(e)?;
    if req[0] != 5 || req[1] != 1 {
        s.write_all(&reply(7)).await.ok();
        return Err("only CONNECT".into());
    }
    let host = match req[3] {
        1 => {
            let mut a = [0u8; 4];
            s.read_exact(&mut a).await.map_err(e)?;
            Ipv4Addr::from(a).to_string()
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
        _ => {
            s.write_all(&reply(8)).await.ok();
            return Err("bad address".into());
        }
    };
    let mut pb = [0u8; 2];
    s.read_exact(&mut pb).await.map_err(e)?;
    Ok((host, u16::from_be_bytes(pb)))
}

/// The user name the Tor helper gets for a connection to `host`: the site
/// and generation when the helper can isolate by them, else its plain name
/// (an older helper still hides the address, without per-site routes).
fn helper_user(base: &str, host: &str, per_site: bool, generation: u64, isolates: bool) -> String {
    if !isolates {
        return base.to_string();
    }
    let tag = if per_site { isolation::site_tag(&isolation::site_of(host)) } else { isolation::ALL_SITES.to_string() };
    isolation::socks_user(base, generation, &tag)
}

async fn through_tor(host: &str, port: u16, per_site: bool, generation: u64, state: &Arc<AppState>, app: Option<AppHandle>) -> Result<Box<dyn Stream>, String> {
    start_tor(state.clone(), app.clone());
    let helper = helper_port(state, app).await?;
    let g = GATEWAY.get().ok_or("The private connection isn't set up.")?;
    let user = helper_user(&g.user, host, per_site, generation, HELPER_ISOLATES.load(Ordering::SeqCst));
    let at = SocketAddr::from((Ipv4Addr::LOCALHOST, helper));
    let s = tokio::time::timeout(Duration::from_secs(120), tokio_socks::tcp::Socks5Stream::connect_with_password(at, (host, port), &user, &g.pass))
        .await
        .map_err(|_| format!("{host} didn't answer through Tor."))?
        .map_err(|e| format!("Couldn't reach {host} through Tor: {e}"))?;
    Ok(Box::new(s))
}

/// Through the person's list: the proxy for this site (or the current
/// one), the next on failure, each failure setting that proxy aside.
async fn through_list(host: &str, port: u16, per_site: bool) -> Result<Box<dyn Stream>, String> {
    let site = isolation::site_hash(&isolation::site_of(host));
    if live::with_ring(|r| r.is_empty()) {
        return Err(crate::shield::text::NO_PROXIES.into());
    }
    let tries = live::with_ring(|r| r.len());
    let mut last = String::new();
    for _ in 0..tries {
        let (i, proxy) = live::with_ring(|r| {
            let now = live::now();
            let picked = if per_site { r.pick_for_site(site, now) } else { r.pick(now) };
            picked.map(|(i, p)| (i, p.to_string()))
        })?;
        match crate::net::connect_through(host, port, Some(&proxy)).await {
            Ok(s) => {
                live::with_ring(|r| r.worked(i));
                return Ok(s);
            }
            Err(e) => {
                live::with_ring(|r| r.failed(i, live::now()));
                last = e;
            }
        }
    }
    Err(last)
}

/// Copies both ways until both ends are done, noting when bytes last moved.
async fn pipe(a: TcpStream, b: Box<dyn Stream>, last: Arc<AtomicU64>) {
    async fn one_way<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(mut r: R, mut w: W, last: &AtomicU64) -> std::io::Result<()> {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            let n = r.read(&mut buf).await?;
            if n == 0 {
                return w.shutdown().await;
            }
            w.write_all(&buf[..n]).await?;
            last.store(live::now(), Ordering::Relaxed);
        }
    }
    let (ar, aw) = tokio::io::split(a);
    let (br, bw) = tokio::io::split(b);
    let _ = tokio::try_join!(one_way(ar, bw, &last), one_way(br, aw, &last));
}

/// A name looked up through Tor (the helper's RESOLVE command, as Tor's
/// own SOCKS port has it).
pub(crate) async fn resolve_through_tor(host: &str) -> Result<Vec<std::net::IpAddr>, String> {
    let (state, app) = APP.get().cloned().ok_or("The address shield isn't running.")?;
    start_tor(state.clone(), app.clone());
    let port = helper_port(&state, app).await?;
    let g = GATEWAY.get().ok_or("The private connection isn't set up.")?;
    let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await.map_err(|e| e.to_string())?;
    let ip = tokio::time::timeout(Duration::from_secs(60), resolve_on(&mut s, &g.user, &g.pass, host)).await.map_err(|_| format!("Tor didn't find {host} in time."))??;
    Ok(vec![ip])
}

async fn resolve_on<S: AsyncRead + AsyncWrite + Unpin>(s: &mut S, user: &str, pass: &str, host: &str) -> Result<std::net::IpAddr, String> {
    let e = |e: std::io::Error| e.to_string();
    if host.len() > 255 || user.len() > 255 || pass.len() > 255 {
        return Err("name too long".into());
    }
    s.write_all(&[5, 1, 2]).await.map_err(e)?;
    let mut m = [0u8; 2];
    s.read_exact(&mut m).await.map_err(e)?;
    if m != [5, 2] {
        return Err("Tor refused the lookup.".into());
    }
    let mut auth = vec![1, user.len() as u8];
    auth.extend_from_slice(user.as_bytes());
    auth.push(pass.len() as u8);
    auth.extend_from_slice(pass.as_bytes());
    s.write_all(&auth).await.map_err(e)?;
    s.read_exact(&mut m).await.map_err(e)?;
    if m[1] != 0 {
        return Err("Tor refused the lookup.".into());
    }
    let mut req = vec![5, 0xF0, 0, 3, host.len() as u8];
    req.extend_from_slice(host.as_bytes());
    req.extend_from_slice(&[0, 0]);
    s.write_all(&req).await.map_err(e)?;
    let mut head = [0u8; 4];
    s.read_exact(&mut head).await.map_err(e)?;
    match head[1] {
        0 => {}
        7 => return Err("This copy of Tor can't look names up; it is updated with SulcusAI's next Tor release.".into()),
        _ => return Err(format!("Tor couldn't find {host}.")),
    }
    let ip = match head[3] {
        1 => {
            let mut a = [0u8; 4];
            s.read_exact(&mut a).await.map_err(e)?;
            std::net::IpAddr::from(a)
        }
        4 => {
            let mut a = [0u8; 16];
            s.read_exact(&mut a).await.map_err(e)?;
            std::net::IpAddr::from(a)
        }
        _ => return Err(format!("Tor couldn't find {host}.")),
    };
    Ok(ip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_route_has_its_own_name() {
        for r in [Route::Web, Route::Updates, Route::Mail, Route::Models] {
            let name = format!("base-{}", route_name(r));
            assert!(kind_of("base", &name).is_some());
        }
        assert_eq!(kind_of("base", "base-web"), Some(Kind::Web));
        assert_eq!(kind_of("base", "base"), None, "a kind is always named");
        assert_eq!(kind_of("base", "other-web"), None);
        assert_eq!(kind_of("base", "base-browser"), None, "the browser has its own entry");
    }

    #[test]
    fn rotations_close_the_right_connections() {
        // Timed: only idle pooled web connections.
        assert!(should_close(Kind::Web, true, false));
        assert!(should_close(Kind::Browser, true, false));
        assert!(!should_close(Kind::Web, false, false), "a request in flight finishes");
        assert!(!should_close(Kind::Mail, true, false), "mail waiting for messages stays");
        // New address now: everything but downloads.
        assert!(should_close(Kind::Mail, false, true));
        assert!(should_close(Kind::Web, false, true));
        assert!(!should_close(Kind::Models, false, true));
        assert!(!should_close(Kind::Updates, true, true));
    }

    #[test]
    fn the_helper_hears_site_and_generation() {
        let per_site = helper_user("gw", "mail.example.co.uk", true, 3, true);
        assert_eq!(per_site, isolation::socks_user("gw", 3, &isolation::site_tag("example.co.uk")));
        assert_eq!(isolation::parse_socks_user("gw", &per_site), Some(Some((3, isolation::site_tag("example.co.uk")))));
        assert_eq!(helper_user("gw", "a.example.com", true, 1, true), helper_user("gw", "b.example.com", true, 1, true), "one site, one route");
        assert_ne!(helper_user("gw", "a.example.com", true, 1, true), helper_user("gw", "example.org", true, 1, true));
        assert_eq!(helper_user("gw", "example.org", false, 2, true), "gw-2-all");
        assert_eq!(helper_user("gw", "example.org", true, 2, false), "gw", "an older helper gets its plain name");
    }

    #[tokio::test]
    async fn the_password_entry_checks_the_name_and_password() {
        let (mut client, mut server) = tokio::io::duplex(1024);
        let srv = tokio::spawn(async move { handshake(&mut server, None, "base", "secret").await });
        client.write_all(&[5, 1, 2]).await.unwrap();
        let mut m = [0u8; 2];
        client.read_exact(&mut m).await.unwrap();
        assert_eq!(m, [5, 2]);
        client.write_all(&[1, 9]).await.unwrap();
        client.write_all(b"base-mail").await.unwrap();
        client.write_all(&[6]).await.unwrap();
        client.write_all(b"secret").await.unwrap();
        client.read_exact(&mut m).await.unwrap();
        assert_eq!(m, [1, 0]);
        assert_eq!(srv.await.unwrap(), Ok(Kind::Mail));

        let (mut client, mut server) = tokio::io::duplex(1024);
        let srv = tokio::spawn(async move { handshake(&mut server, None, "base", "secret").await });
        client.write_all(&[5, 1, 2, 1, 8]).await.unwrap();
        client.write_all(b"base-web").await.unwrap();
        client.write_all(&[5]).await.unwrap();
        client.write_all(b"wrong").await.unwrap();
        client.read_exact(&mut m).await.unwrap();
        client.read_exact(&mut m).await.unwrap();
        assert_eq!(m, [1, 1]);
        assert!(srv.await.unwrap().is_err());

        // No password offered: refused on the password entry.
        let (mut client, mut server) = tokio::io::duplex(1024);
        let srv = tokio::spawn(async move { handshake(&mut server, None, "base", "secret").await });
        client.write_all(&[5, 1, 0]).await.unwrap();
        client.read_exact(&mut m).await.unwrap();
        assert_eq!(m, [5, 0xFF]);
        assert!(srv.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn the_browser_entry_needs_no_password_and_reads_the_target() {
        let (mut client, mut server) = tokio::io::duplex(1024);
        let srv = tokio::spawn(async move {
            let k = handshake(&mut server, Some(Kind::Browser), "base", "secret").await?;
            read_connect(&mut server).await.map(|t| (k, t))
        });
        client.write_all(&[5, 1, 0]).await.unwrap();
        let mut m = [0u8; 2];
        client.read_exact(&mut m).await.unwrap();
        assert_eq!(m, [5, 0]);
        client.write_all(&[5, 1, 0, 3, 11]).await.unwrap();
        client.write_all(b"example.com").await.unwrap();
        client.write_all(&443u16.to_be_bytes()).await.unwrap();
        assert_eq!(srv.await.unwrap(), Ok((Kind::Browser, ("example.com".to_string(), 443))));
    }

    /// A helper that answers RESOLVE the way Tor's SOCKS port does.
    #[tokio::test]
    async fn names_are_looked_up_through_the_helper() {
        let (mut client, mut helper) = tokio::io::duplex(1024);
        let h = tokio::spawn(async move {
            let mut b = [0u8; 3];
            helper.read_exact(&mut b).await.unwrap();
            helper.write_all(&[5, 2]).await.unwrap();
            let mut l = [0u8; 2];
            helper.read_exact(&mut l).await.unwrap();
            let mut u = vec![0u8; l[1] as usize + 1];
            helper.read_exact(&mut u).await.unwrap();
            let mut p = vec![0u8; u[u.len() - 1] as usize];
            helper.read_exact(&mut p).await.unwrap();
            helper.write_all(&[1, 0]).await.unwrap();
            let mut req = [0u8; 5];
            helper.read_exact(&mut req).await.unwrap();
            assert_eq!(req[1], 0xF0, "RESOLVE");
            let mut name = vec![0u8; req[4] as usize + 2];
            helper.read_exact(&mut name).await.unwrap();
            assert_eq!(&name[..name.len() - 2], b"example.com");
            helper.write_all(&[5, 0, 0, 1, 93, 184, 215, 14, 0, 0]).await.unwrap();
        });
        let ip = resolve_on(&mut client, "gw", "pw", "example.com").await.unwrap();
        assert_eq!(ip, "93.184.215.14".parse::<std::net::IpAddr>().unwrap());
        h.await.unwrap();
    }
}
