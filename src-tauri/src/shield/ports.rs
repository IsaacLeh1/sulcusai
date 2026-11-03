// SPDX-License-Identifier: AGPL-3.0-only
//! Random listening ports. SulcusAI's fixed port numbers are a fingerprint:
//! anyone scanning a network for 47816 or 47818 finds SulcusAI. With this
//! on, the listeners other machines can reach take a random port at each
//! start and a new one now and then, never while someone is connected.
//!
//! Ports come from the dynamic range (49152-65535), which no service
//! registers. Windows reserves blocks inside it for Hyper-V and WSL, so a
//! bind can fail with "access denied" there; the picker just tries another
//! candidate, and the caller falls back to letting Windows choose (port 0).
//!
//! SulcusAI opens no ports on the router (no UPnP), so there are none to
//! close when they aren't needed.

use super::ShieldSettings;

pub const LOW: u16 = 49152;
pub const HIGH: u16 = 65535;
/// Enough tries that Windows' reserved blocks can't plausibly use them all
/// up, few enough that a failing network stack gives up quickly.
pub const ATTEMPTS: usize = 32;

/// Every server SulcusAI runs, and whether the shield moves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listener {
    /// The phone page on the local network (fixed 47818 otherwise).
    PhonePage,
    /// Your devices and People direct connections (fixed 47816 otherwise);
    /// discovery tells the others which port it is.
    Devices,
    /// The discovery broadcast port: the others find this PC by its number,
    /// so it can't move.
    Discovery,
    /// The local API and Ollama-style server: only on 127.0.0.1, invisible
    /// to the network, and other programs are set up with their number.
    LocalApi,
    LocalOllama,
    /// The Tor gateway, previews, sign-in callbacks and engines already take
    /// a port Windows picks on 127.0.0.1.
    AlreadyRandom,
}

/// Whether the shield gives this listener a random port.
pub fn randomized(s: &ShieldSettings, l: Listener) -> bool {
    s.enabled && s.random_ports && matches!(l, Listener::PhonePage | Listener::Devices)
}

/// A port in the range, other than those in `avoid` (the one in use now, so
/// a change really changes it).
pub fn candidate(avoid: &[u16], random: &mut impl FnMut() -> u64) -> u16 {
    let span = u64::from(HIGH - LOW) + 1;
    loop {
        let p = LOW + (random() % span) as u16;
        if !avoid.contains(&p) {
            return p;
        }
    }
}

/// A random port that `is_free` accepts (the caller binds it there and
/// keeps the listener, so nothing can take the port in between), or None
/// after `ATTEMPTS` tries: then bind port 0.
pub fn pick(avoid: &[u16], random: &mut impl FnMut() -> u64, mut is_free: impl FnMut(u16) -> bool) -> Option<u16> {
    (0..ATTEMPTS).map(|_| candidate(avoid, random)).find(|&p| is_free(p))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortDecision {
    Keep,
    ChangeNow,
    /// Due, but someone is connected: change once they leave.
    WaitUntilIdle,
}

/// Whether a running listener should move to a new port. `since` is when it
/// took its current port; times are seconds on a steady clock.
pub fn decide(s: &ShieldSettings, l: Listener, since: u64, now: u64, open_connections: usize) -> PortDecision {
    if !randomized(s, l) {
        return PortDecision::Keep;
    }
    let hours = s.port_change_hours.clamp(super::settings::PORT_HOURS_MIN, super::settings::PORT_HOURS_MAX);
    if now < since + u64::from(hours) * 3600 {
        PortDecision::Keep
    } else if open_connections > 0 {
        PortDecision::WaitUntilIdle
    } else {
        PortDecision::ChangeNow
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on() -> ShieldSettings {
        ShieldSettings { enabled: true, random_ports: true, ..Default::default() }
    }

    fn counter(start: u64) -> impl FnMut() -> u64 {
        let mut n = start;
        move || {
            n = n.wrapping_add(1);
            n
        }
    }

    #[test]
    fn only_network_facing_listeners_move() {
        let s = on();
        assert!(randomized(&s, Listener::PhonePage));
        assert!(randomized(&s, Listener::Devices));
        for l in [Listener::Discovery, Listener::LocalApi, Listener::LocalOllama, Listener::AlreadyRandom] {
            assert!(!randomized(&s, l), "{l:?}");
        }
        assert!(!randomized(&ShieldSettings::default(), Listener::PhonePage));
        assert!(!randomized(&ShieldSettings { random_ports: true, ..Default::default() }, Listener::PhonePage), "shield off");
    }

    #[test]
    fn candidates_stay_in_the_dynamic_range() {
        let mut r = counter(0);
        for _ in 0..1000 {
            let p = candidate(&[], &mut r);
            assert!((LOW..=HIGH).contains(&p));
        }
        let mut wraps = counter(u64::MAX - 5);
        assert!((LOW..=HIGH).contains(&candidate(&[], &mut wraps)));
        let mut uuid = super::super::random_u64;
        assert!((LOW..=HIGH).contains(&candidate(&[], &mut uuid)));
    }

    #[test]
    fn the_port_in_use_is_avoided() {
        let mut zero = || 0u64;
        assert_eq!(candidate(&[], &mut zero), LOW);
        let mut seq = counter(u64::MAX);
        assert_eq!(candidate(&[LOW], &mut seq), LOW + 1, "LOW skipped for the next number");
    }

    #[test]
    fn pick_skips_busy_ports_and_gives_up() {
        let mut r = counter(0);
        let busy = [LOW + 1, LOW + 2];
        assert_eq!(pick(&[], &mut r, |p| !busy.contains(&p)), Some(LOW + 3));
        let mut tries = 0;
        assert_eq!(pick(&[], &mut r, |_| { tries += 1; false }), None);
        assert_eq!(tries, ATTEMPTS);
    }

    #[test]
    fn changes_when_due_and_idle() {
        let s = ShieldSettings { port_change_hours: 2, ..on() };
        assert_eq!(decide(&s, Listener::PhonePage, 0, 3600, 0), PortDecision::Keep);
        assert_eq!(decide(&s, Listener::PhonePage, 0, 7200, 0), PortDecision::ChangeNow);
        assert_eq!(decide(&s, Listener::PhonePage, 0, 7200, 1), PortDecision::WaitUntilIdle);
        assert_eq!(decide(&s, Listener::LocalApi, 0, 1_000_000, 0), PortDecision::Keep);
        assert_eq!(decide(&ShieldSettings::default(), Listener::Devices, 0, 1_000_000, 0), PortDecision::Keep);
        let zero = ShieldSettings { port_change_hours: 0, ..on() };
        assert_eq!(decide(&zero, Listener::Devices, 0, 3599, 0), PortDecision::Keep, "at least an hour");
    }
}
