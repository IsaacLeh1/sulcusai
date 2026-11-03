// SPDX-License-Identifier: AGPL-3.0-only
use super::*;

#[tokio::test]
#[ignore]
async fn mail_round_trip() {
    let cfg = AccountConfig {
        name: "Demo".into(),
        email: "demo@example.org".into(),
        username: "demouser".into(),
        password: "demopass".into(),
        imap_host: "127.0.0.1".into(),
        imap_port: 11430,
        imap_security: Security::Plain,
        smtp_host: "127.0.0.1".into(),
        smtp_port: 10250,
        smtp_security: Security::Plain,
        ..Default::default()
    };
    let bad = AccountConfig { password: "wrong".into(), ..cfg.clone() };
    let e = imap_login(&bad).await.unwrap_err();
    println!("wrong password: {e}");
    assert!(e.contains("didn't accept"), "{e}");

    let mut s = imap_login(&cfg).await.unwrap();
    let sent = find_sent(&mut s).await;
    println!("sent folder: {sent:?}");
    let (inbox, validity, _) = sync_folder(&mut s, "INBOX", 0, None).await.unwrap();
    println!("inbox: {} messages, validity {validity:?}", inbox.len());
    for m in inbox.iter().take(4) {
        println!("  {} | {} | {} | seen {}", m.data.from, m.data.subject, m.data.snippet.chars().take(50).collect::<String>(), m.seen);
    }
    assert!(!inbox.is_empty());
    let max = inbox.iter().map(|m| m.uid).max().unwrap();
    let (again, _, _) = sync_folder(&mut s, "INBOX", max, validity).await.unwrap();
    assert!(again.is_empty(), "nothing new on the second sync");

    let d = Draft { to: vec!["ana@example.com".into()], subject: "Re: hello".into(), body: "Sounds good.\nThanks!".into(), ..Default::default() };
    let msg = build_with(&cfg, &d, Some(&inbox[0].data), None).unwrap();
    smtp_send(&cfg, &msg).await.unwrap();
    println!("sent over SMTP");
    let sent = sent.expect("demo data has a Sent folder");
    let before = sync_folder(&mut s, &sent, 0, None).await.unwrap().0.len();
    s.append(&sent, Some("(\\Seen)"), None, msg.formatted()).await.unwrap();
    let after = sync_folder(&mut s, &sent, 0, None).await.unwrap().0;
    println!("sent folder: {before} → {}", after.len());
    assert_eq!(after.len(), before + 1);
    assert!(after.iter().any(|m| m.data.subject == "Re: hello" && m.data.body.contains("Sounds good")));
    s.logout().await.unwrap();
}

/// Read, archive and trash done here reach the server, and what the
/// server changed (read on a phone, archived on the web) comes back.
/// Needs pymap on 11430, as above (start it fresh: it changes the demo data).
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn mail_changes_reach_the_server_and_back() {
    let cfg = AccountConfig {
        name: "Demo".into(),
        email: "demo@example.org".into(),
        username: "demouser".into(),
        password: "demopass".into(),
        imap_host: "127.0.0.1".into(),
        imap_port: 11430,
        imap_security: Security::Plain,
        ..Default::default()
    };
    let state = crate::testutil::scratch_state();
    let c = state.cipher().unwrap();
    {
        let conn = state.db.lock().unwrap();
        db::update_settings(&conn, |s| s.connectivity = crate::net::Connectivity::Web).unwrap();
        store_account(&conn, &c, "acc", &cfg).unwrap();
    }
    // Somewhere to archive to and a trash, as Outlook has them (pymap's
    // own Trash is read-only).
    let mut s = imap_login(&cfg).await.unwrap();
    let _ = s.create("Archive").await;
    let _ = s.delete("Trash").await;
    let _ = s.create("Deleted Items").await;
    for n in 0..2 {
        let raw = format!("From: Bo <bo@example.com>\r\nTo: demo@example.org\r\nSubject: Extra {n}\r\nDate: Tue, 13 Oct 2026 09:30:00 -0600\r\n\r\nHello {n}.\r\n");
        s.append("INBOX", None, None, raw.as_bytes()).await.unwrap();
    }
    s.logout().await.unwrap();

    sync_account(&state, &c, "acc").await.unwrap();
    let inbox: Vec<MailItem> = list(&state.db.lock().unwrap(), &c, "", 100).into_iter().filter(|m| m.folder == "INBOX").collect();
    println!("inbox here: {}", inbox.len());
    assert!(inbox.len() >= 5, "demo data has enough mail");
    let uid_of = |id: &str| state.db.lock().unwrap().query_row("SELECT uid FROM mail_messages WHERE id = ?1", [id], |r| r.get::<_, u32>(0)).unwrap();
    let (read, archived, trashed, phone, web) = (&inbox[0], &inbox[1], &inbox[2], &inbox[3], &inbox[4]);
    let (read_uid, archived_uid, trashed_uid, phone_uid, web_uid) = (uid_of(&read.id), uid_of(&archived.id), uid_of(&trashed.id), uid_of(&phone.id), uid_of(&web.id));
    {
        let conn = state.db.lock().unwrap();
        change(&conn, &[read.id.clone()], if read.seen { Change::Unseen } else { Change::Seen }).unwrap();
        change(&conn, &[archived.id.clone()], Change::Archive).unwrap();
        change(&conn, &[trashed.id.clone()], Change::Trash).unwrap();
    }
    flush_pending(&state, &c).await.unwrap();
    for a in db::actions(&state.db.lock().unwrap(), &c, 5) {
        println!("log: {}", a.summary);
    }
    assert_eq!(state.db.lock().unwrap().query_row("SELECT COUNT(*) FROM mail_pending", [], |r| r.get::<_, i64>(0)).unwrap(), 0, "all sent");

    // What the server has now.
    let mut s = imap_login(&cfg).await.unwrap();
    s.select("INBOX").await.unwrap();
    let left: Vec<u32> = s.uid_search("ALL").await.unwrap().into_iter().collect();
    assert!(!left.contains(&archived_uid) && !left.contains(&trashed_uid), "archived and trashed mail left the inbox");
    let fetched: Vec<_> = s.uid_fetch(read_uid.to_string(), "(UID FLAGS)").await.unwrap().try_collect().await.unwrap();
    let seen_there = fetched[0].flags().any(|f| matches!(f, async_imap::types::Flag::Seen));
    assert_eq!(seen_there, !read.seen, "read state reached the server");
    for folder in ["Archive", "Deleted Items"] {
        let mb = s.examine(folder).await.unwrap();
        println!("{folder} on the server: {} messages", mb.exists);
        assert_eq!(mb.exists, 1);
    }
    // Meanwhile, on a phone: one read, one archived.
    s.select("INBOX").await.unwrap();
    let _: Vec<_> = s.uid_store(phone_uid.to_string(), if phone.seen { "-FLAGS (\\Seen)" } else { "+FLAGS (\\Seen)" }).await.unwrap().try_collect().await.unwrap();
    move_to(&mut s, &web_uid.to_string(), "Archive").await.unwrap();
    s.logout().await.unwrap();

    sync_account(&state, &c, "acc").await.unwrap();
    let conn = state.db.lock().unwrap();
    assert_eq!(get(&conn, &c, &phone.id).unwrap().seen, !phone.seen, "read state came back");
    assert_eq!(get(&conn, &c, &web.id).unwrap().folder, ARCHIVE, "archived elsewhere: out of the inbox here");
    assert!(get(&conn, &c, &trashed.id).is_none());
}
