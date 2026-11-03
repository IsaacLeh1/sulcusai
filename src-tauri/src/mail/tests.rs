// SPDX-License-Identifier: AGPL-3.0-only
use super::*;

const RAW: &[u8] = b"From: Ana Ruiz <ana@example.com>\r\nTo: me@example.org, team@example.org\r\nSubject: Budget review on Friday\r\nMessage-ID: <abc123@example.com>\r\nDate: Tue, 6 Oct 2026 09:30:00 -0600\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHi,\r\n\r\nCan we move the budget review to Friday at 2pm?\r\n\r\nAna\r\n";

#[test]
fn changes_apply_here_and_queue_for_the_server() {
    struct Plain;
    impl crate::crypto::Protector for Plain {
        fn protect(&self, d: &[u8]) -> Result<Vec<u8>, String> {
            Ok(d.to_vec())
        }
        fn unprotect(&self, d: &[u8]) -> Result<Vec<u8>, String> {
            Ok(d.to_vec())
        }
    }
    let dir = std::env::temp_dir().join(format!("sulcusai-mail-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let c = crate::crypto::Vault::open(&dir.join("keys.json"), Box::new(Plain)).unwrap().cipher().unwrap();
    let conn = db::open(&dir.join("t.db")).unwrap();
    store_account(&conn, &c, "acc", &AccountConfig { email: "me@example.org".into(), ..Default::default() }).unwrap();
    for uid in 1..=3 {
        save_item(&conn, &c, "acc", "INBOX", uid, 1_000 * uid as i64, false, &MailData { subject: format!("m{uid}"), ..Default::default() }).unwrap();
    }
    let id = |subject: &str| list(&conn, &c, "", 10).into_iter().find(|m| m.data.subject == subject).unwrap().id;
    let (m1, m2, m3) = (id("m1"), id("m2"), id("m3"));
    change(&conn, &[m1.clone()], Change::Seen).unwrap();
    change(&conn, &[m2.clone()], Change::Archive).unwrap();
    change(&conn, &[m3.clone()], Change::Trash).unwrap();
    assert!(get(&conn, &c, &m1).unwrap().seen);
    assert_eq!(get(&conn, &c, &m2).unwrap().folder, ARCHIVE, "archived mail stays here, out of the inbox");
    assert!(get(&conn, &c, &m3).is_none(), "trashed mail leaves this PC");
    let queued: Vec<(String, u32, String)> = conn
        .prepare("SELECT folder, uid, op FROM mail_pending ORDER BY rowid")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert_eq!(queued, vec![("INBOX".into(), 1, "seen".into()), ("INBOX".into(), 2, "archive".into()), ("INBOX".into(), 3, "trash".into())]);
    // Trashed from the archive: nothing more to tell the server.
    change(&conn, &[m2.clone()], Change::Trash).unwrap();
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM mail_pending", [], |r| r.get::<_, i64>(0)).unwrap(), 3);
}

#[test]
fn mark_read_lists_it_under_marked_read_until_unread_again() {
    struct Plain;
    impl crate::crypto::Protector for Plain {
        fn protect(&self, d: &[u8]) -> Result<Vec<u8>, String> {
            Ok(d.to_vec())
        }
        fn unprotect(&self, d: &[u8]) -> Result<Vec<u8>, String> {
            Ok(d.to_vec())
        }
    }
    let dir = std::env::temp_dir().join(format!("sulcusai-mail-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let c = crate::crypto::Vault::open(&dir.join("keys.json"), Box::new(Plain)).unwrap().cipher().unwrap();
    let conn = db::open(&dir.join("t.db")).unwrap();
    store_account(&conn, &c, "acc", &AccountConfig { email: "me@example.org".into(), ..Default::default() }).unwrap();
    for uid in 1..=3 {
        save_item(&conn, &c, "acc", "INBOX", uid, 1_000 * uid as i64, false, &MailData { subject: format!("m{uid}"), ..Default::default() }).unwrap();
    }
    let id = |subject: &str| list(&conn, &c, "", 10).into_iter().find(|m| m.data.subject == subject).unwrap().id;
    let (m1, m2, m3) = (id("m1"), id("m2"), id("m3"));
    assert!(get(&conn, &c, &m1).unwrap().marked_at.is_none());
    mark_read(&conn, &[m1.clone(), m2.clone()]).unwrap();
    let (a, b) = (get(&conn, &c, &m1).unwrap(), get(&conn, &c, &m2).unwrap());
    assert!(a.seen && b.seen);
    assert!(a.marked_at.is_some() && b.marked_at > a.marked_at, "marked in order");
    // Opening a message reads it without listing it under Marked read.
    change(&conn, std::slice::from_ref(&m3), Change::Seen).unwrap();
    assert!(get(&conn, &c, &m3).unwrap().marked_at.is_none());
    // Mark unread (or the AI's undo): back in the inbox.
    change(&conn, std::slice::from_ref(&m1), Change::Unseen).unwrap();
    let a = get(&conn, &c, &m1).unwrap();
    assert!(!a.seen && a.marked_at.is_none());
    let ops: Vec<String> = conn.prepare("SELECT op FROM mail_pending ORDER BY rowid").unwrap().query_map([], |r| r.get(0)).unwrap().filter_map(Result::ok).collect();
    assert_eq!(ops, ["seen", "seen", "seen", "unseen"], "every mark goes to the server");
}

#[test]
fn list_headers_say_how_to_unsubscribe() {
    let raw = b"From: The Paper <news@paper.example>\r\nTo: me@example.org\r\nSubject: Morning\r\nList-Id: \"The Morning\" <morning.paper.example>\r\nList-Unsubscribe: <mailto:leave@paper.example?subject=unsubscribe%20me>,\r\n <https://paper.example/u?id=1\r\n 23>, <http://plain.example/u>\r\nList-Unsubscribe-Post: List-Unsubscribe=One-Click\r\nContent-Type: text/plain\r\n\r\nNews.\r\n";
    let (m, _) = parse(raw, false);
    assert_eq!(m.unsubscribe, vec!["mailto:leave@paper.example?subject=unsubscribe%20me", "https://paper.example/u?id=123"], "plain http is left out; folding is dropped");
    assert!(m.one_click && m.list_checked);
    assert_eq!((m.list_id.as_str(), m.list_name.as_str()), ("morning.paper.example", "The Morning"));
    // Personal mail: none of it, but read.
    let (p, _) = parse(RAW, false);
    assert!(p.unsubscribe.is_empty() && !p.one_click && p.list_id.is_empty() && p.list_checked);
    // Older mail: the headers fetched on their own.
    let fields = b"List-Unsubscribe: <https://shop.example/out>\r\nList-Id: <deals.shop.example>\r\n\r\n";
    let h = list_headers_raw(fields);
    assert_eq!((h.unsubscribe, h.one_click, h.list_id.as_str(), h.list_name.as_str()), (vec!["https://shop.example/out".to_string()], false, "deals.shop.example", ""));
    assert_eq!(list_headers_raw(b"\r\n"), ListHeaders::default());
    assert!(is_one_click(" list-unsubscribe=one-click\r\n"));
    assert!(!is_one_click("List-Unsubscribe=Later"));
    assert_eq!(parse_list_id("news.example.com"), ("news.example.com".to_string(), String::new()));
}

#[test]
fn invisible_padding_is_dropped() {
    let preview = "Learn the signs. \u{034F} \u{200C}\u{00AD} \u{034F} \u{FEFF}Save  energy";
    assert_eq!(visible(preview), "Learn the signs. Save energy");
}

#[test]
fn searches_understand_from_to_and_subject() {
    let (m, _) = parse(RAW, false);
    let hit = |q: &str| search_terms(q).iter().all(|t| t.matches(&m));
    assert!(hit("budget"));
    assert!(hit("from:Ana"));
    assert!(hit("from:\"ana@example.com\" subject:review"));
    assert!(hit("to:team"));
    assert!(!hit("from:team"));
    assert!(!hit("subject:ana"));
    assert!(hit(""));
    assert_eq!(search_terms("from: x"), vec![Term::Any("x".into())]);
    // How models search: the whole request.
    assert!(hit("my most recent email from Ana"));
    assert_eq!(search_terms("Summarize my latest emails"), vec![]);
}

#[test]
fn messages_are_read() {
    let (m, date) = parse(RAW, false);
    assert_eq!(m.from, "ana@example.com");
    assert_eq!(m.from_name, "Ana Ruiz");
    assert_eq!(m.to, vec!["me@example.org", "team@example.org"]);
    assert_eq!(m.subject, "Budget review on Friday");
    assert_eq!(m.message_id, "abc123@example.com");
    assert!(m.body.contains("Friday at 2pm") && !m.body.contains('\r'));
    assert!(m.snippet.starts_with("Hi, Can we move"));
    assert_eq!(date, Some(1_791_300_600_000));
}

#[test]
fn html_only_mail_becomes_text() {
    let raw = b"From: a@b.c\r\nSubject: x\r\nContent-Type: text/html\r\n\r\n<html><body><p>Your <b>order</b> shipped.</p><script>x()</script></body></html>";
    let (m, _) = parse(raw, false);
    assert!(m.body.contains("order") && m.body.contains("shipped") && !m.body.contains("x()"), "{}", m.body);
}

#[test]
fn providers_are_filled_in() {
    let (g, note) = preset("me@gmail.com");
    assert_eq!((g.imap_host.as_str(), g.imap_port, g.smtp_port), ("imap.gmail.com", 993, 465));
    assert!(note.unwrap().contains("app password"));
    let (o, _) = preset("x@company.org");
    assert_eq!(o.imap_host, "imap.company.org");
    let (i, _) = preset("kim@icloud.com");
    assert_eq!(i.username, "kim");
}

#[test]
fn replies_thread_and_bad_addresses_are_caught() {
    let cfg = AccountConfig { name: "Me".into(), email: "me@example.org".into(), ..Default::default() };
    let (orig, _) = parse(RAW, false);
    let d = Draft { to: vec!["ana@example.com".into()], subject: "Re: Budget review on Friday".into(), body: "Friday works.".into(), ..Default::default() };
    let msg = String::from_utf8(build_with(&cfg, &d, Some(&orig), None).unwrap().formatted()).unwrap();
    assert!(msg.contains("In-Reply-To: <abc123@example.com>") && msg.contains("Friday works."), "{msg}");
    let bad = Draft { to: vec!["not an address".into()], ..d.clone() };
    assert!(build_with(&cfg, &bad, None, None).is_err());
    assert!(build_with(&cfg, &Draft { to: vec![], ..d }, None, None).is_err());
    assert_eq!(addresses("a@x.com, Bo <b@y.com>; "), vec!["a@x.com", "Bo <b@y.com>"]);
}

fn invite_mail(method: &str, how: &str) -> Vec<u8> {
    let ics = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nMETHOD:{method}\r\nBEGIN:VEVENT\r\nUID:inv-1@example.com\r\nSEQUENCE:0\r\nDTSTART:20261023T210000Z\r\nDTEND:20261023T220000Z\r\nSUMMARY:Lunch\r\nORGANIZER;CN=Ana Ruiz:mailto:ana@example.com\r\nATTENDEE;PARTSTAT=NEEDS-ACTION;CN=Me:mailto:me@example.org\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let part = match how {
        // Inline, as Gmail and Outlook send it.
        "inline" => format!("Content-Type: text/calendar; charset=utf-8; method={method}\r\n\r\n{ics}"),
        // Only as a file.
        _ => format!("Content-Type: application/octet-stream; name=invite.ics\r\nContent-Disposition: attachment; filename=invite.ics\r\n\r\n{ics}"),
    };
    format!(
        "From: Ana Ruiz <ana@example.com>\r\nTo: me@example.org\r\nSubject: Invitation: Lunch\r\nMessage-ID: <inv-mail@example.com>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"b1\"\r\n\r\n--b1\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nYou're invited to lunch.\r\n--b1\r\n{part}\r\n--b1--\r\n"
    )
    .into_bytes()
}

#[test]
fn invitations_are_kept_with_the_message() {
    for how in ["inline", "file"] {
        let (m, _) = parse(&invite_mail("REQUEST", how), false);
        let inv = m.invite.unwrap_or_else(|| panic!("{how}: no invitation"));
        assert_eq!((inv.method.as_str(), inv.uid.as_str(), inv.title.as_str(), inv.organizer.as_str()), ("REQUEST", "inv-1@example.com", "Lunch", "ana@example.com"), "{how}");
        assert!(inv.ics.contains("UID:inv-1@example.com"), "{how}: the raw text is kept");
        assert!(m.body.contains("invited to lunch"), "{how}: {}", m.body);
    }
    // A cancellation or a reply isn't an invitation to answer.
    for method in ["CANCEL", "REPLY"] {
        assert!(parse(&invite_mail(method, "inline"), false).0.invite.is_none(), "{method}");
    }
    assert!(parse(RAW, false).0.invite.is_none());
    // Over the size cap: the event is kept, the raw text isn't.
    let big = String::from_utf8(invite_mail("REQUEST", "inline")).unwrap().replace("SUMMARY:Lunch", &format!("SUMMARY:Lunch\r\nDESCRIPTION:{}", "x".repeat(crate::calendar::MAX_ICS)));
    let inv = parse(big.as_bytes(), false).0.invite.unwrap();
    assert!(inv.ics.is_empty() && inv.title == "Lunch");
}

#[test]
fn invitation_answers_go_to_the_organizer_with_a_calendar_reply() {
    let (m, _) = parse(&invite_mail("REQUEST", "inline"), false);
    let inv = m.invite.clone().unwrap();
    assert_eq!(partstat("accept"), Some("ACCEPTED"));
    assert_eq!(partstat("maybe"), Some("TENTATIVE"));
    assert_eq!(partstat("decline"), Some("DECLINED"));
    assert_eq!(partstat("later"), None);
    assert_eq!(invited_as(&inv, "ME@example.org"), "me@example.org", "the address on the invitation");
    assert_eq!(invited_as(&inv, "other@example.org"), "other@example.org");
    let d = invite_reply_draft(&inv, "TENTATIVE", "Isaac", "local-id");
    assert_eq!((d.to.clone(), d.subject.as_str(), d.body.as_str()), (vec!["ana@example.com".to_string()], "Tentative: Lunch", "Isaac has tentatively accepted this invitation."));
    // A title can't add header lines.
    let sneaky = crate::calendar::Invite { title: "Lunch\r\nBcc: x@evil.example".into(), ..inv.clone() };
    assert_eq!(invite_reply_draft(&sneaky, "ACCEPTED", "", "id").subject, "Accepted: Lunch Bcc: x@evil.example");
    let cfg = AccountConfig { name: "Isaac".into(), email: "me@example.org".into(), ..Default::default() };
    let ics = crate::calendar::reply_ics(&inv, "me@example.org", "Isaac", "TENTATIVE", 1_000);
    let msg = String::from_utf8(build_with(&cfg, &d, Some(&m), Some(&ics)).unwrap().formatted()).unwrap();
    assert!(msg.contains("multipart/alternative") && msg.contains("text/calendar") && msg.contains("method=REPLY"), "{msg}");
    assert!(msg.contains("In-Reply-To: <inv-mail@example.com>"), "{msg}");
    // Plain mail stays plain.
    let plain = String::from_utf8(build_with(&cfg, &d, None, None).unwrap().formatted()).unwrap();
    assert!(!plain.contains("text/calendar"));
}

#[test]
fn a_thread_is_the_mail_it_answers_and_answers_to_it() {
    struct Plain;
    impl crate::crypto::Protector for Plain {
        fn protect(&self, d: &[u8]) -> Result<Vec<u8>, String> {
            Ok(d.to_vec())
        }
        fn unprotect(&self, d: &[u8]) -> Result<Vec<u8>, String> {
            Ok(d.to_vec())
        }
    }
    let dir = std::env::temp_dir().join(format!("sulcusai-mail-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let c = crate::crypto::Vault::open(&dir.join("keys.json"), Box::new(Plain)).unwrap().cipher().unwrap();
    let conn = db::open(&dir.join("t.db")).unwrap();
    store_account(&conn, &c, "acc", &AccountConfig { email: "me@example.org".into(), ..Default::default() }).unwrap();
    let msg = |id: &str, irt: &str, refs: &[&str], subject: &str| MailData {
        message_id: id.into(),
        in_reply_to: irt.into(),
        references: refs.iter().map(|r| r.to_string()).collect(),
        subject: subject.into(),
        ..Default::default()
    };
    save_item(&conn, &c, "acc", "INBOX", 1, 100, true, &msg("a@x", "", &[], "Trip")).unwrap();
    save_item(&conn, &c, "acc", "Sent", 1, 200, true, &msg("b@x", "a@x", &["a@x"], "Re: Trip")).unwrap();
    save_item(&conn, &c, "acc", "INBOX", 2, 300, false, &msg("c@x", "b@x", &["a@x", "b@x"], "Re: Trip")).unwrap();
    save_item(&conn, &c, "acc", "INBOX", 3, 250, true, &msg("z@x", "", &[], "Other")).unwrap();
    let all = list(&conn, &c, "", 10);
    let last = all.iter().find(|m| m.data.message_id == "c@x").unwrap();
    let subjects: Vec<String> = thread(&conn, &c, last).into_iter().map(|m| m.data.message_id).collect();
    assert_eq!(subjects, vec!["a@x", "b@x"], "oldest first, the unrelated one left out");
    assert!(thread(&conn, &c, all.iter().find(|m| m.data.message_id == "z@x").unwrap()).is_empty());
}

#[tokio::test]
async fn plain_connections_only_to_this_pc() {
    let cfg = AccountConfig { imap_host: "mail.example.com".into(), imap_port: 143, imap_security: Security::Plain, ..Default::default() };
    assert!(imap_login(&cfg).await.unwrap_err().contains("only allowed to this PC"));
}
