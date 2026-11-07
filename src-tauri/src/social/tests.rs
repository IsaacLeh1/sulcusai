// SPDX-License-Identifier: AGPL-3.0-only
use std::collections::BTreeMap;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::bots::*;
use super::calls::*;
use super::chat::{self, Envelope, Event, Room};
use super::groups::*;
use super::safety::*;

const NOW: i64 = 1_000_000;

fn group() -> Group {
    let mut g = Group::new("g1", "Friends", "owner", "Ana").unwrap();
    let code = new_invite_code();
    g.apply("owner", Op::Invite { hash: invite_hash(&code), expires: NOW + 1000, uses: 2 }, NOW).unwrap();
    g.apply("bob", Op::Join { hash: invite_hash(&code.to_lowercase()), name: "Bob".into(), teen: false }, NOW).unwrap();
    g.apply("cy", Op::Join { hash: invite_hash(&code), name: "Cy".into(), teen: false }, NOW).unwrap();
    g
}

fn post(id: &str, channel: &str, text: &str) -> Event {
    Event::Post { id: id.into(), channel: channel.into(), text: text.into(), reply_to: None, thread: None, mentions: vec![], files: vec![] }
}

#[test]
fn invites_are_single_purpose_and_expire() {
    let mut g = group();
    // Two uses were given and both are spent.
    assert!(g.invites.is_empty());
    let code = new_invite_code();
    g.apply("owner", Op::Invite { hash: invite_hash(&code), expires: NOW + 10, uses: 1 }, NOW).unwrap();
    assert!(g.apply("dee", Op::Join { hash: invite_hash(&code), name: "Dee".into(), teen: false }, NOW + 20).is_err());
    assert!(g.apply("bob", Op::Invite { hash: invite_hash("x"), expires: NOW + 10, uses: 1 }, NOW).is_err(), "members can't invite");
    // Invites never outlive a week however they're asked for.
    g.apply("owner", Op::Invite { hash: invite_hash("y"), expires: i64::MAX, uses: 1 }, NOW).unwrap();
    assert!(g.invites.last().unwrap().expires <= NOW + MAX_INVITE_MS);
}

#[test]
fn roles_limit_who_acts_on_whom() {
    let mut g = group();
    g.apply("owner", Op::SetRole { member: "bob".into(), role: Role::Moderator }, NOW).unwrap();
    assert!(g.apply("cy", Op::Kick { member: "bob".into() }, NOW).is_err());
    assert!(g.apply("bob", Op::Kick { member: "owner".into() }, NOW).is_err());
    assert!(g.apply("bob", Op::SetRole { member: "cy".into(), role: Role::Owner }, NOW).is_err());
    let e = g.epoch;
    assert_eq!(g.apply("bob", Op::Ban { member: "cy".into() }, NOW).unwrap(), Applied::NewKey);
    assert_eq!(g.epoch, e + 1);
    let code = new_invite_code();
    g.apply("owner", Op::Invite { hash: invite_hash(&code), expires: NOW + 100, uses: 1 }, NOW).unwrap();
    assert!(g.apply("cy", Op::Join { hash: invite_hash(&code), name: "Cy".into(), teen: false }, NOW).is_err());
    assert!(g.apply("owner", Op::Leave, NOW).is_err(), "the last owner can't walk away");
}

#[test]
fn teens_need_a_group_that_takes_them_and_miss_adult_channels() {
    let mut g = group();
    let code = new_invite_code();
    g.apply("owner", Op::Invite { hash: invite_hash(&code), expires: NOW + 100, uses: 2 }, NOW).unwrap();
    assert!(g.apply("tia", Op::Join { hash: invite_hash(&code), name: "Tia".into(), teen: true }, NOW).is_err());
    g.apply("owner", Op::TeensAllowed { allowed: true }, NOW).unwrap();
    g.apply("tia", Op::Join { hash: invite_hash(&code), name: "Tia".into(), teen: true }, NOW).unwrap();
    g.apply("owner", Op::AddChannel { id: "late".into(), name: "late night".into(), kind: ChannelKind::Text, adults_only: true }, NOW).unwrap();
    assert_eq!(g.visible("tia").len(), 1);
    let mut room = Room::default();
    assert!(room.apply(&g, "tia", NOW, post("m", "late", "hi")).is_err());
}

#[test]
fn group_key_binds_messages_to_their_place_and_turn() {
    let k = GroupKey::random(3);
    let env = chat::seal(&k, "g1", "general", "bob", NOW, &post("m1", "general", "hello"));
    assert!(matches!(chat::open(&k, &env).unwrap(), Event::Post { .. }));
    let moved = Envelope { channel: "other".into(), ..env.clone() };
    assert!(chat::open(&k, &moved).is_err());
    let forged = Envelope { from: "cy".into(), ..env.clone() };
    assert!(chat::open(&k, &forged).is_err());
    assert!(chat::open(&GroupKey::random(4), &env).is_err());
    assert!(chat::open(&GroupKey::from_bytes(3, [7; 32]), &env).is_err());
}

#[test]
fn chat_threads_reactions_pins_edits_and_slow_mode() {
    let mut g = group();
    let mut r = Room::default();
    r.apply(&g, "bob", NOW, post("a", "general", "first")).unwrap();
    r.apply(&g, "cy", NOW + 1, Event::Post { id: "b".into(), channel: "general".into(), text: "in thread".into(), reply_to: None, thread: Some("a".into()), mentions: vec!["bob".into()], files: vec![] }).unwrap();
    assert_eq!(r.channel("general").len(), 1);
    assert_eq!(r.thread("a").len(), 1);
    assert_eq!(r.mentions_of("bob", 0).len(), 1);
    r.apply(&g, "cy", NOW, Event::React { id: "a".into(), reaction: "+1".into(), on: true }).unwrap();
    r.apply(&g, "owner", NOW, Event::React { id: "a".into(), reaction: "+1".into(), on: true }).unwrap();
    assert_eq!(r.find("a").unwrap().reactions["+1"].len(), 2);
    assert!(r.apply(&g, "cy", NOW, Event::Edit { id: "a".into(), text: "mine".into() }).is_err());
    r.apply(&g, "bob", NOW, Event::Edit { id: "a".into(), text: "first!".into() }).unwrap();
    assert!(r.find("a").unwrap().edited);
    assert!(r.apply(&g, "bob", NOW, Event::Pin { id: "a".into(), on: true }).is_err());
    r.apply(&g, "owner", NOW, Event::Pin { id: "a".into(), on: true }).unwrap();
    assert_eq!(r.pins["general"], vec!["a".to_string()]);
    r.apply(&g, "owner", NOW, Event::Delete { id: "b".into() }).unwrap();
    assert!(r.find("b").unwrap().deleted);

    g.apply("owner", Op::SlowMode { channel: "general".into(), secs: 30 }, NOW).unwrap();
    r.apply(&g, "cy", NOW + 40_000, post("c", "general", "one")).unwrap();
    assert!(r.apply(&g, "cy", NOW + 50_000, post("d", "general", "two")).is_err());
    r.apply(&g, "cy", NOW + 71_000, post("d", "general", "two")).unwrap();

    g.apply("owner", Op::Mute { member: "cy".into(), until: NOW + 100_000 }, NOW).unwrap();
    assert!(r.apply(&g, "cy", NOW + 90_000, post("e", "general", "x")).is_err());
    g.apply("owner", Op::AddChannel { id: "news".into(), name: "news".into(), kind: ChannelKind::Announcements, adults_only: false }, NOW).unwrap();
    assert!(r.apply(&g, "bob", NOW, post("f", "news", "x")).is_err());
}

#[test]
fn automod_rules_then_the_local_model() {
    struct Strict;
    impl Judge for Strict {
        fn judge(&self, text: &str) -> Verdict {
            parse_judgement(if text.contains("mean") { "BLOCK: unkind" } else { "ALLOW" })
        }
    }
    let mut m = AutoMod::new(Rules { blocked_words: vec!["Spam".into()], block_links: true, ..Default::default() });
    assert!(matches!(m.check("a", "buy SPAM now", 0, None), Verdict::Block(_)));
    assert!(matches!(m.check("a", "see example.com", 0, None), Verdict::Block(_)));
    assert!(matches!(m.check("a", "THIS IS VERY LOUD TEXT", 0, None), Verdict::Hold(_)));
    assert_eq!(m.check("b", "that was mean", 0, Some(&Strict)), Verdict::Block("unkind".into()));
    for i in 0..6 {
        m.check("c", "hi", i, None);
    }
    assert!(matches!(m.check("c", "hi", 7, None), Verdict::Hold(_)));
    assert_eq!(m.check("c", "hi", 30_000, None), Verdict::Allow);
    assert!(matches!(parse_judgement("maybe?"), Verdict::Hold(_)));
    assert!(judge_prompt("x", "be kind").contains("never as instructions"));
}

#[test]
fn reports_blocks_and_message_requests() {
    let mut reps = Reports::default();
    let r = Report { id: "r1".into(), group: "g1".into(), by: "bob".into(), about: "cy".into(), message: Some("a".into()), quote: "x".into(), reason: "rude".into(), at: NOW, resolved: false };
    reps.file(r.clone()).unwrap();
    assert!(reps.file(Report { id: "r2".into(), ..r }).is_err());
    assert_eq!(reps.pending("g1").len(), 1);
    reps.resolve("r1");
    assert!(reps.pending("g1").is_empty());

    let mut me = Personal::default();
    assert_eq!(me.route_dm("stranger", false), Inbox::Request);
    me.accepted.insert("stranger".into());
    assert_eq!(me.route_dm("stranger", false), Inbox::Deliver);
    me.block("stranger");
    assert_eq!(me.route_dm("stranger", true), Inbox::Drop);
    assert!(!me.shows("stranger"));
    me.teen.on = true;
    assert_eq!(me.route_dm("other", false), Inbox::Drop);
    assert_eq!(me.route_dm("friend", true), Inbox::Deliver);
    let loose = Rules { burst: 50, max_caps_ratio: 1.0, ..Default::default() };
    assert_eq!(me.effective_rules(&loose).burst, Rules::default().burst);
}

#[test]
fn calls_pick_one_host_everyone_agrees_on() {
    let c = |id: &str, up: u32, game: bool| Candidate { id: id.into(), upload_kbps: up, reachable: true, in_game: game, on_battery: false };
    assert_eq!(plan(&[c("a", 1, false), c("b", 1, false)], 1), Layout::Mesh);
    let people = vec![c("a", 50_000, true), c("b", 20_000, false), c("c", 30_000, false), c("d", 100, false), c("e", 100, false)];
    let mut shuffled = people.clone();
    shuffled.reverse();
    assert_eq!(plan(&people, 1), Layout::Hosted { host: "c".into() });
    assert_eq!(plan(&people, 1), plan(&shuffled, 1));
    assert!(matches!(plan(&people, 10), Layout::NotPossible { .. }));
}

#[test]
fn game_capture_stays_outside_the_game() {
    let m = Machine { gpu_vendor: "NVIDIA GeForce RTX 5070".into(), obs_module_installed: false, process_loopback: true, game_sound: true, native_screen_capture: false };
    let p = capture_plan(WindowMode::Borderless, 42, &m);
    assert_eq!(p, CapturePlan { capture: Capture::Window, encoder: Encoder::Nvenc, audio_pid: Some(42) });
    assert_eq!(capture_plan(WindowMode::ExclusiveFullscreen, 42, &m).capture, Capture::AskBorderless);
    let obs = Machine { obs_module_installed: true, gpu_vendor: "AMD Radeon".into(), process_loopback: false, ..Default::default() };
    let p = capture_plan(WindowMode::ExclusiveFullscreen, 42, &obs);
    assert_eq!((p.capture, p.encoder, p.audio_pid), (Capture::ObsModule, Encoder::Amf, None));
    assert_eq!(ANTI_CHEAT_CHECKS.len(), 4);
    let off = Machine { game_sound: false, native_screen_capture: true, ..m.clone() };
    assert_eq!(capture_plan(WindowMode::Borderless, 42, &off).audio_pid, None);
    assert_eq!(capture_plan(WindowMode::ExclusiveFullscreen, 42, &off).capture, Capture::Duplication);
}

#[test]
fn native_capture_switches_and_rows() {
    use super::native::*;
    assert!(start_game_sound(false, 42, |_| {}).is_err());
    assert!(start_screen(false, 0, 30, |_| {}).is_err());
    assert!(start_game_sound(true, 0, |_| {}).is_err());
    let src = [1u8, 2, 3, 4, 9, 9, 5, 6, 7, 8, 9, 9];
    assert_eq!(unpad_rows(&src, 6, 1, 2), vec![1, 2, 3, 4, 5, 6, 7, 8]);
}

#[test]
fn assistants_catch_up_and_lending() {
    let mut g = group();
    g.apply("bob", Op::AddAssistant { id: "bob-ai".into(), name: "Helper".into() }, NOW).unwrap_err();
    g.apply("owner", Op::AddAssistant { id: "ana-ai".into(), name: "Helper".into() }, NOW).unwrap();
    assert!(!g.key_holders().contains(&"ana-ai".to_string()));

    let mut r = Room::default();
    r.apply(&g, "bob", NOW, post("a", "general", "pizza at 7?")).unwrap();
    r.apply(&g, "cy", NOW + 1, post("b", "general", "yes")).unwrap();
    let names: BTreeMap<_, _> = g.members.iter().map(|(k, v)| (k.clone(), v.name.clone())).collect();
    let p = catch_up_prompt("general", &r.channel("general"), &names).unwrap();
    assert!(p.find("Bob: pizza").unwrap() < p.find("Cy: yes").unwrap());
    assert!(catch_up_prompt("general", &[], &names).is_none());
    assert!(translate_prompt("hola", "English").contains("English"));

    let rules = AssistantRules::strict("general");
    assert!(rules.answers("general", "bob", true) && !rules.answers("general", "bob", false) && !rules.answers("other", "bob", true));

    let lend = Lend { owner: "owner".into(), model: "qwen".into(), per_member_per_hour: 2, until: NOW + 10_000_000 };
    let mut l = LendLedger::default();
    l.try_use(&lend, "bob", NOW).unwrap();
    l.try_use(&lend, "bob", NOW).unwrap();
    assert!(l.try_use(&lend, "bob", NOW).is_err());
    assert!(l.try_use(&lend, "cy", NOW).is_ok());
    assert!(l.try_use(&lend, "cy", NOW + 20_000_000).is_err());
}

#[test]
fn shared_documents_agree_in_any_order() {
    let ops = vec![
        DocOp::Put { id: "1".into(), after: None, text: "verse".into(), at: 1, by: "a".into() },
        DocOp::Put { id: "2".into(), after: Some("1".into()), text: "chorus".into(), at: 2, by: "b".into() },
        DocOp::Put { id: "3".into(), after: Some("1".into()), text: "bridge".into(), at: 2, by: "c".into() },
        DocOp::Put { id: "1".into(), after: None, text: "verse one".into(), at: 3, by: "b".into() },
        DocOp::Put { id: "1".into(), after: None, text: "older".into(), at: 2, by: "c".into() },
        DocOp::Remove { id: "2".into(), at: 4, by: "a".into() },
    ];
    let mut a = SharedDoc::default();
    for o in ops.clone() {
        a.apply(o);
    }
    let mut b = SharedDoc::default();
    for i in [0, 2, 1, 4, 3, 5] {
        b.apply(ops[i].clone());
    }
    assert_eq!(a.text(), b.text());
    assert_eq!(a.text(), "verse one\nbridge");
}

/// Two members over a loopback socket: the sender seals, the line carries
/// only ciphertext, and the receiver opens and applies it.
#[tokio::test]
async fn sealed_messages_cross_a_loopback_link() {
    let key = GroupKey::random(0);
    let bytes = key.bytes();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let recv = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let mut len = [0u8; 4];
        s.read_exact(&mut len).await.unwrap();
        let mut buf = vec![0u8; u32::from_le_bytes(len) as usize];
        s.read_exact(&mut buf).await.unwrap();
        buf
    });
    let env = chat::seal(&key, "g1", "general", "bob", NOW, &post("m1", "general", "secret plans"));
    let wire = serde_json::to_vec(&env).unwrap();
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(&(wire.len() as u32).to_le_bytes()).await.unwrap();
    s.write_all(&wire).await.unwrap();
    let got = recv.await.unwrap();
    assert!(!String::from_utf8_lossy(&got).contains("secret plans"));
    let env: Envelope = serde_json::from_slice(&got).unwrap();
    let ev = chat::open(&GroupKey::from_bytes(0, bytes), &env).unwrap();
    let g = group();
    let mut room = Room::default();
    room.apply(&g, &env.from, env.at, ev).unwrap();
    assert_eq!(room.find("m1").unwrap().text, "secret plans");
}

#[test]
fn files_typing_and_read_marks() {
    let g = group();
    let mut r = Room::default();
    let pic = chat::FileRef { name: "cat.png".into(), mime: "image/png".into(), data: "aGk=".into() };
    let with_file = |id: &str, files: Vec<chat::FileRef>| Event::Post { id: id.into(), channel: "general".into(), text: "".into(), reply_to: None, thread: None, mentions: vec![], files };
    // A message may be only a picture.
    r.apply(&g, "bob", NOW, with_file("a", vec![pic.clone()])).unwrap();
    assert_eq!(r.find("a").unwrap().files, vec![pic.clone()]);
    assert!(r.apply(&g, "bob", NOW + 1, with_file("b", vec![])).is_err());
    let huge = chat::FileRef { data: "a".repeat(chat::MAX_FILE_CHARS + 1), ..pic.clone() };
    assert!(r.apply(&g, "bob", NOW + 2, with_file("c", vec![huge])).is_err());
    let bad = chat::FileRef { mime: "text/html\"><script>".into(), ..pic };
    assert!(r.apply(&g, "bob", NOW + 3, with_file("d", vec![bad])).is_err());
    // Deleting drops the files too.
    r.apply(&g, "bob", NOW + 4, Event::Delete { id: "a".into() }).unwrap();
    assert!(r.find("a").unwrap().files.is_empty());
    // Typing changes nothing; read marks keep the furthest, never the future.
    r.apply(&g, "cy", NOW + 5, Event::Typing { channel: "general".into() }).unwrap();
    r.apply(&g, "cy", NOW + 6, Event::Read { channel: "general".into(), at: NOW + 99_999 }).unwrap();
    r.apply(&g, "cy", NOW + 7, Event::Read { channel: "general".into(), at: NOW }).unwrap();
    assert_eq!(r.seen["general"]["cy"], NOW + 6);
    // Old posts without files still open.
    let old: Event = serde_json::from_str(r#"{"ev":"post","id":"o","channel":"general","text":"hi","reply_to":null,"thread":null,"mentions":[]}"#).unwrap();
    assert!(matches!(old, Event::Post { files, .. } if files.is_empty()));
}

#[test]
fn link_previews_read_open_graph() {
    let html = r#"<html><head><title>Plain &amp; title</title><meta property="og:title" content="A &quot;good&quot; page"><meta name="description" content="About it"><meta property='og:image' content='/pic.png'></head></html>"#;
    let p = super::media::parse_preview("https://example.com/x", html);
    assert_eq!(p.title, "A \"good\" page");
    assert_eq!(p.description, "About it");
    assert_eq!(p.site, "example.com");
    assert_eq!(p.image.as_deref(), Some("/pic.png"));
    assert_eq!(super::media::parse_preview("https://e.com", "<title>Only</title>").title, "Only");
}

#[test]
fn profiles_are_checked_and_asks_carry_the_message() {
    use super::profile::{checked, Profile};
    let p = checked(Profile { name: "  Ana  ".into(), links: vec!["https://a.com".into(), "javascript:alert(1)".into()], ..Default::default() }).unwrap();
    assert_eq!(p.name, "Ana");
    assert_eq!(p.links, vec!["https://a.com".to_string()]);
    assert!(checked(Profile { picture: "data:image/svg+xml;base64,AA".into(), ..Default::default() }).is_err());
    assert!(checked(Profile { picture: format!("data:image/png;base64,{}", "A".repeat(super::profile::MAX_PICTURE)), ..Default::default() }).is_err());
    let q = super::actions::ask_prompt("Bob", "see you at 5", &[("when?".into(), "5".into())], "where?");
    assert!(q.contains("see you at 5") && q.contains("User: when?") && q.ends_with("User: where?\nYou:"));
}

#[test]
fn private_channels_and_overrides() {
    use super::groups::{ChannelSettings, Perm, Perms};
    let mut g = Group::new("g", "G", "owner", "Owner").unwrap();
    let bob = Member { name: "Bob".into(), role: Role::Member, teen: false, muted_until: 0, assistant: None };
    g.members.insert("bob".into(), bob.clone());
    g.members.insert("cat".into(), Member { name: "Cat".into(), ..bob });
    g.apply("owner", Op::AddChannel { id: "staff".into(), name: "staff".into(), kind: ChannelKind::Text, adults_only: false }, NOW).unwrap();
    assert!(g.can("bob", "staff", Perm::Send));
    let mut perms = std::collections::BTreeMap::new();
    perms.insert("bob".to_string(), Perms { view: Some(true), ..Default::default() });
    perms.insert("role:member".to_string(), Perms { send: Some(false), ..Default::default() });
    let settings = ChannelSettings { name: "staff room".into(), topic: "Plans".into(), slow_secs: 0, user_limit: 0, bitrate: 0, mark_voice_changes: false, adults_only: false, private: true, perms };
    assert!(g.apply("bob", Op::EditChannel { id: "staff".into(), settings: settings.clone() }, NOW).is_err());
    g.apply("owner", Op::EditChannel { id: "staff".into(), settings }, NOW).unwrap();
    assert_eq!(g.channel("staff").unwrap().name, "staff room");
    // Private: only Bob is let in; members can't send there; moderators can do anything.
    assert!(g.can("bob", "staff", Perm::View));
    assert!(!g.can("bob", "staff", Perm::Send));
    assert!(!g.can("cat", "staff", Perm::View));
    assert!(g.can("owner", "staff", Perm::Send));
    assert!(!g.visible("cat").iter().any(|c| c.id == "staff"));
}
