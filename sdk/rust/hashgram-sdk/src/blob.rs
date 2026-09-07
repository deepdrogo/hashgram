//! Media: upload to store nodes, download from providers, verify by hash,
//! and encrypt private files before they leave the device.
//!
//! Private files use XChaCha20-Poly1305 with a random key and nonce. The
//! key travels inside the E2EE message that references the blob; the store
//! node holds ciphertext under a CID that names the ciphertext. Public files
//! are uploaded as they are and referenced by CID from social events.

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hashgram_net::NetworkIdentity;
use hashgram_p2p::PeerId;
use hashgram_proto::blob::{chunk_matches, cid, manifest_for};
use hashgram_proto::keys::blake3_hash;
use hashgram_proto::limits::CHUNK_SIZE;
use hashgram_proto::pb;
use hashgram_proto::{dht, signing, Ed25519Signer};
use tracing::debug;

use crate::link::Link;
use crate::SdkError;

/// A private-file key: 32-byte key and 24-byte nonce.
#[derive(Debug, Clone)]
pub struct FileKey {
    /// Key.
    pub key: [u8; 32],
    /// Nonce.
    pub nonce: [u8; 24],
}

/// Encrypts a file for private upload. Returns ciphertext and the key.
pub fn encrypt_private(plaintext: &[u8]) -> Result<(Vec<u8>, FileKey), SdkError> {
    let mut key = [0u8; 32];
    let mut nonce = [0u8; 24];
    getrandom::fill(&mut key).map_err(|_| SdkError::Invalid("no randomness".into()))?;
    getrandom::fill(&mut nonce).map_err(|_| SdkError::Invalid("no randomness".into()))?;
    let cipher = XChaCha20Poly1305::new((&key).into());
    let ct = cipher
        .encrypt(XNonce::from_slice(&nonce), plaintext)
        .map_err(|_| SdkError::Invalid("encryption failed".into()))?;
    Ok((ct, FileKey { key, nonce }))
}

/// Decrypts a private file.
pub fn decrypt_private(ciphertext: &[u8], key: &FileKey) -> Result<Vec<u8>, SdkError> {
    let cipher = XChaCha20Poly1305::new((&key.key).into());
    cipher
        .decrypt(XNonce::from_slice(&key.nonce), ciphertext)
        .map_err(|_| SdkError::Invalid("decryption failed: wrong key or altered file".into()))
}

/// The result of an upload.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Uploaded {
    /// CID, hex.
    pub cid: String,
    /// Bytes stored (ciphertext size for private).
    pub size: u64,
    /// Chunks.
    pub chunks: usize,
    /// Store peers that accepted the whole blob.
    pub providers: Vec<String>,
    /// For private uploads: hex key and nonce to put in the message.
    pub key: Option<String>,
    /// Nonce, hex.
    pub nonce: Option<String>,
    /// BLAKE3 of the plaintext, hex.
    pub plaintext_hash: String,
}

/// Uploads `data` to up to `replicas` store peers. Every chunk is pushed
/// only where the manifest was accepted.
pub async fn upload(
    link: &Link,
    network: &NetworkIdentity,
    device: &Ed25519Signer,
    data: &[u8],
    mime: &str,
    private: bool,
    replicas: usize,
) -> Result<Uploaded, SdkError> {
    let plaintext_hash = hex::encode(blake3_hash(data));
    let (bytes, key) = if private {
        let (ct, k) = encrypt_private(data)?;
        (ct, Some(k))
    } else {
        (data.to_vec(), None)
    };
    let mime = if private {
        "application/octet-stream"
    } else {
        mime
    };
    let manifest =
        manifest_for(&bytes, mime, private).map_err(|e| SdkError::Invalid(e.to_string()))?;
    let c = cid(&manifest).to_vec();

    let stores = link.peers_with_role("store").await;
    let media = link.peers_with_role("media").await;
    let mut targets: Vec<PeerId> = media;
    for s in stores {
        if !targets.contains(&s) {
            targets.push(s);
        }
    }
    if targets.is_empty() {
        return Err(SdkError::Link(crate::link::LinkError::NoPeer("store")));
    }
    let mut providers = Vec::new();
    for p in targets.into_iter().take(replicas.max(1)) {
        let mut put = pb::BlobPutManifest {
            cid: c.clone(),
            manifest: Some(manifest.clone()),
            timestamp: now(),
            ..Default::default()
        };
        signing::sign_blob_upload(network, device, &mut put)?;
        let missing = match link
            .request(p, pb::request::Body::BlobPutManifest(put))
            .await
        {
            Ok(pb::response::Body::BlobPutManifest(r)) if r.accepted => r.missing_chunks,
            Ok(pb::response::Body::BlobPutManifest(r)) => {
                debug!(%p, reason = r.reason, "manifest refused");
                continue;
            }
            Ok(_) => continue,
            Err(e) => {
                debug!(%p, error = %e, "manifest upload failed");
                continue;
            }
        };
        let mut ok = true;
        for index in missing {
            let start = index as usize * CHUNK_SIZE;
            let end = (start + CHUNK_SIZE).min(bytes.len());
            let chunk = bytes.get(start..end).unwrap_or(&[]).to_vec();
            match link
                .request(
                    p,
                    pb::request::Body::BlobPutChunk(pb::BlobPutChunk {
                        cid: c.clone(),
                        index,
                        data: chunk,
                    }),
                )
                .await
            {
                Ok(pb::response::Body::BlobPutChunk(r)) if r.accepted => {}
                other => {
                    debug!(%p, ?other, "chunk refused");
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            providers.push(p.to_string());
        }
    }
    if providers.is_empty() {
        return Err(SdkError::Delivery("no store node accepted the blob".into()));
    }
    Ok(Uploaded {
        cid: hex::encode(&c),
        size: manifest.size,
        chunks: manifest.chunks.len(),
        providers,
        key: key.as_ref().map(|k| hex::encode(k.key)),
        nonce: key.as_ref().map(|k| hex::encode(k.nonce)),
        plaintext_hash,
    })
}

/// Finds providers of a CID: DHT first, then connected store/media peers.
pub async fn providers(link: &Link, c: &[u8]) -> Vec<PeerId> {
    let mut out = link.providers(dht::blob(c)).await;
    for p in link.peers_with_role("store").await {
        if !out.contains(&p) {
            out.push(p);
        }
    }
    for p in link.peers_with_role("media").await {
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// Downloads and verifies a blob from any provider. Returns the bytes and
/// the manifest.
pub async fn download(
    link: &Link,
    c: &[u8],
) -> Result<(Vec<u8>, pb::BlobManifest, PeerId), SdkError> {
    let peers = providers(link, c).await;
    let mut last = SdkError::NotFound(hex::encode(c));
    for p in peers {
        match download_from(link, p, c).await {
            Ok((bytes, m)) => return Ok((bytes, m, p)),
            Err(e) => last = e,
        }
    }
    Err(last)
}

async fn download_from(
    link: &Link,
    p: PeerId,
    c: &[u8],
) -> Result<(Vec<u8>, pb::BlobManifest), SdkError> {
    let m = match link
        .request(
            p,
            pb::request::Body::BlobGetManifest(pb::BlobGetManifest { cid: c.to_vec() }),
        )
        .await?
    {
        pb::response::Body::BlobGetManifest(r) if r.found => r
            .manifest
            .ok_or_else(|| SdkError::NotFound(hex::encode(c)))?,
        _ => return Err(SdkError::NotFound(hex::encode(c))),
    };
    if cid(&m).as_slice() != c {
        link.handle()
            .score(p, hashgram_p2p::ScoreEvent::ServedCorruptData)
            .await;
        return Err(SdkError::Corrupt(format!(
            "{p} served a manifest that does not hash to the cid"
        )));
    }
    let mut out = Vec::with_capacity(m.size as usize);
    for index in 0..m.chunks.len() as u32 {
        let data = match link
            .request(
                p,
                pb::request::Body::BlobGetChunk(pb::BlobGetChunk {
                    cid: c.to_vec(),
                    index,
                }),
            )
            .await?
        {
            pb::response::Body::BlobGetChunk(r) if r.found => r.data,
            _ => {
                return Err(SdkError::NotFound(format!(
                    "chunk {index} of {}",
                    hex::encode(c)
                )))
            }
        };
        if !chunk_matches(&m, index, &data) {
            link.handle()
                .score(p, hashgram_p2p::ScoreEvent::ServedCorruptData)
                .await;
            return Err(SdkError::Corrupt(format!(
                "{p} served chunk {index} that does not match its hash"
            )));
        }
        out.extend_from_slice(&data);
    }
    Ok((out, m))
}

/// Verifies that `data` is the content of `cid_hex`: recomputes the
/// manifest and compares.
#[must_use]
pub fn verify(data: &[u8], mime: &str, encrypted: bool, cid_hex: &str) -> bool {
    manifest_for(data, mime, encrypted)
        .map(|m| hex::encode(cid(&m)) == cid_hex)
        .unwrap_or(false)
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_round_trip_and_tamper_detection() {
        let (ct, key) = encrypt_private(b"secret photo").unwrap();
        assert_eq!(decrypt_private(&ct, &key).unwrap(), b"secret photo");
        let mut bad = ct.clone();
        bad[0] ^= 1;
        assert!(decrypt_private(&bad, &key).is_err());
        let wrong = FileKey {
            key: [0; 32],
            nonce: key.nonce,
        };
        assert!(decrypt_private(&ct, &wrong).is_err());
    }

    #[test]
    fn verify_matches_cid() {
        let m = manifest_for(b"abc", "text/plain", false).unwrap();
        let c = hex::encode(cid(&m));
        assert!(verify(b"abc", "text/plain", false, &c));
        assert!(!verify(b"abd", "text/plain", false, &c));
        assert!(!verify(b"abc", "text/html", false, &c));
    }
}
