// SPDX-License-Identifier: AGPL-3.0-only
use std::collections::VecDeque;

use super::lockout::{FIRST_WAIT_MS, FREE_TRIES, MAX_WAIT_MS};
use super::pairing::{CODE_TTL_MS, PAYLOAD_PREFIX, PENDING_TTL_MS};
use super::*;
use crate::crypto::Cipher;

const T0: i64 = 1_800_000_000_000;

/// Stands in for Windows Hello and the PIN box: gives the queued answers in
/// order and remembers what it was asked.
#[derive(Default)]
struct Fake {
    answers: VecDeque<Confirmation>,
    asked: Vec<Ask>,
}

impl Fake {
    fn with(answers: &[Confirmation]) -> Fake {
        Fake { answers: answers.iter().copied().collect(), asked: Vec::new() }
    }
}

impl Confirmer for Fake {
    fn confirm(&mut self, ask: &Ask) -> Confirmation {
        self.asked.push(ask.clone());
        self.answers.pop_front().unwrap_or(Confirmation::Declined)
    }
}

fn book() -> DeviceBook {
    DeviceBook::new("Desk PC", T0)
}

/// Adds a phone the whole way: offer on this PC, redeem, approve with Hello.
fn add_phone(b: &mut DeviceBook, name: &str, now: i64) -> Added {
    let host = b.host_id().to_string();
    let offer = b.offer(&host, now).unwrap();
    let joined = b.redeem(offer.payload.text(), name, DeviceKind::Phone, now + 1_000).unwrap();
    b.approve(&joined.pending_id, &host, &mut Fake::with(&[Confirmation::Hello]), now + 2_000).unwrap()
}

#[test]
fn new_book_holds_only_this_pc() {
    let b = book();
    let rows = b.rows();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].this_pc);
    assert_eq!(rows[0].name, "Desk PC");
    assert_eq!(rows[0].kind, DeviceKind::Pc);
}

#[test]
fn each_device_gets_its_own_fresh_key() {
    let mut b = book();
    let a = add_phone(&mut b, "Phone A", T0);
    let c = add_phone(&mut b, "Phone B", T0 + 10_000);
    let host_key = b.device(b.host_id()).unwrap().key().clone();
    assert_ne!(a.key, c.key);
    assert_ne!(a.key, host_key);
    assert_eq!(b.device(&a.id).unwrap().key(), &a.key);
    assert_eq!(b.device(&a.id).unwrap().approved_by.as_deref(), Some(b.host_id()));
    assert_eq!(b.rows().len(), 3);
}

#[test]
fn approval_payload_carries_the_code_and_makes_a_qr() {
    let mut b = book();
    let host = b.host_id().to_string();
    let offer = b.offer(&host, T0).unwrap();
    assert_eq!(offer.payload.text(), format!("{PAYLOAD_PREFIX}{}", offer.code.as_str()));
    assert_eq!(offer.code.as_str().len(), 8);
    assert_eq!(offer.expires, T0 + CODE_TTL_MS);
    assert!(offer.payload.qr_svg().unwrap().contains("<svg"));
    // A typed code, with its dash and in lower case, works as well.
    let typed = offer.code.for_typing().to_lowercase();
    assert!(b.redeem(&typed, "Tablet", DeviceKind::Tablet, T0 + 5_000).is_ok());
}

#[test]
fn adding_needs_hello_or_pin_on_the_approving_device() {
    let mut b = book();
    let host = b.host_id().to_string();
    let offer = b.offer(&host, T0).unwrap();
    let joined = b.redeem(offer.payload.text(), "Pixel", DeviceKind::Phone, T0 + 1_000).unwrap();
    assert_eq!(b.pending_for(&host, T0 + 1_000).len(), 1);

    let mut fake = Fake::with(&[Confirmation::Declined]);
    assert_eq!(b.approve(&joined.pending_id, &host, &mut fake, T0 + 2_000).unwrap_err(), DeviceError::NotConfirmed);
    assert_eq!(fake.asked[0].action, SensitiveAction::AddDevice);
    assert_eq!(fake.asked[0].device_id, host);
    assert!(fake.asked[0].message.contains("Pixel"));
    assert_eq!(b.rows().len(), 1);

    let mut none = Fake::with(&[Confirmation::Unavailable]);
    assert_eq!(b.approve(&joined.pending_id, &host, &mut none, T0 + 3_000).unwrap_err(), DeviceError::ConfirmUnavailable);

    let mut pin = Fake::with(&[Confirmation::PinRight]);
    let added = b.approve(&joined.pending_id, &host, &mut pin, T0 + 4_000).unwrap();
    assert_eq!(added.id, joined.pending_id);
    assert!(b.pending_for(&host, T0 + 4_000).is_empty());
    assert_eq!(b.rows().len(), 2);
}

#[test]
fn only_the_device_that_showed_the_code_approves() {
    let mut b = book();
    let host = b.host_id().to_string();
    let phone = add_phone(&mut b, "Phone", T0);
    let offer = b.offer(&phone.id, T0 + 10_000).unwrap();
    let joined = b.redeem(offer.code.as_str(), "Laptop", DeviceKind::Pc, T0 + 11_000).unwrap();
    let mut fake = Fake::with(&[Confirmation::Hello]);
    assert_eq!(b.approve(&joined.pending_id, &host, &mut fake, T0 + 12_000).unwrap_err(), DeviceError::NotApprover);
    assert!(fake.asked.is_empty());
    assert!(b.approve(&joined.pending_id, &phone.id, &mut fake, T0 + 12_000).is_ok());
}

#[test]
fn a_replayed_pairing_code_is_refused() {
    let mut b = book();
    let host = b.host_id().to_string();
    let offer = b.offer(&host, T0).unwrap();
    let text = offer.payload.text().to_string();
    b.redeem(&text, "Phone", DeviceKind::Phone, T0 + 1_000).unwrap();
    assert_eq!(b.redeem(&text, "Attacker", DeviceKind::Phone, T0 + 2_000).unwrap_err(), DeviceError::CodeUsed);
    // Still refused after the first device was approved.
    let pending = b.pending_for(&host, T0 + 2_000)[0].id.clone();
    b.approve(&pending, &host, &mut Fake::with(&[Confirmation::Hello]), T0 + 3_000).unwrap();
    assert_eq!(b.redeem(&text, "Attacker", DeviceKind::Phone, T0 + 4_000).unwrap_err(), DeviceError::CodeUsed);
    assert_eq!(b.pending_for(&host, T0 + 4_000).len(), 0);
}

#[test]
fn an_expired_pairing_code_is_refused() {
    let mut b = book();
    let host = b.host_id().to_string();
    let offer = b.offer(&host, T0).unwrap();
    let err = b.redeem(offer.code.as_str(), "Phone", DeviceKind::Phone, T0 + CODE_TTL_MS).unwrap_err();
    assert_eq!(err, DeviceError::CodeExpired);
    assert!(b.pending_for(&host, T0 + CODE_TTL_MS).is_empty());
    // Just inside the window still works for a fresh code.
    let fresh = b.offer(&host, T0 + 10 * 60_000).unwrap();
    assert!(b.redeem(fresh.code.as_str(), "Phone", DeviceKind::Phone, T0 + 10 * 60_000 + CODE_TTL_MS - 1).is_ok());
}

#[test]
fn an_approval_too_late_is_refused() {
    let mut b = book();
    let host = b.host_id().to_string();
    let offer = b.offer(&host, T0).unwrap();
    let joined = b.redeem(offer.code.as_str(), "Phone", DeviceKind::Phone, T0).unwrap();
    let mut fake = Fake::with(&[Confirmation::Hello]);
    assert_eq!(b.approve(&joined.pending_id, &host, &mut fake, T0 + PENDING_TTL_MS).unwrap_err(), DeviceError::PendingExpired);
    assert!(fake.asked.is_empty());
    assert_eq!(b.approve(&joined.pending_id, &host, &mut fake, T0 + PENDING_TTL_MS).unwrap_err(), DeviceError::UnknownPending);
}

#[test]
fn wrong_pairing_codes_lock_out_for_growing_waits() {
    let mut b = book();
    let host = b.host_id().to_string();
    let offer = b.offer(&host, T0).unwrap();
    for _ in 0..FREE_TRIES {
        assert_eq!(b.redeem("ZZZZZZZZ", "x", DeviceKind::Other, T0).unwrap_err(), DeviceError::CodeWrong);
    }
    assert_eq!(b.redeem("ZZZZZZZZ", "x", DeviceKind::Other, T0).unwrap_err(), DeviceError::CodeWrong);
    // Even the right code waits now.
    assert_eq!(b.redeem(offer.code.as_str(), "x", DeviceKind::Other, T0 + 1).unwrap_err(), DeviceError::Wait { secs: 30 });
    assert!(b.redeem(offer.code.as_str(), "Phone", DeviceKind::Phone, T0 + FIRST_WAIT_MS).is_ok());
}

#[test]
fn removal_revokes_at_once() {
    let mut b = book();
    let host = b.host_id().to_string();
    let phone = add_phone(&mut b, "Phone", T0);
    let msg = b"ts=1&nonce=abc&op=list";
    let tag = phone.key.sign(msg);
    assert!(b.check_request(&phone.id, msg, &tag, T0 + 10_000).is_ok());

    let mut fake = Fake::with(&[Confirmation::Hello]);
    b.remove(&phone.id, &host, &mut fake, T0 + 20_000).unwrap();
    assert_eq!(fake.asked[0].action, SensitiveAction::RemoveDevice);
    assert!(b.is_revoked(&phone.id));
    assert_eq!(b.authorize(&phone.id, T0 + 20_001).unwrap_err(), DeviceError::Revoked);
    assert_eq!(b.check_request(&phone.id, msg, &tag, T0 + 20_001).unwrap_err(), DeviceError::Revoked);
    assert_eq!(b.sign_in(&phone.id, T0 + 20_001).unwrap_err(), DeviceError::Revoked);
    assert_eq!(b.offer(&phone.id, T0 + 20_001).unwrap_err(), DeviceError::Revoked);
    assert!(b.events().iter().any(|e| e.kind == EventKind::Removed && e.device_id == phone.id));
}

#[test]
fn removal_needs_a_confirmation_and_spares_this_pc() {
    let mut b = book();
    let host = b.host_id().to_string();
    let phone = add_phone(&mut b, "Phone", T0);
    assert_eq!(b.remove(&phone.id, &host, &mut Fake::with(&[Confirmation::Declined]), T0 + 5_000).unwrap_err(), DeviceError::NotConfirmed);
    assert!(b.authorize(&phone.id, T0 + 5_000).is_ok());
    assert_eq!(b.remove(&host, &phone.id, &mut Fake::with(&[Confirmation::Hello]), T0 + 5_000).unwrap_err(), DeviceError::CantRemoveThisPc);
}

#[test]
fn a_removed_approver_takes_its_open_codes_and_requests_along() {
    let mut b = book();
    let host = b.host_id().to_string();
    let phone = add_phone(&mut b, "Phone", T0);
    let offer = b.offer(&phone.id, T0 + 10_000).unwrap();
    let other = b.offer(&phone.id, T0 + 10_000).unwrap();
    let joined = b.redeem(other.code.as_str(), "Laptop", DeviceKind::Pc, T0 + 11_000).unwrap();
    b.remove(&phone.id, &host, &mut Fake::with(&[Confirmation::Hello]), T0 + 12_000).unwrap();
    assert!(b.redeem(offer.code.as_str(), "Laptop", DeviceKind::Pc, T0 + 13_000).is_err());
    let mut fake = Fake::with(&[Confirmation::Hello]);
    assert_eq!(b.approve(&joined.pending_id, &phone.id, &mut fake, T0 + 13_000).unwrap_err(), DeviceError::UnknownPending);
}

#[test]
fn requests_need_the_device_key() {
    let mut b = book();
    let phone = add_phone(&mut b, "Phone", T0);
    let other = DeviceKey::generate();
    let msg = b"ts=5&op=chats";
    assert_eq!(b.check_request(&phone.id, msg, &other.sign(msg), T0 + 5_000).unwrap_err(), DeviceError::BadSignature);
    assert_eq!(b.check_request(&phone.id, b"ts=6&op=chats", &phone.key.sign(msg), T0 + 5_000).unwrap_err(), DeviceError::BadSignature);
    assert_eq!(b.check_request("nobody", msg, &phone.key.sign(msg), T0 + 5_000).unwrap_err(), DeviceError::UnknownDevice);
}

#[test]
fn last_seen_is_saved_at_most_once_a_minute() {
    let mut b = book();
    let phone = add_phone(&mut b, "Phone", T0);
    assert!(b.authorize(&phone.id, T0 + 10_000).unwrap());
    assert!(!b.authorize(&phone.id, T0 + 30_000).unwrap());
    assert!(b.authorize(&phone.id, T0 + 70_000).unwrap());
    assert_eq!(b.device(&phone.id).unwrap().last_seen, Some(T0 + 70_000));
}

#[test]
fn was_this_you_events_for_new_devices_and_sign_ins() {
    let mut b = book();
    let phone = add_phone(&mut b, "Pixel", T0);
    let sign = b.sign_in(&phone.id, T0 + 60_000).unwrap();
    let q = b.questions();
    assert_eq!(q.len(), 2);
    assert_eq!(q[0].kind, EventKind::SignIn);
    assert_eq!(q[1].kind, EventKind::NewDevice);
    assert!(q[1].question().contains("Pixel"));
    b.answer(sign, true, T0 + 61_000).unwrap();
    assert_eq!(b.questions().len(), 1);
}

#[test]
fn answering_no_removes_the_device() {
    let mut b = book();
    let phone = add_phone(&mut b, "Stranger", T0);
    let ev = b.questions()[0].id;
    assert_eq!(b.answer(ev, false, T0 + 5_000).unwrap(), Some(phone.id.clone()));
    assert_eq!(b.authorize(&phone.id, T0 + 5_001).unwrap_err(), DeviceError::Revoked);
    assert!(b.events().iter().any(|e| e.kind == EventKind::NotMe));
    assert!(b.questions().is_empty());
    assert_eq!(b.answer(9_999, false, T0).unwrap_err(), DeviceError::UnknownEvent);
}

#[test]
fn events_are_capped() {
    let mut b = book();
    let phone = add_phone(&mut b, "Phone", T0);
    for i in 0..(events::MAX_EVENTS as i64 + 20) {
        b.sign_in(&phone.id, T0 + 100_000 + i).unwrap();
    }
    assert_eq!(b.events().len(), events::MAX_EVENTS);
}

#[test]
fn wrong_pin_lockout_grows_and_resets() {
    let mut l = Lockout::default();
    for _ in 0..FREE_TRIES {
        assert!(!l.failed(T0));
        assert!(l.ready(T0).is_ok());
    }
    assert!(l.failed(T0));
    assert_eq!(l.wait_ms(T0), FIRST_WAIT_MS);
    assert!(l.failed(T0 + FIRST_WAIT_MS));
    assert_eq!(l.wait_ms(T0 + FIRST_WAIT_MS), 2 * FIRST_WAIT_MS);
    assert!(l.failed(T0));
    assert_eq!(l.wait_ms(T0), 4 * FIRST_WAIT_MS);
    for _ in 0..30 {
        l.failed(T0);
    }
    assert_eq!(l.wait_ms(T0), MAX_WAIT_MS);
    // A clock set far back doesn't stretch the wait past the cap.
    assert_eq!(l.wait_ms(T0 - 24 * 60 * 60_000), MAX_WAIT_MS);
    l.succeeded();
    assert_eq!(l.fails(), 0);
    assert!(l.ready(T0).is_ok());
}

#[test]
fn wrong_pins_lock_the_device_out_and_ask_was_this_you() {
    let mut b = book();
    let phone = add_phone(&mut b, "Phone", T0);
    let now = T0 + 100_000;
    let mut wrong = Fake::with(&[Confirmation::PinWrong; 4]);
    for _ in 0..4 {
        assert_eq!(b.confirm(&phone.id, SensitiveAction::ChangePin, "Change the PIN?", &mut wrong, now).unwrap_err(), DeviceError::WrongPin);
    }
    assert_eq!(b.pin_wait_secs(&phone.id, now), 30);
    // While locked out, the confirmer isn't even asked.
    let mut right = Fake::with(&[Confirmation::PinRight]);
    assert_eq!(b.confirm(&phone.id, SensitiveAction::ChangePin, "Change the PIN?", &mut right, now + 1_000).unwrap_err(), DeviceError::Wait { secs: 29 });
    assert!(right.asked.is_empty());
    assert!(b.questions().iter().any(|e| e.kind == EventKind::WrongPins && e.device_id == phone.id));
    // Another device isn't held up by this one's mistakes.
    let host = b.host_id().to_string();
    assert!(b.confirm(&host, SensitiveAction::ChangePin, "Change the PIN?", &mut Fake::with(&[Confirmation::Hello]), now + 1_000).is_ok());
    assert!(b.confirm(&phone.id, SensitiveAction::ChangePin, "Change the PIN?", &mut right, now + FIRST_WAIT_MS).is_ok());
    assert_eq!(b.pin_wait_secs(&phone.id, now + FIRST_WAIT_MS), 0);
}

#[test]
fn lockout_is_kept_across_restarts() {
    let c = Cipher::for_test();
    let mut b = book();
    let phone = add_phone(&mut b, "Phone", T0);
    let now = T0 + 100_000;
    let mut wrong = Fake::with(&[Confirmation::PinWrong; 5]);
    for _ in 0..4 {
        let _ = b.confirm(&phone.id, SensitiveAction::TurnOffLock, "Turn off the lock?", &mut wrong, now);
    }
    let stored = b.seal(&c).unwrap();
    let mut again = DeviceBook::open(&stored, &c).unwrap();
    assert_eq!(again.pin_wait_secs(&phone.id, now), 30);
    // The count carries on after the restart, so the next wait is longer.
    let now = now + FIRST_WAIT_MS;
    assert_eq!(again.confirm(&phone.id, SensitiveAction::TurnOffLock, "x", &mut wrong, now).unwrap_err(), DeviceError::WrongPin);
    let mut again = DeviceBook::open(&again.seal(&c).unwrap(), &c).unwrap();
    assert_eq!(again.pin_wait_secs(&phone.id, now), 60);
    let mut right = Fake::with(&[Confirmation::PinRight]);
    assert!(matches!(again.confirm(&phone.id, SensitiveAction::TurnOffLock, "x", &mut right, now).unwrap_err(), DeviceError::Wait { .. }));

    // The pairing-code lockout survives too.
    let mut b = book();
    for _ in 0..=FREE_TRIES {
        let _ = b.redeem("WRONGCOD", "x", DeviceKind::Other, T0);
    }
    let mut again = DeviceBook::open(&b.seal(&c).unwrap(), &c).unwrap();
    assert!(matches!(again.redeem("WRONGCOD", "x", DeviceKind::Other, T0 + 1).unwrap_err(), DeviceError::Wait { .. }));
}

#[test]
fn the_sealed_list_round_trips_and_hides_keys() {
    let c = Cipher::for_test();
    let mut b = book();
    let phone = add_phone(&mut b, "Phone", T0);
    let hex_key = hex::encode(phone.key.bytes());
    let stored = b.seal(&c).unwrap();
    assert!(!stored.contains(&hex_key));
    let again = DeviceBook::open(&stored, &c).unwrap();
    assert_eq!(again.device(&phone.id).unwrap().key(), &phone.key);
    assert_eq!(again.rows(), b.rows());
    assert_eq!(again.host_id(), b.host_id());
    // The wrong key, plain text and damage are all just "couldn't be read".
    assert_eq!(DeviceBook::open(&stored, &Cipher::for_test()).unwrap_err(), DeviceError::Storage);
    assert_eq!(DeviceBook::open("{}", &c).unwrap_err(), DeviceError::Storage);
    assert_eq!(DeviceBook::open(&c.encrypt("not json"), &c).unwrap_err(), DeviceError::Storage);
}

#[test]
fn secrets_stay_out_of_debug_and_errors() {
    let mut b = book();
    let host = b.host_id().to_string();
    let offer = b.offer(&host, T0).unwrap();
    let code = offer.code.as_str().to_string();
    let joined = b.redeem(&code, "Phone", DeviceKind::Phone, T0 + 1_000).unwrap();
    let added = b.approve(&joined.pending_id, &host, &mut Fake::with(&[Confirmation::Hello]), T0 + 2_000).unwrap();
    let hex_key = hex::encode(added.key.bytes());
    let printed = format!("{offer:?} {added:?} {b:?} {:?}", b.rows());
    assert!(!printed.contains(&code), "pairing code in Debug output");
    assert!(!printed.contains(&hex_key), "device key in Debug output");
    let replay = b.redeem(&code, "Phone", DeviceKind::Phone, T0 + 3_000).unwrap_err();
    assert!(!replay.to_string().contains(&code));
    assert!(!format!("{replay:?}").contains(&code));
    let rows = serde_json::to_string(&b.rows()).unwrap();
    assert!(!rows.contains(&hex_key));
}

#[test]
fn names_are_cleaned() {
    let mut b = book();
    let a = add_phone(&mut b, "  \u{7}\n  ", T0);
    assert_eq!(b.device(&a.id).unwrap().name, "Phone");
    let long = add_phone(&mut b, &"x".repeat(200), T0 + 10_000);
    assert_eq!(b.device(&long.id).unwrap().name.chars().count(), 40);
    b.rename(&long.id, "Work phone").unwrap();
    assert_eq!(b.device(&long.id).unwrap().name, "Work phone");
}

#[test]
fn the_device_list_has_a_limit() {
    let mut b = book();
    for i in 0..(MAX_DEVICES as i64 - 1) {
        add_phone(&mut b, "Phone", T0 + i * 10_000);
    }
    let host = b.host_id().to_string();
    let offer = b.offer(&host, T0 + 1_000_000).unwrap();
    assert_eq!(b.redeem(offer.code.as_str(), "One more", DeviceKind::Phone, T0 + 1_000_001).unwrap_err(), DeviceError::TooManyDevices);
}

#[test]
fn reconfirm_policy() {
    use SensitiveAction::*;
    for a in [AddDevice, RemoveDevice, ChangePin, TurnOffLock, ShowRecoveryCode, DeleteAllData] {
        assert_eq!(freshness(a), Freshness::EveryTime);
        assert!(needs_confirm(a, Some(T0), T0), "{a:?} must ask every time");
    }
    assert!(needs_confirm(RevealSecret, None, T0));
    assert!(!needs_confirm(RevealSecret, Some(T0), T0 + confirm::SHORT_MS - 1));
    assert!(needs_confirm(RevealSecret, Some(T0), T0 + confirm::SHORT_MS));
    assert!(!needs_confirm(OpenProtectedChat, Some(T0), T0 + confirm::LONG_MS - 1));
    assert!(needs_confirm(OpenProtectedChat, Some(T0), T0 + confirm::LONG_MS));
    // A confirmation dated in the future (the clock went back) doesn't count.
    assert!(needs_confirm(ExportBackup, Some(T0 + 60_000), T0));
}

#[test]
fn a_recent_confirmation_covers_windowed_actions_only() {
    let mut b = book();
    let host = b.host_id().to_string();
    let mut fake = Fake::with(&[Confirmation::Hello, Confirmation::Hello]);
    b.confirm(&host, SensitiveAction::RevealSecret, "Show the API key?", &mut fake, T0).unwrap();
    b.confirm(&host, SensitiveAction::ExportBackup, "Export a backup?", &mut fake, T0 + 60_000).unwrap();
    assert_eq!(fake.asked.len(), 1);
    b.confirm(&host, SensitiveAction::ShowRecoveryCode, "Show the recovery code?", &mut fake, T0 + 61_000).unwrap();
    assert_eq!(fake.asked.len(), 2);
    let mut none = Fake::default();
    assert_eq!(b.confirm(&host, SensitiveAction::RevealSecret, "x", &mut none, T0 + 61_000 + confirm::SHORT_MS).unwrap_err(), DeviceError::NotConfirmed);
}

#[test]
fn a_removed_devices_node_id_is_refused_from_then_on() {
    let mut b = book();
    let host = b.host_id().to_string();
    let pc = add_phone(&mut b, "Laptop", T0);
    b.set_node_id(&pc.id, "12D3KooWnode").unwrap();
    assert_eq!(b.rows().iter().find(|r| r.id == pc.id).unwrap().node_id.as_deref(), Some("12D3KooWnode"));
    assert_eq!(b.revoked_node_ids().count(), 0);
    b.remove(&pc.id, &host, &mut Fake::with(&[Confirmation::Hello]), T0 + 20_000).unwrap();
    assert_eq!(b.revoked_node_ids().collect::<Vec<_>>(), ["12D3KooWnode"]);
    let other = add_phone(&mut b, "Other", T0 + 30_000);
    assert_eq!(b.set_node_id(&other.id, "12D3KooWnode").unwrap_err(), DeviceError::Revoked);
}

#[test]
fn lists_sealed_before_node_ids_still_open() {
    let mut v: serde_json::Value = serde_json::to_value(book()).unwrap();
    v.as_object_mut().unwrap().remove("revoked_nodes");
    v["devices"][0].as_object_mut().unwrap().remove("node_id");
    let b: DeviceBook = serde_json::from_value(v).unwrap();
    assert_eq!(b.rows()[0].node_id, None);
}
