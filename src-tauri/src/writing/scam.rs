// SPDX-License-Identifier: AGPL-3.0-only
//! Scam check: a message, a link or a sender read against the signs scams
//! share (pressure, unusual ways to pay, asking for codes or passwords, links
//! that pretend to be somewhere else). Rules, not a model, so it is the same
//! every time and nothing leaves the PC. It says what it noticed and why; it
//! never says a message is safe, only that it found no common signs.

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Sign {
    pub kind: &'static str,
    pub why: String,
    pub weight: u8,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Verdict {
    /// 0 to 100.
    pub score: u8,
    /// "no_signs", "careful" or "likely_scam".
    pub level: &'static str,
    pub signs: Vec<Sign>,
}

/// Names scammers often dress up as, to spot lookalike hosts.
const BRANDS: &[&str] = &[
    "paypal", "amazon", "apple", "microsoft", "google", "netflix", "facebook", "instagram", "whatsapp", "chase",
    "wellsfargo", "bankofamerica", "usps", "fedex", "dhl", "ups", "irs", "coinbase", "binance", "steam", "outlook",
];
const SHORTENERS: &[&str] = &["bit.ly", "tinyurl.com", "t.co", "goo.gl", "ow.ly", "is.gd", "cutt.ly", "rb.gy", "shorturl.at"];

static LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)\b(?:https?://|www\.)[^\s<>"')]+"#).unwrap());

fn phrase_signs(lower: &str, out: &mut Vec<Sign>) {
    let groups: &[(&str, u8, &[&str], &str)] = &[
        ("pressure", 15, &["act now", "within 24 hours", "immediately", "urgent", "final notice", "account will be closed", "suspended", "last chance", "expires today"], "It pushes you to act fast, so there's no time to check."),
        ("payment", 30, &["gift card", "itunes card", "google play card", "steam card", "wire transfer", "western union", "moneygram", "bitcoin", "crypto wallet", "usdt", "zelle me"], "It asks for a way of paying that can't be taken back."),
        ("secrets", 30, &["verification code", "one-time code", " otp ", "your password", "confirm your password", "pin number", "social security number", " ssn ", "login details", "seed phrase", "recovery phrase"], "It asks for a code, password or number that only you should know."),
        ("prize", 20, &["you have won", "you've won", "congratulations! you", "claim your prize", "lottery", "inheritance", "unclaimed funds"], "It offers money or a prize you didn't enter for."),
        ("remote", 25, &["anydesk", "teamviewer", "remote access", "install this app to"], "It asks to see or control your computer."),
        ("secrecy", 15, &["don't tell", "do not tell", "keep this between us", "keep it confidential"], "It asks you to keep it secret from others."),
        ("refund", 15, &["refund", "overpaid", "send back the difference"], "It's about a refund or overpayment, a common setup."),
    ];
    for (kind, weight, words, why) in groups {
        if words.iter().any(|w| lower.contains(w)) {
            out.push(Sign { kind, why: (*why).to_string(), weight: *weight });
        }
    }
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push((prev[j] + usize::from(ca != *cb)).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// The registrable-looking part of a host: "login.paypal.com" -> "paypal".
fn main_label(host: &str) -> &str {
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() >= 3 && ["co", "com", "org", "net", "gov"].contains(&parts[parts.len() - 2]) && parts[parts.len() - 1].len() == 2 {
        parts[parts.len() - 3]
    } else if parts.len() >= 2 {
        parts[parts.len() - 2]
    } else {
        host
    }
}

pub fn host_signs(host: &str, out: &mut Vec<Sign>) {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.parse::<std::net::IpAddr>().is_ok() {
        out.push(Sign { kind: "link", why: format!("A link goes to a bare number address ({host}), not a named site."), weight: 25 });
        return;
    }
    if host.split('.').any(|l| l.starts_with("xn--")) {
        out.push(Sign { kind: "link", why: format!("{host} uses letters from other alphabets that can look like ordinary ones."), weight: 30 });
    }
    if SHORTENERS.contains(&host.as_str()) {
        out.push(Sign { kind: "link", why: format!("{host} hides where the link really goes."), weight: 10 });
    }
    let label = main_label(&host);
    let undigit: String = label.chars().map(|c| match c { '0' => 'o', '1' => 'l', '3' => 'e', '5' => 's', '4' => 'a', c => c }).collect();
    for b in BRANDS {
        if label == *b {
            continue;
        }
        let looks = undigit == *b || (b.len() >= 5 && edit_distance(label, b) == 1) || (b.len() >= 5 && label.contains(b));
        if looks {
            out.push(Sign { kind: "lookalike", why: format!("{host} looks like {b} but isn't its address."), weight: 35 });
            break;
        }
    }
    // The brand sitting in a subdomain of some other site: paypal.com.example.ru
    if BRANDS.iter().any(|b| host.split('.').rev().skip(2).any(|l| l == *b && *b != label)) {
        out.push(Sign { kind: "lookalike", why: format!("{host} puts a well-known name in front of a different site."), weight: 35 });
    }
}

pub fn links(text: &str) -> Vec<String> {
    LINK.find_iter(text).map(|m| m.as_str().trim_end_matches(['.', ',', ';', ':']).to_string()).collect()
}

fn host_of(link: &str) -> Option<String> {
    let with = if link.contains("://") { link.to_string() } else { format!("https://{link}") };
    reqwest::Url::parse(&with).ok()?.host_str().map(str::to_string)
}

/// `sender` is the From address if there is one; a brand named in the text
/// that doesn't match the sender's site is a sign.
pub fn check(text: &str, sender: Option<&str>) -> Verdict {
    let lower = text.to_lowercase();
    let mut signs = Vec::new();
    phrase_signs(&lower, &mut signs);
    let mut seen = std::collections::HashSet::new();
    for l in links(text) {
        if let Some(h) = host_of(&l) {
            if seen.insert(h.clone()) {
                host_signs(&h, &mut signs);
            }
        }
    }
    if let Some(from) = sender.and_then(|s| s.rsplit('@').next()).map(|d| d.trim_end_matches('>').to_ascii_lowercase()) {
        host_signs(&from, &mut signs);
        let free = ["gmail.com", "outlook.com", "hotmail.com", "yahoo.com", "icloud.com", "proton.me", "aol.com"].contains(&from.as_str());
        if let Some(b) = BRANDS.iter().find(|b| lower.contains(&format!("{b} ")) || lower.contains(&format!("{b} team")) || lower.contains(&format!("your {b}"))) {
            if main_label(&from) != *b {
                let how = if free { "a free mail address" } else { "a different site" };
                signs.push(Sign { kind: "sender", why: format!("It speaks for {b} but comes from {how} ({from})."), weight: 30 });
            }
        }
    }
    let pushy = signs.iter().any(|s| s.kind == "pressure") && signs.iter().any(|s| matches!(s.kind, "payment" | "secrets" | "remote"));
    // Pressure together with an ask is the classic shape, worth more than both apart.
    let score = (signs.iter().map(|s| s.weight as u32).sum::<u32>() + if pushy { 15 } else { 0 }).min(100) as u8;
    let level = match score {
        0..=19 => "no_signs",
        20..=49 => "careful",
        _ => "likely_scam",
    };
    signs.sort_by(|a, b| b.weight.cmp(&a.weight));
    Verdict { score, level, signs }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_message_has_no_signs() {
        let v = check("Hi, are we still on for lunch Thursday? See https://www.openstreetmap.org/ for the place.", Some("sam@example.org"));
        assert_eq!(v.level, "no_signs", "{:?}", v.signs);
    }

    #[test]
    fn gift_card_pressure_is_a_scam() {
        let v = check("URGENT: your account will be closed. Buy a gift card and send the code within 24 hours.", None);
        assert_eq!(v.level, "likely_scam");
        assert!(v.signs.iter().any(|s| s.kind == "payment"));
    }

    #[test]
    fn lookalike_hosts() {
        let mut s = Vec::new();
        host_signs("paypa1.com", &mut s);
        assert_eq!(s[0].kind, "lookalike");
        s.clear();
        host_signs("paypal.com.secure-login.ru", &mut s);
        assert!(s.iter().any(|x| x.kind == "lookalike"));
        s.clear();
        host_signs("www.paypal.com", &mut s);
        assert!(s.is_empty(), "{s:?}");
        s.clear();
        host_signs("amazon.co.uk", &mut s);
        assert!(s.is_empty(), "{s:?}");
        s.clear();
        host_signs("xn--pple-43d.com", &mut s);
        assert!(!s.is_empty());
        s.clear();
        host_signs("192.168.4.20", &mut s);
        assert_eq!(s[0].kind, "link");
    }

    #[test]
    fn brand_from_a_free_mailbox() {
        let v = check("Dear customer, your PayPal account is limited. Log in at http://bit.ly/x1", Some("PayPal <support.paypal@gmail.com>"));
        assert!(v.signs.iter().any(|s| s.kind == "sender"), "{:?}", v.signs);
        assert!(v.signs.iter().any(|s| s.kind == "link"));
        assert_ne!(v.level, "no_signs");
    }

    #[test]
    fn links_are_found() {
        assert_eq!(links("go to www.a.com, or (https://b.org/x)."), vec!["www.a.com".to_string(), "https://b.org/x".to_string()]);
    }
}
