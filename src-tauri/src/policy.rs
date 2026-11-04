// SPDX-License-Identifier: AGPL-3.0-only
//! A company policy file, %ProgramData%\SulcusAI\policy.json on Windows
//! (/etc/sulcusai/policy.json elsewhere). IT puts it there with admin
//! rights; the app only reads it, and nothing in the window can change it.
//! It can limit cloud providers and models, force privacy mode, cap spend,
//! turn features off, keep everything on this PC, and set a company
//! gateway. The window shows each lock and why.

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

/// The policy in force. A file that can't be read as a policy locks the
/// cloud entirely: a broken file must not silently lift its limits.
pub fn get() -> Policy {
    match std::fs::read_to_string(path()) {
        Ok(text) => parse(&text),
        Err(_) => Policy::default(),
    }
}

pub fn parse(text: &str) -> Policy {
    serde_json::from_str(text).unwrap_or_else(|_| Policy { local_only: true, organization: "Your organization".into(), ..Default::default() })
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
}
