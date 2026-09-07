# Storage

Media on Hashgram — photos, videos, reels, voice notes, attachments — is
content-addressed and stored on `store` and `media` nodes. Holding a content
identifier is enough to verify every byte received from any node, so
providers are interchangeable and need not be trusted, and storage rewards
are settled on chain against evidence a provider cannot fake without the
bytes.

## Content identifiers

A blob is split into 1 MiB chunks. The manifest lists the BLAKE3 hash of
each chunk; the CID is the BLAKE3 hash of the canonical manifest:

```protobuf
message BlobManifest {
  uint32 version = 1;       // 1
  uint64 size = 2;          // ≤ 4 GiB
  uint32 chunk_size = 3;    // 1048576
  repeated bytes chunks = 4;
  string mime = 5;
  bool encrypted = 6;
}
canonical = u32(version) || u64(size) || u32(chunk_size) || count || each chunk hash || mime || u32(encrypted)
CID = BLAKE3(canonical)
```

Same bytes, same MIME, same encryption flag → same CID everywhere,
regardless of who encoded the manifest. The manifest is verified against the
CID and each chunk against the manifest, so a provider serving a wrong byte
is detected, scored down (`ServedCorruptData`, −50) and tried elsewhere.

## Public and private

- **Public** media (posts, reels, avatars) is stored as is and referenced by
  CID from social events with a `content_hash` (BLAKE3 of the plaintext) so a
  safety verdict can name the content independently of chunking.
- **Private** media (attachments, voice notes) is encrypted on the device
  with XChaCha20-Poly1305 under a random key and nonce before chunking. The
  manifest describes ciphertext (`encrypted = true`,
  `application/octet-stream`); the key and nonce travel inside the E2EE
  message that references the blob (`chat.Attachment`). A store node holds a
  file it cannot open and a CID that names ciphertext.

## Upload

```text
client ── BlobPutManifest (signed "blob-upload") ──▶ provider
       ◀── accepted + missing chunk indices ────────   (resume point)
       ── BlobPutChunk ×N ─────────────────────────▶
       ◀── accepted ───────────────────────────────    complete → provider advertises CID in DHT
```

- The upload authorisation is signed by the uploader's device key with a
  fresh timestamp; quota is per node (`storage_quota_bytes`) and per
  uploader (one eighth of the node's quota).
- A manifest that does not hash to its CID, or a chunk that does not match
  its manifest entry, is refused; the sender is scored.
- Re-sending the manifest returns what is still missing, so an interrupted
  upload resumes. Chunks shared between blobs are stored once
  (reference-counted).
- The SDK uploads to `media` nodes first, then `store` nodes, up to the
  requested replica count, and reports which providers accepted the whole
  blob.

## Download

Providers are found through the DHT (`hashgram/blob/<cid>`), then any
connected `store`/`media` peer. The SDK fetches the manifest, checks it
against the CID, fetches chunks, checks each against the manifest, and moves
to the next provider on any mismatch. After a verified download the client
signs a retrieval receipt for the provider (`docs/SERVICE_REWARDS.md`).

## Replication and repair

Target: **three** copies. Each node tracks, for every complete blob it
holds, which other verified storage peers also hold it (by asking
`BlobHas`), and every minute pushes blobs below target to a storage peer
that lacks them (bounded per pass). A blob with one known copy is reported
as `DEGRADED` by `/v1/blobs/{cid}/health` and in `hashgramctl storage`; one
copy is one copy, and the operator is told so rather than shown a green
light.

The acceptance test uploads a reel video to one node and waits until the
health endpoint reports `HEALTHY` with three known replicas
(`scripts/testnet/phase2.sh`).

## Storage rewards

Bytes held earn only when recorded on chain as a `StorageAssignment` by a
registered assigner and then proven by answered challenges. A node whose
operator is an assigner records assignments for its own complete blobs and
for replicas it pushed; the chain issues challenges each epoch for a random
chunk of a random assignment; the node answers with the chunk's SHA-256 leaf
hash and Merkle path (byte-identical to `x/serviceproof/types/merkle.go`,
pinned by a vector test), signed with its node key. Declared but unused disk
earns nothing. Details in `docs/SERVICE_REWARDS.md`.

## Retention and deletion

Blobs have no protocol-level expiry: a node holds what it accepted until
its operator deletes it (`DELETE /v1/blobs/{cid}` on the local API) or a
trusted safety attestation blocks the CID, after which the node stops
serving it. Quota is enforced at upload time. A node that deletes a blob it
was assigned on chain fails its next challenge, which is the intended
consequence.

## Limits

| Limit | Value |
| --- | --- |
| chunk size | 1 MiB |
| maximum blob | 4 GiB (4,096 chunks) |
| RPC frame | 1 MiB + 64 KiB |
| default per-uploader share | 1/8 of the node quota |
| replication target | 3 |
| repair pass | every 60 s, ≤ 8 pushes per pass per node |

## Operator view

```text
hashgramctl storage            blob store counts, bytes, quota, degraded blobs, store peers,
                               chain assignments and open challenges for this node's operator
curl 127.0.0.1:26672/v1/blobs/<cid>/health
curl -X POST 127.0.0.1:26672/v1/blobs/<cid>/fetch -d '{}'   pull a blob from any provider
```
