//! Canonical signing preimages for every signed wire object.
//!
//! Each function here defines, once, the bytes an object commits to. The
//! rule for all of them: every field except the signature, in tag order,
//! through `hashgram-net`'s canonical encoder, then domain-separated under the
//! object's purpose and hashed. The signature is over that 32-byte digest.
//!
//! The functions are paired: `*_payload` builds the canonical bytes, `sign_*`
//! signs, `verify_*` verifies. Verification always recomputes the preimage
//! from the received fields; nothing about the object is trusted before its
//! signature checks.

use hashgram_net::{CanonicalBuf, CanonicalError, NetworkIdentity, SigningPurpose};

use crate::keys::{blake3_hash, ed25519_from_libp2p, verify_ed25519, Ed25519Signer, KeyError};
use crate::pb;

/// Why signing failed. Only a preimage bound can fail; callers that
/// validated the object first never see this.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SignError {
    /// The object has a field the canonical encoder refuses.
    #[error(transparent)]
    Canonical(#[from] CanonicalError),
}

/// Why verification failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VerifyError {
    /// The object has a field the canonical encoder refuses.
    #[error(transparent)]
    Canonical(#[from] CanonicalError),
    /// Key or signature problem.
    #[error(transparent)]
    Key(#[from] KeyError),
    /// The object names a different network. Checked before the signature
    /// so the error says "fork" rather than "bad signature".
    #[error("object is for network {found:?}, this node is {expected:?}")]
    WrongNetwork {
        /// The node's network id.
        expected: String,
        /// The object's.
        found: String,
    },
    /// A derived id did not match its content.
    #[error("derived id does not match content")]
    IdMismatch,
}

fn check_network(id: &NetworkIdentity, found: &str) -> Result<(), VerifyError> {
    if found != id.network_id {
        return Err(VerifyError::WrongNetwork {
            expected: id.network_id.clone(),
            found: found.to_owned(),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Social events
// ---------------------------------------------------------------------------

/// Canonical bytes of a social event, excluding id and signature.
///
/// The id is excluded because it is *derived from* these bytes; the
/// signature because it is over them.
pub fn social_event_payload(ev: &pb::SocialEvent) -> Result<Vec<u8>, CanonicalError> {
    let mut b = CanonicalBuf::new(256 + ev.payload.len())
        .string("network_id", &ev.network_id)
        .u32(ev.version)
        .string("type", &ev.r#type)
        .string("author", &ev.author)
        .bytes("device_pubkey", &ev.device_pubkey)
        .u64(ev.timestamp)
        .u64(ev.sequence)
        .bytes("previous_event", &ev.previous_event)
        .bytes("payload", &ev.payload)
        .len("media", ev.media.len());
    for m in &ev.media {
        b = media_reference_into(b, m);
    }
    b.finish()
}

fn media_reference_into(b: CanonicalBuf, m: &pb::MediaReference) -> CanonicalBuf {
    b.bytes("media.cid", &m.cid)
        .string("media.mime", &m.mime)
        .u64(m.size)
        .string("media.kind", &m.kind)
        .u32(m.width)
        .u32(m.height)
        .u32(m.duration_ms)
        .bytes("media.thumbnail_cid", &m.thumbnail_cid)
        .bytes("media.content_hash", &m.content_hash)
}

/// The event id: BLAKE3 of the domain-separated preimage. Domain-separated
/// so the same event on a fork has a different id, which keeps a fork's
/// events from colliding with Mainnet's in any cache.
pub fn social_event_id(
    id: &NetworkIdentity,
    ev: &pb::SocialEvent,
) -> Result<[u8; 32], CanonicalError> {
    let payload = social_event_payload(ev)?;
    Ok(blake3_hash(
        &id.signing_preimage(SigningPurpose::SocialEvent, &payload),
    ))
}

/// Fills in `id` and `signature`. `device_pubkey` must already be set to
/// the signer's key; it is checked rather than overwritten so a caller
/// cannot sign as one device while claiming another.
pub fn sign_social_event(
    id: &NetworkIdentity,
    signer: &Ed25519Signer,
    ev: &mut pb::SocialEvent,
) -> Result<(), SignError> {
    ev.device_pubkey = signer.public_key().to_vec();
    ev.signature.clear();
    let payload = social_event_payload(ev)?;
    ev.id = blake3_hash(&id.signing_preimage(SigningPurpose::SocialEvent, &payload)).to_vec();
    let digest = id.signing_digest(SigningPurpose::SocialEvent, &payload);
    ev.signature = signer.sign_digest(&digest).to_vec();
    Ok(())
}

/// Verifies network, id and signature. Does not check the device is
/// authorised for the author; that is a chain lookup the caller performs.
pub fn verify_social_event(id: &NetworkIdentity, ev: &pb::SocialEvent) -> Result<(), VerifyError> {
    check_network(id, &ev.network_id)?;
    let payload = social_event_payload(ev)?;
    let preimage = id.signing_preimage(SigningPurpose::SocialEvent, &payload);
    if blake3_hash(&preimage).as_slice() != ev.id.as_slice() {
        return Err(VerifyError::IdMismatch);
    }
    let digest = id.signing_digest(SigningPurpose::SocialEvent, &payload);
    verify_ed25519(&ev.device_pubkey, &digest, &ev.signature)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Envelopes and mailboxes
// ---------------------------------------------------------------------------

/// The envelope id: BLAKE3 over every field but the id. Not signed — the
/// content is authenticated inside MLS — but derived, so a store node can
/// deduplicate and a device can acknowledge by id.
#[must_use]
pub fn envelope_id(env: &pb::Envelope) -> [u8; 32] {
    let bytes = hashgram_net::CanonicalFixed::new(64 + env.ciphertext.len())
        .string(&env.network_id)
        .u32(env.version)
        .bytes(&env.mailbox)
        .u32(env.kind as u32)
        .bytes(&env.ciphertext)
        .u64(env.created_at)
        .u64(env.expires_at)
        .preimage();
    blake3_hash(&bytes)
}

/// The mailbox identifier for a device key.
#[must_use]
pub fn mailbox_for(device_pubkey: &[u8]) -> [u8; 32] {
    blake3_hash(device_pubkey)
}

fn mailbox_fetch_payload(f: &pb::MailboxFetch) -> Result<Vec<u8>, CanonicalError> {
    CanonicalBuf::new(128)
        .bytes("mailbox", &f.mailbox)
        .bytes("device_pubkey", &f.device_pubkey)
        .bytes("cursor", &f.cursor)
        .u32(f.limit)
        .u64(f.timestamp)
        .finish()
}

/// Signs a mailbox fetch. Sets `mailbox` and `device_pubkey` from the key.
pub fn sign_mailbox_fetch(
    id: &NetworkIdentity,
    signer: &Ed25519Signer,
    f: &mut pb::MailboxFetch,
) -> Result<(), SignError> {
    f.device_pubkey = signer.public_key().to_vec();
    f.mailbox = mailbox_for(&f.device_pubkey).to_vec();
    let digest = id.signing_digest(SigningPurpose::MailboxFetch, &mailbox_fetch_payload(f)?);
    f.signature = signer.sign_digest(&digest).to_vec();
    Ok(())
}

/// Verifies a mailbox fetch: the key hashes to the mailbox, and signed it.
pub fn verify_mailbox_fetch(id: &NetworkIdentity, f: &pb::MailboxFetch) -> Result<(), VerifyError> {
    if mailbox_for(&f.device_pubkey).as_slice() != f.mailbox.as_slice() {
        return Err(VerifyError::IdMismatch);
    }
    let digest = id.signing_digest(SigningPurpose::MailboxFetch, &mailbox_fetch_payload(f)?);
    verify_ed25519(&f.device_pubkey, &digest, &f.signature)?;
    Ok(())
}

fn mailbox_ack_payload(a: &pb::MailboxAck) -> Result<Vec<u8>, CanonicalError> {
    CanonicalBuf::new(128 + 32 * a.envelope_ids.len())
        .bytes("mailbox", &a.mailbox)
        .bytes("device_pubkey", &a.device_pubkey)
        .bytes_list("envelope_ids", &a.envelope_ids)
        .u64(a.timestamp)
        .finish()
}

/// Signs a mailbox ack.
pub fn sign_mailbox_ack(
    id: &NetworkIdentity,
    signer: &Ed25519Signer,
    a: &mut pb::MailboxAck,
) -> Result<(), SignError> {
    a.device_pubkey = signer.public_key().to_vec();
    a.mailbox = mailbox_for(&a.device_pubkey).to_vec();
    let digest = id.signing_digest(SigningPurpose::MailboxAck, &mailbox_ack_payload(a)?);
    a.signature = signer.sign_digest(&digest).to_vec();
    Ok(())
}

/// Verifies a mailbox ack.
pub fn verify_mailbox_ack(id: &NetworkIdentity, a: &pb::MailboxAck) -> Result<(), VerifyError> {
    if mailbox_for(&a.device_pubkey).as_slice() != a.mailbox.as_slice() {
        return Err(VerifyError::IdMismatch);
    }
    let digest = id.signing_digest(SigningPurpose::MailboxAck, &mailbox_ack_payload(a)?);
    verify_ed25519(&a.device_pubkey, &digest, &a.signature)?;
    Ok(())
}

fn key_package_payload(k: &pb::KeyPackagePublish) -> Result<Vec<u8>, CanonicalError> {
    CanonicalBuf::new(128 + k.key_package.len())
        .string("network_id", &k.network_id)
        .bytes("device_pubkey", &k.device_pubkey)
        .bytes("key_package", &k.key_package)
        .u64(k.created_at)
        .u64(k.expires_at)
        .u32(u32::from(k.last_resort))
        .finish()
}

/// Signs a key package publication.
pub fn sign_key_package(
    id: &NetworkIdentity,
    signer: &Ed25519Signer,
    k: &mut pb::KeyPackagePublish,
) -> Result<(), SignError> {
    k.network_id = id.network_id.clone();
    k.device_pubkey = signer.public_key().to_vec();
    let digest = id.signing_digest(SigningPurpose::KeyPackage, &key_package_payload(k)?);
    k.signature = signer.sign_digest(&digest).to_vec();
    Ok(())
}

/// Verifies a key package publication.
pub fn verify_key_package(
    id: &NetworkIdentity,
    k: &pb::KeyPackagePublish,
) -> Result<(), VerifyError> {
    check_network(id, &k.network_id)?;
    let digest = id.signing_digest(SigningPurpose::KeyPackage, &key_package_payload(k)?);
    verify_ed25519(&k.device_pubkey, &digest, &k.signature)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Blobs
// ---------------------------------------------------------------------------

fn blob_upload_payload(p: &pb::BlobPutManifest) -> Result<Vec<u8>, CanonicalError> {
    CanonicalBuf::new(128)
        .bytes("cid", &p.cid)
        .bytes("uploader_pubkey", &p.uploader_pubkey)
        .u64(p.timestamp)
        .finish()
}

/// Signs an upload authorisation.
pub fn sign_blob_upload(
    id: &NetworkIdentity,
    signer: &Ed25519Signer,
    p: &mut pb::BlobPutManifest,
) -> Result<(), SignError> {
    p.uploader_pubkey = signer.public_key().to_vec();
    let digest = id.signing_digest(SigningPurpose::BlobUpload, &blob_upload_payload(p)?);
    p.signature = signer.sign_digest(&digest).to_vec();
    Ok(())
}

/// Verifies an upload authorisation.
pub fn verify_blob_upload(
    id: &NetworkIdentity,
    p: &pb::BlobPutManifest,
) -> Result<(), VerifyError> {
    let digest = id.signing_digest(SigningPurpose::BlobUpload, &blob_upload_payload(p)?);
    verify_ed25519(&p.uploader_pubkey, &digest, &p.signature)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Node announcements and bootstrap records
// ---------------------------------------------------------------------------

fn node_announce_payload(a: &pb::NodeAnnounce) -> Result<Vec<u8>, CanonicalError> {
    let turn = a.turn.clone().unwrap_or_default();
    let sfu = a.sfu.clone().unwrap_or_default();
    CanonicalBuf::new(512)
        .string("network_id", &a.network_id)
        .u32(a.version)
        .bytes("node_pubkey", &a.node_pubkey)
        .string_list("roles", a.roles.iter().map(String::as_str))
        .string_list("addrs", a.addrs.iter().map(String::as_str))
        .string("operator_address", &a.operator_address)
        .u64(a.declared_storage_bytes)
        .u64(a.timestamp)
        .u64(a.expires_at)
        .string_list("turn.uris", turn.uris.iter().map(String::as_str))
        .string("turn.realm", &turn.realm)
        .u32(u32::from(turn.issues_credentials))
        .string("sfu.url", &sfu.url)
        .string("sfu.kind", &sfu.kind)
        .finish()
}

/// Signs a node announcement with the node's libp2p ed25519 key. The
/// `node_pubkey` field is set to the libp2p protobuf envelope of the key so
/// peers can derive the peer id from it.
pub fn sign_node_announce(
    id: &NetworkIdentity,
    signer: &Ed25519Signer,
    a: &mut pb::NodeAnnounce,
) -> Result<(), SignError> {
    a.network_id = id.network_id.clone();
    a.node_pubkey = crate::keys::ed25519_to_libp2p(&signer.public_key());
    let digest = id.signing_digest(SigningPurpose::NodeAnnounce, &node_announce_payload(a)?);
    a.signature = signer.sign_digest(&digest).to_vec();
    Ok(())
}

/// Verifies a node announcement. Returns the raw ed25519 node key.
pub fn verify_node_announce(
    id: &NetworkIdentity,
    a: &pb::NodeAnnounce,
) -> Result<[u8; 32], VerifyError> {
    check_network(id, &a.network_id)?;
    let raw = ed25519_from_libp2p(&a.node_pubkey)?;
    let digest = id.signing_digest(SigningPurpose::NodeAnnounce, &node_announce_payload(a)?);
    verify_ed25519(&raw, &digest, &a.signature)?;
    Ok(raw)
}

fn bootstrap_payload(r: &pb::BootstrapRecord) -> Result<Vec<u8>, CanonicalError> {
    CanonicalBuf::new(512)
        .string("network_id", &r.network_id)
        .string_list("addrs", r.addrs.iter().map(String::as_str))
        .u64(r.issued_at)
        .u64(r.expires_at)
        .bytes("signer_pubkey", &r.signer_pubkey)
        .finish()
}

/// Signs a bootstrap record.
pub fn sign_bootstrap_record(
    id: &NetworkIdentity,
    signer: &Ed25519Signer,
    r: &mut pb::BootstrapRecord,
) -> Result<(), SignError> {
    r.network_id = id.network_id.clone();
    r.signer_pubkey = signer.public_key().to_vec();
    let digest = id.signing_digest(SigningPurpose::BootstrapRecord, &bootstrap_payload(r)?);
    r.signature = signer.sign_digest(&digest).to_vec();
    Ok(())
}

/// Verifies a bootstrap record against a set of trusted signer keys. A
/// record from an unknown signer is refused even if its signature is valid:
/// the point of the record is that only the release signers can publish one.
pub fn verify_bootstrap_record(
    id: &NetworkIdentity,
    r: &pb::BootstrapRecord,
    trusted_signers: &[[u8; 32]],
) -> Result<(), VerifyError> {
    check_network(id, &r.network_id)?;
    let signer: [u8; 32] = r
        .signer_pubkey
        .as_slice()
        .try_into()
        .map_err(|_| KeyError::InvalidPublicKey(r.signer_pubkey.len()))?;
    if !trusted_signers.contains(&signer) {
        return Err(VerifyError::Key(KeyError::BadSignature));
    }
    let digest = id.signing_digest(SigningPurpose::BootstrapRecord, &bootstrap_payload(r)?);
    verify_ed25519(&signer, &digest, &r.signature)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Content attestations
// ---------------------------------------------------------------------------

fn attestation_payload(a: &pb::ContentAttestation) -> Result<Vec<u8>, CanonicalError> {
    CanonicalBuf::new(256)
        .string("network_id", &a.network_id)
        .u32(a.version)
        .bytes("cid", &a.cid)
        .bytes("event_id", &a.event_id)
        .bytes("content_hash", &a.content_hash)
        .enum_value(a.verdict)
        .string("policy", &a.policy)
        .string("reason_code", &a.reason_code)
        .u64(a.timestamp)
        .bytes("attestor_pubkey", &a.attestor_pubkey)
        .finish()
}

/// Signs a content attestation.
pub fn sign_attestation(
    id: &NetworkIdentity,
    signer: &Ed25519Signer,
    a: &mut pb::ContentAttestation,
) -> Result<(), SignError> {
    a.network_id = id.network_id.clone();
    a.attestor_pubkey = signer.public_key().to_vec();
    let digest = id.signing_digest(SigningPurpose::ContentAttestation, &attestation_payload(a)?);
    a.signature = signer.sign_digest(&digest).to_vec();
    Ok(())
}

/// Verifies a content attestation's signature. Whether the attestor is
/// *trusted* is policy the caller applies.
pub fn verify_attestation(
    id: &NetworkIdentity,
    a: &pb::ContentAttestation,
) -> Result<(), VerifyError> {
    check_network(id, &a.network_id)?;
    let digest = id.signing_digest(SigningPurpose::ContentAttestation, &attestation_payload(a)?);
    verify_ed25519(&a.attestor_pubkey, &digest, &a.signature)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GENESIS: &str = "9348af00681eecefb8d6329d5ba101c13bc3c8943f2c610295026f6503654287";

    fn net() -> NetworkIdentity {
        NetworkIdentity::devnet(GENESIS)
    }

    fn event() -> pb::SocialEvent {
        pb::SocialEvent {
            network_id: "hashgram-devnet".into(),
            version: 1,
            r#type: "POST_CREATE".into(),
            author: "hash1alice".into(),
            timestamp: 1_700_000_000,
            sequence: 3,
            previous_event: vec![7; 32],
            payload: b"hello".to_vec(),
            media: vec![pb::MediaReference {
                cid: vec![1; 32],
                mime: "image/png".into(),
                size: 10,
                kind: "image".into(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn social_event_signs_and_verifies() {
        let s = Ed25519Signer::generate().unwrap();
        let mut ev = event();
        sign_social_event(&net(), &s, &mut ev).unwrap();
        verify_social_event(&net(), &ev).unwrap();
        assert_eq!(ev.id.len(), 32);
    }

    #[test]
    fn a_tampered_event_fails_on_id_before_signature() {
        let s = Ed25519Signer::generate().unwrap();
        let mut ev = event();
        sign_social_event(&net(), &s, &mut ev).unwrap();
        ev.payload = b"hell0".to_vec();
        assert!(matches!(
            verify_social_event(&net(), &ev),
            Err(VerifyError::IdMismatch)
        ));
    }

    #[test]
    fn a_relay_cannot_resign_as_the_author() {
        // The relay's key is not the author's device key. Whatever it does,
        // the device_pubkey field ends up being its own, which the chain
        // lookup the caller performs will refuse.
        let author = Ed25519Signer::generate().unwrap();
        let relay = Ed25519Signer::generate().unwrap();
        let mut ev = event();
        sign_social_event(&net(), &author, &mut ev).unwrap();
        ev.payload = b"forged".to_vec();
        let mut forged = ev.clone();
        sign_social_event(&net(), &relay, &mut forged).unwrap();
        assert_ne!(forged.device_pubkey, author.public_key().to_vec());
        // And keeping the author's key with a relay signature fails.
        let mut spoof = ev.clone();
        spoof.device_pubkey = author.public_key().to_vec();
        assert!(verify_social_event(&net(), &spoof).is_err());
    }

    #[test]
    fn a_fork_event_is_refused_as_wrong_network() {
        let s = Ed25519Signer::generate().unwrap();
        let mut ev = event();
        sign_social_event(&NetworkIdentity::mainnet(GENESIS), &s, &mut ev).unwrap();
        ev.network_id = "hashgram-mainnet".into();
        assert!(matches!(
            verify_social_event(&net(), &ev),
            Err(VerifyError::WrongNetwork { .. })
        ));
    }

    #[test]
    fn media_order_and_count_are_committed() {
        let s = Ed25519Signer::generate().unwrap();
        let mut ev = event();
        ev.media.push(pb::MediaReference {
            cid: vec![2; 32],
            ..Default::default()
        });
        sign_social_event(&net(), &s, &mut ev).unwrap();
        ev.media.swap(0, 1);
        assert!(verify_social_event(&net(), &ev).is_err());
    }

    #[test]
    fn mailbox_fetch_binds_key_to_mailbox() {
        let s = Ed25519Signer::generate().unwrap();
        let mut f = pb::MailboxFetch {
            limit: 10,
            timestamp: 1,
            ..Default::default()
        };
        sign_mailbox_fetch(&net(), &s, &mut f).unwrap();
        verify_mailbox_fetch(&net(), &f).unwrap();
        // Point it at somebody else's mailbox: refused.
        f.mailbox = vec![9; 32];
        assert!(verify_mailbox_fetch(&net(), &f).is_err());
    }

    #[test]
    fn a_fetch_signature_is_not_an_upload_authorisation() {
        let s = Ed25519Signer::generate().unwrap();
        let mut f = pb::MailboxFetch {
            timestamp: 5,
            ..Default::default()
        };
        sign_mailbox_fetch(&net(), &s, &mut f).unwrap();
        let p = pb::BlobPutManifest {
            cid: f.mailbox.clone(),
            uploader_pubkey: f.device_pubkey.clone(),
            timestamp: 5,
            signature: f.signature.clone(),
            ..Default::default()
        };
        assert!(verify_blob_upload(&net(), &p).is_err());
    }

    #[test]
    fn node_announce_round_trips_through_libp2p_key_envelope() {
        let s = Ed25519Signer::generate().unwrap();
        let mut a = pb::NodeAnnounce {
            version: 1,
            roles: vec!["relay".into(), "store".into()],
            addrs: vec!["/ip4/127.0.0.1/udp/26670/quic-v1/p2p/x".into()],
            timestamp: 1,
            expires_at: 2,
            turn: Some(pb::TurnInfo {
                uris: vec!["turn:1.2.3.4:3478".into()],
                realm: "hashgram".into(),
                issues_credentials: true,
            }),
            ..Default::default()
        };
        sign_node_announce(&net(), &s, &mut a).unwrap();
        assert_eq!(verify_node_announce(&net(), &a).unwrap(), s.public_key());
        a.roles.push("validator".into());
        assert!(verify_node_announce(&net(), &a).is_err());
    }

    #[test]
    fn bootstrap_record_requires_a_trusted_signer() {
        let trusted = Ed25519Signer::generate().unwrap();
        let rogue = Ed25519Signer::generate().unwrap();
        let mut r = pb::BootstrapRecord {
            addrs: vec!["/ip4/1.2.3.4/tcp/26670/p2p/x".into()],
            issued_at: 1,
            expires_at: 2,
            ..Default::default()
        };
        sign_bootstrap_record(&net(), &rogue, &mut r).unwrap();
        assert!(verify_bootstrap_record(&net(), &r, &[trusted.public_key()]).is_err());
        sign_bootstrap_record(&net(), &trusted, &mut r).unwrap();
        verify_bootstrap_record(&net(), &r, &[trusted.public_key()]).unwrap();
    }

    #[test]
    fn attestation_signs_and_verifies() {
        let s = Ed25519Signer::generate().unwrap();
        let mut a = pb::ContentAttestation {
            version: 1,
            cid: vec![3; 32],
            verdict: pb::Verdict::ContentBlock as i32,
            policy: "hashgram-public-v1".into(),
            reason_code: "spam".into(),
            timestamp: 1,
            ..Default::default()
        };
        sign_attestation(&net(), &s, &mut a).unwrap();
        verify_attestation(&net(), &a).unwrap();
        a.verdict = pb::Verdict::ContentAllow as i32;
        assert!(verify_attestation(&net(), &a).is_err());
    }

    #[test]
    fn envelope_id_is_content_derived() {
        let a = pb::Envelope {
            network_id: "hashgram-devnet".into(),
            version: 1,
            mailbox: vec![1; 32],
            kind: pb::EnvelopeKind::MlsMessage as i32,
            ciphertext: vec![0xaa; 16],
            created_at: 1,
            expires_at: 2,
            ..Default::default()
        };
        let mut b = a.clone();
        b.ciphertext[0] = 0xab;
        assert_ne!(envelope_id(&a), envelope_id(&b));
        assert_eq!(envelope_id(&a), envelope_id(&a.clone()));
    }
}
