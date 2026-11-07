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
pub mod live;
pub mod media;
pub mod profile;
pub mod networks;
pub mod actions;
pub mod native;
pub mod safety;
pub mod store;
pub use store::take_synced_reads;
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
    /// Only the shared game's sound in a call, through process loopback.
    pub game_sound: bool,
    /// Desktop duplication for exclusive full screen games window capture
    /// shows as black.
    pub native_screen_capture: bool,
    /// Previews and players for links in chats fetch from the web.
    pub link_previews: bool,
    pub gif_search: bool,
    /// The user's own GIPHY key.
    pub gif_key: String,
    /// A local model writes the "since you were last here" line.
    pub since_summary: bool,
    /// Extras for everyone: profile colours and frames, names per group,
    /// your own emoji, bigger files and HD screen share.
    pub extras: bool,
}

/// A member's id: the People contact id, or this PC's own id.
pub type MemberId = String;

pub fn now_ms() -> i64 {
    crate::db::now_ms()
}
