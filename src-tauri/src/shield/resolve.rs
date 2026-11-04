// SPDX-License-Identifier: AGPL-3.0-only
//! Name lookups for connections that still go direct while the shield is
//! on (cloud providers reached with a key), the way `dns.rs` decides:
//! through Tor, over encrypted DNS (RFC 8484), or refused.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use super::dns::{self, Lookup};
use super::Path;

/// The resolver for a client that goes direct, or None to keep the usual
/// lookup (the shield off, or names left to Windows).
pub fn for_direct() -> Option<Arc<ShieldResolver>> {
    let s = super::live::current();
    (s.enabled && s.dns != super::settings::DnsSetting::System).then(|| Arc::new(ShieldResolver))
}

pub struct ShieldResolver;

impl reqwest::dns::Resolve for ShieldResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs = lookup(&host).await?;
            Ok(Box::new(addrs.into_iter().map(|ip| SocketAddr::new(ip, 0))) as reqwest::dns::Addrs)
        })
    }
}

async fn lookup(host: &str) -> Result<Vec<IpAddr>, String> {
    let s = super::live::current();
    let found = match dns::lookup_for(host, Path::Direct, &s) {
        Lookup::NotNeeded => host.trim_matches(['[', ']']).parse().map(|ip| vec![ip]).map_err(|_| format!("{host} isn't an address"))?,
        Lookup::LocalOnly | Lookup::System => tokio::net::lookup_host((host, 0)).await.map_err(|e| e.to_string())?.map(|a| a.ip()).collect(),
        Lookup::ThroughTor => crate::privacy::resolve_through_tor(host).await?,
        Lookup::Encrypted(url) => over_https(&url, host).await?,
        Lookup::Refused(e) => return Err(e),
        Lookup::AtTorExit | Lookup::AtProxy => return Err(format!("{host} is looked up at the far end, not here.")),
    };
    if found.is_empty() {
        return Err(format!("{host} wasn't found."));
    }
    Ok(found)
}

/// Both address kinds from the encrypted DNS server at `url`. The server's
/// own name, for a custom one given by name, is the one plain lookup left.
async fn over_https(url: &str, host: &str) -> Result<Vec<IpAddr>, String> {
    use base64::Engine;
    let client = reqwest::Client::builder().no_proxy().https_only(true).timeout(Duration::from_secs(10)).build().map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let mut last_error = None;
    for kind in [A, AAAA] {
        let q = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(query(host, kind)?);
        let reply = async {
            let r = client.get(url).query(&[("dns", q.as_str())]).header("accept", "application/dns-message").send().await.map_err(|e| e.to_string())?;
            let r = r.error_for_status().map_err(|e| e.to_string())?;
            r.bytes().await.map_err(|e| e.to_string())
        };
        match reply.await.and_then(|b| answers(&b)) {
            Ok(found) => out.extend(found),
            Err(e) => last_error = Some(e),
        }
    }
    match (out.is_empty(), last_error) {
        (true, Some(e)) => Err(format!("Encrypted DNS didn't answer for {host}: {e}")),
        _ => Ok(out),
    }
}

const A: u16 = 1;
const AAAA: u16 = 28;

/// A DNS question in wire form (RFC 1035), id 0 as RFC 8484 asks so
/// replies can be cached.
fn query(host: &str, kind: u16) -> Result<Vec<u8>, String> {
    let mut q = vec![0, 0, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    for label in host.trim_end_matches('.').split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err(format!("{host} isn't a valid name."));
        }
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.push(0);
    q.extend_from_slice(&kind.to_be_bytes());
    q.extend_from_slice(&1u16.to_be_bytes());
    Ok(q)
}

/// Skips a name at `at` (labels or a compression pointer), returning where
/// what follows it starts.
fn skip_name(b: &[u8], mut at: usize) -> Option<usize> {
    loop {
        let len = *b.get(at)? as usize;
        match len {
            0 => return Some(at + 1),
            l if l & 0xC0 == 0xC0 => return (at + 2 <= b.len()).then_some(at + 2),
            l => at += 1 + l,
        }
    }
}

/// The addresses in a DNS reply.
fn answers(b: &[u8]) -> Result<Vec<IpAddr>, String> {
    let bad = || "The encrypted DNS reply couldn't be read.".to_string();
    if b.len() < 12 {
        return Err(bad());
    }
    match b[3] & 0x0F {
        0 => {}
        3 => return Ok(Vec::new()),
        code => return Err(format!("the server answered with error {code}")),
    }
    let count = |i: usize| u16::from_be_bytes([b[i], b[i + 1]]) as usize;
    let (questions, records) = (count(4), count(6));
    let mut at = 12;
    for _ in 0..questions {
        at = skip_name(b, at).ok_or_else(bad)? + 4;
    }
    let mut out = Vec::new();
    for _ in 0..records {
        at = skip_name(b, at).ok_or_else(bad)?;
        let head = b.get(at..at + 10).ok_or_else(bad)?;
        let kind = u16::from_be_bytes([head[0], head[1]]);
        let len = u16::from_be_bytes([head[8], head[9]]) as usize;
        let data = b.get(at + 10..at + 10 + len).ok_or_else(bad)?;
        match (kind, len) {
            (A, 4) => out.push(IpAddr::from(<[u8; 4]>::try_from(data).map_err(|_| bad())?)),
            (AAAA, 16) => out.push(IpAddr::from(<[u8; 16]>::try_from(data).map_err(|_| bad())?)),
            _ => {}
        }
        at += 10 + len;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn questions_are_wire_form() {
        let q = query("example.com", A).unwrap();
        assert_eq!(&q[..12], &[0, 0, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&q[12..], b"\x07example\x03com\x00\x00\x01\x00\x01");
        assert!(query("a..b", A).is_err());
        assert!(query(&"x".repeat(64), A).is_err());
    }

    /// A reply with a CNAME, then an A and an AAAA record using
    /// compression pointers back to the question.
    #[test]
    fn replies_give_their_addresses() {
        let mut r = vec![0, 0, 0x81, 0x80, 0, 1, 0, 3, 0, 0, 0, 0];
        r.extend_from_slice(b"\x03www\x07example\x03com\x00\x00\x01\x00\x01");
        r.extend_from_slice(&[0xC0, 12, 0, 5, 0, 1, 0, 0, 0, 60, 0, 2, 0xC0, 16]);
        r.extend_from_slice(&[0xC0, 16, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 93, 184, 215, 14]);
        r.extend_from_slice(&[0xC0, 16, 0, 28, 0, 1, 0, 0, 0, 60, 0, 16]);
        r.extend_from_slice(&[0x26, 0x06, 0x28, 0, 0x02, 0x20, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1]);
        let got = answers(&r).unwrap();
        assert_eq!(got, vec!["93.184.215.14".parse::<IpAddr>().unwrap(), "2606:2800:220:1::1".parse().unwrap()]);
    }

    #[test]
    fn missing_names_and_broken_replies() {
        let nx = [0, 0, 0x81, 0x83, 0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(answers(&nx).unwrap(), Vec::<IpAddr>::new());
        let fail = [0, 0, 0x81, 0x82, 0, 0, 0, 0, 0, 0, 0, 0];
        assert!(answers(&fail).is_err());
        let cut = [0, 0, 0x81, 0x80, 0, 0, 0, 1, 0, 0, 0, 0, 0xC0];
        assert!(answers(&cut).is_err());
        let short_record = [0, 0, 0x81, 0x80, 0, 0, 0, 1, 0, 0, 0, 0, 0xC0, 12, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 1, 2];
        assert!(answers(&short_record).is_err());
    }

    #[test]
    fn off_keeps_the_usual_lookup() {
        assert!(for_direct().is_none());
    }
}
