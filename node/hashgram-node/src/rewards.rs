//! Proof of Useful Service, from the node's side.
//!
//! The chain pays for evidence, and this module produces it:
//!
//! - **Registration.** The node registers as a provider under an *operator*
//!   account whose key lives on this machine (hot, because it must sign
//!   challenge answers unattended) and names a *reward address* that should
//!   not. Compromising the box yields the operator key, the bond it can
//!   unbond after a delay, and nothing that has already been paid.
//! - **Storage challenges.** The chain picks a chunk of an assigned blob;
//!   the node answers with the chunk's leaf hash and Merkle path, signed by
//!   its node key. A node that does not hold the bytes cannot answer.
//! - **Receipts.** Clients that were served hand the node a signed receipt;
//!   the node batches them and submits them each epoch. The chain refuses
//!   self-traffic and replays; this module refuses the obvious cases first
//!   so it does not spend fees on receipts that will bounce.
//! - **Assignments.** If the operator is a registered assigner, the node
//!   records on chain which providers hold which blobs, which is what turns
//!   replicated bytes into paid byte-hours.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;
use hashgram_chain::pb::serviceproof as sp;
use hashgram_chain::{msgs, Client, Wallet};
use hashgram_net::{NetworkIdentity, SigningPurpose};
use hashgram_p2p::{NodeConfig, PeerId};
use hashgram_proto::pb;
use hashgram_proto::{merkle, signing, Ed25519Signer};
use prost::Message;
use redb::{Database, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition};
use serde::Deserialize;
use tracing::{debug, info, warn};

use crate::app::Shared;
use crate::blob::BlobService;

// (epoch, client_pubkey, nonce) -> encoded receipt
const PENDING: TableDefinition<(u64, &[u8], u64), &[u8]> = TableDefinition::new("pending_receipts");
// challenge id -> answered height
const ANSWERED: TableDefinition<u64, u64> = TableDefinition::new("challenges_answered");
// (cid, provider operator) -> replica index
const ASSIGNED: TableDefinition<(&[u8], &str), u32> = TableDefinition::new("assignments_made");

/// Most receipts per submission transaction.
const BATCH: usize = 100;

/// Pending-receipt key: (epoch, client key, nonce).
type PendingKey = (u64, Vec<u8>, u64);

/// The agent.
pub struct RewardsAgent {
    chain: Client,
    wallet: Wallet,
    node_signer: Ed25519Signer,
    network: NetworkIdentity,
    cfg: NodeConfig,
    db: Database,
    epoch: Mutex<(u64, Instant)>,
    is_assigner: Mutex<(Option<bool>, Instant)>,
    stats: Mutex<AgentStats>,
}

/// Counters for the operator API.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct AgentStats {
    /// Receipts accepted from clients since start.
    pub receipts_accepted: u64,
    /// Receipts refused before submission.
    pub receipts_refused: u64,
    /// Receipts the chain accepted.
    pub receipts_settled: u64,
    /// Receipts the chain rejected.
    pub receipts_rejected: u64,
    /// Challenges answered since start.
    pub challenges_answered: u64,
    /// Challenges we could not answer (blob not held).
    pub challenges_failed: u64,
    /// Assignments recorded on chain since start.
    pub assignments_made: u64,
}

/// What the operator API reports.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RewardsView {
    /// Operator address.
    pub operator: String,
    /// Reward address as configured.
    pub reward_address: String,
    /// Provider record from chain, if registered.
    pub provider: Option<serde_json::Value>,
    /// Accrued/paid rewards from chain, if any.
    pub rewards: Option<serde_json::Value>,
    /// Current epoch.
    pub epoch: u64,
    /// Receipts waiting to be submitted.
    pub pending_receipts: u64,
    /// Whether this operator may assign storage.
    pub is_assigner: bool,
    /// Counters.
    pub stats: AgentStats,
}

#[derive(Deserialize)]
struct EpochResponse {
    epoch: EpochJson,
}
#[derive(Deserialize)]
struct EpochJson {
    #[serde(default)]
    number: String,
}

#[derive(Deserialize)]
struct ChallengesResponse {
    #[serde(default)]
    challenges: Vec<ChallengeJson>,
}
#[derive(Deserialize)]
struct ChallengeJson {
    #[serde(default)]
    id: String,
    #[serde(default)]
    blob_id: String,
    #[serde(default)]
    replica_index: u32,
    #[serde(default)]
    chunk_index: u32,
    #[serde(default)]
    answered: bool,
}

#[derive(Deserialize)]
struct ParamsResponse {
    #[serde(default)]
    assigners: Vec<String>,
}

fn role_ids(roles: &[String]) -> Vec<i32> {
    let mut out = Vec::new();
    for r in roles {
        match r.as_str() {
            "store" => {
                out.push(sp::ServiceRole::Storage as i32);
                out.push(sp::ServiceRole::Media as i32);
            }
            "media" => out.push(sp::ServiceRole::Media as i32),
            "relay" | "bootstrap" => out.push(sp::ServiceRole::Relay as i32),
            "call" => out.push(sp::ServiceRole::Call as i32),
            _ => {}
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

impl RewardsAgent {
    /// Opens the agent. `operator_secret` is the 32-byte secp256k1 secret.
    pub fn open(
        db: Database,
        cfg: NodeConfig,
        network: NetworkIdentity,
        operator_secret: &[u8],
        node_signer: Ed25519Signer,
    ) -> anyhow::Result<Arc<Self>> {
        let txn = db.begin_write().context("rewards: begin")?;
        {
            txn.open_table(PENDING)?;
            txn.open_table(ANSWERED)?;
            txn.open_table(ASSIGNED)?;
        }
        txn.commit()?;
        let wallet = Wallet::from_secret(operator_secret).context("operator key")?;
        let chain = Client::new(&cfg.chain_api, &network.chain_id)?;
        Ok(Arc::new(Self {
            chain,
            wallet,
            node_signer,
            network,
            cfg,
            db,
            epoch: Mutex::new((0, Instant::now() - Duration::from_secs(3600))),
            is_assigner: Mutex::new((None, Instant::now() - Duration::from_secs(3600))),
            stats: Mutex::new(AgentStats::default()),
        }))
    }

    /// The operator address.
    #[must_use]
    pub fn operator(&self) -> String {
        self.wallet.address().to_string()
    }

    fn stat(&self, f: impl FnOnce(&mut AgentStats)) {
        f(&mut self.stats.lock().unwrap_or_else(|e| e.into_inner()));
    }

    /// The open epoch, cached for 30 seconds.
    pub async fn current_epoch(&self) -> anyhow::Result<u64> {
        {
            let e = self.epoch.lock().unwrap_or_else(|e| e.into_inner());
            if e.1.elapsed() < Duration::from_secs(30) {
                return Ok(e.0);
            }
        }
        let r: EpochResponse = serde_json::from_value(
            self.chain
                .query("hashgram/serviceproof/v1/epoch/current")
                .await?,
        )?;
        let n: u64 = r.epoch.number.parse().unwrap_or(0);
        *self.epoch.lock().unwrap_or_else(|e| e.into_inner()) = (n, Instant::now());
        Ok(n)
    }

    /// Whether the operator may assign storage, cached for 5 minutes.
    pub async fn is_assigner(&self) -> bool {
        {
            let a = self.is_assigner.lock().unwrap_or_else(|e| e.into_inner());
            if let (Some(v), at) = (a.0, a.1) {
                if at.elapsed() < Duration::from_secs(300) {
                    return v;
                }
            }
        }
        let v = match self.chain.query("hashgram/serviceproof/v1/params").await {
            Ok(v) => serde_json::from_value::<ParamsResponse>(v)
                .map(|p| p.assigners.contains(&self.operator()))
                .unwrap_or(false),
            Err(_) => false,
        };
        *self.is_assigner.lock().unwrap_or_else(|e| e.into_inner()) = (Some(v), Instant::now());
        v
    }

    /// The provider record, if registered.
    pub async fn provider(&self) -> Option<serde_json::Value> {
        let v = self
            .chain
            .query(&format!(
                "hashgram/serviceproof/v1/provider/{}",
                self.operator()
            ))
            .await
            .ok()?;
        v.get("provider").cloned().filter(|p| !p.is_null())
    }

    /// Registers as a provider if not registered and configured to.
    pub async fn ensure_registered(&self) {
        if self.provider().await.is_some() {
            info!(operator = %self.operator(), "provider is registered");
            return;
        }
        if !self.cfg.auto_register_provider {
            warn!(
                operator = %self.operator(),
                "not registered as a provider; set auto_register_provider = true and provider_bond_uhash, or register with hashgramctl"
            );
            return;
        }
        let roles = role_ids(&self.cfg.roles);
        if roles.is_empty() {
            return;
        }
        let msg = sp::MsgRegisterProvider {
            operator: self.operator(),
            reward_address: if self.cfg.reward_address.is_empty() {
                self.operator()
            } else {
                self.cfg.reward_address.clone()
            },
            node_pubkey: self.node_signer.public_key().to_vec(),
            node_key_type: sp::KeyType::Ed25519 as i32,
            roles,
            bond: vec![hashgram_chain::pb::cosmos::base::v1beta1::Coin {
                denom: "uhash".into(),
                amount: self.cfg.provider_bond_uhash.to_string(),
            }],
            declared_storage_bytes: self.cfg.storage_quota_bytes,
            declared_bandwidth_bps: 0,
            moniker: self.cfg.moniker.clone(),
        };
        match self
            .chain
            .sign_and_broadcast(
                &self.wallet,
                vec![msgs::register_provider(&msg)],
                "hashgram provider",
            )
            .await
        {
            Ok(r) => info!(
                tx = r.txhash,
                height = r.height,
                "registered as a useful-service provider"
            ),
            Err(e) => warn!(error = %e, "provider registration failed"),
        }
    }

    // -- receipts -------------------------------------------------------------

    /// Accepts a receipt from a client, or says why not.
    pub async fn accept_receipt(&self, from: PeerId, r: &pb::ServiceReceipt) -> Result<(), String> {
        let refuse = |why: String| {
            self.stat(|s| s.receipts_refused += 1);
            Err(why)
        };
        if r.provider != self.operator() {
            return refuse(format!(
                "receipt names provider {}, this node is {}",
                r.provider,
                self.operator()
            ));
        }
        if !role_ids(&self.cfg.roles).contains(&r.role) {
            return refuse(format!("this node does not offer role {}", r.role));
        }
        if r.units == 0 {
            return refuse("units must be positive".into());
        }
        if let Err(e) = signing::verify_receipt(&self.network, r) {
            return refuse(format!("signature: {e}"));
        }
        if r.client_pubkey == self.node_signer.public_key() {
            return refuse("self-traffic".into());
        }
        let epoch = self.current_epoch().await.map_err(|e| e.to_string())?;
        if r.epoch != epoch {
            return refuse(format!(
                "receipt is for epoch {} but {epoch} is open",
                r.epoch
            ));
        }
        let txn = self.db.begin_write().map_err(|e| e.to_string())?;
        {
            let mut t = txn.open_table(PENDING).map_err(|e| e.to_string())?;
            let key = (r.epoch, r.client_pubkey.as_slice(), r.nonce);
            if t.get(key).map_err(|e| e.to_string())?.is_some() {
                return refuse("duplicate nonce".into());
            }
            t.insert(key, r.encode_to_vec().as_slice())
                .map_err(|e| e.to_string())?;
        }
        txn.commit().map_err(|e| e.to_string())?;
        self.stat(|s| s.receipts_accepted += 1);
        debug!(%from, units = r.units, role = r.role, "receipt accepted");
        Ok(())
    }

    fn pending_count(&self) -> u64 {
        self.db
            .begin_read()
            .ok()
            .and_then(|t| t.open_table(PENDING).ok())
            .and_then(|t| t.len().ok())
            .unwrap_or(0)
    }

    /// Submits pending receipts for the open epoch, in batches.
    pub async fn submit_receipts(&self) {
        let Ok(epoch) = self.current_epoch().await else {
            return;
        };
        let batch: Vec<(PendingKey, pb::ServiceReceipt)> = {
            let Ok(txn) = self.db.begin_read() else {
                return;
            };
            let Ok(t) = txn.open_table(PENDING) else {
                return;
            };
            t.iter()
                .ok()
                .map(|it| {
                    it.filter_map(|r| r.ok())
                        .filter_map(|(k, v)| {
                            let (e, c, n) = k.value();
                            pb::ServiceReceipt::decode(v.value())
                                .ok()
                                .map(|r| ((e, c.to_vec(), n), r))
                        })
                        .take(BATCH)
                        .collect()
                })
                .unwrap_or_default()
        };
        if batch.is_empty() {
            return;
        }
        // Receipts for a closed epoch can never be accepted; drop them.
        let (live, stale): (Vec<_>, Vec<_>) = batch.into_iter().partition(|(k, _)| k.0 == epoch);
        if !stale.is_empty() {
            self.delete_pending(stale.iter().map(|(k, _)| k.clone()).collect());
            self.stat(|s| s.receipts_rejected += stale.len() as u64);
        }
        if live.is_empty() {
            return;
        }
        let receipts: Vec<sp::ServiceReceipt> = live
            .iter()
            .map(|(_, r)| sp::ServiceReceipt {
                provider: r.provider.clone(),
                role: r.role,
                client_pubkey: r.client_pubkey.clone(),
                client_key_type: r.client_key_type,
                epoch: r.epoch,
                nonce: r.nonce,
                units: r.units,
                expiry_height: r.expiry_height,
                client_signature: r.client_signature.clone(),
            })
            .collect();
        let n = receipts.len();
        let msg = sp::MsgSubmitReceipts {
            submitter: self.operator(),
            receipts,
        };
        match self
            .chain
            .sign_and_broadcast(
                &self.wallet,
                vec![msgs::submit_receipts(&msg)],
                "hashgram receipts",
            )
            .await
        {
            Ok(r) => {
                info!(tx = r.txhash, count = n, "receipts submitted");
                self.stat(|s| s.receipts_settled += n as u64);
                self.delete_pending(live.iter().map(|(k, _)| k.clone()).collect());
            }
            Err(e) => {
                // A batch the chain rejects wholesale is not retried forever:
                // it is dropped and counted, and the reason is logged.
                warn!(error = %e, count = n, "receipt submission rejected; dropping batch");
                self.stat(|s| s.receipts_rejected += n as u64);
                self.delete_pending(live.iter().map(|(k, _)| k.clone()).collect());
            }
        }
    }

    fn delete_pending(&self, keys: Vec<PendingKey>) {
        let Ok(txn) = self.db.begin_write() else {
            return;
        };
        {
            let Ok(mut t) = txn.open_table(PENDING) else {
                return;
            };
            for (e, c, n) in &keys {
                let _ = t.remove((*e, c.as_slice(), *n));
            }
        }
        let _ = txn.commit();
    }

    // -- challenges -------------------------------------------------------------

    /// Answers every open challenge we can.
    pub async fn answer_challenges(&self, blobs: &BlobService) {
        let Ok(v) = self
            .chain
            .query(&format!(
                "hashgram/serviceproof/v1/challenges/{}",
                self.operator()
            ))
            .await
        else {
            return;
        };
        let Ok(list) = serde_json::from_value::<ChallengesResponse>(v) else {
            return;
        };
        for ch in list.challenges.into_iter().filter(|c| !c.answered) {
            let id: u64 = ch.id.parse().unwrap_or(0);
            if self.already_answered(id) {
                continue;
            }
            let Some(cid) = base64_decode(&ch.blob_id) else {
                continue;
            };
            let Some((leaves, chunk)) = blobs.chunk_leaves_and_chunk(&cid, ch.chunk_index) else {
                warn!(
                    challenge = id,
                    cid = hex::encode(&cid),
                    "challenged for a blob this node does not hold"
                );
                self.stat(|s| s.challenges_failed += 1);
                continue;
            };
            let Some(path) = merkle::proof_from_leaves(&leaves, ch.chunk_index as usize) else {
                self.stat(|s| s.challenges_failed += 1);
                continue;
            };
            let chunk_hash = merkle::leaf_hash(&chunk);
            let payload = merkle::challenge_response_bytes(id, &chunk_hash, &path);
            let digest = self
                .network
                .signing_digest(SigningPurpose::StorageChallenge, &payload);
            let sig = self.node_signer.sign_digest(&digest);
            let msg = sp::MsgAnswerChallenge {
                operator: self.operator(),
                response: Some(sp::ChallengeResponse {
                    challenge_id: id,
                    chunk_hash: chunk_hash.to_vec(),
                    merkle_path: path.iter().map(|p| p.to_vec()).collect(),
                    node_signature: sig.to_vec(),
                }),
            };
            match self
                .chain
                .sign_and_broadcast(
                    &self.wallet,
                    vec![msgs::answer_challenge(&msg)],
                    "hashgram challenge",
                )
                .await
            {
                Ok(r) => {
                    info!(
                        challenge = id,
                        tx = r.txhash,
                        replica = ch.replica_index,
                        "storage challenge answered"
                    );
                    self.mark_answered(id, r.height);
                    self.stat(|s| s.challenges_answered += 1);
                }
                Err(e) => {
                    warn!(challenge = id, error = %e, "challenge answer rejected");
                    self.stat(|s| s.challenges_failed += 1);
                }
            }
        }
    }

    fn already_answered(&self, id: u64) -> bool {
        self.db
            .begin_read()
            .ok()
            .and_then(|t| t.open_table(ANSWERED).ok())
            .and_then(|t| t.get(id).ok().flatten().map(|_| ()))
            .is_some()
    }

    fn mark_answered(&self, id: u64, height: u64) {
        if let Ok(txn) = self.db.begin_write() {
            if let Ok(mut t) = txn.open_table(ANSWERED) {
                let _ = t.insert(id, height);
            }
            let _ = txn.commit();
        }
    }

    // -- assignments ----------------------------------------------------------------

    /// Records on chain that `provider` holds `cid`, if this operator is an
    /// assigner and it has not been recorded already.
    pub async fn assign(
        &self,
        blobs: &BlobService,
        cid: &[u8],
        provider: &str,
        replica_index: u32,
    ) {
        if !self.is_assigner().await {
            return;
        }
        if self.assignment_recorded(cid, provider) {
            return;
        }
        let Some(m) = blobs.manifest(cid) else { return };
        let Some(root) = blobs.merkle_root(cid) else {
            return;
        };
        let msg = sp::MsgAssignStorage {
            assigner: self.operator(),
            assignment: Some(sp::StorageAssignment {
                blob_id: cid.to_vec(),
                provider: provider.to_owned(),
                replica_index,
                size_bytes: m.size,
                chunk_size: m.chunk_size,
                chunk_count: m.chunks.len() as u32,
                merkle_root: root.to_vec(),
                assigned_epoch: 0,
                last_credited_epoch: 0,
                active: true,
            }),
        };
        match self
            .chain
            .sign_and_broadcast(
                &self.wallet,
                vec![msgs::assign_storage(&msg)],
                "hashgram assign",
            )
            .await
        {
            Ok(r) => {
                info!(
                    cid = hex::encode(cid),
                    provider,
                    replica_index,
                    tx = r.txhash,
                    "storage assignment recorded"
                );
                if let Ok(txn) = self.db.begin_write() {
                    if let Ok(mut t) = txn.open_table(ASSIGNED) {
                        let _ = t.insert((cid, provider), replica_index);
                    }
                    let _ = txn.commit();
                }
                self.stat(|s| s.assignments_made += 1);
            }
            Err(e) => {
                debug!(cid = hex::encode(cid), provider, error = %e, "assignment not recorded");
                // Remember refusals for existing assignments so we do not retry.
                if e.to_string().contains("already assigned") {
                    if let Ok(txn) = self.db.begin_write() {
                        if let Ok(mut t) = txn.open_table(ASSIGNED) {
                            let _ = t.insert((cid, provider), replica_index);
                        }
                        let _ = txn.commit();
                    }
                }
            }
        }
    }

    fn assignment_recorded(&self, cid: &[u8], provider: &str) -> bool {
        self.db
            .begin_read()
            .ok()
            .and_then(|t| t.open_table(ASSIGNED).ok())
            .and_then(|t| t.get((cid, provider)).ok().flatten().map(|_| ()))
            .is_some()
    }

    /// One pass over this node's complete blobs: assign replica 0 to
    /// ourselves, and known replicas to their holders. Bounded per pass.
    pub async fn assignment_pass(&self, shared: &Arc<Shared>, blobs: &BlobService, max: usize) {
        if !self.is_assigner().await {
            return;
        }
        let Ok(cids) = blobs.complete_cids() else {
            return;
        };
        let me = self.operator();
        let mut done = 0;
        for cid in cids {
            if done >= max {
                break;
            }
            if !self.assignment_recorded(&cid, &me) {
                self.assign(blobs, &cid, &me, 0).await;
                done += 1;
            }
            for (i, peer) in blobs.known_holders(&cid).into_iter().enumerate() {
                let Some(op) = shared.announces.operator_of(&peer) else {
                    continue;
                };
                if op.is_empty() || self.assignment_recorded(&cid, &op) {
                    continue;
                }
                self.assign(blobs, &cid, &op, (i + 1) as u32).await;
                done += 1;
            }
        }
    }

    /// The operator view.
    pub async fn view(&self) -> RewardsView {
        RewardsView {
            operator: self.operator(),
            reward_address: self.cfg.reward_address.clone(),
            provider: self.provider().await,
            rewards: self
                .chain
                .query(&format!(
                    "hashgram/serviceproof/v1/rewards/{}",
                    self.operator()
                ))
                .await
                .ok(),
            epoch: self.current_epoch().await.unwrap_or(0),
            pending_receipts: self.pending_count(),
            is_assigner: self.is_assigner().await,
            stats: self.stats.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        }
    }
}

/// Standard base64 decode (gateway bytes).
fn base64_decode(s: &str) -> Option<Vec<u8>> {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(s.len());
    let mut buf = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        if c == b'=' {
            break;
        }
        let v = T.iter().position(|t| *t == c)? as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_map_to_chain_roles_without_duplicates() {
        let r = role_ids(&[
            "store".into(),
            "media".into(),
            "relay".into(),
            "bootstrap".into(),
        ]);
        assert_eq!(r, vec![1, 2, 4]);
        assert!(role_ids(&["indexer".into()]).is_empty());
    }

    #[test]
    fn base64_decodes_gateway_bytes() {
        assert_eq!(base64_decode("AAEC").unwrap(), vec![0, 1, 2]);
        assert_eq!(base64_decode("+/8=").unwrap(), vec![0xfb, 0xff]);
        assert!(base64_decode("!!").is_none());
    }
}
