// SPDX-License-Identifier: AGPL-3.0-only
//! What the built-in browser window is opened with while the shield is on.
//!
//! The browser is WebView2 (Edge). Its traffic goes through a proxy given at
//! creation; Chromium can't send a SOCKS user name, so it can't use the Tor
//! gateway's password-protected entry or tag each request with its site. It
//! needs a loopback entry without a password, and the gateway then isolates
//! by the server each connection is for. That is weaker than isolating by
//! the page in the address bar: a tracker embedded on many sites keeps one
//! exit address across them until the next rotation. Blocking third-party
//! cookies and rotating are what cover that.

use super::{Path, ShieldSettings};

/// Tauri passes these to WebView2 by default and drops them once any
/// arguments are set, so they are repeated to keep the browser as it was.
const TAURI_DEFAULT_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection";

/// WebRTC would otherwise send UDP straight out and can show this PC's real
/// addresses to any page, proxy or not.
const NO_WEBRTC_LEAK: &str = "--force-webrtc-ip-handling-policy=disable_non_proxied_udp";

/// QUIC is UDP, which a SOCKS proxy can't carry; switching it off keeps every
/// connection on TCP through the proxy instead of failing over to direct.
const NO_QUIC: &str = "--disable-quic";

/// Values most Windows PCs report, in place of this PC's own (CPU threads
/// and memory are among the most identifying). Only the page and its frames
/// see the change: web workers still read the real thread count, and sites
/// that ask for detailed client hints in HTTP headers still get them.
pub const COMMON_FINGERPRINT_SCRIPT: &str = r#"(() => {
  const fix = (o, k, v) => { try { Object.defineProperty(o, k, { get: () => v, configurable: true }); } catch (_) {} };
  fix(Navigator.prototype, 'hardwareConcurrency', 8);
  fix(Navigator.prototype, 'deviceMemory', 8);
  const ua = navigator.userAgentData;
  if (ua && ua.getHighEntropyValues) {
    const low = { brands: ua.brands, mobile: ua.mobile, platform: ua.platform };
    try { Object.defineProperty(Object.getPrototypeOf(ua), 'getHighEntropyValues', { value: () => Promise.resolve({ ...low }), configurable: true }); } catch (_) {}
  }
})();"#;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BrowserPlan {
    /// For `WebviewWindowBuilder::proxy_url`; None: direct.
    pub proxy: Option<String>,
    /// For `WebviewWindowBuilder::additional_browser_args`; empty: Tauri's defaults.
    pub args: Vec<String>,
    /// Run in every frame before the page's own scripts.
    pub init_script: Option<&'static str>,
    pub block_third_party_cookies: bool,
    /// Clear history, cookies and cache when the browser window closes.
    pub clear_on_close: bool,
}

/// The browser's settings for this shield, or why it can't open: when the
/// shield sends traffic through Tor or a proxy, the browser must too, and
/// without a usable `browser_proxy` (the gateway's password-free loopback
/// entry, or the person's proxy) it doesn't open rather than going direct.
pub fn plan(s: &ShieldSettings, path: Path, browser_proxy: Option<&str>) -> Result<BrowserPlan, String> {
    if !s.enabled {
        return Ok(BrowserPlan::default());
    }
    let proxy = match path {
        Path::Direct => None,
        Path::Tor | Path::OwnProxy => {
            let p = browser_proxy.ok_or_else(|| "The browser can't go through the address shield yet, so it stays closed instead of showing this PC's address.".to_string())?;
            let u = reqwest::Url::parse(p).map_err(|_| "The browser's proxy address isn't usable.".to_string())?;
            if !u.username().is_empty() || u.password().is_some() {
                return Err("The browser can't sign in to a proxy. It needs one without a user name and password.".into());
            }
            let scheme = match u.scheme() {
                // Chromium's SOCKS5 always looks names up at the proxy.
                "socks5" | "socks5h" => "socks5",
                "http" => "http",
                _ => return Err("The browser's proxy must be socks5:// or http://.".into()),
            };
            let (Some(host), Some(port)) = (u.host_str(), u.port_or_known_default()) else {
                return Err("The browser's proxy needs a host and a port.".into());
            };
            Some(format!("{scheme}://{host}:{port}"))
        }
    };
    let mut args = Vec::new();
    if proxy.is_some() || s.browser_common_fingerprint {
        args = vec![TAURI_DEFAULT_ARGS.to_string(), NO_WEBRTC_LEAK.to_string()];
        if proxy.is_some() {
            args.push(NO_QUIC.to_string());
        }
    }
    Ok(BrowserPlan {
        proxy,
        args,
        init_script: s.browser_common_fingerprint.then_some(COMMON_FINGERPRINT_SCRIPT),
        block_third_party_cookies: s.browser_block_third_party_cookies,
        clear_on_close: s.browser_clear_on_close,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on() -> ShieldSettings {
        ShieldSettings { enabled: true, ..Default::default() }
    }

    #[test]
    fn shield_off_changes_nothing() {
        let s = ShieldSettings { browser_common_fingerprint: true, browser_clear_on_close: true, ..Default::default() };
        assert_eq!(plan(&s, Path::Direct, None).unwrap(), BrowserPlan::default());
    }

    #[test]
    fn tor_without_a_browser_entry_stays_closed() {
        assert!(plan(&on(), Path::Tor, None).is_err());
        assert!(plan(&on(), Path::OwnProxy, None).is_err());
    }

    #[test]
    fn proxy_is_used_without_leaks() {
        let p = plan(&on(), Path::Tor, Some("socks5h://127.0.0.1:45001")).unwrap();
        assert_eq!(p.proxy.as_deref(), Some("socks5://127.0.0.1:45001"));
        assert!(p.args.iter().any(|a| a == NO_WEBRTC_LEAK));
        assert!(p.args.iter().any(|a| a == NO_QUIC));
        assert!(p.args.iter().any(|a| a == TAURI_DEFAULT_ARGS), "keeps Tauri's own");
        let http = plan(&on(), Path::OwnProxy, Some("http://10.0.0.2:3128")).unwrap();
        assert_eq!(http.proxy.as_deref(), Some("http://10.0.0.2:3128"));
    }

    #[test]
    fn proxies_the_browser_cant_use_are_refused() {
        assert!(plan(&on(), Path::Tor, Some("socks5h://u:p@127.0.0.1:1")).is_err(), "no SOCKS sign-in in Chromium");
        assert!(plan(&on(), Path::OwnProxy, Some("socks4://127.0.0.1:1")).is_err());
        assert!(plan(&on(), Path::OwnProxy, Some("socks5://127.0.0.1")).is_err(), "no port");
    }

    #[test]
    fn browser_options_follow_their_switches() {
        let s = ShieldSettings { browser_common_fingerprint: true, browser_block_third_party_cookies: true, browser_clear_on_close: true, ..on() };
        let p = plan(&s, Path::Direct, None).unwrap();
        assert_eq!(p.proxy, None);
        assert_eq!(p.init_script, Some(COMMON_FINGERPRINT_SCRIPT));
        assert!(p.args.iter().any(|a| a == NO_WEBRTC_LEAK));
        assert!(!p.args.iter().any(|a| a == NO_QUIC), "QUIC is fine without a proxy");
        assert!(p.block_third_party_cookies && p.clear_on_close);
        let plain = plan(&on(), Path::Direct, None).unwrap();
        assert!(plain.args.is_empty() && plain.init_script.is_none() && !plain.clear_on_close);
    }
}
