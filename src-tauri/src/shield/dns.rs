// SPDX-License-Identifier: AGPL-3.0-only
//! How a request's site name gets turned into an address. A plain lookup
//! goes to this network's DNS server in the clear, so the network (and the
//! internet provider) sees every site name even when the connection itself
//! goes through Tor or a proxy. The shield keeps names off that path:
//! through Tor or a proxy the far end looks the name up; for connections
//! that still go direct, the name is looked up through Tor or over
//! encrypted DNS (DNS over HTTPS), as chosen.
//!
//! Local names (`localhost`, a PC or printer on this network) always use
//! Windows' own lookup: nobody outside can answer them, and sending them
//! out would only tell strangers what's on this network.

use super::settings::{DnsSetting, DohProvider};
use super::{Path, ShieldSettings};

/// Encrypted DNS servers reached by address, so finding the DNS server
/// doesn't itself need a plain lookup.
pub const CLOUDFLARE: &str = "https://1.1.1.1/dns-query";
pub const QUAD9: &str = "https://9.9.9.9/dns-query";

const LOCAL_SUFFIXES: &[&str] = &[".local", ".lan", ".home", ".home.arpa", ".internal", ".localhost", ".localdomain"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    /// The host is already an address.
    NotNeeded,
    /// A name on this PC or network: Windows looks it up.
    LocalOnly,
    /// Through Tor (`socks5h`): the exit relay looks it up.
    AtTorExit,
    /// Through the person's proxy (`socks5h` or HTTP CONNECT): the proxy
    /// looks it up.
    AtProxy,
    /// A direct connection whose name is looked up through Tor first.
    ThroughTor,
    /// A direct connection whose name is looked up over DNS over HTTPS here.
    Encrypted(String),
    /// Windows looks it up, as without the shield.
    System,
    /// The chosen way can't be used: refuse rather than look it up in the clear.
    Refused(String),
}

pub fn is_local_name(host: &str) -> bool {
    let h = host.trim().trim_end_matches('.').to_ascii_lowercase();
    !h.contains('.') || h == "localhost" || LOCAL_SUFFIXES.iter().any(|s| h.ends_with(s))
}

/// The encrypted DNS server chosen, or why it can't be used.
pub fn doh_url(s: &ShieldSettings) -> Result<String, String> {
    match s.doh_provider {
        DohProvider::Cloudflare => Ok(CLOUDFLARE.into()),
        DohProvider::Quad9 => Ok(QUAD9.into()),
        DohProvider::Custom => check_custom(&s.doh_custom).map(|()| s.doh_custom.trim().to_string()),
    }
}

/// A custom encrypted DNS address must be https with a host, or names would
/// leave in the clear after all.
pub fn check_custom(url: &str) -> Result<(), String> {
    let u = reqwest::Url::parse(url.trim()).map_err(|_| "That isn't an encrypted DNS address. It looks like https://dns.example/dns-query.".to_string())?;
    if u.scheme() != "https" {
        return Err("Encrypted DNS addresses start with https://.".into());
    }
    if u.host_str().is_none_or(str::is_empty) {
        return Err("The encrypted DNS address needs a server name or address.".into());
    }
    Ok(())
}

/// How the name for a request to `host`, going out by `path`, is looked up.
pub fn lookup_for(host: &str, path: Path, s: &ShieldSettings) -> Lookup {
    let bare = host.trim().trim_start_matches('[').trim_end_matches(']');
    if bare.parse::<std::net::IpAddr>().is_ok() {
        return Lookup::NotNeeded;
    }
    if is_local_name(bare) {
        return Lookup::LocalOnly;
    }
    match path {
        Path::Tor => Lookup::AtTorExit,
        Path::OwnProxy => Lookup::AtProxy,
        Path::Direct if !s.enabled => Lookup::System,
        Path::Direct => match s.dns {
            DnsSetting::System => Lookup::System,
            DnsSetting::Tor => Lookup::ThroughTor,
            DnsSetting::Encrypted => match doh_url(s) {
                Ok(u) => Lookup::Encrypted(u),
                Err(e) => Lookup::Refused(e),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(dns: DnsSetting) -> ShieldSettings {
        ShieldSettings { enabled: true, dns, ..Default::default() }
    }

    #[test]
    fn addresses_and_local_names_never_leave() {
        let s = on(DnsSetting::Tor);
        assert_eq!(lookup_for("1.2.3.4", Path::Tor, &s), Lookup::NotNeeded);
        assert_eq!(lookup_for("[::1]", Path::Direct, &s), Lookup::NotNeeded);
        for h in ["localhost", "nas", "printer.local", "router.lan", "box.home.arpa", "svc.internal", "LOCALHOST."] {
            assert_eq!(lookup_for(h, Path::Tor, &s), Lookup::LocalOnly, "{h}");
        }
        assert!(!is_local_name("example.com"));
        assert!(!is_local_name("localhost.example.com"), "only a real local suffix counts");
    }

    #[test]
    fn tor_and_proxies_look_names_up_at_the_far_end() {
        for dns in [DnsSetting::System, DnsSetting::Tor, DnsSetting::Encrypted] {
            assert_eq!(lookup_for("example.com", Path::Tor, &on(dns)), Lookup::AtTorExit);
            assert_eq!(lookup_for("example.com", Path::OwnProxy, &on(dns)), Lookup::AtProxy);
        }
    }

    #[test]
    fn direct_requests_follow_the_dns_setting() {
        assert_eq!(lookup_for("example.com", Path::Direct, &on(DnsSetting::System)), Lookup::System);
        assert_eq!(lookup_for("example.com", Path::Direct, &on(DnsSetting::Tor)), Lookup::ThroughTor);
        assert_eq!(lookup_for("example.com", Path::Direct, &on(DnsSetting::Encrypted)), Lookup::Encrypted(CLOUDFLARE.into()));
        let quad9 = ShieldSettings { doh_provider: DohProvider::Quad9, ..on(DnsSetting::Encrypted) };
        assert_eq!(lookup_for("example.com", Path::Direct, &quad9), Lookup::Encrypted(QUAD9.into()));
        let off = ShieldSettings { dns: DnsSetting::Encrypted, ..Default::default() };
        assert_eq!(lookup_for("example.com", Path::Direct, &off), Lookup::System, "shield off");
    }

    #[test]
    fn a_bad_custom_server_refuses_instead_of_leaking() {
        let bad = ShieldSettings { doh_provider: DohProvider::Custom, doh_custom: "http://dns.example/dns-query".into(), ..on(DnsSetting::Encrypted) };
        assert!(matches!(lookup_for("example.com", Path::Direct, &bad), Lookup::Refused(_)));
        let good = ShieldSettings { doh_custom: " https://dns.example/dns-query ".into(), ..bad };
        assert_eq!(lookup_for("example.com", Path::Direct, &good), Lookup::Encrypted("https://dns.example/dns-query".into()));
        assert!(check_custom("nonsense").is_err());
        assert!(check_custom("https://1.1.1.1/dns-query").is_ok());
    }
}
