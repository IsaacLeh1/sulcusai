//! When a saved password may be filled. The rule is deliberately narrow: the
//! page's origin must be exactly the saved origin, over https, in the top
//! frame or a frame of that same origin. No suffix, subdomain or "close
//! enough" matching, because that is exactly what phishing pages rely on.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    pub scheme: String,
    pub host: String,
    pub port: u16,
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let default = match self.scheme.as_str() {
            "https" => 443,
            "http" => 80,
            _ => 0,
        };
        if self.port == default {
            write!(f, "{}://{}", self.scheme, self.host)
        } else {
            write!(f, "{}://{}:{}", self.scheme, self.host, self.port)
        }
    }
}

/// Parses the origin of an http or https address. Refuses anything a browser
/// would show differently from what it means: user info before the host
/// (`https://bank.com@evil.com`), raw Unicode hosts (browsers hand over
/// punycode), percent signs, empty labels and control characters.
pub fn parse_origin(url: &str) -> Option<Origin> {
    let url = url.trim();
    if url.chars().any(|c| c.is_control() || c == ' ') {
        return None;
    }
    let colon = url.find(':')?;
    let scheme = url[..colon].to_ascii_lowercase();
    let default_port = match scheme.as_str() {
        "https" => 443,
        "http" => 80,
        _ => return None,
    };
    let rest = url[colon + 1..].strip_prefix("//")?;
    let end = rest.find(['/', '\\', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    if authority.contains('@') {
        return None;
    }
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let close = v6.find(']')?;
        let host = &v6[..close];
        if host.is_empty() || !host.chars().all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.') {
            return None;
        }
        let after = &v6[close + 1..];
        let port = match after {
            "" => None,
            p => Some(p.strip_prefix(':')?),
        };
        (format!("[{}]", host.to_ascii_lowercase()), port)
    } else {
        let (h, p) = match authority.rfind(':') {
            Some(i) => (&authority[..i], Some(&authority[i + 1..])),
            None => (authority, None),
        };
        (check_host(h)?, p)
    };
    let port = match port {
        None | Some("") => default_port,
        Some(p) if p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()) => p.parse::<u16>().ok()?,
        Some(_) => return None,
    };
    Some(Origin { scheme, host, port })
}

fn check_host(host: &str) -> Option<String> {
    let host = host.to_ascii_lowercase();
    let host = host.strip_suffix('.').unwrap_or(&host);
    if host.is_empty() || host.len() > 253 || !host.is_ascii() {
        return None;
    }
    for label in host.split('.') {
        if label.is_empty() || label.len() > 63 || label.starts_with('-') || label.ends_with('-') {
            return None;
        }
        if !label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
            return None;
        }
    }
    Some(host.to_string())
}

/// The form an origin is stored in, or `None` for anything that isn't a
/// plain http or https address.
pub fn normalize_origin(url: &str) -> Option<String> {
    parse_origin(url).map(|o| o.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The saved entry or the page isn't a well-formed https address.
    NotHttps,
    BadAddress,
    /// A frame inside the page comes from another site than the page.
    FrameFromOtherSite,
    /// A different site that resembles the saved one.
    Lookalike,
    OtherSite,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Refusal::NotHttps => "Passwords are only filled on https pages.",
            Refusal::BadAddress => "This page's address can't be checked.",
            Refusal::FrameFromOtherSite => "This sign-in box is from another site embedded in the page.",
            Refusal::Lookalike => "This site looks like the saved one but isn't it.",
            Refusal::OtherSite => "This password was saved for another site.",
        })
    }
}

/// Whether the password saved for `saved` may go into a box in a frame at
/// `frame_url` on a page whose address bar shows `top_url`.
pub fn may_fill(saved: &str, frame_url: &str, top_url: &str) -> Result<(), Refusal> {
    let saved = parse_origin(saved).ok_or(Refusal::BadAddress)?;
    let top = parse_origin(top_url).ok_or(Refusal::BadAddress)?;
    let frame = parse_origin(frame_url).ok_or(Refusal::BadAddress)?;
    if saved.scheme != "https" || top.scheme != "https" || frame.scheme != "https" {
        return Err(Refusal::NotHttps);
    }
    if frame != top {
        return Err(Refusal::FrameFromOtherSite);
    }
    if frame == saved {
        return Ok(());
    }
    if frame.host != saved.host && looks_like(&saved.host, &frame.host) {
        return Err(Refusal::Lookalike);
    }
    Err(Refusal::OtherSite)
}

/// Only picks the wording of a refusal; the decision itself is exact match.
fn looks_like(saved: &str, page: &str) -> bool {
    if page.starts_with(&format!("{saved}.")) || page.contains(&format!("-{saved}")) || page.contains(&format!("{saved}-")) {
        return true;
    }
    if page.split('.').any(|l| l.starts_with("xn--")) && !saved.split('.').any(|l| l.starts_with("xn--")) {
        return true;
    }
    let (a, b) = (skeleton(saved), skeleton(page));
    a == b || (a.len() >= 5 && edit_distance(&a, &b) <= 2)
}

fn skeleton(host: &str) -> String {
    let s: String = host
        .chars()
        .filter(|c| *c != '-')
        .map(|c| match c {
            '0' => 'o',
            '1' | 'i' => 'l',
            '3' => 'e',
            '5' => 's',
            '4' => 'a',
            c => c,
        })
        .collect();
    s.replace("rn", "m").replace("vv", "w")
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) > 2 {
        return 3;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(ca != cb)).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAVED: &str = "https://www.paypal.com";

    fn fill(page: &str) -> Result<(), Refusal> {
        may_fill(SAVED, page, page)
    }

    #[test]
    fn fills_the_exact_site_only() {
        assert_eq!(fill("https://www.paypal.com/signin?x=1"), Ok(()));
        assert_eq!(fill("https://WWW.PayPal.com./signin"), Ok(()));
        assert_eq!(fill("https://www.paypal.com:443/"), Ok(()));
        assert_eq!(fill("https://www.paypal.com#login"), Ok(()));
    }

    #[test]
    fn refuses_lookalikes() {
        for page in [
            "https://www.paypa1.com/",
            "https://www.paypal.com.evil.example/",
            "https://www.paypal.co/",
            "https://www.paypai.com/",
            "https://www.pay-pal.com/",
            "https://www.xn--pypal-4ve.com/",
            "https://www.paypal.com-secure.example/",
            "https://secure-www.paypal.com.example/",
        ] {
            assert_eq!(fill(page), Err(Refusal::Lookalike), "{page}");
        }
    }

    #[test]
    fn refuses_other_sites_and_subdomains() {
        assert_eq!(fill("https://example.com/"), Err(Refusal::OtherSite));
        assert!(fill("https://paypal.com/").is_err());
        assert!(fill("https://evil.www.paypal.com/").is_err());
        assert!(fill("https://www.paypal.com:8443/").is_err());
    }

    #[test]
    fn refuses_plain_http() {
        assert_eq!(fill("http://www.paypal.com/"), Err(Refusal::NotHttps));
        assert_eq!(may_fill("http://www.paypal.com", "http://www.paypal.com/", "http://www.paypal.com/"), Err(Refusal::NotHttps));
        assert_eq!(may_fill("http://www.paypal.com", SAVED, SAVED), Err(Refusal::NotHttps));
    }

    #[test]
    fn refuses_frames_from_other_sites() {
        assert_eq!(may_fill(SAVED, SAVED, "https://evil.example/"), Err(Refusal::FrameFromOtherSite));
        assert_eq!(may_fill(SAVED, "https://evil.example/", SAVED), Err(Refusal::FrameFromOtherSite));
        assert_eq!(may_fill(SAVED, "https://www.paypal.com/frame", "https://www.paypal.com/page"), Ok(()));
    }

    #[test]
    fn refuses_tricky_addresses() {
        for page in [
            "https://www.paypal.com@evil.example/",
            "https://evil.example\\@www.paypal.com/",
            "https://www.paypal.com%2eevil.example/",
            "https://www.p\u{430}ypal.com/",
            "https://www..paypal.com/",
            "https://www.paypal.com:99999/",
            "https://www.paypal.com:44x/",
            "javascript:alert(1)//https://www.paypal.com",
            "data:text/html,https://www.paypal.com",
            "https://www.paypal.com\n.evil.example/",
            "",
            "https://",
            "https:www.paypal.com",
        ] {
            assert!(fill(page).is_err(), "{page:?}");
        }
        assert_eq!(fill("https://evil.example\\@www.paypal.com/"), Err(Refusal::OtherSite));
    }

    #[test]
    fn origins_normalize() {
        assert_eq!(normalize_origin("HTTPS://Example.COM:443/a/b?c").as_deref(), Some("https://example.com"));
        assert_eq!(normalize_origin("https://example.com:8443").as_deref(), Some("https://example.com:8443"));
        assert_eq!(normalize_origin("http://[::1]:8080/x").as_deref(), Some("http://[::1]:8080"));
        assert_eq!(normalize_origin("android://abc@com.example.app/"), None);
        assert_eq!(normalize_origin("example.com"), None);
    }

    #[test]
    fn never_panics_on_odd_input() {
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let alphabet: Vec<char> = "htps:/[]@.\\%#?-x1:0é\u{0}a".chars().collect();
        for _ in 0..20_000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let len = (seed % 40) as usize;
            let mut s = String::new();
            let mut r = seed;
            for _ in 0..len {
                r = r.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                s.push(alphabet[(r >> 33) as usize % alphabet.len()]);
            }
            let _ = may_fill(SAVED, &s, &s);
            let _ = normalize_origin(&s);
        }
    }
}
