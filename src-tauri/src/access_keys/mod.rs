// SPDX-License-Identifier: AGPL-3.0-only
//! Access keys: one key per app, each allowed exactly the scopes the person
//! chose for it, and nothing at all until they choose.
//!
//! - The secret is shown once, when the key is made. Only its hash is kept,
//!   and a presented key is checked against every stored hash in constant
//!   time, without stopping at the first match.
//! - `authorize` refuses unknown, revoked, expired and out-of-scope keys,
//!   and file scopes outside the key's folders. Every decision goes to the
//!   audit log, scrubbed of secrets.
//! - Revoking takes effect on the next request; the key stays listed, marked
//!   revoked, so the log still names the app.
//! - The single local API key from before becomes a legacy key with every
//!   scope (`migrate_legacy`), so programs already set up keep working until
//!   the person replaces it with narrower keys.
//!
//! Times are milliseconds since 1970, as in `db::now_ms`. The whole store is
//! plain data (serde), saved by the caller.

// Nothing outside the tests calls this until the servers and the settings
// page are wired to it; drop this then.
#![cfg_attr(not(test), allow(dead_code))]

mod audit;
mod folders;
mod scopes;
mod secret;

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

pub use audit::{AuditEntry, AuditLog, Outcome};
pub use scopes::{catalog, Scope, ScopeInfo};

const NAME_MAX: usize = 80;
const LEGACY_NAME: &str = "Programs using the local API key from before access keys";
const UNKNOWN_APP: &str = "unknown key";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyRecord {
    pub id: String,
    pub name: String,
    hash: String,
    pub hint: String,
    pub scopes: Vec<Scope>,
    #[serde(default)]
    pub folders: Vec<String>,
    /// File scopes reach every folder; only the legacy key has this, since
    /// the old key could already reach whatever the assistant could.
    #[serde(default)]
    pub all_folders: bool,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub last_used_at: Option<i64>,
    pub revoked_at: Option<i64>,
    #[serde(default)]
    pub legacy: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Active,
    Expired,
    Revoked,
}

impl KeyRecord {
    pub fn status(&self, now: i64) -> Status {
        if self.revoked_at.is_some() {
            Status::Revoked
        } else if self.expires_at.is_some_and(|e| now >= e) {
            Status::Expired
        } else {
            Status::Active
        }
    }
}

/// A key as a settings page shows it: everything but the hash.
#[derive(Debug, Clone, Serialize)]
pub struct KeyView {
    pub id: String,
    pub name: String,
    pub hint: String,
    pub scopes: Vec<ScopeInfo>,
    pub folders: Vec<String>,
    pub all_folders: bool,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub last_used_at: Option<i64>,
    pub revoked_at: Option<i64>,
    pub status: Status,
    pub legacy: bool,
}

/// What the person chose for a new key.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct NewKey {
    pub name: String,
    pub scopes: Vec<Scope>,
    pub folders: Vec<String>,
    pub expires_at: Option<i64>,
}

/// The one time the secret exists outside the app that receives it.
pub struct Created {
    pub id: String,
    pub secret: Zeroizing<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Denial {
    UnknownKey,
    Revoked,
    Expired,
    NotAllowed,
    /// A file scope asked without saying which file, so the folders can't be checked.
    NeedsPath,
    OutsideFolders,
}

impl Denial {
    /// For the app that asked, in plain words.
    pub fn message(self) -> &'static str {
        match self {
            Denial::UnknownKey => "Missing or wrong access key. Make one in SulcusAI, Settings, Access keys.",
            Denial::Revoked => "This access key was turned off in SulcusAI.",
            Denial::Expired => "This access key has run out. Make a new one in SulcusAI, Settings, Access keys.",
            Denial::NotAllowed => "This access key isn't allowed to do that. Its permissions can be changed in SulcusAI, Settings, Access keys.",
            Denial::NeedsPath => "Say which file or folder this is for.",
            Denial::OutsideFolders => "This access key can only reach the folders chosen for it.",
        }
    }

    /// 401 when the key itself is no good, 403 when it is good but not for this.
    pub fn http_status(self) -> u16 {
        match self {
            Denial::UnknownKey | Denial::Revoked | Denial::Expired => 401,
            Denial::NotAllowed | Denial::NeedsPath | Denial::OutsideFolders => 403,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allowed { key_id: String, app: String },
    Denied(Denial),
}

impl Decision {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Decision::Allowed { .. })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AccessKeys {
    keys: Vec<KeyRecord>,
    pub audit: AuditLog,
}

fn clean_scopes(scopes: &[Scope]) -> Vec<Scope> {
    let mut s = scopes.to_vec();
    s.sort();
    s.dedup();
    s
}

fn clean_name(name: &str) -> Result<String, String> {
    let n = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if n.is_empty() {
        return Err("Give the key the name of the app that will use it.".into());
    }
    Ok(n.chars().take(NAME_MAX).collect())
}

impl AccessKeys {
    pub fn create(&mut self, new: NewKey, now: i64) -> Result<Created, String> {
        let name = clean_name(&new.name)?;
        if new.expires_at.is_some_and(|e| e <= now) {
            return Err("The end date has already passed.".into());
        }
        let folders = folders::clean(&new.folders)?;
        let secret = secret::generate();
        let id = uuid::Uuid::new_v4().simple().to_string();
        let scopes = clean_scopes(&new.scopes);
        let detail = scope_list(&scopes);
        self.keys.push(KeyRecord {
            id: id.clone(),
            name: name.clone(),
            hash: secret::hash(&secret),
            hint: secret::hint(&secret),
            scopes,
            folders,
            all_folders: false,
            created_at: now,
            expires_at: new.expires_at,
            last_used_at: None,
            revoked_at: None,
            legacy: false,
        });
        self.audit.record(AuditEntry { at: now, key_id: Some(id.clone()), app: name, scope: None, outcome: Outcome::Created, detail });
        Ok(Created { id, secret })
    }

    pub fn revoke(&mut self, id: &str, now: i64) -> Result<(), String> {
        let k = self.keys.iter_mut().find(|k| k.id == id).ok_or("There's no such access key.")?;
        if k.revoked_at.is_some() {
            return Ok(());
        }
        k.revoked_at = Some(now);
        let app = k.name.clone();
        self.audit.record(AuditEntry { at: now, key_id: Some(id.to_string()), app, scope: None, outcome: Outcome::Revoked, detail: String::new() });
        Ok(())
    }

    /// Replaces a key's scopes and folders. A revoked key stays revoked:
    /// bringing one back would surprise whoever revoked it.
    pub fn set_scopes(&mut self, id: &str, scopes: &[Scope], folders: &[String], now: i64) -> Result<(), String> {
        let folders = folders::clean(folders)?;
        let k = self.keys.iter_mut().find(|k| k.id == id).ok_or("There's no such access key.")?;
        if k.revoked_at.is_some() {
            return Err("This key was revoked. Make a new one instead.".into());
        }
        k.scopes = clean_scopes(scopes);
        k.folders = folders;
        let detail = scope_list(&k.scopes);
        let app = k.name.clone();
        self.audit.record(AuditEntry { at: now, key_id: Some(id.to_string()), app, scope: None, outcome: Outcome::Changed, detail });
        Ok(())
    }

    /// Newest first.
    pub fn list(&self, now: i64) -> Vec<KeyView> {
        let mut out: Vec<KeyView> = self
            .keys
            .iter()
            .map(|k| KeyView {
                id: k.id.clone(),
                name: k.name.clone(),
                hint: k.hint.clone(),
                scopes: k.scopes.iter().map(|s| s.info()).collect(),
                folders: k.folders.clone(),
                all_folders: k.all_folders,
                created_at: k.created_at,
                expires_at: k.expires_at,
                last_used_at: k.last_used_at,
                revoked_at: k.revoked_at,
                status: k.status(now),
                legacy: k.legacy,
            })
            .collect();
        out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        out
    }

    pub fn get(&self, id: &str) -> Option<&KeyRecord> {
        self.keys.iter().find(|k| k.id == id)
    }

    /// Whether `presented` may use `scope` now. File scopes are refused
    /// here; they need `authorize_path`.
    pub fn authorize(&mut self, presented: &str, scope: Scope, now: i64) -> Decision {
        self.decide(presented, scope, None, now)
    }

    /// Whether `presented` may use a file scope on `path`, which should be
    /// resolved (links followed) before it gets here.
    pub fn authorize_path(&mut self, presented: &str, scope: Scope, path: &str, now: i64) -> Decision {
        self.decide(presented, scope, Some(path), now)
    }

    /// Every stored hash is compared, even after a match, so the time taken
    /// doesn't depend on which key matched or how many there are before it.
    fn find(&self, presented: &str) -> Option<usize> {
        let h = secret::hash(presented);
        let mut hit = None;
        for (i, k) in self.keys.iter().enumerate() {
            if secret::ct_eq(k.hash.as_bytes(), h.as_bytes()) && hit.is_none() {
                hit = Some(i);
            }
        }
        hit
    }

    fn decide(&mut self, presented: &str, scope: Scope, path: Option<&str>, now: i64) -> Decision {
        let detail = path.unwrap_or_default().to_string();
        let Some(i) = self.find(presented) else {
            let reason = Denial::UnknownKey;
            self.audit.record(AuditEntry { at: now, key_id: None, app: UNKNOWN_APP.into(), scope: Some(scope), outcome: Outcome::Denied { reason }, detail });
            return Decision::Denied(reason);
        };
        let k = &self.keys[i];
        let refused = match k.status(now) {
            Status::Revoked => Some(Denial::Revoked),
            Status::Expired => Some(Denial::Expired),
            Status::Active if !k.scopes.contains(&scope) => Some(Denial::NotAllowed),
            Status::Active if scope.is_files() => match path {
                None => Some(Denial::NeedsPath),
                Some(p) if k.all_folders => (!folders::valid(p)).then_some(Denial::OutsideFolders),
                Some(p) if k.folders.iter().any(|f| folders::within(p, f)) => None,
                Some(_) => Some(Denial::OutsideFolders),
            },
            Status::Active => None,
        };
        let (key_id, app) = (k.id.clone(), k.name.clone());
        let outcome = match refused {
            Some(reason) => Outcome::Denied { reason },
            None => Outcome::Allowed,
        };
        self.audit.record(AuditEntry { at: now, key_id: Some(key_id.clone()), app: app.clone(), scope: Some(scope), outcome, detail });
        match refused {
            Some(reason) => Decision::Denied(reason),
            None => {
                self.keys[i].last_used_at = Some(now);
                Decision::Allowed { key_id, app }
            }
        }
    }

    /// Turns the single local API key from before into a key with every
    /// scope, so programs set up with it keep working. Run once when the app
    /// first starts with access keys, and again whenever that old key is
    /// replaced: the new one takes over and the old one is revoked, as
    /// making a new local API key always did. Returns the legacy key's id,
    /// or None when there was no old key.
    pub fn migrate_legacy(&mut self, old_key: &str, now: i64) -> Option<String> {
        let old_key = old_key.trim();
        if old_key.is_empty() {
            return None;
        }
        let hash = secret::hash(old_key);
        if let Some(k) = self.keys.iter().find(|k| k.legacy && k.revoked_at.is_none() && secret::ct_eq(k.hash.as_bytes(), hash.as_bytes())) {
            return Some(k.id.clone());
        }
        // Turned off in Settings: it stays off until a new local API key is made.
        if self.keys.iter().any(|k| k.legacy && secret::ct_eq(k.hash.as_bytes(), hash.as_bytes())) {
            return None;
        }
        let stale: Vec<String> = self.keys.iter().filter(|k| k.legacy && k.revoked_at.is_none()).map(|k| k.id.clone()).collect();
        for id in stale {
            let _ = self.revoke(&id, now);
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        let scopes = Scope::ALL.to_vec();
        let detail = scope_list(&scopes);
        self.keys.push(KeyRecord {
            id: id.clone(),
            name: LEGACY_NAME.into(),
            hash,
            hint: secret::hint(old_key),
            scopes,
            folders: Vec::new(),
            all_folders: true,
            created_at: now,
            expires_at: None,
            last_used_at: None,
            revoked_at: None,
            legacy: true,
        });
        self.audit.record(AuditEntry { at: now, key_id: Some(id.clone()), app: LEGACY_NAME.into(), scope: None, outcome: Outcome::Created, detail });
        Some(id)
    }
}

fn scope_list(scopes: &[Scope]) -> String {
    if scopes.is_empty() {
        return "no permissions".into();
    }
    scopes.iter().map(|s| s.id()).collect::<Vec<_>>().join(", ")
}
