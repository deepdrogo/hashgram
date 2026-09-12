# Hashgram One — AI Handoff

Purpose: let another engineering session (human or AI) continue this
project with minimal rediscovery. Everything here is true of branch
`hashgram-one` of `deepdrogo/hashgram_windows` as of 2026-09-12. Where
something is unfinished it is listed as such; nothing is hidden.

---

## 1. What Hashgram One is

A private communication and storage platform on the existing Hashgram
chain (`hashgram-1`, Cosmos SDK, fixed 1,000,000,000 HASH supply) and
libp2p network. Consumer pillars: **Mail, Drive, People, Feed, Spaces,
Earn, Wallet, Network**. The blockchain holds only identity, usernames,
balances and provider records; every application payload travels inside
MLS ciphertext that no node decodes. Tagline: *One identity. One inbox.
One vault. One network.*

Positioning documents: `docs/HASHGRAM_ONE_ARCHITECTURE.md` (target
architecture, honest not-built list), `docs/HASHGRAM_ONE_AUDIT.md`
(baseline before the transformation, findings F1–F12 / G1–G7).

## 2. What was changed (this transformation)

| Area | Change | Where |
| --- | --- | --- |
| Wire protocol | New `hashgram.app.v1` package (AppMessage envelope + Mail/Drive/People/Circles/Spaces/DeviceSync bodies) carried inside MLS as `ChatMessage{kind: CHAT_KIND_APP, app}`. Additive; nodes unchanged. | `proto/hashgram/app/v1/app.proto`, `proto/hashgram/chat/v1/chat.proto`, `node/hashgram-proto/{build.rs,src/lib.rs}` |
| New crate | `hashgram-app`: pure application protocol (versioning gate, mail model/threading/bounds, spam policy, drive segment crypto + manifest + merge, space signed log + roles, circles, people state machine, app signing domains). 39 unit tests + fuzz smoke. | `node/hashgram-app/` |
| SDK | Application layer: `HashgramOne` facade and per-app APIs; encrypted local store; sync engine; devices; wallet/provider/network facades; backup; storage leases; AI interfaces. 26 unit tests. | `sdk/rust/hashgram-sdk/src/{app,mail,drive,people,feed,circles,spaces,devices,sync,store,wallet,provider,network,backup,storage_lease,ai}.rs` |
| SDK fixes | `Messaging::sync`: Welcomes processed first, failed envelopes retried and **not acked** (previously lost); DHT provider lookups bounded (`PROVIDER_QUERY_TIMEOUT` 3 s); `mls_mut`, `deliver_raw`; `Mail::send_built`. | `sdk/rust/hashgram-sdk/src/{messaging,link,mail}.rs` |
| Node | Security fixes F1 (TURN sentinel), F2 (mailbox replay cache), F3 (handshake role bounds), F4 (incomplete-upload TTL), F5 (pending receipt cap), F6 (O(1) stats); Kademlia provider queries answer on first result; metrics counters; README rewritten. | `node/hashgram-node/src/{calls,api,mailbox,blob,rewards,main}.rs`, `node/hashgram-p2p/src/swarm.rs`, `node/hashgram-proto/src/{limits,validate}.rs`, `node/README.md` |
| Indexer (Go) | Balances + validators projections, providers `total_paid`, leaderboards (`/v1/leaderboards/{holders,validators,providers,earners}`), `/v1/validators`, `/v1/network/stats`, G7 fixes (pagination cursor persistence, LIKE escaping). 18 tests. | `indexer/{balances,validators,accounts,api_network}.go`, `docs/INDEXER.md` |
| Gateway | `hashgram-mail-gateway`: SMTP server (no TLS/AUTH; fronted), SMTP client with STARTTLS, MIME ↔ `MailMessage`, DKIM rsa/ed25519, MX lookup, SQLite queues, policy, metrics. 80 tests. | `services/mail-gateway/`, `docs/MAIL_GATEWAY.md` |
| Devtools | Mock chain gateway gained a DEVNET identity/username registry (`POST /devnet/identity`) so unfunded test identities can be resolved. | `node/hashgram-devtools/src/bin/mock_gateway.rs` |
| CLI | `hashgram-client one …` — the developer harness over the facade (mail, drive, people, feed, circle, space, devices, provider, network, sync, balance, backup). | `node/hashgram-client/src/one.rs` |
| Tests | `scripts/testnet/hashgram-one-e2e.sh` (31 assertions on a local devnet), `node/fuzz/fuzz_targets/app_message.rs`. | |
| Docs | HASHMAIL, HASHDRIVE, SPACES, SYNC_ENGINE, PRIVACY_MODEL, MULTI_DEVICE_SECURITY, ADR_HASH_STORAGE_MARKET, MAIL_GATEWAY, INDEXER, DESKTOP_APP_MASTER_PROMPT, DESKTOP_APP_IMPLEMENTATION_CHECKLIST, this file; README/ARCHITECTURE status corrected (Mainnet launched; desktop exists). | `docs/` |

**No consensus change was made.** No `x/` module, genesis, param or
tokenomics value changed. Mainnet nodes built before this branch
interoperate with Hashgram One clients (verified against the live genesis
node on 2026-09-12).

## 3. Current architecture (one screen)

```
Desktop / CLI ──► hashgram_sdk::HashgramOne ──► Mail Drive People Feed Circles Spaces Devices Sync Wallet Provider Network
                         │                         (views over)  hashgram_app (pure rules/crypto)
                         ├─ Messaging (MLS groups over mailboxes)  ─┐
                         ├─ blob (encrypted objects)                ├─► hashgram-node store/relay/media (unchanged wire)
                         ├─ social (signed public events)          ─┘
                         ├─ ChainClient (P2P relay, cross-checked)  ─► hashgramd REST via relay nodes
                         └─ LocalStore (encrypted redb) + Vault (Argon2id/XChaCha20)
services/mail-gateway  ──► owns a bridge identity; SMTP in/out; sees only external plaintext
indexer (Go/Postgres)  ──► public read model: balances, leaderboards, validators, social
```

## 4. Major directories

See `README.md` "Repository layout" and `docs/HASHGRAM_ONE_AUDIT.md` §1.
New: `node/hashgram-app`, `services/mail-gateway`, `proto/hashgram/app/v1`.

## 5. Build

```bash
# Rust (workspace root is node/; rust-version 1.90; use nice on shared hosts)
cd node
cargo build --release --locked -p hashgram-node -p hashgram-client -p hashgram-mail-gateway
cargo build -p hashgram-devtools                 # mock-gateway (devnet only)
# Go
go build ./...
```

CI (`scripts/dev/ci.sh`, `.github/workflows/ci.yml`) runs rustfmt, clippy
`-D warnings` with `--exclude hashgram-desktop --exclude hashgram-node-service`,
`cargo test --workspace` (same excludes), cargo-audit, Go lint/test/fuzz,
docs policy (`scripts/dev/check-docs.sh` — note it prefers
`node/target/release/hashgram-client` and will flag `one` until a release
client with the new command is built).

## 6. Run

Local devnet with everything: `scripts/testnet/hashgram-one-e2e.sh debug`
(needs the three debug binaries). Manual devnet: see the script's setup
section (mock gateway on 31417, node on 26790, clients configured with
`--network devnet --chain-api http://127.0.0.1:31417`).

Mainnet client (read-only + Drive works without funds):
```bash
export HASHGRAM_PASSPHRASE=…
hashgram-client --home ~/.hashgram-client configure --network mainnet \
  --genesis-hash e322bc2319f6e0173286fa526dab5a8ff8ad0797c7b80dd03e7c9d98621d5e4d
hashgram-client identity create --offline
hashgram-client one sync && hashgram-client one balance && hashgram-client one drive ls /
```
Sending mail requires both parties to have on-chain identities (gas) and
published key packages.

Gateway: `hashgram-mail-gateway init|register|check-config|dns|run` with
`gateway.toml` (see `services/mail-gateway/gateway.example.toml`,
`docs/MAIL_GATEWAY.md`).

## 7. Test

```bash
cd node && cargo test --workspace --exclude hashgram-desktop --exclude hashgram-node-service   # 366 tests
go test ./indexer/... ./x/... ./app/...                                                        # Go
scripts/testnet/hashgram-one-e2e.sh debug                                                      # 31 e2e checks, ~45 s
cd node/fuzz && cargo +nightly fuzz run app_message                                            # optional
```

Results on 2026-09-12: all Rust and Go tests green; e2e 31/31; Mainnet
smoke (Drive round trip through the genesis node's relay, balance,
provider list, network overview) passed.

## 8. Security constraints (do not regress)

* Keys never leave Rust; the frontend gets views only (see
  `DESKTOP_APP_MASTER_PROMPT.md` §0).
* `AppMessage` readers go through `hashgram_app::envelope::open`
  (version gate → `Unsupported`, never partial rendering).
* Mail displays `authenticated_sender` (MLS) — a claimed `from` that
  differs is dropped as impersonation (`sdk/mail.rs::receive_mail`).
* Space events: signature + `actor == MLS sender` + role table in
  `hashgram_app::space::State::apply`; the SDK applies locally before
  sending so rule violations never leave the device.
* Drive: every object is segment-AEAD with index+size AAD; plaintext hash
  verified; capabilities only inside MLS; revoke + fresh key per version.
* Local store sealed per record with AAD = namespace‖key.
* Backup format `HGBKUP` v1 excludes device seed and MLS state.
* Node: TURN requests need the sentinel limit; mailbox replay cache;
  handshake roles bounded; incomplete uploads expire.
* Never log subjects/bodies/keys; addresses never as metric labels
  (`docs/LOGGING_POLICY.md`).

## 9. Main APIs

`sdk/rust/hashgram-sdk/src/app.rs` → `HashgramOne::{open, with_account,
save, mail(), drive(), people(), feed(), circles(), spaces(), devices(),
sync(), wallet(), provider(), network_api(), leases()}`. The full method
list with signatures is in `DESKTOP_APP_MASTER_PROMPT.md` §1.5; each
module's doc comment explains its model.

## 10. Known limitations

* **Funding a new user**: identity registration needs gas; welcome
  attestors are empty on Mainnet; there is no faucet. A brand-new user can
  use Drive locally and read the chain but cannot receive mail until
  registered.
* **Single operator on Mainnet**: one bootstrap/store/relay node; chain
  reads are "verified by 1 node".
* **Storage market**: only the client side of ADR option B; `LeaseOffer`
  RPCs and node acceptance are protocol v1.1 work.
* **Gateway**: no TLS/AUTH on the inbound SMTP listener (front it), no
  inbound SPF/DKIM verification (reads `Authentication-Results` from a
  fronting MTA), no DSN to Internet senders.
* **Drive**: one manifest per identity (fine to ~10⁵ entries); no
  provider-enforced deletion; two devices that created Drives before ever
  syncing are not merged automatically.
* **Mail search** is a bounded linear scan in the SDK; the desktop should
  keep its own index for large mailboxes.
* **Circles** have no roles; any member may add members.
* Protocol v1.1 (F1 dedicated purpose, F2 responder binding) and the Go
  consensus fixes G1–G6 are specified, not shipped.

## 11. Future consensus upgrades (specified, not done)

`docs/HASHGRAM_ONE_ARCHITECTURE.md` §12 and `docs/ADR_HASH_STORAGE_MARKET.md`
§6: G1 (score submitter of forged receipts), G2 (prune challenges), G3–G6
pagination/checks, `x/identity` `MsgUpdateParams`, `x/storagemarket`.

## 12. Desktop readiness

The SDK surface the desktop needs exists and is exercised end to end by
`hashgram-client one` and the e2e script. `docs/DESKTOP_APP_MASTER_PROMPT.md`
is the standalone build prompt; `docs/DESKTOP_APP_IMPLEMENTATION_CHECKLIST.md`
the staged plan. The previous app (`apps/desktop`, v0.1.1) still compiles
against the unchanged legacy SDK modules; its messaging/reels/channels/calls
screens are to be removed by the desktop task, not by platform work.

## 13. Incomplete items / high-priority TODOs

1. Protocol v1.1: `LeaseOffer/LeaseStatus/LeaseTerminate` RPC arms +
   node-side lease acceptance; `turn-credential` signing purpose (F1
   full fix); responder peer id in `MailboxFetch/Ack/BlobPutManifest`
   preimages (F2 full fix). Requires Go+Rust purpose-set change and a
   coordinated release.
2. Streaming Drive upload/download API in the SDK (segment-by-segment to
   disk) for files above a few hundred MiB; the primitives exist
   (`SegmentEncryptor/Decryptor`, `upload_sealed`), the convenience
   wrapper does not.
3. Gateway inbound TLS/AUTH or a documented fronting MTA recipe with
   SPF/DKIM verification; DSNs.
4. Indexer: equivocation detection for social events (F9).
5. Governance: register a storage assigner and welcome attestors, or
   ship `x/storagemarket`.
6. Go consensus fixes G1–G6 in the first upgrade.
7. `hashgram-client` unit tests (still zero; the e2e script covers it).

## 14. Known technical debt

* `Messaging` persists MLS state as a whole snapshot per save; fine for
  hundreds of groups, not thousands.
* `MailState` indexes are rebuilt from a full store scan at open.
* `Feed::timeline` scans all cached events; acceptable for followed-author
  feeds, not for an Explore feed (use the indexer).
* Two copies of the "providers then store peers" ordering remain in
  `blob.rs`/`messaging.rs`.
* `hashgram.chat.v1.Attachment` and `hashgram.app.v1.BlobRef` are
  duplicate shapes (import-cycle avoidance).

## 15. Git

Branch `hashgram-one`, 16 commits on top of `main` (`17c1f1e`), pushed to
`origin` (`deepdrogo/hashgram_windows`) on 2026-09-12. Open a PR against
`main` when reviewed. The genesis node runs the release binaries built
from this branch (`/usr/local/bin/hashgram-node`, previous binary kept as
`hashgram-node.prev`). The public `deepdrogo/hashgram`
repository lags this one (it does not even contain the chain relay) and
should be synced separately.
