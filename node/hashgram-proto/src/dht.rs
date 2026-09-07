//! Kademlia provider keys.
//!
//! A provider record says "this peer holds X". The key for X is a prefixed
//! BLAKE3-sized identifier so a blob CID and a mailbox id can never collide
//! even though both are 32 bytes. Shared between nodes (which advertise)
//! and clients (which look up), so one place defines the prefixes.

use crate::keys::blake3_hash;

/// Provider key for a blob CID.
#[must_use]
pub fn blob(cid: &[u8]) -> Vec<u8> {
    let mut k = b"hashgram/blob/".to_vec();
    k.extend_from_slice(cid);
    k
}

/// Provider key for a mailbox id (BLAKE3 of a device public key).
#[must_use]
pub fn mailbox(mailbox_id: &[u8]) -> Vec<u8> {
    let mut k = b"hashgram/mailbox/".to_vec();
    k.extend_from_slice(mailbox_id);
    k
}

/// Provider key for a device's mailbox, from its public key.
#[must_use]
pub fn mailbox_for_device(device_pubkey: &[u8]) -> Vec<u8> {
    mailbox(&blake3_hash(device_pubkey))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_do_not_collide_across_kinds() {
        let x = [7u8; 32];
        assert_ne!(blob(&x), mailbox(&x));
        assert_eq!(mailbox_for_device(b"k"), mailbox(&blake3_hash(b"k")));
    }
}
