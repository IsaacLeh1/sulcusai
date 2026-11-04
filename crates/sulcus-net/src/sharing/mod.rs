// SPDX-License-Identifier: AGPL-3.0-only
//! Sharing and private networks: share links and pieces, model cards passed
//! between people, the virtual LAN's rules and remote control's handshake.
//! Everything here is plain data and checks, so it is tested without sockets.

pub mod share;
pub mod modelcard;
pub mod vlan;
pub mod remote;
pub mod roster;
pub mod wire;
#[cfg(test)]
mod wire_tests;
