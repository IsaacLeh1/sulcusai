// SPDX-License-Identifier: AGPL-3.0-only
//! Remote control's handshake and limits, on the PC being controlled.
//! Off unless the switch is on; each request shows a code both sides must
//! see match, waits at most 30 seconds for Accept, and while it runs the
//! screen shows a border and a bar with Stop. Stop, the kill hotkey or
//! turning the switch off end it at once, and input is taken only while
//! the session is live.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const ACCEPT_MS: u64 = 30_000;

/// A code shown on both PCs, made from both ids and a fresh nonce, so a
/// third party cannot ask with someone else's code.
pub fn matching_code(controller: &str, host: &str, nonce: &[u8; 16]) -> String {
    let mut h = Sha256::new();
    h.update(b"sulcus-remote-v1\n");
    h.update(controller.as_bytes());
    h.update(b"\n");
    h.update(host.as_bytes());
    h.update(nonce);
    let d = h.finalize();
    let n = u32::from_be_bytes([d[0], d[1], d[2], d[3]]) % 1_000_000;
    format!("{n:06}")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum State {
    Idle,
    Asking { controller: String, code: String, since_ms: u64 },
    Live { controller: String, since_ms: u64 },
}

/// What the screen should show; sounds play on each change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Show {
    pub prompt: bool,
    pub border: bool,
    pub bar: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    SwitchOff,
    Busy,
    NoRequest,
    TimedOut,
    WrongCode,
}

#[derive(Debug)]
pub struct Host {
    pub enabled: bool,
    state: State,
}

impl Host {
    pub fn new(enabled: bool) -> Self {
        Host { enabled, state: State::Idle }
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn request(&mut self, controller: &str, code: &str, now_ms: u64) -> Result<(), Refused> {
        if !self.enabled {
            return Err(Refused::SwitchOff);
        }
        self.tick(now_ms);
        if self.state != State::Idle {
            return Err(Refused::Busy);
        }
        self.state = State::Asking { controller: controller.into(), code: code.into(), since_ms: now_ms };
        Ok(())
    }

    /// The person accepts after checking the code on their screen matches
    /// the one the controller sees.
    pub fn accept(&mut self, code_seen: &str, now_ms: u64) -> Result<(), Refused> {
        self.tick(now_ms);
        match &self.state {
            State::Asking { controller, code, .. } => {
                if code != code_seen {
                    self.state = State::Idle;
                    return Err(Refused::WrongCode);
                }
                self.state = State::Live { controller: controller.clone(), since_ms: now_ms };
                Ok(())
            }
            State::Idle => Err(Refused::TimedOut),
            State::Live { .. } => Err(Refused::Busy),
        }
    }

    /// Ends the request or session: Decline, Stop, the kill hotkey.
    pub fn stop(&mut self) {
        self.state = State::Idle;
    }

    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        if !on {
            self.stop();
        }
    }

    /// Requests not accepted within 10 seconds lapse.
    pub fn tick(&mut self, now_ms: u64) {
        if let State::Asking { since_ms, .. } = self.state {
            if now_ms.saturating_sub(since_ms) >= ACCEPT_MS {
                self.state = State::Idle;
            }
        }
    }

    /// Whether input from this peer may reach the PC now.
    pub fn may_input(&self, from: &str) -> bool {
        self.enabled && matches!(&self.state, State::Live { controller, .. } if controller == from)
    }

    pub fn show(&self) -> Show {
        match self.state {
            State::Idle => Show { prompt: false, border: false, bar: false },
            State::Asking { .. } => Show { prompt: true, border: false, bar: false },
            State::Live { .. } => Show { prompt: false, border: true, bar: true },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_match_on_both_sides_and_differ_per_nonce() {
        let a = matching_code("c", "h", &[1; 16]);
        assert_eq!(a.len(), 6);
        assert_eq!(a, matching_code("c", "h", &[1; 16]));
        assert_ne!(a, matching_code("c", "h", &[2; 16]));
        assert_ne!(a, matching_code("x", "h", &[1; 16]));
    }

    #[test]
    fn off_by_default_refuses() {
        let mut h = Host::new(false);
        assert_eq!(h.request("c", "123456", 0), Err(Refused::SwitchOff));
        assert!(!h.may_input("c"));
    }

    #[test]
    fn accept_within_ten_seconds_then_stop() {
        let mut h = Host::new(true);
        h.request("c", "123456", 1_000).unwrap();
        assert!(h.show().prompt);
        assert!(!h.may_input("c"));
        assert_eq!(h.request("d", "000000", 2_000), Err(Refused::Busy));
        h.accept("123456", 10_999).unwrap();
        assert_eq!(h.show(), Show { prompt: false, border: true, bar: true });
        assert!(h.may_input("c"));
        assert!(!h.may_input("d"));
        h.stop();
        assert!(!h.may_input("c"));
        assert!(!h.show().border);
    }

    #[test]
    fn late_or_wrong_code_is_refused() {
        let mut h = Host::new(true);
        h.request("c", "123456", 0).unwrap();
        assert_eq!(h.accept("123456", ACCEPT_MS), Err(Refused::TimedOut));
        h.request("c", "123456", 20_000).unwrap();
        assert_eq!(h.accept("654321", 20_001), Err(Refused::WrongCode));
        assert_eq!(h.state(), &State::Idle);
    }

    #[test]
    fn switch_off_ends_a_live_session() {
        let mut h = Host::new(true);
        h.request("c", "1", 0).unwrap();
        h.accept("1", 1).unwrap();
        h.set_enabled(false);
        assert!(!h.may_input("c"));
        assert_eq!(h.state(), &State::Idle);
    }
}
