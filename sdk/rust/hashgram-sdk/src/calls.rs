//! Calls: finding infrastructure, getting TURN credentials, and signalling.
//!
//! Media never touches a Hashgram node except as TURN relay bytes it cannot
//! read (WebRTC is encrypted with DTLS-SRTP end to end between the peers, or
//! peer-to-SFU). The network's job is discovery and signalling:
//!
//! 1. **Discover.** Call nodes announce TURN URIs and an optional SFU in
//!    their signed `NodeAnnounce`. [`discover`] lists them.
//! 2. **Credentials.** A device asks a call node for time-limited TURN
//!    credentials by proving it holds its device key. [`turn_credentials`].
//! 3. **Signal.** SDP offers/answers and ICE candidates travel as
//!    [`chat::CallSignal`] messages inside the conversation's MLS group, so
//!    signalling is end-to-end encrypted and authenticated like any other
//!    message. [`Messaging::send_call_signal`](crate::messaging::Messaging::send_call_signal).
//!
//! The WebRTC stack itself belongs to the application (every platform has
//! one); the SDK hands it ICE servers and a signalling channel.

use hashgram_net::NetworkIdentity;
use hashgram_p2p::PeerId;
use hashgram_proto::limits::TURN_SENTINEL_LIMIT;
use hashgram_proto::pb;
use hashgram_proto::{signing, Ed25519Signer};

use crate::link::Link;
use crate::SdkError;

/// A call node as announced.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CallNode {
    /// TURN URIs.
    pub turn_uris: Vec<String>,
    /// TURN realm.
    pub realm: String,
    /// Whether it issues credentials over the protocol.
    pub issues_credentials: bool,
    /// SFU URL, if any.
    pub sfu_url: Option<String>,
    /// Operator address, for checking the provider on chain.
    pub operator: String,
    /// Peer id derived from the announcement key, if parseable.
    pub peer: Option<String>,
    /// Multiaddrs.
    pub addrs: Vec<String>,
}

/// Lists call infrastructure known to any verified peer.
pub async fn discover(link: &Link) -> Result<Vec<CallNode>, SdkError> {
    let list = link.announcements(&["call"]).await?;
    Ok(list
        .into_iter()
        .map(|a| CallNode {
            turn_uris: a.turn.as_ref().map(|t| t.uris.clone()).unwrap_or_default(),
            realm: a.turn.as_ref().map(|t| t.realm.clone()).unwrap_or_default(),
            issues_credentials: a.turn.as_ref().is_some_and(|t| t.issues_credentials),
            sfu_url: a
                .sfu
                .as_ref()
                .map(|s| s.url.clone())
                .filter(|u| !u.is_empty()),
            operator: a.operator_address,
            peer: hashgram_p2p::libp2p_identity::PublicKey::try_decode_protobuf(&a.node_pubkey)
                .ok()
                .map(|k| k.to_peer_id().to_string()),
            addrs: a.addrs,
        })
        .collect())
}

/// ICE server entry, in the shape WebRTC stacks take.
#[derive(Debug, Clone, serde::Serialize)]
pub struct IceServer {
    /// URIs.
    pub urls: Vec<String>,
    /// Username.
    pub username: String,
    /// Credential.
    pub credential: String,
    /// Unix seconds.
    pub expires_at: u64,
}

/// Requests TURN credentials from a call node. Falls back to any verified
/// peer claiming the `call` role when `peer` is `None`.
pub async fn turn_credentials(
    link: &Link,
    network: &NetworkIdentity,
    device: &Ed25519Signer,
    peer: Option<PeerId>,
) -> Result<IceServer, SdkError> {
    let peers = match peer {
        Some(p) => vec![p],
        None => link.peers_with_role("call").await,
    };
    if peers.is_empty() {
        return Err(SdkError::Link(crate::link::LinkError::NoPeer("call")));
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // The sentinel limit is what distinguishes this preimage from a real
    // fetch: the node's fetch validator refuses it and its TURN path
    // requires it, so this signature cannot be replayed as a mailbox read
    // and a mailbox read cannot be replayed for credentials.
    let mut f = pb::MailboxFetch {
        limit: TURN_SENTINEL_LIMIT,
        timestamp: now,
        ..Default::default()
    };
    signing::sign_mailbox_fetch(network, device, &mut f)?;
    let req = pb::TurnCredentialRequest {
        device_pubkey: f.device_pubkey.clone(),
        timestamp: now,
        signature: f.signature.clone(),
    };
    let mut last = SdkError::Link(crate::link::LinkError::NoPeer("call"));
    for p in peers {
        match link
            .request(p, pb::request::Body::TurnCredentials(req.clone()))
            .await
        {
            Ok(pb::response::Body::TurnCredentials(r)) if r.issued => {
                return Ok(IceServer {
                    urls: r.uris,
                    username: r.username,
                    credential: r.password,
                    expires_at: r.expires_at,
                })
            }
            Ok(pb::response::Body::TurnCredentials(r)) => last = SdkError::Invalid(r.reason),
            Ok(_) => last = SdkError::Link(crate::link::LinkError::Unexpected),
            Err(e) => last = SdkError::Link(e),
        }
    }
    Err(last)
}
