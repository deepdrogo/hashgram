//! Node announcements: what this node offers, and what others say they do.
//!
//! An announcement is a signed, expiring claim. Peers keep a table of the
//! ones they have verified and answer "who serves TURN?" from it. The table
//! is discovery, not trust: a client that finds a store node here still
//! verifies every byte it receives by hash, and the chain still pays only
//! against evidence. What the table removes is the need for a fixed address
//! anywhere — `call.hashgram.io` does not exist because this does.

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use hashgram_net::NetworkIdentity;
use hashgram_p2p::PeerId;
use hashgram_proto::pb;
use hashgram_proto::{signing, validate, Ed25519Signer};

/// How long an announcement this node publishes is valid.
pub const ANNOUNCE_TTL_SECS: u64 = 3600;

/// How often this node republishes.
pub const ANNOUNCE_INTERVAL_SECS: u64 = 600;

/// Most announcements kept. Bounded so a flood of valid-but-useless
/// announcements from many keys cannot grow memory without limit.
const MAX_ENTRIES: usize = 4096;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Announcements from other nodes, by peer id.
#[derive(Default)]
pub struct AnnounceTable {
    entries: RwLock<HashMap<PeerId, pb::NodeAnnounce>>,
    /// Operator addresses learned at the handshake, for peers that have not
    /// announced yet.
    operators: RwLock<HashMap<PeerId, String>>,
}

/// Why an announcement was not accepted.
#[derive(Debug, thiserror::Error)]
pub enum AnnounceError {
    /// Shape.
    #[error(transparent)]
    Invalid(#[from] validate::ValidationError),
    /// Signature or network.
    #[error(transparent)]
    Verify(#[from] signing::VerifyError),
    /// The key does not decode to a peer id.
    #[error("node_pubkey does not decode to a peer id")]
    PeerId,
    /// Older than what we hold.
    #[error("announcement is older than the one already held")]
    Stale,
}

impl AnnounceTable {
    /// Validates, verifies and records an announcement. Returns the peer id
    /// it is for.
    pub fn accept(
        &self,
        identity: &NetworkIdentity,
        a: pb::NodeAnnounce,
    ) -> Result<PeerId, AnnounceError> {
        validate::node_announce(&a, now())?;
        signing::verify_node_announce(identity, &a)?;
        let peer = libp2p_peer_id(&a.node_pubkey).ok_or(AnnounceError::PeerId)?;
        let mut entries = self.entries.write().unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = entries.get(&peer) {
            if existing.timestamp >= a.timestamp {
                return Err(AnnounceError::Stale);
            }
        }
        if entries.len() >= MAX_ENTRIES && !entries.contains_key(&peer) {
            // Drop the soonest-expiring entry to make room.
            if let Some(victim) = entries
                .iter()
                .min_by_key(|(_, v)| v.expires_at)
                .map(|(k, _)| *k)
            {
                entries.remove(&victim);
            }
        }
        entries.insert(peer, a);
        Ok(peer)
    }

    /// Records the operator address a peer claimed at the handshake.
    pub fn note_operator(&self, peer: PeerId, operator: &str) {
        let mut ops = self.operators.write().unwrap_or_else(|e| e.into_inner());
        if operator.is_empty() {
            ops.remove(&peer);
        } else if ops.len() < MAX_ENTRIES {
            ops.insert(peer, operator.to_owned());
        }
    }

    /// The operator address of a peer, from its announcement or handshake.
    #[must_use]
    pub fn operator_of(&self, peer: &PeerId) -> Option<String> {
        if let Some(a) = self
            .entries
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(peer)
        {
            if !a.operator_address.is_empty() {
                return Some(a.operator_address.clone());
            }
        }
        self.operators
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(peer)
            .cloned()
    }

    /// Drops expired entries.
    pub fn prune(&self) {
        let t = now();
        let mut entries = self.entries.write().unwrap_or_else(|e| e.into_inner());
        entries.retain(|_, a| a.expires_at > t);
    }

    /// Announcements claiming every role in `roles` (any role if empty),
    /// newest first.
    #[must_use]
    pub fn query(&self, roles: &[String], limit: usize) -> Vec<pb::NodeAnnounce> {
        let t = now();
        let entries = self.entries.read().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<&pb::NodeAnnounce> = entries
            .values()
            .filter(|a| a.expires_at > t)
            .filter(|a| roles.iter().all(|r| a.roles.contains(r)))
            .collect();
        out.sort_by_key(|a| std::cmp::Reverse(a.timestamp));
        out.into_iter().take(limit.clamp(1, 256)).cloned().collect()
    }

    /// Number of live entries.
    #[must_use]
    pub fn len(&self) -> usize {
        let t = now();
        self.entries
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter(|a| a.expires_at > t)
            .count()
    }

    /// Whether nothing is held.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Derives the libp2p peer id from a protobuf-encoded public key.
fn libp2p_peer_id(encoded: &[u8]) -> Option<PeerId> {
    hashgram_p2p::libp2p_identity::PublicKey::try_decode_protobuf(encoded)
        .ok()
        .map(|k| k.to_peer_id())
}

/// What this node publishes about itself.
pub struct SelfAnnounce {
    /// Roles.
    pub roles: Vec<String>,
    /// Reachable addresses, with `/p2p/`.
    pub addrs: Vec<String>,
    /// Operator address on chain, if registered.
    pub operator_address: String,
    /// Declared storage.
    pub declared_storage_bytes: u64,
    /// TURN details when serving calls.
    pub turn: Option<pb::TurnInfo>,
    /// SFU details when serving calls.
    pub sfu: Option<pb::SfuInfo>,
}

impl SelfAnnounce {
    /// Builds and signs the announcement.
    pub fn sign(
        &self,
        identity: &NetworkIdentity,
        signer: &Ed25519Signer,
    ) -> Result<pb::NodeAnnounce, signing::SignError> {
        let t = now();
        let mut a = pb::NodeAnnounce {
            version: 1,
            roles: self.roles.clone(),
            addrs: self.addrs.iter().take(16).cloned().collect(),
            operator_address: self.operator_address.clone(),
            declared_storage_bytes: self.declared_storage_bytes,
            timestamp: t,
            expires_at: t + ANNOUNCE_TTL_SECS,
            turn: self.turn.clone(),
            sfu: self.sfu.clone(),
            ..Default::default()
        };
        signing::sign_node_announce(identity, signer, &mut a)?;
        Ok(a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GENESIS: &str = "9348af00681eecefb8d6329d5ba101c13bc3c8943f2c610295026f6503654287";

    fn announce(signer: &Ed25519Signer, roles: &[&str], ts_offset: u64) -> pb::NodeAnnounce {
        let id = NetworkIdentity::devnet(GENESIS);
        let mut a = SelfAnnounce {
            roles: roles.iter().map(|r| (*r).to_owned()).collect(),
            addrs: vec!["/ip4/127.0.0.1/tcp/1/p2p/12D3KooWtest".into()],
            operator_address: String::new(),
            declared_storage_bytes: 0,
            turn: None,
            sfu: None,
        }
        .sign(&id, signer)
        .unwrap();
        a.timestamp += ts_offset;
        a.expires_at += ts_offset;
        signing::sign_node_announce(&id, signer, &mut a).unwrap();
        a
    }

    #[test]
    fn accepts_verifies_and_queries_by_role() {
        let id = NetworkIdentity::devnet(GENESIS);
        let t = AnnounceTable::default();
        let s1 = Ed25519Signer::generate().unwrap();
        let s2 = Ed25519Signer::generate().unwrap();
        t.accept(&id, announce(&s1, &["store", "relay"], 0))
            .unwrap();
        t.accept(&id, announce(&s2, &["call", "turn"], 0)).unwrap();
        assert_eq!(t.query(&["store".into()], 10).len(), 1);
        assert_eq!(t.query(&["turn".into()], 10).len(), 1);
        assert_eq!(t.query(&[], 10).len(), 2);
        assert_eq!(t.query(&["store".into(), "call".into()], 10).len(), 0);
    }

    #[test]
    fn a_tampered_announcement_is_refused() {
        let id = NetworkIdentity::devnet(GENESIS);
        let t = AnnounceTable::default();
        let s = Ed25519Signer::generate().unwrap();
        let mut a = announce(&s, &["relay"], 0);
        a.roles.push("store".into());
        assert!(matches!(t.accept(&id, a), Err(AnnounceError::Verify(_))));
        assert!(t.is_empty());
    }

    #[test]
    fn a_fork_announcement_is_refused() {
        let id = NetworkIdentity::devnet(GENESIS);
        let fork = NetworkIdentity::mainnet(GENESIS);
        let t = AnnounceTable::default();
        let s = Ed25519Signer::generate().unwrap();
        let mut a = announce(&s, &["relay"], 0);
        signing::sign_node_announce(&fork, &s, &mut a).unwrap();
        assert!(t.accept(&id, a).is_err());
    }

    #[test]
    fn newer_replaces_older_and_older_is_refused() {
        let id = NetworkIdentity::devnet(GENESIS);
        let t = AnnounceTable::default();
        let s = Ed25519Signer::generate().unwrap();
        // Timestamps must not be in the future by more than the skew, so
        // build "older" by signing first with a smaller timestamp.
        let newer = announce(&s, &["relay"], 0);
        let mut older = newer.clone();
        older.timestamp -= 100;
        signing::sign_node_announce(&id, &s, &mut older).unwrap();
        t.accept(&id, older).unwrap();
        t.accept(&id, newer.clone()).unwrap();
        let mut again = newer;
        again.timestamp -= 50;
        signing::sign_node_announce(&id, &s, &mut again).unwrap();
        assert!(matches!(t.accept(&id, again), Err(AnnounceError::Stale)));
        assert_eq!(t.len(), 1);
    }
}
