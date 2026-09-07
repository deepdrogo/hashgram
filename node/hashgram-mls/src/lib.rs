//! End-to-end encryption for Hashgram messaging: MLS (RFC 9420) via OpenMLS.
//!
//! Nothing cryptographic is invented here. OpenMLS provides the protocol;
//! this crate decides how Hashgram uses it:
//!
//! - **Ciphersuite** `MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519`, the
//!   mandatory-to-implement suite, so every client agrees without
//!   negotiation.
//! - **Credential**: a basic credential whose identity is
//!   `<address>/<hex device public key>`, and whose signature key *is* the
//!   device key registered on chain. A member's claim to be Alice is checked
//!   by asking the chain whether that key is one of Alice's devices. No
//!   certificate authority, no server-issued identity.
//! - **One group per conversation**, including 1:1 chats (a two-member
//!   group). Every device of every participant is a member, which is what
//!   makes multi-device work without any device ever sharing a private key.
//! - **State** lives in an in-memory store the caller snapshots into its
//!   encrypted vault; nothing is written to disk by this crate.
//!
//! What a relay or store node can learn from a message this crate produces:
//! its length, and that it is an MLS PrivateMessage. The group id is inside
//! the ciphertext framing as MLS defines it; the sender and content are not.

#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::panic,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
    )
)]

use std::collections::{BTreeMap, HashMap};

use ::tls_codec::{Deserialize as TlsDeserialize, Serialize as TlsSerialize};
use openmls::prelude::*;
use openmls_basic_credential::SignatureKeyPair;
use openmls_memory_storage::MemoryStorage;
use openmls_rust_crypto::RustCrypto;
use openmls_traits::OpenMlsProvider;
use serde::{Deserialize, Serialize};

/// The one ciphersuite Hashgram uses.
pub const CIPHERSUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;

/// Why an operation failed.
#[derive(Debug, thiserror::Error)]
pub enum MlsError {
    /// OpenMLS refused.
    #[error("mls: {0}")]
    Protocol(String),
    /// A wire message did not parse.
    #[error("mls message did not decode: {0}")]
    Decode(String),
    /// The group is not known to this device.
    #[error("unknown group {0}")]
    UnknownGroup(String),
    /// The message was not for this group, or was of a type we do not handle.
    #[error("unexpected message: {0}")]
    Unexpected(String),
    /// Snapshot could not be read.
    #[error("mls state snapshot is invalid: {0}")]
    Snapshot(String),
}

fn proto<E: std::fmt::Display>(e: E) -> MlsError {
    MlsError::Protocol(e.to_string())
}

/// The OpenMLS provider: RustCrypto for primitives, an in-memory store for
/// state. The store is the thing the caller snapshots.
#[derive(Default)]
pub struct Provider {
    crypto: RustCrypto,
    storage: MemoryStorage,
}

impl OpenMlsProvider for Provider {
    type CryptoProvider = RustCrypto;
    type RandProvider = RustCrypto;
    type StorageProvider = MemoryStorage;

    fn storage(&self) -> &Self::StorageProvider {
        &self.storage
    }
    fn crypto(&self) -> &Self::CryptoProvider {
        &self.crypto
    }
    fn rand(&self) -> &Self::RandProvider {
        &self.crypto
    }
}

/// Local, unencrypted-in-memory bookkeeping about a group. Persisted inside
/// the same snapshot as the MLS state, so it is encrypted at rest with it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GroupMeta {
    /// Human name, empty for a direct chat.
    pub name: String,
    /// Whether this is a two-party conversation.
    pub direct: bool,
    /// Unix seconds when this device joined or created it.
    pub joined_at: u64,
}

/// The result of processing an inbound MLS message.
#[derive(Debug)]
pub enum Inbound {
    /// Application data from a member. `sender` is the credential identity
    /// (`<address>/<hex device key>`).
    Application {
        /// Group id bytes.
        group_id: Vec<u8>,
        /// Sender's credential identity.
        sender: String,
        /// Sender's signature (device) public key.
        sender_key: Vec<u8>,
        /// Plaintext.
        data: Vec<u8>,
    },
    /// A commit was merged; membership or keys changed.
    Commit {
        /// Group id bytes.
        group_id: Vec<u8>,
        /// New epoch.
        epoch: u64,
    },
    /// A proposal was received and stored for the next commit.
    Proposal {
        /// Group id bytes.
        group_id: Vec<u8>,
    },
    /// A welcome was processed and a group joined.
    Joined {
        /// Group id bytes.
        group_id: Vec<u8>,
    },
}

/// A member of a group.
#[derive(Debug, Clone, Serialize)]
pub struct MemberInfo {
    /// Leaf index.
    pub index: u32,
    /// Credential identity, `<address>/<hex device key>`.
    pub identity: String,
    /// Signature (device) public key.
    pub signature_key: Vec<u8>,
}

impl MemberInfo {
    /// The address part of the identity.
    #[must_use]
    pub fn address(&self) -> &str {
        self.identity.split('/').next().unwrap_or("")
    }
}

/// A device's MLS client.
pub struct MlsClient {
    provider: Provider,
    signer: SignatureKeyPair,
    credential: CredentialWithKey,
    identity: String,
    groups: BTreeMap<Vec<u8>, GroupMeta>,
}

#[derive(Serialize, Deserialize)]
struct SnapshotFormat {
    version: u32,
    identity: String,
    groups: BTreeMap<String, GroupMeta>,
    /// hex key -> hex value
    store: BTreeMap<String, String>,
}

/// The credential identity string for a device.
#[must_use]
pub fn identity_for(address: &str, device_pubkey: &[u8]) -> String {
    format!("{address}/{}", hex::encode(device_pubkey))
}

/// Splits a credential identity into address and device key.
#[must_use]
pub fn parse_identity(identity: &str) -> Option<(String, Vec<u8>)> {
    let (addr, key) = identity.split_once('/')?;
    Some((addr.to_owned(), hex::decode(key).ok()?))
}

fn join_config() -> MlsGroupJoinConfig {
    MlsGroupJoinConfig::builder()
        .use_ratchet_tree_extension(true)
        .wire_format_policy(PURE_CIPHERTEXT_WIRE_FORMAT_POLICY)
        .build()
}

impl MlsClient {
    /// A client for a device: `device_seed` is the ed25519 seed whose public
    /// key is registered on chain for `address`.
    pub fn new(address: &str, device_seed: &[u8; 32]) -> Result<Self, MlsError> {
        let sk = ed25519_signing_key(device_seed);
        let public = sk.verifying_key().to_bytes().to_vec();
        let signer = SignatureKeyPair::from_raw(
            SignatureScheme::ED25519,
            device_seed.to_vec(),
            public.clone(),
        );
        let provider = Provider::default();
        signer.store(provider.storage()).map_err(proto)?;
        let identity = identity_for(address, &public);
        let credential = BasicCredential::new(identity.clone().into_bytes());
        let credential = CredentialWithKey {
            credential: credential.into(),
            signature_key: signer.public().into(),
        };
        Ok(Self {
            provider,
            signer,
            credential,
            identity,
            groups: BTreeMap::new(),
        })
    }

    /// Restores a client from a snapshot.
    pub fn restore(
        address: &str,
        device_seed: &[u8; 32],
        snapshot: &[u8],
    ) -> Result<Self, MlsError> {
        let mut c = Self::new(address, device_seed)?;
        let snap: SnapshotFormat =
            serde_json::from_slice(snapshot).map_err(|e| MlsError::Snapshot(e.to_string()))?;
        if snap.version != 1 {
            return Err(MlsError::Snapshot(format!("version {}", snap.version)));
        }
        if snap.identity != c.identity {
            return Err(MlsError::Snapshot(
                "snapshot belongs to another device".into(),
            ));
        }
        {
            let mut values = c
                .provider
                .storage
                .values
                .write()
                .map_err(|_| MlsError::Snapshot("store poisoned".into()))?;
            for (k, v) in snap.store {
                let k = hex::decode(k).map_err(|e| MlsError::Snapshot(e.to_string()))?;
                let v = hex::decode(v).map_err(|e| MlsError::Snapshot(e.to_string()))?;
                values.insert(k, v);
            }
        }
        for (gid, meta) in snap.groups {
            let gid = hex::decode(gid).map_err(|e| MlsError::Snapshot(e.to_string()))?;
            c.groups.insert(gid, meta);
        }
        Ok(c)
    }

    /// Serialises all state. The caller encrypts this: it contains every
    /// group secret this device holds.
    pub fn snapshot(&self) -> Result<Vec<u8>, MlsError> {
        let values = self
            .provider
            .storage
            .values
            .read()
            .map_err(|_| MlsError::Snapshot("store poisoned".into()))?;
        let snap = SnapshotFormat {
            version: 1,
            identity: self.identity.clone(),
            groups: self
                .groups
                .iter()
                .map(|(k, v)| (hex::encode(k), v.clone()))
                .collect(),
            store: values
                .iter()
                .map(|(k, v)| (hex::encode(k), hex::encode(v)))
                .collect(),
        };
        serde_json::to_vec(&snap).map_err(|e| MlsError::Snapshot(e.to_string()))
    }

    /// This device's credential identity.
    #[must_use]
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// The device public key.
    #[must_use]
    pub fn device_pubkey(&self) -> &[u8] {
        self.signer.public()
    }

    /// Creates a fresh key package for others to add this device with.
    /// Returns the TLS-serialised `KeyPackage`. Each call produces a new
    /// one; a key package is consumed when used in a Welcome.
    pub fn key_package(&self) -> Result<Vec<u8>, MlsError> {
        let bundle = KeyPackage::builder()
            .build(
                CIPHERSUITE,
                &self.provider,
                &self.signer,
                self.credential.clone(),
            )
            .map_err(proto)?;
        bundle.key_package().tls_serialize_detached().map_err(proto)
    }

    /// A last-resort key package: marked with the MLS last-resort extension
    /// so OpenMLS keeps its private key after use and it may be handed out
    /// again when the one-time supply runs dry. Weaker forward secrecy for
    /// the Welcome than a one-time package, which is why it is the fallback
    /// and not the norm.
    pub fn key_package_last_resort(&self) -> Result<Vec<u8>, MlsError> {
        let bundle = KeyPackage::builder()
            .mark_as_last_resort()
            .build(
                CIPHERSUITE,
                &self.provider,
                &self.signer,
                self.credential.clone(),
            )
            .map_err(proto)?;
        bundle.key_package().tls_serialize_detached().map_err(proto)
    }

    /// Creates a group. Returns the group id.
    pub fn create_group(&mut self, meta: GroupMeta) -> Result<Vec<u8>, MlsError> {
        let mut id = [0u8; 16];
        getrandom::fill(&mut id).map_err(|_| MlsError::Protocol("no randomness".into()))?;
        let mut gid = b"hashgram:".to_vec();
        gid.extend_from_slice(&id);
        let group_id = GroupId::from_slice(&gid);
        MlsGroup::builder()
            .with_group_id(group_id)
            .ciphersuite(CIPHERSUITE)
            .use_ratchet_tree_extension(true)
            .with_wire_format_policy(PURE_CIPHERTEXT_WIRE_FORMAT_POLICY)
            .build(&self.provider, &self.signer, self.credential.clone())
            .map_err(proto)?;
        self.groups.insert(gid.clone(), meta);
        Ok(gid)
    }

    fn load(&self, group_id: &[u8]) -> Result<MlsGroup, MlsError> {
        MlsGroup::load(self.provider.storage(), &GroupId::from_slice(group_id))
            .map_err(proto)?
            .ok_or_else(|| MlsError::UnknownGroup(hex::encode(group_id)))
    }

    /// Adds members by their key packages. Returns `(commit, welcome)`, both
    /// TLS-serialised `MlsMessageOut`: the commit goes to existing members,
    /// the welcome to the new ones. The commit is merged locally.
    pub fn add_members(
        &mut self,
        group_id: &[u8],
        key_packages: &[Vec<u8>],
    ) -> Result<(Vec<u8>, Vec<u8>), MlsError> {
        let mut group = self.load(group_id)?;
        let mut kps = Vec::with_capacity(key_packages.len());
        for raw in key_packages {
            let kp_in = KeyPackageIn::tls_deserialize(&mut raw.as_slice())
                .map_err(|e| MlsError::Decode(e.to_string()))?;
            let kp = kp_in
                .validate(self.provider.crypto(), ProtocolVersion::Mls10)
                .map_err(proto)?;
            kps.push(kp);
        }
        let (commit, welcome, _) = group
            .add_members(&self.provider, &self.signer, &kps)
            .map_err(proto)?;
        group.merge_pending_commit(&self.provider).map_err(proto)?;
        Ok((
            commit.to_bytes().map_err(proto)?,
            welcome.to_bytes().map_err(proto)?,
        ))
    }

    /// Removes members by leaf index. Returns the commit for the remaining
    /// members (the removed ones learn by being unable to decrypt).
    pub fn remove_members(
        &mut self,
        group_id: &[u8],
        indices: &[u32],
    ) -> Result<Vec<u8>, MlsError> {
        let mut group = self.load(group_id)?;
        let leaves: Vec<LeafNodeIndex> = indices.iter().map(|i| LeafNodeIndex::new(*i)).collect();
        let (commit, _, _) = group
            .remove_members(&self.provider, &self.signer, &leaves)
            .map_err(proto)?;
        group.merge_pending_commit(&self.provider).map_err(proto)?;
        commit.to_bytes().map_err(proto)
    }

    /// Joins a group from a Welcome (TLS-serialised `MlsMessageIn`).
    pub fn join(&mut self, welcome: &[u8], meta: GroupMeta) -> Result<Vec<u8>, MlsError> {
        let msg = MlsMessageIn::tls_deserialize(&mut &*welcome)
            .map_err(|e| MlsError::Decode(e.to_string()))?;
        let welcome = match msg.extract() {
            MlsMessageBodyIn::Welcome(w) => w,
            _ => return Err(MlsError::Unexpected("not a welcome".into())),
        };
        let staged = StagedWelcome::new_from_welcome(&self.provider, &join_config(), welcome, None)
            .map_err(proto)?;
        let group = staged.into_group(&self.provider).map_err(proto)?;
        let gid = group.group_id().as_slice().to_vec();
        self.groups.insert(gid.clone(), meta);
        Ok(gid)
    }

    /// Encrypts application data for a group. Returns the TLS-serialised
    /// `MlsMessageOut`.
    pub fn encrypt(&mut self, group_id: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, MlsError> {
        let mut group = self.load(group_id)?;
        let out = group
            .create_message(&self.provider, &self.signer, plaintext)
            .map_err(proto)?;
        out.to_bytes().map_err(proto)
    }

    /// Processes an inbound message: application data, commit or proposal.
    /// The group is identified from the message itself.
    pub fn process(&mut self, message: &[u8]) -> Result<Inbound, MlsError> {
        let msg = MlsMessageIn::tls_deserialize(&mut &*message)
            .map_err(|e| MlsError::Decode(e.to_string()))?;
        let protocol = msg
            .try_into_protocol_message()
            .map_err(|e| MlsError::Unexpected(format!("not a protocol message (welcome?): {e}")))?;
        let gid = protocol.group_id().as_slice().to_vec();
        let mut group = self.load(&gid)?;
        let processed = group
            .process_message(&self.provider, protocol)
            .map_err(proto)?;
        let sender_key = match processed.sender() {
            Sender::Member(idx) => group
                .member_at(*idx)
                .map(|m| m.signature_key.clone())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let sender = credential_identity(processed.credential());
        match processed.into_content() {
            ProcessedMessageContent::ApplicationMessage(m) => Ok(Inbound::Application {
                group_id: gid,
                sender,
                sender_key,
                data: m.into_bytes(),
            }),
            ProcessedMessageContent::StagedCommitMessage(c) => {
                group
                    .merge_staged_commit(&self.provider, *c)
                    .map_err(proto)?;
                Ok(Inbound::Commit {
                    group_id: gid,
                    epoch: group.epoch().as_u64(),
                })
            }
            ProcessedMessageContent::ProposalMessage(p) => {
                group
                    .store_pending_proposal(self.provider.storage(), *p)
                    .map_err(proto)?;
                Ok(Inbound::Proposal { group_id: gid })
            }
            ProcessedMessageContent::ExternalJoinProposalMessage(_) => Err(MlsError::Unexpected(
                "external join proposals are not accepted".into(),
            )),
            // Our own messages coming back (a store node returned a message
            // this device sent): already applied locally, nothing to do.
            ProcessedMessageContent::OwnPendingCommit
            | ProcessedMessageContent::OwnPrivateMessage => Ok(Inbound::Proposal { group_id: gid }),
        }
    }

    /// Members of a group.
    pub fn members(&self, group_id: &[u8]) -> Result<Vec<MemberInfo>, MlsError> {
        let group = self.load(group_id)?;
        Ok(group
            .members()
            .map(|m| MemberInfo {
                index: m.index.u32(),
                identity: credential_identity(&m.credential),
                signature_key: m.signature_key,
            })
            .collect())
    }

    /// Current epoch of a group.
    pub fn epoch(&self, group_id: &[u8]) -> Result<u64, MlsError> {
        Ok(self.load(group_id)?.epoch().as_u64())
    }

    /// Groups this device is in.
    #[must_use]
    pub fn groups(&self) -> &BTreeMap<Vec<u8>, GroupMeta> {
        &self.groups
    }

    /// Updates a group's local metadata.
    pub fn set_meta(&mut self, group_id: &[u8], meta: GroupMeta) {
        self.groups.insert(group_id.to_vec(), meta);
    }

    /// Member device keys other than this device's, for delivery.
    pub fn recipient_devices(&self, group_id: &[u8]) -> Result<Vec<Vec<u8>>, MlsError> {
        Ok(self
            .members(group_id)?
            .into_iter()
            .filter(|m| m.signature_key != self.signer.public())
            .map(|m| m.signature_key)
            .collect())
    }
}

fn credential_identity(c: &Credential) -> String {
    BasicCredential::try_from(c.clone())
        .map(|b| String::from_utf8_lossy(b.identity()).into_owned())
        .unwrap_or_default()
}

fn ed25519_signing_key(seed: &[u8; 32]) -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(seed)
}

/// A map from device key to the group members that use it, for delivery
/// planning across many groups.
pub type DeviceIndex = HashMap<Vec<u8>, Vec<Vec<u8>>>;

#[cfg(test)]
mod tests {
    use super::*;

    fn client(addr: &str, seed: u8) -> MlsClient {
        MlsClient::new(addr, &[seed; 32]).unwrap()
    }

    #[test]
    fn two_devices_exchange_messages_through_ciphertext_only() {
        let mut alice = client("hash1alice", 1);
        let mut bob = client("hash1bob", 2);

        let gid = alice
            .create_group(GroupMeta {
                direct: true,
                ..Default::default()
            })
            .unwrap();
        let bob_kp = bob.key_package().unwrap();
        let (_commit, welcome) = alice.add_members(&gid, &[bob_kp]).unwrap();
        let joined = bob.join(&welcome, GroupMeta::default()).unwrap();
        assert_eq!(joined, gid);

        let ct = alice.encrypt(&gid, b"hello bob").unwrap();
        // The ciphertext does not contain the plaintext or the sender's
        // address.
        assert!(!ct.windows(9).any(|w| w == b"hello bob"));
        assert!(!ct.windows(10).any(|w| w == b"hash1alice"));

        match bob.process(&ct).unwrap() {
            Inbound::Application {
                sender,
                data,
                sender_key,
                ..
            } => {
                assert_eq!(data, b"hello bob");
                assert!(sender.starts_with("hash1alice/"));
                assert_eq!(sender_key, alice.device_pubkey());
            }
            other => panic!("{other:?}"),
        }
        let reply = bob.encrypt(&gid, b"hi alice").unwrap();
        match alice.process(&reply).unwrap() {
            Inbound::Application { data, .. } => assert_eq!(data, b"hi alice"),
            other => panic!("{other:?}"),
        }
        assert_eq!(alice.members(&gid).unwrap().len(), 2);
        assert_eq!(
            alice.recipient_devices(&gid).unwrap(),
            vec![bob.device_pubkey().to_vec()]
        );
    }

    #[test]
    fn a_third_party_cannot_read_and_a_removed_member_is_cut_off() {
        let mut alice = client("hash1alice", 1);
        let mut bob = client("hash1bob", 2);
        let mut carol = client("hash1carol", 3);
        let mut eve = client("hash1eve", 9);

        let gid = alice
            .create_group(GroupMeta {
                name: "trio".into(),
                ..Default::default()
            })
            .unwrap();
        let (_c, welcome) = alice
            .add_members(
                &gid,
                &[bob.key_package().unwrap(), carol.key_package().unwrap()],
            )
            .unwrap();
        bob.join(&welcome, GroupMeta::default()).unwrap();
        carol.join(&welcome, GroupMeta::default()).unwrap();
        assert_eq!(alice.members(&gid).unwrap().len(), 3);

        let ct = alice.encrypt(&gid, b"secret").unwrap();
        assert!(eve.process(&ct).is_err(), "a non-member decrypted");
        assert!(matches!(
            bob.process(&ct).unwrap(),
            Inbound::Application { .. }
        ));
        assert!(matches!(
            carol.process(&ct).unwrap(),
            Inbound::Application { .. }
        ));

        // Remove Carol; Bob merges the commit; Carol can no longer read.
        let carol_idx = alice
            .members(&gid)
            .unwrap()
            .into_iter()
            .find(|m| m.address() == "hash1carol")
            .unwrap()
            .index;
        let commit = alice.remove_members(&gid, &[carol_idx]).unwrap();
        assert!(matches!(
            bob.process(&commit).unwrap(),
            Inbound::Commit { .. }
        ));
        let ct2 = alice.encrypt(&gid, b"after removal").unwrap();
        assert!(matches!(
            bob.process(&ct2).unwrap(),
            Inbound::Application { .. }
        ));
        assert!(carol.process(&ct2).is_err(), "a removed member decrypted");
        assert_eq!(alice.members(&gid).unwrap().len(), 2);
    }

    #[test]
    fn state_survives_a_snapshot_round_trip() {
        let mut alice = client("hash1alice", 1);
        let mut bob = client("hash1bob", 2);
        let gid = alice.create_group(GroupMeta::default()).unwrap();
        let (_c, welcome) = alice
            .add_members(&gid, &[bob.key_package().unwrap()])
            .unwrap();
        bob.join(
            &welcome,
            GroupMeta {
                direct: true,
                ..Default::default()
            },
        )
        .unwrap();

        let snap = bob.snapshot().unwrap();
        let mut bob2 = MlsClient::restore("hash1bob", &[2; 32], &snap).unwrap();
        assert!(bob2.groups().contains_key(&gid));
        assert!(bob2.groups()[&gid].direct);

        let ct = alice.encrypt(&gid, b"still here").unwrap();
        match bob2.process(&ct).unwrap() {
            Inbound::Application { data, .. } => assert_eq!(data, b"still here"),
            other => panic!("{other:?}"),
        }
        // A snapshot is bound to the device.
        assert!(MlsClient::restore("hash1bob", &[7; 32], &snap).is_err());
    }

    #[test]
    fn identity_string_round_trips() {
        let id = identity_for("hash1x", &[0xab; 32]);
        let (a, k) = parse_identity(&id).unwrap();
        assert_eq!(a, "hash1x");
        assert_eq!(k, vec![0xab; 32]);
    }
}
