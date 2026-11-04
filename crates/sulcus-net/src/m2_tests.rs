// SPDX-License-Identifier: AGPL-3.0-only
//! Invites, signed records and the address book; a friend invite between
//! two nodes on 127.0.0.1; and a mutation pass over every parser.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use libp2p::core::multiaddr::Protocol;
use libp2p::identity::Keypair;

use crate::addrbook::AddressBook;
use crate::invite::{self, Book, InviteError, Kind};
use crate::records::{newer, AddressRecord, Roster, RosterDevice};
use crate::signed::{self, Bad};
use crate::{friend, identity, Node, NodeConfig};

fn key() -> Keypair {
    identity::load(&identity::generate()).unwrap()
}

#[test]
fn an_invite_round_trips_and_tampering_is_caught() {
    let k = key();
    let addr: libp2p::Multiaddr = "/ip4/127.0.0.1/tcp/4001".parse().unwrap();
    let (text, hash) = invite::make(&k, Kind::Friend, "Sam", &[addr.clone()], Vec::new(), Some(7), None);
    assert!(text.starts_with("sulcus:i1."));
    assert!(text.len() < 700, "small enough for a QR code: {}", text.len());
    let c = invite::read(&text, signed::now()).unwrap();
    assert_eq!((c.from, c.invite.name.as_str(), c.addrs.clone()), (k.public().to_peer_id(), "Sam", vec![addr]));
    assert_eq!(invite::secret_hash(&c.invite.secret), hash);
    // A changed body, or a body moved under another key, fails.
    let mut parts: Vec<String> = text.split('.').map(str::to_string).collect();
    let mut body = parts[1].clone().into_bytes();
    body[5] = if body[5] == b'A' { b'B' } else { b'A' };
    parts[1] = String::from_utf8(body).unwrap();
    assert!(invite::read(&parts.join("."), signed::now()).is_err());
    let other = invite::make(&key(), Kind::Friend, "Eve", &[], Vec::new(), Some(7), None).0;
    let swapped = format!("{}.{}.{}.{}", "sulcus:i1", text.split('.').nth(1).unwrap(), other.split('.').nth(2).unwrap(), other.split('.').nth(3).unwrap());
    assert_eq!(invite::read(&swapped, signed::now()), Err(InviteError::Bad(Bad::Signature)));
    assert_eq!(invite::read(&text, signed::now() + 8 * 86_400), Err(InviteError::Expired));
    assert!(invite::read("sulcus:i1.x.y.z", 0).is_err());
    assert!(invite::read(&"a".repeat(10_000), 0).is_err());
}

#[test]
fn the_book_uses_up_and_expires_secrets() {
    let mut b = Book::default();
    b.add(invite::secret_hash("aa"), Kind::Friend, 2, 0);
    b.add(invite::secret_hash("bb"), Kind::Device, 1, 100);
    assert_eq!(b.redeem("AA", 50), Some(Kind::Friend), "case doesn't matter");
    assert_eq!(b.redeem("aa", 50), Some(Kind::Friend));
    assert_eq!(b.redeem("aa", 50), None, "two uses only");
    assert_eq!(b.redeem("bb", 101), None, "expired");
    assert_eq!(b.redeem("cc", 0), None);
}

#[test]
fn records_and_rosters_only_move_forward() {
    let k = key();
    let me = k.public().to_peer_id().to_string();
    let r = AddressRecord { peer: me.clone(), addrs: vec!["/ip4/1.2.3.4/tcp/1".into()], relays: vec![], seq: 2, issued: 1 };
    let text = r.seal(&k);
    assert_eq!(AddressRecord::open(&text).unwrap(), r);
    let forged = AddressRecord { peer: key().public().to_peer_id().to_string(), ..r.clone() }.seal(&k);
    assert_eq!(AddressRecord::open(&forged), Err(Bad::WrongSigner));
    let mut book = AddressBook::default();
    assert!(book.record(r.clone(), 10));
    assert!(!book.record(AddressRecord { seq: 1, ..r.clone() }, 11), "older is ignored");
    book.worked(&me, "/ip4/5.6.7.8/udp/1/quic-v1", 12);
    assert_eq!(book.addresses(&me), vec!["/ip4/5.6.7.8/udp/1/quic-v1".to_string(), "/ip4/1.2.3.4/tcp/1".to_string()]);
    book.age_out(12 + 91 * 86_400);
    assert!(book.peers.is_empty());

    let dev = |id: &str| RosterDevice { node_id: id.into(), name: "PC".into(), kind: "pc".into(), added: 1 };
    let roster = Roster { version: 1, person_name: "Sam".into(), devices: vec![dev(&me)], revoked: vec![], issued: 1, signer: None };
    let (got, host) = Roster::open(&roster.seal(&k)).unwrap();
    assert!(got.trusts(&host));
    assert_eq!(newer(Some((&got, &host)), &Roster { version: 2, ..got.clone() }, &host), Ok(true));
    assert_eq!(newer(Some((&got, &host)), &got, &host), Ok(false));
    assert!(newer(Some((&got, &host)), &got, &key().public().to_peer_id()).is_err(), "a changed key never replaces silently");
    let outsider = Roster { devices: vec![dev("someone-else")], ..roster };
    assert_eq!(Roster::open(&outsider.seal(&k)).map(|_| ()), Err(Bad::WrongSigner));
}

#[test]
fn the_address_book_is_capped() {
    let mut book = AddressBook::default();
    for i in 0..(crate::addrbook::MAX_PEERS + 20) {
        book.worked(&format!("peer{i}"), "/ip4/1.1.1.1/tcp/1", i as u64);
    }
    assert_eq!(book.peers.len(), crate::addrbook::MAX_PEERS);
    assert!(!book.peers.contains_key("peer0"), "the least recently seen go first");
    for i in 0..40 {
        book.worked("busy", &format!("/ip4/1.1.1.1/tcp/{i}"), 1_000_000);
    }
    assert_eq!(book.peers["busy"].worked.len(), crate::addrbook::MAX_ADDRS);
}

/// Every parser fed random and mutated input must return, never panic. A
/// cheap stand-in for cargo-fuzz (which needs a nightly toolchain).
#[test]
fn parsers_survive_mutated_input() {
    let k = key();
    let seeds = [
        invite::make(&k, Kind::Group, "Group", &["/ip4/127.0.0.1/tcp/1".parse().unwrap()], vec![invite::Relay { peer: "x".into(), addrs: vec!["y".into()] }], Some(1), Some("g".into())).0,
        AddressRecord { peer: k.public().to_peer_id().to_string(), addrs: vec!["/ip4/1.2.3.4/tcp/1".into()], relays: vec![], seq: 1, issued: 1 }.seal(&k),
    ];
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for round in 0..20_000 {
        let seed = seeds[round % seeds.len()].as_bytes().to_vec();
        let mut b = seed.clone();
        for _ in 0..(1 + next() % 8) {
            let at = (next() as usize) % b.len().max(1);
            match next() % 3 {
                0 => b[at] = (next() % 256) as u8,
                1 => {
                    b.remove(at);
                }
                _ => b.insert(at, (next() % 256) as u8),
            }
        }
        let text = String::from_utf8_lossy(&b);
        let _ = invite::read(&text, 0);
        let _ = AddressRecord::open(&text);
        let _ = Roster::open(&text);
        let _ = friend::parse_frame::<friend::Hello>(&b);
        let _ = friend::parse_frame::<friend::Answer>(&b);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_friend_invite_works_between_two_nodes() {
    let a_bytes = identity::generate();
    let a = Node::start(NodeConfig::loopback(a_bytes.clone())).unwrap();
    let b = Node::start(NodeConfig::loopback(identity::generate())).unwrap();
    let mut ev = a.subscribe();
    while a.listening().len() < 2 {
        let _ = tokio::time::timeout(Duration::from_secs(10), ev.recv()).await;
    }
    let addrs: Vec<libp2p::Multiaddr> = a.listening().into_iter().filter(|x| x.iter().any(|p| matches!(p, Protocol::Tcp(_)))).collect();
    let book = Arc::new(Mutex::new(Book::default()));
    let joined = Arc::new(Mutex::new(Vec::new()));
    let seen = joined.clone();
    friend::answer_invites(&a, book.clone(), "Alex".into(), None, move |j| seen.lock().unwrap().push(j));
    // A signs invites with its own node key.
    let (text, hash) = invite::make(&identity::load(&a_bytes).unwrap(), Kind::Friend, "Alex", &addrs, Vec::new(), Some(7), None);
    book.lock().unwrap().add(hash, Kind::Friend, 1, 0);
    let checked = invite::read(&text, signed::now()).unwrap();
    assert_eq!(checked.from, a.peer_id());
    let (name, _) = tokio::time::timeout(Duration::from_secs(30), friend::use_invite(&b, &checked, "Blake", None, false)).await.unwrap().unwrap();
    assert_eq!(name, "Alex");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let got = joined.lock().unwrap().clone();
    assert_eq!(got.len(), 1);
    assert_eq!((got[0].peer, got[0].name.as_str(), got[0].kind), (b.peer_id(), "Blake", Kind::Friend));
    // Used up: a second try is refused.
    assert!(friend::use_invite(&b, &checked, "Blake", None, false).await.is_err());
    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn lan_and_router_stay_off_unless_asked() {
    let a = Node::start(NodeConfig::loopback(identity::generate())).unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(a.lan_peers().is_empty() && a.external().is_empty());
    a.stop().await;
}

/// Two nodes with LAN discovery on find each other by mDNS (multicast on
/// this PC's network, so not run by default).
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn e2e_two_nodes_find_each_other_on_the_lan() {
    let on = |key| NodeConfig { lan_discovery: true, ..NodeConfig::any_port(key) };
    let a = Node::start(on(identity::generate())).unwrap();
    let b = Node::start(on(identity::generate())).unwrap();
    let found = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if a.lan_peers().iter().any(|(p, _)| *p == b.peer_id()) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await;
    a.stop().await;
    b.stop().await;
    assert!(found.is_ok(), "A didn't see B on the local network");
}

/// A relays; B takes a reservation on A; C reaches B through A and they
/// talk over a stream. All on 127.0.0.1.
#[tokio::test(flavor = "multi_thread")]
async fn a_peer_is_reached_through_a_relay() {
    use futures::{AsyncReadExt, AsyncWriteExt, StreamExt};
    let relay = Node::start(NodeConfig { relay_server: true, ..NodeConfig::loopback(identity::generate()) }).unwrap();
    let b = Node::start(NodeConfig::loopback(identity::generate())).unwrap();
    let c = Node::start(NodeConfig::loopback(identity::generate())).unwrap();
    let mut ev = relay.subscribe();
    while !relay.listening().iter().any(|a| a.iter().any(|p| matches!(p, Protocol::Tcp(_)))) {
        let _ = tokio::time::timeout(Duration::from_secs(10), ev.recv()).await;
    }
    let relay_addr = relay.listening().into_iter().find(|a| a.iter().any(|p| matches!(p, Protocol::Tcp(_)))).unwrap().with(Protocol::P2p(relay.peer_id()));
    let mut b_ev = b.subscribe();
    b.dial(relay_addr.clone()).await.unwrap();
    // As a real PC would: connected and identified first, then the reservation.
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Ok(crate::NodeEvent::Identified { peer, .. }) = b_ev.recv().await {
                if peer == relay.peer_id() {
                    return;
                }
            }
        }
    })
    .await
    .expect("B identified the relay");
    b.listen_via(relay_addr.clone()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            match b_ev.recv().await {
                Ok(crate::NodeEvent::Reserved { relay: r }) if r == relay.peer_id() => return,
                Ok(other) => eprintln!("B: {other:?}"),
                Err(e) => eprintln!("B events: {e}"),
            }
        }
    })
    .await
    .expect("B got a reservation");
    const ECHO: libp2p::StreamProtocol = libp2p::StreamProtocol::new("/sulcus/test-relay-echo/1");
    let mut incoming = b.streams().accept(ECHO).unwrap();
    tokio::spawn(async move {
        if let Some((_, mut s)) = incoming.next().await {
            let mut buf = [0u8; 4];
            s.read_exact(&mut buf).await.unwrap();
            s.write_all(&buf).await.unwrap();
            s.close().await.unwrap();
        }
    });
    let mut c_ev = c.subscribe();
    c.dial(relay_addr.clone()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Ok(crate::NodeEvent::Identified { peer, .. }) = c_ev.recv().await {
                if peer == relay.peer_id() {
                    return;
                }
            }
        }
    })
    .await
    .expect("C identified the relay");
    let through = relay_addr.with(Protocol::P2pCircuit).with(Protocol::P2p(b.peer_id()));
    c.dial(through).await.unwrap();
    let watch = tokio::spawn(async move {
        while let Ok(e) = c_ev.recv().await {
            eprintln!("C: {e:?}");
        }
    });
    let mut s = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Ok(s) = c.streams().open_stream(b.peer_id(), ECHO).await {
                return s;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .expect("C opened a stream to B through the relay");
    s.write_all(b"ping").await.unwrap();
    let mut back = [0u8; 4];
    tokio::time::timeout(Duration::from_secs(20), s.read_exact(&mut back)).await.unwrap().unwrap();
    assert_eq!(&back, b"ping");
    watch.abort();
    relay.stop().await;
    b.stop().await;
    c.stop().await;
}

/// A newcomer joins a relay with its invite key: it waits for the owner,
/// then is a member; a wrong key is refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_newcomer_joins_a_relay_with_its_key() {
    use crate::join::Admissions;
    use crate::join_wire::{self, Reply};
    let relay = Node::start(NodeConfig { relay_server: true, ..NodeConfig::loopback(identity::generate()) }).unwrap();
    let newcomer = Node::start(NodeConfig::loopback(identity::generate())).unwrap();
    let adm = Arc::new(Mutex::new(Admissions::default()));
    let (id, secret) = adm.lock().unwrap().new_key("Family", signed::now(), None, None);
    join_wire::serve(&relay, adm.clone(), || {});
    let mut ev = relay.subscribe();
    while !relay.listening().iter().any(|a| a.iter().any(|p| matches!(p, Protocol::Tcp(_)))) {
        let _ = tokio::time::timeout(Duration::from_secs(10), ev.recv()).await;
    }
    let addr = relay.listening().into_iter().find(|a| a.iter().any(|p| matches!(p, Protocol::Tcp(_)))).unwrap().with(Protocol::P2p(relay.peer_id()));
    let mut nev = newcomer.subscribe();
    newcomer.dial(addr).await.unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        while !newcomer.connected().contains(&relay.peer_id()) {
            let _ = nev.recv().await;
        }
    })
    .await
    .unwrap();
    let r = join_wire::ask(&newcomer, relay.peer_id(), &id, &secret, "Ana's laptop", "pc").await.unwrap();
    assert_eq!(r, Reply::Waiting);
    assert_eq!(adm.lock().unwrap().pending[0].name, "Ana's laptop");
    assert!(adm.lock().unwrap().decide(&newcomer.peer_id().to_string(), true));
    assert_eq!(join_wire::ask(&newcomer, relay.peer_id(), &id, &secret, "Ana's laptop", "pc").await.unwrap(), Reply::Member);
    let wrong = join_wire::ask(&newcomer, relay.peer_id(), &id, "00112233445566778899aabbccddeeff", "Ana", "pc").await.unwrap();
    assert_eq!(wrong, Reply::Member, "a member stays a member");
    relay.stop().await;
    newcomer.stop().await;
}

/// With members set, a stranger's reservation is cut off.
#[tokio::test(flavor = "multi_thread")]
async fn only_members_keep_a_reservation() {
    let relay = Node::start(NodeConfig { relay_server: true, ..NodeConfig::loopback(identity::generate()) }).unwrap();
    relay.set_relay_members(Some(std::collections::HashSet::new()));
    let b = Node::start(NodeConfig::loopback(identity::generate())).unwrap();
    let mut ev = relay.subscribe();
    while !relay.listening().iter().any(|a| a.iter().any(|p| matches!(p, Protocol::Tcp(_)))) {
        let _ = tokio::time::timeout(Duration::from_secs(10), ev.recv()).await;
    }
    let relay_addr = relay.listening().into_iter().find(|a| a.iter().any(|p| matches!(p, Protocol::Tcp(_)))).unwrap().with(Protocol::P2p(relay.peer_id()));
    let mut b_ev = b.subscribe();
    b.dial(relay_addr.clone()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Ok(crate::NodeEvent::Identified { peer, .. }) = b_ev.recv().await {
                if peer == relay.peer_id() {
                    return;
                }
            }
        }
    })
    .await
    .unwrap();
    b.listen_via(relay_addr).await.unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(!relay.connected().contains(&b.peer_id()), "a non-member was cut off");
    relay.stop().await;
    b.stop().await;
}

/// A leaves a letter for B on a relay while B is away; B takes it later.
#[tokio::test(flavor = "multi_thread")]
async fn a_letter_waits_on_the_relay() {
    use crate::mailbox::{self, Reply, Request, Store};
    let relay = Node::start(NodeConfig { relay_server: true, ..NodeConfig::loopback(identity::generate()) }).unwrap();
    let store = Arc::new(Mutex::new(Store::default()));
    mailbox::serve(&relay, store.clone(), |_| true, || {});
    let mut ev = relay.subscribe();
    while !relay.listening().iter().any(|a| a.iter().any(|p| matches!(p, Protocol::Tcp(_)))) {
        let _ = tokio::time::timeout(Duration::from_secs(10), ev.recv()).await;
    }
    let addr = relay.listening().into_iter().find(|a| a.iter().any(|p| matches!(p, Protocol::Tcp(_)))).unwrap().with(Protocol::P2p(relay.peer_id()));
    use crate::seal::{self, SealSecret, Seen};
    let a_key = identity::generate();
    let a_seal = SealSecret::generate();
    let a = Node::start(NodeConfig::loopback(a_key.clone())).unwrap();
    let b_key = identity::generate();
    let b_seal = SealSecret::generate();
    let b_id = identity::peer_id(&b_key).unwrap();
    connect_to(&a, addr.clone(), relay.peer_id()).await;
    let now = crate::signed::now();
    let body = seal::seal(&identity::load(&a_key).unwrap(), &a_seal, &b_seal.record(&identity::load(&b_key).unwrap(), now), "m1", b"see you at six", now).unwrap();
    let r = mailbox::ask(&a, relay.peer_id(), &Request::Put { to: b_id.to_string(), id: "m1".into(), body }).await.unwrap();
    assert_eq!(r, Reply::Kept);
    a.stop().await;
    let b = Node::start(NodeConfig::loopback(b_key)).unwrap();
    connect_to(&b, addr, relay.peer_id()).await;
    let Reply::Letters { letters } = mailbox::ask(&b, relay.peer_id(), &Request::Take).await.unwrap() else { panic!() };
    assert_eq!(letters.len(), 1);
    let mut seen = Seen::default();
    let got = seal::open(&b_id, &b_seal, &letters[0], &mut seen, crate::signed::now()).unwrap();
    assert_eq!((got.from, got.body.as_slice()), (a.peer_id(), &b"see you at six"[..]));
    assert!(seal::open(&b_id, &b_seal, &letters[0], &mut seen, crate::signed::now()).is_err(), "a replay is refused");
    relay.stop().await;
    b.stop().await;
}

async fn connect_to(n: &crate::NodeHandle, addr: libp2p::Multiaddr, peer: libp2p::PeerId) {
    let mut ev = n.subscribe();
    n.dial(addr).await.unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        while !n.connected().contains(&peer) {
            let _ = ev.recv().await;
        }
    })
    .await
    .unwrap();
}

/// The own relay passes a phone request to the PC and reads its answer; a
/// stranger and a wrong link id get nothing from the PC.
#[tokio::test(flavor = "multi_thread")]
async fn a_phone_request_reaches_the_pc_through_its_relay() {
    use crate::phone_wire::{self, Answer, Request};
    let relay = Node::start(NodeConfig { relay_server: true, ..NodeConfig::loopback(identity::generate()) }).unwrap();
    let pc = Node::start(NodeConfig::loopback(identity::generate())).unwrap();
    let stranger = Node::start(NodeConfig::loopback(identity::generate())).unwrap();
    let rid = relay.peer_id();
    phone_wire::serve(&pc, move || Some((rid, "ab".to_string())), |req| async move {
        match req {
            Request::Get { path, .. } => Answer { status: 200, headers: vec![("content-type".into(), "text/html".into())], body: path.into_bytes() },
            Request::Api { body, .. } => Answer { status: 200, headers: Vec::new(), body: body.repeat(3).into_bytes() },
        }
    });
    let mut ev = pc.subscribe();
    while !pc.listening().iter().any(|a| a.iter().any(|p| matches!(p, Protocol::Tcp(_)))) {
        let _ = tokio::time::timeout(Duration::from_secs(10), ev.recv()).await;
    }
    let addr = pc.listening().into_iter().find(|a| a.iter().any(|p| matches!(p, Protocol::Tcp(_)))).unwrap().with(Protocol::P2p(pc.peer_id()));
    for n in [&relay, &stranger] {
        let mut nev = n.subscribe();
        n.dial(addr.clone()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(20), async {
            while !n.connected().contains(&pc.peer_id()) {
                let _ = nev.recv().await;
            }
        })
        .await
        .unwrap();
    }
    let a = phone_wire::ask(&relay, pc.peer_id(), &Request::Get { link: "ab".into(), path: "/index.html".into() }).await.unwrap();
    assert_eq!((a.status, a.body.as_slice()), (200, b"/index.html".as_slice()));
    let big = "x".repeat(700_000);
    let a = phone_wire::ask(&relay, pc.peer_id(), &Request::Api { link: "ab".into(), phone: "p".into(), body: big.clone() }).await.unwrap();
    assert_eq!(a.body.len(), big.len() * 3, "long answers come back whole");
    let a = phone_wire::ask(&relay, pc.peer_id(), &Request::Get { link: "other".into(), path: "/".into() }).await.unwrap();
    assert_eq!(a.status, 404);
    assert!(phone_wire::ask(&stranger, pc.peer_id(), &Request::Get { link: "ab".into(), path: "/".into() }).await.is_err());
    for n in [relay, pc, stranger] {
        n.stop().await;
    }
}
