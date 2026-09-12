# Hashgram One — Repository Audit (pre-transformation)

Date: 2026-09-12. Base: `deepdrogo/hashgram_windows` `main` @ `17c1f1e` (the
superset of `deepdrogo/hashgram` `main` @ `872b95d`; the two repositories have
separate histories, and only the former contains the chain relay that Mainnet
currently runs). Branch: `hashgram-one`.

This document records what existed **before** the Hashgram One transformation,
what is production-usable, what is kept, extended, or replaced, and what is
incomplete or insecure. It is the baseline against which
`HASHGRAM_ONE_ARCHITECTURE.md` describes the target.

Line counts: ~106k Go, ~34k Rust (node + SDK + desktop), 5k protobuf, 620
tracked files.

---

## 1. Repository layout (as found)

| Path | Purpose | Verdict |
| --- | --- | --- |
| `app/` | Cosmos-SDK app wiring, params, mainnet pin (`app/params/mainnet/*`) | **Keep unchanged.** Consensus-critical. |
| `x/{network,founder,feerouter,welcome,serviceproof,username,identity,treasury}` | Chain modules | **Keep.** Off-chain product does not require consensus changes. Two Go fixes (§6). |
| `proto/hashgram/{feerouter,founder,identity,network,serviceproof,treasury,username,welcome}/v1` | Chain protobuf | Keep. |
| `proto/hashgram/p2p/v1`, `proto/hashgram/chat/v1` | Off-chain wire types | **Extend** (new `hashgram.app.v1`, `CHAT_KIND_APP`). Additive only. |
| `node/hashgram-net` | Network identity, canonical encoding, signing domains, handshake | Keep unchanged. Best code in the tree. |
| `node/hashgram-proto` | Wire types, limits, signing, validation, CID | Extend (new `app` module). |
| `node/hashgram-chain` | REST/relay chain client, wallet, tx msgs, transport | Keep. |
| `node/hashgram-identity` | Encrypted vault (Argon2id + XChaCha20-Poly1305) | Keep unchanged. |
| `node/hashgram-mls` | OpenMLS 0.9 wrapper, one ciphersuite | Keep; reused for Mail, Circles, Spaces. |
| `node/hashgram-p2p` | libp2p swarm, handshake gate, limits, scoring | Keep; one bounded-input fix (F3). |
| `node/hashgram-node` | Store/relay/media node daemon | Keep; lifecycle fixes (F1, F4, F5). |
| `node/hashgram-client` | Reference CLI (1,582 lines, 0 tests) | Extend with `mail`, `drive`, `people`, `space` commands as the dev harness. |
| `node/hashgram-devtools` | mock chain gateway (devnet only) | Keep. |
| `node/fuzz` | 8 cargo-fuzz targets | Keep; add app payload target. |
| `sdk/rust/hashgram-sdk` | Client SDK: account, link, messaging, social, blob, calls, chain relay | **Extend heavily.** This is the Hashgram One application boundary. |
| `apps/desktop` | Tauri 2 + SolidJS Windows app (97 commands, v0.1.1) | **Out of scope for this task** (rebuilt later from `DESKTOP_APP_MASTER_PROMPT.md`). Not deleted. |
| `indexer/` | Go, PostgreSQL, 19 read endpoints | Extend: balances projection, leaderboards, network stats. |
| `safety/` | Safety engine (public content only) | Keep. |
| `cmd/` | hashgramd, hashgramctl, keygen, indexer, safety, test-client | Keep. |
| `genesis/`, `tools/`, `scripts/`, `deploy/` | Genesis build, simulators, CI, systemd | Keep. |
| `docs/` | 28 documents | Update stale status lines; add Hashgram One set. |

---

## 2. What is production-usable today

**Chain (Go).** Cosmos SDK v0.53 / CometBFT 0.38, fixed supply 1,000,000,000
HASH, no `x/mint`, no minter permission, a single governance authority, 360 Go
test functions, reproducible release, devnet + four-validator + phase2
acceptance suites. Mainnet `hashgram-1` launched 2026-09-10 (genesis hash
`e322bc23…d5e4d`), height ~45,800 at audit time, one genesis validator.

**Identity model (Go + Rust).** Account secp256k1 (`hash1…`) → root ed25519
(offline-capable) → per-device ed25519 keys with root-signed certificates,
rotation counter, guardian-based recovery with mandatory delay. Device
authority is checked against the chain by every node. Solid.

**Off-chain protocol (Rust).** Domain-separated canonical signing (13
purposes, Go/Rust parity vectors), bounds before allocation, strict lints
(`unwrap_used = deny`, `panic = deny`), 203 Rust tests, 8 fuzz targets.
Handshake pins magic/version/network/chain/genesis, fail-closed.

**Messaging.** MLS groups over store-and-forward mailboxes; every device of
every participant is a member; key packages (one-time + last-resort) held by
store nodes; envelopes carry only `mailbox = blake3(device key)`, size, times.
Works end-to-end on Mainnet (verified 2026-09-12: publish-keys, receive).

**Blobs.** Content-addressed (CID = BLAKE3 of canonical manifest), 1 MiB
chunks, ≤ 4 GiB, hash-verified per chunk, private blobs client-encrypted
(XChaCha20-Poly1305), replication target 3, DHT providers.

**Social.** Signed, hash-chained per-author events, 12 payload types, 64
gossip shards, device authority enforced by nodes, indexer projection.

**Chain relay.** Allow-listed reads and broadcast over `/hashgram/rpc/1` with
two-operator cross-checking in the SDK. Deployed to Mainnet during this
session.

**Rewards.** `x/serviceproof`: bonded providers, chain-issued storage
challenges with Merkle proofs, client-signed relay/media receipts, epoch
settlement from a finite 500M reserve, concentration discount, jail/slash.

**Vault.** Argon2id (64 MiB/t=3) + XChaCha20-Poly1305, atomic 0600 writes,
zeroization, no plaintext leaks (tested).

---

## 3. What remains unchanged

* Consensus: every `x/` module's state machine, params, genesis, tokenomics.
  **No consensus change is made in this transformation.**
* `hashgram-net`, `hashgram-identity`, `hashgram-mls` public APIs.
* Wire protocol on `/hashgram/rpc/1` (request/response bodies, gossip topics,
  limits). Nodes built before Hashgram One interoperate with Hashgram One
  clients without change: all new application content travels inside MLS
  ciphertext the node never decodes.
* The vault file format (v1) and the profile/peerstore files.

---

## 4. What is extended

| Area | Extension |
| --- | --- |
| `proto/hashgram/app/v1/app.proto` (new) | `AppMessage` envelope + HashMail, HashDrive, People, Circles, Spaces, DeviceSync bodies. |
| `proto/hashgram/chat/v1/chat.proto` | `CHAT_KIND_APP = 11`, field `app = 13`. Old clients ignore unknown kinds. |
| `node/hashgram-proto` | `pub mod app`; `build.rs` compiles the app package with `extern_path`. |
| `node/hashgram-app` (new crate) | Pure application protocol: versioning, Mail model/threading/spam scoring, Drive segment encryption + manifests + merge, capabilities, Space event signing + role state machine, Circles, People. No network dependency, so the gateway and any binding can reuse it. |
| `sdk/rust/hashgram-sdk` | New modules `mail`, `drive`, `people`, `feed`, `circles`, `spaces`, `devices`, `sync`, `provider`, `network`, `store`, `app_client`. |
| `node/hashgram-client` | `mail`, `drive`, `people`, `space`, `circle`, `provider`, `network` subcommands (developer harness). |
| `indexer/` | `balances` projection from bank events + genesis; `/v1/leaderboards/*`, `/v1/network/stats`, `/v1/validators`, `/v1/providers/stats`. |
| `services/mail-gateway` (new) | External SMTP compatibility gateway: address mapping, MIME ↔ `MailMessage`, DKIM/SPF policy, queue. Native HashMail does not depend on it. |
| `docs/` | `HASHGRAM_ONE_*`, `HASHMAIL.md`, `HASHDRIVE.md`, `SPACES.md`, `PRIVACY_MODEL.md`, `MULTI_DEVICE_SECURITY.md`, `ADR_HASH_STORAGE_MARKET.md`, `SYNC_ENGINE.md`, `MAIL_GATEWAY.md`, `DESKTOP_APP_MASTER_PROMPT.md`, `DESKTOP_APP_IMPLEMENTATION_CHECKLIST.md`, `HASHGRAM_ONE_AI_HANDOFF.md`. |

---

## 5. What is replaced or demoted

* **Product framing.** README/ARCHITECTURE positioning ("decentralised
  Twitter + messenger + wallet") is replaced by Hashgram One (Mail, Drive,
  People, Feed, Spaces, Earn, Wallet, Network). Code is not deleted.
* **Calls.** `calls.rs` (TURN credential issuance, SFU announce) and
  `CHAT_KIND_CALL` remain in the tree and on the wire, but are **not part of
  the Hashgram One product surface** for the desktop client. They are
  demoted to "infrastructure available to future clients". Reason: the
  product direction (user decision, 2026-09-12) removes calls; removing the
  code now would break existing devnet acceptance tests for no product gain.
* **Chat-line messaging UI** (Messages/Reels/Stories/Channels routes in
  `apps/desktop`) is superseded by HashMail + Feed + Circles in the new
  desktop application. The underlying `Messaging` SDK type is kept because
  HashMail is built on it.
* `node/README.md` (stale, says the node is unimplemented) is rewritten.

---

## 6. Duplicated, incomplete, insecure, prototype-quality

### 6.1 Duplicated
* `hashgram.chat.v1.Attachment` and the new `hashgram.app.v1.BlobRef` carry
  the same fields (deliberate: avoids an import cycle; documented in the proto).
* Two `now()` helpers per SDK module; three copies of the "providers then
  store peers" candidate ordering (`blob.rs`, `messaging.rs`). Consolidated in
  the SDK `net` helpers during this work where touched.
* `apps/desktop/src-tauri` re-implements hex↔struct views the SDK could own
  (`MessageView`, `MediaView`). Addressed by the new SDK view types.

### 6.2 Incomplete (declared, not hidden)
* `x/serviceproof` storage assignment: mainnet `assigners: []`, so storage
  credit is inert until governance registers an assigner.
* `x/welcome` attestors: `[]` on mainnet; welcome rewards inert.
* `app/upgrades.Registry()` is empty; no upgrade has ever been exercised.
* `x/identity` has no `MsgUpdateParams`.
* Four `ServiceKind`s (IDENTITY, STORAGE, RELAY, CALL) are declared but never
  charged.
* No SDK invariants registered in any Hashgram module.
* `x/treasury` and `cmd/` have zero tests.
* `hashgram-client` has zero tests.
* Blob storage has **no retention, TTL or reclamation policy**.
* Social equivocation (same `(author, sequence)`, different id) is stored but
  never detected.
* `dns_seeds.txt` empty; one bootstrap operator.
* Docs say Mainnet "not launched" and desktop "not built"; both are false.

### 6.3 Insecure / needs fixing (found in this audit)

Rust node:

| ID | Severity | Finding | Action in this transformation |
| --- | --- | --- | --- |
| F1 | High | A `MailboxFetch{cursor:[], limit:0}` signature is byte-identical to the TURN credential request; a store node can replay a legitimate fetch to obtain TURN credentials labelled with the victim's device (`calls.rs:121`, `api.rs:954`). | **Fixed**: TURN requests must now carry a non-zero `limit` sentinel that `validate::mailbox_fetch` refuses for real fetches (`TURN_SENTINEL_LIMIT`), and the SDK/desktop path is updated. A dedicated purpose needs a `hashgram-net` purpose-set change on both Go and Rust and is scheduled (see ARCHITECTURE §12). |
| F2 | High | Signed `MailboxFetch`/`MailboxAck`/`BlobPutManifest` bind a timestamp but no responder identity; replayable within ±300 s across store nodes holding the same mailbox. | **Documented + mitigated**: SDK now uses per-peer fresh timestamps and the node refuses a `(mailbox, timestamp, cursor)` triple it has already served within the window (`mailbox.rs` replay cache). Full fix (responder peer id in preimage) is a wire change scheduled for protocol v2. |
| F3 | Medium | Handshake `roles` unbounded (≈1 MiB per peer, persisted). | **Fixed**: bounded to 16 roles × 16 bytes at `swarm.rs` verify; excess rejected as `MalformedFrame`. |
| F4 | High | Partial blob manifests reserve quota forever; per-uploader key unauthenticated. | **Fixed**: incomplete-upload TTL (24 h) sweep in the maintenance tick; quota released on expiry. |
| F5 | Medium | Unbounded pending-receipt table. | **Fixed**: cap `MAX_PENDING_RECEIPTS = 50_000`, oldest dropped, metric exposed. |
| F6 | Low | `/v1/status` walks every stored chunk. | Fixed: `blob::stats()` uses the `uploader_usage` totals. |
| F7 | Low | Frame memory amplification (64 streams × 128 peers × 1 MiB). | Documented; global in-flight budget is a libp2p-level change deferred. |
| F8 | Design | Mailbox deposit griefing (anyone may fill a mailbox). | Documented in PRIVACY_MODEL / HASHMAIL as a stated tension; spam defence is at the application layer. |
| F9 | Low | Equivocation undetected. | Documented; indexer detection scheduled. |
| F10 | Low | Orphaned expiry rows on ack. | Left; bounded by retention. |
| F11 | Low | Receipt batch all-or-nothing. | Documented. |
| F12 | Docs | Stale `node/README.md`. | Rewritten. |

Go chain (no consensus change is made now; these are recorded for the first
governance upgrade, see `HASHGRAM_ONE_ARCHITECTURE.md` §12):

| ID | Severity | Finding |
| --- | --- | --- |
| G1 | High | `MsgSubmitReceipts` scores the **named provider** (+50) for a receipt with a bad signature although anyone may submit; two forged receipts jail and slash an innocent provider (`x/serviceproof/keeper/receipts.go:196`). Requires a consensus upgrade to fix (score submitter, or only when submitter == operator). |
| G2 | High | `ExpireChallenges` walks every challenge ever issued on every block; challenges are never pruned (`challenges.go:229`). Consensus upgrade: prune answered challenges, index open ones. |
| G3 | Medium | `OpenChallenges` REST endpoint does a full-table scan without pagination. |
| G4 | Medium | Revoked devices accumulate unboundedly per identity; `?include_revoked=true` returns them all. |
| G5 | Medium | `ReverseLookup` unbounded. |
| G6 | Low | `RevokeDevice` does not check `identity.Revoked` (dead read). |
| G7 | Low | Indexer registry sync silently stops after 200k rows; `LIKE` wildcards unescaped. **Fixed in indexer (off-chain).** |

### 6.4 Prototype-quality
`hashgram-node/blob.rs` lifecycle, `rewards.rs` durability, `safety.rs`,
`api.rs` (unauthenticated loopback convenience), `hashgram-client` (untested),
`hashgram-mls` state persistence (whole-store snapshot per save).

---

## 7. Which functionality supports Hashgram One

| Pillar | Existing foundation | Gap filled by this work |
| --- | --- | --- |
| Hash Identity | `x/identity`, `x/username`, vault, device certs, recovery | `alice@hashgram.io` ↔ username resolution; device reconciliation into MLS groups; `DeviceSync` key handoff. |
| HashMail | MLS + mailboxes + key packages + blobs | `MailMessage` model, threading, folders/flags store, receipts, spam/contacts policy, BCC semantics, attachments (inline / blob / Drive capability). |
| HashDrive | Blob protocol, private encryption | Segmented streaming encryption, encrypted `DriveManifest`, folders, versions, trash, capabilities (snapshot/live), revocation semantics, multi-device keyring. |
| People | Social `FOLLOW`/`PROFILE_UPDATE`, chain lookups | Contact requests over MLS, friend/block/mute/trusted states, unified search (username / mail address / wallet). |
| Feed | Social events, indexer | Typed Feed API (friends / following / explore), pagination, polls, Drive refs, private posts via Circles. |
| Circles | MLS groups | `CircleEvent` content, history policy, membership → MLS roster. |
| Spaces | MLS groups | Signed hash-chained `SpaceEvent` log with role state machine, Space Drive (folder capabilities), announcements, posts. |
| Earn | `x/serviceproof`, node rewards agent | Provider lifecycle API, earnings view, storage-market ADR. |
| Wallet | `hashgram-chain`, relay | Unchanged; wrapped in SDK `wallet` facade. |
| Network / Top holders | Indexer | Balances projection, leaderboards, validators, providers, stats. |

---

## 8. Protocol changes and compatibility

* **Wire-compatible.** No change to `/hashgram/rpc/1` request/response
  shapes, gossip topics, or limits. `CHAT_KIND_APP` is a new enum value inside
  MLS plaintext; nodes never see it, old clients ignore it.
* **Vault-compatible.** New state lives under new `extra` keys
  (`mail_*`, `drive_*`, `people_*`, `space_*`) and in the SDK local store.
* **Chain-compatible.** No `x/` change. Everything Hashgram One needs on chain
  (identity, devices, usernames, balances, providers) already exists.
* **Breaking if done later:** F1 full fix (new signing purpose) and F2 full
  fix (responder id in preimage) change signed preimages and require a
  coordinated protocol minor version. Both are specified in the architecture
  document with a rollout plan.

---

## 9. Test inventory at baseline

Go: 28 files / 360 functions. Rust: 232 `#[test]` + 16 `#[tokio::test]`, 8
fuzz targets. JS: 34 `it()`. E2E: devnet, four-validator, phase2 (63 checks).
Mainnet verification 2026-09-12 (this session): relay + client flows pass.
