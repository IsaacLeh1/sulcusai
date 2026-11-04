// SPDX-License-Identifier: AGPL-3.0-only
//! SulcusAI's network layer, shared by the app and the Sulcus Relay program.

pub mod lockout;
pub mod security_notice;
pub mod behaviour;
pub mod identity;
pub mod node;

/// For addresses and ids at the edges; everything else goes through `Node`.
pub use libp2p;
pub use node::{Node, NodeConfig, NodeError, NodeEvent, NodeHandle};

#[cfg(test)]
mod tests;
