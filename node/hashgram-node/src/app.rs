//! The node application: routes swarm events to services.
//!
//! The swarm decides *who* may talk (handshake, limits, scoring). This layer
//! decides *what* they may ask, by role: a node without the `store` role
//! answers a mailbox request with `unsupported` rather than pretending. Each
//! service is a module with a narrow interface, so a later machine that runs
//! only `media` runs only the blob service and nothing else is compiled in
//! as live code paths for that role.

use std::sync::Arc;
use std::time::Duration;

use hashgram_net::NetworkIdentity;
use hashgram_p2p::{topics, Event, MessageAcceptance, NodeConfig, NodeHandle, PeerId, ScoreEvent};
use hashgram_proto::frame::decode_bounded;
use hashgram_proto::limits::MAX_GOSSIP_FRAME;
use hashgram_proto::pb;
use hashgram_proto::Ed25519Signer;
use prost::Message;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::announce::{AnnounceTable, SelfAnnounce, ANNOUNCE_INTERVAL_SECS};

/// Shared node state, reachable from the API and the services.
pub struct Shared {
    /// P2P handle.
    pub handle: NodeHandle,
    /// Configuration.
    pub config: NodeConfig,
    /// Network identity.
    pub identity: NetworkIdentity,
    /// Announcements from peers.
    pub announces: AnnounceTable,
    /// Signer for this node's own announcements.
    pub announce_signer: Ed25519Signer,
    /// When the node started.
    pub started: std::time::Instant,
    /// Optional services, present according to role.
    pub services: Services,
    /// Useful-service agent, when an operator key is configured.
    pub rewards: Option<Arc<crate::rewards::RewardsAgent>>,
    /// coturn shared secret, when this node issues TURN credentials.
    pub turn_secret: Option<Vec<u8>>,
}

/// Role-gated services.
#[derive(Default)]
pub struct Services {
    /// Mailbox and key-package store (`store` role).
    pub mailbox: Option<Arc<crate::mailbox::MailboxService>>,
    /// Blob store (`store` or `media` role).
    pub blob: Option<Arc<crate::blob::BlobService>>,
    /// Social event log (every node relays; `indexer` keeps everything).
    pub social: Option<Arc<crate::social::SocialService>>,
    /// Safety attestation table (every node honours; `safety` publishes).
    pub safety: Option<Arc<crate::safety::SafetyService>>,
    /// Chain relay: read queries and broadcast forwarded to the co-located
    /// gateway (`relay` or `bootstrap` role, chain reachable).
    pub chain_relay: Option<Arc<crate::chain_relay::ChainRelayService>>,
}

fn err(code: &str, msg: impl Into<String>) -> pb::Response {
    pb::Response {
        body: Some(pb::response::Body::Error(pb::Error {
            code: code.to_owned(),
            message: msg.into(),
        })),
    }
}

/// Runs the application loop until the event stream ends.
pub async fn run(shared: Arc<Shared>, mut events: mpsc::Receiver<Event>) {
    let network_id = shared.identity.network_id.clone();

    // Every node relays announcements and safety attestations, and the
    // social shards it is configured for.
    shared
        .handle
        .subscribe(&topics::announce(&network_id))
        .await;
    shared.handle.subscribe(&topics::safety(&network_id)).await;
    if let Some(social) = &shared.services.social {
        for t in social.topics() {
            shared.handle.subscribe(&t).await;
        }
    }
    if let Some(mailbox) = &shared.services.mailbox {
        for t in mailbox.topics() {
            shared.handle.subscribe(&t).await;
        }
    }

    // Periodic self-announcement.
    let announcer = {
        let shared = shared.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(ANNOUNCE_INTERVAL_SECS));
            // First tick fires immediately; wait a little for listen addrs.
            tokio::time::sleep(Duration::from_secs(5)).await;
            loop {
                publish_self(&shared).await;
                shared.announces.prune();
                if let Some(s) = &shared.services.safety {
                    s.prune();
                }
                interval.tick().await;
            }
        })
    };

    while let Some(event) = events.recv().await {
        match event {
            Event::InboundRequest {
                peer,
                request,
                channel,
            } => {
                let response = handle_request(&shared, peer, request).await;
                shared.handle.respond(channel, response).await;
            }
            Event::Gossip {
                topic,
                source,
                id,
                data,
            } => {
                let verdict = handle_gossip(&shared, &topic, source, &data).await;
                shared.handle.report_gossip(id, source, verdict).await;
            }
            Event::PeerVerified {
                peer,
                roles,
                operator,
            } => {
                debug!(%peer, ?roles, operator, "verified");
                shared.announces.note_operator(peer, &operator);
                if let Some(blob) = &shared.services.blob {
                    blob.on_peer_verified(peer, &roles);
                }
            }
            Event::PeerRejected { peer, reason } => {
                info!(%peer, reason, "rejected");
            }
            Event::PeerDisconnected(peer) => {
                if let Some(blob) = &shared.services.blob {
                    blob.on_peer_lost(peer);
                }
            }
            Event::ExternalAddr(_) | Event::Listening(_) => {
                // Re-announce soon with the new address set.
                let shared = shared.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    publish_self(&shared).await;
                });
            }
        }
    }
    announcer.abort();
}

async fn publish_self(shared: &Arc<Shared>) {
    let Some(stats) = shared.handle.stats().await else {
        return;
    };
    let mut addrs: Vec<String> = stats.external_addrs.clone();
    for a in &shared.config.announce_addrs {
        if !addrs.contains(a) {
            addrs.push(a.clone());
        }
    }
    // A node that has learned no external address yet still announces its
    // roles so peers already connected to it know what it serves; they
    // have its address from the connection.
    let addrs: Vec<String> = addrs
        .into_iter()
        .map(|a| {
            if a.contains("/p2p/") {
                a
            } else {
                format!("{a}/p2p/{}", stats.peer_id)
            }
        })
        .collect();

    let (turn, sfu) = crate::calls::announce_info(&shared.config);

    let me = SelfAnnounce {
        roles: shared.config.roles.clone(),
        addrs,
        operator_address: shared.config.operator_address.clone(),
        declared_storage_bytes: shared.config.storage_quota_bytes,
        turn,
        sfu,
    };
    let Ok(a) = me.sign(&shared.identity, &shared.announce_signer) else {
        warn!("could not sign self-announcement");
        return;
    };
    let gossip = pb::Gossip {
        body: Some(pb::gossip::Body::NodeAnnounce(a)),
    };
    if let Err(e) = shared
        .handle
        .publish(
            &topics::announce(&shared.identity.network_id),
            gossip.encode_to_vec(),
        )
        .await
    {
        debug!(
            error = e,
            "self-announcement not published (no peers yet is normal)"
        );
    }
}

async fn handle_request(shared: &Arc<Shared>, peer: PeerId, request: pb::Request) -> pb::Response {
    use pb::request::Body as B;
    let Some(body) = request.body else {
        return err("invalid", "empty request");
    };
    match body {
        B::Handshake(_) | B::PeerExchange(_) => err("invalid", "handled by the transport layer"),

        B::MailboxPut(_)
        | B::MailboxFetch(_)
        | B::MailboxAck(_)
        | B::KeyPackagePublish(_)
        | B::KeyPackageFetch(_) => match &shared.services.mailbox {
            Some(m) => m.handle(shared, peer, body).await,
            None => err("unsupported", "this node does not serve the store role"),
        },

        B::BlobGetManifest(_)
        | B::BlobGetChunk(_)
        | B::BlobHas(_)
        | B::BlobPutManifest(_)
        | B::BlobPutChunk(_) => match &shared.services.blob {
            Some(b) => b.handle(shared, peer, body).await,
            None => err(
                "unsupported",
                "this node does not serve the store or media role",
            ),
        },

        B::EventFetch(_) | B::EventPublish(_) => match &shared.services.social {
            Some(s) => s.handle(shared, peer, body).await,
            None => err("unsupported", "this node does not keep social events"),
        },

        B::AnnounceQuery(q) => {
            let list = shared.announces.query(&q.roles, q.limit as usize);
            pb::Response {
                body: Some(pb::response::Body::AnnounceQuery(pb::AnnounceQueryResult {
                    announcements: list,
                })),
            }
        }

        B::AttestationQuery(q) => match &shared.services.safety {
            Some(s) => s.query(&q),
            None => pb::Response {
                body: Some(pb::response::Body::AttestationQuery(
                    pb::AttestationQueryResult::default(),
                )),
            },
        },

        B::TurnCredentials(req) => {
            let Some(secret) = &shared.turn_secret else {
                return err("unsupported", "this node does not issue TURN credentials");
            };
            match crate::calls::issue_for_request(shared, secret, &req) {
                Ok(c) => pb::Response {
                    body: Some(pb::response::Body::TurnCredentials(
                        pb::TurnCredentialResult {
                            issued: true,
                            reason: String::new(),
                            username: c.username,
                            password: c.password,
                            expires_at: c.expires_at,
                            uris: c.uris,
                        },
                    )),
                },
                Err(why) => {
                    shared
                        .handle
                        .score(peer, ScoreEvent::InvalidSignature)
                        .await;
                    pb::Response {
                        body: Some(pb::response::Body::TurnCredentials(
                            pb::TurnCredentialResult {
                                issued: false,
                                reason: why,
                                ..Default::default()
                            },
                        )),
                    }
                }
            }
        }

        B::ChainQuery(q) => match &shared.services.chain_relay {
            Some(r) => r.query(peer, &q).await,
            None => err(
                "unsupported",
                "this node does not relay chain queries (needs the relay or bootstrap role and a chain node)",
            ),
        },

        B::ChainBroadcast(b) => match &shared.services.chain_relay {
            Some(r) => r.broadcast(peer, &b).await,
            None => err(
                "unsupported",
                "this node does not relay chain broadcasts (needs the relay or bootstrap role and a chain node)",
            ),
        },

        B::ReceiptDeliver(d) => {
            let Some(agent) = &shared.rewards else {
                return err("unsupported", "this node is not a registered provider");
            };
            let Some(r) = d.receipt else {
                return err("invalid", "no receipt");
            };
            let (accepted, reason) = match agent.accept_receipt(peer, &r).await {
                Ok(()) => (true, String::new()),
                Err(why) => (false, why),
            };
            pb::Response {
                body: Some(pb::response::Body::ReceiptDeliver(
                    pb::ReceiptDeliverResult { accepted, reason },
                )),
            }
        }
    }
}

async fn handle_gossip(
    shared: &Arc<Shared>,
    topic: &str,
    source: PeerId,
    data: &[u8],
) -> MessageAcceptance {
    let msg: pb::Gossip = match decode_bounded(data, MAX_GOSSIP_FRAME) {
        Ok(m) => m,
        Err(e) => {
            debug!(%source, error = %e, "undecodable gossip");
            shared
                .handle
                .score(source, ScoreEvent::MalformedFrame)
                .await;
            return MessageAcceptance::Reject;
        }
    };
    let Some(body) = msg.body else {
        return MessageAcceptance::Reject;
    };
    use pb::gossip::Body as G;
    match body {
        G::NodeAnnounce(a) => {
            if topic != topics::announce(&shared.identity.network_id) {
                return MessageAcceptance::Reject;
            }
            match shared.announces.accept(&shared.identity, a) {
                Ok(_) => MessageAcceptance::Accept,
                Err(crate::announce::AnnounceError::Stale) => MessageAcceptance::Ignore,
                Err(e) => {
                    debug!(%source, error = %e, "announcement refused");
                    MessageAcceptance::Reject
                }
            }
        }
        G::Attestation(a) => match &shared.services.safety {
            Some(s) => s.accept_gossip(shared, a).await,
            None => MessageAcceptance::Ignore,
        },
        G::SocialEvent(ev) => match &shared.services.social {
            Some(s) => s.accept_gossip(shared, topic, source, ev).await,
            None => MessageAcceptance::Ignore,
        },
        G::EventAnnounce(_) => MessageAcceptance::Accept,
        G::MailboxNotify(mailbox) => match &shared.services.mailbox {
            Some(m) => m.on_notify(&mailbox),
            None => MessageAcceptance::Accept,
        },
    }
}
