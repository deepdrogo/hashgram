//! Paid storage leases — ADR option B (`docs/ADR_HASH_STORAGE_MARKET.md`).
//!
//! A client and a provider agree a signed [`StorageLease`]; the client pays
//! per epoch with an ordinary `MsgSend` whose memo names the lease, and
//! **verifies availability itself before every payment** (`BlobHas` plus a
//! sampled, hash-checked chunk fetch) — the paying client is its own
//! assigner and its own verifier, reusing the challenge idea of
//! `x/serviceproof` without any consensus change.
//!
//! What is implemented here: the lease model and canonical signing (app
//! purpose `storage-lease`), integer price arithmetic, the verification
//! procedure over the existing blob RPCs, payment memo construction and the
//! `MsgSend`, and local bookkeeping. What is NOT yet implemented: the
//! `LeaseOffer`/`LeaseStatus`/`LeaseTerminate` RPC bodies on
//! `/hashgram/rpc/1` and the provider-side acceptance in `hashgram-node`
//! (both are additive wire changes scheduled with protocol v1.1; until
//! then a lease is agreed out of band and the provider pins the blobs by
//! ordinary replication). [`Lease::offer`] therefore returns
//! `Unsupported` today and says so.

use hashgram_app::signing::{self, AppPurpose};
use hashgram_net::CanonicalBuf;
use hashgram_proto::blob::{chunk_matches, cid as cid_of};
use hashgram_proto::keys::Ed25519Signer;
use hashgram_proto::pb;

use crate::app::HashgramOne;
use crate::SdkError;

/// Lease format version.
pub const LEASE_VERSION: u32 = 1;
/// Blobs per lease.
pub const MAX_LEASE_BLOBS: usize = 256;
/// Epochs per lease.
pub const MAX_LEASE_EPOCHS: u64 = 365;
/// One GiB.
pub const GIB: u64 = 1 << 30;
/// Memo prefix.
pub const MEMO_PREFIX: &str = "hgl1";
/// Chunks sampled per blob: `max(2, ceil(log2(chunks)))`.
#[must_use]
pub fn sample_size(chunks: u32) -> u32 {
    let lg = 32 - chunks.max(1).leading_zeros();
    lg.max(2).min(chunks.max(1))
}
const NS: &str = "storage_lease";

/// The lease agreement.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StorageLease {
    /// 1.
    pub version: u32,
    /// Network id.
    pub network_id: String,
    /// 16 random bytes (hex).
    pub lease_id: String,
    /// Payer address.
    pub client: String,
    /// Client device key (hex).
    pub client_device_pubkey: String,
    /// Provider operator address.
    pub provider: String,
    /// Provider node key (hex).
    pub provider_node_pubkey: String,
    /// CIDs (hex).
    pub blob_ids: Vec<String>,
    /// Sum of ciphertext sizes.
    pub total_bytes: u64,
    /// Replicas promised (advisory).
    pub replication: u32,
    /// Price.
    pub price_uhash_per_gib_epoch: u64,
    /// First epoch.
    pub start_epoch: u64,
    /// Last epoch (inclusive).
    pub end_epoch: u64,
    /// Unix seconds.
    pub created_at: u64,
    /// Client signature (hex).
    pub client_signature: String,
    /// Provider signature (hex), empty until accepted.
    pub provider_signature: String,
}

impl StorageLease {
    /// Per-epoch amount: `ceil(total_bytes × price / GiB)`.
    pub fn due_uhash(&self) -> Result<u64, SdkError> {
        let n = u128::from(self.total_bytes) * u128::from(self.price_uhash_per_gib_epoch);
        let due = n.div_ceil(u128::from(GIB));
        if due == 0 {
            return Err(SdkError::Invalid("lease per-epoch due is zero".into()));
        }
        u64::try_from(due).map_err(|_| SdkError::Invalid("lease due overflows".into()))
    }

    /// Structural validation.
    pub fn validate(&self) -> Result<(), SdkError> {
        if self.version != LEASE_VERSION {
            return Err(SdkError::Unsupported(format!(
                "lease version {}",
                self.version
            )));
        }
        if hex::decode(&self.lease_id).map(|b| b.len()).unwrap_or(0) != 16 {
            return Err(SdkError::Invalid("lease_id".into()));
        }
        hashgram_app::ids::require_address("client", &self.client)?;
        hashgram_app::ids::require_address("provider", &self.provider)?;
        if self.blob_ids.is_empty() || self.blob_ids.len() > MAX_LEASE_BLOBS {
            return Err(SdkError::Invalid("blob count".into()));
        }
        for c in &self.blob_ids {
            if hex::decode(c).map(|b| b.len()).unwrap_or(0) != 32 {
                return Err(SdkError::Invalid("blob id".into()));
            }
        }
        if self.end_epoch < self.start_epoch || self.end_epoch - self.start_epoch > MAX_LEASE_EPOCHS
        {
            return Err(SdkError::Invalid("epoch range".into()));
        }
        self.due_uhash()?;
        Ok(())
    }

    /// Canonical bytes both signatures commit to (everything but the
    /// signatures, in field order).
    pub fn canonical(&self) -> Result<Vec<u8>, SdkError> {
        let mut b = CanonicalBuf::new(512)
            .u32(self.version)
            .string("network_id", &self.network_id)
            .bytes("lease_id", &hex::decode(&self.lease_id).unwrap_or_default())
            .string("client", &self.client)
            .bytes(
                "client_device_pubkey",
                &hex::decode(&self.client_device_pubkey).unwrap_or_default(),
            )
            .string("provider", &self.provider)
            .bytes(
                "provider_node_pubkey",
                &hex::decode(&self.provider_node_pubkey).unwrap_or_default(),
            )
            .len("blob_ids", self.blob_ids.len());
        for c in &self.blob_ids {
            b = b.bytes("blob_id", &hex::decode(c).unwrap_or_default());
        }
        Ok(b.u64(self.total_bytes)
            .u32(self.replication)
            .u64(self.price_uhash_per_gib_epoch)
            .u64(self.start_epoch)
            .u64(self.end_epoch)
            .u64(self.created_at)
            .finish()?)
    }

    /// Payment memo for an epoch.
    #[must_use]
    pub fn memo(&self, epoch: u64) -> String {
        format!("{MEMO_PREFIX}:{}:{epoch}", self.lease_id)
    }

    /// Parses a memo back into (lease id hex, epoch).
    #[must_use]
    pub fn parse_memo(memo: &str) -> Option<(String, u64)> {
        let mut it = memo.split(':');
        if it.next()? != MEMO_PREFIX {
            return None;
        }
        let id = it.next()?;
        let epoch = it.next()?.parse().ok()?;
        if it.next().is_some() || hex::decode(id).map(|b| b.len()).unwrap_or(0) != 16 {
            return None;
        }
        Some((id.to_owned(), epoch))
    }

    /// Signs as the client.
    pub fn sign_client(
        &mut self,
        network: &hashgram_net::NetworkIdentity,
        device: &Ed25519Signer,
    ) -> Result<(), SdkError> {
        self.client_device_pubkey = hex::encode(device.public_key());
        let payload = self.canonical()?;
        self.client_signature = hex::encode(signing::sign(
            network,
            AppPurpose::StorageLease,
            device,
            &payload,
        ));
        Ok(())
    }

    /// Verifies both present signatures.
    pub fn verify(&self, network: &hashgram_net::NetworkIdentity) -> Result<(), SdkError> {
        self.validate()?;
        if self.network_id != network.network_id {
            return Err(SdkError::Invalid("lease is for another network".into()));
        }
        let payload = self.canonical()?;
        let cpk = hex::decode(&self.client_device_pubkey)
            .map_err(|e| SdkError::Invalid(e.to_string()))?;
        let csig =
            hex::decode(&self.client_signature).map_err(|e| SdkError::Invalid(e.to_string()))?;
        signing::verify(network, AppPurpose::StorageLease, &cpk, &payload, &csig)
            .map_err(|e| SdkError::Invalid(format!("client signature: {e}")))?;
        if !self.provider_signature.is_empty() {
            let ppk = hex::decode(&self.provider_node_pubkey)
                .map_err(|e| SdkError::Invalid(e.to_string()))?;
            let psig = hex::decode(&self.provider_signature)
                .map_err(|e| SdkError::Invalid(e.to_string()))?;
            signing::verify(network, AppPurpose::StorageLease, &ppk, &payload, &psig)
                .map_err(|e| SdkError::Invalid(format!("provider signature: {e}")))?;
        }
        Ok(())
    }
}

/// Verification outcome for one epoch.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Verification {
    /// Epoch.
    pub epoch: u64,
    /// Passed.
    pub passed: bool,
    /// Blobs checked.
    pub blobs: usize,
    /// Chunks sampled.
    pub sampled: usize,
    /// Failures (cid hex, reason).
    pub failures: Vec<(String, String)>,
    /// Unix seconds.
    pub at: u64,
}

/// Local lease record.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LeaseRecord {
    /// The lease.
    pub lease: StorageLease,
    /// Provider peer id (string) to verify against.
    pub provider_peer: String,
    /// Last epoch paid.
    pub last_paid_epoch: u64,
    /// Verification history (bounded).
    pub verifications: Vec<Verification>,
    /// Payment tx hashes by epoch.
    pub payments: std::collections::BTreeMap<u64, String>,
    /// Terminated.
    pub terminated: bool,
}

/// The lease API.
pub struct Lease<'a> {
    pub(crate) one: &'a mut HashgramOne,
}

impl<'a> Lease<'a> {
    /// Builds and signs a lease for the given CIDs at a provider.
    #[allow(clippy::too_many_arguments)]
    pub fn draft(
        &mut self,
        provider: &str,
        provider_node_pubkey_hex: &str,
        blob_ids_hex: &[String],
        total_bytes: u64,
        price_uhash_per_gib_epoch: u64,
        start_epoch: u64,
        epochs: u64,
        replication: u32,
    ) -> Result<StorageLease, SdkError> {
        let device = self.one.account.device()?;
        let mut l = StorageLease {
            version: LEASE_VERSION,
            network_id: self.one.network.network_id.clone(),
            lease_id: hex::encode(hashgram_app::ids::random_id()?),
            client: self.one.account.address().to_owned(),
            client_device_pubkey: String::new(),
            provider: provider.to_owned(),
            provider_node_pubkey: provider_node_pubkey_hex.to_owned(),
            blob_ids: blob_ids_hex.to_vec(),
            total_bytes,
            replication,
            price_uhash_per_gib_epoch,
            start_epoch,
            end_epoch: start_epoch + epochs.saturating_sub(1),
            created_at: hashgram_app::ids::now_secs(),
            client_signature: String::new(),
            provider_signature: String::new(),
        };
        l.validate()?;
        l.sign_client(&self.one.network, &device)?;
        Ok(l)
    }

    /// Sends the offer to the provider. Not yet supported on the wire (see
    /// module doc); records the lease locally so verification and payment
    /// can proceed for an out-of-band agreement.
    pub async fn offer(
        &mut self,
        lease: &StorageLease,
        provider_peer: &str,
    ) -> Result<(), SdkError> {
        lease.verify(&self.one.network)?;
        let rec = LeaseRecord {
            lease: lease.clone(),
            provider_peer: provider_peer.to_owned(),
            last_paid_epoch: 0,
            verifications: Vec::new(),
            payments: Default::default(),
            terminated: false,
        };
        self.one.store.put(NS, lease.lease_id.as_bytes(), &rec)?;
        Err(SdkError::Unsupported(
            "LeaseOffer is not on the wire yet (protocol v1.1); the lease was recorded locally"
                .into(),
        ))
    }

    /// Records an out-of-band agreed lease (provider signature optional).
    pub fn record(&mut self, lease: &StorageLease, provider_peer: &str) -> Result<(), SdkError> {
        lease.verify(&self.one.network)?;
        let rec = LeaseRecord {
            lease: lease.clone(),
            provider_peer: provider_peer.to_owned(),
            last_paid_epoch: 0,
            verifications: Vec::new(),
            payments: Default::default(),
            terminated: false,
        };
        self.one.store.put(NS, lease.lease_id.as_bytes(), &rec)
    }

    /// Leases we hold.
    pub fn list(&self) -> Result<Vec<LeaseRecord>, SdkError> {
        Ok(self
            .one
            .store
            .scan::<LeaseRecord>(NS)?
            .into_iter()
            .map(|(_, r)| r)
            .collect())
    }

    /// Verifies availability at the provider for the given epoch: BlobHas
    /// for every CID plus sampled chunk fetches checked against the
    /// manifest hashes. Never pays.
    pub async fn verify_epoch(
        &mut self,
        lease_id_hex: &str,
        epoch: u64,
    ) -> Result<Verification, SdkError> {
        let mut rec: LeaseRecord = self
            .one
            .store
            .get(NS, lease_id_hex.as_bytes())?
            .ok_or_else(|| SdkError::NotFound("lease".into()))?;
        let peer: crate::PeerId = rec
            .provider_peer
            .parse()
            .map_err(|_| SdkError::Invalid("provider peer id".into()))?;
        let mut v = Verification {
            epoch,
            passed: true,
            at: hashgram_app::ids::now_secs(),
            ..Default::default()
        };
        for cid_hex in &rec.lease.blob_ids {
            let cid = hex::decode(cid_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
            v.blobs += 1;
            // 1. Presence.
            let has = self
                .one
                .link
                .request(
                    peer,
                    pb::request::Body::BlobHas(pb::BlobHas { cid: cid.clone() }),
                )
                .await;
            let (present, total) = match has {
                Ok(pb::response::Body::BlobHas(h)) if h.has_manifest => {
                    (h.chunks_present, h.chunks_total)
                }
                Ok(_) => {
                    v.failures
                        .push((cid_hex.clone(), "manifest missing".into()));
                    continue;
                }
                Err(e) => {
                    v.failures
                        .push((cid_hex.clone(), format!("unreachable: {e}")));
                    continue;
                }
            };
            if present < total {
                v.failures
                    .push((cid_hex.clone(), format!("{present}/{total} chunks present")));
                continue;
            }
            // 2. Manifest (we verify it hashes to the CID ourselves).
            let m = match self
                .one
                .link
                .request(
                    peer,
                    pb::request::Body::BlobGetManifest(pb::BlobGetManifest { cid: cid.clone() }),
                )
                .await
            {
                Ok(pb::response::Body::BlobGetManifest(r)) if r.found => {
                    r.manifest.unwrap_or_default()
                }
                _ => {
                    v.failures
                        .push((cid_hex.clone(), "manifest fetch failed".into()));
                    continue;
                }
            };
            if cid_of(&m).as_slice() != cid.as_slice() {
                v.failures
                    .push((cid_hex.clone(), "manifest does not hash to cid".into()));
                continue;
            }
            // 3. Sampled chunks from a local CSPRNG.
            let n = m.chunks.len() as u32;
            let k = sample_size(n);
            let mut picks = Vec::with_capacity(k as usize);
            let mut rnd = [0u8; 4];
            while picks.len() < k as usize && n > 0 {
                getrandom::fill(&mut rnd).map_err(|_| SdkError::Invalid("no randomness".into()))?;
                let idx = u32::from_le_bytes(rnd) % n;
                if !picks.contains(&idx) {
                    picks.push(idx);
                }
            }
            for index in picks {
                v.sampled += 1;
                match self
                    .one
                    .link
                    .request(
                        peer,
                        pb::request::Body::BlobGetChunk(pb::BlobGetChunk {
                            cid: cid.clone(),
                            index,
                        }),
                    )
                    .await
                {
                    Ok(pb::response::Body::BlobGetChunk(c))
                        if c.found && chunk_matches(&m, index, &c.data) => {}
                    Ok(pb::response::Body::BlobGetChunk(c)) if c.found => {
                        self.one
                            .link
                            .handle()
                            .score(peer, hashgram_p2p::ScoreEvent::ServedCorruptData)
                            .await;
                        v.failures
                            .push((cid_hex.clone(), format!("chunk {index} hash mismatch")));
                    }
                    _ => v
                        .failures
                        .push((cid_hex.clone(), format!("chunk {index} missing"))),
                }
            }
        }
        v.passed = v.failures.is_empty();
        rec.verifications.push(v.clone());
        if rec.verifications.len() > 400 {
            rec.verifications.remove(0);
        }
        self.one.store.put(NS, lease_id_hex.as_bytes(), &rec)?;
        Ok(v)
    }

    /// Pays an epoch **only if** the recorded verification for it passed.
    /// Returns the tx hash.
    pub async fn pay_epoch(
        &mut self,
        lease_id_hex: &str,
        epoch: u64,
        pay_to: &str,
    ) -> Result<String, SdkError> {
        let mut rec: LeaseRecord = self
            .one
            .store
            .get(NS, lease_id_hex.as_bytes())?
            .ok_or_else(|| SdkError::NotFound("lease".into()))?;
        if rec.terminated {
            return Err(SdkError::Invalid("lease terminated".into()));
        }
        if epoch < rec.lease.start_epoch || epoch > rec.lease.end_epoch {
            return Err(SdkError::Invalid("epoch outside the lease".into()));
        }
        if rec.payments.contains_key(&epoch) {
            return Err(SdkError::Invalid("epoch already paid".into()));
        }
        let passed = rec
            .verifications
            .iter()
            .rev()
            .find(|v| v.epoch == epoch)
            .map(|v| v.passed)
            .unwrap_or(false);
        if !passed {
            return Err(SdkError::Invalid(
                "epoch not verified; refusing to pay".into(),
            ));
        }
        let due = rec.lease.due_uhash()?;
        let memo = rec.lease.memo(epoch);
        let hash = self
            .one
            .wallet()
            .send(pay_to, u128::from(due), &memo)
            .await?;
        rec.payments.insert(epoch, hash.clone());
        rec.last_paid_epoch = rec.last_paid_epoch.max(epoch);
        self.one.store.put(NS, lease_id_hex.as_bytes(), &rec)?;
        Ok(hash)
    }

    /// Marks a lease terminated locally (the signed `LeaseTerminate` RPC
    /// arrives with protocol v1.1).
    pub fn terminate(&mut self, lease_id_hex: &str) -> Result<(), SdkError> {
        let mut rec: LeaseRecord = self
            .one
            .store
            .get(NS, lease_id_hex.as_bytes())?
            .ok_or_else(|| SdkError::NotFound("lease".into()))?;
        rec.terminated = true;
        self.one.store.put(NS, lease_id_hex.as_bytes(), &rec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lease() -> StorageLease {
        StorageLease {
            version: 1,
            network_id: "hashgram-devnet".into(),
            lease_id: hex::encode([7u8; 16]),
            client: "hash1e0tl2hff03hu4g3sawcjqa2p9tc4uh24e4vfl5".into(),
            client_device_pubkey: String::new(),
            provider: "hash178njz3gft77798ssh6lrqxh2hskw5dm68f2uh5".into(),
            provider_node_pubkey: hex::encode([9u8; 32]),
            blob_ids: vec![hex::encode([1u8; 32])],
            total_bytes: 3 * GIB + 1,
            replication: 1,
            price_uhash_per_gib_epoch: 100,
            start_epoch: 10,
            end_epoch: 19,
            created_at: 1,
            client_signature: String::new(),
            provider_signature: String::new(),
        }
    }

    #[test]
    fn price_is_integer_ceiling() {
        let l = lease();
        assert_eq!(l.due_uhash().unwrap(), 301);
        let mut z = l.clone();
        z.price_uhash_per_gib_epoch = 0;
        assert!(z.due_uhash().is_err());
    }

    #[test]
    fn memo_round_trip() {
        let l = lease();
        let m = l.memo(12);
        assert_eq!(StorageLease::parse_memo(&m), Some((l.lease_id.clone(), 12)));
        assert_eq!(StorageLease::parse_memo("hgl1:zz:1"), None);
        assert_eq!(StorageLease::parse_memo("x:aa:1"), None);
    }

    #[test]
    fn sign_and_verify() {
        let net = hashgram_net::NetworkIdentity::devnet("0".repeat(64));
        let dev = Ed25519Signer::from_secret([3; 32]);
        let mut l = lease();
        l.sign_client(&net, &dev).unwrap();
        l.verify(&net).unwrap();
        let mut t = l.clone();
        t.price_uhash_per_gib_epoch += 1;
        assert!(t.verify(&net).is_err());
        // Signing domains carry the network magic and id (the genesis hash
        // is checked at the handshake), so a Mainnet verifier refuses a
        // devnet-signed lease.
        let other = hashgram_net::NetworkIdentity::mainnet("1".repeat(64));
        let mut o = l.clone();
        o.network_id = other.network_id.clone();
        assert!(
            o.verify(&other).is_err(),
            "signed under a different network"
        );
        // Provider acceptance signature.
        let node = Ed25519Signer::from_secret([9; 32]);
        l.provider_node_pubkey = hex::encode(node.public_key());
        l.sign_client(&net, &dev).unwrap();
        let payload = l.canonical().unwrap();
        l.provider_signature = hex::encode(signing::sign(
            &net,
            AppPurpose::StorageLease,
            &node,
            &payload,
        ));
        l.verify(&net).unwrap();
    }

    #[test]
    fn samples() {
        assert_eq!(sample_size(1), 1);
        assert_eq!(sample_size(2), 2);
        assert_eq!(sample_size(4096), 13);
        assert_eq!(sample_size(3), 2);
    }
}
