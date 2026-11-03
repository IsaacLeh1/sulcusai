// SPDX-License-Identifier: AGPL-3.0-only
//! The shield's switches. Everything starts off; numbers have sensible
//! defaults so turning a part on needs no typing.

use serde::{Deserialize, Serialize};

pub const KEY: &str = "shield";

pub const ROTATE_MINUTES_DEFAULT: u32 = 10;
pub const ROTATE_MINUTES_MIN: u32 = 1;
/// A day: past that, rotation stops meaning much and a typo (minutes typed
/// as seconds) would quietly turn it off.
pub const ROTATE_MINUTES_MAX: u32 = 24 * 60;

pub const PORT_HOURS_DEFAULT: u32 = 24;
pub const PORT_HOURS_MIN: u32 = 1;
pub const PORT_HOURS_MAX: u32 = 24 * 7;

/// A list longer than this is almost certainly pasted by mistake, and every
/// entry is tried in turn when others fail.
pub const MAX_PROXIES: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Via {
    #[default]
    Tor,
    /// The person's own proxies or VPNs' local proxies, taken in turn.
    OwnList,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DnsSetting {
    /// Windows looks names up, as without the shield (unless the request
    /// goes through Tor or a proxy, which look names up themselves).
    #[default]
    System,
    Tor,
    Encrypted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DohProvider {
    #[default]
    Cloudflare,
    Quad9,
    /// `doh_custom` holds the address.
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShieldSettings {
    pub enabled: bool,
    pub via: Via,
    /// A different Tor circuit (so usually a different exit address) per website.
    pub per_site: bool,
    /// New addresses on a timer (Tor circuits, or the next proxy in the list).
    pub rotate: bool,
    pub rotate_minutes: u32,
    pub proxies: Vec<String>,
    /// Social, VPN and file-sharing connections to other people only through
    /// relays, never straight to their PC.
    pub peers_relays_only: bool,
    pub random_ports: bool,
    pub port_change_hours: u32,
    pub dns: DnsSetting,
    pub doh_provider: DohProvider,
    pub doh_custom: String,
    pub browser_common_fingerprint: bool,
    pub browser_block_third_party_cookies: bool,
    pub browser_clear_on_close: bool,
}

impl Default for ShieldSettings {
    fn default() -> Self {
        ShieldSettings {
            enabled: false,
            via: Via::Tor,
            per_site: false,
            rotate: false,
            rotate_minutes: ROTATE_MINUTES_DEFAULT,
            proxies: Vec::new(),
            peers_relays_only: false,
            random_ports: false,
            port_change_hours: PORT_HOURS_DEFAULT,
            dns: DnsSetting::System,
            doh_provider: DohProvider::Cloudflare,
            doh_custom: String::new(),
            browser_common_fingerprint: false,
            browser_block_third_party_cookies: false,
            browser_clear_on_close: false,
        }
    }
}

impl ShieldSettings {
    /// What was saved, made safe to use: numbers kept in range and the proxy
    /// list trimmed, normalized and without repeats. Bad entries are dropped
    /// here; `problems` is what tells the person about them.
    pub fn cleaned(mut self) -> Self {
        self.rotate_minutes = self.rotate_minutes.clamp(ROTATE_MINUTES_MIN, ROTATE_MINUTES_MAX);
        self.port_change_hours = self.port_change_hours.clamp(PORT_HOURS_MIN, PORT_HOURS_MAX);
        let mut seen = Vec::new();
        for p in &self.proxies {
            if let Ok(n) = super::proxies::normalize(p) {
                if !seen.contains(&n) && seen.len() < MAX_PROXIES {
                    seen.push(n);
                }
            }
        }
        self.proxies = seen;
        self.doh_custom = self.doh_custom.trim().to_string();
        self
    }

    /// Plain-words problems with what was typed, for the window, before saving.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        for p in &self.proxies {
            if let Err(e) = super::proxies::normalize(p) {
                out.push(format!("\"{}\": {e}", p.trim()));
            }
        }
        if self.proxies.len() > MAX_PROXIES {
            out.push(format!("Only the first {MAX_PROXIES} proxies are used."));
        }
        if self.enabled && self.via == Via::OwnList && !self.proxies.iter().any(|p| super::proxies::normalize(p).is_ok()) {
            out.push(super::text::NO_PROXIES.into());
        }
        if self.dns == DnsSetting::Encrypted && self.doh_provider == DohProvider::Custom {
            if let Err(e) = super::dns::check_custom(&self.doh_custom) {
                out.push(e);
            }
        }
        if self.dns == DnsSetting::Tor && !self.enabled {
            out.push("Looking names up through Tor needs the shield on.".into());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everything_starts_off() {
        let s = ShieldSettings::default();
        assert!(!s.enabled && !s.per_site && !s.rotate && !s.peers_relays_only && !s.random_ports);
        assert!(!s.browser_common_fingerprint && !s.browser_block_third_party_cookies && !s.browser_clear_on_close);
        assert_eq!(s.dns, DnsSetting::System);
        assert_eq!(s.rotate_minutes, 10);
        assert!(s.proxies.is_empty());
    }

    #[test]
    fn missing_fields_take_defaults() {
        let s: ShieldSettings = serde_json::from_str(r#"{"enabled":true,"rotate":true}"#).unwrap();
        assert!(s.enabled && s.rotate);
        assert_eq!(s.rotate_minutes, ROTATE_MINUTES_DEFAULT);
        assert_eq!(s.via, Via::Tor);
        let round: ShieldSettings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(round, s);
    }

    #[test]
    fn cleaned_clamps_and_tidies_the_list() {
        let s = ShieldSettings {
            rotate_minutes: 0,
            port_change_hours: 10_000,
            proxies: vec![" 127.0.0.1:1080 ".into(), "socks5h://127.0.0.1:1080".into(), "nonsense".into(), "http://10.0.0.2:3128".into()],
            ..Default::default()
        }
        .cleaned();
        assert_eq!(s.rotate_minutes, ROTATE_MINUTES_MIN);
        assert_eq!(s.port_change_hours, PORT_HOURS_MAX);
        assert_eq!(s.proxies, vec!["socks5h://127.0.0.1:1080".to_string(), "http://10.0.0.2:3128".to_string()]);
    }

    #[test]
    fn problems_are_named() {
        let s = ShieldSettings { enabled: true, via: Via::OwnList, proxies: vec!["nonsense".into()], ..Default::default() };
        let p = s.problems();
        assert_eq!(p.len(), 2, "{p:?}");
        assert!(p[0].starts_with("\"nonsense\""));
        assert!(ShieldSettings::default().problems().is_empty());
        let dns = ShieldSettings { dns: DnsSetting::Encrypted, doh_provider: DohProvider::Custom, doh_custom: "http://plain.example/dns-query".into(), ..Default::default() };
        assert_eq!(dns.problems().len(), 1);
    }
}
