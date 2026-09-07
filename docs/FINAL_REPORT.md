# Hashgram Build Report

What was built, what was verified, the operator procedures you asked for, and
what is not done.

Read [PHASE1_REPORT.md](PHASE1_REPORT.md) alongside this. That document holds
the verbatim output of every test run; this one holds the architecture, the
procedures, and an honest accounting of the remainder.

---

## 1. What exists

### Binaries

| Binary | Language | Purpose |
| --- | --- | --- |
| `hashgramd` | Go | The chain: Cosmos SDK v0.53.8 on CometBFT v0.38.26 |
| `hashgramctl` | Go | Operator CLI: init, genesis, join, roles, status, storage, rewards, health, backup, preflight |
| `hashgram-keygen` | Go | Offline key generation. No networking, no disk writes. |
| `hashgram-test-client` | Go | Developer client for the chain: wallet, staking, Founder verification, signing domains |
| `hashgram-indexer` | Go | PostgreSQL projection of chain and public social data; read API on `127.0.0.1:1318` |
| `hashgram-safety` | Go | Content review pipeline that signs `ContentAttestation`s |
| `hashgram-node` | Rust | The P2P node: swarm, mailboxes, social events, blob storage, rewards agent, local API |
| `hashgram-client` | Rust | Developer client for the network: identity, messaging, groups, social, reels, blobs, calls |

The Go binaries are static, CGO-free and reproducible, built with `-trimpath
-buildvcs=false` and no build-date or commit stamp. The Rust binaries are
built with a pinned toolchain and `Cargo.lock`; `scripts/dev/release.sh`
records both sets in `SHA256SUMS`.

### Code

| | |
| --- | --- |
| Authored Go | 37,723 lines, 179 files |
| Go tests | 8,214 lines, 351 test and fuzz functions |
| Rust | 21,766 lines, 67 files, 9 crates + 8 fuzz targets |
| Generated protobuf (Go) | 67,107 lines |
| Proto definitions | 5,001 lines, 50 files |
| Shell (install, testnet, CI) | 5,055 lines, 13 files |
| Documentation | 24 documents |

### The chain

Eight custom modules on the standard Cosmos SDK set.

| Module | What it enforces |
| --- | --- |
| `x/network` | Five-part network identity, genesis hash pinning, nine signing domains |
| `x/founder` | The revenue ledger and a compile-time fee ceiling governance cannot raise |
| `x/feerouter` | Splits fee revenue; has no access to transferred principal |
| `x/welcome` | Tiered joining reward, capped, requiring a signed attestation |
| `x/serviceproof` | Proof of Useful Service: client-signed receipts, storage challenges, bonds, fraud scoring |
| `x/username` | `@name` registry with confusable-character defence |
| `x/identity` | Root identities and device certificates. Public keys only. |
| `x/treasury` | Four named genesis allocations, spendable only by governance |

`x/mint` and `x/circuit` are **absent**, not configured. That is what makes
"no inflation" and "no kill switch" structural rather than a setting.

### The network

The off-chain layer, in Rust, in the `node/` workspace:

| Crate | What it does |
| --- | --- |
| `hashgram-net` | Network identity, signing domains, canonical preimages **byte-identical to Go**, verified against generated vectors; the five-part handshake |
| `hashgram-proto` | `prost` types for `proto/hashgram/p2p/v1` and `chat/v1` |
| `hashgram-p2p` | libp2p swarm: QUIC and TCP, Kademlia, Gossipsub, identify, AutoNAT, circuit relay, DCUtR; Hashgram handshake on every connection; peer scoring, per-peer rate limits, persistent peerstore |
| `hashgram-identity` | Encrypted vault (Argon2id + XChaCha20-Poly1305), device keys, device certificates matching `x/identity` |
| `hashgram-mls` | OpenMLS 0.7 wrapper: key packages, groups, Welcome, application messages, state snapshots |
| `hashgram-chain` | `cosmrs` client: queries, signing, broadcast of Hashgram transactions |
| `hashgram-node` | The daemon: mailbox store, social event log, blob store with replication and repair, safety enforcement, rewards agent, TURN credential issuance, local JSON API, Prometheus metrics |
| `hashgram-sdk` (`sdk/rust`) | Client facade: link, identity, messaging, social, blob, calls, receipts |
| `hashgram-client` | Reference client on the SDK |

And in Go, `indexer/` and `safety/` with their `cmd/` entry points. The
wire protocol is `docs/PROTOCOL.md`; the subsystems each have their own
document (`MESSAGING`, `SOCIAL_PROTOCOL`, `STORAGE`, `CALLS`, `MODERATION`,
`SERVICE_REWARDS`).

---

## 2. Verifying the Founder claims

The two claims most worth checking independently, with the exact commands.

### 200,000,000 HASH, with 180,000,000 vesting

```bash
hashgramctl wallet-info <founder-address>
```

Expect, for a genesis that funded no launch accounts:

```text
Balance           200000000000000 uhash  (200000000 HASH)
Spendable now     20000000000000 uhash  (20000000 HASH)
Locked (vesting)  180000000000000 uhash  (180000000 HASH)
```

Launch validators are funded at genesis out of the Founder's unlocked
portion (`--genesis-account`), so a real launch shows the balance and the
spendable figure reduced by exactly that sum, and the locked figure unchanged.
The distribution table `init-mainnet-genesis` printed lists every launch
account by address; the sum of the Founder balance and the launch accounts is
200,000,000.

Or without trusting `hashgramctl`:

```bash
hashgramd query bank balances <founder-address>
hashgramd query auth account <founder-address>
```

The second shows the `PeriodicVestingAccount` with its 96 monthly periods. The
lock is enforced by the state machine, not by a promise.

**If "Spendable now" reads 200,000,000, the vesting account was not created.**
Stop and investigate before anyone transacts.

### 1% of fee revenue, and nothing from transfers

```bash
hashgram-test-client founder verify --node tcp://127.0.0.1:26657
```

Reports the configured basis points (must be 100), the compile-time ceiling
(also 100), the accrued and paid totals, and the beneficiary history.

Then check the **realised** share, which is what the chain did rather than
what it is set to:

```bash
curl -s localhost:1317/hashgram/feerouter/v1/totals
```

```text
realised_bps = 10000 * founder_share / total_qualifying
```

Should be 100 or one below, because integer truncation always rounds in the
network's favour.

### Prove a transfer is untaxed

```bash
hashgram-test-client wallet balance <bob>
hashgram-test-client wallet send --from alice --to bob --amount 100
hashgram-test-client wallet balance <bob>
```

Bob's balance must rise by exactly 100 HASH. The devnet acceptance suite
asserts this automatically, and on the recorded run it reported:

```text
[ ok ] Alice sent 100 HASH and Bob received exactly 100 HASH (no transfer tax)
[ ok ] Founder accrued 2 uhash from transaction gas fees
```

Two uhash, not two HASH: 1% of the ~200 uhash of gas those transactions
actually consumed. A real measurement of a real fee, alongside a transfer that
delivered in full.

### Prove no coin can be created

```bash
hashgramd query bank total --denom uhash      # exactly 1000000000000000
go test ./app/ -run TestNoModuleCanMint -v
go test ./app/ -run TestMintModuleIsNotWired -v
```

And at runtime, `hashgram_supply_over_ceiling_uhash` must be zero forever. It
is computed with exact integer arithmetic before export, so a one-uhash breach
is visible.

---

## 3. Genesis procedure

Summarised. Full detail with checklists in
[FOUNDER_LAUNCH_RUNBOOK.md](FOUNDER_LAUNCH_RUNBOOK.md).

```text
A  Generate the Founder cold wallet on an OFFLINE machine
   ./hashgram-keygen new
   Write the mnemonic on paper, twice, two locations.
   Verify by comparing the DERIVED ADDRESS, not by the mnemonic parsing.
   Only the public hash1... address leaves that machine.

B  Prepare each host
   sudo ./scripts/install/bootstrap-ubuntu.sh
   hashgramctl init --moniker <name>
   sudo ./scripts/install/monitoring.sh

C  Preliminary genesis, with the launch validators' hot operator addresses
   hashgramctl init-mainnet-genesis --founder-address hash1... \
     --genesis-account hash1<operator>=1000000HASH
   (funded from the Founder's unlocked 20M; total stays 1,000,000,000)

D  Each validator: hashgramd genesis gentx operator <amount>uhash --chain-id hashgram-1 ...
   Genesis machine: copy gentxs to config/gentx/, then
   hashgramctl finalize-genesis          # pins the FINAL genesis hash
   Record the hash. Have a second person rebuild it from the same inputs.

E  Publish the genesis file, and its hash through channels INDEPENDENT of
   the file. A file next to its own hash proves nothing.

F  Operators join
   hashgramctl join-mainnet --genesis-url <url> \
     --genesis-hash <from an independent source> --peers <id>@<host>:26656
   hashgramctl network-info             # confirm the pin

G  hashgramctl mainnet-preflight && hashgramctl start
   Verify: supply, Founder allocation, vesting, fee share, no mint module

H  Governance registers the first storage assigner (hashgramctl propose add-assigner)
I  hashgramctl configure-role relay,store,media,bootstrap on the serving hosts
```

Step A cannot be done by this software, and step E is the one most often done
wrong.

**On BIP-39 checksums.** The checksum catches most single-word errors and
typos. It does **not** reliably catch two words being swapped, because a swap
can produce a valid checksum. An earlier version of `hashgram-keygen`'s help
text overstated this, and a swapped-word test produced a silently valid, wrong
address. So the verification that matters is the derived address matching, not
the mnemonic being accepted.

---

## 4. If this VPS is compromised

You asked for this scenario specifically. Here is what an attacker with root
on this server gets, what they do not, and what to do.

### What they get

- The chain database, which is public data replicated to every node
- The node's P2P identity key
- `priv_validator_key.json`, **if** a validator runs here and the key is not
  behind a remote signer
- The ability to serve wrong answers to anything querying this node's RPC
- Whatever is in the operator's shell history and SSH keys

### What they do not get

- **The Founder private key.** It was generated on an offline machine and this
  server has never held it. The genesis tooling has no code path that could
  generate one, so this is structural rather than a claim.
- **Any user's private key.** `x/identity` stores public keys only. There is
  no key escrow.
- **Any message plaintext.** Messages are MLS-encrypted on the sender's
  device; a store node holds ciphertext, and the acceptance test greps every
  store node's database for the test message and finds nothing.
- **Any private media.** Private blobs are encrypted before upload; the
  node stores ciphertext and never receives the key.
- **The ability to forge blocks the rest of the network accepts.** A stolen
  consensus key can double-sign, which gets you slashed; it cannot make other
  validators accept invalid blocks.
- **The ability to mint, freeze, or reverse anything.** No such transaction
  exists.

### What to do, in order

```bash
# 1. Stop signing immediately. Every additional block deepens a slash.
hashgramctl stop

# 2. Assume the consensus key is stolen. Do NOT restore it elsewhere:
#    two signers with one key is a 5% slash with no recovery.

# 3. Rebuild the host from scratch. Do not clean it in place; you cannot
#    prove you found everything.

# 4. Generate a NEW consensus key on the new host. Never reuse the old one.

# 5. Rotate SSH keys and anything that machine could reach.

# 6. Check whether the operator key was ever on that host. If it was, move
#    the stake using a key that was not.

# 7. Confirm the new host is on the right network:
hashgramctl network-info      # compare against your recorded genesis hash
```

### What the network loses

Nothing, if the validator set is distributed. Consensus continues on two
thirds of voting power, users hold their own keys, and a seized node yields no
user private keys and no message plaintext.

**Today the validator set is not distributed**, which is the honest limitation
recorded in [DECENTRALIZATION.md](DECENTRALIZATION.md) §3. A single-VPS
network losing that VPS loses the network. That is a deployment problem, not a
code problem, and it is fixed by other people running validators.

---

## 5. Adding a second server

You asked exactly what may be copied and what never may. Here it is.

### Never copy

| File | Why |
| --- | --- |
| `priv_validator_key.json` | Two processes with one consensus key is a 5% slash with no recovery. `priv_validator_state.json` prevents a single node from double signing and **cannot** protect against two nodes, because neither sees the other's file. |
| `node_key.json` | Two nodes claiming one P2P identity confuses peer tracking and address books. Generate a new one; it costs reputation and nothing else. |
| The keyring | Account keys belong on one machine. |

`hashgramctl backup` excludes all three deliberately. A restore that recreated
a consensus key on a second host would be a double-signing accident waiting
for someone to press a button.

### Safe to copy

| File | Notes |
| --- | --- |
| `genesis.json` | Public data. **Verify its hash on arrival** rather than trusting the copy. |
| `/etc/hashgram/network.json` | The pinned identity. Copying it is how the second node inherits the correct pin. |
| `config.toml`, `app.toml` | Copy, then change the moniker, and remove any validator-specific settings. |

### The procedure

```bash
# On the new server:
sudo ./scripts/install/bootstrap-ubuntu.sh

# Join using the hash from an INDEPENDENT source, not from the file you
# just copied. The whole point of the pin is not having to trust the file.
hashgramctl join-mainnet \
  --genesis-url https://... \
  --genesis-hash <sha256 from an independent source> \
  --peers <nodeid>@<first-server>:26656

# Roles this machine will serve. Note the guidance in NODE_ROLES.md about
# which combinations to avoid.
hashgramctl configure-role relay,store

# Confirm it landed on the right network before starting.
hashgramctl network-info

sudo ./scripts/install/monitoring.sh
hashgramctl mainnet-preflight
hashgramctl start
hashgramctl status
```

### Getting the first server's node id

```bash
# On the first server:
hashgramctl node-info          # includes the node id
```

### If the second server is also a validator

**Generate a new consensus key.** Do not copy the first one.

Two validators mean two entries in the validator set, two separate gentxs (if
before genesis) or two separate `MsgCreateValidator` transactions (if after),
and two separate stakes. They are two validators, not one validator on two
machines — that second thing is the double-signing mistake.

If what you want is redundancy rather than a second validator, the answer is a
**sentry architecture**: one validator with no public P2P listener, peering
only with sentry nodes you control. The sentries are full nodes with no
consensus key, so losing one costs nothing.

```text
   internet ──▶ sentry 1 ──┐
   internet ──▶ sentry 2 ──┼──▶ validator (no public listener, one key)
   internet ──▶ sentry 3 ──┘
```

---

## 6. Role separation examples

Each role runs as its own unprivileged user with its own data directory, so
compromising one does not yield the others.

### One machine, full node only

The default after `hashgramctl init`. Follows the chain, serves local queries,
relays transactions. No roles configured, no slashing risk.

```bash
hashgramctl init --moniker my-node
hashgramctl join-mainnet --genesis-url ... --genesis-hash ... --peers ...
hashgramctl start
```

### One machine, validator

```bash
hashgramctl configure-role validator
hashgramctl restart
hashgramctl validator          # signing status and safety checks
```

Runs as `hashgram-chain` in `/var/lib/hashgram/chain` at mode 0750. Do **not**
add other roles: a validator that misses precommits because a neighbour
process is busy gets jailed for something that is not a Hashgram problem.

### One machine, storage and relay

```bash
hashgramctl configure-role relay,store,media   # creates the operator key, prints the address to fund
hashgramctl restart
hashgramctl storage           # assignments, challenges, disk, blob health
hashgramctl rewards           # credit, receipts, payouts from the node and the chain
```

Runs as `hashgram-node` in `/var/lib/hashgram/node`. Combines naturally:
bandwidth and disk on one box. The operator key is a hot key that pays
fees for registration, challenge answers and receipt submission; the reward
address it names can be a cold wallet.

### Three machines, separated by risk

```text
Machine 1   validator only
            hashgram-chain, /var/lib/hashgram/chain
            No public P2P listener. Remote signer.

Machine 2   relay, store, media, bootstrap
            hashgram-node, /var/lib/hashgram/node
            Public P2P. Bandwidth and disk.

Machine 3   indexer, safety
            hashgram-index and hashgram-safety, separate directories
            The safety unit lists the chain and node data directories under
            InaccessiblePaths=, so a content scanner cannot read consensus
            state or the envelope store even if its code tried.
```

### Combinations to avoid

| Combination | Why |
| --- | --- |
| `validator` + `safety` | The safety engine has the largest attack surface of any role: it processes untrusted content and talks to third-party services. Not next to a consensus signing key. |
| `validator` + `call` | The call role needs 49152–65535/UDP open. Not next to a signing key. |
| `validator` + anything CPU-heavy | Missed precommits lead to jailing. |

### What the firewall opens per role

| Role | Ports |
| --- | --- |
| Any chain node | 26656/tcp |
| relay, store, bootstrap | 26670/tcp and 26670/udp |
| call | 3478, 5349 tcp+udp, 49152–65535/udp |
| indexer | none; PostgreSQL is localhost only |
| safety | none inbound |

**Never opened:** 26657, 1317, 1318, 9091, 26660, 26671, 26672, 5432, and
the monitoring ports. `mainnet-preflight` fails if the admin RPC or the
node's local API is publicly reachable.

---

## 7. Test results

Full verbatim output in [PHASE1_REPORT.md](PHASE1_REPORT.md). Summary:

| Suite | Result |
| --- | --- |
| `go test -race ./...` | All packages passing, 351 test and fuzz functions |
| `cargo test --workspace` + SDK | 188 tests passing |
| `scripts/testnet/devnet.sh` | 32 acceptance checks against a live chain, all passing |
| `scripts/testnet/four-validator.sh` | Consensus, resilience and fork isolation, all passing |
| `scripts/testnet/phase2.sh` | 58 network acceptance checks, all passing (verbatim below) |
| `gofmt`, `go vet`, `staticcheck` | Clean |
| `gosec` | Clean |
| `gitleaks` | No secrets, in the repository and in a node's data directory |
| `govulncheck` | Zero unreviewed vulnerabilities in the shipped binaries |
| `cargo clippy` | Clean with the workspace deny list |
| `cargo audit` | Clean; four advisories ignored with written justification in `node/.cargo/audit.toml` |
| Fuzzing | 8 `cargo-fuzz` targets (nightly, `make fuzz`) plus stable decoder smoke tests in every `cargo test`; 3 Go native fuzzers run in CI for a short budget |
| `scripts/dev/release.sh --verify` | Byte-identical across builds |

### The network acceptance run

Four validators, three P2P nodes (A: relay+store+bootstrap, B: store+media,
C: relay+store), one indexer, one safety engine, two clients, all on this
host. Output of `scripts/testnet/phase2.sh`, colour codes removed:

```text
[ ok ] four validators running
[ ok ] genesis hash 18fe15f0859b85936f39856c8dcf4604840ac9adcdd0ad88980d41343934da6c
[ ok ] node0 verified 2 peer(s)
[ ok ] node1 verified 2 peer(s)
[ ok ] node2 verified 2 peer(s)
[ ok ] node A refused the fork at the handshake: genesis hash mismatch
[ ok ] fork peer is banned on node A
[ ok ] the fork verified nobody
[ ok ] alice identity registered
[ ok ] bob identity registered
[ ok ] alice has 1 device(s) on chain
[ ok ] 3 providers registered
[ ok ] alice key packages published
[ ok ] bob key packages published
[ ok ] alice sent
[ ok ] bob decrypted alice's message
[ ok ] alice decrypted bob's reply
[ ok ] one direct conversation reused for both directions
[ ok ] no plaintext in any store node's mailbox database
[ ok ] group created
[ ok ] bob decrypted the group message
[ ok ] profile published
[ ok ] post published
[ ok ] bob follows alice
[ ok ] reel published, video e4fdbd4d...
[ ok ] node0 holds 3 of alice's events
[ ok ] node1 holds 3 of alice's events
[ ok ] node2 holds 3 of alice's events
[ ok ] reel video is HEALTHY (3 known replicas)
[ ok ] private blob uploaded 55e36e0e...
[ ok ] bob downloaded and decrypted the private blob byte-for-byte
[ ok ] stored blob is ciphertext
[ ok ] safety attestor 65fc7354...
[ ok ] indexer serves alice's post
[ ok ] indexer serves the reel
[ ok ] bob's following feed shows alice
[ ok ] indexer lists 3 providers from chain
[ ok ] safety engine blocked the scam post
[ ok ] node B no longer serves the blocked post
[ ok ] indexer hides the blocked post
[ ok ] node A recorded 2 storage assignment(s) on chain
[ ok ] chain issued 2 storage challenge(s) to node A
[ ok ] node A answered a challenge with a Merkle proof
[ ok ] chain records challenges_passed=1 for node A
[ ok ] client-signed receipts produced 880588 units of relay/retrieval credit across the providers
[warn] node A not yet paid (settlement pays at epoch close; storage needs a full epoch held)
[ ok ] alice sent with B down
[ ok ] bob received with B down
[ ok ] private blob still downloadable with B down
[ ok ] chain advanced from 131 to 134 with the genesis validator and node A gone
[ ok ] bob sent through node C only
[ ok ] alice received through node C only
[ ok ] no validator key, client vault or mnemonic material in any P2P node directory
[ ok ] gitleaks finds nothing in node C's data directory
```

The one warning is expected: storage credit is paid at epoch close for
bytes held a full epoch, and the test does not wait an epoch. The
`challenges_passed=1` line and the receipt credit show the inputs to that
payment are recorded.

The "genesis validator and node A gone" check is the genesis-VPS
destruction scenario: the first validator and the bootstrap node are
killed together, the chain keeps producing blocks on the remaining three,
and the two clients keep messaging through node C alone.

### The finding worth knowing

The fork-isolation test was written expecting any fork to be refused at the
CometBFT handshake, and it **failed**. Investigating showed the test's
expectation was wrong about which layer rejects, and the gap is real:

**CometBFT's peer handshake compares chain ids. It does not hash the genesis
file.** A fork keeping the real chain id and changing only the genesis opens a
transport connection. Consensus refuses its blocks — the run confirms the real
chain kept advancing, all four validators kept one app hash, and the validator
set stayed at exactly four — but the socket opens.

Three layers turn a fork away today: the CometBFT handshake for a different
chain id, consensus for a same-chain-id fork, and `hashgramctl join-mainnet`
for an operator handed the wrong file. On the Hashgram P2P layer the gap
does not exist: the Rust handshake runs before any application protocol,
verifies all five identity parts including the genesis hash, and the
acceptance run shows a same-chain-id fork refused, banned, and left with
no verified peers.

---

## 8. What the scanners surfaced, and what was done

The scanners were not run once and declared green. They surfaced work that was
actually done rather than suppressed.

**Four copies of the signing preimage framing.** `x/welcome`,
`x/serviceproof` and `x/identity` each had their own length-prefixed encoder.
Four copies is four places for framing to drift, and framing drift in signing
code is signature confusion. Consolidated into `app/canonical`, which enforces
the bound all four assumed, with ten tests.

**Thirteen symbol-level dependency vulnerabilities in the shipped binary.** Ten
dependencies upgraded, closing all thirteen.

**One advisory verified as a false positive.** GO-2024-2584 matched. The
upstream advisory says the slashing evasion was patched in SDK 0.50.5; this
tree is on v0.53.8. The Go vulnerability database entry carries a second
affected range with no fixed event, so every release in that line matches
regardless. Allowlisted with the evidence.

**A Makefile claim that was wrong.** `CGO_ENABLED=1` with a comment saying CGO
was required. Tested rather than trusted: a CGO-free build ran as a devnet and
produced blocks. Binaries are now static and 27% smaller.

**A diagnostic that warned on every healthy node.** `hashgramctl network-info`
compared the genesis file's hash against the hash of the genesis CometBFT
serves. They can never match. It now compares chain ids.

**A pruning bug in the Rust scoreboard.** It reused the decay timestamp as a
proxy for last-seen, and the decay pass advances it for every peer, so pruning
was a no-op and the scoreboard grew without bound. A test caught it.

**A duplicate Makefile target.** Two `release` targets; the second pointed at a
script that was never written. Make silently uses the last one, so
`make release` ran a missing script while the documentation described the
working one.

---

## 9. What is not done

The specification was 116 sections. Phases 1 and 2 are built and tested.

### Built and tested

- The entire chain: eight modules, genesis tooling, operator CLI
- The P2P node: swarm, handshake, discovery, mailboxes, social events, blob
  storage with replication and repair, rewards agent, TURN credentials
- The Rust SDK and reference client; the Go indexer and safety engine
- Monitoring: custom metrics, dashboards, alert rules, for chain and node
- CI: tests, static analysers, secret scanner, vulnerability scanners for
  Go and Rust, fuzz smoke runs
- Reproducible release build of all eight binaries
- 24 documents, verified against the code by a CI check
- A 58-check network acceptance suite including fork rejection, failover
  and genesis-host destruction

### Not built

| | |
| --- | --- |
| Applications | Windows, iOS, Android. Specifications only (`docs/PROMPT_*.md`); they bind `hashgram-sdk`. |
| SDK bindings | Only Rust. UniFFI/C-ABI bindings for Swift, Kotlin and C# are the apps' first task. |
| WebRTC media in the SDK | The SDK issues TURN credentials and carries signalling; a media stack is the application's. |
| SFU end-to-end encryption | LiveKit group calls are encrypted to the SFU, not through it. 1:1 calls are peer-to-peer. |
| Call receipts | Defined on chain and in the protocol; the reference client does not yet produce them. |
| Push notifications | None. Clients poll mailboxes or stay connected. |
| Safety OCR and frame extraction | Hook points exist; only the hash, text and HTTP-model stages are implemented. |
| Multi-assigner storage | Storage assignments come from the assigner set named at genesis; adding assigners is a governance action. |
| Mainnet | Requires a Founder address generated off this server. |

### Known limitations

Beyond what is unbuilt, the limitations that determine how much to rely on
what exists:

**Everything runs on one server.** The four-validator test runs four
validators on one host as one operating-system user. It tests consensus and
gossip and nothing about partitions, independent operators or geographic
distribution. Four validators on one VPS is still one VPS.

**The Founder holds 20% of a stake-weighted governance.** The code limits what
that reaches — no minting, no fee increase, no freezing, no bypassing vesting
— and cannot limit voting weight.

**The welcome attestor is trusted.** The chain verifies the signature; it
cannot verify the attestor is honest. The per-epoch cap bounds the damage and
on-chain visibility makes abuse detectable after the fact.

**Storage challenges prove retrievability, not unique replication.** A
challenge proves a provider can produce a chunk, not that it stores it
independently rather than fetching it on demand.

**Store-and-forward metadata is visible to store nodes.** Content is
ciphertext, but a store node knows which mailbox received an envelope of
what size, when. `docs/MESSAGING.md` states exactly what a store learns.

**The acceptance run is one host.** Three P2P nodes and four validators on
one machine test protocol behaviour, failover logic and fork rejection. They
do not test NAT traversal across real networks, geographic latency or
independent operators. AutoNAT, relay and DCUtR are wired and unit-tested;
their behaviour across real NATs has not been observed.

**No formal verification and no independent audit.** 351 Go and
188 Rust tests, eight fuzz targets and a live acceptance suite
are good and are not proofs. One implementation, no second to disagree
with it. OpenMLS and libp2p carry their own audits; the code that joins
them does not.

---

## 10. What would most improve this

In order of how much each would change the picture:

1. **An independent security audit**, published, with findings addressed.
2. **Twenty or more validators, independently operated, in five or more
   jurisdictions**, with no operator above 10% of voting power.
3. **A second implementation of the protocol**, so a consensus bug in one is
   caught by the other rather than becoming network-wide.
4. **Cross-machine reproducibility** confirmed by a second builder.
5. **Native applications**, so the network has users who are not running
   a command line.

The first three cannot be produced by writing more code, which is the honest
reason to list them first.

---

## 11. Where to start reading

| If you want to | Read |
| --- | --- |
| Understand the design | [ARCHITECTURE.md](ARCHITECTURE.md) |
| Check the economics | [TOKENOMICS.md](TOKENOMICS.md) |
| Launch mainnet | [FOUNDER_LAUNCH_RUNBOOK.md](FOUNDER_LAUNCH_RUNBOOK.md) |
| Run a node | [OPERATIONS.md](OPERATIONS.md), [NODE_ROLES.md](NODE_ROLES.md) |
| Know what an attacker can do | [THREAT_MODEL.md](THREAT_MODEL.md) |
| Know where control actually sits | [DECENTRALIZATION.md](DECENTRALIZATION.md) |
| Build a client | [CLIENT_CONNECTIVITY_SPEC.md](CLIENT_CONNECTIVITY_SPEC.md), [PROMPT_WINDOWS_DESKTOP.md](PROMPT_WINDOWS_DESKTOP.md) |
| Understand the wire protocol | [PROTOCOL.md](PROTOCOL.md), then [MESSAGING.md](MESSAGING.md), [SOCIAL_PROTOCOL.md](SOCIAL_PROTOCOL.md), [STORAGE.md](STORAGE.md), [CALLS.md](CALLS.md) |
| Earn from a node | [SERVICE_REWARDS.md](SERVICE_REWARDS.md) |
| Run or audit moderation | [MODERATION.md](MODERATION.md) |
| See the actual test output | [PHASE1_REPORT.md](PHASE1_REPORT.md), §7 above |
| Recover from something | [DISASTER_RECOVERY.md](DISASTER_RECOVERY.md) |

The two most useful commands:

```bash
scripts/testnet/devnet.sh
scripts/testnet/phase2.sh
```

The first builds a genesis with the real tooling, starts a real chain, and
asserts the economic claims against it rather than against a mock. The
second starts the whole network on one machine and asserts that messages
decrypt only for their recipients, media replicates, forks are refused,
and the network survives losing the host it started on. If you only run one
thing, run the first; if you run two, run both.
