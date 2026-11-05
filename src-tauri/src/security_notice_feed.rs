// SPDX-License-Identifier: AGPL-3.0-only
//! Getting the signed security notice (`security_notice.rs`) onto this PC:
//! fetched from the project's own GitHub release on start and about daily,
//! checked, kept, and shown on the Security page. Every check is in
//! `security_notice`; this file only moves bytes and keeps the last verified
//! list. Without a trusted signing key nothing is fetched, because nothing
//! could be accepted.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::net::{self, Purpose};
use crate::security_notice::{self as notice, Installed, NoticeError, PublicKey, Severity, TrustState};
use crate::{db, AppState, AppStateRef};

const STATE_KEY: &str = "security_notice_state";
const META_KEY: &str = "security_notice_meta";
/// The newest notice is the release asset of the latest release, so
/// publishing one is uploading a file; no server of anyone's is involved.
const URL: &str = "https://github.com/IsaacLeh1/sulcusai/releases/latest/download/security-notice.json";
const EVERY_MS: i64 = 24 * 3600 * 1000;

/// What the last check did, for the page. Kept apart from the trust state
/// because a failed or empty check must not touch the accepted notice.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Meta {
    last_check: i64,
    /// "new", "same", "none" (nothing published yet), "rejected", "failed".
    last_result: Option<String>,
    last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct HitView {
    pub kind: String,
    pub id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MessageView {
    /// "info", "warning" or "critical".
    pub severity: &'static str,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct NoticeView {
    /// "no_key" (this build trusts no signing key yet, so no notice can be
    /// accepted), "waiting" (a key exists, nothing accepted yet) or "current".
    pub status: &'static str,
    pub last_check: i64,
    pub last_result: Option<String>,
    pub last_error: Option<String>,
    pub sequence: u64,
    pub issued: i64,
    pub update_needed: bool,
    pub lowest_safe_version: String,
    pub revoked: Vec<HitView>,
    pub messages: Vec<MessageView>,
}

fn load(state: &AppState) -> (TrustState, Meta) {
    let conn = state.db.lock().unwrap();
    (db::get(&conn, STATE_KEY).unwrap_or_default(), db::get(&conn, META_KEY).unwrap_or_default())
}

/// Takes in a received file: the new trust state and what to tell the page.
/// A file already held, or older, is a normal answer (peers will resend),
/// not an error.
fn apply(raw: &[u8], trust: &TrustState, meta: &Meta, built_in: &[PublicKey], now_secs: i64, now_ms: i64) -> (TrustState, Meta) {
    let mut meta = Meta { last_check: now_ms, ..meta.clone() };
    match notice::accept_with(raw, trust, built_in, now_secs) {
        Ok((_, next)) => {
            meta.last_result = Some("new".into());
            meta.last_error = None;
            (next, meta)
        }
        Err(NoticeError::AlreadyHave | NoticeError::Older) => {
            meta.last_result = Some("same".into());
            meta.last_error = None;
            (trust.clone(), meta)
        }
        Err(e) => {
            meta.last_result = Some("rejected".into());
            meta.last_error = Some(e.to_string());
            (trust.clone(), meta)
        }
    }
}

fn build_view(trust: &TrustState, meta: &Meta, built_in: &[PublicKey], app_version: &str, installed: &[Installed]) -> NoticeView {
    let keyed = !notice::trusted_keys(built_in, trust).is_empty();
    let current = notice::current_with(trust, built_in);
    let status = match (&current, keyed) {
        (Some(_), _) => "current",
        (None, true) => "waiting",
        (None, false) => "no_key",
    };
    let mut view = NoticeView {
        status,
        last_check: meta.last_check,
        last_result: meta.last_result.clone(),
        last_error: meta.last_error.clone(),
        sequence: 0,
        issued: 0,
        update_needed: false,
        lowest_safe_version: String::new(),
        revoked: Vec::new(),
        messages: Vec::new(),
    };
    if let Some(n) = current {
        let a = notice::assess(&n, app_version, installed);
        view.sequence = n.sequence;
        view.issued = n.issued;
        view.update_needed = a.update_needed;
        view.lowest_safe_version = a.lowest_safe_version;
        view.revoked = a.revoked.into_iter().map(|r| HitView { kind: r.kind, id: r.id, reason: r.reason }).collect();
        view.messages = a
            .messages
            .into_iter()
            .map(|m| MessageView {
                severity: match m.severity {
                    Severity::Info => "info",
                    Severity::Critical => "critical",
                    Severity::Warning | Severity::Other => "warning",
                },
                text: m.text,
            })
            .collect();
    }
    view
}

fn view(state: &AppState) -> NoticeView {
    let (trust, meta) = load(state);
    // Engines and helpers are both unpacked downloads, matched by the
    // catalog's archive hash; the kind is only a label.
    let installed: Vec<Installed> = crate::integrity::installed_archives(&state.paths)
        .into_iter()
        .map(|(dir, sha256)| Installed { kind: "engine".into(), id: dir, sha256 })
        .collect();
    build_view(&trust, &meta, &notice::built_in_keys(), env!("CARGO_PKG_VERSION"), &installed)
}

fn now_secs() -> i64 {
    db::now_ms() / 1000
}

/// Reads the response without letting a huge file fill memory: the notice
/// limit plus a little for the envelope.
async fn download(client: &reqwest::Client) -> Result<Option<Vec<u8>>, String> {
    let mut resp = client.get(URL).timeout(Duration::from_secs(30)).send().await.map_err(|e| format!("Couldn't reach GitHub for the security notice: {e}"))?;
    // No file published yet is the normal state before the first notice.
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Err(format!("GitHub answered {} for the security notice.", resp.status()));
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| format!("The security notice download stopped: {e}"))? {
        body.extend_from_slice(&chunk);
        if body.len() > notice::MAX_NOTICE_BYTES {
            return Err(NoticeError::TooLarge.to_string());
        }
    }
    Ok(Some(body))
}

/// One check. `manual` is the person's click; the automatic one also obeys
/// the switch that turns automatic update checks off.
async fn refresh(state: &AppState, manual: bool) -> NoticeView {
    let built_in = notice::built_in_keys();
    let (trust, meta) = load(state);
    let nothing_to_check = notice::trusted_keys(&built_in, &trust).is_empty();
    let auto_off = !manual && db::get::<serde_json::Value>(&state.db.lock().unwrap(), "updates").is_some_and(|v| v["auto"] == false);
    if nothing_to_check || auto_off {
        return view(state);
    }
    let result = match net::external_client(state.settings().connectivity, Purpose::Notices, false) {
        Ok(client) => {
            state.log("network", "Checked GitHub for a security notice");
            download(&client).await
        }
        Err(e) => Err(e),
    };
    let (next_trust, next_meta) = match result {
        Ok(Some(raw)) => apply(&raw, &trust, &meta, &built_in, now_secs(), db::now_ms()),
        Ok(None) => (trust.clone(), Meta { last_check: db::now_ms(), last_result: Some("none".into()), last_error: None }),
        Err(e) => (trust.clone(), Meta { last_check: db::now_ms(), last_result: Some("failed".into()), last_error: Some(e) }),
    };
    store(state, &next_trust, &next_meta);
    view(state)
}

fn store(state: &AppState, trust: &TrustState, meta: &Meta) {
    let conn = state.db.lock().unwrap();
    let _ = db::set(&conn, STATE_KEY, trust);
    let _ = db::set(&conn, META_KEY, meta);
}

/// Checks soon after the app opens and about daily after.
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        crate::quiet::nap(Duration::from_secs(60)).await;
        loop {
            let state = app.state::<std::sync::Arc<AppState>>().inner().clone();
            if db::now_ms() - load(&state).1.last_check >= EVERY_MS {
                refresh(&state, false).await;
            }
            crate::quiet::nap(Duration::from_secs(3600)).await;
        }
    });
}

/// The sequence number of the notice in use (0 for none).
pub fn sequence(state: &AppState) -> u64 {
    load(state).0.last_sequence
}

/// The accepted notice exactly as received, for passing to a peer.
pub fn notice_for_peers(state: &AppState) -> Option<String> {
    load(state).0.notice
}

/// A notice a peer sent. Same checks as a fetched one, so the sender is not
/// trusted with anything; true when it was newer and is now in use.
pub fn take_from_peer(state: &AppState, raw: &[u8]) -> bool {
    let (trust, meta) = load(state);
    let (next, _) = apply(raw, &trust, &meta, &notice::built_in_keys(), now_secs(), db::now_ms());
    let changed = next != trust;
    if changed {
        // The page's "last check" stays about GitHub, so only the trust state is kept.
        let conn = state.db.lock().unwrap();
        let _ = db::set(&conn, STATE_KEY, &next);
    }
    changed
}

#[tauri::command(async)]
pub fn security_notice_view(state: AppStateRef) -> NoticeView {
    view(&state)
}

#[tauri::command]
pub async fn security_notice_check(state: AppStateRef<'_>) -> Result<NoticeView, String> {
    Ok(refresh(&state, true).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security_notice::dev_sign::DevKey;
    use crate::security_notice::{Message, Notice, Revoked};

    const NOW: i64 = 1_793_000_000;
    const HASH: &str = "8ce71f060c08cc8b89115eab0d63209d80537936706544d95c3b936fd0f63095";

    fn notice_n(seq: u64) -> Notice {
        Notice {
            sequence: seq,
            issued: NOW - 60,
            lowest_safe_version: "0.9.60".into(),
            revoked: vec![Revoked { kind: "engine".into(), id: "llama".into(), sha256: HASH.into(), reason: "unsafe".into() }],
            revoked_keys: vec![],
            next_keys: vec![],
            messages: vec![Message { severity: Severity::Critical, text: "Update now.".into(), below_version: None }],
        }
    }

    #[test]
    fn no_signing_key_is_stated_plainly() {
        let v = build_view(&TrustState::default(), &Meta::default(), &[], "0.9.49", &[]);
        assert_eq!(v.status, "no_key");
        assert!(v.messages.is_empty() && v.revoked.is_empty());
        assert_eq!(notice::built_in_keys().len(), 2, "the signing key and its backup");
    }

    #[test]
    fn a_notice_signed_by_a_test_key_shows() {
        let k = DevKey::from_seed([9; 32]);
        let keys = [k.public()];
        let raw = k.sign(&notice_n(1));
        let (trust, meta) = apply(raw.as_bytes(), &TrustState::default(), &Meta::default(), &keys, NOW, 5);
        assert_eq!(meta.last_result.as_deref(), Some("new"));
        let installed = [Installed { kind: "engine".into(), id: "llama-b1".into(), sha256: HASH.into() }];
        let v = build_view(&trust, &meta, &keys, "0.9.49", &installed);
        assert_eq!(v.status, "current");
        assert!(v.update_needed);
        assert_eq!(v.revoked.len(), 1);
        assert_eq!(v.revoked[0].id, "llama-b1");
        assert_eq!(v.messages[0].severity, "critical");
    }

    #[test]
    fn a_key_with_no_notice_yet_is_waiting() {
        let k = DevKey::from_seed([9; 32]);
        assert_eq!(build_view(&TrustState::default(), &Meta::default(), &[k.public()], "1.0.0", &[]).status, "waiting");
    }

    #[test]
    fn bad_files_leave_the_held_notice_alone() {
        let k = DevKey::from_seed([9; 32]);
        let other = DevKey::from_seed([8; 32]);
        let keys = [k.public()];
        let (trust, _) = apply(k.sign(&notice_n(2)).as_bytes(), &TrustState::default(), &Meta::default(), &keys, NOW, 1);

        let (same, meta) = apply(k.sign(&notice_n(2)).as_bytes(), &trust, &Meta::default(), &keys, NOW, 2);
        assert_eq!((same, meta.last_result.as_deref(), meta.last_error), (trust.clone(), Some("same"), None));
        let (older, meta) = apply(k.sign(&notice_n(1)).as_bytes(), &trust, &Meta::default(), &keys, NOW, 3);
        assert_eq!((older, meta.last_result.as_deref()), (trust.clone(), Some("same")));

        let (kept, meta) = apply(other.sign(&notice_n(3)).as_bytes(), &trust, &Meta::default(), &keys, NOW, 4);
        assert_eq!(kept, trust);
        assert_eq!(meta.last_result.as_deref(), Some("rejected"));
        assert!(meta.last_error.unwrap().contains("doesn't trust"));
        let (kept, meta) = apply(b"<html>not a notice</html>", &trust, &Meta::default(), &keys, NOW, 5);
        assert_eq!((kept, meta.last_result.as_deref()), (trust, Some("rejected")));
    }

    #[test]
    fn with_no_key_every_notice_is_rejected() {
        let k = DevKey::from_seed([9; 32]);
        let (trust, meta) = apply(k.sign(&notice_n(1)).as_bytes(), &TrustState::default(), &Meta::default(), &[], NOW, 1);
        assert_eq!(trust, TrustState::default());
        assert_eq!(meta.last_result.as_deref(), Some("rejected"));
    }

    #[test]
    fn stored_state_survives_a_restart() {
        let k = DevKey::from_seed([9; 32]);
        let keys = [k.public()];
        let (trust, meta) = apply(k.sign(&notice_n(4)).as_bytes(), &TrustState::default(), &Meta::default(), &keys, NOW, 1);
        let back: TrustState = serde_json::from_str(&serde_json::to_string(&trust).unwrap()).unwrap();
        assert_eq!(build_view(&back, &meta, &keys, "0.9.49", &[]).sequence, 4);
    }
}
