// SPDX-License-Identifier: AGPL-3.0-only
//! What the shield says next to its switches. It promises only what it
//! does: the limits come first-class, not in small print, because someone
//! relying on this for safety needs to know where it stops.

pub const NO_PROXIES: &str = "Add at least one proxy or VPN address, or choose Tor.";

pub const TITLE: &str = "Address shield";

pub const SUMMARY: &str = "Hides this PC's internet address from the websites, services and people SulcusAI connects to, and makes SulcusAI harder to follow from one site to the next. It covers SulcusAI only, not other programs on this PC.";

/// Shown under the switch, always, not only when it's on.
pub const LIMITS: &[&str] = &[
    "It reduces tracking. It doesn't make anyone untraceable: what you type into a site, accounts you sign in to and files you share can still say who you are.",
    "Your internet provider (or the Wi-Fi's owner) can still see that you use Tor or a VPN, just not which sites you visit through it.",
    "People you connect to directly (People, your own VPN, file sharing) still see this PC's real address unless \"Don't show my address to peers\" is on.",
    "Some countries block or restrict Tor, and some networks (schools, workplaces) block it. Where using Tor could get you in trouble, don't turn it on.",
    "Tor is slower than a direct connection, and some sites show extra checks or refuse Tor addresses.",
];

pub const TOR: &str = "Through Tor: traffic goes through three volunteer-run relays, so sites see the last relay's address, not yours. Built in; nothing to install.";
pub const OWN_LIST: &str = "Through your own proxies or VPNs: sites see their address. They can see everything SulcusAI sends through them that isn't encrypted, so use ones you trust.";
pub const PER_SITE: &str = "A different address per website: each site gets its own Tor route, so two sites can't tell they're talking to the same PC by its address. Now and then Tor picks the same exit for two sites by chance.";
pub const PER_SITE_BROWSER: &str = "In the built-in browser, the route is chosen per server rather than per page, so an advertiser embedded on many sites can still see one address across them until the next change.";
pub const ROTATE: &str = "New address every few minutes: new connections get a new address. Connections already open (mail waiting for messages, a download in progress) keep theirs until they end.";
pub const NEW_NOW: &str = "New address now: every connection gets a new address, and open ones reconnect. Downloads in progress finish first.";
pub const PEERS: &str = "Don't show my address to peers: People, VPN and file-sharing connections go only through a relay (a friend's or your own), so others see the relay's address. Slower, and uses the relay's data.";
pub const PORTS: &str = "Random ports: the ports other devices reach SulcusAI on change at each start and now and then (never while someone is connected), so a scan doesn't find SulcusAI by its usual numbers. A phone using the local address scans the new code after a change; one linked through the relay notices nothing. SulcusAI opens no ports on your router.";
pub const DNS_TOR: &str = "Look up site names through Tor: your network's DNS server doesn't see which sites SulcusAI asks for.";
pub const DNS_ENCRYPTED: &str = "Encrypted DNS: site names are looked up over HTTPS with the provider you choose. Your network doesn't see them; that provider does.";
pub const BROWSER_FINGERPRINT: &str = "Common fingerprint: the browser reports the same values as most Windows PCs instead of this PC's own. It makes this PC blend in more; it doesn't make it identical to everyone else's.";
pub const BROWSER_COOKIES: &str = "Block third-party cookies: advertisers embedded in pages can't set cookies that follow you between sites. A few sign-ins and embedded videos may stop working.";
pub const BROWSER_CLEAR: &str = "Clear history when the browser closes: history, cookies and saved site data are deleted, so every session starts signed out.";

/// The Wi-Fi hardware (MAC) address only reaches the local network (the
/// router and anyone on that Wi-Fi), never websites, and Windows already
/// randomizes it. SulcusAI explains and opens the page; it never changes
/// system settings itself.
pub const WIFI: &str = "Your Wi-Fi hardware address is seen only by the Wi-Fi network you join, never by websites. Windows can randomize it: Settings > Network & internet > Wi-Fi > Random hardware addresses (for every network), or open a network's properties and set Random hardware address to On or Change daily. SulcusAI never changes Windows settings itself; this button only opens that page.";
/// The Windows Settings page the Wi-Fi button opens.
pub const WIFI_SETTINGS_URI: &str = "ms-settings:network-wifi";

/// Seconds as words for waits ("50 seconds", "2 minutes").
pub fn duration_words(secs: u64) -> String {
    match secs {
        0..=1 => "a moment".into(),
        2..=89 => format!("{secs} seconds"),
        90..=5399 => format!("{} minutes", (secs + 30) / 60),
        _ => format!("{} hours", (secs + 1800) / 3600),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_read_naturally() {
        assert_eq!(duration_words(0), "a moment");
        assert_eq!(duration_words(50), "50 seconds");
        assert_eq!(duration_words(90), "2 minutes");
        assert_eq!(duration_words(30 * 60), "30 minutes");
        assert_eq!(duration_words(3 * 3600), "3 hours");
    }

    #[test]
    fn the_limits_are_all_said() {
        let all = LIMITS.join(" ");
        for must in ["internet provider", "real address", "countries", "doesn't make anyone untraceable"] {
            assert!(all.contains(must), "missing: {must}");
        }
        assert!(WIFI.contains("never changes Windows settings"));
        assert!(PEERS.contains("relay"));
    }

    #[test]
    fn house_style() {
        let mut every: Vec<&str> = LIMITS.to_vec();
        every.extend([SUMMARY, TOR, OWN_LIST, PER_SITE, PER_SITE_BROWSER, ROTATE, NEW_NOW, PEERS, PORTS, DNS_TOR, DNS_ENCRYPTED, BROWSER_FINGERPRINT, BROWSER_COOKIES, BROWSER_CLEAR, WIFI, NO_PROXIES]);
        for t in every {
            assert!(!t.contains('\u{2014}'), "no em dashes: {t}");
            assert!(t.chars().all(|c| (c as u32) < 0x2190), "no arrows, symbols or emoji: {t}");
        }
    }
}
