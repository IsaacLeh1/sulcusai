// SPDX-License-Identifier: AGPL-3.0-only
//! Calls waiting on the person's answer, and what they allowed for a chat.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// Allow this kind of action for the rest of the chat.
    AllowForChat,
    Deny,
}

impl Decision {
    pub fn parse(s: &str) -> Option<Decision> {
        match s {
            "allow" => Some(Decision::Allow),
            "always" => Some(Decision::AllowForChat),
            "deny" => Some(Decision::Deny),
            _ => None,
        }
    }
}

/// An approval waiting for the user.
#[derive(Debug, Clone, Serialize)]
pub struct PendingApproval {
    pub chat_id: String,
    pub call_id: String,
    pub tool: String,
    pub risk: Risk,
    pub preview: Preview,
    /// Only this one call can be allowed, not the kind for the whole chat:
    /// deletes and other commands that ask even in Auto, and sends.
    pub once: bool,
    /// What "Always allow…" would save; None where a rule couldn't skip the question.
    pub rule: Option<allowances::Suggestion>,
}

/// Shared between the loop and the approval command.
#[derive(Default)]
pub struct Approvals {
    pub(super) waiting: Mutex<HashMap<String, (PendingApproval, oneshot::Sender<Decision>)>>,
    /// Risks the user approved for a whole chat.
    allowed: Mutex<HashMap<String, HashSet<Risk>>>,
    /// Privacy mode: the level up to which a chat may send without asking again.
    shared: Mutex<HashMap<String, u8>>,
    /// Privacy mode: the level a chat's online model was allowed to receive.
    cloud: Mutex<HashMap<String, u8>>,
}

impl Approvals {
    /// What's waiting for an answer (tests answer these like the user would).
    #[cfg(test)]
    pub fn waiting(&self) -> Vec<PendingApproval> {
        self.waiting.lock().unwrap().values().map(|(p, _)| p.clone()).collect()
    }

    /// Every approval waiting for an answer, in all chats.
    pub fn all_waiting(&self) -> Vec<PendingApproval> {
        self.waiting.lock().unwrap().values().map(|(p, _)| p.clone()).collect()
    }

    pub fn answer(&self, call_id: &str, d: Decision) -> bool {
        match self.waiting.lock().unwrap().remove(call_id) {
            Some((p, tx)) => {
                // A card for one call only (a delete, a send) allows just that call.
                let d = if d == Decision::AllowForChat && p.once { Decision::Allow } else { d };
                if d == Decision::AllowForChat {
                    if p.preview.kind == "privacy" {
                        // "Don't ask again here up to this level."
                        let level = p.preview.detail.as_deref().and_then(|d| serde_json::from_str::<crate::outgoing::Review>(d).ok()).map_or(0, |r| r.level);
                        let mut shared = self.shared.lock().unwrap();
                        let up_to = shared.entry(p.chat_id).or_default();
                        *up_to = (*up_to).max(level);
                    } else {
                        self.allowed.lock().unwrap().entry(p.chat_id).or_default().insert(p.risk);
                    }
                }
                tx.send(d).is_ok()
            }
            None => false,
        }
    }

    pub fn pending(&self, chat_id: &str) -> Vec<PendingApproval> {
        self.waiting.lock().unwrap().values().filter(|(p, _)| p.chat_id == chat_id).map(|(p, _)| p.clone()).collect()
    }

    pub(super) fn allowed(&self, chat_id: &str, risk: Risk) -> bool {
        self.allowed.lock().unwrap().get(chat_id).is_some_and(|s| s.contains(&risk))
    }

    pub(super) fn shared_up_to(&self, chat_id: &str) -> u8 {
        self.shared.lock().unwrap().get(chat_id).copied().unwrap_or(0)
    }

    pub(super) fn cloud_ok(&self, chat_id: &str) -> u8 {
        self.cloud.lock().unwrap().get(chat_id).copied().unwrap_or(0)
    }

    pub(super) fn allow_cloud(&self, chat_id: &str, level: u8) {
        let mut cloud = self.cloud.lock().unwrap();
        let ok = cloud.entry(chat_id.to_string()).or_default();
        *ok = (*ok).max(level);
    }

    pub fn forget_chat(&self, chat_id: &str) {
        self.allowed.lock().unwrap().remove(chat_id);
        self.shared.lock().unwrap().remove(chat_id);
        self.cloud.lock().unwrap().remove(chat_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approving_for_the_chat_remembers_the_risk() {
        let a = Approvals::default();
        let (tx, _rx) = oneshot::channel();
        let p = PendingApproval {
            chat_id: "c".into(),
            call_id: "k".into(),
            tool: "write_file".into(),
            risk: Risk::Write,
            preview: Preview { title: "t".into(), kind: "text", detail: None, note: None },
            once: false,
            rule: None,
        };
        a.waiting.lock().unwrap().insert("k".into(), (p, tx));
        assert_eq!(a.pending("c").len(), 1);
        assert!(a.answer("k", Decision::AllowForChat));
        assert!(a.allowed("c", Risk::Write));
        assert!(!a.allowed("c", Risk::Execute));
        assert!(!a.answer("k", Decision::Allow), "already answered");
        a.forget_chat("c");
        assert!(!a.allowed("c", Risk::Write));
    }
}
