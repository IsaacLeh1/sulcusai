// SPDX-License-Identifier: AGPL-3.0-only
//! Words the shield's core says itself. What the window shows next to the
//! switches (the limits first, not in small print) lives in
//! `src/components/ShieldSettings.tsx`, where it can be translated.

pub const NO_PROXIES: &str = "Add at least one proxy or VPN address, or choose Tor.";

/// The Windows Settings page the Wi-Fi button opens: Windows randomizes
/// the hardware address itself, and SulcusAI never changes system settings.
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
}
