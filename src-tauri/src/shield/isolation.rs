// SPDX-License-Identifier: AGPL-3.0-only
//! A different Tor circuit, and so usually a different exit address, per
//! website. Requests tell the local Tor gateway which site they are for
//! through the SOCKS user name; the gateway keeps one Arti isolation token
//! per site and rotation generation, so sites can't be linked by sharing an
//! exit and a rotation gives every site a fresh one.
//!
//! A "site" is the registrable domain (`news.example.co.uk` and
//! `www.example.co.uk` are one site, `example.co.uk`) so a site's own
//! subdomains share an address: a login on `accounts.` and the page on
//! `www.` from two countries trips fraud checks.
//!
//! Without the full Public Suffix List (a new crate and a list to keep
//! current), the registrable domain is worked out from rules that cover
//! nearly all real traffic: two-label country suffixes like `co.uk`,
//! `com.au` and `ne.jp`, plus a short list of hosting platforms where every
//! subdomain is someone else's site (`github.io`, `pages.dev`...). The
//! fallback for anything else is the last two labels, so an unlisted
//! platform's sites (`a.unlisted-host.net`, `b.unlisted-host.net`) share one
//! circuit with each other. That weakens isolation only between sites on
//! that one platform; they still never share with any other site.

use std::collections::HashMap;

/// Hosting platforms where each subdomain belongs to a different person.
const PLATFORMS: &[&str] = &[
    "github.io",
    "gitlab.io",
    "blogspot.com",
    "wordpress.com",
    "tumblr.com",
    "substack.com",
    "herokuapp.com",
    "netlify.app",
    "vercel.app",
    "pages.dev",
    "workers.dev",
    "web.app",
    "firebaseapp.com",
    "appspot.com",
    "azurewebsites.net",
    "cloudfront.net",
    "s3.amazonaws.com",
    "glitch.me",
    "fly.dev",
    "onrender.com",
    "ngrok.io",
    "ngrok-free.app",
    "hf.space",
    "neocities.org",
    "itch.io",
];

/// Second-level labels that countries use under their two-letter suffix
/// (`co.uk`, `com.br`, `ac.jp`, `gob.mx`...).
const COUNTRY_SECOND_LEVEL: &[&str] = &[
    "ac", "co", "com", "edu", "gob", "gov", "go", "gv", "ltd", "mil", "ne", "net", "nhs", "nic", "nom", "or", "org", "plc", "sch", "info", "biz", "ed", "gr", "lg", "ad", "res", "mod",
];

/// Past this many sites in one generation the table starts over: every
/// site then gets a fresh circuit, which costs a little time and nothing else.
const MAX_SITES: usize = 4096;

/// The tag a request carries when "a different address per website" is off:
/// everything shares one circuit until the next rotation.
pub const ALL_SITES: &str = "all";

fn is_suffix(labels: &[&str]) -> bool {
    match labels {
        [second, tld] => {
            PLATFORMS.contains(&format!("{second}.{tld}").as_str())
                || (tld.len() == 2 && tld.bytes().all(|b| b.is_ascii_alphabetic()) && COUNTRY_SECOND_LEVEL.contains(second))
        }
        [a, b, c] => PLATFORMS.contains(&format!("{a}.{b}.{c}").as_str()),
        _ => false,
    }
}

/// The site a host belongs to (see the module notes). Addresses and
/// single-label names (`localhost`, a PC on the network) are their own site.
pub fn site_of(host: &str) -> String {
    let h = host.trim().trim_end_matches('.').trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    if h.parse::<std::net::IpAddr>().is_ok() {
        return h;
    }
    let labels: Vec<&str> = h.split('.').collect();
    if labels.len() <= 2 || labels.iter().any(|l| l.is_empty()) {
        return h;
    }
    let n = labels.len();
    for take in [3, 2] {
        if n > take && is_suffix(&labels[n - take..]) {
            return labels[n - take - 1..].join(".");
        }
    }
    labels[n - 2..].join(".")
}

/// The site a URL is for, or None when it has no host.
pub fn site_of_url(url: &str) -> Option<String> {
    let u = reqwest::Url::parse(url).ok()?;
    u.host_str().filter(|h| !h.is_empty()).map(site_of)
}

/// A stable, short stand-in for a site name, so the SOCKS user name stays
/// within its 255 bytes whatever the site. FNV-1a: not secret, only local.
pub fn site_hash(site: &str) -> u64 {
    site.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

pub fn site_tag(site: &str) -> String {
    format!("{:016x}", site_hash(site))
}

/// The SOCKS user name for a request: the gateway's own name, then the
/// rotation generation and the site's tag.
pub fn socks_user(base: &str, generation: u64, tag: &str) -> String {
    format!("{base}-{generation}-{tag}")
}

/// What a SOCKS user name asks for. None: not this gateway's name at all.
/// The plain name (older callers) means no isolation asked for.
pub fn parse_socks_user(base: &str, got: &str) -> Option<Option<(u64, String)>> {
    if got == base {
        return Some(None);
    }
    let rest = got.strip_prefix(base)?.strip_prefix('-')?;
    let (generation, tag) = rest.split_once('-')?;
    let generation = generation.parse().ok()?;
    let ok = tag == ALL_SITES || (tag.len() == 16 && tag.bytes().all(|b| b.is_ascii_hexdigit()));
    ok.then(|| Some((generation, tag.to_string())))
}

/// The gateway address a request to `target` should use: the gateway's
/// `socks5h://user:pass@127.0.0.1:port` with the user name carrying the
/// site and generation. Meant for `reqwest::Proxy::custom`, which asks per
/// request, so a client built once still follows rotations.
pub fn proxy_for_target(gateway: &str, target: &str, generation: u64, per_site: bool) -> Option<String> {
    let mut g = reqwest::Url::parse(gateway).ok()?;
    let tag = if per_site { site_tag(&site_of_url(target)?) } else { ALL_SITES.to_string() };
    let user = socks_user(g.username(), generation, &tag);
    g.set_username(&user).ok()?;
    Some(g.to_string())
}

/// One isolation token per site for the current generation, kept by the
/// gateway. `T` is Arti's `IsolationToken` there; tests use numbers.
#[derive(Debug)]
pub struct IsolationTable<T> {
    generation: u64,
    tokens: HashMap<String, T>,
}

impl<T: Clone> Default for IsolationTable<T> {
    fn default() -> Self {
        IsolationTable { generation: 0, tokens: HashMap::new() }
    }
}

impl<T: Clone> IsolationTable<T> {
    /// The token for this site and generation, made on first use. A newer
    /// generation drops every older token. A request still carrying an
    /// older generation (sent just before a rotation) gets the current
    /// token: an old circuit is never brought back.
    pub fn token(&mut self, generation: u64, tag: &str, make: impl FnOnce() -> T) -> T {
        if generation > self.generation {
            self.generation = generation;
            self.tokens.clear();
        }
        if self.tokens.len() >= MAX_SITES && !self.tokens.contains_key(tag) {
            self.tokens.clear();
        }
        self.tokens.entry(tag.to_string()).or_insert_with(make).clone()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn len(&self) -> usize {
        self.tokens.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sites_are_registrable_domains() {
        assert_eq!(site_of("www.example.com"), "example.com");
        assert_eq!(site_of("a.b.c.example.com"), "example.com");
        assert_eq!(site_of("example.com"), "example.com");
        assert_eq!(site_of("WWW.Example.COM."), "example.com");
        assert_eq!(site_of("news.bbc.co.uk"), "bbc.co.uk");
        assert_eq!(site_of("www.abc.net.au"), "abc.net.au");
        assert_eq!(site_of("shop.example.com.br"), "example.com.br");
        assert_eq!(site_of("www.example.ne.jp"), "example.ne.jp");
        assert_eq!(site_of("co.uk"), "co.uk");
        assert_eq!(site_of("bbc.co.uk"), "bbc.co.uk");
    }

    #[test]
    fn hosting_platforms_split_per_subdomain() {
        assert_eq!(site_of("alice.github.io"), "alice.github.io");
        assert_eq!(site_of("docs.alice.github.io"), "alice.github.io");
        assert_ne!(site_of("alice.github.io"), site_of("bob.github.io"));
        assert_eq!(site_of("bucket.s3.amazonaws.com"), "bucket.s3.amazonaws.com");
        assert_eq!(site_of("github.io"), "github.io");
        assert_eq!(site_of("github.com"), "github.com", "the company site is an ordinary site");
    }

    #[test]
    fn unknown_suffixes_fall_back_to_two_labels() {
        assert_eq!(site_of("a.unlisted-host.net"), "unlisted-host.net");
        assert_eq!(site_of("b.unlisted-host.net"), "unlisted-host.net");
        assert_eq!(site_of("www.example.co"), "example.co", "a country code alone isn't a two-part suffix");
    }

    #[test]
    fn addresses_and_local_names_are_their_own_site() {
        assert_eq!(site_of("93.184.216.34"), "93.184.216.34");
        assert_eq!(site_of("[2606:2800:220:1::1]"), "2606:2800:220:1::1");
        assert_eq!(site_of("localhost"), "localhost");
        assert_eq!(site_of("nas"), "nas");
        assert_eq!(site_of("odd..name.com"), "odd..name.com");
    }

    #[test]
    fn urls_give_their_site() {
        assert_eq!(site_of_url("https://maps.google.com/x?y=1").as_deref(), Some("google.com"));
        assert_eq!(site_of_url("http://[::1]:8080/").as_deref(), Some("::1"));
        assert_eq!(site_of_url("not a url"), None);
        assert_eq!(site_of_url("mailto:me@example.com"), None);
    }

    #[test]
    fn socks_user_round_trips() {
        let base = "0123456789abcdef0123456789abcdef";
        let tag = site_tag("example.com");
        assert_eq!(tag.len(), 16);
        let u = socks_user(base, 7, &tag);
        assert!(u.len() <= 255);
        assert_eq!(parse_socks_user(base, &u), Some(Some((7, tag))));
        assert_eq!(parse_socks_user(base, &socks_user(base, 0, ALL_SITES)), Some(Some((0, ALL_SITES.to_string()))));
        assert_eq!(parse_socks_user(base, base), Some(None));
        assert_eq!(parse_socks_user(base, "someone-else-1-abc"), None);
        assert_eq!(parse_socks_user(base, &format!("{base}-x-{}", site_tag("a"))), None);
        assert_eq!(parse_socks_user(base, &format!("{base}-1-not-hex")), None);
    }

    #[test]
    fn gateway_address_carries_site_and_generation() {
        let gw = "socks5h://0123456789abcdef0123456789abcdef:secret@127.0.0.1:45000";
        let a = proxy_for_target(gw, "https://www.example.com/a", 3, true).unwrap();
        let b = proxy_for_target(gw, "https://login.example.com/b", 3, true).unwrap();
        let c = proxy_for_target(gw, "https://other.org/", 3, true).unwrap();
        assert_eq!(a, b, "one site, one circuit");
        assert_ne!(a, c);
        assert!(a.starts_with("socks5h://0123456789abcdef0123456789abcdef-3-"), "{a}");
        assert!(a.ends_with(":secret@127.0.0.1:45000"), "{a}");
        assert_ne!(a, proxy_for_target(gw, "https://www.example.com/a", 4, true).unwrap(), "rotation changes it");
        let shared = proxy_for_target(gw, "https://other.org/", 3, false).unwrap();
        assert!(shared.contains("-3-all:"), "{shared}");
        assert_eq!(proxy_for_target(gw, "no host", 3, true), None);
    }

    #[test]
    fn tokens_per_site_and_generation() {
        let mut next = 0u32;
        let mut make = || {
            next += 1;
            next
        };
        let mut t = IsolationTable::default();
        let a1 = t.token(1, "a", &mut make);
        assert_eq!(t.token(1, "a", &mut make), a1, "same site, same token");
        let b1 = t.token(1, "b", &mut make);
        assert_ne!(a1, b1);
        let a2 = t.token(2, "a", &mut make);
        assert_ne!(a2, a1, "rotation: new token");
        assert_eq!(t.len(), 1, "older generation dropped");
        assert_eq!(t.token(1, "a", &mut make), a2, "a late request never revives the old circuit");
        assert_eq!(t.generation(), 2);
    }

    #[test]
    fn the_table_stays_bounded() {
        let mut t = IsolationTable::default();
        for i in 0..(MAX_SITES + 10) {
            t.token(0, &i.to_string(), || i);
        }
        assert!(t.len() <= MAX_SITES);
    }
}
