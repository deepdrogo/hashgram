//! Content safety attestations, as every node sees them.
//!
//! The safety *engine* — the thing that looks at public content and decides
//! — is a separate Go service with its own Unix user, no access to this
//! node's data, and a signing key of its own. What lives here is the
//! consequence: a table of signed verdicts, and the two questions the rest of
//! the node asks it: "is this blocked?" and "what do we know about this?".
//!
//! Only verdicts from attestors in `trusted_attestors` are enforced. Others
//! are kept so an operator can see them, and are not relayed, so an
//! attacker cannot use the safety topic as a free broadcast channel.
//!
//! Nothing here can name private content. An attestation carries a CID, an
//! event id or a public content hash; an E2EE envelope has none of those
//! visible to anyone but its recipients.

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Context;
use hashgram_p2p::{topics, MessageAcceptance};
use hashgram_proto::pb;
use hashgram_proto::{signing, validate};
use prost::Message;
use redb::{Database, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition};
use tracing::{debug, info};

use crate::app::Shared;
use crate::store::now;

/// Attestation key: (subject_kind, subject, attestor_pubkey).
type AttestationKey<'a> = (u8, &'a [u8], &'a [u8]);
// (subject_kind, subject, attestor_pubkey) -> encoded ContentAttestation
const ATTESTATIONS: TableDefinition<AttestationKey<'static>, &[u8]> =
    TableDefinition::new("attestations");

const KIND_CID: u8 = 1;
const KIND_EVENT: u8 = 2;
const KIND_CONTENT: u8 = 3;

/// The service.
pub struct SafetyService {
    db: Database,
    trusted: HashSet<[u8; 32]>,
}

/// Summary for the operator API.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SafetyStats {
    /// Attestations held.
    pub attestations: u64,
    /// Trusted attestor keys configured.
    pub trusted_attestors: usize,
}

impl SafetyService {
    /// Opens the table with the configured trusted attestors (hex ed25519).
    pub fn open(db: Database, trusted_hex: &[String]) -> anyhow::Result<Arc<Self>> {
        let txn = db.begin_write().context("safety: begin")?;
        {
            txn.open_table(ATTESTATIONS)?;
        }
        txn.commit().context("safety: init")?;
        let mut trusted = HashSet::new();
        for h in trusted_hex {
            let bytes =
                hex::decode(h).with_context(|| format!("trusted attestor {h:?} is not hex"))?;
            let key: [u8; 32] = bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("trusted attestor {h:?} is not 32 bytes"))?;
            trusted.insert(key);
        }
        if trusted.is_empty() {
            info!("no trusted safety attestors configured; verdicts are recorded but not enforced");
        }
        Ok(Arc::new(Self { db, trusted }))
    }

    /// Whether an attestor is trusted.
    #[must_use]
    pub fn is_trusted(&self, pubkey: &[u8]) -> bool {
        pubkey
            .try_into()
            .map(|k: [u8; 32]| self.trusted.contains(&k))
            .unwrap_or(false)
    }

    fn subject(a: &pb::ContentAttestation) -> (u8, &[u8]) {
        if !a.cid.is_empty() {
            (KIND_CID, &a.cid)
        } else if !a.event_id.is_empty() {
            (KIND_EVENT, &a.event_id)
        } else {
            (KIND_CONTENT, &a.content_hash)
        }
    }

    /// Validates, verifies and records an attestation. Returns whether it
    /// came from a trusted attestor.
    pub fn accept(&self, shared: &Shared, a: &pb::ContentAttestation) -> Result<bool, String> {
        validate::attestation(a, now()).map_err(|e| e.to_string())?;
        signing::verify_attestation(&shared.identity, a).map_err(|e| e.to_string())?;
        let (kind, subject) = Self::subject(a);
        let txn = self.db.begin_write().map_err(|e| e.to_string())?;
        {
            let mut t = txn.open_table(ATTESTATIONS).map_err(|e| e.to_string())?;
            let key = (kind, subject, a.attestor_pubkey.as_slice());
            if let Some(existing) = t.get(key).map_err(|e| e.to_string())? {
                if let Ok(old) = pb::ContentAttestation::decode(existing.value()) {
                    if old.timestamp >= a.timestamp {
                        return Ok(self.is_trusted(&a.attestor_pubkey));
                    }
                }
            }
            t.insert(key, a.encode_to_vec().as_slice())
                .map_err(|e| e.to_string())?;
        }
        txn.commit().map_err(|e| e.to_string())?;
        Ok(self.is_trusted(&a.attestor_pubkey))
    }

    /// Gossip entry point.
    pub async fn accept_gossip(
        &self,
        shared: &Arc<Shared>,
        a: pb::ContentAttestation,
    ) -> MessageAcceptance {
        match self.accept(shared, &a) {
            Ok(true) => MessageAcceptance::Accept,
            Ok(false) => MessageAcceptance::Ignore,
            Err(e) => {
                debug!(error = e, "attestation refused");
                MessageAcceptance::Reject
            }
        }
    }

    /// Publishes an attestation produced by the co-located safety engine.
    pub async fn publish(
        &self,
        shared: &Arc<Shared>,
        a: pb::ContentAttestation,
    ) -> Result<(), String> {
        self.accept(shared, &a)?;
        let g = pb::Gossip {
            body: Some(pb::gossip::Body::Attestation(a)),
        };
        shared
            .handle
            .publish(
                &topics::safety(&shared.identity.network_id),
                g.encode_to_vec(),
            )
            .await
    }

    /// The effective verdict for a subject from trusted attestors: the most
    /// recent one wins. `None` when nothing trusted has been said.
    pub fn verdict(&self, kind: u8, subject: &[u8]) -> Option<pb::Verdict> {
        let txn = self.db.begin_read().ok()?;
        let t = txn.open_table(ATTESTATIONS).ok()?;
        let mut best: Option<pb::ContentAttestation> = None;
        for r in t
            .range((kind, subject, [].as_slice())..=(kind, subject, [0xffu8; 32].as_slice()))
            .ok()?
        {
            let Ok((_, v)) = r else { continue };
            let Ok(a) = pb::ContentAttestation::decode(v.value()) else {
                continue;
            };
            if !self.is_trusted(&a.attestor_pubkey) {
                continue;
            }
            if best.as_ref().is_none_or(|b| a.timestamp > b.timestamp) {
                best = Some(a);
            }
        }
        best.and_then(|a| pb::Verdict::try_from(a.verdict).ok())
    }

    /// Whether a subject is blocked by a trusted attestor.
    #[must_use]
    pub fn is_blocked_cid(&self, cid: &[u8]) -> bool {
        matches!(self.verdict(KIND_CID, cid), Some(pb::Verdict::ContentBlock))
    }

    /// Whether an event is blocked by a trusted attestor.
    #[must_use]
    pub fn is_blocked_event(&self, event_id: &[u8]) -> bool {
        matches!(
            self.verdict(KIND_EVENT, event_id),
            Some(pb::Verdict::ContentBlock)
        )
    }

    /// Everything known about some subjects, trusted or not.
    pub fn query(&self, q: &pb::AttestationQuery) -> pb::Response {
        let mut out = Vec::new();
        if let Ok(txn) = self.db.begin_read() {
            if let Ok(t) = txn.open_table(ATTESTATIONS) {
                let mut subjects: Vec<(u8, &[u8])> = q
                    .cids
                    .iter()
                    .take(100)
                    .map(|c| (KIND_CID, c.as_slice()))
                    .collect();
                subjects.extend(
                    q.event_ids
                        .iter()
                        .take(100)
                        .map(|e| (KIND_EVENT, e.as_slice())),
                );
                for (kind, subject) in subjects {
                    if let Ok(range) = t.range(
                        (kind, subject, [].as_slice())..=(kind, subject, [0xffu8; 32].as_slice()),
                    ) {
                        for r in range.flatten() {
                            if let Ok(a) = pb::ContentAttestation::decode(r.1.value()) {
                                out.push(a);
                            }
                        }
                    }
                }
            }
        }
        pb::Response {
            body: Some(pb::response::Body::AttestationQuery(
                pb::AttestationQueryResult { attestations: out },
            )),
        }
    }

    /// Drops nothing today: attestations do not expire, an UNBLOCK
    /// supersedes a BLOCK by timestamp. Kept as a hook for the periodic task.
    pub fn prune(&self) {}

    /// Statistics.
    pub fn stats(&self) -> anyhow::Result<SafetyStats> {
        let txn = self.db.begin_read()?;
        let n = txn.open_table(ATTESTATIONS)?.len()?;
        Ok(SafetyStats {
            attestations: n,
            trusted_attestors: self.trusted.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hashgram_net::NetworkIdentity;
    use hashgram_proto::Ed25519Signer;

    const GENESIS: &str = "9348af00681eecefb8d6329d5ba101c13bc3c8943f2c610295026f6503654287";

    fn open(trusted: &[String]) -> Arc<SafetyService> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "hg-safety-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        SafetyService::open(crate::store::open(&dir, "safety").unwrap(), trusted).unwrap()
    }

    fn att(
        signer: &Ed25519Signer,
        cid: &[u8],
        verdict: pb::Verdict,
        ts: u64,
    ) -> pb::ContentAttestation {
        let id = NetworkIdentity::devnet(GENESIS);
        let mut a = pb::ContentAttestation {
            version: 1,
            cid: cid.to_vec(),
            verdict: verdict as i32,
            policy: "hashgram-public-v1".into(),
            reason_code: "test".into(),
            timestamp: ts,
            ..Default::default()
        };
        signing::sign_attestation(&id, signer, &mut a).unwrap();
        a
    }

    #[test]
    fn trusted_block_is_enforced_and_unblock_supersedes() {
        let attestor = Ed25519Signer::generate().unwrap();
        let s = open(&[hex::encode(attestor.public_key())]);
        let cid = [3u8; 32];
        let t = now();
        // Signature verification needs the identity; use a minimal Shared
        // stand-in by calling the inner verification path directly.
        let id = NetworkIdentity::devnet(GENESIS);
        let store = |a: &pb::ContentAttestation| {
            validate::attestation(a, now()).unwrap();
            signing::verify_attestation(&id, a).unwrap();
            let (kind, subject) = SafetyService::subject(a);
            let txn = s.db.begin_write().unwrap();
            {
                let mut tb = txn.open_table(ATTESTATIONS).unwrap();
                tb.insert(
                    (kind, subject, a.attestor_pubkey.as_slice()),
                    a.encode_to_vec().as_slice(),
                )
                .unwrap();
            }
            txn.commit().unwrap();
        };
        store(&att(&attestor, &cid, pb::Verdict::ContentBlock, t - 10));
        assert!(s.is_blocked_cid(&cid));
        store(&att(&attestor, &cid, pb::Verdict::ContentUnblock, t));
        assert!(!s.is_blocked_cid(&cid));
        assert_eq!(s.stats().unwrap().attestations, 1);
    }

    #[test]
    fn an_untrusted_attestor_is_recorded_but_not_enforced() {
        let trusted = Ed25519Signer::generate().unwrap();
        let rogue = Ed25519Signer::generate().unwrap();
        let s = open(&[hex::encode(trusted.public_key())]);
        let cid = [4u8; 32];
        let a = att(&rogue, &cid, pb::Verdict::ContentBlock, now());
        let (kind, subject) = SafetyService::subject(&a);
        let txn = s.db.begin_write().unwrap();
        {
            let mut tb = txn.open_table(ATTESTATIONS).unwrap();
            tb.insert(
                (kind, subject, a.attestor_pubkey.as_slice()),
                a.encode_to_vec().as_slice(),
            )
            .unwrap();
        }
        txn.commit().unwrap();
        assert!(!s.is_blocked_cid(&cid));
        assert!(!s.is_trusted(&rogue.public_key()));
        let q = s.query(&pb::AttestationQuery {
            cids: vec![cid.to_vec()],
            event_ids: vec![],
        });
        match q.body {
            Some(pb::response::Body::AttestationQuery(r)) => assert_eq!(r.attestations.len(), 1),
            _ => panic!(),
        }
    }
}
