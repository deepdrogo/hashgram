//! Live swarm tests: three nodes on loopback, and a fork that is refused.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use hashgram_net::NetworkIdentity;
use hashgram_p2p::peerstore::Peerstore;
use hashgram_p2p::{start, Event, Keypair, Multiaddr, NodeConfig, NodeHandle, Transport};
use hashgram_proto::pb;
use prometheus_client::registry::Registry;
use tokio::sync::mpsc;

const GENESIS: &str = "9348af00681eecefb8d6329d5ba101c13bc3c8943f2c610295026f6503654287";
const FORK_GENESIS: &str = "eed34034d774c8a2f0b1e5c6d7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d3e4f5a6";

struct TestNode {
    handle: NodeHandle,
    events: mpsc::Receiver<Event>,
    addr: Multiaddr,
}

async fn node(genesis: &str, roles: &[&str], bootstrap: &[Multiaddr]) -> TestNode {
    let mut cfg: NodeConfig = toml::from_str("").unwrap();
    cfg.network = "devnet".into();
    cfg.genesis_hash = genesis.into();
    cfg.roles = roles.iter().map(|r| (*r).to_owned()).collect();
    cfg.listen_addr = "127.0.0.1".parse().unwrap();
    cfg.listen_port = free_port();
    cfg.transport = Transport::Tcp;
    cfg.bootstrap_peers = bootstrap.iter().map(ToString::to_string).collect();
    cfg.min_peers = 1;
    let identity = NetworkIdentity::devnet(genesis);
    let key = Keypair::generate_ed25519();
    let mut registry = Registry::default();
    let (handle, mut events, _task) =
        start(cfg, identity, key, Peerstore::in_memory(), &mut registry).unwrap();
    // Wait for the listen address.
    let addr = loop {
        match tokio::time::timeout(Duration::from_secs(5), events.recv()).await {
            Ok(Some(Event::Listening(a))) => break a,
            Ok(Some(_)) => continue,
            _ => panic!("node did not start listening"),
        }
    };
    let addr = addr.with(libp2p::multiaddr::Protocol::P2p(handle.peer_id()));
    TestNode {
        handle,
        events,
        addr,
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn wait_for<F: FnMut(&Event) -> bool>(
    events: &mut mpsc::Receiver<Event>,
    mut pred: F,
) -> Event {
    loop {
        match tokio::time::timeout(Duration::from_secs(15), events.recv()).await {
            Ok(Some(e)) => {
                if pred(&e) {
                    return e;
                }
            }
            Ok(None) => panic!("event stream closed"),
            Err(_) => panic!("timed out waiting for event"),
        }
    }
}

#[tokio::test]
async fn three_nodes_verify_each_other_and_exchange_requests() {
    let mut a = node(GENESIS, &["bootstrap", "relay"], &[]).await;
    let mut b = node(GENESIS, &["store"], &[a.addr.clone()]).await;
    let mut c = node(GENESIS, &["relay"], &[a.addr.clone()]).await;

    // A sees B and C verify; B and C see A.
    let mut seen = 0;
    while seen < 2 {
        if let Event::PeerVerified { roles, .. } =
            wait_for(&mut a.events, |e| matches!(e, Event::PeerVerified { .. })).await
        {
            assert!(roles == vec!["store"] || roles == vec!["relay"]);
            seen += 1;
        }
    }
    if let Event::PeerVerified { peer, roles } =
        wait_for(&mut b.events, |e| matches!(e, Event::PeerVerified { .. })).await
    {
        assert_eq!(peer, a.handle.peer_id());
        assert_eq!(roles, vec!["bootstrap", "relay"]);
    }
    wait_for(&mut c.events, |e| matches!(e, Event::PeerVerified { .. })).await;

    // B sends an application-level request; A's application answers.
    let b_handle = b.handle.clone();
    let a_peer = a.handle.peer_id();
    let req = tokio::spawn(async move {
        b_handle
            .request(
                a_peer,
                vec![],
                pb::Request {
                    body: Some(pb::request::Body::AnnounceQuery(pb::AnnounceQuery {
                        roles: vec![],
                        limit: 5,
                    })),
                },
            )
            .await
    });
    if let Event::InboundRequest {
        peer,
        request,
        channel,
    } = wait_for(&mut a.events, |e| matches!(e, Event::InboundRequest { .. })).await
    {
        assert_eq!(peer, b.handle.peer_id());
        assert!(matches!(
            request.body,
            Some(pb::request::Body::AnnounceQuery(_))
        ));
        a.handle
            .respond(
                channel,
                pb::Response {
                    body: Some(pb::response::Body::AnnounceQuery(
                        pb::AnnounceQueryResult::default(),
                    )),
                },
            )
            .await;
    }
    let resp = req.await.unwrap().unwrap();
    assert!(matches!(
        resp.body,
        Some(pb::response::Body::AnnounceQuery(_))
    ));

    // Peer exchange is answered by the swarm itself, from verified peers
    // only: B learns C's address from A without A's application involved.
    let resp = b
        .handle
        .request(
            a.handle.peer_id(),
            vec![],
            pb::Request {
                body: Some(pb::request::Body::PeerExchange(pb::PeerExchange {
                    limit: 5,
                })),
            },
        )
        .await
        .unwrap();
    match resp.body {
        Some(pb::response::Body::PeerExchange(r)) => {
            let c_id = c.handle.peer_id().to_string();
            assert!(
                r.addrs.iter().any(|a| a.ends_with(&c_id)),
                "peer exchange did not offer C: {:?}",
                r.addrs
            );
        }
        other => panic!("unexpected response {other:?}"),
    }

    // Stats reflect the verified set.
    let stats = a.handle.stats().await.unwrap();
    assert_eq!(stats.verified, 2);
    assert!(stats.known >= 2);
    let store_peers = a.handle.peers_with_role("store").await;
    assert_eq!(store_peers.len(), 1);
    assert_eq!(store_peers.first().map(|p| p.0), Some(b.handle.peer_id()));
}

#[tokio::test]
async fn a_fork_with_a_different_genesis_is_rejected() {
    let mut mainnet_like = node(GENESIS, &["bootstrap"], &[]).await;
    let mut fork = node(FORK_GENESIS, &["relay"], &[mainnet_like.addr.clone()]).await;

    // The fork dials us and sends its handshake; we refuse on the genesis
    // hash and it learns why.
    match wait_for(&mut mainnet_like.events, |e| {
        matches!(e, Event::PeerRejected { .. })
    })
    .await
    {
        Event::PeerRejected { peer, reason } => {
            assert_eq!(peer, fork.handle.peer_id());
            assert!(reason.contains("genesis hash mismatch"), "reason: {reason}");
        }
        _ => unreachable!(),
    }
    match wait_for(&mut fork.events, |e| {
        matches!(e, Event::PeerRejected { .. })
    })
    .await
    {
        Event::PeerRejected { reason, .. } => {
            assert!(reason.contains("genesis"), "reason: {reason}")
        }
        _ => unreachable!(),
    }

    // Nothing was verified on either side, and the fork is now banned.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let stats = mainnet_like.handle.stats().await.unwrap();
    assert_eq!(stats.verified, 0);
    assert_eq!(stats.banned, 1);

    // A request from the fork is refused before any handler runs: the
    // request errors at the requester because the peer is banned.
    let err = fork
        .handle
        .request(
            mainnet_like.handle.peer_id(),
            vec![mainnet_like.addr.clone()],
            pb::Request {
                body: Some(pb::request::Body::PeerExchange(pb::PeerExchange {
                    limit: 1,
                })),
            },
        )
        .await;
    assert!(err.is_err(), "a fork obtained a response: {err:?}");
}

#[tokio::test]
async fn an_unauthenticated_request_is_refused() {
    // B dials A and immediately queues an application request. The swarm
    // handshakes first, then flushes the queue, so the request reaches A's
    // application only after A has verified B.
    let mut a = node(GENESIS, &["bootstrap"], &[]).await;
    let b = node(GENESIS, &[], &[]).await;

    let b_handle = b.handle.clone();
    let a_addr = a.addr.clone();
    let a_peer = a.handle.peer_id();
    let req = tokio::spawn(async move {
        b_handle
            .request(
                a_peer,
                vec![a_addr],
                pb::Request {
                    body: Some(pb::request::Body::AnnounceQuery(
                        pb::AnnounceQuery::default(),
                    )),
                },
            )
            .await
    });
    wait_for(&mut a.events, |e| matches!(e, Event::PeerVerified { .. })).await;
    if let Event::InboundRequest { channel, .. } =
        wait_for(&mut a.events, |e| matches!(e, Event::InboundRequest { .. })).await
    {
        a.handle
            .respond(
                channel,
                pb::Response {
                    body: Some(pb::response::Body::AnnounceQuery(
                        pb::AnnounceQueryResult::default(),
                    )),
                },
            )
            .await;
    }
    assert!(req.await.unwrap().is_ok());
}

#[tokio::test]
async fn gossip_reaches_a_subscribed_verified_peer() {
    let mut a = node(GENESIS, &["bootstrap"], &[]).await;
    let mut b = node(GENESIS, &[], &[a.addr.clone()]).await;
    wait_for(&mut a.events, |e| matches!(e, Event::PeerVerified { .. })).await;
    wait_for(&mut b.events, |e| matches!(e, Event::PeerVerified { .. })).await;

    let topic = hashgram_p2p::topics::announce("hashgram-devnet");
    a.handle.subscribe(&topic).await;
    b.handle.subscribe(&topic).await;
    // Gossipsub needs a heartbeat to form the mesh.
    tokio::time::sleep(Duration::from_millis(1500)).await;

    let mut published = false;
    for _ in 0..10 {
        if a.handle.publish(&topic, b"hello".to_vec()).await.is_ok() {
            published = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    assert!(published, "publish never succeeded");
    match wait_for(&mut b.events, |e| matches!(e, Event::Gossip { .. })).await {
        Event::Gossip {
            data,
            source,
            topic: t,
            ..
        } => {
            assert_eq!(data, b"hello");
            assert_eq!(source, a.handle.peer_id());
            assert_eq!(t, topic);
        }
        _ => unreachable!(),
    }
}
