//! The storage-challenge Merkle tree, byte-identical to
//! `x/serviceproof/types/merkle.go`.
//!
//! SHA-256 with a `0x00` prefix for leaves and `0x01` for internal nodes
//! (second-preimage domain separation, as RFC 6962 does). An odd node at any
//! level is paired with itself. A blob's chunks are the leaves; the root is
//! what the chain records in a storage assignment, and a challenge is
//! answered with the leaf hash of one chunk plus its path.

use hashgram_net::CanonicalFixed;
use sha2::{Digest, Sha256};

/// Leaf hash of a chunk.
#[must_use]
pub fn leaf_hash(chunk: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update([0x00]);
    h.update(chunk);
    h.finalize().into()
}

fn node_hash(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update([0x01]);
    h.update(left);
    h.update(right);
    h.finalize().into()
}

fn next_level(level: &[[u8; 32]]) -> Vec<[u8; 32]> {
    let mut next = Vec::with_capacity(level.len().div_ceil(2));
    let mut i = 0;
    while i < level.len() {
        let left = level.get(i).copied().unwrap_or([0; 32]);
        let right = level.get(i + 1).copied().unwrap_or(left);
        next.push(node_hash(&left, &right));
        i += 2;
    }
    next
}

/// Root over leaf hashes (already hashed chunks). An empty tree is
/// SHA-256 of nothing, as in Go.
#[must_use]
pub fn root_from_leaves(leaves: &[[u8; 32]]) -> [u8; 32] {
    if leaves.is_empty() {
        return Sha256::digest([]).into();
    }
    let mut level = leaves.to_vec();
    while level.len() > 1 {
        level = next_level(&level);
    }
    level.first().copied().unwrap_or([0; 32])
}

/// Root over chunk contents.
#[must_use]
pub fn root(chunks: &[&[u8]]) -> [u8; 32] {
    let leaves: Vec<[u8; 32]> = chunks.iter().map(|c| leaf_hash(c)).collect();
    root_from_leaves(&leaves)
}

/// Sibling path for leaf `index`, bottom up. `None` if out of range.
#[must_use]
pub fn proof_from_leaves(leaves: &[[u8; 32]], index: usize) -> Option<Vec<[u8; 32]>> {
    if index >= leaves.len() {
        return None;
    }
    let mut level = leaves.to_vec();
    let mut idx = index;
    let mut path = Vec::new();
    while level.len() > 1 {
        let mut sibling = idx ^ 1;
        if sibling >= level.len() {
            sibling = idx;
        }
        path.push(level.get(sibling).copied().unwrap_or([0; 32]));
        level = next_level(&level);
        idx /= 2;
    }
    Some(path)
}

/// Verifies a path, as the chain does.
#[must_use]
pub fn verify(
    root: &[u8; 32],
    leaf: &[u8; 32],
    path: &[[u8; 32]],
    index: u32,
    leaf_count: u32,
) -> bool {
    if leaf_count == 0 || index >= leaf_count {
        return false;
    }
    // Expected depth: number of levels above the leaves.
    let mut depth = 0u32;
    let mut n = leaf_count;
    while n > 1 {
        n = n.div_ceil(2);
        depth += 1;
    }
    if path.len() as u32 != depth {
        return false;
    }
    let mut acc = *leaf;
    let mut idx = index;
    for sib in path {
        acc = if idx & 1 == 0 {
            node_hash(&acc, sib)
        } else {
            node_hash(sib, &acc)
        };
        idx /= 2;
    }
    &acc == root
}

/// The bytes a provider signs to answer a challenge: mirrors
/// `CanonicalChallengeResponseBytes` in Go.
#[must_use]
pub fn challenge_response_bytes(
    challenge_id: u64,
    chunk_hash: &[u8],
    path: &[[u8; 32]],
) -> Vec<u8> {
    CanonicalFixed::new(64 + 40 * path.len())
        .u64(challenge_id)
        .bytes(chunk_hash)
        .bytes64_slice(path.iter().map(|p| p.as_slice()))
        .preimage()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proofs_verify_for_every_leaf_including_odd_trees() {
        for n in 1..=9usize {
            let chunks: Vec<Vec<u8>> = (0..n).map(|i| vec![i as u8; 100 + i]).collect();
            let refs: Vec<&[u8]> = chunks.iter().map(Vec::as_slice).collect();
            let r = root(&refs);
            let leaves: Vec<[u8; 32]> = refs.iter().map(|c| leaf_hash(c)).collect();
            for i in 0..n {
                let path = proof_from_leaves(&leaves, i).unwrap();
                assert!(
                    verify(&r, &leaves[i], &path, i as u32, n as u32),
                    "n={n} i={i}"
                );
                // Wrong leaf fails.
                let other = leaf_hash(b"nope");
                assert!(!verify(&r, &other, &path, i as u32, n as u32));
                // Wrong index fails when it changes the pairing side.
                if n > 1 {
                    assert!(
                        !verify(&r, &leaves[i], &path, ((i + 1) % n) as u32, n as u32)
                            || n == 2 && false
                    );
                }
            }
        }
    }

    #[test]
    fn single_leaf_root_is_its_leaf_hash_and_empty_is_sha256_of_nothing() {
        let l = leaf_hash(b"x");
        assert_eq!(root(&[b"x"]), l);
        assert_eq!(
            hex::encode(root_from_leaves(&[])),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn known_vector_two_leaves() {
        // Go: NodeHash(LeafHash("a"), LeafHash("b")). Computed independently
        // with the same prefixes; pins the byte layout.
        let a = leaf_hash(b"a");
        let b = leaf_hash(b"b");
        let mut h = Sha256::new();
        h.update([1u8]);
        h.update(a);
        h.update(b);
        let expect: [u8; 32] = h.finalize().into();
        assert_eq!(root(&[b"a", b"b"]), expect);
    }

    #[test]
    fn matches_the_go_implementation_vectors() {
        // Produced by x/serviceproof/types MerkleRoot / MerkleProof /
        // CanonicalChallengeResponseBytes over chunks ["a","b","c"], index 2,
        // challenge id 5. A drift on either side fails here.
        let chunks: [&[u8]; 3] = [b"a", b"b", b"c"];
        assert_eq!(
            hex::encode(root(&chunks)),
            "e9636069c740c9ff51625b01a0b040396d265a9b920cc6febdfa5ecc9f58ecce"
        );
        let leaves: Vec<[u8; 32]> = chunks.iter().map(|c| leaf_hash(c)).collect();
        let p = proof_from_leaves(&leaves, 2).unwrap();
        assert_eq!(
            p.iter().map(hex::encode).collect::<Vec<_>>(),
            vec![
                "597fcb31282d34654c200d3418fca5705c648ebf326ec73d8ddef11841f876d8",
                "b137985ff484fb600db93107c77b0365c80d78f5b429ded0fd97361d077999eb"
            ]
        );
        assert_eq!(
            hex::encode(challenge_response_bytes(5, &leaf_hash(b"c"), &p)),
            "00000000000000050000000000000020597fcb31282d34654c200d3418fca5705c648ebf326ec73d8ddef11841f876d8\
             00000000000000020000000000000020597fcb31282d34654c200d3418fca5705c648ebf326ec73d8ddef11841f876d8\
             0000000000000020b137985ff484fb600db93107c77b0365c80d78f5b429ded0fd97361d077999eb"
        );
    }
}
