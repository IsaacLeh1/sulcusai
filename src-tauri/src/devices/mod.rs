// SPDX-License-Identifier: AGPL-3.0-only
//! Authorized devices: the phones, tablets and PCs allowed to reach this
//! SulcusAI, each with a key of its own.
//!
//! Everything here is policy and storage, with the time passed in, so it can
//! be tested without a clock, a window or Windows Hello. The list is one
//! `DeviceBook`, saved sealed with the data key (`seal`, `open`); its device
//! keys are separate random keys, so a phone that leaks its key gives away
//! that phone's access and nothing else.
//!
//! Secrets (device keys, pairing codes) are never part of an error or a
//! `Debug` print; errors carry names and waits only.

// The phone page and Settings, Security call into this after merge; until
// then only the tests do.
#![allow(dead_code, unused_imports)]

mod confirm;
mod events;
mod lockout;
mod pairing;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use hmac::{Hmac, Mac};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::crypto::Cipher;

pub use confirm::{freshness, needs_confirm, Ask, Confirmation, Confirmer, Freshness, SensitiveAction};
pub use events::{DeviceEvent, EventKind};
pub use lockout::Lockout;
pub use pairing::{Added, ApprovalPayload, Joined, NewOffer, PairingCode, PendingView};

/// Plenty for one person; keeps a flood of pairing requests from growing the
/// saved list without bound.
pub const MAX_DEVICES: usize = 20;
/// `last_seen` is saved at most this often, since every request touches it.
const SEEN_EVERY_MS: i64 = 60_000;
const NAME_CHARS: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeviceKind {
    Pc,
    Phone,
    Tablet,
    Browser,
    Other,
}

impl DeviceKind {
    fn default_name(self) -> &'static str {
        match self {
            DeviceKind::Pc => "PC",
            DeviceKind::Phone => "Phone",
            DeviceKind::Tablet => "Tablet",
            DeviceKind::Browser => "Browser",
            DeviceKind::Other => "Device",
        }
    }
}

/// A device's own 256-bit key. Wiped from memory when dropped, and hidden
/// from `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct DeviceKey(Zeroizing<[u8; 32]>);

impl DeviceKey {
    /// Always fresh random bytes: never derived from, or equal to, the data key.
    pub fn generate() -> DeviceKey {
        DeviceKey(Zeroizing::new(crate::sync::random_bytes::<32>()))
    }

    pub fn bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// HMAC-SHA256 of `message`: what a device sends to prove it holds the key.
    pub fn sign(&self, message: &[u8]) -> [u8; 32] {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&self.0[..]).expect("HMAC takes any key length");
        mac.update(message);
        mac.finalize().into_bytes().into()
    }

    fn verify(&self, message: &[u8], tag: &[u8]) -> bool {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&self.0[..]).expect("HMAC takes any key length");
        mac.update(message);
        mac.verify_slice(tag).is_ok()
    }
}

impl fmt::Debug for DeviceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DeviceKey(hidden)")
    }
}

impl Serialize for DeviceKey {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&Zeroizing::new(hex::encode(&self.0[..])))
    }
}

impl<'de> Deserialize<'de> for DeviceKey {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<DeviceKey, D::Error> {
        let text = Zeroizing::new(String::deserialize(d)?);
        let mut key = Zeroizing::new([0u8; 32]);
        // A fixed message: serde's own would quote the bad value.
        hex::decode_to_slice(text.as_bytes(), &mut key[..]).map_err(|_| serde::de::Error::custom("bad device key"))?;
        Ok(DeviceKey(key))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub kind: DeviceKind,
    pub added: i64,
    pub last_seen: Option<i64>,
    /// The device that approved this one; `None` for this PC.
    pub approved_by: Option<String>,
    /// The device's peer-to-peer node id (PCs only; phones have no node).
    /// Defaults so lists sealed before the network existed still open.
    #[serde(default)]
    pub node_id: Option<String>,
    key: DeviceKey,
}

impl Device {
    pub fn key(&self) -> &DeviceKey {
        &self.key
    }
}

/// A device as the Security page lists it: no key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRow {
    pub id: String,
    pub name: String,
    pub kind: DeviceKind,
    pub added: i64,
    pub last_seen: Option<i64>,
    pub this_pc: bool,
    pub node_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceError {
    UnknownDevice,
    Revoked,
    BadSignature,
    CodeWrong,
    CodeUsed,
    CodeExpired,
    UnknownPending,
    PendingExpired,
    NotApprover,
    TooManyDevices,
    CantRemoveThisPc,
    Wait { secs: u64 },
    WrongPin,
    NotConfirmed,
    ConfirmUnavailable,
    UnknownEvent,
    Storage,
}

impl fmt::Display for DeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use DeviceError::*;
        let text = match self {
            UnknownDevice => "This device isn't on SulcusAI's device list.",
            Revoked => "This device was removed from SulcusAI. Add it again from a device that's still on the list.",
            BadSignature => "This request couldn't be checked. Reload the page and try again.",
            CodeWrong => "That pairing code isn't right.",
            CodeUsed => "That pairing code was already used. Start again on a device that's on the list.",
            CodeExpired => "That pairing code has expired. Start again on a device that's on the list.",
            UnknownPending => "There's no request to add that device.",
            PendingExpired => "The request to add that device has expired. Start again.",
            NotApprover => "Approve this device on the device that showed the pairing code.",
            TooManyDevices => "Too many devices are on the list. Remove one first.",
            CantRemoveThisPc => "This PC can't be removed from its own list.",
            Wait { secs } => return write!(f, "Too many wrong tries. Try again in {secs} s."),
            WrongPin => "That PIN isn't right.",
            NotConfirmed => "Not confirmed, so nothing was changed.",
            ConfirmUnavailable => "Set up Windows Hello or a PIN first (Settings, Security).",
            UnknownEvent => "That alert is no longer there.",
            Storage => "The device list couldn't be read.",
        };
        f.write_str(text)
    }
}

impl std::error::Error for DeviceError {}

impl From<lockout::Wait> for DeviceError {
    fn from(w: lockout::Wait) -> Self {
        DeviceError::Wait { secs: w.secs }
    }
}

impl From<DeviceError> for String {
    fn from(e: DeviceError) -> String {
        e.to_string()
    }
}

/// The whole device list and the state that has to survive a restart:
/// revocations, open pairing codes (as hashes), lockouts, confirmations and
/// "Was this you?" events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceBook {
    host_id: String,
    devices: Vec<Device>,
    /// Removed device ids. Ids are random and never reused, so the list only
    /// grows by one per removal.
    revoked: BTreeSet<String>,
    /// Node ids of removed devices, refused on every connection.
    #[serde(default)]
    revoked_nodes: BTreeSet<String>,
    offers: Vec<pairing::Offer>,
    pending: Vec<pairing::Pending>,
    /// Wrong PINs per device, so a device someone else holds can't lock this
    /// PC out by guessing.
    pin_lockouts: BTreeMap<String, Lockout>,
    pairing_lockout: Lockout,
    /// When each device last confirmed with Windows Hello or the PIN.
    confirmed: BTreeMap<String, i64>,
    events: Vec<DeviceEvent>,
    next_event: u64,
}

fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..16].to_string()
}

fn clean_name(name: &str, kind: DeviceKind) -> String {
    let name: String = name.chars().filter(|c| !c.is_control()).collect::<String>().trim().chars().take(NAME_CHARS).collect();
    if name.is_empty() {
        kind.default_name().to_string()
    } else {
        name
    }
}

impl DeviceBook {
    /// A new list holding only this PC, which approves the first other device.
    pub fn new(host_name: &str, now: i64) -> DeviceBook {
        let host = Device {
            id: new_id(),
            name: clean_name(host_name, DeviceKind::Pc),
            kind: DeviceKind::Pc,
            added: now,
            last_seen: Some(now),
            approved_by: None,
            node_id: None,
            key: DeviceKey::generate(),
        };
        DeviceBook {
            host_id: host.id.clone(),
            devices: vec![host],
            revoked: BTreeSet::new(),
            revoked_nodes: BTreeSet::new(),
            offers: Vec::new(),
            pending: Vec::new(),
            pin_lockouts: BTreeMap::new(),
            pairing_lockout: Lockout::default(),
            confirmed: BTreeMap::new(),
            events: Vec::new(),
            next_event: 0,
        }
    }

    pub fn host_id(&self) -> &str {
        &self.host_id
    }

    pub fn device(&self, id: &str) -> Option<&Device> {
        self.devices.iter().find(|d| d.id == id)
    }

    pub fn rows(&self) -> Vec<DeviceRow> {
        self.devices
            .iter()
            .map(|d| DeviceRow { id: d.id.clone(), name: d.name.clone(), kind: d.kind, added: d.added, last_seen: d.last_seen, this_pc: d.id == self.host_id, node_id: d.node_id.clone() })
            .collect()
    }

    pub fn is_revoked(&self, id: &str) -> bool {
        self.revoked.contains(id)
    }

    /// The check on every request: refused at once once a device is removed.
    /// Notes when it was last seen; returns true when that's worth saving.
    pub fn authorize(&mut self, id: &str, now: i64) -> Result<bool, DeviceError> {
        if self.revoked.contains(id) {
            return Err(DeviceError::Revoked);
        }
        let d = self.devices.iter_mut().find(|d| d.id == id).ok_or(DeviceError::UnknownDevice)?;
        let save = d.last_seen.is_none_or(|t| now - t >= SEEN_EVERY_MS || t > now);
        if save {
            d.last_seen = Some(now);
        }
        Ok(save)
    }

    /// `authorize`, plus proof that the request came from the holder of the
    /// device's key (an HMAC of `message`, which should carry a time and a
    /// nonce so it can't be replayed).
    pub fn check_request(&mut self, id: &str, message: &[u8], tag: &[u8], now: i64) -> Result<bool, DeviceError> {
        if self.revoked.contains(id) {
            return Err(DeviceError::Revoked);
        }
        let d = self.device(id).ok_or(DeviceError::UnknownDevice)?;
        if !d.key.verify(message, tag) {
            return Err(DeviceError::BadSignature);
        }
        self.authorize(id, now)
    }

    /// A device started a new session: a "Was this you?" event.
    pub fn sign_in(&mut self, id: &str, now: i64) -> Result<u64, DeviceError> {
        self.authorize(id, now)?;
        let name = self.device(id).map(|d| d.name.clone()).unwrap_or_default();
        Ok(self.push_event(EventKind::SignIn, id, &name, now))
    }

    pub fn rename(&mut self, id: &str, name: &str) -> Result<(), DeviceError> {
        let d = self.devices.iter_mut().find(|d| d.id == id).ok_or(DeviceError::UnknownDevice)?;
        d.name = clean_name(name, d.kind);
        Ok(())
    }

    /// Removes a device after the person at `by` confirms. Effective at once:
    /// the next `authorize` for it fails.
    pub fn remove(&mut self, id: &str, by: &str, confirmer: &mut dyn Confirmer, now: i64) -> Result<(), DeviceError> {
        if id == self.host_id {
            return Err(DeviceError::CantRemoveThisPc);
        }
        let name = self.device(id).ok_or(DeviceError::UnknownDevice)?.name.clone();
        let message = format!("Remove \"{name}\" from SulcusAI?");
        self.confirm(by, SensitiveAction::RemoveDevice, &message, confirmer, now)?;
        self.revoke(id);
        self.push_event(EventKind::Removed, id, &name, now);
        Ok(())
    }

    /// Drops the device and everything tied to it, and puts it on the
    /// revocation list. Codes it handed out and devices waiting for its
    /// approval go too.
    fn revoke(&mut self, id: &str) {
        if let Some(n) = self.device(id).and_then(|d| d.node_id.clone()) {
            self.revoked_nodes.insert(n);
        }
        self.devices.retain(|d| d.id != id);
        self.revoked.insert(id.to_string());
        self.offers.retain(|o| o.by != id);
        self.pending.retain(|p| p.by != id);
        self.pin_lockouts.remove(id);
        self.confirmed.remove(id);
    }

    /// Asks the person at `device_id` to confirm `action` when the policy
    /// says so. Wrong PINs count toward that device's lockout.
    pub fn confirm(&mut self, device_id: &str, action: SensitiveAction, message: &str, confirmer: &mut dyn Confirmer, now: i64) -> Result<(), DeviceError> {
        self.authorize(device_id, now)?;
        if !needs_confirm(action, self.confirmed.get(device_id).copied(), now) {
            return Ok(());
        }
        self.pin_lockouts.get(device_id).map_or(Ok(()), |l| l.ready(now))?;
        let ask = Ask { action, device_id: device_id.to_string(), message: message.to_string() };
        match confirmer.confirm(&ask) {
            Confirmation::Hello | Confirmation::PinRight => {
                self.pin_lockouts.remove(device_id);
                self.confirmed.insert(device_id.to_string(), now);
                Ok(())
            }
            Confirmation::PinWrong => {
                let started = self.pin_lockouts.entry(device_id.to_string()).or_default().failed(now);
                if started {
                    let name = self.device(device_id).map(|d| d.name.clone()).unwrap_or_default();
                    self.push_event(EventKind::WrongPins, device_id, &name, now);
                }
                Err(DeviceError::WrongPin)
            }
            Confirmation::Declined => Err(DeviceError::NotConfirmed),
            Confirmation::Unavailable => Err(DeviceError::ConfirmUnavailable),
        }
    }

    /// Seconds a device must wait before its next PIN try.
    pub fn pin_wait_secs(&self, device_id: &str, now: i64) -> u64 {
        self.pin_lockouts.get(device_id).map_or(0, |l| ((l.wait_ms(now) + 999) / 1000) as u64)
    }

    /// Records a device's node id. A node id a removed device used can't
    /// come back under another device.
    pub fn set_node_id(&mut self, device_id: &str, node_id: &str) -> Result<(), DeviceError> {
        if self.revoked_nodes.contains(node_id) {
            return Err(DeviceError::Revoked);
        }
        let d = self.devices.iter_mut().find(|d| d.id == device_id).ok_or(DeviceError::UnknownDevice)?;
        d.node_id = Some(node_id.to_string());
        Ok(())
    }

    /// For the node's connection gate.
    pub fn revoked_node_ids(&self) -> impl Iterator<Item = &str> {
        self.revoked_nodes.iter().map(String::as_str)
    }

    /// The list as text to store, sealed with the data key.
    pub fn seal(&self, c: &Cipher) -> Result<String, DeviceError> {
        let plain = Zeroizing::new(serde_json::to_string(self).map_err(|_| DeviceError::Storage)?);
        Ok(c.encrypt(&plain))
    }

    /// Plain text is refused: `Cipher::decrypt` lets it through for old
    /// columns, but a device list was never stored unsealed, so plain text
    /// here can only be tampering.
    pub fn open(stored: &str, c: &Cipher) -> Result<DeviceBook, DeviceError> {
        if !Cipher::is_encrypted(stored) {
            return Err(DeviceError::Storage);
        }
        let plain = Zeroizing::new(c.decrypt(stored).map_err(|_| DeviceError::Storage)?);
        serde_json::from_str(&plain).map_err(|_| DeviceError::Storage)
    }
}
