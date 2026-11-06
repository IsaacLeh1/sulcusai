// SPDX-License-Identifier: AGPL-3.0-only
//! A company policy file, %ProgramData%\SulcusAI\policy.json on Windows
//! (/etc/sulcusai/policy.json elsewhere). IT puts it there with admin
//! rights; the app only reads it, and nothing in the window can change it.
//! It can limit cloud providers and models, force privacy mode, cap spend,
//! turn features off, keep everything on this PC, and set a company
//! gateway. The window shows each lock and why.
//!
//! Group Policy (the ADMX templates in packaging/policy) writes the same
//! limits under HKLM\SOFTWARE\Policies\SulcusAI. A public key set there,
//! or a policy.pub beside the file, means the file must carry a matching
//! minisign signature (policy.json.minisig).

use serde::{Deserialize, Serialize};

use crate::cloud::Kind;

/// The provider id a policy's gateway appears under.
pub const GATEWAY_ID: &str = "company-gateway";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Gateway {
    pub name: String,
    pub base_url: String,
    pub kind: Kind,
    /// Sent with every request through it (org or project ids, a tenant).
    pub headers: std::collections::BTreeMap<String, String>,
    /// PEM text of the company CA, trusted for the gateway only.
    pub ca_cert: String,
    pub proxy: String,
    /// The gateway is the only cloud provider allowed.
    pub only: bool,
    /// The gateway takes no key from the user (it signs in another way).
    pub keyless: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Policy {
    /// Shown with each lock, so people know who set it.
    pub organization: String,
    pub contact: String,
    /// Provider ids ("anthropic", "azure", "custom-...") or kinds
    /// ("bedrock"); none: any.
    pub allowed_providers: Option<Vec<String>>,
    /// Model ids, or prefixes ending in "*"; none: any.
    pub allowed_models: Option<Vec<String>>,
    pub force_redact: bool,
    /// US dollars per month on cloud models, across all providers.
    pub monthly_cap_usd: Option<f64>,
    /// Feature ids as on the Features page ("browser_control", "email").
    pub features_off: Vec<String>,
    /// Nothing goes to cloud models at all.
    pub local_only: bool,
    pub gateway: Option<Gateway>,
}

pub fn path() -> std::path::PathBuf {
    if cfg!(windows) {
        std::path::PathBuf::from(std::env::var_os("ProgramData").unwrap_or_else(|| "C:\\ProgramData".into())).join("SulcusAI").join("policy.json")
    } else {
        std::path::PathBuf::from("/etc/sulcusai/policy.json")
    }
}

/// The policy in force. A file that can't be read as a policy, or isn't
/// signed when a key says it must be, locks the cloud entirely: a broken or
/// swapped file must not silently lift its limits.
pub fn get() -> Policy {
    let gpo = Gpo::read();
    let file = gpo.policy_file.clone().filter(|f| !f.trim().is_empty()).map(std::path::PathBuf::from).unwrap_or_else(path);
    let key = gpo.public_key.clone().filter(|k| !k.trim().is_empty()).or_else(|| std::fs::read_to_string(file.with_file_name("policy.pub")).ok());
    let from_file = match std::fs::read(&file) {
        Ok(bytes) => {
            let sig = std::fs::read_to_string(sig_path(&file)).ok();
            match key.as_deref().map(|k| verify(&bytes, sig.as_deref(), k)) {
                Some(Err(_)) => locked(),
                _ => parse(&String::from_utf8_lossy(&bytes)),
            }
        }
        // A key with no file still means IT expected one.
        Err(_) if key.is_some() => locked(),
        Err(_) => Policy::default(),
    };
    gpo.apply(from_file)
}

fn sig_path(file: &std::path::Path) -> std::path::PathBuf {
    let mut name = file.file_name().unwrap_or_default().to_os_string();
    name.push(".minisig");
    file.with_file_name(name)
}

fn locked() -> Policy {
    Policy { local_only: true, organization: "Your organization".into(), ..Default::default() }
}

pub fn parse(text: &str) -> Policy {
    serde_json::from_str(text).unwrap_or_else(|_| locked())
}

/// Checks the file against a minisign public key (the whole .pub file, or
/// just its base64 line).
pub fn verify(bytes: &[u8], sig: Option<&str>, key: &str) -> Result<(), String> {
    let key = key.trim();
    let pk = minisign_verify::PublicKey::decode(key).or_else(|_| minisign_verify::PublicKey::from_base64(key)).map_err(|_| "The policy key isn't readable.")?;
    let sig = minisign_verify::Signature::decode(sig.ok_or("The policy file isn't signed.")?).map_err(|_| "The policy signature isn't readable.")?;
    pk.verify(bytes, &sig, false).map_err(|_| "The policy file doesn't match its signature.".to_string())
}

/// Limits from Group Policy. Each one only ever tightens the file's.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Gpo {
    pub policy_file: Option<String>,
    pub public_key: Option<String>,
    pub organization: Option<String>,
    pub contact: Option<String>,
    pub local_only: bool,
    pub force_redact: bool,
    pub monthly_cap_usd: Option<f64>,
    /// Comma or semicolon separated, as typed into the policy editor.
    pub allowed_providers: Option<String>,
    pub allowed_models: Option<String>,
    pub features_off: Option<String>,
}

fn list(s: &str) -> Vec<String> {
    s.split([',', ';', '\n']).map(str::trim).filter(|x| !x.is_empty()).map(str::to_string).collect()
}

/// Two allow-lists together allow only what both allow.
fn narrow(a: Option<Vec<String>>, b: Option<Vec<String>>) -> Option<Vec<String>> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.into_iter().filter(|x| b.contains(x)).collect()),
        (a, b) => a.or(b),
    }
}

impl Gpo {
    pub fn apply(&self, mut p: Policy) -> Policy {
        if let Some(o) = self.organization.clone().filter(|o| !o.trim().is_empty()) {
            p.organization = o;
        }
        if let Some(c) = self.contact.clone().filter(|c| !c.trim().is_empty()) {
            p.contact = c;
        }
        p.local_only |= self.local_only;
        p.force_redact |= self.force_redact;
        if let Some(cap) = self.monthly_cap_usd {
            p.monthly_cap_usd = Some(p.monthly_cap_usd.map_or(cap, |c| c.min(cap)));
        }
        p.allowed_providers = narrow(p.allowed_providers.take(), self.allowed_providers.as_deref().map(list));
        p.allowed_models = narrow(p.allowed_models.take(), self.allowed_models.as_deref().map(list));
        for f in self.features_off.as_deref().map(list).unwrap_or_default() {
            if !p.features_off.contains(&f) {
                p.features_off.push(f);
            }
        }
        p
    }

    #[cfg(windows)]
    pub fn read() -> Gpo {
        use windows::core::HSTRING;
        use windows::Win32::System::Registry::*;
        const KEY: &str = "SOFTWARE\\Policies\\SulcusAI";
        let text = |name: &str| -> Option<String> {
            let mut buf = vec![0u16; 8192];
            let mut len = (buf.len() * 2) as u32;
            // SAFETY: the buffer and its length are ours.
            unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, &HSTRING::from(KEY), &HSTRING::from(name), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut len)) }.ok().ok()?;
            Some(String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)]))
        };
        let num = |name: &str| -> Option<u32> {
            let mut v = 0u32;
            let mut len = 4u32;
            // SAFETY: a DWORD-sized value we own.
            unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, &HSTRING::from(KEY), &HSTRING::from(name), RRF_RT_REG_DWORD, None, Some((&mut v as *mut u32).cast()), Some(&mut len)) }.ok().ok()?;
            Some(v)
        };
        Gpo {
            policy_file: text("PolicyFile"),
            public_key: text("PolicyPublicKey"),
            organization: text("Organization"),
            contact: text("Contact"),
            local_only: num("LocalOnly") == Some(1),
            force_redact: num("ForceRedact") == Some(1),
            monthly_cap_usd: num("MonthlyCapUsd").map(f64::from),
            allowed_providers: text("AllowedProviders"),
            allowed_models: text("AllowedModels"),
            features_off: text("FeaturesOff"),
        }
    }

    #[cfg(not(windows))]
    pub fn read() -> Gpo {
        Gpo::default()
    }
}

impl Policy {
    pub fn active(&self) -> bool {
        *self != Policy::default()
    }

    fn by(&self) -> String {
        let who = if self.organization.trim().is_empty() { "Your organization" } else { self.organization.trim() };
        if self.contact.trim().is_empty() { format!("{who} set this.") } else { format!("{who} set this; ask {}.", self.contact.trim()) }
    }

    pub fn provider_ok(&self, id: &str, kind: Kind) -> bool {
        if self.local_only {
            return false;
        }
        if self.gateway.as_ref().is_some_and(|g| g.only) {
            return id == GATEWAY_ID;
        }
        if id == GATEWAY_ID && self.gateway.is_some() {
            return true;
        }
        let kind_name = serde_json::to_value(kind).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
        self.allowed_providers.as_ref().is_none_or(|list| list.iter().any(|a| a == id || *a == kind_name))
    }

    pub fn model_ok(&self, model: &str) -> bool {
        self.allowed_models.as_ref().is_none_or(|list| list.iter().any(|a| match a.strip_suffix('*') { Some(pre) => model.starts_with(pre), None => a == model }))
    }

    pub fn feature_off(&self, id: &str) -> bool {
        self.features_off.iter().any(|f| f == id)
    }

    /// Why a cloud model can't be used, if it can't.
    pub fn refuse(&self, provider_id: &str, kind: Kind, model: &str, spent_usd: f64) -> Option<String> {
        if self.local_only {
            return Some(format!("Cloud models are turned off on this PC. {}", self.by()));
        }
        if !self.provider_ok(provider_id, kind) {
            return Some(format!("That cloud provider isn't allowed on this PC. {}", self.by()));
        }
        if !self.model_ok(model) {
            return Some(format!("That cloud model isn't allowed on this PC. {}", self.by()));
        }
        if let Some(cap) = self.monthly_cap_usd {
            if spent_usd >= cap {
                return Some(format!("This month's cloud spending limit (${cap:.2}) is used up. {}", self.by()));
            }
        }
        None
    }

    /// The locks in plain words, for the window.
    pub fn locks(&self) -> Vec<Lock> {
        let why = self.by();
        let mut out = Vec::new();
        let mut add = |what: String| out.push(Lock { what, why: why.clone() });
        if self.local_only {
            add("Everything stays on this PC: no cloud models.".into());
        }
        if let Some(g) = &self.gateway {
            add(format!("Cloud requests {} the company gateway{}.", if g.only { "only go through" } else { "can go through" }, if g.name.is_empty() { String::new() } else { format!(" ({})", g.name) }));
        }
        if let Some(p) = &self.allowed_providers {
            add(format!("Allowed cloud providers: {}.", if p.is_empty() { "none".into() } else { p.join(", ") }));
        }
        if let Some(m) = &self.allowed_models {
            add(format!("Allowed cloud models: {}.", if m.is_empty() { "none".into() } else { m.join(", ") }));
        }
        if self.force_redact {
            add("Privacy mode stays on: personal details are replaced before anything is sent.".into());
        }
        if let Some(c) = self.monthly_cap_usd {
            add(format!("Cloud spending is capped at ${c:.2} a month."));
        }
        if !self.features_off.is_empty() {
            add(format!("Turned off: {}.", self.features_off.join(", ")));
        }
        out
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Lock {
    pub what: String,
    pub why: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PolicyView {
    pub active: bool,
    pub path: String,
    pub locks: Vec<Lock>,
    pub force_redact: bool,
    pub local_only: bool,
}

pub fn view() -> PolicyView {
    let p = get();
    PolicyView { active: p.active(), path: path().display().to_string(), locks: p.locks(), force_redact: p.force_redact, local_only: p.local_only }
}

#[tauri::command(async)]
pub fn policy_view() -> PolicyView {
    view()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locks_follow_the_file() {
        let p = parse(r#"{ "organization": "Acme", "allowed_providers": ["azure", "bedrock"], "allowed_models": ["gpt-4o*", "us.anthropic.claude-sonnet-4-6"], "monthly_cap_usd": 50, "force_redact": true, "features_off": ["browser_control"] }"#);
        assert!(p.active());
        assert!(p.provider_ok("azure", Kind::Azure));
        assert!(p.provider_ok("bedrock-2", Kind::Bedrock), "kinds count too");
        assert!(!p.provider_ok("openai", Kind::Openai));
        assert!(p.model_ok("gpt-4o-mini") && p.model_ok("us.anthropic.claude-sonnet-4-6") && !p.model_ok("o3"));
        assert!(p.refuse("azure", Kind::Azure, "gpt-4o", 10.0).is_none());
        assert!(p.refuse("azure", Kind::Azure, "gpt-4o", 50.0).unwrap().contains("Acme"));
        assert!(p.feature_off("browser_control"));
        assert_eq!(p.locks().len(), 5);
    }

    #[test]
    fn a_gateway_only_policy_allows_just_the_gateway() {
        let p = parse(r#"{ "gateway": { "name": "LLM gateway", "base_url": "https://llm.corp.example/v1", "only": true } }"#);
        assert!(p.provider_ok(GATEWAY_ID, Kind::Openai));
        assert!(!p.provider_ok("anthropic", Kind::Anthropic));
    }

    #[test]
    fn a_broken_file_locks_the_cloud() {
        let p = parse("{ not json");
        assert!(p.local_only);
        assert!(p.refuse("openai", Kind::Openai, "gpt-4o", 0.0).is_some());
        assert_eq!(parse("").local_only, true);
        assert!(!Policy::default().active());
    }

    #[test]
    fn group_policy_only_tightens() {
        let file = parse(r#"{ "allowed_providers": ["azure", "openai"], "monthly_cap_usd": 50, "features_off": ["email"] }"#);
        let g = Gpo { allowed_providers: Some("azure; bedrock".into()), monthly_cap_usd: Some(80.0), features_off: Some("browser_control, email".into()), force_redact: true, organization: Some("Acme".into()), ..Default::default() };
        let p = g.apply(file);
        assert_eq!(p.allowed_providers, Some(vec!["azure".to_string()]));
        assert_eq!(p.monthly_cap_usd, Some(50.0));
        assert_eq!(p.features_off, vec!["email".to_string(), "browser_control".to_string()]);
        assert!(p.force_redact && !p.local_only);
        assert_eq!(p.organization, "Acme");
        assert_eq!(Gpo::default().apply(Policy::default()), Policy::default());
        assert_eq!(Gpo { allowed_models: Some("gpt-4o*".into()), ..Default::default() }.apply(Policy::default()).allowed_models, Some(vec!["gpt-4o*".to_string()]));
    }

    #[test]
    fn a_signed_policy_needs_its_signature() {
        // minisign-verify's own published test pair; it signs only the text "test".
        let key = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
        let sig = "untrusted comment: signature from minisign secret key\nRUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=\ntrusted comment: timestamp:1633700835\tfile:test\tprehashed\nwLMDjy9FLAuxZ3q4NlEvkgtyhrr0gtTu6KC4KBJdITbbOeAi1zBIYo0v4iTgt8jJpIidRJnp94ABQkJAgAooBQ==\n";
        assert!(verify(b"test", Some(sig), key).is_ok());
        assert!(verify(b"tampered", Some(sig), key).is_err());
        assert!(verify(b"test", None, key).unwrap_err().contains("isn't signed"));
        assert!(verify(b"test", Some(sig), "nonsense").is_err());
    }
}
