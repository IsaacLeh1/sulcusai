// SPDX-License-Identifier: AGPL-3.0-only
//! Connectivity levels. Every request that leaves the PC asks this module first.

use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Connectivity {
    /// Nothing leaves the PC (default).
    #[default]
    Offline,
    /// Web access for search, browsing and account sync; AI stays local.
    Web,
    /// Cloud AI providers and cloud sync are allowed too.
    Cloud,
}

// Cloud is used once cloud providers (phase 6) land.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// Engine or model download the user started. Allowed at every level:
    /// it only contacts the file host and carries no user data.
    ModelDownload,
    /// Web search, page fetches, browser, email/calendar sync.
    Web,
    /// Cloud AI providers, cloud sync relay.
    Cloud,
}

/// `chat_web` is the per-chat globe toggle, which turns web on for one chat
/// while the app stays Offline.
pub fn allowed(level: Connectivity, purpose: Purpose, chat_web: bool) -> bool {
    match purpose {
        Purpose::ModelDownload => true,
        Purpose::Web => level != Connectivity::Offline || chat_web,
        Purpose::Cloud => level == Connectivity::Cloud,
    }
}

pub fn blocked_message(purpose: Purpose) -> &'static str {
    match purpose {
        Purpose::ModelDownload => "",
        Purpose::Web => "This needs web access, which is off. Turn on web for this chat or switch to Local AI + Web.",
        Purpose::Cloud => "This needs a cloud provider, which is off. Switch to the Cloud level in Settings to use it.",
    }
}

/// Client for requests that leave the PC; refuses when the level forbids it.
pub fn external_client(level: Connectivity, purpose: Purpose, chat_web: bool) -> Result<reqwest::Client, String> {
    if !allowed(level, purpose, chat_web) {
        return Err(blocked_message(purpose).to_string());
    }
    reqwest::Client::builder()
        .user_agent(concat!("SulcusAI/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())
}

/// Client for the app's own engines on 127.0.0.1. Ignores system proxies,
/// which would otherwise intercept localhost traffic.
pub fn local_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("local HTTP client")
}

#[cfg(test)]
mod tests {
    use super::*;
    use Connectivity::*;

    #[test]
    fn offline_blocks_web_and_cloud() {
        assert!(!allowed(Offline, Purpose::Web, false));
        assert!(!allowed(Offline, Purpose::Cloud, false));
        assert!(allowed(Offline, Purpose::ModelDownload, false));
    }

    #[test]
    fn chat_globe_enables_web_but_never_cloud() {
        assert!(allowed(Offline, Purpose::Web, true));
        assert!(!allowed(Offline, Purpose::Cloud, true));
        assert!(!allowed(Web, Purpose::Cloud, true));
    }

    #[test]
    fn levels_are_cumulative() {
        assert!(allowed(Web, Purpose::Web, false));
        assert!(allowed(Cloud, Purpose::Web, false));
        assert!(allowed(Cloud, Purpose::Cloud, false));
    }

    #[test]
    fn external_client_refuses_when_blocked() {
        assert!(external_client(Offline, Purpose::Web, false).is_err());
        assert!(external_client(Offline, Purpose::ModelDownload, false).is_ok());
    }
}
