// SPDX-License-Identifier: AGPL-3.0-only
//! The person's own list of proxies (a VPN app's local proxy counts), taken
//! in turn. A proxy that stops answering is set aside for a while and the
//! next one is used; when none answers, requests wait with a reason instead
//! of quietly going out directly.

/// First pause after a proxy fails. Doubles with each failure in a row, so a
/// proxy that is down for the day isn't knocked on every minute.
pub const FIRST_PAUSE_SECS: u64 = 60;
pub const LONGEST_PAUSE_SECS: u64 = 30 * 60;

/// A proxy address as SulcusAI uses it, or why it can't be used. A bare
/// `host:port` is taken as SOCKS5. `socks5://` becomes `socks5h://` so names
/// are looked up at the proxy: looking them up here would show every site
/// name to this network's DNS server, which is what the shield is avoiding.
pub fn normalize(p: &str) -> Result<String, String> {
    let p = p.trim();
    if p.is_empty() {
        return Err("The proxy address is empty.".into());
    }
    let with_scheme = if p.contains("://") { p.to_string() } else { format!("socks5h://{p}") };
    let url = reqwest::Url::parse(&with_scheme).map_err(|_| "That isn't a proxy address. It looks like 127.0.0.1:1080 or socks5://host:port.".to_string())?;
    let scheme = match url.scheme() {
        "socks5" | "socks5h" => "socks5h",
        "http" => "http",
        "https" => "https",
        "socks4" | "socks4a" => return Err("SOCKS4 proxies can't carry everything SulcusAI sends. Use the SOCKS5 or HTTP address your proxy or VPN gives.".into()),
        other => return Err(format!("\"{other}\" proxies aren't supported. Use socks5:// or http://.")),
    };
    let (Some(host), Some(port)) = (url.host_str(), url.port_or_known_default()) else {
        return Err("The proxy address needs a host and a port, like 127.0.0.1:1080.".into());
    };
    if !(url.path().is_empty() || url.path() == "/") || url.query().is_some() {
        return Err("A proxy address has only a host and a port, no path.".into());
    }
    let auth = match (url.username(), url.password()) {
        ("", None) => String::new(),
        (u, None) => format!("{u}@"),
        (u, Some(pw)) => format!("{u}:{pw}@"),
    };
    Ok(format!("{scheme}://{auth}{host}:{port}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Down {
    until: u64,
    strikes: u32,
}

/// The list in use and which proxy is current. Times are seconds on any
/// steady clock the caller keeps; only differences matter.
#[derive(Debug, Clone)]
pub struct ProxyRing {
    entries: Vec<String>,
    current: usize,
    down: Vec<Option<Down>>,
}

impl ProxyRing {
    /// `start` picks the first proxy (taken modulo the length): starting at
    /// a random place means each session doesn't open on the same proxy.
    pub fn new(entries: Vec<String>, start: usize) -> Self {
        let n = entries.len();
        ProxyRing { current: if n == 0 { 0 } else { start % n }, down: vec![None; n], entries }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn up(&self, i: usize, now: u64) -> bool {
        self.down[i].is_none_or(|d| now >= d.until)
    }

    /// The first usable proxy at or after `from`, going round the list.
    fn next_up(&self, from: usize, now: u64) -> Option<usize> {
        let n = self.entries.len();
        (0..n).map(|k| (from + k) % n).find(|&i| self.up(i, now))
    }

    fn all_down(&self, now: u64) -> String {
        let wait = self.down.iter().flatten().map(|d| d.until.saturating_sub(now)).min().unwrap_or(0);
        format!("None of your proxies is answering, so SulcusAI waits instead of going out directly. It tries again in {}.", super::text::duration_words(wait))
    }

    /// The proxy to use now: the current one, or the next that answers.
    /// Returns its index too, for `failed` and `worked`.
    pub fn pick(&mut self, now: u64) -> Result<(usize, &str), String> {
        if self.entries.is_empty() {
            return Err(super::text::NO_PROXIES.into());
        }
        let i = self.next_up(self.current, now).ok_or_else(|| self.all_down(now))?;
        self.current = i;
        Ok((i, &self.entries[i]))
    }

    /// With "a different address per website" on, sites are spread over the
    /// list by a hash of the site, so two sites usually see two proxies. The
    /// current position is added in so rotation moves every site along.
    pub fn pick_for_site(&self, site_hash: u64, now: u64) -> Result<(usize, &str), String> {
        let n = self.entries.len();
        if n == 0 {
            return Err(super::text::NO_PROXIES.into());
        }
        let from = ((site_hash % n as u64) as usize + self.current) % n;
        let i = self.next_up(from, now).ok_or_else(|| self.all_down(now))?;
        Ok((i, &self.entries[i]))
    }

    /// Moves on to the next proxy that answers (a timed rotation or "New
    /// address now"). With one proxy, or only one answering, it stays put.
    pub fn rotate(&mut self, now: u64) -> Option<usize> {
        let n = self.entries.len();
        if n == 0 {
            return None;
        }
        let i = self.next_up((self.current + 1) % n, now)?;
        self.current = i;
        Some(i)
    }

    /// A proxy refused or didn't answer: pause it and move off it.
    pub fn failed(&mut self, i: usize, now: u64) {
        let Some(slot) = self.down.get_mut(i) else { return };
        let strikes = slot.map_or(0, |d| d.strikes) + 1;
        let pause = FIRST_PAUSE_SECS.saturating_mul(1u64 << (strikes - 1).min(16)).min(LONGEST_PAUSE_SECS);
        *slot = Some(Down { until: now + pause, strikes });
        if i == self.current {
            if let Some(next) = self.next_up(i, now) {
                self.current = next;
            }
        }
    }

    pub fn worked(&mut self, i: usize) {
        if let Some(slot) = self.down.get_mut(i) {
            *slot = None;
        }
    }

    /// The list changed in Settings: keep using the same proxy when it's
    /// still there, and keep pauses for proxies that stayed.
    pub fn replace(&mut self, entries: Vec<String>) {
        let keep = self.entries.get(self.current).cloned();
        let down = entries.iter().map(|e| self.entries.iter().position(|o| o == e).and_then(|j| self.down[j])).collect();
        self.current = keep.and_then(|k| entries.iter().position(|e| *e == k)).unwrap_or(0);
        self.entries = entries;
        self.down = down;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(n: usize) -> ProxyRing {
        ProxyRing::new((0..n).map(|i| format!("socks5h://127.0.0.1:{}", 1080 + i)).collect(), 0)
    }

    #[test]
    fn normalizes_what_people_paste() {
        assert_eq!(normalize("127.0.0.1:1080").unwrap(), "socks5h://127.0.0.1:1080");
        assert_eq!(normalize("socks5://10.0.0.2:9050").unwrap(), "socks5h://10.0.0.2:9050");
        assert_eq!(normalize("http://proxy.example:3128/").unwrap(), "http://proxy.example:3128");
        assert_eq!(normalize("socks5://me:p%40ss@127.0.0.1:1080").unwrap(), "socks5h://me:p%40ss@127.0.0.1:1080");
        assert_eq!(normalize("[::1]:1080").unwrap(), "socks5h://[::1]:1080");
        assert!(normalize("").is_err());
        assert!(normalize("proxy.example").is_err(), "no port");
        assert!(normalize("socks4://1.2.3.4:1080").is_err());
        assert!(normalize("ftp://1.2.3.4:21").is_err());
        assert!(normalize("http://1.2.3.4:8080/path").is_err());
    }

    #[test]
    fn rotation_goes_round_in_order() {
        let mut r = ring(3);
        assert_eq!(r.pick(0).unwrap().0, 0);
        assert_eq!(r.rotate(0), Some(1));
        assert_eq!(r.rotate(0), Some(2));
        assert_eq!(r.rotate(0), Some(0));
        let mut one = ring(1);
        assert_eq!(one.rotate(0), Some(0), "one proxy stays put");
        assert_eq!(ProxyRing::new(Vec::new(), 7).rotate(0), None);
        assert_eq!(ProxyRing::new(vec!["a".into(), "b".into()], 5).pick(0).unwrap().0, 1, "start is taken modulo the length");
    }

    #[test]
    fn a_failed_proxy_is_skipped_until_its_pause_ends() {
        let mut r = ring(3);
        r.failed(0, 100);
        assert_eq!(r.pick(100).unwrap().0, 1, "moved off the failed one");
        assert_eq!(r.rotate(100), Some(2));
        assert_eq!(r.rotate(100), Some(1), "0 is still paused");
        assert_eq!(r.rotate(100 + FIRST_PAUSE_SECS), Some(2));
        assert_eq!(r.rotate(100 + FIRST_PAUSE_SECS), Some(0), "pause over: tried again");
    }

    #[test]
    fn pauses_grow_and_reset_when_it_works() {
        let mut r = ring(2);
        r.failed(0, 0);
        r.failed(0, 0);
        r.failed(0, 0);
        assert_eq!(r.down[0].unwrap().until, 4 * FIRST_PAUSE_SECS);
        for _ in 0..40 {
            r.failed(0, 0);
        }
        assert_eq!(r.down[0].unwrap().until, LONGEST_PAUSE_SECS);
        r.worked(0);
        assert!(r.down[0].is_none());
    }

    #[test]
    fn all_down_waits_instead_of_going_direct() {
        let mut r = ring(2);
        r.failed(0, 0);
        r.failed(1, 0);
        let e = r.pick(10).unwrap_err();
        assert!(e.contains("instead of going out directly"), "{e}");
        assert!(e.contains("50 seconds"), "{e}");
        assert!(r.pick_for_site(3, 10).is_err());
        assert!(r.pick(FIRST_PAUSE_SECS).is_ok(), "pauses over: tried again");
        assert!(ProxyRing::new(Vec::new(), 0).pick(0).is_err());
    }

    #[test]
    fn sites_spread_over_the_list_and_move_with_rotation() {
        let mut r = ring(4);
        let a = r.pick_for_site(1, 0).unwrap().0;
        let b = r.pick_for_site(2, 0).unwrap().0;
        assert_ne!(a, b);
        r.rotate(0);
        assert_eq!(r.pick_for_site(1, 0).unwrap().0, (a + 1) % 4);
        r.failed(2, 0);
        assert_eq!(r.pick_for_site(1, 0).unwrap().0, 3, "skips the paused one");
    }

    #[test]
    fn replacing_the_list_keeps_the_current_proxy_and_pauses() {
        let mut r = ring(3);
        r.rotate(0);
        r.failed(2, 0);
        let current = r.entries[1].clone();
        let paused = r.entries[2].clone();
        r.replace(vec!["socks5h://127.0.0.1:9999".into(), paused.clone(), current.clone()]);
        assert_eq!(r.pick(0).unwrap().1, current);
        assert!(r.down[1].is_some(), "the paused proxy stays paused");
        r.replace(vec!["http://127.0.0.1:3128".into()]);
        assert_eq!(r.pick(0).unwrap().0, 0);
    }
}
