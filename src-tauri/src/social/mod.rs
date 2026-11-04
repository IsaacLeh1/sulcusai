// SPDX-License-Identifier: AGPL-3.0-only
//! The social side: invite-only groups run on their members' PCs, built on
//! People (`sync::people`), which already links and authenticates contacts.
//!
//! - `groups`: who is in a group, roles, invites, the group key and its turns.
//! - `chat`: channels, threads, replies, reactions, pins, edits, slow mode,
//!   all sealed with the group key before they leave the PC.
//! - `safety`: automatic moderation (rules first, a local model second),
//!   reports, block, mute, message requests and teen settings.
//! - `calls`: which member's PC hosts a call and how a game is captured.
//! - `bots`: assistants as members, catch-me-up, translation, lending models,
//!   and documents edited together.
//!
//! Nothing here lists a group anywhere or touches a server: a group exists
//! only on its members' PCs, and the switch for all of it starts off.

pub mod bots;
pub mod calls;
pub mod chat;
pub mod groups;
pub mod safety;
#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

/// The switches for the social side. Safeguards (encryption, reports, teen
/// settings, every limit) have no switch.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Switches {
    /// Groups at all; off means no group traffic is sent or taken.
    pub groups: bool,
    pub calls: bool,
    /// The local model reviewing what rules alone can't decide.
    pub model_moderation: bool,
    pub assistants_in_groups: bool,
}

/// A member's id: the People contact id, or this PC's own id.
pub type MemberId = String;

pub fn now_ms() -> i64 {
    crate::db::now_ms()
}
