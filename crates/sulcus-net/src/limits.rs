// SPDX-License-Identifier: AGPL-3.0-only
//! Caps for every PC or relay that carries others' traffic: data a day and a
//! month, speed (token buckets, total and per person), connections, and the
//! proof-of-work stamp strangers pay before being helped. Always on; the
//! numbers can be changed but not switched off.

use std::collections::HashMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const DAY: u64 = 86_400;
const MONTH: u64 = 30 * DAY;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Caps {
    pub day_bytes: u64,
    pub month_bytes: u64,
    /// Bytes a second for everyone together, and for one person.
    pub total_rate: u64,
    pub person_rate: u64,
    pub connections: u32,
    pub person_connections: u32,
}

impl Default for Caps {
    /// A PC helping a group.
    fn default() -> Self {
        Caps { day_bytes: 2_000_000_000, month_bytes: 60_000_000_000, total_rate: 10_000_000, person_rate: 2_000_000, connections: 64, person_connections: 16 }
    }
}

impl Caps {
    /// A Sulcus Relay: 20 GB a day, stopping at 8 TB a month (under Oracle's free 10 TB).
    pub fn relay() -> Self {
        Caps { day_bytes: 20_000_000_000, month_bytes: 8_000_000_000_000, total_rate: 50_000_000, ..Caps::default() }
    }
}

/// Bytes carried in the current day and month windows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meter {
    pub day_start: u64,
    pub day: u64,
    pub month_start: u64,
    pub month: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    /// Over the day's cap: refuse until the day turns.
    DayFull,
    /// Over the month's cap: stop carrying until the month turns.
    MonthFull,
}

impl Meter {
    fn roll(&mut self, now: u64) {
        if now >= self.day_start + DAY {
            self.day_start = now - (now - self.day_start) % DAY.max(1);
            self.day = 0;
        }
        if now >= self.month_start + MONTH {
            self.month_start = now;
            self.month = 0;
        }
    }

    /// Whether `bytes` more may be carried now, counting them if so.
    pub fn take(&mut self, caps: &Caps, bytes: u64, now: u64) -> Verdict {
        if self.day_start == 0 && self.month_start == 0 {
            self.day_start = now;
            self.month_start = now;
        }
        self.roll(now);
        if self.month.saturating_add(bytes) > caps.month_bytes {
            return Verdict::MonthFull;
        }
        if self.day.saturating_add(bytes) > caps.day_bytes {
            return Verdict::DayFull;
        }
        self.day += bytes;
        self.month += bytes;
        Verdict::Ok
    }
}

/// A token bucket: up to one second of `rate` saved up.
#[derive(Debug, Clone)]
pub struct Bucket {
    rate: u64,
    tokens: f64,
    at: Instant,
}

impl Bucket {
    pub fn new(rate: u64) -> Self {
        Bucket { rate, tokens: rate as f64, at: Instant::now() }
    }

    /// How long to wait before sending `bytes` (zero: send now). Slowing
    /// down comes before refusing.
    pub fn wait(&mut self, bytes: u64, now: Instant) -> std::time::Duration {
        let gained = now.saturating_duration_since(self.at).as_secs_f64() * self.rate as f64;
        self.tokens = (self.tokens + gained).min(self.rate as f64);
        self.at = now;
        self.tokens -= bytes as f64;
        if self.tokens >= 0.0 || self.rate == 0 {
            std::time::Duration::ZERO
        } else {
            std::time::Duration::from_secs_f64(-self.tokens / self.rate as f64)
        }
    }
}

/// Open connections, in total and per person.
#[derive(Debug, Default)]
pub struct Connections {
    per: HashMap<String, u32>,
    total: u32,
}

impl Connections {
    pub fn open(&mut self, caps: &Caps, who: &str) -> bool {
        let n = self.per.get(who).copied().unwrap_or(0);
        if self.total >= caps.connections || n >= caps.person_connections {
            return false;
        }
        self.per.insert(who.to_string(), n + 1);
        self.total += 1;
        true
    }

    pub fn close(&mut self, who: &str) {
        if let Some(n) = self.per.get_mut(who) {
            *n -= 1;
            self.total = self.total.saturating_sub(1);
            if *n == 0 {
                self.per.remove(who);
            }
        }
    }
}

const STAMP: &[u8] = b"SulcusAI admit v1";
/// About a million hashes: under a second on a PC.
pub const BITS_AT_REST: u32 = 20;

fn stamp_hash(helper: &str, stranger: &str, challenge: &[u8], nonce: u64) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(STAMP);
    h.update(helper.as_bytes());
    h.update(stranger.as_bytes());
    h.update(challenge);
    h.update(nonce.to_be_bytes());
    h.finalize().into()
}

fn zero_bits(h: &[u8; 32]) -> u32 {
    let mut n = 0;
    for b in h {
        if *b == 0 {
            n += 8;
        } else {
            return n + b.leading_zeros();
        }
    }
    n
}

/// Whether a stranger's stamp has at least `bits` leading zero bits.
pub fn stamp_ok(helper: &str, stranger: &str, challenge: &[u8], nonce: u64, bits: u32) -> bool {
    zero_bits(&stamp_hash(helper, stranger, challenge, nonce)) >= bits
}

/// The stranger's side: find a nonce.
pub fn make_stamp(helper: &str, stranger: &str, challenge: &[u8], bits: u32) -> u64 {
    (0u64..).find(|n| stamp_ok(helper, stranger, challenge, *n, bits)).unwrap_or(0)
}

/// Stamps get harder as load nears the caps: one more bit per tenth past half.
pub fn bits_for(load: f64) -> u32 {
    let extra = ((load - 0.5).max(0.0) * 10.0).floor() as u32;
    (BITS_AT_REST + extra).min(28)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_stops_at_day_and_month() {
        let caps = Caps { day_bytes: 100, month_bytes: 250, ..Caps::default() };
        let mut m = Meter::default();
        assert_eq!(m.take(&caps, 80, 1000), Verdict::Ok);
        assert_eq!(m.take(&caps, 30, 1001), Verdict::DayFull);
        assert_eq!(m.take(&caps, 90, 1000 + DAY), Verdict::Ok, "a new day");
        assert_eq!(m.take(&caps, 90, 1000 + 2 * DAY), Verdict::MonthFull, "80 + 90 + 90 > 250");
        assert_eq!(m.take(&caps, 90, 1000 + MONTH), Verdict::Ok, "a new month");
        assert!(Caps::relay().month_bytes < 10_000_000_000_000);
    }

    #[test]
    fn bucket_slows_down() {
        let t = Instant::now();
        let mut b = Bucket::new(1000);
        assert_eq!(b.wait(1000, t), std::time::Duration::ZERO);
        let w = b.wait(500, t);
        assert!(w >= std::time::Duration::from_millis(499) && w <= std::time::Duration::from_millis(501));
    }

    #[test]
    fn connection_caps() {
        let caps = Caps { connections: 3, person_connections: 2, ..Caps::default() };
        let mut c = Connections::default();
        assert!(c.open(&caps, "a") && c.open(&caps, "a"));
        assert!(!c.open(&caps, "a"), "per person");
        assert!(c.open(&caps, "b"));
        assert!(!c.open(&caps, "c"), "total");
        c.close("a");
        assert!(c.open(&caps, "c"));
    }

    #[test]
    fn stamps() {
        let n = make_stamp("h", "s", b"chal", 12);
        assert!(stamp_ok("h", "s", b"chal", n, 12));
        assert_eq!(bits_for(0.2), BITS_AT_REST);
        assert_eq!(bits_for(0.75), BITS_AT_REST + 2);
        assert_eq!(bits_for(5.0), 28);
    }
}
