# ADR: Long-Term Paid Storage in HASH

| | |
| --- | --- |
| Status | **Accepted** for option B (off-chain leases); option C **specified, not implemented** |
| Date | 2026-09-12 |
| Branch | `hashgram-one` |
| Consensus impact of this ADR | **None.** No `x/` module, param, message or store key changes on this branch. |
| Related | `SERVICE_REWARDS.md`, `TOKENOMICS.md`, `STORAGE.md`, `DECENTRALIZATION.md`, `HASHGRAM_ONE_ARCHITECTURE.md` §10, audit G1–G6 |

Spelling is British throughout.

---

## 1. Context

**The reserve is finite and declining.** `x/serviceproof` pays providers
from a 500,000,000 HASH reserve created at genesis and never topped up
(`TOKENOMICS.md`). Each epoch (21,600 blocks ≈ 24 h) the budget is
`min(floor(remaining × 5 / 10,000), 250,000 HASH)`. The schedule is
geometric: ~16 % of the reserve remains after ten years, ~2.6 % after
twenty. Per-epoch subsidy therefore falls by roughly half every 3.8 years.
There is no minting and no mechanism to refill the reserve; the supply
invariant forbids one.

**Storage credit today requires chain assignment.** Bytes earn only when a
registered *assigner* records a `StorageAssignment` (`MsgAssignStorage`)
and the provider then answers chain-issued Merkle challenges (4 per epoch,
chunk index from the previous app hash, `challenges.go`). Declared capacity
earns nothing. Mainnet genesis ships `assigners: []`; **no storage credit
has ever been paid on Mainnet**, and none can be until governance names an
assigner, which `DECENTRALIZATION.md` lists as a centralisation point.

**Providers hold ciphertext.** Drive objects, attachments and manifests are
client-encrypted (`drive.rs`); the provider knows a CID, a size and a chunk
count. It cannot select what to keep by content and has no relationship
with the owner beyond the uploader's device key on `BlobPutManifest`.

**Blobs have no retention policy.** `STORAGE.md`: a node holds what it
accepted until its operator deletes it. Nothing tells a provider which
blobs anyone still wants, so the honest choices are "keep everything"
(unbounded disk) or "delete arbitrarily" (data loss for users who cannot
tell).

**One token.** HASH is the only asset. There is no second storage token,
no stablecoin, no bridge, and this ADR does not introduce one.

**Governance is slow and the chain is live.** Any consensus change is a
coordinated binary upgrade adopted by the validator set through
`x/upgrade`; `app/upgrades.Registry()` is empty and no upgrade has ever
been exercised. Clients ship before the first upgrade.

## 2. Problem

Users need a way to pay a provider, in HASH, to keep specific encrypted
bytes available for a specified period, such that:

* payment is for **demonstrated availability**, not for claimed capacity or
  for a promise;
* the arrangement survives the decline of the reward reserve, i.e. the
  money comes from the user, not from emission;
* the provider learns nothing about the content and as little as possible
  about the user;
* no single party (assigner, marketplace operator, gateway) is required for
  the arrangement to exist;
* it can ship **now**, without a consensus change, and evolve into
  something stronger when governance is ready.

## 3. Options considered

### A. Do nothing — reward-only storage

Rely on `x/serviceproof` emission and eventually an assigner set.

### B. Off-chain storage leases paid by ordinary bank transfers

The client and a provider agree a signed `StorageLease`; the client pays by
`MsgSend` (or `x/authz`-delegated periodic sends) and includes the lease id
in the memo; the provider signs a lease receipt; the **client verifies
availability itself** with `BlobHas` and sampled chunk fetches through the
existing blob protocol, and stops paying when verification fails. No
consensus change.

### C. On-chain escrow module `x/storagemarket`

A consensus module with lease objects, provider offers, escrow, per-epoch
settlement conditioned on challenge success (the existing
`x/serviceproof` challenge machinery, assigned by lease), refunds and
disputes. Requires a governance upgrade.

### D. Payment channels

A unidirectional HASH payment channel per (client, provider) opened on chain,
micropayments per proven period off chain, closed on chain. Requires a
channel module (consensus change) *and* client-side verification like B.

## 4. Evaluation

| Criterion | A: reward-only | B: off-chain lease | C: `x/storagemarket` | D: payment channels |
| --- | --- | --- | --- | --- |
| No consensus change now | Yes | **Yes** | No (module, store key, upgrade) | No (channel module) |
| Sustainable after the reserve declines | **No**: subsidy → 0 asymptotically; nothing replaces it | **Yes**: user-funded | **Yes**: user-funded | **Yes**: user-funded |
| Verifiability — never pay for claimed capacity | Chain challenges (strong) but inert on Mainnet (no assigner) | Client-verified: `BlobHas` + sampled chunk fetch with hash checks. Strong *for the paying client*, unobservable by third parties | Chain-verified via challenges assigned by lease (strong, public) | As B for verification; channel only moves money |
| Abuse resistance | Concentration cap, bond, jail | Provider can take one period's payment then vanish (bounded loss = one period); client can stop paying after service (bounded loss = one period). No slashing | Escrow + bond + slashing; refund on failure | Channel bounds loss to one micro-period; dispute on close |
| UX | Free to users; but nobody is paid on Mainnet today | Client must stay online periodically to verify and pay; auto-pay via `x/authz` grant possible; provider discovery via existing DHT + provider registry | Pay once, escrow settles; best UX | Complex (open/close, watchtower for unilateral close) |
| Decentralisation — no single assigner | **Worst**: depends on a governance-named assigner set | **Best**: any client, any provider, no registry required | Good: assignment by lease removes the assigner role | Good |
| Privacy | Provider sees uploader device key | Provider sees payer address ↔ CIDs (§7.3) | Lease is **public on chain**: address ↔ CID ↔ provider ↔ size | Channel on chain links address ↔ provider; CIDs off chain |
| Implementation cost | 0 | SDK + node acceptance path; ~weeks | Module + upgrade + audit; ~months plus governance | Module + SDK + watchtower; largest |

Assessment:

* **A** fails sustainability and, on Mainnet as shipped, pays nothing at all.
* **D** costs more than C and delivers less; payment channels solve a
  high-frequency payment problem this workload does not have (one payment
  per epoch per lease is trivially affordable on chain).
* **B** meets every "now" criterion. Its weaknesses are bounded loss without
  slashing and dependence on the client being online to verify. Both are
  acceptable for a first version and are exactly what C later fixes.
* **C** is the right end state but cannot ship on this branch, and its
  design benefits from data B will produce (real prices, real failure
  rates, real lease sizes).

## 5. Decision

**Implement B now, as `hashgram-sdk::storage_lease` plus provider-side
acceptance in `hashgram-node`. Specify C as the first storage-related
governance upgrade. Make no consensus change on this branch.**

### 5.1 Protocol messages (off-chain, `hashgram.p2p.v1`, additive)

New RPC bodies on `/hashgram/rpc/1`, additive to `rpc.proto`; nodes that do
not know them return the existing `Unsupported` result and clients treat the
provider as not offering leases.

```protobuf
// A signed agreement that `provider` keeps `blob_ids` available for the
// client from start_epoch to end_epoch inclusive, at the stated price.
message StorageLease {
  uint32 version = 1;                 // 1
  string network_id = 2;              // refuse cross-network leases
  bytes  lease_id = 3;                // 16 random bytes chosen by the client
  string client = 4;                  // hash1… payer address
  bytes  client_device_pubkey = 5;    // the device that signs and verifies
  string provider = 6;                // provider operator address (x/serviceproof)
  bytes  provider_node_pubkey = 7;    // the node key that signs acceptance
  repeated bytes blob_ids = 8;        // CIDs, ≤ MAX_LEASE_BLOBS (256)
  uint64 total_bytes = 9;             // sum of ciphertext sizes, ≤ 4 GiB × 256
  uint32 replication = 10;            // replicas the provider promises to hold on distinct nodes it operates (advisory, see §7)
  uint64 price_uhash_per_gib_epoch = 11;
  uint64 start_epoch = 12;            // x/serviceproof epoch numbers
  uint64 end_epoch = 13;              // inclusive; ≤ start + MAX_LEASE_EPOCHS (365)
  uint64 created_at = 14;             // unix seconds
  bytes  client_signature = 15;       // device key, purpose "storage-lease" (app-level domain)
  bytes  provider_signature = 16;     // node key, same purpose; empty until accepted
}

message LeaseOffer      { StorageLease lease = 1; }                 // client → provider
message LeaseOfferResult{ bool accepted = 1; string reason = 2; StorageLease lease = 3; } // provider echoes with its signature
message LeasePaymentNotice { bytes lease_id = 1; uint64 epoch = 2; string tx_hash = 3; }   // client → provider after MsgSend
message LeaseReceipt    { bytes lease_id = 1; uint64 epoch = 2; string tx_hash = 3; uint64 amount_uhash = 4; uint64 at = 5; bytes provider_signature = 6; }
message LeaseStatus     { bytes lease_id = 1; }                      // either side
message LeaseStatusResult { StorageLease lease = 1; uint64 last_paid_epoch = 2; repeated uint32 blobs_missing_chunks = 3; bool terminated = 4; string reason = 5; }
message LeaseTerminate  { bytes lease_id = 1; uint64 at = 2; string reason = 3; bytes signature = 4; } // either side, signed
```

Signing uses `hashgram-app::signing` with a new **application-level**
purpose string `storage-lease`, built on `hashgram-net`'s canonical encoder
exactly as `space-event` is. It does **not** touch the 13 protocol purposes
shared with Go, so no `hashgram-net`/`x/network` change is needed.

Price arithmetic: `due_uhash(epoch) = ceil(total_bytes × price / GiB)`,
integer, computed identically by both sides; the SDK refuses a lease whose
per-epoch due is 0.

### 5.2 Payment

The client pays with an ordinary `MsgSend{from: client, to: provider
reward_address or operator, amount: due_uhash}` whose memo is
`hgl1:<lease_id hex>:<epoch>`. Nothing on chain interprets the memo; it is
the provider's evidence and the client's bookkeeping. Optional: the client
grants `x/authz` `SendAuthorization` with a spend limit to a local
"auto-pay" key so a device-only vault can pay without the account key.

Payment timing: **in arrears**, at the start of epoch *e + 1* for epoch *e*,
after the client's own verification of *e* passed. The provider's exposure
is therefore one epoch of service; the client's exposure is zero for
unverified service. A provider may require a **first-epoch prepayment**
(`LeaseOfferResult.reason = "prepay"`) to price the counterparty risk; the
SDK caps prepayment at one epoch.

### 5.3 Client-side verification (the rule that makes B honest)

The client **must** verify before paying; the SDK does this in the
`SyncEngine` storage sub-sync, per lease, per epoch:

1. `BlobHas(cid)` to the provider for every leased CID; require
   `has_manifest && chunks_present == chunks_total`.
2. Sampled chunk fetch: for each blob, fetch `k = max(2, ⌈log2(chunks)⌉)`
   chunk indices chosen from a local CSPRNG (the provider cannot predict
   them), verify each against the manifest's chunk hash; any mismatch or
   timeout is a failure. The client already holds every manifest (CID =
   BLAKE3(manifest)) so no trust in the provider's manifest is needed.
3. Record `{lease_id, epoch, passed, failures}` in the local store.
4. Pay only if passed. On failure: do not pay; retry within the epoch up to
   3 times; if still failing, mark the epoch unpaid, send `LeaseTerminate`
   if two consecutive epochs fail, and surface "provider X failed
   verification; re-replicate" to the user. The SDK then uploads to another
   provider using the ordinary blob path.

This reuses the challenge *idea* of `x/serviceproof` — random chunk,
hash-verified — without the chain: the paying client is its own assigner
and its own verifier. The provider learns nothing new: it already serves
chunks to anyone with the CID.

### 5.4 Provider-side acceptance (`hashgram-node`)

* Config: `[lease] enabled, price_uhash_per_gib_epoch, max_total_bytes,
  max_lease_epochs, require_prepay`.
* On `LeaseOffer`: verify network, client signature, device authority on
  chain (existing check), `price ≥ configured`, capacity within
  `max_total_bytes` minus outstanding leases, blobs already complete
  locally (`BlobHas` self-check); then sign and store the lease in a new
  redb table `leases` keyed by `lease_id`, and **pin** the blobs
  (reference-counted so a blob under two leases is pinned twice).
* On `LeasePaymentNotice`: query the co-located chain node for the tx,
  verify recipient/amount/memo, record `last_paid_epoch`, return a
  `LeaseReceipt`.
* Maintenance tick: leases unpaid for `grace_epochs` (default 2) are
  terminated and unpinned. **Unpinned blobs become eligible for the
  operator's retention policy** — this is the first mechanism in Hashgram
  that tells a provider what it may delete, and is a direct consequence of
  this ADR (`PRIVACY_MODEL.md` §7).
* Metrics: `hashgram_leases_active`, `hashgram_lease_bytes_pinned`,
  `hashgram_lease_payments_total{outcome}`; no client addresses in labels.

### 5.5 Interaction with rewards

A leased blob may **also** be assigned on chain by an assigner and earn
emission; the two are independent. The provider is paid twice for the same
bytes in that case, which is intended during the transition: emission is
subsidy, lease is revenue. Under C the two merge (§6.6).

## 6. Option C — specification for the governance upgrade

Written so the module can be implemented without re-deriving the design;
numbers are proposals for the governance discussion, not decisions.

### 6.1 Module

`x/storagemarket`, store key `storagemarket`, consensus version 1, added to
`app/app.go` `NewKVStoreKeys` and to the module manager after
`serviceproof` (it depends on `serviceproof`'s epoch and provider keepers).
Module account `storagemarket_escrow` with **no** `Minter` permission
(`app_test.go` `TestNoModuleCanMint` continues to hold).

### 6.2 State

| Key | Value | Notes |
| --- | --- | --- |
| `0x01 ‖ lease_id` | `Lease` | See below |
| `0x02 ‖ provider ‖ lease_id` | `[]` | Index: leases by provider |
| `0x03 ‖ client ‖ lease_id` | `[]` | Index: leases by client |
| `0x04 ‖ end_epoch(BE) ‖ lease_id` | `[]` | Index: expiry queue |
| `0x05 ‖ provider` | `ProviderOffer` | Published price and capacity |
| `0x06 ‖ lease_id ‖ epoch(BE)` | `EpochSettlement` | Paid / refunded per epoch |
| `0x07` | `Params` | |

```protobuf
message Lease {
  bytes  lease_id = 1;
  string client = 2;
  string provider = 3;
  bytes  merkle_root = 4;        // over the leased blobs' chunk hashes, same tree as StorageAssignment
  uint64 total_bytes = 5;
  uint32 chunk_size = 6;
  uint32 chunk_count = 7;
  uint32 replication = 8;
  uint64 price_uhash_per_gib_epoch = 9;
  uint64 start_epoch = 10;
  uint64 end_epoch = 11;
  cosmos.base.v1beta1.Coin escrow = 12;     // remaining
  uint64 last_settled_epoch = 13;
  LeaseState state = 14;                    // PENDING, ACTIVE, TERMINATED, EXPIRED, DISPUTED
  uint64 failed_epochs = 15;
}
message ProviderOffer { string provider = 1; uint64 price_uhash_per_gib_epoch = 2; uint64 available_bytes = 3; uint64 max_lease_epochs = 4; }
message EpochSettlement { uint64 epoch = 1; uint64 challenges_issued = 2; uint64 challenges_passed = 3; cosmos.base.v1beta1.Coin paid = 4; cosmos.base.v1beta1.Coin refunded = 5; }
```

**Privacy note, stated up front:** a `Lease` publishes `client ↔ provider ↔
merkle_root ↔ total_bytes` permanently. It does **not** publish CIDs (the
Merkle root over chunk hashes is one-way and the chunk hashes are of
ciphertext), but it does publish that *this address pays for this much
storage with this provider*. B publishes only a `MsgSend` with a memo. This
is a real cost of C and the governance proposal must say so.

### 6.3 Messages

| Message | Signer | Effect |
| --- | --- | --- |
| `MsgPublishOffer{price, available_bytes, max_epochs}` | provider operator | Upsert `ProviderOffer`; requires an active, unjailed `x/serviceproof` provider with `STORAGE` role |
| `MsgCreateLease{lease_id, provider, merkle_root, total_bytes, chunk_size, chunk_count, replication, epochs, escrow}` | client | Requires `escrow ≥ due × epochs`; moves escrow to the module account; state `PENDING`; **also writes `StorageAssignment`s** for the provider via the serviceproof keeper, with `assigner = module address` (§6.6) |
| `MsgAcceptLease{lease_id, node_signature}` | provider operator | `PENDING → ACTIVE`; must arrive within `accept_window_blocks` or the lease auto-refunds |
| `MsgTerminateLease{lease_id}` | client **or** provider | Client: refund unspent escrow minus `early_termination_bps`; provider: refund all unspent escrow and add `fraud_score += provider_termination_penalty` |
| `MsgDispute{lease_id, epoch}` | client | Marks an epoch disputed if challenges passed but the client's own sampled fetch failed; resolved by the next epoch's challenges (2 consecutive chain passes → dismissed; a chain failure → refund + slash) |
| `MsgUpdateParams` | governance authority | |

### 6.4 Settlement (per epoch, in `x/serviceproof` `BeginBlocker` after `SettleEpoch`)

For every `ACTIVE` lease with `start_epoch ≤ e ≤ end_epoch`:

```text
issued, passed = challenges this epoch on the lease's assignments
due            = ceil(total_bytes × price / GiB)
if issued == 0: carry (no evidence, no payment, no refund; escrow untouched)
else:
  paid    = due × passed / issued     → provider reward_address
  refund  = due − paid                → client
  if passed < issued: failed_epochs += 1
  if failed_epochs ≥ params.max_failed_epochs: state = TERMINATED, refund remaining escrow
escrow −= due; last_settled_epoch = e
if e == end_epoch: state = EXPIRED, release assignments
```

Settlement never fails a block (errors logged, retried), consistent with
`settlement.go`'s stated rule.

### 6.5 Params

| Param | Proposed default | Bound in `Validate` |
| --- | --- | --- |
| `enabled` | true | |
| `min_lease_epochs` | 7 | ≥ 1 |
| `max_lease_epochs` | 365 | ≤ 3,650 |
| `max_lease_bytes` | 1 TiB | ≤ 16 TiB |
| `accept_window_blocks` | 21,600 | ≥ 100 |
| `max_failed_epochs` | 3 | ≥ 1 |
| `early_termination_bps` | 500 (5 %) | ≤ 2,000 |
| `provider_termination_penalty` | 25 | ≤ 100 |
| `challenges_per_lease_epoch` | 4 | ≥ 1 |
| `min_price_uhash_per_gib_epoch` | 0 | |

### 6.6 What changes in `x/serviceproof`

* **Assignment by lease.** `MsgCreateLease` writes `StorageAssignment`s
  with the module account as assigner. The `assigners` param set remains
  (governance may still name assigners for subsidised public data) but is
  no longer the only source; the centralisation point in
  `DECENTRALIZATION.md` §3 "Storage assigners are a named set" becomes
  optional rather than load-bearing.
* **Challenge attribution.** `StorageChallenge` gains `lease_id` (0 for
  assigner-originated) so settlement can count per lease. Additive proto
  field; existing challenges decode with 0.
* **Emission for leased bytes.** Leased assignments accrue storage credit
  **at a governance-set fraction** `leased_credit_bps` (proposed 0 at
  upgrade, so leased bytes are paid by the client only and the reserve is
  preserved for public data; governance may raise it). This closes the
  double payment noted in §5.5.
* **G1/G2 fixes ride along.** The same upgrade should fix receipt scoring
  (G1) and prune answered challenges (G2), since C multiplies the number of
  challenges and G2 would otherwise become a liveness problem.

### 6.7 Upgrade plan

| Item | Value |
| --- | --- |
| Upgrade handler name | `v2-storagemarket` (entry in `app/upgrades.Registry()`) |
| Store upgrades | `Added: ["storagemarket"]` via `storetypes.StoreUpgrades` in the handler |
| Genesis params | `x/storagemarket` `Params` as §6.5 with `enabled = true`; `leased_credit_bps = 0` in `x/serviceproof` params |
| Module versions | `storagemarket` 1; `serviceproof` consensus version +1 with a migration that sets `leased_credit_bps` and initialises `lease_id = 0` on existing challenges (no-op for proto3 defaults) |
| Client migration | The SDK converts an active B lease to a C lease only on explicit user action (`storage_lease::migrate_to_chain`): funds escrow, creates the lease, and sends `LeaseTerminate` for the off-chain one after `MsgAcceptLease` lands. B remains supported indefinitely for clients that prefer not to publish a lease |
| Rollout | Proposal text must include the §6.2 privacy note and the emission change; testnet exercise via `scripts/testnet/phase2.sh` extended with a lease lifecycle check |

### 6.8 Invariants (registered with the SDK crisis module)

1. `sum(Lease.escrow over non-terminal leases) == balance(storagemarket_escrow)`.
2. For every `ACTIVE` lease, `last_settled_epoch ≥ start_epoch − 1` and
   `≤ current epoch − 1`.
3. Every `StorageAssignment` with the module as assigner references an
   existing non-terminal lease.
4. Total supply unchanged (existing `hashgram_supply_over_ceiling_uhash`).

## 7. Consequences

### 7.1 Positive

* Users can pay for storage on Mainnet today without waiting for
  governance; providers have revenue independent of the reserve.
* Providers, for the first time, know which bytes are wanted (pinned) and
  may delete the rest under a stated policy.
* The client-verifies rule means no HASH is ever paid for bytes a provider
  did not demonstrably serve to the payer.
* The assigner centralisation point is bypassed for paid storage now and
  removed structurally by C.
* One token; no new asset, no bridge, no oracle.

### 7.2 Negative

* **B has no slashing.** A provider that accepts prepayment and vanishes
  loses only reputation (the SDK keeps a local provider score; there is no
  shared one). Loss is bounded to one epoch's payment per lease.
* **B needs the client online** at least once per epoch per lease to verify
  and pay. A device off for a week misses payments; the provider's
  `grace_epochs` decides whether the blobs survive. The SDK warns before
  the grace period ends and supports `x/authz` auto-pay, but auto-pay
  without verification would defeat the rule, so auto-pay only pays epochs
  the device verified.
* **B's verification is private.** Nobody but the client knows a provider
  failed; a bad provider can fail many clients before its reputation
  suffers. C makes failure public.
* **C publishes leases.** Address ↔ provider ↔ size, permanently.
* **Replication is promised, not proven,** in both B and C (the challenge
  proves one copy can be produced; `THREAT_MODEL.md` §3). `replication` in
  the lease is priced as a promise. A client wanting real redundancy leases
  the same blobs from **two operators** and pays twice; the SDK's default
  lease flow offers exactly that.

### 7.3 Privacy

B: the provider learns `payer address ↔ set of CIDs ↔ paying device key`.
Today it already learns `uploader device key ↔ CID`; the new fact is the
address (public on chain anyway via the device key, `PRIVACY_MODEL.md`
§4.1) and the payment. Anyone reading the chain sees `MsgSend client →
provider, amount, memo hgl1:<id>:<epoch>` and can infer "client rents
≈ amount/price GiB from provider". A client that objects can pay from a
separate funded address; the SDK supports a distinct `payer` key.

## 8. Risks

| Risk | Likelihood | Impact | Mitigation |
| --- | --- | --- | --- |
| Providers refuse B because loss is unbounded from their side | Medium | B unused | Prepay option (one epoch); short `grace_epochs`; unpaid → unpin quickly |
| Clients pay without verifying (third-party clients) | Medium | Pays for nothing | The rule is normative in this ADR and in `storage_lease` docs; the reference SDK cannot pay unverified epochs |
| Price discovery fails (no market, one provider) | High early | Monopoly pricing | Offers are public over RPC; the indexer may list them (off-chain); C makes offers on-chain |
| Sampled verification misses partial loss | Low per epoch; compounding | Paying for 99 % of a file | k grows with chunk count; a single missing chunk is caught with probability 1 − (1 − 1/n)^k per epoch and certainly on full download |
| Lease pinning used to fill a provider's disk with junk | Medium | Denial of capacity | Provider caps `max_total_bytes` and requires payment before pinning beyond the first epoch |
| C's G2-style unbounded challenge walk | Certain without fix | Liveness | G2 fix is a prerequisite of the same upgrade |
| Escrow accounting bug in C | Low | Funds stuck or minted | Invariant 1 and 4; escrow module has no Minter; upgrade exercised on testnet first |

## 9. Open questions

1. Should B's `LeaseReceipt` be countersigned into a `ServiceReceipt`-like
   object that could later be honoured by C as evidence of pre-upgrade
   service? Currently no: keep B and C independent.
2. `leased_credit_bps` at upgrade: 0 preserves the reserve; a non-zero
   value would subsidise early leases and bootstrap the market. Governance
   question.
3. Should providers be able to publish offers on chain **before** C
   (a `MsgUpdateProvider` field)? It would be a consensus change for a
   convenience; rejected for now — offers travel over RPC and the indexer
   may aggregate them.
4. Dispute resolution in C relies on the next epoch's chain challenges. Is
   one epoch of latency acceptable, or should a dispute trigger an
   immediate challenge? Immediate challenges reintroduce a predictable
   chunk index unless derived from a future app hash; deferred.
5. Whether `x/authz` auto-pay should be able to pay for an epoch the device
   has not itself verified when *another* of the user's devices verified it
   (via `DeviceSync`). Probably yes; not designed here.
6. Deletion semantics for unleased blobs: provider policy only, or a
   protocol-level `retention_policy` announcement so clients can choose
   providers by it? Leaning towards the announcement (additive
   `announce.proto` field), not on this branch.

## 10. Statement on consensus

**No consensus change is made in this branch.** Everything in §5 is
off-chain: new RPC bodies inside the existing `/hashgram/rpc/1` protocol,
an application-level signing purpose, SDK code, node configuration and a
node-local table. Payment is ordinary `x/bank`. Option C (§6) is a
specification for a future governance proposal and touches `app/app.go`,
`app/upgrades`, `x/serviceproof` and a new module; none of that exists in
the tree at the time of this ADR.
