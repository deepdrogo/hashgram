# Hashgram One — Target Architecture

**One identity. One inbox. One vault. One network.**

Hashgram One is a private communication and storage platform built on the
existing Hashgram chain, P2P network and cryptography. The blockchain and
libp2p swarm become infrastructure; the consumer-facing concepts are Mail,
Drive, People, Feed, Spaces, Earn, Wallet and Network.

This document describes the architecture as implemented on the `hashgram-one`
branch. Where something is designed but not yet built it says so explicitly.
Companion documents: `HASHGRAM_ONE_AUDIT.md` (baseline), `HASHMAIL.md`,
`HASHDRIVE.md`, `SPACES.md`, `SYNC_ENGINE.md`, `PRIVACY_MODEL.md`,
`MULTI_DEVICE_SECURITY.md`, `ADR_HASH_STORAGE_MARKET.md`, `MAIL_GATEWAY.md`.

---

## 1. Layering

```
┌──────────────────────────────────────────────────────────────────────────┐
│ Clients: Desktop (Tauri/Solid), CLI (hashgram-client), future mobile      │
│   consume ONLY the application SDK below; never raw consensus/wire types  │
├──────────────────────────────────────────────────────────────────────────┤
│ sdk/rust/hashgram-sdk  — the application boundary                         │
│   HashgramOne (facade) · mail · drive · people · feed · circles · spaces   │
│   devices · sync · provider · network · wallet · store (local encrypted DB)│
│   account · link · messaging (MLS transport) · blob · social · chain_relay │
├──────────────────────────────────────────────────────────────────────────┤
│ node/hashgram-app      — pure application protocol (no I/O)               │
│   version · mail · drive (segment crypto, manifests, merge) · capability   │
│   people · circle · space (signed event log + role state machine) · spam   │
├──────────────────────────────────────────────────────────────────────────┤
│ node/hashgram-mls · hashgram-proto (p2p, chat, app) · hashgram-chain       │
│ hashgram-identity (vault) · hashgram-net (identity, canonical, purposes)   │
├──────────────────────────────────────────────────────────────────────────┤
│ node/hashgram-node (store · relay · media · bootstrap) over hashgram-p2p   │
│   mailboxes · key packages · blobs · social log · chain relay · rewards    │
├──────────────────────────────────────────────────────────────────────────┤
│ hashgramd (Cosmos SDK): identity · username · serviceproof · bank · …      │
└──────────────────────────────────────────────────────────────────────────┘
      services/mail-gateway (optional SMTP bridge)     indexer (read model)
```

Rule: **everything cryptographic lives in Rust** (`hashgram-app`,
`hashgram-sdk` and below). JavaScript/TypeScript never sees a mnemonic, root
key, device key, Drive key, manifest key or MLS state.

---

## 2. Where state lives

| State | Location | Encrypted by | Notes |
| --- | --- | --- | --- |
| Identity root, devices, usernames, balances, providers | **Chain** | — (public) | Only global-consensus facts. |
| Envelopes (MLS ciphertext) in transit | Store nodes (redb) | MLS | Node sees `mailbox=blake3(device key)`, size, times. ≤ 30 days. |
| Key packages | Store nodes | — (public MLS key packages) | Signed by device key. |
| Drive objects, mail blob attachments, media | Store/media nodes | Client (XChaCha20-Poly1305) | Ciphertext only; CID = BLAKE3(manifest). |
| Drive manifest (tree, versions, shares) | Store nodes as a private blob | Drive manifest key | Pointer + key live in the vault and travel to other devices via `DeviceSync`. |
| Mail folders, flags, threads, contacts, Space state, sync cursors | **Local client store** (`hashgram-sdk::store`, redb, per-record sealed) | Local store key derived from device seed | Never uploaded. Flags converge across devices via `MailStateHint`. |
| MLS group state | Vault `extra["mls_state"]` | Vault (Argon2id + XChaCha20) | Unchanged from baseline. |
| Public posts, follows, profiles | Store nodes social log + indexer | — (signed public) | Unchanged. |
| Private (Circle/Space) posts | Inside MLS groups | MLS | Never leave the group. |
| Indexer projections (balances, leaderboards, public social) | PostgreSQL | — | Read model; rebuildable; never private content. |
| Gateway queue (external mail) | Gateway host | Gateway-local | Sees external plaintext by necessity; documented. |

Nothing private is placed on chain. Subjects, recipients, filenames, folder
structure and share graphs are never visible to a node.

---

## 3. The application envelope

All Hashgram One application traffic is `hashgram.app.v1.AppMessage`, carried
as `ChatMessage{kind: CHAT_KIND_APP, app: …}` inside an MLS application
message (`proto/hashgram/app/v1/app.proto`). The MLS group is the security
boundary: only members decrypt; the sender's address and device are
authenticated by MLS credentials that are the on-chain device keys.

Versioning:

* `AppMessage.version` (envelope) and `min_reader_version`.
* Per-body `version` (Mail 1, Drive 1, Space 1, …).
* Unknown `oneof` arm → `Unsupported` (ciphertext kept for a later build).
* `hashgram-app::version` centralises the supported set and the
  `check(version, min_reader)` rule used by every reader.

Groups are keyed by participant set. `Messaging` (MLS transport) is shared by
chat (legacy), Mail, People, Circles and Spaces. A Circle or Space **is** an
MLS group with a `GroupMeta.kind` tag.

---

## 4. HashMail

Native HashMail is end-to-end encrypted mail between Hashgram identities.

* **Address forms.** `hash1…` (authoritative), `@alice` (username on chain),
  `alice@hashgram.io` (mail form of the username). Resolution:
  `x/username/lookup/{name}` → owner address. Display names inside a message
  are hints and are re-resolved before display.
* **Transport.** One `MailMessage` per recipient **set**: the sender's
  devices + every device of To+CC form one MLS group (reused for the same set,
  found via `Messaging::conversations()` by member-address set). BCC
  recipients each get a separate copy in a sender+recipient group with
  `bcc_copy = true`; the To/CC list is still visible to them (like SMTP), the
  other recipients never learn of them.
* **Threads.** `thread_id` (16 bytes, = first message's id), `in_reply_to`,
  `references` — RFC 5322 semantics so gateway bridging is lossless.
* **Folders/flags** are local state: Inbox, Sent, Drafts, Archive, Trash,
  Spam, Starred, labels, read/unread. `MailStateHint` in the self-group keeps
  a user's devices converged without a server.
* **Attachments.** ≤ 64 KiB inline; otherwise an encrypted blob (`BlobRef`,
  snapshot) or a `DriveCapability` (snapshot or live). See §5.
* **Delivery state.** `MailReceipt{DELIVERED}` is sent automatically when a
  device decrypts; `READ` only if the reader opted in.
* **Spam.** `hashgram-app::spam::Policy`: contacts → Inbox; unknown senders →
  "Requests" (a folder) with a local trust score from username age, prior
  interaction, block/mute lists and (for gateway mail) gateway auth results;
  hard block list; per-sender rate window. No central authority decides who
  may send; nothing leaves the device. Node-level mailbox quotas remain the
  transport backstop (F8 in the audit).
* **Expiration.** `expire_after_secs` advisory; `MAX_MAIL_BODY = 1 MiB`,
  `MAX_RECIPIENTS = 100`, `MAX_REFERENCES = 64`, `MAX_ATTACHMENTS = 64`.

Detailed spec: `HASHMAIL.md`.

## 5. HashDrive

* **Objects.** A file is encrypted client-side into a `DriveObjectRef`:
  segmented XChaCha20-Poly1305 (segment = 1 MiB − 16 B plaintext → exactly
  one 1 MiB blob chunk), nonce = base_nonce ⊕ le64(i) in the last 8 bytes,
  AAD = `"hashgram-drive-v1" || le64(i) || le64(total_size)`, so segments
  cannot be reordered, dropped or truncated. Streaming: encrypt/decrypt one
  segment at a time; upload/download resume at chunk granularity through the
  existing `missing_chunks` protocol.
* **Manifest.** `DriveManifest{entries, shares, tombstones, revision}` is the whole tree
  (folders are entries with `kind=FOLDER`; children point at `parent_id`).
  Encrypted with the Drive manifest key (random at Drive creation, shared to
  the owner's other devices via `DeviceSync.DriveKeyring`, never derived from
  the root seed so a device-only vault can hold it) and stored as a private
  blob. Vault keeps `{drive_id, manifest_key, manifest_cid, revision}`.
* **Concurrency.** `hashgram-app::drive::merge(a, b)`: per-entry
  last-writer-wins by `modified_at_ms` then device key; versions are unioned;
  a trashed entry beats an untrashed one with older modification;
  permanent deletes leave tombstones so a concurrent device cannot
  resurrect the entry (an edit after the delete wins). Revision = max+1.
  Deterministic and commutative, tested.
* **Versions, trash, restore.** Every overwrite appends the previous
  `DriveObjectRef` to `versions` (bounded, oldest evicted); trash is a flag;
  restore clears it; permanent delete removes the entry (blobs remain until
  provider expiry — Drive has no way to force a provider to delete, stated
  honestly).
* **Sharing.** A `DriveCapability` contains the object key; holding it is the
  permission. Snapshot = fixed `DriveObjectRef`. Live = owner pushes
  `DriveShareUpdate` on every new version to the group the share travelled in
  (`DriveShareRecord.group_id`). Revoke = `DriveShareRevoke` + the next
  version is encrypted under a fresh key. Bytes already downloaded cannot be
  recalled (documented). Folder shares carry an encrypted
  `DriveFolderManifest`.
* **Integrity.** Plaintext BLAKE3 in every ref; chunk hashes from the blob
  layer; AEAD tags per segment. A download is not surfaced until all three
  hold.

Detailed spec: `HASHDRIVE.md`.

## 6. Mail + Drive integration

`MailAttachment.source = drive(DriveCapability)`. Composer chooses:
*Snapshot* → the SDK creates an immutable copy ref (same CID, same key — the
object is immutable anyway; a later edit creates a new object under a new key,
so the recipient's snapshot cannot follow). *Live* → the SDK records a
`DriveShareRecord{mode: LIVE, group_id}` so subsequent versions push
`DriveShareUpdate` into the mail thread's group. Recipient clients present
both as attachments; a live one shows "updated" when a newer version arrives.
Received attachments can be "saved to Drive": the object ref is copied into
the recipient's manifest with `attrs.origin = mail:<id>`, no re-upload.

## 7. People

* Lookup by username (`x/username/lookup`), by mail address (`local@…` →
  username), by wallet address (`x/identity/identity/{addr}` + reverse
  username), by public profile (social `PROFILE_UPDATE`).
* Contact requests travel as `ContactRequest`/`ContactResponse` in a direct
  MLS group; friend state is local (`ContactRecord.states`), synced across the
  user's devices via `ContactsSnapshot`.
* Follow/unfollow remain public social events. Block/mute are local and feed
  the spam policy and Feed filters.
* Wallet disclosure: `ProfileCard.wallet_address` is sent only to the
  contacts the user chooses; the indexer never links addresses to usernames
  unless the user registered the username with that address (which is public
  by construction on chain and is shown as "verified").

## 8. Feed and Circles

* **Public feed** = existing social events. SDK `feed` module provides
  `friends()`, `following()`, `explore()`, `author()`, `thread()`, `post()`,
  `comment()`, `react()`, `repost()`, `poll()` with cursor pagination
  (`before_ts`). Friends/following feeds are chronological; explore is served
  by the indexer's chronological/hashtag endpoints. No opaque ranking.
* **Circles** = MLS groups (`GroupMeta.kind = "circle"`) with `CircleEvent`
  content. Adding a member = MLS add (new epoch → they cannot decrypt earlier
  messages; history is re-shared only if `history_visible_to_new_members`).
  Removing a member = MLS remove commit; forward secrecy from the next epoch.
  Private posts in the feed are Circle posts merged client-side.

## 9. Spaces

A Space = one MLS group + a signed, hash-chained `SpaceEvent` log replayed by
every member into `hashgram-app::space::State`. Roles: Owner, Admin, Member,
Guest. Rules (enforced identically by every reader, so an unauthorised event
is rejected by all):

| Event | Allowed actor |
| --- | --- |
| Create | first event only; actor becomes Owner |
| InfoUpdate, Announcement, DriveShare/Unshare | Admin+ |
| MemberAdd(role ≤ own), MemberRemove(target < own) | Admin+ |
| RoleChange | Owner (any) / Admin (to ≤ Member, target < Admin) |
| Post, Comment | Member+ (Guest read-only) |

Signatures: device ed25519 over the canonical event under purpose
`space-event` (implemented in `hashgram-app` with `hashgram-net`'s
canonical builder and a new app-level domain string; it does not touch the
13 protocol purposes). Replay protection: per-actor `sequence` must strictly
increase. MLS membership is reconciled to the role table: every `MemberAdd`
triggers `Messaging::add_participant`, every remove a removal commit.
Space Drive = `SpaceDriveShare` folder capabilities; Space Mail = mail whose
recipient set is the Space group. Nothing about a Space is on chain.

## 10. Earn / providers

Unchanged economics (`x/serviceproof`). SDK `provider` module exposes
`status()`, `register()`, `update()`, `unbond()`, `withdraw()`, `earnings()`,
`lifecycle()` (state derived from chain fields: Unregistered → Registered →
Bonding → WaitingForAssignment → Active → Degraded → ChallengeFailed →
Jailed → Unbonding → Offline). Rewards continue to depend on chain-issued
challenges and client-signed receipts; nothing here rewards declared
capacity. Long-term paid storage: `ADR_HASH_STORAGE_MARKET.md` (decision:
off-chain lease protocol first, escrow module as a later governance upgrade).

## 11. Indexer, leaderboards, network stats

Additive Go changes in `indexer/`: a `balances` table projected from genesis
balances + `coin_spent`/`coin_received` events; endpoints
`/v1/leaderboards/holders`, `/v1/leaderboards/validators`,
`/v1/leaderboards/providers`, `/v1/leaderboards/earners`,
`/v1/network/stats`, `/v1/validators`. Usernames are joined only through the
public on-chain owner relation. Private content is never indexed.

## 12. Security model changes and scheduled protocol upgrades

Implemented now (no wire break): F3 bound, F4 TTL sweep, F5 cap, F6 stats,
F1 sentinel, F2 replay cache, indexer G7. Scheduled for **protocol minor
v1.1** (coordinated node+client release, no consensus change): new signing
purpose `turn-credential`; responder peer id in `MailboxFetch`/`MailboxAck`/
`BlobPutManifest` preimages. Scheduled for the **first governance upgrade**
(consensus): G1 (score submitter), G2 (prune challenges), G3–G6 pagination
and checks, `x/identity` `MsgUpdateParams`. None of these are needed for
Hashgram One clients to ship.

## 13. Multi-device and recovery

See `MULTI_DEVICE_SECURITY.md`. Summary: devices are on-chain; MLS groups hold
every device; `devices::reconcile()` removes revoked devices from every group
the local device is in (commit per group) and adds missing active devices;
Drive manifest key, contacts and mail flags travel via `DeviceSync` in the
self-group; a revoked device stops receiving from the next epoch of each
group but keeps what it already has (unavoidable); `Drive::rekey` re-seals
a file under a fresh key and revokes its live shares. Recovery: mnemonic
(root), guardian recovery (on chain, existing), and — new — the encrypted
vault backup (`hashgram_sdk::backup::export_backup` / `import_backup`,
format `HGBKUP` v1, Argon2id 256 MiB + XChaCha20-Poly1305, device seed and
MLS state deliberately excluded) that the host of the file cannot decrypt.

## 14. Sync engine

`hashgram-sdk::sync::SyncEngine` runs explicit rounds through states
`Offline → Connecting → Discovering → Syncing{mail, drive, feed, spaces,
wallet} → Idle → Backoff(n)`. Each sub-sync is resumable (mailbox cursors per
store peer, blob `missing_chunks`, Drive revision), idempotent (AppMessage id
dedup) and reports progress events. Spec: `SYNC_ENGINE.md`.

## 15. External mail gateway

`services/mail-gateway`: a separate binary that owns an ordinary Hashgram
identity ("bridge identity") and speaks SMTP outward. Inbound: MIME →
`MailMessage{origin: EXTERNAL_GATEWAY, external: {auth_results, …}}` sent
through MLS to the mapped user. Outbound: a user sends native mail to the
bridge identity with an external `To`; the gateway renders MIME, DKIM-signs
and relays. The gateway never holds user root keys; it sees plaintext of
external mail by necessity and clients label such mail. Alternative operators
can run their own gateway under their own domain. Spec: `MAIL_GATEWAY.md`.

## 16. Hash AI (interfaces only)

`hashgram-sdk::ai` defines `AiProvider` with `LocalAiProvider` (no-op
reference) and a `RemoteAiProvider` trait requiring explicit
`UserConsent` per request category. No implementation transmits content;
nothing in consensus or the node is aware of it.

## 17. Databases

| Component | Engine | Migration |
| --- | --- | --- |
| Node | redb (unchanged) | table-versioned |
| SDK local store | redb, `schema_version` key, sealed values | `store::migrate()` forward-only |
| Indexer | PostgreSQL | numbered migrations in `indexer/schema.go` |
| Gateway | SQLite | embedded migrations |

## 18. Observability

Node metrics extended: `hashgram_blob_incomplete_expired_total`,
`hashgram_receipts_dropped_total`, `hashgram_mailbox_replay_refused_total`.
SDK emits `tracing` spans; never logs plaintext, keys, subjects or addresses
at info level (subjects/bodies never at any level).

## 19. Verification

* Unit tests: `hashgram-app` (39 + fuzz smoke), `hashgram-sdk` (26),
  node/p2p/proto fixes (F1–F6 tests), indexer Go tests.
* `scripts/testnet/hashgram-one-e2e.sh`: a local devnet (mock chain
  gateway with a DEVNET identity registry, one store/relay node, two
  clients) driven through the same `HashgramOne` facade the desktop uses;
  31 assertions over Mail (send, Requests filing, attachments inline +
  live Drive, reply, receipts, threads, BCC), Drive (put/get, versions,
  live share update, save-shared, revoke), People, Spaces (roles enforced),
  Circles (poll tally), devices. Runs in ~45 s.
* Mainnet (2026-09-12): `hashgram-client one drive put/get`, `one sync`,
  `one balance`, `one network overview`, `one provider list` through the
  genesis node's P2P relay — Drive round trip and balance verified.
* Fuzz: `node/fuzz/fuzz_targets/app_message.rs` (+ mirrored smoke test).

## 20. What is not built (honest list)

* Mail gateway: see `MAIL_GATEWAY.md` for the exact implemented/stubbed
  split (SMTP transport details, TLS, DKIM key handling).
* Storage lease RPC bodies (`LeaseOffer` etc.) and provider-side
  acceptance: the client model, signing, verify-before-pay and payment are
  implemented (`hashgram_sdk::storage_lease`); the wire arms and node side
  are protocol v1.1 work (`ADR_HASH_STORAGE_MARKET.md` §5).
* Merkle light client; push notifications; group-call E2EE; storage market
  escrow module (`x/storagemarket`, consensus upgrade); equivocation
  detection in the indexer; protocol v1.1 signing changes (F1 purpose, F2
  responder binding); the Go-side consensus fixes G1–G6.
* Multi-device caveat: two devices that each created a Drive before ever
  syncing keep separate Drives (`Drive::apply_keyring` refuses to merge
  different drive ids); the user picks one.
