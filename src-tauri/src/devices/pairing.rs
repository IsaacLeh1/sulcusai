// SPDX-License-Identifier: AGPL-3.0-only
//! Adding a device, approved on one already on the list:
//!
//! 1. `offer` on an existing device: a single-use code that expires in two
//!    minutes, shown as a QR code (`ApprovalPayload`) and as text to type.
//! 2. `redeem` from the new device: the code is spent at once, right or not
//!    yet approved, and the new device waits for approval.
//! 3. `approve` back on the device that showed the code, after Windows Hello
//!    or the PIN: the new device gets its own key.
//!
//! Only a hash of each code is kept, so the saved list can't be read for
//! codes still open.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::{clean_name, new_id, Confirmer, Device, DeviceBook, DeviceError, DeviceKey, DeviceKind, EventKind, SensitiveAction, MAX_DEVICES};

/// Long enough to scan or type, short enough that a photographed code is
/// stale soon after.
pub const CODE_TTL_MS: i64 = 2 * 60_000;
pub const PENDING_TTL_MS: i64 = 5 * 60_000;
/// Spent and expired codes are remembered this long past their expiry, so
/// a replay gets "already used" or "expired" rather than "wrong".
const REMEMBER_MS: i64 = 30 * 60_000;
/// Crockford's base32: no I, L, O or U, so a typed code can't be misread.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// 40 bits; with the expiry, single use and the lockout on wrong codes,
/// guessing one in time is out of reach.
const CODE_LEN: usize = 8;
pub const PAYLOAD_PREFIX: &str = "sulcusai-add:1:";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Offer {
    hash: String,
    pub(super) by: String,
    expires: i64,
    used: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Pending {
    id: String,
    name: String,
    kind: DeviceKind,
    pub(super) by: String,
    expires: i64,
}

/// A pairing code. Wiped when dropped and hidden from `Debug`.
pub struct PairingCode(Zeroizing<String>);

impl PairingCode {
    fn generate() -> PairingCode {
        let raw = Zeroizing::new(crate::sync::random_bytes::<CODE_LEN>());
        // 256 is a multiple of 32, so masking keeps every letter equally likely.
        PairingCode(Zeroizing::new(raw.iter().map(|b| ALPHABET[(b & 31) as usize] as char).collect()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Split in two halves for reading aloud or typing: "ABCD-EFGH".
    pub fn for_typing(&self) -> Zeroizing<String> {
        let (a, b) = self.0.split_at(CODE_LEN / 2);
        Zeroizing::new(format!("{a}-{b}"))
    }
}

impl std::fmt::Debug for PairingCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PairingCode(hidden)")
    }
}

/// What the QR code on the approving device holds.
pub struct ApprovalPayload(Zeroizing<String>);

impl ApprovalPayload {
    pub fn text(&self) -> &str {
        &self.0
    }

    pub fn qr_svg(&self) -> Result<String, String> {
        let qr = qrcode::QrCode::with_error_correction_level(self.0.as_bytes(), qrcode::EcLevel::M).map_err(|_| "Couldn't make the QR code.".to_string())?;
        Ok(qr.render::<qrcode::render::svg::Color>().min_dimensions(240, 240).quiet_zone(true).build())
    }
}

impl std::fmt::Debug for ApprovalPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ApprovalPayload(hidden)")
    }
}

#[derive(Debug)]
pub struct NewOffer {
    pub code: PairingCode,
    pub payload: ApprovalPayload,
    pub expires: i64,
}

/// The new device after `redeem`: waiting for approval under this id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Joined {
    pub pending_id: String,
    pub expires: i64,
}

/// A device waiting for approval, as the approving device shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingView {
    pub id: String,
    pub name: String,
    pub kind: DeviceKind,
    pub expires: i64,
}

/// An approved device. `key` goes to the new device once, over the pairing
/// connection, and is never shown.
#[derive(Debug)]
pub struct Added {
    pub id: String,
    pub key: DeviceKey,
}

/// Upper case, separators and a scanned payload's prefix dropped, and the
/// letters people confuse with digits mapped the way Crockford's base32 does.
fn normalize(scanned: &str) -> Zeroizing<String> {
    let s = scanned.trim();
    let s = s.strip_prefix(PAYLOAD_PREFIX).unwrap_or(s);
    Zeroizing::new(
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| match c.to_ascii_uppercase() {
                'O' => '0',
                'I' | 'L' => '1',
                c => c,
            })
            .collect(),
    )
}

fn hash(code: &str) -> String {
    hex::encode(Sha256::digest(code.as_bytes()))
}

impl DeviceBook {
    fn prune(&mut self, now: i64) {
        self.offers.retain(|o| now < o.expires + REMEMBER_MS);
        self.pending.retain(|p| now < p.expires);
    }

    /// A new single-use pairing code from device `by`, which must be on the list.
    pub fn offer(&mut self, by: &str, now: i64) -> Result<NewOffer, DeviceError> {
        self.authorize(by, now)?;
        self.prune(now);
        let code = PairingCode::generate();
        let expires = now + CODE_TTL_MS;
        self.offers.push(Offer { hash: hash(code.as_str()), by: by.to_string(), expires, used: false });
        let payload = ApprovalPayload(Zeroizing::new(format!("{PAYLOAD_PREFIX}{}", code.as_str())));
        Ok(NewOffer { code, payload, expires })
    }

    /// The new device presents a scanned or typed code. A code works once:
    /// replaying it, even before approval, is refused. Wrong codes count
    /// toward a lockout shared by all codes.
    pub fn redeem(&mut self, scanned: &str, name: &str, kind: DeviceKind, now: i64) -> Result<Joined, DeviceError> {
        self.pairing_lockout.ready(now)?;
        self.prune(now);
        let h = hash(&normalize(scanned));
        let Some(i) = self.offers.iter().position(|o| o.hash == h) else {
            self.pairing_lockout.failed(now);
            return Err(DeviceError::CodeWrong);
        };
        if self.offers[i].used {
            self.pairing_lockout.failed(now);
            return Err(DeviceError::CodeUsed);
        }
        if now >= self.offers[i].expires {
            return Err(DeviceError::CodeExpired);
        }
        self.offers[i].used = true;
        let by = self.offers[i].by.clone();
        if self.revoked.contains(&by) || self.device(&by).is_none() {
            return Err(DeviceError::Revoked);
        }
        if self.devices.len() + self.pending.len() >= MAX_DEVICES {
            return Err(DeviceError::TooManyDevices);
        }
        self.pairing_lockout.succeeded();
        let id = new_id();
        let expires = now + PENDING_TTL_MS;
        self.pending.push(Pending { id: id.clone(), name: clean_name(name, kind), kind, by, expires });
        Ok(Joined { pending_id: id, expires })
    }

    /// Devices waiting for `by` to approve them.
    pub fn pending_for(&self, by: &str, now: i64) -> Vec<PendingView> {
        self.pending
            .iter()
            .filter(|p| p.by == by && now < p.expires)
            .map(|p| PendingView { id: p.id.clone(), name: p.name.clone(), kind: p.kind, expires: p.expires })
            .collect()
    }

    /// Approves a waiting device on the device that showed its code, after
    /// Windows Hello or the PIN there. A wrong PIN leaves the request open
    /// until it expires.
    pub fn approve(&mut self, pending_id: &str, by: &str, confirmer: &mut dyn Confirmer, now: i64) -> Result<Added, DeviceError> {
        let i = self.pending.iter().position(|p| p.id == pending_id).ok_or(DeviceError::UnknownPending)?;
        if now >= self.pending[i].expires {
            self.pending.remove(i);
            return Err(DeviceError::PendingExpired);
        }
        if self.pending[i].by != by {
            return Err(DeviceError::NotApprover);
        }
        let message = format!("Add \"{}\" to SulcusAI?", self.pending[i].name);
        self.confirm(by, SensitiveAction::AddDevice, &message, confirmer, now)?;
        // `confirm` takes the whole book, so the index isn't trusted across it.
        let i = self.pending.iter().position(|p| p.id == pending_id).ok_or(DeviceError::UnknownPending)?;
        let p = self.pending.remove(i);
        let key = DeviceKey::generate();
        self.devices.push(Device { id: p.id.clone(), name: p.name.clone(), kind: p.kind, added: now, last_seen: None, approved_by: Some(by.to_string()), node_id: None, key: key.clone() });
        self.push_event(EventKind::NewDevice, &p.id, &p.name, now);
        Ok(Added { id: p.id, key })
    }

    /// Turns a waiting device away.
    pub fn decline(&mut self, pending_id: &str, by: &str) -> Result<(), DeviceError> {
        let i = self.pending.iter().position(|p| p.id == pending_id).ok_or(DeviceError::UnknownPending)?;
        if self.pending[i].by != by {
            return Err(DeviceError::NotApprover);
        }
        self.pending.remove(i);
        Ok(())
    }
}
