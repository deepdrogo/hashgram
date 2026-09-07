//! Device certificates and root-key rotation, signed the way `x/identity`
//! verifies them.
//!
//! The canonical bytes here mirror `x/identity/types/certificate.go` field
//! for field. A certificate is the root key's statement "this device key
//! speaks for me until this height"; the chain checks the signature against
//! the identity's current root and rotation count, so a certificate issued
//! under a superseded root stops verifying the moment the root rotates.

use hashgram_net::{CanonicalBuf, CanonicalError, NetworkIdentity, SigningPurpose};
use hashgram_proto::Ed25519Signer;

use crate::pb::identity::{DeviceCertificate, KeyType};

/// Canonical bytes of a device certificate, excluding the signature.
pub fn certificate_bytes(
    root_address: &str,
    c: &DeviceCertificate,
) -> Result<Vec<u8>, CanonicalError> {
    CanonicalBuf::new(128)
        .string("root_address", root_address)
        .string("device_id", &c.device_id)
        .bytes("device_pubkey", &c.device_pubkey)
        .enum_value(c.device_key_type)
        .u32(c.rotation_count)
        .height(c.expiry_height)
        .finish()
}

/// Issues a certificate for an ed25519 device key, signed by the ed25519
/// root key.
pub fn issue_certificate(
    network: &NetworkIdentity,
    root: &Ed25519Signer,
    root_address: &str,
    device_id: &str,
    device_pubkey: &[u8; 32],
    rotation_count: u32,
    expiry_height: i64,
) -> Result<DeviceCertificate, CanonicalError> {
    let mut cert = DeviceCertificate {
        device_id: device_id.to_owned(),
        device_pubkey: device_pubkey.to_vec(),
        device_key_type: KeyType::Ed25519 as i32,
        rotation_count,
        expiry_height,
        signature: Vec::new(),
    };
    let payload = certificate_bytes(root_address, &cert)?;
    let digest = network.signing_digest(SigningPurpose::DeviceCert, &payload);
    cert.signature = root.sign_digest(&digest).to_vec();
    Ok(cert)
}

/// Verifies a certificate against a root public key, as the chain does.
pub fn verify_certificate(
    network: &NetworkIdentity,
    root_pubkey: &[u8],
    root_address: &str,
    c: &DeviceCertificate,
) -> Result<(), String> {
    let payload = certificate_bytes(root_address, c).map_err(|e| e.to_string())?;
    let digest = network.signing_digest(SigningPurpose::DeviceCert, &payload);
    hashgram_proto::keys::verify_ed25519(root_pubkey, &digest, &c.signature)
        .map_err(|e| e.to_string())
}

/// Canonical bytes the outgoing root signs to authorise a rotation.
pub fn rotation_bytes(
    address: &str,
    new_root_pubkey: &[u8],
    kt: KeyType,
    current_rotation: u32,
) -> Result<Vec<u8>, CanonicalError> {
    CanonicalBuf::new(128)
        .string("address", address)
        .bytes("new_root_pubkey", new_root_pubkey)
        .enum_value(kt as i32)
        .u32(current_rotation)
        .finish()
}

/// Signs a rotation with the old root.
pub fn sign_rotation(
    network: &NetworkIdentity,
    old_root: &Ed25519Signer,
    address: &str,
    new_root_pubkey: &[u8; 32],
    current_rotation: u32,
) -> Result<Vec<u8>, CanonicalError> {
    let payload = rotation_bytes(address, new_root_pubkey, KeyType::Ed25519, current_rotation)?;
    let digest = network.signing_digest(SigningPurpose::DeviceCert, &payload);
    Ok(old_root.sign_digest(&digest).to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GENESIS: &str = "9348af00681eecefb8d6329d5ba101c13bc3c8943f2c610295026f6503654287";

    #[test]
    fn certificate_bytes_match_the_go_layout() {
        // Go: String(root) String(id) Bytes(pubkey) Enum(kt) Uint32(rot) Height(exp)
        let c = DeviceCertificate {
            device_id: "d1".into(),
            device_pubkey: vec![0xab; 32],
            device_key_type: KeyType::Ed25519 as i32,
            rotation_count: 3,
            expiry_height: 1000,
            signature: vec![],
        };
        let b = certificate_bytes("hash1root", &c).unwrap();
        let mut expected = Vec::new();
        expected.extend_from_slice(&9u32.to_be_bytes());
        expected.extend_from_slice(b"hash1root");
        expected.extend_from_slice(&2u32.to_be_bytes());
        expected.extend_from_slice(b"d1");
        expected.extend_from_slice(&32u32.to_be_bytes());
        expected.extend_from_slice(&[0xab; 32]);
        expected.extend_from_slice(&2u32.to_be_bytes());
        expected.extend_from_slice(&3u32.to_be_bytes());
        expected.extend_from_slice(&1000u64.to_be_bytes());
        assert_eq!(b, expected);
    }

    #[test]
    fn issue_and_verify_round_trip_and_rotation_invalidates() {
        let net = NetworkIdentity::devnet(GENESIS);
        let root = Ed25519Signer::generate().unwrap();
        let device = Ed25519Signer::generate().unwrap();
        let cert = issue_certificate(
            &net,
            &root,
            "hash1root",
            "phone",
            &device.public_key(),
            0,
            5000,
        )
        .unwrap();
        verify_certificate(&net, &root.public_key(), "hash1root", &cert).unwrap();
        // Another root's key does not verify it.
        let other = Ed25519Signer::generate().unwrap();
        assert!(verify_certificate(&net, &other.public_key(), "hash1root", &cert).is_err());
        // A certificate for a different rotation count does not verify.
        let mut stale = cert.clone();
        stale.rotation_count = 1;
        assert!(verify_certificate(&net, &root.public_key(), "hash1root", &stale).is_err());
        // Network-bound.
        assert!(verify_certificate(
            &NetworkIdentity::mainnet(GENESIS),
            &root.public_key(),
            "hash1root",
            &cert
        )
        .is_err());
    }
}
