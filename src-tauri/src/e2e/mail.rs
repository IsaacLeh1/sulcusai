// SPDX-License-Identifier: AGPL-3.0-only
//! Mail and calendar with a real model.

use super::*;

/// Tasks set off by new mail and by an event about to start (Qwen3 4B 2507
/// as the model): only the matching mail sets its task off, each task's
/// reply is about what set it off, and neither goes off twice.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_mail_and_events_set_off_tasks() {
    use crate::schedule::{self, Cadence, Schedule};
    let state = media_state();
    let c = state.work_cipher().unwrap();
    let real = Paths::new(app_data_dir()).unwrap();
    let file = real.models.join("qwen3-4b-2507").join("Qwen3-4B-Instruct-2507-Q4_K_M.gguf");
    crate::local::import(&state, &file).unwrap();
    let id = crate::local::list(&state.db.lock().unwrap()).into_iter().find(|m| m.local.as_ref().is_some_and(|l| std::path::Path::new(&l.path) == file)).unwrap().id;
    let task = |sid: &str, name: &str, prompt: &str, cadence: Cadence| {
        let mut s = Schedule { id: sid.into(), name: name.into(), prompt: prompt.into(), cadence, model_id: Some(id.clone()), project_id: None, allow_changes: false, actions: Vec::new(), enabled: true, last_run: None, next_run: None, last_chat: None, armed: None, handled: Vec::new() };
        schedule::arm(&mut s, None);
        schedule::save(&state.db.lock().unwrap(), &c, &s).unwrap();
    };
    task("mail", "Invoices", "Say who the new mail is from and what it's about, in one short sentence.", Cadence::OnMail { from: "dana".into(), subject: "invoice".into() });
    task("event", "Before meetings", "Say which meeting is about to start and where, in one short sentence.", Cadence::OnEvent { minutes: 15, title: String::new() });
    let (mut known, mut started) = (schedule::Known::new(), true);
    assert!(schedule::triggered(&state, &c, &mut known, &mut started).is_empty(), "nothing has arrived yet");

    let now = crate::db::now_ms();
    {
        let conn = state.db.lock().unwrap();
        crate::mail::store_account(&conn, &c, "acc", &crate::mail::AccountConfig { name: "Me".into(), email: "me@example.org".into(), ..Default::default() }).unwrap();
        let mail = |from: &str, name: &str, subject: &str, body: &str| crate::mail::MailData { from: from.into(), from_name: name.into(), to: vec!["me@example.org".into()], subject: subject.into(), snippet: body.into(), body: body.into(), ..Default::default() };
        crate::mail::save_item(&conn, &c, "acc", "INBOX", 1, now, false, &mail("dana@example.com", "Dana Ruiz", "Invoice for October", "Hi, the October invoice for the garden work is attached: $420, due November 10.")).unwrap();
        crate::mail::save_item(&conn, &c, "acc", "INBOX", 2, now, false, &mail("sam@example.com", "Sam", "Lunch on Friday?", "Want to get lunch on Friday?")).unwrap();
    }
    let event = crate::calendar::NewEvent { title: "Budget review".into(), start: now + 10 * 60_000, end: now + 40 * 60_000, all_day: false, location: "Room 4B".into(), notes: String::new(), attendees: Vec::new(), calendar: None };
    crate::calendar::create(&state, &c, event).await.unwrap();

    let fired = schedule::triggered(&state, &c, &mut known, &mut started);
    assert_eq!(fired.len(), 2, "one for the mail, one for the event");
    for (t, f) in &fired {
        let f = f.as_ref().unwrap();
        println!("{}: {}", t.name, f.what);
        let (chat, result) = schedule::run_with_cause(&state, t, Some(&f.what), std::sync::Arc::new(|_: &str, _: serde_json::Value| {})).await.unwrap();
        result.unwrap();
        let said = crate::db::messages(&state.db.lock().unwrap(), &c, &chat).into_iter().rev().find(|m| m.role == "assistant").map(|m| m.content).unwrap_or_default();
        println!("it said: {said}");
        let low = said.to_lowercase();
        if t.id == "mail" {
            assert!(!f.what.contains("Lunch"), "Sam's mail doesn't match");
            assert!(low.contains("dana") && low.contains("invoice"), "{said}");
        } else {
            assert!(low.contains("budget"), "{said}");
        }
        let mut done = t.clone();
        schedule::remember(&mut done, f.ids.clone());
        schedule::save(&state.db.lock().unwrap(), &c, &done).unwrap();
    }
    assert!(schedule::triggered(&state, &c, &mut known, &mut started).is_empty(), "not again for the same mail or event");
    state.engine.lock().await.stop().await;
}

/// Email and calendar with the real model, against the local test servers
/// (pymap on 11430, the SMTP script on 10250). Approvals are answered Allow,
/// as the user would, and recorded.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_email_and_calendar() {
    let (state, ep, cipher) = agent_harness().await;
    {
        let conn = state.db.lock().unwrap();
        crate::db::update_settings(&conn, |s| s.connectivity = crate::net::Connectivity::Web).unwrap();
        let cfg = crate::mail::AccountConfig {
            name: "Demo".into(),
            email: "demo@example.org".into(),
            username: "demouser".into(),
            password: "demopass".into(),
            imap_host: "127.0.0.1".into(),
            imap_port: 11430,
            imap_security: crate::mail::Security::Plain,
            smtp_host: "127.0.0.1".into(),
            smtp_port: 10250,
            smtp_security: crate::mail::Security::Plain,
            ..Default::default()
        };
        crate::mail::store_account(&conn, &cipher, "acc", &cfg).unwrap();
    }
    let n = crate::mail::sync_account(&state, &cipher, "acc").await.unwrap();
    println!("synced {n} messages");

    let approved = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let (st, log) = (state.clone(), approved.clone());
    let approver = tokio::spawn(async move {
        loop {
            for p in st.approvals.waiting() {
                log.lock().unwrap().push(format!("{} [{:?}]", p.preview.title, p.risk));
                st.approvals.answer(&p.call_id, crate::agent::Decision::Allow);
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    });

    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, Some("qwen3-1.7b".into())).unwrap();
    let r = say(&state, &ep, &cipher, &chat.id, "Did I get an email about an important notice? Who sent it? /no_think").await;
    println!("mail answer: {r}");
    assert!(r.to_lowercase().contains("corp@example.com"), "{r}");

    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, Some("qwen3-1.7b".into())).unwrap();
    let r = say(&state, &ep, &cipher, &chat.id, "Put lunch with Bo on my calendar for friday at noon. /no_think").await;
    println!("calendar answer: {r}");
    let now = crate::db::now_ms();
    let events = crate::calendar::events(&state.db.lock().unwrap(), &cipher, now - 86_400_000, now + 8 * 86_400_000);
    println!("events: {:?}", events.iter().map(|e| (&e.data.title, e.start)).collect::<Vec<_>>());
    assert!(events.iter().any(|e| e.data.title.to_lowercase().contains("lunch")));

    let r = say(&state, &ep, &cipher, &chat.id, "When am I free on friday for an hour? /no_think").await;
    println!("free answer: {r}");

    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, Some("qwen3-1.7b".into())).unwrap();
    let r = say(&state, &ep, &cipher, &chat.id, "Email ana@example.com and tell her the budget review moved to Friday at 2pm. Sign it Demo. /no_think").await;
    println!("send answer: {r}");
    approver.abort();
    let log = approved.lock().unwrap().clone();
    println!("approvals: {log:#?}");
    assert!(log.iter().any(|l| l.contains("Send an email to ana@example.com") && l.contains("Submit")), "sending asked first");
    assert!(log.iter().any(|l| l.contains("Lunch")), "adding the event asked first");
    assert!(!log.iter().any(|l| l.contains("invite")), "no made-up invitations");
    state.engine.lock().await.stop().await;
}

/// A test mailbox: a shipping notice, two newsletters, a security alert, a
/// party invitation, reminders, mail from Mom and 22 store promotions. Mail
/// older than 4 hours is read unless `all_unread`.
fn seed_test_mail(state: &std::sync::Arc<crate::AppState>, cipher: &crate::crypto::Cipher, all_unread: bool) {
    let now = crate::db::now_ms();
    let hour = 3_600_000;
    {
        let conn = state.db.lock().unwrap();
        let cfg = crate::mail::AccountConfig { name: "Isaac".into(), email: "isaac@example.org".into(), ..Default::default() };
        crate::mail::store_account(&conn, cipher, "acc", &cfg).unwrap();
        let long = "Read more about this week's stories. ".repeat(600);
        let mails: Vec<(&str, &str, &str, String, i64)> = vec![
            ("deals@amazon.com", "Amazon", "Your package has shipped", "Your order of a USB-C cable has shipped and arrives Thursday.".into(), 1),
            ("news@nytimes.com", "The New York Times", "The Morning: the week ahead", long.clone(), 2),
            ("no-reply@accounts.google.com", "Google", "Security alert", "SulcusAI was granted access to your Google Account isaac@example.org. If you did not grant access, you should check this activity and secure your account. Check activity: https://myaccount.google.com/notifications".into(), 3),
            ("sam@example.org", "Sam Rivera", "Party tonight!", "Hey! My birthday party is tonight at 6pm at my place, 42 Elm Street. Bring snacks.".into(), 5),
            ("calendar-notification@google.com", "Google Calendar", "Reminder: Dentist @ Wed", "This is a reminder for Dentist on Wednesday at 9am.".into(), 26),
            ("newsletter@medium.com", "Medium Daily Digest", "Stories for you", long.clone(), 30),
            ("no-reply@accounts.google.com", "Google", "Your Google Account storage is almost full", "You've used 14 GB of your 15 GB of Google storage.".into(), 50),
            ("mom@example.org", "Mom", "Sunday dinner", "Are you coming Sunday? Dad is making lasagna.".into(), 52),
            // Marketing that sounds urgent, next to real deadlines.
            ("deals@lenovo.example", "Lenovo US", "Last Chance for Your Extra 10% Off", "Ends tonight at midnight! Use code EXTRA10 on laptops and monitors.".into(), 8),
            ("giveaway@717supply.example", "717 SUPPLY", "FINAL HOURS: Your Entries Count TWICE", "Enter now to win a Tahoe, a Corvette or $75,000. Final hours, don't miss out!".into(), 9),
            ("quotes@libertymutual.example", "Liberty Mutual", "Your quote is patiently waiting, Isaac.", "Customized coverage so you only pay for what you need. Finish your quote today.".into(), 10),
            ("rewards@dutchbros.example", "Dutch Bros Coffee", "2X POINTS today only", "Earn double points on every drink today only. Open the app to activate.".into(), 11),
            ("billing@rockymountainpower.example", "Rocky Mountain Power", "Your bill of $84.20 is due Oct 20", "Your October statement is ready. Amount due: $84.20 by October 20. Pay online or set up autopay.".into(), 12),
            ("noreply@delta.example", "Delta", "Check in for your flight to Denver tomorrow", "Your flight DL1234 departs SLC tomorrow at 8:05 AM. Check in now to get your boarding pass.".into(), 13),
        ];
        let mut all = mails;
        for i in 0..22 {
            all.push(("promo@store.example", "Store", "Weekly sale", format!("Sale number {i}: everything 20% off. {}", "Shop now. ".repeat(200)), 60 + i * 5));
        }
        for (uid, (from, name, subject, body, hours_ago)) in all.into_iter().enumerate() {
            let data = crate::mail::MailData { from: from.into(), from_name: name.into(), to: vec!["isaac@example.org".into()], subject: subject.into(), snippet: body.chars().take(160).collect(), body, ..Default::default() };
            crate::mail::save_item(&conn, cipher, "acc", "INBOX", uid as u32 + 1, now - hours_ago * hour, !all_unread && hours_ago > 4, &data).unwrap();
        }
    }
}

/// Sorting unread mail with a real model (SULCUSAI_TEST_GGUF and friends, as
/// above): promotions and newsletters are marked read, people and alerts stay.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_mail_triage_on_model() {
    let ctx = std::env::var("SULCUSAI_TEST_CTX").ok().and_then(|v| v.parse().ok()).unwrap_or(8192);
    let layers = std::env::var("SULCUSAI_TEST_LAYERS").ok().and_then(|v| v.parse().ok()).unwrap_or(99);
    let running = std::env::var("SULCUSAI_TEST_PORT").ok().and_then(|p| p.parse().ok()).map(|port| engine::Endpoint { port, key: std::env::var("SULCUSAI_TEST_KEY").unwrap_or_default(), ctx, extra: serde_json::Value::Null, cloud: None });
    let blob = std::env::var("SULCUSAI_TEST_GGUF").unwrap_or_default();
    let stop_after = running.is_none();
    let (state, ep, cipher) = agent_harness_on(Some((blob.into(), ctx, layers)), running).await;
    seed_test_mail(&state, &cipher, true);
    let t = std::time::Instant::now();
    let report = crate::triage::run(&state, &cipher, &ep, "test model", false, |done, total| println!("sorted {done} of {total}")).await.unwrap();
    println!("took {:.1} s", t.elapsed().as_secs_f32());
    if stop_after {
        state.engine.lock().await.stop().await;
    }
    for i in &report.items {
        println!("{} | {:<28} | {:<45} | {} ({})", if i.important { "KEEP " } else { "READ " }, i.from_name, i.subject, i.reason, i.next);
    }
    let kept = |name: &str, subject: &str| report.items.iter().any(|i| i.from_name == name && i.subject.contains(subject) && i.important);
    assert_eq!(report.looked_at, 36);
    assert!(kept("Sam Rivera", "Party") && kept("Mom", "Sunday") && kept("Google", "Security"), "people and the security alert stay unread");
    assert!(kept("Rocky Mountain Power", "bill") && kept("Delta", "flight"), "a bill due and a flight are kept");
    let urgent_ads = ["Lenovo US", "717 SUPPLY", "Liberty Mutual", "Dutch Bros Coffee"];
    let ads_cleared = report.items.iter().filter(|i| urgent_ads.contains(&i.from_name.as_str()) && i.marked_read).count();
    println!("urgent-sounding ads cleared: {ads_cleared} of 4");
    assert!(ads_cleared >= 3, "marketing urgency doesn't make an ad important");
    let promos = report.items.iter().filter(|i| i.from_name == "Store").count();
    let cleared = report.items.iter().filter(|i| i.from_name == "Store" && i.marked_read).count();
    println!("promotions cleared: {cleared} of {promos}");
    assert!(cleared * 10 >= promos * 8, "at least 80% of the promotions are marked read");
    let unread_after = crate::mail::list(&state.db.lock().unwrap(), &cipher, "", 100).iter().filter(|m| !m.seen).count();
    assert_eq!(unread_after, report.items.iter().filter(|i| i.important).count(), "exactly the kept ones are still unread");
}

/// Mail and calendar questions the way people ask them, on the model Ollama
/// downloaded (qwen2.5-coder 14B, the one these went wrong on): every tool
/// call is printed with how long each answer took.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_mail_questions_on_ollama_model() {
    // The settings SulcusAI picks for it on a laptop (it doesn't all fit on the graphics card).
    let ctx = std::env::var("SULCUSAI_TEST_CTX").ok().and_then(|v| v.parse().ok()).unwrap_or(8192);
    let layers = std::env::var("SULCUSAI_TEST_LAYERS").ok().and_then(|v| v.parse().ok()).unwrap_or(47);
    // SULCUSAI_TEST_PORT/KEY: use the engine the app already has loaded.
    let running = std::env::var("SULCUSAI_TEST_PORT").ok().and_then(|p| p.parse().ok()).map(|port| engine::Endpoint { port, key: std::env::var("SULCUSAI_TEST_KEY").unwrap_or_default(), ctx, extra: serde_json::Value::Null, cloud: None });
    let blob = std::env::var("SULCUSAI_TEST_GGUF").unwrap_or_default();
    let stop_after = running.is_none();
    let (state, ep, cipher) = agent_harness_on(Some((blob.into(), ctx, layers)), running).await;
    // Approval cards are answered as a person would: yes, except opening a sign-in page.
    let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let (st, log) = (state.clone(), asked.clone());
    let approver = tokio::spawn(async move {
        loop {
            for p in st.approvals.waiting() {
                log.lock().unwrap().push(p.preview.title.clone());
                let yes = !p.preview.title.starts_with("Connect");
                st.approvals.answer(&p.call_id, if yes { crate::agent::Decision::Allow } else { crate::agent::Decision::Deny });
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    });
    let hour = 3_600_000;
    seed_test_mail(&state, &cipher, false);
    let today6 = {
        use chrono::{Local, TimeZone};
        let d = Local::now().date_naive().and_hms_opt(18, 0, 0).unwrap();
        Local.from_local_datetime(&d).earliest().unwrap().timestamp_millis()
    };
    crate::calendar::create(&state, &cipher, crate::calendar::NewEvent { title: "Sam's Birthday Party".into(), start: today6, end: today6 + 3 * hour, all_day: false, location: "42 Elm Street".into(), notes: String::new(), attendees: vec![], calendar: None }).await.unwrap();

    let mut report = Vec::new();
    let questions = ["Summarize my most recent email from Google", "When is sam's party?", "Do I have any unread email?", "What did my mom ask me?", "What email accounts do I have connected?", "Connect my Outlook email too"];
    let only: Option<usize> = std::env::var("SULCUSAI_TEST_ONLY").ok().and_then(|v| v.parse().ok());
    for (i, q) in questions.into_iter().enumerate() {
        if only.is_some_and(|o| o != i) {
            continue;
        }
        let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, None).unwrap();
        let t = std::time::Instant::now();
        let answer = say(&state, &ep, &cipher, &chat.id, q).await;
        let secs = t.elapsed().as_secs_f32();
        let msgs = crate::db::messages(&state.db.lock().unwrap(), &cipher, &chat.id);
        let calls: Vec<String> = msgs.iter().flat_map(|m| m.tool_calls.clone().unwrap_or_default()).map(|c| format!("{}({})", c.name, c.arguments)).collect();
        println!("\n=== {q}\n{secs:.1} s, {} tool calls: {calls:?}\n{answer}", calls.len());
        let used = state.contexts.lock().unwrap().get(&chat.id).map(|c| format!("system {} + tools {} tokens of {}", c.system_tokens + c.profile_tokens, c.tools_tokens, c.ctx)).unwrap_or_default();
        println!("context: {used}");
        report.push((q, secs, calls.len(), answer));
    }
    approver.abort();
    println!("approval cards: {:?}", asked.lock().unwrap());
    if stop_after {
        state.engine.lock().await.stop().await;
    }
    for (q, secs, calls, _) in &report {
        println!("{q}: {secs:.1} s, {calls} calls");
    }
    let low = |i: usize| report[i].3.to_lowercase();
    let mut failed = Vec::new();
    if !(low(0).contains("sulcusai") || low(0).contains("security")) { failed.push("summarize the Google email"); }
    if !(low(1).contains("6") || low(1).contains("six")) { failed.push("find the party time"); }
    if !(low(2).contains("amazon") || low(2).contains("security") || low(2).contains("times")) { failed.push("list unread email"); }
    if !(low(3).contains("sunday") || low(3).contains("dinner") || low(3).contains("lasagna")) { failed.push("read mom's email"); }
    if !low(4).contains("isaac@example.org") { failed.push("say which account is connected"); }
    if !asked.lock().unwrap().iter().any(|t| t.contains("Microsoft")) && !low(5).contains("sign") { failed.push("offer the Outlook sign-in"); }
    if report.iter().any(|r| r.2 > 5) { failed.push("stay under 6 tool calls"); }
    assert!(failed.is_empty(), "didn't: {failed:?}");
}
