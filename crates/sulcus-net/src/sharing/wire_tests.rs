// SPDX-License-Identifier: AGPL-3.0-only
//! Sharing between nodes in one process, on 127.0.0.1 only.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use libp2p::core::multiaddr::Protocol;
use libp2p::PeerId;

use super::remote::Host;
use super::share::{self, Manifest};
use super::wire::{self, Input, Pieces, RemoteLink, VlanLink};
use crate::{identity, Node, NodeConfig, NodeHandle};

fn node() -> NodeHandle {
    Node::start(NodeConfig::loopback(identity::generate())).unwrap()
}

async fn link(a: &NodeHandle, b: &NodeHandle) {
    let mut ev = b.subscribe();
    let addr = loop {
        if let Some(x) = b.listening().into_iter().find(|x| x.iter().any(|p| matches!(p, Protocol::Tcp(_)))) {
            break x.with(Protocol::P2p(b.peer_id()));
        }
        let _ = tokio::time::timeout(Duration::from_secs(10), ev.recv()).await;
    };
    let mut ev = a.subscribe();
    a.dial(addr).await.unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        while !a.connected().contains(&b.peer_id()) {
            let _ = ev.recv().await;
        }
    })
    .await
    .unwrap();
}

struct Store {
    m: Manifest,
    pieces: Vec<Vec<u8>>,
    only: Option<Vec<u32>>,
    lie: bool,
}

impl Pieces for Store {
    fn manifest(&self, id: &str) -> Option<Manifest> {
        (self.m.id() == id).then(|| self.m.clone())
    }
    fn have(&self, _: &str) -> Vec<u32> {
        self.only.clone().unwrap_or_else(|| (0..self.pieces.len() as u32).collect())
    }
    fn piece(&self, _: &str, i: u32) -> Option<Vec<u8>> {
        let mut p = self.pieces.get(i as usize)?.clone();
        if self.lie {
            p[0] ^= 1;
        }
        Some(p)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_share_comes_from_two_sources_and_a_liar_is_ignored() {
    let data: Vec<u8> = (0..50_000u32).map(|i| (i * 7 % 256) as u8).collect();
    let key = share::new_key();
    let (m, sealed) = share::seal_file("game.zip", &data, &key, 4096);
    let (a, b, liar, me) = (node(), node(), node(), node());
    let half: Vec<u32> = (0..m.pieces.len() as u32).filter(|i| i % 2 == 0).collect();
    wire::serve_share(&a, Arc::new(Store { m: m.clone(), pieces: sealed.clone(), only: Some(half), lie: false }), |_| true);
    wire::serve_share(&b, Arc::new(Store { m: m.clone(), pieces: sealed.clone(), only: None, lie: false }), |_| true);
    wire::serve_share(&liar, Arc::new(Store { m: m.clone(), pieces: sealed.clone(), only: None, lie: true }), |_| true);
    for n in [&a, &b, &liar] {
        link(&me, n).await;
    }
    let l = share::make_link(&m, &key, &a.peer_id().to_string());
    let mut seen = 0;
    let (_, got) = wire::fetch(&me, &l, &[liar.peer_id(), a.peer_id(), b.peer_id()], |d, _| seen = d).await.unwrap();
    assert_eq!(got, data);
    assert_eq!(seen as usize, m.pieces.len());
    for n in [a, b, liar, me] {
        n.stop().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn shares_are_refused_to_people_not_allowed() {
    let key = share::new_key();
    let (m, sealed) = share::seal_file("x", b"secret", &key, 4096);
    let (a, me) = (node(), node());
    wire::serve_share(&a, Arc::new(Store { m: m.clone(), pieces: sealed, only: None, lie: false }), |_| false);
    link(&me, &a).await;
    let l = share::make_link(&m, &key, "a");
    assert!(wire::fetch(&me, &l, &[a.peer_id()], |_, _| {}).await.is_err());
    a.stop().await;
    me.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cards_and_vlan_packets_cross() {
    let (a, b) = (node(), node());
    let me = b.peer_id();
    wire::serve_cards(&a, move |p| (*p == me).then(|| vec!["smc1.card".to_string()]));
    let got: Arc<Mutex<Vec<(String, PeerId, Vec<u8>)>>> = Arc::default();
    let g = got.clone();
    wire::serve_vlan(&a, move |net, p, pkt| g.lock().unwrap().push((net.to_string(), *p, pkt)));
    link(&b, &a).await;
    assert_eq!(wire::fetch_cards(&b, a.peer_id()).await.unwrap(), vec!["smc1.card".to_string()]);
    let mut v = VlanLink::open(&b, a.peer_id(), "n1").await.unwrap();
    v.send(&[1, 2, 3]).await.unwrap();
    v.send(&[4]).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while got.lock().unwrap().len() < 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let g = got.lock().unwrap().clone();
    assert_eq!(g[0], ("n1".to_string(), b.peer_id(), vec![1, 2, 3]));
    assert_eq!(g[1].2, vec![4]);
    a.stop().await;
    b.stop().await;
}

fn ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_input_only_after_accept_and_until_stop() {
    let (pc, friend) = (node(), node());
    let host = Arc::new(Mutex::new(Host::new(true)));
    let codes: Arc<Mutex<HashMap<String, String>>> = Arc::default();
    let inputs: Arc<Mutex<Vec<Input>>> = Arc::default();
    let (h2, c2, i2) = (host.clone(), codes.clone(), inputs.clone());
    wire::serve_remote(
        &pc,
        host.clone(),
        ms,
        move |p, code| {
            c2.lock().unwrap().insert(p.to_string(), code.to_string());
            // The person checks the code and presses Accept.
            h2.lock().unwrap().accept(code, ms()).unwrap();
        },
        move |i| i2.lock().unwrap().push(i),
        || Some(vec![0xFF, 0xD8, 1]),
    );
    link(&friend, &pc).await;
    let mut r = RemoteLink::ask(&friend, pc.peer_id(), "123456").await.unwrap();
    r.send(&Input::Move { x: 0.5, y: 0.5 }).await.unwrap();
    r.send(&Input::Key { code: "KeyA".into(), down: true }).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while inputs.lock().unwrap().len() < 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(host.lock().unwrap().show().bar);
    let pic = tokio::time::timeout(Duration::from_secs(5), r.frames.recv()).await.unwrap().unwrap();
    assert_eq!(pic, vec![0xFF, 0xD8, 1]);
    host.lock().unwrap().stop();
    let _ = r.send(&Input::Click { button: 0, down: true }).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(inputs.lock().unwrap().len(), 2, "nothing after Stop");
    assert_eq!(codes.lock().unwrap()[&friend.peer_id().to_string()], "123456");

    host.lock().unwrap().set_enabled(false);
    assert!(RemoteLink::ask(&friend, pc.peer_id(), "1").await.is_err());
    pc.stop().await;
    friend.stop().await;
}
