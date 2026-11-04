// SPDX-License-Identifier: AGPL-3.0-only
//! Creating media: the planning half of each tool (what to ask the engines
//! for), kept free of engines so it can be tested without a card.
//! Every tool that can show or sound like a real person goes through
//! [`consent::check`] first.

pub mod batch;
pub mod commands;
pub mod consent;
pub mod endframe;
pub mod face_chat;
pub mod lipsync;
pub mod mic;
pub mod regions;
pub mod sfx;
pub mod storyboard;
pub mod tags;
pub mod textcut;
pub mod tiles;
