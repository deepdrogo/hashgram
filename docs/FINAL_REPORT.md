# Hashgram Build Report

What was built, what was verified, the operator procedures you asked for, and
what is not done.

Read [PHASE1_REPORT.md](PHASE1_REPORT.md) alongside this. That document holds
the verbatim output of every test run; this one holds the architecture, the
procedures, and an honest accounting of the remainder.

---

## 1. What exists

### Binaries

| Binary | Purpose | Size |
| --- | --- | --- |
| `hashgramd` | The node: Cosmos SDK v0.53.8 on CometBFT v0.38.26 | 116 MB |
| `hashgramctl` | Operator CLI: 23 subcommands | 112 MB |
| `hashgram-test-client` | Developer client, exercises the protocol from outside a node | 34 MB |
| `hashgram-keygen` | Offline key generation. No networking, no disk writes. | 34 MB |

Static, CGO-free, reproducible. Built with `-trimpath -buildvcs=false` and no
build-date or commit stamp, because either would make the output
unreproducible and an operator who cannot reproduce a checksum cannot verify a
download.

### Code

| | |
| --- | --- |
| Authored Go | 33,486 lines, 160 files |
| Go tests | 7,914 lines, 343 test functions |
| Rust | 3 crates, 72 tests |
| Generated protobuf | 60,273 lines |
| Proto definitions | 3,232 lines, 33 files |
| Shell (install, testnet, CI) | 3,757 lines, 10 files |
| Documentation | 16 documents |

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

### The Rust layer

Phase 2 foundation, tested and CI-wired:

- `hashgram-net`: network identity, signing domains, canonical preimages
  **byte-identical to Go**, verified against 90 generated vectors
- `hashgram-net::Handshake`: verifies all five identity parts, closing the gap
  CometBFT leaves open
- `hashgram-p2p`: peer scoring with proportional decay, per-subnet connection
  limits, config validation

The libp2p swarm itself and everything riding on it is not built. See §9.

---

## 2. Verifying the Founder claims

The two claims most worth checking independently, with the exact commands.

### 200,000,000 HASH, with 180,000,000 vesting

```bash
hashgramctl wallet-info <founder-address>
```

Expect:

```text
Balance           200000000000000 uhash  (200000000 HASH)
Spendable now     20000000000000 uhash  (20000000 HASH)
Locked (vesting)  180000000000000 uhash  (180000000 HASH)
```

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
   hashgramctl mainnet-preflight        # until it passes

C  Collect gentxs from the initial validators

D  hashgramctl init-mainnet-genesis --founder-address hash1...
   Review the printed distribution. Record the genesis hash.
   Have a second person rebuild it and confirm the same hash.

E  Publish the genesis file, and its hash through channels INDEPENDENT of
   the file. A file next to its own hash proves nothing.

F  Operators join
   hashgramctl join-mainnet --genesis-url <url> \
     --genesis-hash <from an independent source> --peers <id>@<host>:26656
   hashgramctl network-info             # confirm the pin

G  Verify: supply, Founder allocation, vesting, fee share, no mint module
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
- **Any message plaintext.** Nothing is logged, and Phase 2's messaging is
  end-to-end encrypted by design.
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

### One machine, storage and relay (Phase 2)

```bash
hashgramctl configure-role relay,store,media
hashgramctl restart
hashgramctl storage           # assignments, challenges, disk
hashgramctl rewards           # credit and earnings
```

Runs as `hashgram-node` in `/var/lib/hashgram/node`. Combines naturally:
bandwidth and disk on one box.

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

**Never opened:** 26657, 1317, 9090, 26660, 5432, and the monitoring ports.
`mainnet-preflight` fails if the admin RPC is publicly reachable.

---

## 7. Test results

Full verbatim output in [PHASE1_REPORT.md](PHASE1_REPORT.md). Summary:

| Suite | Result |
| --- | --- |
| `go test -race ./...` | 16 packages, all passing, 343 test functions |
| `cargo test --all` | 72 tests passing |
| `scripts/testnet/devnet.sh` | 22 acceptance checks against a live chain, all passing |
| `scripts/testnet/four-validator.sh` | Consensus, resilience and fork isolation, all passing |
| `gofmt`, `go vet`, `staticcheck` | Clean |
| `gosec` | Clean, 140 files, 25,706 lines |
| `gitleaks` | No secrets |
| `govulncheck` | Zero unreviewed vulnerabilities in the shipped binary |
| `cargo clippy` | Clean with the workspace deny list |
| `scripts/dev/release.sh --verify` | Byte-identical across builds |

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
for an operator handed the wrong file. The Rust `hashgram-net::Handshake`
closes the transport-layer gap by verifying all five identity parts, and is
tested.

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

The specification was 116 sections. Phase 1 is complete. Phase 2 is a
foundation plus a large remainder.

### Built and tested

- The entire chain: eight modules, genesis tooling, operator CLI
- Monitoring: 22 custom metrics, three dashboards, 24 alert rules
- CI: tests, four static analysers, a secret scanner, a vulnerability scanner
- Reproducible release build
- 16 documents, verified against the code by a CI check
- Rust: network identity with Go parity, the five-part handshake, peer
  scoring, connection limits

### Not built

| | |
| --- | --- |
| libp2p swarm | Transport, Gossipsub, Kademlia, identify, autonat, relay |
| Bootstrap discovery | DHT, peer exchange, signed records |
| OpenMLS messaging | End-to-end encryption, groups, multi-device |
| Envelope store | Store-and-forward for offline recipients |
| Social events | Signed posts, follows, reactions, channels, reels |
| Blob storage | Content addressing, chunking, replication, repair |
| PostgreSQL indexer | And every query it would serve |
| Safety engine | Content attestations |
| Calls | Node announcements, TURN reconfiguration, SFU |
| Client SDKs | Rust, TypeScript, Swift, Kotlin, C# |
| Fuzz targets | P2P parsing, social events, blob manifests |
| Applications | Windows, iOS, Android. Specifications only. |
| Mainnet | Requires a Founder address generated off this server. |

This was a deliberate choice, not an oversight. The plan for this work stated
that every phase would actually be built, run and tested before moving on,
rather than leaving empty placeholders. A messaging layer stubbed out to tick
a box would be worse than an absent one: it would look like a foundation to
build on, and it would have to be thrown away.

The signing domains for social events, content attestations and node
announcements **are** defined and have test vectors, so a client or a future
node can implement the signing side ahead of the transport.

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

**No formal verification and no independent audit.** 343 Go tests and 72 Rust
tests are good and are not proofs. One implementation, no second to disagree
with it.

---

## 10. What would most improve this

In order of how much each would change the picture:

1. **An independent security audit**, published, with findings addressed.
2. **Twenty or more validators, independently operated, in five or more
   jurisdictions**, with no operator above 10% of voting power.
3. **A second implementation of the protocol**, so a consensus bug in one is
   caught by the other rather than becoming network-wide.
4. **Cross-machine reproducibility** confirmed by a second builder.
5. **The Phase 2 P2P layer**, built with the same standard as Phase 1.

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
| See the actual test output | [PHASE1_REPORT.md](PHASE1_REPORT.md) |
| Recover from something | [DISASTER_RECOVERY.md](DISASTER_RECOVERY.md) |

The single most useful command:

```bash
scripts/testnet/devnet.sh
```

It builds a genesis with the real tooling, starts a real chain, and asserts
the economic claims against it rather than against a mock. If you only run one
thing, run that.
