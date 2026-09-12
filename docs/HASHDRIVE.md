# HashDrive — Specification

Status: describes the code on the `hashgram-one` branch. Cryptography,
manifest, merge and capabilities: `node/hashgram-app/src/drive.rs`.
Network, keyring, sharing and device sync: `sdk/rust/hashgram-sdk/src/drive.rs`
with `blob.rs` (`upload_sealed`, `download`) and `devices.rs`. Wire shape:
`proto/hashgram/app/v1/app.proto`. Harness: `hashgram-client one drive …`.
Companion documents: `STORAGE.md` (the blob protocol underneath),
`PRIVACY_MODEL.md` §3.5, `MULTI_DEVICE_SECURITY.md`, `HASHMAIL.md` §5,
`SPACES.md`.

HashDrive is the user's encrypted file tree on the network. Providers store
ciphertext they cannot open; the tree itself is one more encrypted blob;
the keys live in the vault and inside MLS messages, never on a node.

---

## 1. Object encryption

A file becomes one immutable *Drive object*. Constants (`drive.rs`):

| Constant | Value | Meaning |
| --- | --- | --- |
| `TAG_LEN` | 16 | Poly1305 tag |
| `SEGMENT_PLAINTEXT` | `CHUNK_SIZE − TAG_LEN` = 1,048,576 − 16 = 1,048,560 | plaintext bytes per segment |
| `AAD_DOMAIN` | `b"hashgram-drive-v1"` | AAD prefix |
| `MAX_OBJECT_PLAINTEXT` | 4 GiB − 4096 × 16 B | largest plaintext (fits `MAX_BLOB_SIZE` after tags) |

`ObjectKey{key: [u8;32], base_nonce: [u8;24]}` is generated per object by
`ObjectKey::generate` (`getrandom`). Segment `i` is sealed with
XChaCha20-Poly1305 under:

```
nonce_i = base_nonce with its last 8 bytes XOR le64(i)        (ObjectKey::nonce)
aad_i   = "hashgram-drive-v1" ‖ le64(i) ‖ le64(total_plaintext_size)   (fn aad)
```

`segment_count(size) = max(1, ⌈size / SEGMENT_PLAINTEXT⌉)` — an empty file
is one authenticated empty segment. `ciphertext_size(size) = size +
segment_count × 16`.

`SegmentEncryptor::new(key, total)` / `seal(segment)` requires every segment
to be exactly `SEGMENT_PLAINTEXT` bytes except the last, refuses sealing
past `count`, and accumulates a BLAKE3 of the plaintext
(`plaintext_hash()`). `SegmentDecryptor::new(key, total)` / `open(chunk)`
refuses more segments than the object declares, checks each decrypted
length, and `verify(expected_hash)` fails unless `finished()` and the hash
matches. Because index and total size are in the AAD, a provider cannot
reorder, drop, duplicate, truncate or extend segments without an
`AppError::Integrity` (tested in `tamper_reorder_truncate_detected`).

One-shot helpers: `seal_object(plaintext) → (ciphertext, DriveObjectRef)`
(fresh key), `seal_object_with(plaintext, key)` (used for the manifest),
`open_object(ciphertext, ref)` (checks `ciphertext.len() ==
ciphertext_size(ref.size)`, every tag, then the plaintext hash), and
`open_object_with_key(ciphertext, key)` for a manifest whose reference is
not yet known (size derived from the ciphertext length; content is
verified by decoding, not by hash).

`object_ref(ciphertext, key, size, hash)` builds
`DriveObjectRef{version: DRIVE_VERSION = 1, cid, key, size, segment_size:
SEGMENT_PLAINTEXT, plaintext_hash}` where `cid = hashgram_proto::blob::cid(
manifest_for(ciphertext, "application/octet-stream", encrypted=true))`,
i.e. exactly the CID the blob layer will compute. `validate_object_ref`
requires a 32-byte cid and hash, 32/24-byte key material, `segment_size ==
SEGMENT_PLAINTEXT` (a different value is `Unsupported`, not an error), and
`size ≤ MAX_OBJECT_PLAINTEXT`.

### 1.1 Why a segment is a blob chunk

Every sealed segment is `SEGMENT_PLAINTEXT + TAG_LEN = CHUNK_SIZE` bytes, so
segment `i` *is* blob chunk `i`. Consequences:

* streaming: a 4 GiB file is encrypted and decrypted one MiB at a time;
* resume: `blob::upload_sealed` sends `BlobPutManifest` and then only the
  chunk indexes the node lists in `missing_chunks`, so an interrupted upload
  continues where it stopped; downloads fetch chunk by chunk and each chunk
  is hash-checked (`chunk_matches`) before the AEAD tag is checked;
* the provider learns the number of segments, and hence the plaintext size
  to within 16 bytes per MiB (`PRIVACY_MODEL.md` §3.5).

---

## 2. The manifest

`hashgram_app::drive::Manifest` wraps `DriveManifest{version, drive_id (16
random bytes), revision, updated_at_ms, device_pubkey, entries, shares,
tombstones}`. It is a flat list; folders are entries with
`kind = FOLDER`; children point at `parent_id` (empty = root).

`DriveEntry{id (16 bytes, stable across rename/move), parent_id, kind,
name, mime, size, created_at_ms, modified_at_ms, current, versions,
trashed, trashed_at_ms, attrs, starred}`.

| Bound | Value | Enforced in |
| --- | --- | --- |
| `MAX_ENTRIES` | 500,000 | `from_pb`, `mkdir`, `add_file` |
| `MAX_NAME` | 255 bytes | names; no `/`, `\`, `.`, `..` |
| `MAX_MIME` | 128 | — |
| `MAX_VERSIONS_PER_ENTRY` | 50 | `update_file` evicts oldest; `validate_entry` |
| `MAX_ATTRS` / `MAX_ATTR` | 32 attributes / 1024-byte values, 64-byte keys | `set_attr` |
| `MAX_TOMBSTONES` | 10,000 | `delete`, `merge` |
| note on a version | 256 bytes | `update_file` |

`Manifest::from_pb` validates every entry, rejects duplicate ids, a parent
that is not a folder, and cycles (`check_acyclic`). Sibling names are unique
case-insensitively among non-trashed entries (`require_free_name`,
`eq_ignore_ascii_case`). `resolve_path("/Docs/A.TXT")` walks non-trashed
children case-insensitively; `path(id)` renders `/Docs/Sub/b.txt`.

Operations: `mkdir`, `add_file`, `update_file` (previous `current` becomes
`DriveVersion{version_no, object, created_at_ms, device_pubkey, note}`;
returns the new version number), `version_no` (1 for a never-updated
file), `restore_version` (re-applies an old object as a new version, nothing
lost), `rename`, `mv` (refuses moving a folder into its own subtree),
`copy_file` (same object ref, no re-upload), `set_starred`, `set_attr`,
`list(parent, trashed)` (folders first, then name), `starred`, `search`,
`used_bytes`, `subtree`.

### 2.1 Trash, restore, delete

Deletion is always two steps. `trash(id)` flags the whole subtree with
`trashed_at_ms`. `restore(id)` clears the flags; if the parent is gone or
trashed the entry moves to root; if the name now collides it becomes
`<name> (restored <ms>)`. `delete(id)` refuses an entry that is not trashed,
removes the subtree, appends a `DriveTombstone{id, at_ms}` per removed id
(oldest evicted past `MAX_TOMBSTONES`), and marks every share of a removed
entry `revoked`. `empty_trash` deletes every trashed root. The blobs stay on
providers until their own expiry: Drive has no way to force a provider to
delete (§9).

Every mutation calls `bump(device)`: `revision += 1`, `updated_at_ms = now`,
`device_pubkey = writer`.

---

## 3. Manifest key, keyring, vault

The manifest is sealed with `Manifest::seal(manifest_key)` =
`seal_object_with(encode(), key)` and uploaded as a private blob. The key is
random at Drive creation and is **not** derived from the root seed, so a
device-only vault can hold it and a revoked device's copy can, in principle,
be rotated away from later.

`hashgram_sdk::drive::Keyring{drive_id, key, base_nonce, manifest_cid,
manifest_ref, revision}` (hex strings) is stored in the vault's `extra` map
under `VAULT_DRIVE_KEYRING = "drive_keyring"` by `DriveState::persist`. The
decoded manifest is cached in the local store under `drive/manifest/current`.
`DriveState::load` handles three cases: keyring + cache → use both; keyring
without cache → empty manifest with the same `drive_id` at revision 0 (sync
will fetch); no keyring → new Drive, new key, `dirty = true`.

---

## 4. Commit

`Drive::upload/update/mkdir/…` change the manifest locally
(`save_manifest_locally` sets `dirty`). `Drive::commit`:

1. returns early if `!dirty` and a manifest has been committed before;
2. `manifest.seal(keyring.object_key())`;
3. `blob::upload_sealed(…, REPLICAS = 3)`;
4. updates `keyring.{manifest_cid, manifest_ref, revision}`, clears
   `dirty`, `persist`s to the vault;
5. `announce_keyring()` — sends `DeviceSync{drive_keyring}` to the self
   group (no-op on a single-device account).

Objects themselves are uploaded with `REPLICAS = 3` at `upload`/`update`
time, before the manifest references them. The sync engine commits a dirty
manifest in its `Outbox` stage; the CLI commits after every mutating
command.

---

## 5. Multi-device merge

`drive::merge(a, b)` is deterministic (tested commutative) and refuses
different `drive_id`s. Rules, per entry id:

| Situation | Result |
| --- | --- |
| entry on one side only | kept |
| both sides | winner = later `modified_at_ms`; tie → side whose manifest `device_pubkey` is greater; winner's mutable fields (name, parent, trashed, starred, attrs, current) |
| versions | union by `version_no`; if the loser's `current` CID is absent from the union it is appended as a new version with note `"concurrent edit"`; bounded by `MAX_VERSIONS_PER_ENTRY` |
| trash | sticky: if the loser is trashed and `trashed_at_ms > winner.modified_at_ms`, the merged entry is trashed |
| `created_at_ms` | minimum of both |
| shares | union by `share_id`; `revoked` is sticky |
| tombstones | union, latest `at_ms` per id; an entry with a tombstone is dropped **unless** its `modified_at_ms > tombstone.at_ms` (edit-after-delete resurrects it rather than losing work) |
| orphans | an entry whose parent no longer exists moves to root |
| sibling name collision | the later duplicate becomes `<name> (<first 4 id bytes hex>)` |
| revision | `max(a, b) + 1`; `updated_at_ms`/`device_pubkey` from the newer side |

### 5.1 `DeviceSync.DriveKeyring` → `Drive::apply_keyring`

When another of our devices commits, its `DriveKeyring{drive_id,
manifest_key, manifest_cid, revision}` arrives in the self group
(`Devices::handle_incoming` accepts it only from our own address):

* different `drive_id` and we have no content → adopt the incoming key and
  id (revision 0), then continue;
* different `drive_id` and we already have content → **kept local**, a
  warning is logged, nothing merged. This is the documented limitation:
  two devices that each created a Drive before ever syncing hold two Drives,
  and the user merges manually;
* same id, `revision ≤ ours` or empty CID → nothing;
* otherwise download the manifest by CID, `open_object_with_key`, decode,
  and either adopt it (we are empty at revision ≤ 1) or `merge`. The local
  manifest is saved; `dirty` is set iff the merged revision exceeds the
  incoming one, so the merged result is committed on the next round.

`Devices::bootstrap_new_device` calls `announce_keyring` so a freshly added
device receives the key without waiting for the next commit.

---

## 6. Sharing

Holding a `DriveCapability{version: CAPABILITY_VERSION = 1, share_id,
owner, entry_id, name, mime, size, object, mode, permission, version_no,
granted_at_ms, folder}` **is** the permission: it contains the object key.
It is only ever carried inside MLS.

| Mode | Semantics |
| --- | --- |
| `SNAPSHOT` | fixed `DriveObjectRef`; later edits do not reach the grantee |
| `LIVE` | `Drive::update` sends `DriveShareUpdate{share_id, object, version_no, name, size}` to `DriveShareRecord.group_id` for every non-revoked live share of the entry (`Manifest::live_shares_of`) |

`Drive::share(entry, "addr1,addr2", mode, permission, note)`: finds or
creates the grantees' conversation group, `grant_in_group`, then sends
`DriveShare{capability, note}` there. `grant_in_group(id, label, gid, mode,
permission)` (also used by mail attachments and Spaces) builds a
`DriveFolderManifest` for folders (§7) and calls `Manifest::grant`, which
refuses trashed entries, sets `version_no`, and appends
`DriveShareRecord{share_id, entry_id, grantee, mode, permission,
granted_at_ms, revoked, revoked_at_ms, group_id}`. `DrivePermission::WRITE`
exists in the model for Spaces but nothing in this branch acts on it.

On the recipient, `Drive::handle_incoming` stores a `SharedWithMe{capability,
from, group_id, note, received_at_ms, updates, revoked}` under
`drive/shared_with_me/<share_id>`, and:

* ignores a `DriveShare` whose `capability.owner` is not the MLS sender;
* applies a `DriveShareUpdate` only if the record exists, came from the same
  sender, is not revoked, and is `LIVE` (updates to snapshots are ignored);
* marks `revoked` on `DriveShareRevoke` from the same sender.

`Drive::save_capability(cap, parent)` copies the object ref into our own
manifest with `attrs.origin = share:<share_id>` and `attrs.owner`; no
re-upload, and from then on the grantee holds the key like any other entry.

### 6.1 Revocation

`Drive::revoke(share_id)` → `Manifest::revoke` (sticky `revoked`, error if
already revoked) → `DriveShareRevoke` into the share's group. Semantics,
stated plainly:

* Revocation stops future `DriveShareUpdate`s.
* Bytes the grantee already downloaded, and the object key in the
  capability they hold, cannot be recalled. The current version stays
  readable to them for as long as its blob exists on providers.
* The next version is protected because `seal_object` always generates a
  fresh key per object; `Manifest::has_active_shares` exists so a UI can
  say so.

---

## 7. Folder shares

`Manifest::folder_manifest(folder_id)` produces `DriveFolderManifest{version,
folder_id, name, revision, updated_at_ms, entries}`: the non-trashed subtree
with ids preserved, the folder's direct children re-parented to root, and
`versions` cleared. `grant_in_group` seals it with `seal_object`, uploads it,
and grants a capability with `folder = true` whose `object` is the sealed
folder manifest. `Drive::shared_folder_entries(cap)` downloads and decodes
it; `save_capability` refuses folder capabilities (save files individually).
A folder share is a snapshot of the listing at grant time even in `LIVE`
mode: nothing re-sends the folder manifest on later changes.

---

## 8. Integrity

A download is surfaced only when all three hold:

1. blob layer: the manifest hashes to the CID and every chunk matches its
   hash (`blob::download_from`, a mismatch scores the peer
   `ServedCorruptData`);
2. AEAD: every segment authenticates under `(key, nonce_i, aad_i)`;
3. `plaintext_hash` (BLAKE3 of the whole plaintext) matches the reference.

`Drive::download_object` performs exactly this and signs a retrieval
receipt for the serving peer (`ROLE_MEDIA`), which is how media nodes earn.

---

## 9. What providers see, and limits

| Party | Sees |
| --- | --- |
| Store/media node | ciphertext, CID, ciphertext size, chunk count, uploader device key (quota), who fetched which CID when |
| Grantee | the shared entry's name, MIME, size, content; nothing else of the tree |
| Owner's other devices | everything, via `DriveKeyring` |

Honest caveats:

* **No provider-enforced deletion.** `delete`/`empty_trash` remove entries
  and keys from the manifest; the ciphertext lives until the provider
  expires it. Blob retention has no TTL or reclamation policy yet
  (`HASHGRAM_ONE_AUDIT.md` §6.2, `ADR_HASH_STORAGE_MARKET.md`).
* **Single-manifest design.** The whole tree is one blob re-sealed and
  re-uploaded on every commit; size grows with entries × versions.
  `MAX_ENTRIES` is a safety bound (~100 MiB sealed), not a comfortable
  working size. Each entry carries up to 50 `DriveObjectRef`s (~150 bytes
  each).
* **Merge is last-writer-wins per entry**, with content preserved as
  versions but names/parents not: a concurrent rename and move produce the
  later writer's view.
* **Separate Drives on two devices** are not merged automatically (§5.1).
* **Revocation cannot recall bytes** (§6.1); folder shares do not follow
  edits (§7).
* `PendingUpload`/`pending_uploads` bookkeeping exists but `Drive::upload`
  does not yet write to it; a crash mid-upload leaves partial chunks the
  node expires after 24 h (audit F4), and the caller retries.
* Uploads and commits require a verified store peer; offline mutations stay
  `dirty` until the next successful round.

---

## 10. CLI examples

```bash
hashgram-client one drive mkdir /Docs
hashgram-client one drive put contract.txt /Docs --name contract.txt   # prints entry id + revision
hashgram-client one drive ls /Docs
hashgram-client one drive get /Docs/contract.txt out.txt
hashgram-client one drive update <entry> contract-v2.txt                # "version 2"
hashgram-client one drive versions <entry>
hashgram-client one drive get <entry> old.txt --version 1
hashgram-client one drive restore-version <entry> 1
hashgram-client one drive rename <entry> final.txt
hashgram-client one drive mv <entry> /Archive
hashgram-client one drive rm <entry>          # trash
hashgram-client one drive trash               # list trash roots
hashgram-client one drive restore <entry>
hashgram-client one drive purge <entry>       # permanent; must be trashed
hashgram-client one drive empty-trash
hashgram-client one drive star <entry>

# sharing
hashgram-client one drive share <entry> hash1bob…,hash1carol… --live --note 'for review'
hashgram-client one drive shares
hashgram-client one drive revoke <share_id>
# on the grantee
hashgram-client one sync
hashgram-client one drive shared-with-me
hashgram-client one drive get-shared <share_id> out.txt
hashgram-client one drive save-shared <share_id> /

hashgram-client one drive commit
hashgram-client one drive usage
hashgram-client one drive search contract
```

`scripts/testnet/hashgram-one-e2e.sh` covers put/get round trip, live
share delivered as a mail attachment, `update` producing v2 and updating
the grantee's capability, download via capability, save-shared, and
revocation visibility.
