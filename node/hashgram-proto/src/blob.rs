//! Content identifiers and chunking.
//!
//! A CID is BLAKE3 over the canonical manifest bytes. The manifest lists the
//! BLAKE3 hash of each chunk, so holding a CID lets a client verify a
//! manifest, and holding a manifest lets it verify every chunk, from any
//! provider, without trusting the provider.

use hashgram_net::CanonicalFixed;

use crate::keys::blake3_hash;
use crate::limits::{CHUNK_SIZE, MAX_BLOB_SIZE, MAX_CHUNKS};
use crate::pb;

/// Why a manifest is unusable.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManifestError {
    /// Version other than 1.
    #[error("unsupported manifest version {0}")]
    Version(u32),
    /// Chunk size is not the protocol's.
    #[error("chunk size {0} is not {CHUNK_SIZE}")]
    ChunkSize(u32),
    /// Size above the maximum.
    #[error("blob of {0} bytes exceeds the {MAX_BLOB_SIZE} byte maximum")]
    TooLarge(u64),
    /// The chunk list does not match the size.
    #[error("size {size} implies {expected} chunks, manifest lists {listed}")]
    ChunkCount {
        /// Declared size.
        size: u64,
        /// Chunks the size implies.
        expected: usize,
        /// Chunks listed.
        listed: usize,
    },
    /// A chunk hash is not 32 bytes.
    #[error("chunk {0} hash is not 32 bytes")]
    ChunkHash(usize),
    /// An empty blob.
    #[error("a blob must have at least one byte")]
    Empty,
}

/// Canonical bytes of a manifest.
#[must_use]
pub fn manifest_bytes(m: &pb::BlobManifest) -> Vec<u8> {
    CanonicalFixed::new(64 + 40 * m.chunks.len())
        .u32(m.version)
        .u64(m.size)
        .u32(m.chunk_size)
        .bytes64_slice(&m.chunks)
        .string(&m.mime)
        .u32(u32::from(m.encrypted))
        .preimage()
}

/// The CID of a manifest.
#[must_use]
pub fn cid(m: &pb::BlobManifest) -> [u8; 32] {
    blake3_hash(&manifest_bytes(m))
}

/// How many chunks a blob of `size` bytes has.
#[must_use]
pub fn chunk_count(size: u64) -> usize {
    size.div_ceil(CHUNK_SIZE as u64) as usize
}

/// Checks a manifest's internal consistency. Does not check content.
pub fn validate_manifest(m: &pb::BlobManifest) -> Result<(), ManifestError> {
    if m.version != 1 {
        return Err(ManifestError::Version(m.version));
    }
    if m.chunk_size as usize != CHUNK_SIZE {
        return Err(ManifestError::ChunkSize(m.chunk_size));
    }
    if m.size == 0 {
        return Err(ManifestError::Empty);
    }
    if m.size > MAX_BLOB_SIZE {
        return Err(ManifestError::TooLarge(m.size));
    }
    let expected = chunk_count(m.size);
    if expected > MAX_CHUNKS || m.chunks.len() != expected {
        return Err(ManifestError::ChunkCount {
            size: m.size,
            expected,
            listed: m.chunks.len(),
        });
    }
    for (i, c) in m.chunks.iter().enumerate() {
        if c.len() != 32 {
            return Err(ManifestError::ChunkHash(i));
        }
    }
    Ok(())
}

/// Builds a manifest from in-memory data.
///
/// For large files a streaming builder that hashes chunk by chunk is the
/// right tool; this exists for the common case of a photo or a short video
/// that fits in memory, and for tests.
pub fn manifest_for(
    data: &[u8],
    mime: &str,
    encrypted: bool,
) -> Result<pb::BlobManifest, ManifestError> {
    if data.is_empty() {
        return Err(ManifestError::Empty);
    }
    if data.len() as u64 > MAX_BLOB_SIZE {
        return Err(ManifestError::TooLarge(data.len() as u64));
    }
    let chunks = data
        .chunks(CHUNK_SIZE)
        .map(|c| blake3_hash(c).to_vec())
        .collect();
    Ok(pb::BlobManifest {
        version: 1,
        size: data.len() as u64,
        chunk_size: CHUNK_SIZE as u32,
        chunks,
        mime: mime.to_owned(),
        encrypted,
    })
}

/// The byte range of chunk `index` within a blob of `size` bytes, or `None`
/// if the index is out of range.
#[must_use]
pub fn chunk_range(size: u64, index: u32) -> Option<std::ops::Range<usize>> {
    let start = (index as u64).checked_mul(CHUNK_SIZE as u64)?;
    if start >= size {
        return None;
    }
    let end = (start + CHUNK_SIZE as u64).min(size);
    Some(start as usize..end as usize)
}

/// Verifies a chunk against its manifest entry.
#[must_use]
pub fn chunk_matches(m: &pb::BlobManifest, index: u32, data: &[u8]) -> bool {
    let Some(range) = chunk_range(m.size, index) else {
        return false;
    };
    if data.len() != range.len() {
        return false;
    }
    m.chunks
        .get(index as usize)
        .is_some_and(|h| h.as_slice() == blake3_hash(data).as_slice())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cid_is_stable_and_content_bound() {
        let a = manifest_for(b"hello", "text/plain", false).unwrap();
        let b = manifest_for(b"hello", "text/plain", false).unwrap();
        let c = manifest_for(b"hellp", "text/plain", false).unwrap();
        assert_eq!(cid(&a), cid(&b));
        assert_ne!(cid(&a), cid(&c));
        validate_manifest(&a).unwrap();
    }

    #[test]
    fn multi_chunk_manifest_is_consistent() {
        let data = vec![7u8; CHUNK_SIZE * 2 + 5];
        let m = manifest_for(&data, "application/octet-stream", true).unwrap();
        assert_eq!(m.chunks.len(), 3);
        validate_manifest(&m).unwrap();
        for i in 0..3u32 {
            let r = chunk_range(m.size, i).unwrap();
            assert!(chunk_matches(&m, i, &data[r]));
        }
        assert!(chunk_range(m.size, 3).is_none());
        assert!(!chunk_matches(&m, 0, &data[..10]));
    }

    #[test]
    fn a_manifest_lying_about_its_size_is_refused() {
        let mut m = manifest_for(b"hello", "text/plain", false).unwrap();
        m.size = CHUNK_SIZE as u64 * 5;
        assert!(matches!(
            validate_manifest(&m),
            Err(ManifestError::ChunkCount { .. })
        ));
        m.size = 0;
        assert!(matches!(validate_manifest(&m), Err(ManifestError::Empty)));
    }

    #[test]
    fn an_absurd_chunk_count_is_refused_without_allocating() {
        let m = pb::BlobManifest {
            version: 1,
            size: MAX_BLOB_SIZE + 1,
            chunk_size: CHUNK_SIZE as u32,
            chunks: vec![],
            mime: String::new(),
            encrypted: false,
        };
        assert!(matches!(
            validate_manifest(&m),
            Err(ManifestError::TooLarge(_))
        ));
    }
}
