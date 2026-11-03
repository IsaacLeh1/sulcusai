//! Password health, worked out entirely on this PC, and new passwords.

use std::collections::HashMap;

use serde::Serialize;
use zeroize::Zeroizing;

use super::store::Entry;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Strength {
    VeryWeak,
    Weak,
    Fair,
    Strong,
}

// The most used passwords in public leak lists, lowercased. Anything built on
// one of these falls to the first minutes of any guessing run.
const COMMON: &[&str] = &[
    "123456", "password", "12345678", "qwerty", "123456789", "12345", "1234", "111111", "1234567", "dragon",
    "123123", "baseball", "abc123", "football", "monkey", "letmein", "696969", "shadow", "master", "666666",
    "qwertyuiop", "123321", "mustang", "1234567890", "michael", "654321", "superman", "1qaz2wsx", "7777777",
    "121212", "000000", "qazwsx", "123qwe", "killer", "trustno1", "jordan", "jennifer", "zxcvbnm", "asdfgh",
    "hunter", "buster", "soccer", "harley", "batman", "andrew", "tigger", "sunshine", "iloveyou", "2000",
    "charlie", "robert", "thomas", "hockey", "ranger", "daniel", "starwars", "klaster", "112233", "george",
    "computer", "michelle", "jessica", "pepper", "1111", "zxcvbn", "555555", "11111111", "131313", "freedom",
    "777777", "pass", "maggie", "159753", "aaaaaa", "ginger", "princess", "joshua", "cheese", "amanda",
    "summer", "love", "ashley", "nicole", "chelsea", "biteme", "matthew", "access", "yankees", "987654321",
    "dallas", "austin", "thunder", "taylor", "matrix", "welcome", "admin", "login", "passw0rd", "p@ssw0rd",
    "password1", "qwerty123", "abcdef", "abcd1234", "secret", "changeme", "default", "guest", "root", "test",
    "football1", "baseball1", "monkey1", "letmein1", "iloveyou1", "princess1", "sunshine1", "whatever",
    "asdf", "asdfasdf", "qwer1234", "1q2w3e4r", "1q2w3e", "zaq12wsx", "q1w2e3r4", "aa123456", "password123",
];

/// Undoes the usual substitutions so "P@ssw0rd!" is seen as "password".
fn unleet(s: &str) -> String {
    s.chars()
        .map(|c| match c.to_ascii_lowercase() {
            '@' | '4' => 'a',
            '3' => 'e',
            '1' | '!' | '|' => 'i',
            '0' => 'o',
            '$' | '5' => 's',
            '7' => 't',
            c => c,
        })
        .collect()
}

pub fn is_common(password: &str) -> bool {
    let lower = password.to_lowercase();
    let core = lower.trim_end_matches(|c: char| c.is_ascii_digit() || c.is_ascii_punctuation());
    let core = core.trim_start_matches(|c: char| c.is_ascii_digit() || c.is_ascii_punctuation());
    COMMON.iter().any(|w| *w == lower || *w == core || unleet(&lower) == unleet(w) || (!core.is_empty() && unleet(core) == unleet(w)))
}

/// A rough guess at how hard the password is to guess, in the spirit of
/// zxcvbn but much smaller: character pool times length, after discounting
/// repeats, keyboard and alphabet runs, common words and the site's own name
/// or the username.
pub fn strength(password: &str, context: &[&str]) -> Strength {
    if password.is_empty() || is_common(password) {
        return Strength::VeryWeak;
    }
    let chars: Vec<char> = password.chars().collect();
    let mut pool = 0u32;
    if chars.iter().any(|c| c.is_ascii_lowercase()) {
        pool += 26;
    }
    if chars.iter().any(|c| c.is_ascii_uppercase()) {
        pool += 26;
    }
    if chars.iter().any(|c| c.is_ascii_digit()) {
        pool += 10;
    }
    if chars.iter().any(|c| c.is_ascii_punctuation() || *c == ' ') {
        pool += 33;
    }
    if chars.iter().any(|c| !c.is_ascii()) {
        pool += 100;
    }

    let mut effective = 0f64;
    for (i, c) in chars.iter().enumerate() {
        let prev = i.checked_sub(1).map(|j| chars[j]);
        let repeat = prev == Some(*c);
        let run = prev.is_some_and(|p| {
            let d = (*c as i64) - (p as i64);
            (d == 1 || d == -1) && c.is_ascii_alphanumeric() && p.is_ascii_alphanumeric()
        });
        effective += if repeat || run { 0.25 } else { 1.0 };
    }

    let lower = unleet(&password.to_lowercase());
    let mut words: Vec<&str> = COMMON.iter().copied().filter(|w| w.len() >= 4 && !w.bytes().all(|b| b.is_ascii_digit())).collect();
    words.extend(["qwert", "asdf", "zxcv", "1234", "abcd"]);
    let mut owned: Vec<String> = Vec::new();
    for c in context {
        let c = c.to_lowercase();
        // The site's own name ("github" for github.com) or the username's
        // local part is the first thing an attacker tries.
        let main = c.split(['@', '.', '/', ':']).filter(|p| p.len() >= 3 && !matches!(*p, "https" | "http" | "www" | "com" | "org" | "net")).max_by_key(|p| p.len());
        if let Some(m) = main {
            owned.push(unleet(m));
        }
    }
    for w in words.iter().map(|w| unleet(w)).chain(owned) {
        if w.len() >= 4 && lower.contains(&w) {
            effective -= (w.chars().count() as f64 - 1.0).max(0.0);
        }
    }
    let effective = effective.max(1.0);
    let bits = effective * f64::from(pool.max(10)).log2();
    match bits {
        b if b < 30.0 => Strength::VeryWeak,
        b if b < 45.0 => Strength::Weak,
        b if b < 65.0 => Strength::Fair,
        _ => Strength::Strong,
    }
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct CheckReport {
    /// Entry ids with their strength, for the weak and very weak ones.
    pub weak: Vec<(String, Strength)>,
    /// Groups of entry ids that share one password.
    pub reused: Vec<Vec<String>>,
    /// Entry ids whose password hasn't changed in `OLD_AFTER_DAYS`.
    pub old: Vec<String>,
}

pub const OLD_AFTER_DAYS: i64 = 365;

/// The report carries ids only, so it can be shown or kept without holding
/// any password.
pub fn check(entries: &[Entry], now_ms: i64) -> CheckReport {
    let mut report = CheckReport::default();
    let mut by_password: HashMap<&str, Vec<String>> = HashMap::new();
    let cutoff = now_ms - OLD_AFTER_DAYS * 24 * 60 * 60 * 1000;
    for e in entries {
        if e.password.is_empty() {
            continue;
        }
        let s = strength(&e.password, &[&e.origin, &e.username]);
        if s <= Strength::Weak {
            report.weak.push((e.id.clone(), s));
        }
        by_password.entry(e.password.as_str()).or_default().push(e.id.clone());
        if e.changed_ms > 0 && e.changed_ms < cutoff {
            report.old.push(e.id.clone());
        }
    }
    report.reused = by_password.into_values().filter(|ids| ids.len() > 1).collect();
    report.reused.sort();
    report
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(default)]
pub struct GenOptions {
    pub length: usize,
    pub lower: bool,
    pub upper: bool,
    pub digits: bool,
    pub symbols: bool,
    /// Leaves out characters that are easy to misread when typed by hand.
    pub avoid_ambiguous: bool,
}

impl Default for GenOptions {
    fn default() -> Self {
        GenOptions { length: 20, lower: true, upper: true, digits: true, symbols: true, avoid_ambiguous: false }
    }
}

pub const MIN_LENGTH: usize = 8;
pub const MAX_LENGTH: usize = 128;
const AMBIGUOUS: &str = "Il1O0o|`'\"";
// Symbols that sign-up forms almost always accept.
const SYMBOLS: &str = "!#$%&*+-.:;=?@^_~";

/// Uniform in `0..n` from the OS generator, by rejection, so no character is
/// more likely than another.
fn uniform(n: u32) -> u32 {
    use aes_gcm::aead::rand_core::RngCore;
    debug_assert!(n > 0);
    let zone = u32::MAX - (u32::MAX % n);
    loop {
        let r = aes_gcm::aead::OsRng.next_u32();
        if r < zone {
            return r % n;
        }
    }
}

/// A random password with at least one character from each chosen group.
pub fn generate(opts: &GenOptions) -> Zeroizing<String> {
    let mut groups: Vec<Vec<char>> = Vec::new();
    let mut add = |on: bool, set: &str| {
        if on {
            let g: Vec<char> = set.chars().filter(|c| !opts.avoid_ambiguous || !AMBIGUOUS.contains(*c)).collect();
            if !g.is_empty() {
                groups.push(g);
            }
        }
    };
    add(opts.lower, "abcdefghijklmnopqrstuvwxyz");
    add(opts.upper, "ABCDEFGHIJKLMNOPQRSTUVWXYZ");
    add(opts.digits, "0123456789");
    add(opts.symbols, SYMBOLS);
    if groups.is_empty() {
        groups.push("abcdefghijkmnpqrstuvwxyz".chars().collect());
    }
    let length = opts.length.clamp(MIN_LENGTH, MAX_LENGTH).max(groups.len());
    let all: Vec<char> = groups.iter().flatten().copied().collect();
    let mut out: Zeroizing<Vec<char>> = Zeroizing::new(Vec::with_capacity(length));
    for g in &groups {
        out.push(g[uniform(g.len() as u32) as usize]);
    }
    while out.len() < length {
        out.push(all[uniform(all.len() as u32) as usize]);
    }
    for i in (1..out.len()).rev() {
        let j = uniform(i as u32 + 1) as usize;
        out.swap(i, j);
    }
    let mut s = Zeroizing::new(String::with_capacity(length));
    s.extend(out.iter());
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, password: &str, changed_ms: i64) -> Entry {
        let mut e = Entry::login("https://example.com", "", password);
        e.id = id.into();
        e.changed_ms = changed_ms;
        e
    }

    #[test]
    fn strength_ranks_obvious_cases() {
        for p in ["password", "P@ssw0rd!", "123456", "qwerty123", "Password1", "aaaaaaaaaaaa", "abcdefghijk"] {
            assert!(strength(p, &[]) <= Strength::Weak, "{p}");
        }
        assert!(strength("github2024!", &["https://github.com"]) <= Strength::Weak);
        assert!(strength("k7#Qm2!vX9pL$w4Z", &[]) == Strength::Strong);
        assert!(strength("correct horse battery staple", &[]) >= Strength::Fair);
    }

    #[test]
    fn report_finds_weak_reused_and_old() {
        let now = 1_800_000_000_000;
        let day = 24 * 60 * 60 * 1000;
        let entries = vec![
            entry("a", "k7#Qm2!vX9pL$w4Z", now - day),
            entry("b", "k7#Qm2!vX9pL$w4Z", now - 400 * day),
            entry("c", "letmein", now),
            entry("d", "Zr8!nq3$Lp0@wT6y", now),
        ];
        let r = check(&entries, now);
        assert_eq!(r.reused, vec![vec!["a".to_string(), "b".to_string()]]);
        assert_eq!(r.old, vec!["b".to_string()]);
        assert_eq!(r.weak, vec![("c".to_string(), Strength::VeryWeak)]);
    }

    #[test]
    fn generated_passwords_follow_options() {
        let p = generate(&GenOptions::default());
        assert_eq!(p.chars().count(), 20);
        assert!(p.chars().any(|c| c.is_ascii_lowercase()));
        assert!(p.chars().any(|c| c.is_ascii_uppercase()));
        assert!(p.chars().any(|c| c.is_ascii_digit()));
        assert!(p.chars().any(|c| SYMBOLS.contains(c)));
        assert_eq!(strength(&p, &[]), Strength::Strong);

        let digits = generate(&GenOptions { length: 3, lower: false, upper: false, symbols: false, ..Default::default() });
        assert_eq!(digits.len(), MIN_LENGTH);
        assert!(digits.chars().all(|c| c.is_ascii_digit()));

        let plain = generate(&GenOptions { length: 500, avoid_ambiguous: true, ..Default::default() });
        assert_eq!(plain.len(), MAX_LENGTH);
        assert!(!plain.chars().any(|c| AMBIGUOUS.contains(c)));

        let none = generate(&GenOptions { lower: false, upper: false, digits: false, symbols: false, ..Default::default() });
        assert_eq!(none.len(), 20);
        assert_ne!(*generate(&GenOptions::default()), *generate(&GenOptions::default()));
    }

    #[test]
    fn generator_is_not_visibly_biased() {
        let mut counts = [0u32; 10];
        let opts = GenOptions { length: 100, lower: false, upper: false, symbols: false, ..Default::default() };
        for _ in 0..200 {
            for c in generate(&opts).chars() {
                counts[c.to_digit(10).unwrap() as usize] += 1;
            }
        }
        // 2,000 expected per digit; a fair generator stays well inside this.
        assert!(counts.iter().all(|n| (1700..2300).contains(n)), "{counts:?}");
    }
}
