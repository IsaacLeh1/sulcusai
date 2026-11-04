// SPDX-License-Identifier: AGPL-3.0-only
//! SulcusAI's network layer, shared by the app and the Sulcus Relay program.

pub mod lockout;
pub mod security_notice;
pub mod behaviour;
pub mod identity;
pub mod node;
pub mod signed;
pub mod invite;
pub mod records;
pub mod addrbook;
pub mod friend;
pub mod ladder;
pub mod join;
pub mod join_wire;
pub mod admin_wire;
pub mod mailbox;
pub mod seal;

/// For addresses and ids at the edges; everything else goes through `Node`.
pub use libp2p;
pub use node::{Node, NodeConfig, NodeError, NodeEvent, NodeHandle};

#[cfg(test)]
mod tests;
#[cfg(test)]
mod m2_tests;
