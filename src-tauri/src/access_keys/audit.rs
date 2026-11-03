// SPDX-License-Identifier: AGPL-3.0-only
//! The record of every access decision and every change to a key, so a
//! person can see which app asked for what and when, and above all what was
//! refused. Text goes through `secret_scrub::scrub` before it's kept: an app
//! name or a path can carry a pasted token.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use super::scopes::Scope;
use super::Denial;
use crate::secret_scrub::scrub;

/// Old entries go first; this keeps months of normal use without the log
/// growing without end when an app retries a refused request in a loop.
pub const MAX_ENTRIES: usize = 5000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    Allowed,
    Denied { reason: Denial },
    Created,
    Changed,
    Revoked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub at: i64,
    /// None when the key presented matched no key at all.
    pub key_id: Option<String>,
    pub app: String,
    pub scope: Option<Scope>,
    pub outcome: Outcome,
    /// The path for file scopes, or what changed on a key.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AuditLog {
    entries: VecDeque<AuditEntry>,
}

impl AuditLog {
    pub fn record(&mut self, mut e: AuditEntry) {
        e.app = scrub(&e.app);
        e.detail = scrub(&e.detail);
        if self.entries.len() >= MAX_ENTRIES {
            self.entries.pop_front();
        }
        self.entries.push_back(e);
    }

    pub fn entries(&self) -> impl DoubleEndedIterator<Item = &AuditEntry> {
        self.entries.iter()
    }

    /// Newest first, for a settings page.
    pub fn recent(&self, n: usize) -> Vec<AuditEntry> {
        self.entries.iter().rev().take(n).cloned().collect()
    }

    pub fn denials(&self) -> Vec<AuditEntry> {
        self.entries.iter().filter(|e| matches!(e.outcome, Outcome::Denied { .. })).cloned().collect()
    }

    pub fn for_key(&self, id: &str) -> Vec<AuditEntry> {
        self.entries.iter().filter(|e| e.key_id.as_deref() == Some(id)).cloned().collect()
    }
}
