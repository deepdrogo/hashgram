# Client Connectivity Specification

The complete set of interfaces a client may rely on, and what it must not
assume exists.

This is the reference the three build prompts draw on. Endpoint paths were
extracted from `proto/hashgram/*/v1/query.proto`, transaction types from
`tx.proto`, and CLI commands from `--help` on built binaries.
`scripts/dev/check-docs.sh` verifies in CI that nothing here is invented.

---

## 1. Endpoints

| Service | Port | Bound to | Reach it via |
| --- | --- | --- | --- |
| CometBFT RPC | 26657 | localhost | SSH tunnel, or a public endpoint an operator chose to expose |
| Cosmos REST | 1317 | localhost | as above |
| Cosmos gRPC | 9091 | localhost | as above |
| Prometheus metrics | 26660 | localhost | SSH tunnel only |
| Consensus P2P | 26656 | all interfaces | node to node, not for clients |
| Hashgram P2P | 26670 | all interfaces | **clients connect here**: libp2p QUIC (`/udp/26670/quic-v1`) or TCP |
| hashgram-node local API | 26672 | localhost | same-host clients, `hashgramctl`, indexer, safety engine |
| hashgram-node metrics | 26671 | localhost | SSH tunnel only |
| Indexer read API | 1318 | localhost | same-host or behind an operator's reverse proxy |
| TURN | 3478, 5349 | all interfaces | call nodes only |

**None of the client-facing ports are publicly exposed on a correctly
configured node.** `hashgramctl mainnet-preflight` fails the launch if the
admin RPC is reachable from outside. A client therefore talks to either a
local node the user runs, or a public endpoint an operator deliberately
published.

Design for both. Ship a configurable endpoint list with per-endpoint health,
and never hard-code a single hostname: that makes one operator load-bearing
for every user, which is the opposite of the point.

---

## 2. CometBFT RPC

Standard CometBFT v0.38. The endpoints a client needs:

| Endpoint | Returns |
| --- | --- |
| `GET /status` | Height, chain id, catching-up, node info, validator info |
| `GET /net_info` | Connected peers |
| `GET /abci_info` | Application version and last block height |
| `GET /validators` | The current validator set with voting power |
| `GET /block?height=N` | A block |
| `GET /tx?hash=0x…` | A transaction result |
| `POST /broadcast_tx_sync` | Submit a signed transaction; returns on CheckTx |
| `POST /broadcast_tx_commit` | Submit and wait for a block. Avoid: it holds a connection open for the block time. |
| `GET /genesis_chunked?chunk=N` | The genesis document, in chunks |

### The genesis hash trap

**The SHA-256 of `/genesis_chunked` output never equals the SHA-256 of the
genesis file.** CometBFT parses the file and re-serialises it: it drops the
SDK's `app_name` and `app_version` fields, renders `initial_height` as a
string rather than a number, and emits compact rather than indented JSON.

Measured on a healthy devnet node:

```text
sha256(genesis.json on disk)     9348af00681eecefb8d6329d5ba101c13bc3c8943f2c610295026f6503654287
sha256(/genesis_chunked output)  efdc56f3bacfca368ef0be84e1b4c6da225c1bd2f042697e7f506b06d0acf5ce
```

So do not build a "verify the network's genesis" feature by hashing the RPC
response. It will always mismatch, and a diagnostic that always warns is one
users learn to ignore. `hashgramctl network-info` made exactly this mistake.

Verify the **chain id** from `/status`, which is the identity CometBFT itself
enforces at the peer handshake. To verify a genesis hash, hash a file the user
obtained independently.

---

## 3. Cosmos SDK REST

| Endpoint | Use |
| --- | --- |
| `/cosmos/bank/v1beta1/balances/{address}` | Balances |
| `/cosmos/bank/v1beta1/supply/by_denom?denom=uhash` | Total supply |
| `/cosmos/auth/v1beta1/accounts/{address}` | Account number, sequence, vesting schedule |
| `/cosmos/staking/v1beta1/delegations/{address}` | Delegations |
| `/cosmos/staking/v1beta1/validators` | Validators |
| `/cosmos/staking/v1beta1/params` | Unbonding period, max validators |
| `/cosmos/distribution/v1beta1/delegators/{address}/rewards` | Pending rewards |
| `/cosmos/slashing/v1beta1/params` | Slash fractions, jail duration |
| `/cosmos/gov/v1/proposals` | Governance proposals |
| `/cosmos/gov/v1/params/{params_type}` | Voting period, deposits, thresholds |
| `/cosmos/tx/v1beta1/txs?events=…` | Transaction history |
| `/cosmos/tx/v1beta1/simulate` | Gas estimation |

---

## 4. Hashgram REST

The complete list. 39 endpoints.

### Network

```text
GET /hashgram/network/v1/info
GET /hashgram/network/v1/fork_isolation
GET /hashgram/network/v1/signing_domain/{purpose}
```

`{purpose}` is one of: `social-event`, `device-cert`, `service-receipt`,
`storage-challenge`, `eligibility-attestation`, `content-attestation`,
`bootstrap-record`, `peer-handshake`, `node-announce`.

`signing_domain` is the endpoint a client uses to obtain the exact domain
string it must embed in a signature, rather than constructing it and getting
the format subtly wrong.

### Founder and fee routing

```text
GET /hashgram/founder/v1/params
GET /hashgram/founder/v1/revenue
GET /hashgram/founder/v1/beneficiary_history
GET /hashgram/feerouter/v1/params
GET /hashgram/feerouter/v1/totals
GET /hashgram/feerouter/v1/service_revenue
```

`founder/v1/params` reports the configured basis points and the compile-time
ceiling. `feerouter/v1/totals` reports what was actually collected and split,
so a client can compute the **realised** share:

```text
realised_bps = 10000 * founder_share / total_qualifying
```

Showing the realised figure next to the configured one is what demonstrates
the 1% is what the chain did, not merely what it is set to.

### Treasury

```text
GET /hashgram/treasury/v1/reserves
GET /hashgram/treasury/v1/reserve/{name}
GET /hashgram/treasury/v1/disbursements
```

`{name}`: `treasury`, `growth`, `dev_grants`, `liquidity`.

### Usernames

```text
GET /hashgram/username/v1/params
GET /hashgram/username/v1/lookup/{name}
GET /hashgram/username/v1/reverse/{owner}
GET /hashgram/username/v1/availability/{name}
GET /hashgram/username/v1/registrations
```

`availability` reports a confusable collision as unavailable **with the
reason**. A client resolving a name a user typed should surface that reason:
the chain refuses to register a visually confusable name, but a user can still
be handed a similar *already registered* one.

### Identity

```text
GET /hashgram/identity/v1/identity/{address}
GET /hashgram/identity/v1/devices/{address}
GET /hashgram/identity/v1/device/{address}/{device_id}
GET /hashgram/identity/v1/resolve_device_key
GET /hashgram/identity/v1/recovery/{root_address}
GET /hashgram/identity/v1/identities
```

**Only public keys are stored.** No endpoint returns anything private, because
nothing private is on chain. `resolve_device_key` maps a device public key back
to its owning identity, which is how a client verifies that a signature came
from an authorised device.

### Welcome rewards

```text
GET /hashgram/welcome/v1/params
GET /hashgram/welcome/v1/status
GET /hashgram/welcome/v1/tiers
GET /hashgram/welcome/v1/claim/{subject}
GET /hashgram/welcome/v1/claims
```

`status` reports the next sequence number, which determines the tier, and how
much of the capped pool remains.

### Useful-service rewards

```text
GET /hashgram/serviceproof/v1/params
GET /hashgram/serviceproof/v1/reserve
GET /hashgram/serviceproof/v1/provider/{operator}
GET /hashgram/serviceproof/v1/providers
GET /hashgram/serviceproof/v1/epoch/current
GET /hashgram/serviceproof/v1/epoch/{number}
GET /hashgram/serviceproof/v1/rewards/{operator}
GET /hashgram/serviceproof/v1/assignments/{provider}
GET /hashgram/serviceproof/v1/challenges/{provider}
GET /hashgram/serviceproof/v1/fraud/{provider}
GET /hashgram/serviceproof/v1/emission_schedule
```

`emission_schedule` projects the declining reward curve forward from the
current reserve, using the same function the state machine settles with.

---

## 5. gRPC

The same services over gRPC on 9091 (not 9090, which Prometheus uses). Generate a client from
`proto/hashgram/*/v1/*.proto`.

Prefer gRPC for anything polled: it is a persistent connection and typed at
compile time, so a renamed field is a build error rather than a null at
runtime.

---

## 6. Transactions

### Signing

| | |
| --- | --- |
| Sign mode | `SIGN_MODE_DIRECT` |
| Account curve | secp256k1 |
| BIP-44 coin type | 118 |
| Derivation path | `m/44'/118'/0'/0/0` |
| Address prefix | `hash` |
| Fee denomination | `uhash` |

Coin type 118 is Cosmos's, chosen so standard hardware wallets and keyring
tooling work unmodified.

### Hashgram transaction types

| Module | Message | Sender |
| --- | --- | --- |
| `founder` | `MsgClaimFounderRevenue` | anyone |
| `founder` | `MsgUpdateParams` | governance |
| `username` | `MsgRegister`, `MsgRenew`, `MsgTransfer`, `MsgSetTransferable`, `MsgRelease` | owner |
| `username` | `MsgUpdateParams` | governance |
| `identity` | `MsgCreateIdentity`, `MsgAddDevice`, `MsgRevokeDevice`, `MsgRotateRootKey`, `MsgSetRecoveryConfig`, `MsgRevokeIdentity` | identity owner |
| `identity` | `MsgInitiateRecovery`, `MsgApproveRecovery`, `MsgCancelRecovery`, `MsgExecuteRecovery` | guardian or owner |
| `welcome` | `MsgClaimWelcome` | subject, with an attestation |
| `welcome` | `MsgUpdateParams` | governance |
| `serviceproof` | `MsgRegisterProvider`, `MsgUpdateProvider`, `MsgSubmitReceipts`, `MsgAnswerChallenge`, `MsgUnjail`, `MsgBeginUnbonding`, `MsgWithdrawBond`, `MsgAssignStorage`, `MsgReleaseStorage` | provider operator |
| `serviceproof` | `MsgUpdateParams` | governance |
| `feerouter` | `MsgUpdateParams` | governance |
| `treasury` | `MsgSpend` | governance |

Plus the standard SDK messages for transfers, staking, governance and authz.

---

## 7. Signing Hashgram-specific objects

Some objects are signed outside a transaction: device certificates, service
receipts, storage challenge responses, eligibility attestations. These use
Hashgram's own canonical framing, and a client that gets it wrong produces a
signature nobody can verify.

### The digest

```text
digest = SHA-256(
    network_magic(4)
    || len(domain) as u64 big-endian
    || domain
    || len(payload) as u64 big-endian
    || payload
)
```

where `domain` is `hashgram/v{protocol}/{network_id}/{purpose}`, obtainable
from `/hashgram/network/v1/signing_domain/{purpose}`.

The length prefixes are mandatory. Without them, `(domain="ab",
payload="c")` and `(domain="a", payload="bc")` hash identically and a
signature for one verifies for the other.

### The payload

The inner payload has its own framing, with **32-bit** length prefixes and a
16 MiB per-field bound. Field order is fixed per object type and is documented
in `app/canonical/encode.go` and in each module's `types` package.

The two prefix widths differ deliberately. The outer wrapper frames an
arbitrary-length payload, where a 64-bit prefix cannot overflow and so needs
no bounds check. The inner fields are bounded by validation, where a 32-bit
prefix is compact and the bound check catches a genuine bug.

### Test against the vectors, not against this document

`node/testdata/signing-vectors.json` contains 90 preimages and 90 digests
across both networks and all nine purposes, generated from the Go
implementation.

```bash
go run ./tools/signing-vectors > node/testdata/signing-vectors.json
```

Check your implementation against those. A specification can be misread twice
in the same way; a generated vector cannot. The Rust implementation in
`node/hashgram-net` does exactly this, and the mechanism was verified by
breaking the framing on purpose to confirm the tests fail.

---

## 8. Data handling requirements

**Amounts are strings in JSON.** `uhash` reaches 10^15, beyond the
exact-integer range of an IEEE double. Parse into a big-integer or arbitrary
precision decimal type. Never `double`, `float`, or JavaScript `number`.

**Account sequence numbers must be refetched before every transaction.** Two
transactions signed with the same sequence means the second is rejected.

**Addresses use the `hash` Bech32 prefix.** Validate the prefix and the
checksum. A `cosmos1…` address is not a Hashgram address.

**Vesting accounts report a schedule.** Read it from
`/cosmos/auth/v1beta1/accounts/{address}` and show spendable separately from
locked, with the next unlock time. "Why can't I send my own coins" is the most
common question a vesting account generates.

**Transfers are not taxed.** Say so at the confirmation step. Users arriving
from chains with transfer taxes assume otherwise, and the fee is separate:
it goes to validators, with 1% of it to the Founder.

---

## 9. Metrics, for operator-facing clients

Prometheus on 26660, localhost only. Requires `prometheus = true` in the
node's `config.toml`.

Three families share the endpoint: `cometbft_*` from CometBFT, `hashgram_*`
from `app/metrics.go`, and `tx_*` plus `*_blocker` from the SDK.

### Two traps

**`cometbft_p2p_peers` does not exist until the node's first peer event.** It
is a lazily-created Prometheus child, so an absent series means "never
peered", which is a different state from zero peers. Render them differently
and do not alert on a comparison that cannot fire.

**CometBFT exports no missed-blocks counter.** Derive validator signing health
from `cometbft_consensus_latest_block_height` minus
`cometbft_consensus_validator_last_signed_height`.

### The one to watch

`hashgram_supply_over_ceiling_uhash` must be zero forever. It is computed with
exact integer arithmetic on the node before export, so a one-uhash breach is
visible despite the absolute totals being floats.

---

## 10. The peer-to-peer interfaces

Everything off-chain is reached over libp2p on port 26670 with the protocol
in `docs/PROTOCOL.md`. A client is a light swarm: it dials one or more nodes
(bootstrap multiaddrs with `/p2p/<peer-id>`), completes the Hashgram
handshake — which verifies the genesis hash, so a client pinned to Mainnet
cannot be fed a fork's data — learns the nodes' roles, and then:

| Need | Protocol | SDK |
| --- | --- | --- |
| Publish key packages, deliver and fetch messages | `/hashgram/rpc/1` mailbox bodies; MLS inside | `hashgram_sdk::messaging` |
| Publish and fetch social events | `EventPublish`, `EventFetch`; gossip on shards | `hashgram_sdk::social` |
| Upload and download media, private encryption | blob bodies; DHT providers | `hashgram_sdk::blob` |
| Find call nodes, get TURN credentials, signal | `AnnounceQuery`, `TurnCredentialRequest`; MLS `CallSignal` | `hashgram_sdk::calls` |
| Pay providers for service | `ReceiptDeliver` | automatic in the SDK |
| Read the chain and broadcast transactions with no REST endpoint | `ChainQuery`, `ChainBroadcast` (served by `relay`/`bootstrap` nodes; allow-listed read paths only) | `hashgram_sdk::chain_relay`, `hashgram_sdk::chain_client_over_link` |

### Chain access without a server

`ChainClient` (`hashgram_chain::Client`) works over a pluggable transport:
HTTP to a REST gateway (§3–§4 above), or the P2P chain relay. Over the
relay every read is issued to **two nodes run by different operators** and
compared byte for byte after JSON normalisation, with heights within 3
blocks; a mismatch marks both nodes disputed and asks a third. The result
carries a `Verification` (which peers answered, whether they agreed,
whether only one operator was reachable) for the UI to show as "verified by
2 nodes" or as a warning. `simulate` is not available over the relay, so the
client estimates gas instead of asking. `hashgram-client` uses the relay
whenever `chain_api` is empty, which is the default:

```text
hashgram-client configure --network mainnet --genesis-hash <hash>   # no --chain-api
hashgram-client wallet balance                                    # "verified by 2 nodes (2 operators)"
```

The precedence an application should use, each with a live health
indicator: a node on the same machine (`127.0.0.1`), then the P2P relay
across ≥ 2 nodes, then HTTPS endpoints the user pasted. Never a single
hardcoded hostname as the only way in.

`hashgram-sdk` (`sdk/rust/hashgram-sdk`) implements all of it and
`hashgram-client` (`node/hashgram-client`) is the reference command line on
top: `identity`, `wallet`, `message`, `group`, `post`, `reel`, `blob`,
`call`, `net`. A native application either binds the SDK (recommended) or
reimplements against the protobuf definitions and the signing vectors.

### Keystore

The SDK stores keys in a vault file encrypted with XChaCha20-Poly1305 under
an Argon2id key derived from a passphrase (`hashgram-identity`). It holds
the account secret (optional), the identity root seed (optional), this
device's seed, the MLS state snapshot, the social sequence chain, mailbox
cursors and seen envelope ids. Nothing in it is ever written in the clear.
A phone that holds only its device key can message and post; the device
holding the root key adds and revokes devices.

### Same-host and indexer APIs

On a machine running the node, `127.0.0.1:26672` offers the same services
over JSON (`/v1/status`, `/v1/peers`, `/v1/social/events`,
`/v1/social/author/{address}`, `/v1/blobs`, `/v1/blobs/{cid}`,
`/v1/blobs/{cid}/health`, `/v1/announcements`, `/v1/rewards`,
`/v1/safety/attestations/{subject}`, `/v1/calls/turn-credentials`, and
`/v1/chain/{path}` forwarding read queries to the chain). The indexer's read
API on `127.0.0.1:1318` serves feeds (`docs/SOCIAL_PROTOCOL.md`,
"Projections"). Both are loopback by default; exposing them is an operator's
reverse-proxy decision, and a client must treat an indexer as a cache it
can cross-check against events it verifies itself.

### What still does not exist

- Native applications for iOS and Android (the prompts describe them); the
  Windows application is being built in `apps/desktop/`.
- A Merkle-proof light client. The P2P chain relay cross-checks two
  operators' answers, which removes the single server but does not prove an
  answer against a block header. Proofs are a later milestone; until then
  "verified by 2 nodes" means exactly that and no more.
- Push notifications: clients poll or stay connected.
- Call receipts from the reference client; a WebRTC media stack in the SDK.
- End-to-end encryption of SFU-hosted group calls against the SFU operator.
- A token bridge.

---

## 11. Reference implementations

`cmd/hashgram-test-client` (Go) exercises wallet, staking, Founder
verification and signing domains against a running chain.
`node/hashgram-client` (Rust, on `hashgram-sdk`) exercises everything else.

```bash
make build && make rust-release
scripts/testnet/devnet.sh                     # a real chain to talk to

build/hashgram-test-client wallet balance <address>
build/hashgram-test-client founder verify

export HASHGRAM_PASSPHRASE=...                # never a flag
hashgram-client configure --network devnet --genesis-hash <sha256> \
  --chain-api http://127.0.0.1:1317 --bootstrap /ip4/127.0.0.1/udp/26670/quic-v1/p2p/<peer-id>
hashgram-client identity create               # wallet + root + device; prints the recovery phrase once
hashgram-client identity publish-keys
hashgram-client message send hash1... "hello"
hashgram-client message receive
hashgram-client post "first post" --tag hashgram
hashgram-client reel publish video.mp4 --caption "..."
hashgram-client blob upload photo.jpg --mime image/jpeg --private
hashgram-client call discover
```

Copy their behaviour rather than this document's prose where the two could
disagree. `scripts/testnet/phase2.sh` runs the full client flow against a
live network and is the executable form of this specification.
