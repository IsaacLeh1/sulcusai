// SPDX-License-Identifier: AGPL-3.0-only
//! Address shield: makes it harder for the sites, peers and networks
//! SulcusAI talks to to see or follow this PC's address. One switch, off by
//! default, with each part below it also off until chosen.
//!
//! This folder is the decision side only: which way a request goes, which
//! circuit or proxy it gets, when addresses and ports change, how names are
//! looked up, and what the browser is told. Nothing here opens a connection;
//! `privacy.rs` (the Tor gateway), `net.rs` (clients), the local servers and
//! `browser.rs` act on these decisions.

// Nothing calls into the shield until it is wired to the gateway, the
// servers and the browser, so its public items look unused for now.
#![allow(dead_code)]

pub mod browser;
pub mod dns;
pub mod isolation;
pub mod peers;
pub mod ports;
pub mod proxies;
pub mod rotation;
pub mod settings;
pub mod text;

pub use settings::{ShieldSettings, Via};

/// The way a request leaves this PC while the shield is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Path {
    Tor,
    OwnProxy,
    Direct,
}

/// Which way requests go. Fails closed: a shield that is on but has nowhere
/// safe to send traffic refuses instead of going direct.
pub fn path(s: &ShieldSettings) -> Result<Path, String> {
    if !s.enabled {
        return Ok(Path::Direct);
    }
    match s.via {
        Via::Tor => Ok(Path::Tor),
        Via::OwnList if s.proxies.iter().any(|p| proxies::normalize(p).is_ok()) => Ok(Path::OwnProxy),
        Via::OwnList => Err(text::NO_PROXIES.into()),
    }
}

/// A random number without adding a crate: a v4 UUID is 122 random bits
/// from the OS generator, and folding its halves together cancels out the
/// fixed version and variant bits (they sit in different positions).
pub(crate) fn random_u64() -> u64 {
    let v = uuid::Uuid::new_v4().as_u128();
    (v as u64) ^ ((v >> 64) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_goes_direct_and_list_without_proxies_refuses() {
        assert_eq!(path(&ShieldSettings::default()), Ok(Path::Direct));
        let tor = ShieldSettings { enabled: true, ..Default::default() };
        assert_eq!(path(&tor), Ok(Path::Tor));
        let empty = ShieldSettings { enabled: true, via: Via::OwnList, ..Default::default() };
        assert!(path(&empty).is_err());
        let junk = ShieldSettings { enabled: true, via: Via::OwnList, proxies: vec!["not a proxy".into()], ..Default::default() };
        assert!(path(&junk).is_err());
        let one = ShieldSettings { enabled: true, via: Via::OwnList, proxies: vec!["127.0.0.1:1080".into()], ..Default::default() };
        assert_eq!(path(&one), Ok(Path::OwnProxy));
    }

    #[test]
    fn random_numbers_differ() {
        let a: Vec<u64> = (0..8).map(|_| random_u64()).collect();
        let mut b = a.clone();
        b.sort_unstable();
        b.dedup();
        assert_eq!(a.len(), b.len());
    }
}
