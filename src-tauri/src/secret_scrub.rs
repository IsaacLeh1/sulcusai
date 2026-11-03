// SPDX-License-Identifier: AGPL-3.0-only
//! Takes secrets out of text before it's written to a log, a crash report or
//! a feedback report. Each one is swapped for a marker that says what was
//! there ("[removed: GitHub token]"), so the line still makes sense.
//!
//! Removed: provider keys (OpenAI-style sk-, Anthropic, Google, GitHub,
//! Slack, AWS, Stripe, Hugging Face, Groq, xAI, Replicate, Telegram bots),
//! JWTs, private key blocks, Authorization and Bearer values, passwords in
//! URLs (user:pass@), values after words like key, token, secret, password
//! or cookie, and SulcusAI's own: its local API key (sk-sulcus-), the
//! phone pairing link's key (#p=...), PC and People pairing codes (K7Q2-9XMB,
//! after the word "code" or "pair") and the data recovery code.
//!
//! Kept, on purpose: UUIDs (chat, message and job ids aren't secrets, and a
//! log needs them to follow one chat), sha256 hashes and other long hex in
//! ordinary sentences (the integrity report names them), version numbers,
//! paths, addresses without a password, emails (crash.rs's `tidy` takes out
//! the user's folder and name; cloud.rs's Redactor is for personal details).
//! A long hex or base64 run counts as a secret only after a word that says
//! so, or after "Bearer": the relay and phone tokens are 64 hex characters,
//! the same as a sha256, and only the word before tells them apart.
//!
//! Nothing is put back, unlike cloud.rs's Redactor. practice::yourcode's
//! SECRET_TEXT only asks whether a whole file holds a secret.

use std::sync::LazyLock;

use regex::Regex;

const MARK: &str = "[removed: ";

/// A rule's test of a candidate: the whole text and where the secret is.
type Test = fn(&str, usize, usize) -> bool;

struct Rule {
    label: &'static str,
    re: Regex,
    /// The capture groups that can hold the secret, first match wins
    /// (none: the whole match).
    groups: &'static [usize],
    test: Option<Test>,
}

fn rule(label: &'static str, re: &str, groups: &'static [usize], test: Option<Test>) -> Rule {
    Rule { label, re: Regex::new(re).unwrap_or_else(|e| panic!("{label}: {e}")), groups, test }
}

/// A run with a digit or a symbol in it, or long and mixed-case: what a key
/// looks like, unlike a word such as "Negotiate" or "required".
fn token_like(v: &str) -> bool {
    let n = v.chars().count();
    let digit = v.chars().any(|c| c.is_ascii_digit());
    let symbol = v.chars().any(|c| "+/=_~.-".contains(c));
    let upper = v.chars().filter(char::is_ascii_uppercase).count();
    let lower = v.chars().filter(char::is_ascii_lowercase).count();
    (n >= 8 && (digit || symbol)) || (n >= 12 && upper >= 2 && lower >= 2)
}

fn token_at(text: &str, s: usize, e: usize) -> bool {
    token_like(&text[s..e])
}

fn keyed_value(text: &str, s: usize, e: usize) -> bool {
    let v = &text[s..e];
    v.len() >= 12 && token_like(v)
}

const NOT_PASSWORDS: &[&str] = &["true", "false", "null", "none", "nil", "yes", "no", "required", "optional", "hidden", "set", "unset", "empty", "missing", "redacted", "undefined"];

fn password_value(text: &str, s: usize, e: usize) -> bool {
    let v = text[s..e].trim();
    !v.is_empty() && !v.chars().all(|c| c == '*' || c == '•') && !NOT_PASSWORDS.contains(&v.to_ascii_lowercase().as_str())
}

fn letter_and_digit(v: &str) -> bool {
    v.chars().any(|c| c.is_ascii_digit()) && v.chars().any(|c| c.is_ascii_alphabetic())
}

/// Not a piece of something longer joined by hyphens (a UUID's 4-4 middle).
fn standalone(text: &str, s: usize, e: usize) -> bool {
    text[..s].chars().next_back() != Some('-') && text[e..].chars().next() != Some('-')
}

static MARKERS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[removed: [^\]]*\]").unwrap());

/// A pairing code is 8 letters and digits, which many ordinary things are
/// too (a "B200-SXM4" card): it counts only after "code" or "pair" on the
/// same line, markers left out so a removed code doesn't vouch for the next.
fn pairing_code(text: &str, s: usize, e: usize) -> bool {
    if !standalone(text, s, e) || !letter_and_digit(&text[s..e]) {
        return false;
    }
    let line = &text[text[..s].rfind('\n').map_or(0, |i| i + 1)..s];
    let line = MARKERS.replace_all(line, "");
    let near: String = line.chars().rev().take(40).collect::<Vec<_>>().into_iter().rev().collect();
    let near = near.to_ascii_lowercase();
    near.contains("code") || near.contains("pair")
}

fn recovery_code(text: &str, s: usize, e: usize) -> bool {
    standalone(text, s, e) && letter_and_digit(&text[s..e])
}

/// Characters a value after "key=" can hold: up to a space, quote or
/// separator, and never a marker's bracket.
const VALUE: &str = r#"[^\s"'`,;&<>(){}\[\]]+"#;
/// The SulcusAI code alphabet (crypto.rs, sync): no I, L, O or U.
const CODE: &str = "[0-9A-HJKMNP-TV-Za-hjkmnp-tv-z]";

static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    vec![
        rule("private key", r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----(?:.*?-----END [A-Z0-9 ]*PRIVATE KEY-----|.*)", &[], None),
        // SulcusAI's and Anthropic's before the general sk- rule, so the marker says which.
        rule("SulcusAI API key", r"\bsk-sulcus-[A-Za-z0-9]{6,}", &[], None),
        rule("Anthropic key", r"\bsk-ant-[A-Za-z0-9_-]{16,}", &[], None),
        rule("OpenAI-style key", r"\bsk-[A-Za-z0-9_-]{20,}", &[], None),
        rule("Stripe key", r"\b(?:sk|rk)_(?:live|test)_[0-9A-Za-z]{16,}", &[], None),
        rule("Google API key", r"\bAIza[0-9A-Za-z_-]{35}", &[], None),
        rule("Google access token", r"\bya29\.[0-9A-Za-z_-]{20,}", &[], None),
        rule("GitHub token", r"\bgh[pousr]_[A-Za-z0-9]{30,}|\bgithub_pat_[A-Za-z0-9_]{30,}", &[], None),
        rule("Slack token", r"\bxox[abprs]-[A-Za-z0-9-]{10,}", &[], None),
        rule("AWS access key", r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b", &[], None),
        rule("Hugging Face token", r"\bhf_[A-Za-z0-9]{30,}", &[], None),
        rule("Groq key", r"\bgsk_[A-Za-z0-9]{30,}", &[], None),
        rule("xAI key", r"\bxai-[A-Za-z0-9]{30,}", &[], None),
        rule("Replicate token", r"\br8_[A-Za-z0-9]{30,}", &[], None),
        // No boundary before: in an address it follows "bot" (/bot123456:AA...).
        rule("Telegram bot token", r"\d{6,12}:[A-Za-z0-9_-]{30,}", &[], None),
        rule("JWT", r"\beyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]*", &[], None),
        rule("phone pairing key", r"#p=([A-Za-z0-9_-]+\.[A-Za-z0-9_-]{16,})", &[1], None),
        rule("password", r"[A-Za-z][A-Za-z0-9+.-]*://[^\s/?#@:\[\]]+:([^\s/?#@\[\]]+)@", &[1], None),
        rule("bearer token", r"(?i)\bbearer\s+([A-Za-z0-9._~+/=-]{8,})", &[1], Some(token_at)),
        rule("authorization", r#"(?i)\b(?:proxy-)?authorization["']?\s*[:=]\s*["']?(?:[A-Za-z]+\s+)?([A-Za-z0-9._~+/=-]{8,})"#, &[1], Some(token_at)),
        rule(
            "password",
            &format!(r#"(?i)\b(?:[a-z0-9]+[_.-])*(?:passw(?:or)?d|passwd|passphrase|pwd)["']?\s*[:=]\s*(?:"([^"\r\n]+)"|'([^'\r\n]+)'|({VALUE}))"#),
            &[1, 2, 3],
            Some(password_value),
        ),
        rule(
            "secret",
            &format!(
                r#"(?i)\b(?:[a-z0-9]+[_.-])*(?:api[_-]?key|apikey|key|secret|token|auth|credentials?|private[_-]?key|access[_-]?key|session[_-]?(?:id|key|token)|cookie)["']?\s*[:=]\s*["']?({VALUE})"#
            ),
            &[1],
            Some(keyed_value),
        ),
        rule("recovery code", &format!(r"\b{CODE}{{5}}(?:-{CODE}{{5}}){{3}}\b"), &[], Some(recovery_code)),
        rule("pairing code", &format!(r"\b{CODE}{{4}}-{CODE}{{4}}\b"), &[], Some(pairing_code)),
    ]
});

fn pass(text: &str, r: &Rule) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for caps in r.re.captures_iter(text) {
        let m = if r.groups.is_empty() { caps.get(0) } else { r.groups.iter().find_map(|g| caps.get(*g)) };
        let Some(m) = m else { continue };
        if m.start() < last || m.as_str().contains(MARK) {
            continue;
        }
        if let Some(test) = r.test {
            if !test(text, m.start(), m.end()) {
                continue;
            }
        }
        out.push_str(&text[last..m.start()]);
        out.push_str(MARK);
        out.push_str(r.label);
        out.push(']');
        last = m.end();
    }
    if last == 0 {
        return text.to_string();
    }
    out.push_str(&text[last..]);
    out
}

fn once(text: &str) -> String {
    RULES.iter().fold(text.to_string(), |t, r| pass(&t, r))
}

/// The text with every secret it can recognise swapped for a marker.
/// Scrubbing what's already scrubbed changes nothing.
pub fn scrub(text: &str) -> String {
    let mut out = once(text);
    // A removal can bring a code within reach of its "code" word; another
    // round settles it. Rarely more than one is needed.
    for _ in 0..4 {
        let next = once(&out);
        if next == out {
            break;
        }
        out = next;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Secrets are put together at run time, so no scanner (GitHub's, or
    /// SulcusAI's own) takes this file for one holding real keys.
    fn k(parts: &[&str]) -> String {
        parts.concat()
    }

    fn removed(text: &str, label: &str, secret: &str) {
        let out = scrub(text);
        assert!(!out.contains(secret), "{label}: still in {out:?}");
        assert!(out.contains(&format!("[removed: {label}]")), "{label}: no marker in {out:?}");
        assert_eq!(scrub(&out), out, "{label}: twice isn't once");
    }

    #[test]
    fn provider_keys_are_removed() {
        let openai = k(&["sk-", "proj-", "Xr4kT9qLm2Vb8NwZc5Yh1JdFgA7sPe3U"]);
        removed(&format!("OpenAI said 401 for key {openai}"), "OpenAI-style key", &openai);
        let router = k(&["sk-", "or-v1-", "0d2f9c7a41b85e36f0d2f9c7a41b85e36f0d2f9c7a41b85e3"]);
        removed(&format!("x-api-key: {router}"), "OpenAI-style key", "0d2f9c7a41b85e36");
        let ant = k(&["sk-", "ant-", "api03-", "Qm7Zr2Xk9LpT4vB8nW1cY6hJ3dF5gS0aE"]);
        removed(&format!("anthropic: invalid x-api-key {ant}"), "Anthropic key", &ant);
        let google = k(&["AI", "za", "SyD3x8Lk2mQ9vT7rN4bW6cZ1hJ5fG0pYsUe"]);
        removed(&format!("GET https://generativelanguage.googleapis.com/v1/models?key={google} failed"), "Google API key", &google);
        let ya = k(&["ya", "29.", "a0AfB_byC1d2E3f4G5h6I7j8K9l0"]);
        removed(&format!("drive token {ya}"), "Google access token", &ya);
        for gh in [k(&["gh", "p_", "R8x2Kq9Lm4Tz7Vb1Nw5Yc3Hd6Fg0Js8Ae2Pu"]), k(&["gh", "o_", "R8x2Kq9Lm4Tz7Vb1Nw5Yc3Hd6Fg0Js8Ae2Pu"]), k(&["github", "_pat_", "11ABCDEFG0", "_Rx8Kq2Lm9Tz4Vb7Nw1Yc5Hd3Fg6Js0Ae"])] {
            removed(&format!("git push https://x-access-token:{gh}@github.com/a/b.git"), "GitHub token", &gh);
            removed(&format!("using {gh} for the gist"), "GitHub token", &gh);
        }
        removed("git push https://x-access-token:plainpassword1@example.org/a/b.git", "password", "plainpassword1");
        for slack in [k(&["xo", "xb-", "1234567890-0987654321-AbCdEfGhIjKlMnOpQrStUv"]), k(&["xo", "xp-", "1234567890-0987654321-AbCdEfGh"])] {
            removed(&format!("slack {slack} ok"), "Slack token", &slack);
        }
        let aws = k(&["AKIA", "IOSFODNN7", "EXAMPLE"]);
        removed(&format!("aws id {aws} region us-west-2"), "AWS access key", &aws);
        let aws_secret = k(&["wJalrXUtnFEMI/K7MDENG/", "bPxRfiCYEXAMPLEKEY"]);
        removed(&format!("AWS_SECRET_ACCESS_KEY={aws_secret}"), "secret", &aws_secret);
        let stripe = k(&["sk", "_live_", "51H8xQ2Lm9Tz4Vb7Nw1Yc5Hd"]);
        removed(&format!("stripe {stripe}"), "Stripe key", &stripe);
        let hf = k(&["hf", "_", "aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789"]);
        removed(&format!("huggingface token {hf} (read)"), "Hugging Face token", &hf);
        let groq = k(&["gs", "k_", "aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789"]);
        removed(&format!("groq {groq}"), "Groq key", &groq);
        let tg = k(&["7012345678", ":", "AAHk3x9Lm2Qv8Tz4Nw1Yc5Bd7Fg0Js6Pe2Ur"]);
        removed(&format!("POST https://api.telegram.org/bot{tg}/getUpdates: 409"), "Telegram bot token", &tg);
    }

    #[test]
    fn tokens_and_passwords_are_removed() {
        let jwt = k(&["eyJ", "hbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9", ".", "eyJ", "zdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4ifQ", ".", "SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c"]);
        removed(&format!("cookie session={jwt}; Path=/"), "JWT", "SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c");
        removed("Set-Cookie: token=Zx9Lm2Qv8Tz4Nw1Yc5Bd7Fg0; HttpOnly", "secret", "Zx9Lm2Qv8Tz4Nw1Yc5Bd7Fg0");
        removed(&format!("got {jwt} back"), "JWT", "SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c");
        let relay = "9f3c2a7e1b5d8046c2e9a1f7b3d5c8e04a6f2b9d1c7e3a5f8b0d2c4e6a8f1b3d";
        removed(&format!("PUT /v1/box/abc/7 Authorization: Bearer {relay}"), "bearer token", relay);
        removed(&format!("headers: {{\"authorization\": \"Basic dXNlcjpwYXNzd29yZA==\"}}"), "authorization", "dXNlcjpwYXNzd29yZA==");
        removed(&format!("link_token={relay}"), "secret", relay);
        removed(&format!("{{\"api_key\": \"{relay}\"}}"), "secret", relay);
        removed("proxy http://me:hunter2!@10.0.0.5:3128 refused", "password", "hunter2");
        removed("socks5://me:S3cr3t%21@proxy.example:1080", "password", "S3cr3t%21");
        removed("PASSWORD=correct-horse-9", "password", "correct-horse-9");
        removed("smtp: password: \"two words here\"", "password", "two words here");
        removed("db_password='hunter2'", "password", "hunter2");
        removed("client_secret=0a1b2c3d4e5f6a7b8c9d", "secret", "0a1b2c3d4e5f6a7b8c9d");
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA7\nabc\n-----END RSA PRIVATE KEY-----";
        let out = scrub(&format!("before\n{pem}\nafter"));
        assert_eq!(out, "before\n[removed: private key]\nafter");
        assert_eq!(scrub("-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA (cut off"), "[removed: private key]");
    }

    #[test]
    fn sulcusai_own_keys_are_removed() {
        let local = k(&["sk-", "sulcus-", "3f9a1c7e5b2d4086a1c3e5f7b9d0e2a4"]);
        removed(&format!("API server key {local}"), "SulcusAI API key", &local);
        removed("curl -H \"Authorization: Bearer sk-sulcus-abc123\" http://127.0.0.1:7340/v1/models", "SulcusAI API key", "sk-sulcus-abc123");
        let link = "https://sulcusai-relay.example.workers.dev/l/0a1b2c3d4e5f60718293a4b5c6d7e8f9/#p=6f1e2d3c4b5a6978.Qm9vYmFyQmF6UXV4MTIzNDU2Nzg5MGFiY2RlZmdoaWo";
        let out = scrub(&format!("phone link {link}"));
        assert!(out.ends_with("/#p=[removed: phone pairing key]"), "{out}");
        assert!(out.contains("/l/0a1b2c3d4e5f60718293a4b5c6d7e8f9/"), "the link id isn't the key: {out}");
        removed("Pairing with code K7Q2-9XMB on DESKTOP-1", "pairing code", "K7Q2-9XMB");
        removed("pair_device code=\"k7q2-9xmb\" target=None", "pairing code", "k7q2-9xmb");
        removed("codes: K7Q2-9XMB, then 4HWZ-2PNA", "pairing code", "4HWZ-2PNA");
        removed("Recovery code: 7KQ2M-9XMBT-H4PNA-2RZ8C", "recovery code", "7KQ2M-9XMBT-H4PNA-2RZ8C");
    }

    #[test]
    fn ordinary_text_is_left_alone() {
        for text in [
            "Loaded Qwen3-8B-Q4_K_M.gguf on Vulkan0 (NVIDIA GeForce RTX 5070) in 3.2 s, 49/49 layers",
            r"C:\Users\me\AppData\Local\SulcusAI\engines\b11450-vulkan-x64\llama-server.exe",
            "/home/me/.local/share/sulcusai/engines/b11450-linux-vulkan-x64/llama-server",
            "sha256 8ce71f060c08cc8b89115eab0d63209d80537936706544d95c3b936fd0f63095 matched the catalog",
            "expected 5f21f3e6d86b18f4a3177d4fb24de452f84d093f62afbe231e0d13194ca7132c, found 8ce71f060c08cc8b89115eab0d63209d80537936706544d95c3b936fd0f63095",
            "SulcusAI 0.9.49 (v1.0.0-beta.2, build b11450, commit daf8823)",
            "chat 550e8400-e29b-41d4-a716-446655440000 saved; message 6F9619FF-8B86-D011-B42D-00C04FC964FF",
            "Opened chat 7d3c0a9e2b1f4c6d8e0a2b4c6d8e0f1a with 12 messages",
            "max_tokens=512, top_k=40, temperature=0.7, n_ctx=16384",
            "The key to good prompts is clear words; the token count was 1024.",
            "Bearer bonds and well-known keys are explained in chapter 4.",
            "https://huggingface.co/Qwen/Qwen3-8B-GGUF/resolve/main/Qwen3-8B-Q4_K_M.gguf",
            "http://127.0.0.1:7340/v1/models answered 200 at 12:20:05 on 2026-11-03",
            "the task-skeleton and risk-assessment folders, disk-cache and ask-first",
            "Password: required, token: none, key: Enter, secret: true, password=****",
            "NVIDIA B200-SXM4 192 GB, Intel(R) UHD Graphics 770",
            "Write to someone@example.com or open mailto:someone@example.com",
            "monkey=banana keyboard=us passkeys are off",
            "Unpaired the previous phone: only one phone can be paired",
            "error code 0x80070005 (access is denied)",
            "",
            "Zürich 東京 ✓ naïve text",
        ] {
            assert_eq!(scrub(text), text, "changed: {text}");
        }
    }

    /// A small generator, so the test needs no crate and repeats exactly.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn pick<'a>(&mut self, from: &[&'a str]) -> &'a str {
            from[(self.next() % from.len() as u64) as usize]
        }
    }

    #[test]
    fn random_text_never_panics_and_twice_is_once() {
        let pieces = [
            "sk-", "sk-ant-", "sk-sulcus-", "AIza", "ghp_", "github_pat_", "xoxb-", "AKIA", "eyJ", ".", "-", "_", ":", "@", "://", "#p=", "Bearer ", "Basic ",
            "Authorization: ", "password=", "\"", "'", "token: ", "api_key=", "code ", "pair ", "K7Q2", "9XMB", "7KQ2M", "[removed: ", "]", "\n", " ", "=", "/",
            "é", "東", "\u{0}", "-----BEGIN PRIVATE KEY-----", "-----END PRIVATE KEY-----", "0123456789abcdef", "ABCDEFGHJKMNPQRS", "aB3", "%", "\\", "\r\n",
        ];
        let mut rng = Rng(0x5eed_1234_abcd_0042);
        for _ in 0..3000 {
            let mut text = String::new();
            for _ in 0..(rng.next() % 40) {
                text.push_str(rng.pick(&pieces));
                if rng.next() % 3 == 0 {
                    let len = rng.next() % 40;
                    let chars = b"abcdefghijkmnpqrstuvwxyzABCDEFGHJKMNPQRSTVWXYZ0123456789+/=_-";
                    text.extend((0..len).map(|_| char::from(chars[(rng.next() % chars.len() as u64) as usize])));
                }
            }
            let once = scrub(&text);
            assert_eq!(scrub(&once), once, "from {text:?}");
        }
        // Arbitrary bytes made into text.
        for _ in 0..2000 {
            let bytes: Vec<u8> = (0..rng.next() % 200).map(|_| rng.next() as u8).collect();
            let text = String::from_utf8_lossy(&bytes).into_owned();
            let once = scrub(&text);
            assert_eq!(scrub(&once), once);
        }
    }
}
