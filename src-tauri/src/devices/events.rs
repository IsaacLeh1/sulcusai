// SPDX-License-Identifier: AGPL-3.0-only
//! "Was this you?" events: a new device, a sign-in, or a run of wrong PINs.
//! Answering "No" removes the device at once.

use serde::{Deserialize, Serialize};

use super::{DeviceBook, DeviceError};

/// Older events drop off; the list is for recent surprises, not an audit log.
pub const MAX_EVENTS: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    NewDevice,
    SignIn,
    WrongPins,
    Removed,
    /// Someone answered "No, this wasn't me".
    NotMe,
}

impl EventKind {
    fn asks(self) -> bool {
        matches!(self, EventKind::NewDevice | EventKind::SignIn | EventKind::WrongPins)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceEvent {
    pub id: u64,
    pub at: i64,
    pub kind: EventKind,
    pub device_id: String,
    pub device_name: String,
    /// `Some(true)` "Yes, it was me", `Some(false)` "No".
    pub answer: Option<bool>,
}

impl DeviceEvent {
    pub fn asks(&self) -> bool {
        self.kind.asks() && self.answer.is_none()
    }

    pub fn question(&self) -> String {
        match self.kind {
            EventKind::NewDevice => format!("\"{}\" was added to SulcusAI. Was this you?", self.device_name),
            EventKind::SignIn => format!("\"{}\" signed in to SulcusAI. Was this you?", self.device_name),
            EventKind::WrongPins => format!("Several wrong PINs were typed on \"{}\". Was this you?", self.device_name),
            EventKind::Removed => format!("\"{}\" was removed and can no longer reach SulcusAI.", self.device_name),
            EventKind::NotMe => format!("\"{}\" was removed after \"This wasn't me\".", self.device_name),
        }
    }
}

impl DeviceBook {
    pub(super) fn push_event(&mut self, kind: EventKind, device_id: &str, device_name: &str, now: i64) -> u64 {
        self.next_event += 1;
        let id = self.next_event;
        self.events.push(DeviceEvent { id, at: now, kind, device_id: device_id.to_string(), device_name: device_name.to_string(), answer: None });
        if self.events.len() > MAX_EVENTS {
            let extra = self.events.len() - MAX_EVENTS;
            self.events.drain(..extra);
        }
        id
    }

    pub fn events(&self) -> &[DeviceEvent] {
        &self.events
    }

    /// Events still waiting for "Was this you?", newest first.
    pub fn questions(&self) -> Vec<&DeviceEvent> {
        self.events.iter().rev().filter(|e| e.asks()).collect()
    }

    /// Records the answer. "No" removes the device straight away (this PC
    /// itself can't be removed; the answer is still kept). Returns the id of
    /// the device that was removed, if any.
    pub fn answer(&mut self, event_id: u64, was_me: bool, now: i64) -> Result<Option<String>, DeviceError> {
        let e = self.events.iter_mut().find(|e| e.id == event_id).ok_or(DeviceError::UnknownEvent)?;
        if !e.kind.asks() {
            return Err(DeviceError::UnknownEvent);
        }
        e.answer = Some(was_me);
        if was_me {
            return Ok(None);
        }
        let id = e.device_id.clone();
        if self.host_id == id || self.revoked.contains(&id) {
            return Ok(None);
        }
        let Some(name) = self.devices.iter().find(|d| d.id == id).map(|d| d.name.clone()) else { return Ok(None) };
        self.revoke(&id);
        self.push_event(EventKind::NotMe, &id, &name, now);
        Ok(Some(id))
    }
}
