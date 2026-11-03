// SPDX-License-Identifier: AGPL-3.0-only
//! Windows Hello or the PIN again before sensitive actions.
//!
//! The policy is pure: which actions need a fresh confirmation, and how long
//! one lasts. Asking is left to a `Confirmer`, so this module never touches
//! Windows Hello or the PIN itself and tests can use a fake.

use serde::{Deserialize, Serialize};

/// What a confirmation may be asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SensitiveAction {
    AddDevice,
    RemoveDevice,
    ChangePin,
    TurnOffLock,
    ShowRecoveryCode,
    /// API keys, passwords and tokens shown in full.
    RevealSecret,
    ExportBackup,
    ChangeSecuritySetting,
    OpenProtectedChat,
    DeleteAllData,
}

/// How long a confirmation covers an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// Asked every time: these hand over control of the account or its keys,
    /// so a confirmation left over from a minute ago must not be enough.
    EveryTime,
    /// A confirmation within this many milliseconds covers it.
    Within(i64),
}

pub const SHORT_MS: i64 = 5 * 60_000;
pub const LONG_MS: i64 = 15 * 60_000;

pub fn freshness(action: SensitiveAction) -> Freshness {
    use SensitiveAction::*;
    match action {
        AddDevice | RemoveDevice | ChangePin | TurnOffLock | ShowRecoveryCode | DeleteAllData => Freshness::EveryTime,
        RevealSecret | ExportBackup | ChangeSecuritySetting => Freshness::Within(SHORT_MS),
        // Opening several protected chats in a row shouldn't mean a prompt each time.
        OpenProtectedChat => Freshness::Within(LONG_MS),
    }
}

/// Whether `action` needs Windows Hello or the PIN now, given when this
/// device last confirmed.
pub fn needs_confirm(action: SensitiveAction, last_confirmed: Option<i64>, now: i64) -> bool {
    match (freshness(action), last_confirmed) {
        (Freshness::EveryTime, _) | (_, None) => true,
        // A time ahead of the clock means the clock moved back: ask again.
        (Freshness::Within(ms), Some(at)) => at > now || now - at >= ms,
    }
}

/// What the person is asked to confirm. Holds names, never secrets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub action: SensitiveAction,
    pub device_id: String,
    pub message: String,
}

/// The answer from Windows Hello or the PIN prompt. A PIN is checked by the
/// confirmer (the vault knows it); only right or wrong comes back, so the
/// PIN itself never reaches this module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirmation {
    Hello,
    PinRight,
    PinWrong,
    Declined,
    /// Neither Windows Hello nor a PIN is set up on this device.
    Unavailable,
}

/// Asks the person at a device to confirm. The app's implementation shows the
/// Windows Hello prompt (`crate::hello::verify`) or the PIN box.
pub trait Confirmer {
    fn confirm(&mut self, ask: &Ask) -> Confirmation;
}
