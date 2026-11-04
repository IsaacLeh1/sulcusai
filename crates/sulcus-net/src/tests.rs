// SPDX-License-Identifier: AGPL-3.0-only
//! Two nodes in one process, on 127.0.0.1 only.

use std::time::Duration;

use futures::{AsyncReadExt, AsyncWriteExt, StreamExt};
use libp2p::core::multiaddr::{Multiaddr, Protocol};
use libp2p::{PeerId, StreamProtocol};
use tokio::time::timeout;

use crate::{identity, Node, NodeConfig, NodeEvent, NodeHandle};

const LIMIT: Duration = Duration::from_secs(20);
const ECHO: StreamProtocol = StreamProtocol::new("/sulcus/test-echo/1");

fn node() -> NodeHandle {
    Node::start(NodeConfig::loopback(identity::generate())).unwrap()
}

async fn addr(n: &NodeHandle, quic: bool) -> Multiaddr {
    let mut ev = n.subscribe();
    loop {
        if let Some(a) = n.listening().into_iter().find(|a| a.iter().any(|p| matches!(p, Protocol::QuicV1)) == quic) {
            return a.with(Protocol::P2p(n.peer_id()));
        }
        let _ = timeout(LIMIT, ev.recv()).await.expect("listening");
    }
}

async fn connected(n: &NodeHandle, peer: PeerId) {
    let mut ev = n.subscribe();
    timeout(LIMIT, async {
        while !n.connected().contains(&peer) {
            match ev.recv().await {
                Ok(NodeEvent::Connected { peer: p, .. }) if p == peer => return,
                Ok(NodeEvent::DialFailed { error, .. }) => panic!("dial failed: {error}"),
                _ => {}
            }
        }
    })
    .await
    .expect("connected in time");
}

async fn connect_and_echo(quic: bool) {
    let a = node();
    let b = node();
    let mut incoming = a.streams().accept(ECHO).unwrap();
    tokio::spawn(async move {
        while let Some((_, mut s)) = incoming.next().await {
            let mut buf = [0u8; 5];
            s.read_exact(&mut buf).await.unwrap();
            s.write_all(&buf).await.unwrap();
            s.close().await.unwrap();
        }
    });
    let target = addr(&a, quic).await;
    b.dial(target).await.unwrap();
    connected(&b, a.peer_id()).await;
    connected(&a, b.peer_id()).await;
    let mut s = timeout(LIMIT, b.streams().open_stream(a.peer_id(), ECHO)).await.unwrap().unwrap();
    s.write_all(b"hello").await.unwrap();
    let mut back = [0u8; 5];
    timeout(LIMIT, s.read_exact(&mut back)).await.unwrap().unwrap();
    assert_eq!(&back, b"hello");
    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn two_nodes_connect_over_quic() {
    connect_and_echo(true).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn two_nodes_connect_over_tcp() {
    connect_and_echo(false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn blocked_peer_is_refused_both_ways() {
    let a = node();
    let b = node();
    a.set_blocked([b.peer_id()]);
    b.dial(addr(&a, true).await).await.unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(!a.connected().contains(&b.peer_id()));
    b.set_blocked([a.peer_id()]);
    assert!(b.dial(addr(&a, false).await).await.is_err());
    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_frees_the_ports() {
    let a = node();
    let tcp = addr(&a, false).await;
    a.stop().await;
    assert!(a.listening().is_empty());
    let port = tcp.iter().find_map(|p| match p {
        Protocol::Tcp(p) => Some(p),
        _ => None,
    });
    std::net::TcpListener::bind(("127.0.0.1", port.unwrap())).expect("port free after stop");
    assert!(a.dial(tcp).await.is_err());
}

#[test]
fn node_key_round_trips_and_rejects_junk() {
    let k = identity::generate();
    assert_eq!(identity::peer_id(&k).unwrap(), identity::peer_id(&k).unwrap());
    assert!(identity::load(b"not a key").is_err());
    assert_ne!(identity::peer_id(&k).unwrap(), identity::peer_id(&identity::generate()).unwrap());
}

