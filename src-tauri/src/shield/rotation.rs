// SPDX-License-Identifier: AGPL-3.0-only
//! New addresses on a timer and on "New address now".
//!
//! Through Tor, a rotation starts a new generation: every site gets a new
//! isolation token, so new connections build new circuits (usually a new
//! exit address). Through the person's own list, it moves to the next proxy.
//! Either way pooled keep-alive connections are dropped, since reusing one
//! would keep the old address. A timed rotation leaves long-lived
//! connections (mail waiting for new messages, People links) alone so they
//! aren't cut every few minutes; they pick up the new address when they next
//! reconnect. "New address now" reconnects those too, because the person
//! asked for everything to change. Downloads in progress always finish
//! where they started: cutting a multi-gigabyte model download hides nothing.

use super::proxies::ProxyRing;
use super::{ShieldSettings, Via};

/// Building circuits takes a few seconds and Tor itself rate-limits fresh
/// circuits, so presses closer together than this change nothing more.
pub const MANUAL_GAP_SECS: u64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Timer,
    Manual,
}

/// What a rotation changes, for the gateway, clients and long-lived links.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RotationPlan {
    pub reason: Reason,
    /// The isolation generation requests carry from now on.
    pub generation: u64,
    /// The own-list proxy now current, when going through the list.
    pub next_proxy: Option<usize>,
    pub drop_pooled_connections: bool,
    pub reconnect_long_lived: bool,
}

/// Times are seconds on a steady clock the caller keeps.
#[derive(Debug, Clone)]
pub struct Rotator {
    generation: u64,
    last: u64,
    last_manual: Option<u64>,
}

impl Rotator {
    pub fn new(now: u64) -> Self {
        Rotator { generation: 0, last: now, last_manual: None }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn interval(s: &ShieldSettings) -> u64 {
        u64::from(s.rotate_minutes.clamp(super::settings::ROTATE_MINUTES_MIN, super::settings::ROTATE_MINUTES_MAX)) * 60
    }

    /// When the next timed rotation happens, if one is set.
    pub fn next_at(&self, s: &ShieldSettings) -> Option<u64> {
        (s.enabled && s.rotate).then(|| self.last + Self::interval(s))
    }

    fn rotate(&mut self, reason: Reason, now: u64, s: &ShieldSettings, ring: Option<&mut ProxyRing>) -> RotationPlan {
        self.generation += 1;
        self.last = now;
        let next_proxy = match (s.via, ring) {
            (Via::OwnList, Some(r)) => r.rotate(now),
            _ => None,
        };
        RotationPlan { reason, generation: self.generation, next_proxy, drop_pooled_connections: true, reconnect_long_lived: reason == Reason::Manual }
    }

    /// Called now and then (once a minute is plenty): a plan when a timed
    /// rotation is due. After sleep or hibernation it rotates once, not once
    /// per missed interval.
    pub fn tick(&mut self, s: &ShieldSettings, now: u64, ring: Option<&mut ProxyRing>) -> Option<RotationPlan> {
        let due = self.next_at(s)?;
        (now >= due).then(|| self.rotate(Reason::Timer, now, s, ring))
    }

    /// "New address now". It also restarts the timer, so a timed rotation
    /// doesn't follow a moment later.
    pub fn new_address_now(&mut self, s: &ShieldSettings, now: u64, ring: Option<&mut ProxyRing>) -> Result<RotationPlan, String> {
        if !s.enabled {
            return Err("Turn on the address shield first.".into());
        }
        if self.last_manual.is_some_and(|t| now < t + MANUAL_GAP_SECS) {
            return Err("You got a new address a moment ago. Try again in a few seconds.".into());
        }
        self.last_manual = Some(now);
        Ok(self.rotate(Reason::Manual, now, s, ring))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(rotate: bool, minutes: u32) -> ShieldSettings {
        ShieldSettings { enabled: true, rotate, rotate_minutes: minutes, ..Default::default() }
    }

    #[test]
    fn timer_rotates_on_schedule() {
        let s = on(true, 10);
        let mut r = Rotator::new(1000);
        assert_eq!(r.next_at(&s), Some(1600));
        assert!(r.tick(&s, 1599, None).is_none());
        let p = r.tick(&s, 1600, None).unwrap();
        assert_eq!(p.reason, Reason::Timer);
        assert_eq!(p.generation, 1);
        assert!(p.drop_pooled_connections && !p.reconnect_long_lived);
        assert_eq!(r.next_at(&s), Some(2200));
    }

    #[test]
    fn no_timer_when_off() {
        let mut r = Rotator::new(0);
        assert!(r.tick(&on(false, 10), 1_000_000, None).is_none());
        assert!(r.tick(&ShieldSettings { rotate: true, ..Default::default() }, 1_000_000, None).is_none(), "shield off");
        assert_eq!(r.generation(), 0);
    }

    #[test]
    fn a_long_sleep_rotates_once() {
        let s = on(true, 10);
        let mut r = Rotator::new(0);
        assert!(r.tick(&s, 10 * 3600, None).is_some());
        assert!(r.tick(&s, 10 * 3600 + 1, None).is_none());
        assert_eq!(r.generation(), 1);
    }

    #[test]
    fn interval_follows_the_setting_and_its_limits() {
        let r = Rotator::new(0);
        assert_eq!(r.next_at(&on(true, 3)), Some(180));
        assert_eq!(r.next_at(&on(true, 0)), Some(60), "at least a minute");
        assert_eq!(r.next_at(&on(true, u32::MAX)), Some(24 * 3600), "at most a day");
    }

    #[test]
    fn new_address_now_reconnects_everything_and_resets_the_timer() {
        let s = on(true, 10);
        let mut r = Rotator::new(0);
        let p = r.new_address_now(&s, 500, None).unwrap();
        assert_eq!(p.reason, Reason::Manual);
        assert!(p.reconnect_long_lived && p.drop_pooled_connections);
        assert_eq!(r.next_at(&s), Some(1100));
        assert!(r.new_address_now(&s, 505, None).is_err(), "too soon");
        assert!(r.new_address_now(&s, 510, None).is_ok());
        assert_eq!(r.generation(), 2);
        assert!(r.new_address_now(&ShieldSettings::default(), 9999, None).is_err(), "shield off");
        let manual_only = on(false, 10);
        assert!(r.new_address_now(&manual_only, 9999, None).is_ok(), "works without the timer");
    }

    #[test]
    fn own_list_moves_to_the_next_proxy() {
        let s = ShieldSettings { via: Via::OwnList, ..on(true, 10) };
        let mut ring = ProxyRing::new(vec!["socks5h://127.0.0.1:1".into(), "socks5h://127.0.0.1:2".into()], 0);
        assert_eq!(Rotator::new(0).tick(&s, 600, Some(&mut ring)).unwrap().next_proxy, Some(1));
        let tor = on(true, 10);
        assert_eq!(Rotator::new(0).tick(&tor, 600, Some(&mut ring)).unwrap().next_proxy, None, "Tor has no list");
    }
}
