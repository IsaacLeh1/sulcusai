// SPDX-License-Identifier: AGPL-3.0-only
use super::*;

const NOW: i64 = 1_800_000_000_000;
const DAY: i64 = 86_400_000;

fn key(store: &mut AccessKeys, name: &str, scopes: &[Scope]) -> Created {
    store.create(NewKey { name: name.into(), scopes: scopes.to_vec(), ..Default::default() }, NOW).unwrap()
}

fn compares() -> usize {
    secret::COMPARES.with(|c| c.get())
}

#[cfg(windows)]
const WORK: &str = r"C:\Users\someone\Work";
#[cfg(not(windows))]
const WORK: &str = "/home/someone/Work";

fn under_work(rest: &str) -> String {
    format!("{WORK}{}{rest}", std::path::MAIN_SEPARATOR)
}

#[test]
fn hash_is_sha256_hex_and_never_the_secret() {
    let s = secret::generate();
    assert!(s.starts_with(secret::PREFIX));
    assert_eq!(s.len(), secret::PREFIX.len() + 64);
    let h = secret::hash(&s);
    assert_eq!(h.len(), 64);
    assert!(h.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(h, secret::hash(&format!("  {}\n", s.as_str())));
    assert_ne!(h, secret::hash(&secret::generate()));
    assert!(!h.contains(&s[secret::PREFIX.len()..]));
}

#[test]
fn secrets_are_random() {
    let a = secret::generate();
    let b = secret::generate();
    assert_ne!(a.as_str(), b.as_str());
}

#[test]
fn the_secret_is_kept_only_as_a_hash() {
    let mut store = AccessKeys::default();
    let made = key(&mut store, "Notes app", &[Scope::UseModels]);
    let saved = serde_json::to_string(&store).unwrap();
    assert!(!saved.contains(made.secret.as_str()));
    assert!(saved.contains(&secret::hash(&made.secret)));
    let shown = serde_json::to_string(&store.list(NOW)).unwrap();
    assert!(!shown.contains(made.secret.as_str()));
    assert!(!shown.contains(&secret::hash(&made.secret)));
    assert_eq!(store.list(NOW)[0].hint, made.secret[made.secret.len() - 4..]);
    // Saved and loaded again, the key still works.
    let mut back: AccessKeys = serde_json::from_str(&saved).unwrap();
    assert!(back.authorize(&made.secret, Scope::UseModels, NOW).is_allowed());
}

#[test]
fn keys_are_taken_out_of_logs_by_secret_scrub() {
    let s = secret::generate();
    let line = crate::secret_scrub::scrub(&format!("Authorization: {}", s.as_str()));
    assert!(!line.contains(&s[secret::PREFIX.len()..]));
    let bare = crate::secret_scrub::scrub(&format!("key was {} here", s.as_str()));
    assert!(!bare.contains(&s[secret::PREFIX.len()..]), "{bare}");
}

#[test]
fn ct_eq_compares_whole_values() {
    assert!(secret::ct_eq(b"abcdef", b"abcdef"));
    assert!(!secret::ct_eq(b"abcdef", b"abcdeg"));
    assert!(!secret::ct_eq(b"Xbcdef", b"abcdef"));
    assert!(!secret::ct_eq(b"abc", b"abcdef"));
    assert!(secret::ct_eq(b"", b""));
}

#[test]
fn lookup_compares_every_key_in_constant_time() {
    let mut store = AccessKeys::default();
    let first = key(&mut store, "First", &[Scope::UseModels]);
    key(&mut store, "Second", &[Scope::UseModels]);
    key(&mut store, "Third", &[Scope::UseModels]);
    let before = compares();
    assert!(store.authorize(&first.secret, Scope::UseModels, NOW).is_allowed());
    // A match on the first key still compares against the other two.
    assert_eq!(compares() - before, 3);
    let before = compares();
    assert_eq!(store.authorize("sk-sulcus-akwrong", Scope::UseModels, NOW), Decision::Denied(Denial::UnknownKey));
    assert_eq!(compares() - before, 3);
}

#[test]
fn a_new_key_can_do_nothing() {
    let mut store = AccessKeys::default();
    let made = key(&mut store, "Empty", &[]);
    for s in Scope::ALL {
        let d = if s.is_files() {
            store.authorize_path(&made.secret, s, &under_work("a.txt"), NOW)
        } else {
            store.authorize(&made.secret, s, NOW)
        };
        assert_eq!(d, Decision::Denied(Denial::NotAllowed), "{s:?}");
    }
    assert!(NewKey::default().scopes.is_empty());
    let none: NewKey = serde_json::from_str(r#"{"name":"From the page"}"#).unwrap();
    assert!(none.scopes.is_empty());
}

#[test]
fn only_the_chosen_scopes_are_allowed() {
    let mut store = AccessKeys::default();
    let made = key(&mut store, "Calendar sync", &[Scope::ReadCalendar, Scope::ReadCalendar]);
    assert_eq!(store.get(&made.id).unwrap().scopes, vec![Scope::ReadCalendar]);
    assert_eq!(
        store.authorize(&made.secret, Scope::ReadCalendar, NOW),
        Decision::Allowed { key_id: made.id.clone(), app: "Calendar sync".into() }
    );
    assert_eq!(store.authorize(&made.secret, Scope::WriteCalendar, NOW), Decision::Denied(Denial::NotAllowed));
    assert_eq!(store.authorize(&made.secret, Scope::SendMail, NOW), Decision::Denied(Denial::NotAllowed));
}

#[test]
fn expired_keys_are_refused() {
    let mut store = AccessKeys::default();
    let made = store.create(NewKey { name: "Trial".into(), scopes: vec![Scope::UseModels], expires_at: Some(NOW + DAY), ..Default::default() }, NOW).unwrap();
    assert!(store.authorize(&made.secret, Scope::UseModels, NOW + DAY - 1).is_allowed());
    assert_eq!(store.authorize(&made.secret, Scope::UseModels, NOW + DAY), Decision::Denied(Denial::Expired));
    assert_eq!(store.list(NOW + DAY)[0].status, Status::Expired);
    assert!(store.create(NewKey { name: "Late".into(), expires_at: Some(NOW), ..Default::default() }, NOW).is_err());
}

#[test]
fn revoking_works_at_once() {
    let mut store = AccessKeys::default();
    let made = key(&mut store, "Old script", &[Scope::UseModels]);
    assert!(store.authorize(&made.secret, Scope::UseModels, NOW).is_allowed());
    store.revoke(&made.id, NOW + 1).unwrap();
    assert_eq!(store.authorize(&made.secret, Scope::UseModels, NOW + 2), Decision::Denied(Denial::Revoked));
    assert_eq!(store.list(NOW)[0].status, Status::Revoked);
    // Revoking twice keeps the first time.
    store.revoke(&made.id, NOW + 5).unwrap();
    assert_eq!(store.get(&made.id).unwrap().revoked_at, Some(NOW + 1));
    assert!(store.revoke("nope", NOW).is_err());
    assert!(store.set_scopes(&made.id, &Scope::ALL, &[], NOW).is_err());
    assert!(store.audit.entries().any(|e| e.outcome == Outcome::Revoked && e.key_id.as_deref() == Some(made.id.as_str())));
}

#[test]
fn denials_are_logged_with_app_scope_time_and_reason() {
    let mut store = AccessKeys::default();
    let made = key(&mut store, "Mail helper", &[Scope::ReadMail]);
    store.authorize(&made.secret, Scope::SendMail, NOW + 7);
    let denied = store.audit.denials();
    assert_eq!(denied.len(), 1);
    let e = &denied[0];
    assert_eq!(e.app, "Mail helper");
    assert_eq!(e.key_id.as_deref(), Some(made.id.as_str()));
    assert_eq!(e.scope, Some(Scope::SendMail));
    assert_eq!(e.at, NOW + 7);
    assert_eq!(e.outcome, Outcome::Denied { reason: Denial::NotAllowed });
    let json = serde_json::to_value(e).unwrap();
    assert_eq!(json["outcome"]["kind"], "denied");
    assert_eq!(json["outcome"]["reason"], "not_allowed");
    assert_eq!(json["scope"], "send_mail");
}

#[test]
fn unknown_keys_are_logged_without_the_key() {
    let mut store = AccessKeys::default();
    let tried = "sk-sulcus-akdeadbeefdeadbeefdeadbeef";
    assert_eq!(store.authorize(tried, Scope::ReadChats, NOW), Decision::Denied(Denial::UnknownKey));
    assert_eq!(store.authorize("", Scope::ReadChats, NOW), Decision::Denied(Denial::UnknownKey));
    let denied = store.audit.denials();
    assert_eq!(denied.len(), 2);
    assert_eq!(denied[0].app, "unknown key");
    assert_eq!(denied[0].key_id, None);
    assert!(!serde_json::to_string(&store.audit).unwrap().contains("deadbeef"));
}

#[test]
fn allowed_use_is_logged_and_sets_last_used() {
    let mut store = AccessKeys::default();
    let made = key(&mut store, "Writer", &[Scope::UseModels]);
    assert_eq!(store.list(NOW)[0].last_used_at, None);
    store.authorize(&made.secret, Scope::ReadChats, NOW + 3);
    assert_eq!(store.list(NOW)[0].last_used_at, None, "a refusal isn't a use");
    store.authorize(&made.secret, Scope::UseModels, NOW + 9);
    assert_eq!(store.list(NOW)[0].last_used_at, Some(NOW + 9));
    let last = store.audit.recent(1).pop().unwrap();
    assert_eq!(last.outcome, Outcome::Allowed);
    assert_eq!(last.scope, Some(Scope::UseModels));
    assert_eq!(store.audit.for_key(&made.id).len(), 3, "created, refused, allowed");
}

#[test]
fn the_audit_log_is_scrubbed() {
    let mut store = AccessKeys::default();
    let token = ["ghp_", "Xr4kT9qLm2Vb8NwZc5Yh1JdFgA7sPe3U0abc"].concat();
    let made = store
        .create(NewKey { name: format!("Bot token={token}"), scopes: vec![Scope::ReadFiles], folders: vec![WORK.into()], ..Default::default() }, NOW)
        .unwrap();
    let path = under_work(&format!("password={token}.txt"));
    store.authorize_path(&made.secret, Scope::ReadFiles, &path, NOW);
    store.authorize(&made.secret, Scope::SendMail, NOW);
    let all = serde_json::to_string(&store.audit).unwrap();
    assert!(!all.contains(&token), "{all}");
    assert!(all.contains("[removed"), "{all}");
    // The settings page still shows the name as it was typed.
    assert!(store.list(NOW)[0].name.contains(&token));
}

#[test]
fn file_scopes_stay_in_the_chosen_folders() {
    let mut store = AccessKeys::default();
    let made = store
        .create(NewKey { name: "Indexer".into(), scopes: vec![Scope::ReadFiles], folders: vec![WORK.into()], ..Default::default() }, NOW)
        .unwrap();
    let s = made.secret.as_str();
    assert!(store.authorize_path(s, Scope::ReadFiles, &under_work("plan.md"), NOW).is_allowed());
    assert!(store.authorize_path(s, Scope::ReadFiles, WORK, NOW).is_allowed());
    assert_eq!(store.authorize_path(s, Scope::ReadFiles, &format!("{WORK}-old{}a.txt", std::path::MAIN_SEPARATOR), NOW), Decision::Denied(Denial::OutsideFolders));
    assert_eq!(store.authorize_path(s, Scope::ReadFiles, &under_work("../Secrets/a.txt"), NOW), Decision::Denied(Denial::OutsideFolders));
    assert_eq!(store.authorize_path(s, Scope::ReadFiles, "plan.md", NOW), Decision::Denied(Denial::OutsideFolders));
    assert_eq!(store.authorize(s, Scope::ReadFiles, NOW), Decision::Denied(Denial::NeedsPath));
    assert_eq!(store.authorize_path(s, Scope::WriteFiles, &under_work("plan.md"), NOW), Decision::Denied(Denial::NotAllowed));
    // File scope with no folders chosen reaches nothing.
    let bare = key(&mut store, "No folders", &[Scope::ReadFiles]);
    assert_eq!(store.authorize_path(&bare.secret, Scope::ReadFiles, &under_work("plan.md"), NOW), Decision::Denied(Denial::OutsideFolders));
}

#[cfg(windows)]
#[test]
fn windows_folders_ignore_case_and_slashes() {
    assert!(folders::within(r"c:\users\SOMEONE\work\a.txt", WORK));
    assert!(folders::within("C:/Users/someone/Work/a.txt", WORK));
    assert!(folders::within(r"\\?\C:\Users\someone\Work\a.txt", WORK));
    assert!(!folders::within(r"D:\Users\someone\Work\a.txt", WORK));
}

#[test]
fn folders_must_be_full_paths() {
    assert!(folders::clean(&["Work".into()]).is_err());
    assert!(folders::clean(&[under_work("../x")]).is_err());
    assert_eq!(folders::clean(&[WORK.into(), format!(" {WORK} "), String::new()]).unwrap(), vec![WORK.to_string()]);
    let mut store = AccessKeys::default();
    assert!(store.create(NewKey { name: "Bad".into(), folders: vec!["relative".into()], ..Default::default() }, NOW).is_err());
}

#[test]
fn scopes_can_be_changed_and_the_change_is_logged() {
    let mut store = AccessKeys::default();
    let made = key(&mut store, "Helper", &[]);
    store.set_scopes(&made.id, &[Scope::UseAssistant], &[], NOW + 1).unwrap();
    assert!(store.authorize(&made.secret, Scope::UseAssistant, NOW + 2).is_allowed());
    let changed = store.audit.entries().find(|e| e.outcome == Outcome::Changed).unwrap();
    assert_eq!(changed.detail, "use_assistant");
    store.set_scopes(&made.id, &[], &[], NOW + 3).unwrap();
    assert_eq!(store.authorize(&made.secret, Scope::UseAssistant, NOW + 4), Decision::Denied(Denial::NotAllowed));
}

#[test]
fn names_are_required_and_tidied() {
    let mut store = AccessKeys::default();
    assert!(store.create(NewKey { name: "   ".into(), ..Default::default() }, NOW).is_err());
    let made = key(&mut store, "  My   app ", &[]);
    assert_eq!(store.get(&made.id).unwrap().name, "My app");
    let long = key(&mut store, &"x".repeat(500), &[]);
    assert_eq!(store.get(&long.id).unwrap().name.len(), NAME_MAX);
}

#[test]
fn the_old_local_api_key_becomes_a_full_legacy_key() {
    let mut store = AccessKeys::default();
    let old = "sk-sulcus-3f9a1c7e5b2d4086a1c3e5f7b9d0e2a4";
    assert_eq!(store.migrate_legacy("  ", NOW), None);
    let id = store.migrate_legacy(old, NOW).unwrap();
    let rec = store.get(&id).unwrap();
    assert!(rec.legacy && rec.all_folders);
    assert_eq!(rec.scopes, Scope::ALL.to_vec());
    assert_eq!(rec.expires_at, None);
    assert!(!serde_json::to_string(&store).unwrap().contains(old));
    for s in Scope::ALL {
        let d = if s.is_files() { store.authorize_path(old, s, &under_work("a"), NOW) } else { store.authorize(old, s, NOW) };
        assert!(d.is_allowed(), "{s:?}");
    }
    assert_eq!(store.authorize_path(old, Scope::ReadFiles, "relative.txt", NOW), Decision::Denied(Denial::OutsideFolders));
    // Running it again changes nothing.
    assert_eq!(store.migrate_legacy(old, NOW + 1), Some(id.clone()));
    assert_eq!(store.list(NOW).len(), 1);
    // A replaced old key takes over and the one before stops working.
    let newer = "sk-sulcus-0d2f9c7a41b85e36f0d2f9c7a41b85e3";
    let id2 = store.migrate_legacy(newer, NOW + 2).unwrap();
    assert_ne!(id, id2);
    assert_eq!(store.authorize(old, Scope::UseModels, NOW + 3), Decision::Denied(Denial::Revoked));
    assert!(store.authorize(newer, Scope::UseModels, NOW + 3).is_allowed());
    // It can be revoked like any other key.
    store.revoke(&id2, NOW + 4).unwrap();
    assert_eq!(store.authorize(newer, Scope::UseModels, NOW + 5), Decision::Denied(Denial::Revoked));
}

#[test]
fn the_list_shows_newest_first_with_last_use() {
    let mut store = AccessKeys::default();
    let a = store.create(NewKey { name: "A".into(), scopes: vec![Scope::UseModels], ..Default::default() }, NOW).unwrap();
    store.create(NewKey { name: "B".into(), ..Default::default() }, NOW + 10).unwrap();
    store.authorize(&a.secret, Scope::UseModels, NOW + 20);
    let l = store.list(NOW + 30);
    assert_eq!(l.iter().map(|k| k.name.as_str()).collect::<Vec<_>>(), ["B", "A"]);
    assert_eq!(l[1].last_used_at, Some(NOW + 20));
    assert_eq!(l[1].scopes[0].id, "use_models");
    assert_eq!(l[0].status, Status::Active);
}

#[test]
fn the_audit_log_keeps_the_newest_entries() {
    let mut store = AccessKeys::default();
    for i in 0..(audit::MAX_ENTRIES as i64 + 10) {
        store.authorize("wrong", Scope::UseModels, i);
    }
    assert_eq!(store.audit.entries().count(), audit::MAX_ENTRIES);
    assert_eq!(store.audit.entries().next().unwrap().at, 10);
    assert_eq!(store.audit.recent(1)[0].at, audit::MAX_ENTRIES as i64 + 9);
}

#[test]
fn every_scope_has_plain_words() {
    let all = catalog();
    assert_eq!(all.len(), Scope::ALL.len());
    let mut ids: Vec<_> = all.iter().map(|i| i.id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), all.len());
    for i in &all {
        assert!(!i.title.is_empty() && i.description.len() > 20, "{:?}", i.scope);
        assert!(!i.description.contains('\u{2014}') && !i.title.contains('\u{2014}'));
        assert_eq!(Scope::parse(i.id), Some(i.scope));
        assert_eq!(serde_json::to_value(i.scope).unwrap(), i.id);
    }
    assert_eq!(Scope::parse(" READ_MAIL "), Some(Scope::ReadMail));
    assert_eq!(Scope::parse("everything"), None);
    assert!(Scope::SendMail.info().risk > Scope::UseModels.info().risk);
}

#[test]
fn denials_say_what_to_do() {
    for d in [Denial::UnknownKey, Denial::Revoked, Denial::Expired, Denial::NotAllowed, Denial::NeedsPath, Denial::OutsideFolders] {
        assert!(!d.message().is_empty());
        assert!(!d.message().contains('\u{2014}'));
    }
    assert_eq!(Denial::UnknownKey.http_status(), 401);
    assert_eq!(Denial::NotAllowed.http_status(), 403);
}
