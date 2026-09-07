# Hashgram Architecture

How the pieces fit together, and why they were split the way they were.

This describes what is built. Phase 2 components are marked as such and are
described in the future tense.

## Layers

```text
┌─────────────────────────────────────────────────────────────────┐
│  Clients                          Windows / iOS / Android       │
│                                   (not built; specs only)       │
└─────────────────────────────────────────────────────────────────┘
                │                              │
                │ chain queries and txs        │ messaging, social, media
                ▼                              ▼
┌───────────────────────────────┐  ┌──────────────────────────────┐
│  hashgramd                    │  │  hashgram-node   (Phase 2)   │
│  Cosmos SDK v0.53 + CometBFT  │  │  Rust, libp2p, OpenMLS       │
│                               │  │                              │
│  Consensus, state, tokens,    │  │  E2EE messaging, social       │
│  identity registry, rewards   │  │  events, blob storage, calls │
└───────────────────────────────┘  └──────────────────────────────┘
                │                              │
                └──────────────┬───────────────┘
                               ▼
                    ┌──────────────────────┐
                    │  Shared identity     │
                    │  x/network domains   │
                    │  x/identity keys     │
                    └──────────────────────┘
```

The split matters. **Consensus carries only what must be globally agreed**:
balances, stake, the identity registry, reward settlement, governance. Message
content, social posts and media never touch the chain. They are exchanged
peer-to-peer and, in the case of private messages, are end-to-end encrypted so
that no node other than the participants can read them.

Putting messages on chain would have made them permanent, public and
replicated to every node forever. It would also have made throughput a
consensus problem. Both are the wrong trade for a messenger.

## What consensus stores

Everything the chain stores is public and permanent. That constrains what may
be put there, and the constraint is enforced by the choice of what to store
rather than by policy:

- Balances, stake, delegations, governance proposals and votes
- **Public keys only.** Root identity keys and device certificate public keys.
  No Hashgram component ever holds a user's private key.
- Username registrations and their confusable skeletons
- Useful-service provider registrations, bonds, receipts, storage assignments
  and challenge outcomes
- The Founder revenue ledger and the fee routing totals
- Network identity and the pinned genesis hash

What is deliberately absent: message content, message metadata beyond what
delivery requires, social post bodies, media bytes, IP addresses.

## The application

`hashgramd` is a Cosmos SDK v0.53 application on CometBFT v0.38. Both are the
2025.1 release family rather than the newest available, chosen because that
line is the most widely deployed, has complete tooling, and is supported by
`tmkms` for remote validator signing. Moving forward is what `x/upgrade` is
for.

### Modules

```text
app/app.go
├── Standard SDK
│   auth, bank, staking, slashing, distribution, gov, evidence,
│   vesting, upgrade, consensus, genutil, feegrant, authz
│
├── NOT wired, deliberately
│   x/mint      no inflation is possible, not merely configured to zero
│   x/circuit   no authority may halt a message type
│
└── Hashgram
    x/network        identity, genesis pinning, signing domains
    x/founder        Founder revenue ledger, compile-time fee ceiling
    x/feerouter      fee revenue split
    x/welcome        tiered joining reward
    x/serviceproof   Proof of Useful Service
    x/username       @name registry
    x/identity       root identities and device certificates
    x/treasury       named genesis allocations
```

### Excluding x/mint rather than zeroing it

A chain with `x/mint` configured for zero inflation still contains a code path
that creates coins, and a governance proposal that changes one parameter turns
it on. A chain without the module has no such path.

The stronger property is enforced twice. `maccPerms` in
[`app/app.go`](../app/app.go) grants no module account the `Minter`
permission, and [`app/app_test.go`](../app/app_test.go) asserts both facts:
that no module can mint, and that `x/mint` and `x/circuit` are not in the
module manager.

### Excluding x/circuit

`x/circuit` lets a designated authority disable specific message types at
runtime. It is a sensible feature for a chain that wants an emergency brake,
and it is precisely the "central kill switch" Hashgram is meant not to have.
Its absence means no account, including the Founder's, can stop a transfer.

## Block flow

```text
PreBlocker    x/upgrade      apply a scheduled upgrade at its height
              ↓
BeginBlocker  x/distribution pay the previous block's fees to validators
              x/feerouter    take the Founder share of gas, then sweep
                             service-fee revenue into the fee collector
              x/serviceproof advance the epoch; settle the closed one
              x/slashing     record signing information
              x/evidence     handle double-signing evidence
              ↓
Transactions
              ↓
EndBlocker    x/staking      apply validator set changes
              x/gov          tally completed proposals
              x/founder      pay accrued revenue if the period has elapsed
              app/metrics.go publish the hashgram_* gauges
```

The ordering in `x/feerouter`'s BeginBlocker is load-bearing rather than
arbitrary. Gas fees are measured **before** service-fee revenue is swept into
the fee collector. Reversing it would mean the swept service remainder sits in
the fee collector when the gas pass measures it, and the Founder share would
be taken from the same revenue twice. There is a test for exactly that:
[`x/feerouter/keeper/route_test.go`](../x/feerouter/keeper/route_test.go).

## Fee routing

```text
                    ┌──────────────────────────┐
   gas fees ───────▶│  fee collector           │
                    │  (x/auth module account) │
                    └──────────────────────────┘
                              │
                    BeginBlock pass 1
                              │
                     ┌────────┴────────┐
                     │                 │
                  1% founder      99% x/distribution
                                       → validators and delegators

                    ┌──────────────────────────┐
service fees ──────▶│  feerouter revenue pool  │
(username           └──────────────────────────┘
 registration,                │
 storage, relay)    BeginBlock pass 2
                              │
                     ┌────────┴────────┐
                     │                 │
                  1% founder      remainder swept into the
                                  fee collector, joining this
                                  block's distribution

   transfers ──────▶  untouched. 100 HASH sent is 100 HASH received.
```

The accounting identity `total_qualifying = founder + validator + treasury` is
re-asserted after every update, and the module refuses to persist books that
do not balance.
[`x/feerouter/types/genesis.go`](../x/feerouter/types/genesis.go)

## Network identity

Five parts, fixed at compile time except for the genesis hash, which is
computed at genesis and then pinned:

| Part | Mainnet value | Purpose |
| --- | --- | --- |
| Network name | `Hashgram Mainnet` | Human-readable |
| Network id | `hashgram-mainnet` | Application-level identity |
| Chain id | `hashgram-1` | CometBFT consensus identity |
| Genesis hash | computed at genesis | The actual network fingerprint |
| Network magic | 4 bytes | Cheap first-byte rejection at the P2P layer |

Plus a protocol major version, which gates wire compatibility.

### What each layer actually checks

This is worth stating precisely, because the layers are not interchangeable
and the four-validator test demonstrates the gap:

1. **CometBFT's handshake compares chain ids.** It does not hash the genesis
   file. A fork under its own chain id is refused before a connection opens. A
   fork that keeps `hashgram-1` and changes only the genesis **will** open a
   transport connection.
2. **Consensus rejects it anyway.** The fork's validators are not in the real
   validator set and its app hash diverges at height one, so it cannot inject
   a block or move the real chain. The two run as disjoint chains that happen
   to share a TCP connection.
3. **Operator tooling is what stops a person being fed a fork.**
   `hashgramctl join-mainnet` requires `--genesis-hash` and refuses any
   genesis that does not match it.
4. **The Phase 2 P2P handshake will verify all five parts**, closing the
   transport-layer gap that CometBFT leaves open.

`scripts/testnet/four-validator.sh` tests all of layers 1 to 3, including the
case where layer 1 does not hold, and says so in its output rather than
claiming a rejection that did not happen.

### Signature domain separation

Every signature in Hashgram is over a domain-separated digest:

```text
SHA-256( network_magic(4) || len(domain) || domain || len(payload) || payload )
```

where `domain` encodes the network and the purpose, one of: social event,
device certificate, service receipt, storage challenge, eligibility
attestation, content attestation, bootstrap record, peer handshake, node
announcement.

Two consequences. A signature produced for one purpose cannot be
reinterpreted as another, and a signature produced on devnet cannot be
replayed on mainnet. [`app/params/network.go`](../app/params/network.go)

### Canonical encoding

Protobuf is not a canonical encoding: field ordering, varint padding and
unknown fields all admit several encodings of the same message. A signature
over "whatever proto produced" is a signature over something the verifier may
not reproduce.

Every signing preimage is therefore hand-built with explicit length prefixes,
in one place: [`app/canonical`](../app/canonical). Four modules previously had
their own copy of that framing, which is four places for it to drift, and
framing drift in signing code is signature confusion.

The package has two builders. `Buf` uses 32-bit length prefixes for bounded
fields and returns an error if a field exceeds the bound, so a caller fails
closed rather than signing a wrapped length. `Fixed` uses 64-bit prefixes for
arbitrary-length payloads, where no length can overflow and so no error is
possible; the distinction is enforced by the type rather than by a comment.

## Useful-service rewards

```text
Provider registers ──▶ posts a bond ──▶ becomes eligible
                                             │
        ┌────────────────────────────────────┼─────────────────────┐
        │                                    │                     │
   relay / call                          storage              retrieval
   client-signed                     network-assigned         client-signed
   receipts                          bytes + challenges       receipts
        │                                    │                     │
        └────────────────────────────────────┼─────────────────────┘
                                             ▼
                                    credit for the epoch
                                             │
                         concentration discount: at most 20% of a
                         provider's credit may come from one counterparty
                                             │
                                             ▼
                              epoch settlement (~daily)
                                             │
              budget = min( 5 bps of the remaining reserve, 250,000 HASH )
                                             │
              per-provider cap: 5% of the budget, so twenty providers
              are needed before the whole budget is drawable
                                             │
                                             ▼
                              paid from the finite reserve
                       undrawn budget stays in the reserve for later epochs
```

Three properties are worth calling out because they are what make the scheme
resistant rather than merely plausible:

**Self-reported work earns nothing.** A relay receipt must be signed by the
client that was served, and the client's public key is inside the signed
bytes. A provider cannot be its own client: that is checked and rejected.

**A fake-traffic ring is unprofitable.** The concentration cap means at most
20% of a provider's credit may come from a single counterparty, so two nodes
signing each other's receipts collect at most a fifth of what they claim while
an honest relay serving many clients is unaffected. This is the single most
load-bearing anti-abuse parameter in the module, and `Validate` refuses to let
governance disable it.

**Declared capacity is advertising.** A provider that declares a petabyte and
is assigned nothing earns nothing. Payment follows assignment and challenge
success, not the operator's own claim.

## Node roles

One machine, one or more roles. `hashgramctl configure-role` sets them and
`hashgramctl start` starts only the services those roles need.

| Role | Service | What it does |
| --- | --- | --- |
| `validator` | `hashgramd` | Signs blocks. The role with slashing risk. |
| `relay` | `hashgram-node` | Forwards encrypted envelopes (Phase 2) |
| `store` | `hashgram-node` | Stores encrypted blobs (Phase 2) |
| `media` | `hashgram-node` | Serves media manifests and chunks (Phase 2) |
| `indexer` | `hashgram-indexer` | PostgreSQL index of public data (Phase 2) |
| `bootstrap` | `hashgram-node` | Helps new nodes discover peers (Phase 2) |
| `call` | `coturn` | TURN relay for calls (Phase 2) |
| `safety` | `hashgram-safety` | Scans **public** content only (Phase 2) |

Full requirements per role, and which roles should not share a machine, in
[NODE_ROLES.md](NODE_ROLES.md).

## Process and privilege separation

Each role runs as its own unprivileged user with its own data directory, so
that compromising one does not yield the others:

```text
hashgram-chain   /var/lib/hashgram/chain    consensus and state
hashgram-node    /var/lib/hashgram/node     P2P, storage, relay
hashgram-index   /var/lib/hashgram/index    PostgreSQL index
hashgram-safety  /var/lib/hashgram/safety   content scanning
hashgram-call    /var/lib/hashgram/call     TURN
```

The safety engine's unit makes the chain and node data directories explicitly
inaccessible via `InaccessiblePaths=`. It scans public content; it has no
business reading consensus state or the envelope store.

Every unit is hardened with `ProtectSystem=strict`, `NoNewPrivileges`, an
empty `CapabilityBoundingSet`, `SystemCallFilter`, `MemoryDenyWriteExecute`
and `UMask=0077`, among others. [`deploy/systemd/`](../deploy/systemd)

## Observability

CometBFT exposes consensus, mempool and P2P metrics. The SDK exposes
transaction counts and per-module block timings. Neither can know whether
Hashgram's economic invariants still hold, so [`app/metrics.go`](../app/metrics.go)
adds 22 gauges covering total supply against its ceiling, the Founder ledger,
the service reserve and the welcome pool.

Registered directly with the Prometheus default registry rather than through
the SDK's telemetry package, because that API takes a `float32` whose 24-bit
mantissa cannot represent a uhash amount above about 1.7e7 exactly. Supply
reaches 1e15, so total supply and its ceiling would both round to the same
`float32` and a breach would be invisible in the one metric that exists to
detect it.

Metric names in the dashboards were taken from a running node, not from
memory. That is how two real traps were found: `cometbft_p2p_peers` does not
exist until a node's first peer event, so an alert comparing it to zero cannot
fire on a never-peered node; and CometBFT exports no missed-blocks counter at
all, so validator signing has to be inferred from the gap between the chain
height and the validator's last signed height.

## Repository layout

```text
app/               application wiring, params, canonical encoding, metrics
x/                 the eight Hashgram modules
cmd/               hashgramd, hashgramctl, hashgram-test-client, hashgram-keygen
genesis/           deterministic Mainnet genesis builder
proto/             protobuf definitions and vendored third-party protos
deploy/systemd/    hardened unit files
deploy/monitoring/ Prometheus config, alert rules, Grafana dashboards
scripts/install/   host bootstrap and monitoring installers
scripts/testnet/   devnet and four-validator acceptance suites
scripts/dev/       CI pipeline, release build, policy checkers
tools/             tokenomics simulator
docs/              this documentation
node/              Rust workspace (Phase 2, empty)
sdk/               client SDKs (Phase 2, empty)
```

## What is not built

Stated plainly so that nothing above reads as more complete than it is:

- The Rust `hashgram-node`: libp2p transport, OpenMLS end-to-end encryption,
  the offline envelope store, signed social events, content-addressed blob
  storage, the PostgreSQL indexer, the safety engine, call discovery.
- Client applications for any platform.
- A token bridge. Interfaces are sketched; nothing is deployed.
- Mainnet itself, which requires a Founder address generated on a machine that
  is not this server.
