// SPDX-License-Identifier: AGPL-3.0-only
//! Waits after wrong PINs (or wrong pairing codes) that grow with each
//! mistake. The state is plain data saved with the device list, so quitting
//! and starting SulcusAI again does not reset the count.

use serde::{Deserialize, Serialize};

use super::DeviceError;

/// A typo or two shouldn't cost anything.
pub const FREE_TRIES: u32 = 3;
pub const FIRST_WAIT_MS: i64 = 30_000;
/// Long enough to make guessing a 6-digit PIN take years, short enough that
/// the right person is never shut out for a whole day.
pub const MAX_WAIT_MS: i64 = 60 * 60_000;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lockout {
    fails: u32,
    wait_until: i64,
}

impl Lockout {
    pub fn fails(&self) -> u32 {
        self.fails
    }

    /// Milliseconds left to wait. Clamped so that a clock set back after a
    /// restart can't turn one wait into a much longer one.
    pub fn wait_ms(&self, now: i64) -> i64 {
        (self.wait_until - now).clamp(0, MAX_WAIT_MS)
    }

    pub fn ready(&self, now: i64) -> Result<(), DeviceError> {
        match self.wait_ms(now) {
            0 => Ok(()),
            ms => Err(DeviceError::Wait { secs: ((ms + 999) / 1000) as u64 }),
        }
    }

    /// Counts a mistake. Returns true when this one started a wait, the
    /// moment worth telling the person about.
    pub fn failed(&mut self, now: i64) -> bool {
        self.fails = self.fails.saturating_add(1);
        if self.fails <= FREE_TRIES {
            return false;
        }
        let doublings = (self.fails - FREE_TRIES - 1).min(20);
        let wait = FIRST_WAIT_MS.saturating_mul(1i64 << doublings).min(MAX_WAIT_MS);
        self.wait_until = now + wait;
        true
    }

    pub fn succeeded(&mut self) {
        *self = Lockout::default();
    }
}
