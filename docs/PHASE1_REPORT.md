# Phase 1 Checkpoint Report

What was built, what was actually run, what the runs produced, and what is not
finished.

Every result below is copied from a real run on the server, not summarised
from intent. Commands are reproducible.

**Date of this run:** the figures come from a single sequence of runs on
Ubuntu 24.04.4 LTS, 2 vCPU, 7.8 GB RAM.

---

## 1. What exists

| | |
| --- | --- |
| Go | 1.26.8 |
| Cosmos SDK | v0.53.8 |
| CometBFT | v0.38.26 |
| Authored Go | 33,486 lines across 160 files |
| Tests | 7,914 lines across 21 files, 343 test functions |
| Generated protobuf | 60,273 lines |
| Proto definitions | 3,232 lines across 33 files |
| Shell (install, testnet, CI) | 3,757 lines across 10 files |
| Documentation | 3,678 lines across 12 files |

Four binaries: `hashgramd`, `hashgramctl`, `hashgram-test-client`,
`hashgram-keygen`.

Eight custom modules: `x/network`, `x/founder`, `x/feerouter`, `x/welcome`,
`x/serviceproof`, `x/username`, `x/identity`, `x/treasury`.

`x/mint` and `x/circuit` are deliberately absent, which is what makes "no
inflation" and "no kill switch" structural rather than configured.

---

## 2. Test results

```bash
go test -race -timeout 30m ./...
```

Actual output:

```text
ok  github.com/hashgram/hashgram/app                             3.564s
ok  github.com/hashgram/hashgram/app/canonical                   1.267s
ok  github.com/hashgram/hashgram/app/params                      1.125s
ok  github.com/hashgram/hashgram/genesis                         8.683s
ok  github.com/hashgram/hashgram/tools/tokenomics-simulator/model 6.217s
ok  github.com/hashgram/hashgram/x/feerouter/keeper               1.294s
ok  github.com/hashgram/hashgram/x/founder/keeper                 1.212s
ok  github.com/hashgram/hashgram/x/founder/types                  1.160s
ok  github.com/hashgram/hashgram/x/identity/keeper                2.083s
ok  github.com/hashgram/hashgram/x/network/keeper                 1.225s
ok  github.com/hashgram/hashgram/x/serviceproof/keeper           23.104s
ok  github.com/hashgram/hashgram/x/serviceproof/types             3.462s
ok  github.com/hashgram/hashgram/x/username/keeper                1.248s
ok  github.com/hashgram/hashgram/x/username/types                 1.149s
ok  github.com/hashgram/hashgram/x/welcome/keeper                 1.959s
ok  github.com/hashgram/hashgram/x/welcome/types                  1.167s
```

16 packages, all passing, with the race detector on.

---

## 3. Devnet acceptance run

```bash
scripts/testnet/devnet.sh
```

Builds a genesis with the real tooling, starts a chain, and asserts the
economic claims against it. Actual output:

```text
[ ok ] blocks are being produced (1 -> 2)
[ ok ] total supply is exactly 1,000,000,000 HASH
[ ok ] supply unchanged across blocks: there is no inflation
[ ok ] Founder holds exactly 200,000,000 HASH
[ ok ] Founder has exactly 20,000,000 HASH spendable at genesis
[ ok ] Founder revenue beneficiary is the supplied address
[ ok ] Founder revenue share is 100 bps (1%)
[ ok ] Alice sent 100 HASH and Bob received exactly 100 HASH (no transfer tax)
[ ok ] Founder accrued 2 uhash from transaction gas fees
[ ok ] on-chain network id is hashgram-devnet (not mainnet)
[ ok ] genesis on disk still matches the pinned hash
[ ok ] no mint module: inflation is not merely zero, it is absent
[ ok ] 4 named treasury reserves are individually queryable
[ ok ] useful-service reserve holds exactly 500,000,000 HASH
[ ok ] welcome pool holds exactly 1,850,000 HASH
[ ok ] no welcome attestors registered: creating a key earns nothing
[ ok ] @alice registered and resolves to Alice
[ ok ] @a1ice rejected as visually confusable with @alice
[ ok ] @admin is reserved
[ ok ] protocol revenue is recorded per service kind
[ ok ] this node is validating with voting power 90000000
[ ok ] mainnet-preflight correctly refuses to pass on a devnet

All acceptance checks passed.
```

Two of these are worth reading carefully.

**"Founder accrued 2 uhash from transaction gas fees."** Two uhash, not two
HASH. That is 1% of the roughly 200 uhash of gas the test transactions
actually consumed. It is a real measurement of a real fee, and it is the
strongest available demonstration that the share applies to fees rather than
to principal: the same run confirms Bob received all 100 HASH of the transfer.

**"mainnet-preflight correctly refuses to pass on a devnet."** This tests the
gate itself. The preflight check is the only thing standing between a devnet
key and a live network, so a run that proves it refuses is worth more than one
that proves it accepts.

### Live state from that run

`hashgramctl chain-status`:

```text
Network            Hashgram Devnet (DEVNET ONLY)
Network id         hashgram-devnet
Chain id           hashgram-devnet-1
Genesis hash       9348af00681eecefb8d6329d5ba101c13bc3c8943f2c610295026f6503654287
Height             14
Catching up        false
Validators         1 total, 1 bonded, 0 jailed
  resilience       1 validator: this is a bootstrap single point of
                   availability, not a decentralised network
Total supply       1,000,000,000,000,000 uhash
  fixed supply     correct: exactly 1,000,000,000 HASH, and there is no
                   minting module
```

`hashgram-test-client founder verify`:

```text
FOUNDER CONFIGURATION
Beneficiary       hash1c8mh6gupw9sv8c3eq20x0aqljjysyuqrym67ee
Revenue share     100 bps (1.00%)
Ceiling           100 bps; governance cannot exceed this without a
                  new binary the validator set adopts

ALLOCATION
Balance           200000000000000 uhash  (200000000 HASH)
Spendable now     20000000000000 uhash  (20000000 HASH)
Locked (vesting)  180000000000000 uhash  (180000000 HASH)

Correct: exactly 200,000,000 HASH, the documented Founder allocation.

REVENUE LEDGER
Total accrued     10004uhash
Total paid        0
Pending           10004uhash

BENEFICIARY HISTORY
height 0          (genesis) -> hash1c8mh6gupw9sv8c3eq20x0aqljjysyuqrym67ee
```

Note that the tool tells you how to check its own claim without trusting it:

```bash
hashgramd query bank balances hash1t9zc2z9qsf707huqa5y3a0vgpra7vlyhyvdeeh
```

---

## 4. Four-validator run

```bash
scripts/testnet/four-validator.sh
```

Four validators with equal stake on one host, then a validator is killed,
then two different kinds of fork try to join. Actual output:

```text
[ ok ] peer discovery works: every validator found the other three
[ ok ] block propagation: all four within 2 blocks (4, 4, 4, 4)
[ ok ] four bonded validators
[ ok ] supply unchanged across blocks with four validators: no inflation
[ ok ] delegation increased validator stake from 50000000000000 to 50001000000000

[ ok ] validator3 stopped
[ ok ] chain continued: 4 blocks in 20 seconds (8 -> 12)
[ ok ] losing one of four validators does not stop the network
[ ok ] the three survivors remain in agreement (12, 12, 12)
[ ok ] transactions still process with one validator down
[ ok ] validator3 rejoined and caught up to height 14

[ ok ] REJECTED at the handshake: the alien fork established 0 peers
[ ok ] the real validators name the reason in the alien fork's log
[ ok ] squatting fork shares the chain id but not the genesis: 9fe8508487bf7822...
[ ok ] as expected, the transport connected (2 peer(s)): chain id alone is
       not fork protection
[ ok ] the two chains never converge: app hash 8175B348... vs 5F7E9625...
[ ok ] all four real validators still share one app hash: the fork injected nothing
[ ok ] the real chain advanced 19 -> 26 while the fork was connected
[ ok ] the real validator set is still exactly 4: the fork's validator was
       never admitted
[ ok ] hashgramctl refused the fork genesis against the pinned hash (exit 1)
[ ok ] even a self-consistent fork genesis is refused

All checks passed.
```

### The finding in that run

The fork test was originally written expecting any fork to be refused at the
handshake, and it **failed**. Investigating showed why, and the finding is
real rather than a test bug:

**CometBFT's peer handshake compares chain ids. It does not hash the genesis
file.** A fork that keeps `hashgram-devnet-1` and changes only the genesis
therefore opens a transport connection. It cannot join consensus — the run
above confirms the real chain kept advancing, all four validators kept one app
hash, and the validator set stayed at exactly four — but the connection
happens.

The test now checks all three layers separately and reports what each one
actually does, including the case where the first one does not hold. That is
also the concrete justification for the pinned genesis hash in
`/etc/hashgram/network.json` and for the `genesis_hash` field in the Phase 2
P2P handshake: they are not duplicated effort, they close a gap CometBFT
leaves open.

### What this run did not demonstrate

Stated in the script's own output, and worth repeating: all four validators
run on one host as one operating-system user. That tests consensus and gossip.
It does not test network partitions, independent operators, or geographic
distribution.

**Four validators on one VPS is still one VPS.**

---

## 5. Security scan results

```bash
scripts/dev/ci.sh
```

Actual output:

```text
>>> gofmt          all files formatted
>>> go vet         clean
>>> staticcheck    clean on authored code (32 findings, all in generated files)
>>> gosec          clean: 139 files, 25572 lines scanned
>>> gitleaks       no secrets found in the working tree
>>> govulncheck    no unreviewed vulnerabilities in build/hashgramd
                   23 dependency advisories matched at module level only
                   2 reviewed and accepted, with reasons in this script
>>> policy checks  logging policy holds
                   dashboards and alert rules valid
                   documentation matches the code
>>> go test -race  all tests pass (16 packages)

CI passed.
```

### What the scanners found and what was done about it

The scanners were not run once and declared green. They surfaced real work.

**Four copies of the signing preimage framing logic.** `x/welcome`,
`x/serviceproof` and `x/identity` each had their own implementation of
length-prefixed canonical encoding. Four copies is four places for the framing
to drift, and framing drift in signing code is signature confusion.
Consolidated into `app/canonical`, which enforces the length bound all four
previously assumed, with ten tests including one that asserts `("ab","c")` and
`("a","bc")` cannot collide.

**Two layers needed different prefix widths.** The outer domain-separation
wrapper and Merkle challenge responses frame arbitrary-length payloads, not
bounded fields, so a 32-bit prefix needed an overflow check with no meaningful
handling at that layer. They now use 64-bit prefixes, which cannot overflow
because a Go `len()` is a non-negative int. The distinction between the two
builders is enforced by the type rather than by a comment.

**A hand-rolled hex parser.** `hashgram-keygen` parsed hex with a `Sscanf`
loop. Replaced with `encoding/hex`, which validates strictly and returns a
typed error naming the offending byte.

**Two unbounded narrowings in consensus code.** `ChallengeChunkIndex` was
being reused to pick which storage assignment to challenge, which required
converting a loop counter and a slice length to `uint32`. Split into a
separate `SelectIndex` that works in int space, domain-separated from the
chunk index so the two selections cannot correlate. Five tests.

**A dead check that staticcheck caught.** A guard added around the `statfs`
block size also checked `Blocks` and `Bavail` for negativity — but both are
already `uint64` on Linux, so those comparisons were dead code. Corrected.

**Thirteen symbol-level dependency vulnerabilities in the shipped binary.**
Ten dependencies upgraded, closing all thirteen. Down to zero unreviewed.

**One advisory verified as a false positive.** GO-2024-2584, slashing evasion
in the Cosmos SDK, matched. The upstream advisory (ASA-2024-005) says it was
patched in SDK 0.50.5 and 0.47.10; this tree is on v0.53.8. The Go
vulnerability database entry carries a second affected range, "introduced
0.50.0", with no corresponding fixed event, so every release in that line
matches regardless of the patch. Recorded on an allowlist with the evidence,
not silently ignored.

### A correction to the Makefile

`CGO_ENABLED` was set to 1 with a comment claiming CGO was required for the
database backend. Tested rather than trusted: a `CGO_ENABLED=0` build of
`hashgramd` was run as a single-node devnet and produced blocks. The default
backend is goleveldb and the default secp256k1 is btcec, both pure Go.

CGO is now off by default. The binaries are static, do not depend on the build
host's libc, and are 27% smaller. It also made the build reproducible, which
matters more than the size.

---

## 6. Reproducible release

```bash
VERSION=v0.1.0-devnet scripts/dev/release.sh --verify
```

Builds twice with the build cache cleared in between, then compares bytes:

```text
[ ok ] hashgramd is byte-identical across builds (116211874 bytes)
[ ok ] hashgramctl is byte-identical across builds (112033954 bytes)
[ ok ] hashgram-test-client is byte-identical across builds (34341026 bytes)
[ ok ] hashgram-keygen is byte-identical across builds (34058402 bytes)

d522c552ee8b20ad1b1eed22ad99bb4216d2d116c261c24a572cbaf56130936c  hashgram-keygen
8b377702d6f481eac652b0656ab8daafb7003007254051662bf86755ae4a3632  hashgram-test-client
fcfe4b3266999f2e9169627b5d1c0454bc656c20d9329ae91a3e6140d6757308  hashgramctl
98fcd3164f93e9868af5ec477c32b04a5089252fde8387499e9e05249d5df628  hashgramd

The build is reproducible on this machine.
```

These checksums matched across three independent builds in the same session.

**What this proves:** the build embeds no timestamps, paths or git state, so
two builds from the same source produce identical bytes.

**What it does not prove:** that a different machine produces the same bytes.
That depends on the Go toolchain matching exactly, which is why `go.mod` pins
it, why `dist/BUILD_INFO` records it, and why CI fails if the runner
disagrees. Cross-machine reproducibility should be confirmed by a second
builder before a release is published.

---

## 7. Observability

22 Hashgram gauges, verified present on a running node:

```text
hashgram_supply_total_uhash                  1e+14   (devnet total)
hashgram_supply_ceiling_uhash                1e+15
hashgram_supply_over_ceiling_uhash           0
hashgram_founder_fee_basis_points            100
hashgram_founder_revenue_accrued_uhash       0
hashgram_founder_revenue_paid_uhash          0
hashgram_founder_revenue_pending_uhash       0
hashgram_revenue_qualifying_total_uhash      0
hashgram_revenue_founder_share_uhash         0
hashgram_revenue_validator_share_uhash       0
hashgram_service_reserve_remaining_uhash     0
hashgram_service_reserve_remaining_ratio     0
hashgram_service_emitted_total_uhash         0
hashgram_service_epoch                       0
hashgram_service_epoch_budget_uhash          0
hashgram_service_bonded_total_uhash          0
hashgram_service_providers{status=...}        0
hashgram_service_open_challenges             0
hashgram_service_storage_assignments         0
hashgram_service_assigned_bytes              0
hashgram_welcome_next_sequence               1
hashgram_welcome_paid_total_uhash            0
hashgram_welcome_pool_remaining_uhash        1.85e+12
```

Three dashboards with 42 panels, 24 alert rules loaded, none firing. Verified
end to end through Grafana's datasource proxy, which is the only check that
exercises the whole path a panel uses:

```text
chain height           349
supply over ceiling    0
founder fee bps        100
welcome tier HASH      50
disk percent           23.624910446860415
```

### Three things found by measuring rather than assuming

Metric names were taken from scraping a running node, and that is how these
surfaced.

**`cometbft_p2p_peers` does not exist until a node's first peer event.** It is
a lazily-created Prometheus child. An alert comparing it to zero cannot fire
on a node that has never peered, so there is a separate `absent()` rule for
that state and the dashboard panel reads "never peered" rather than zero.

**CometBFT exports no missed-blocks counter.** Validator signing health has to
be derived from the gap between the chain height and the validator's last
signed height. A dashboard promising a missed-blocks graph would have been
promising a metric that does not exist.

**The SDK's telemetry API cannot represent a uhash amount.** It takes a
`float32`, whose 24-bit mantissa is exact only to about 1.7e7. Supply reaches
1e15, so total supply and its ceiling would both round to the same `float32`
and a breach would be invisible in the one metric that exists to detect it.
The Hashgram gauges are registered directly with the Prometheus default
registry as `float64`, and the excess is computed with exact integers before
conversion.

### A bug found while writing this report

`hashgramctl network-info` compared the hash of the genesis file on disk
against the hash of the genesis the RPC serves, and warned when they differed,
suggesting a restart was needed.

They can never match. CometBFT parses the genesis file and re-serialises it:
it drops the SDK's `app_name` and `app_version` fields, renders
`initial_height` as a string, and emits compact rather than indented JSON.
Verified directly — the disk file hashes to `9348af00...` and the RPC response
to `efdc56f3...` on a healthy node.

So the check warned on every healthy node, which is how operators learn to
ignore diagnostics. It now compares the **chain id** the running node reports
against the pinned chain id — the identity CometBFT actually enforces at the
handshake — and states plainly why the genesis hashes are not comparable.

---

## 8. Your Founder key

`hashgram-keygen` is built and ready. **Do not run it on this server.**

```bash
# On the build machine, record the checksum:
sha256sum build/hashgram-keygen

# Copy it to an offline machine. Verify the checksum there. Then:
./hashgram-keygen new
```

The binary contains no networking code and writes nothing to disk. The
mnemonic is printed once, to your terminal.

**Nothing in this repository will ever ask for your seed phrase.** The only
thing that leaves the offline machine is the public `hash1...` address, and
that is all the genesis tooling needs:

```bash
hashgramctl init-mainnet-genesis --founder-address hash1...
```

The tooling has no code path that generates an address, which is what makes
"this server never saw your private key" a structural property rather than a
promise.

Verify your written backup by deriving the address and comparing it:

```bash
./hashgram-keygen derive
```

Be precise about what this catches. BIP-39's checksum detects most single-word
errors and typos. It does **not** reliably detect two words being swapped,
because a swap can produce a valid checksum. An earlier version of this tool's
help text overstated that, and a swapped-word test produced a silently valid,
wrong address. So the verification that matters is the **derived address
matching**, not the mnemonic being accepted.

Full procedure: [FOUNDER_LAUNCH_RUNBOOK.md](FOUNDER_LAUNCH_RUNBOOK.md) Part A.

---

## 9. Commands you can run right now

```bash
# Everything, from scratch
make build
make ci

# A real chain with 22 acceptance checks against it
scripts/testnet/devnet.sh
scripts/testnet/devnet.sh clean

# Four validators, a killed validator, two kinds of fork
scripts/testnet/four-validator.sh
scripts/testnet/four-validator.sh clean

# Tokenomics over decades, using the chain's own emission functions
make tools
build/tokenomics-simulator -all

# Reproducible release with checksums
scripts/dev/release.sh --verify

# Monitoring, reachable only over SSH
sudo scripts/install/monitoring.sh
ssh -N -L 9090:127.0.0.1:9090 -L 3000:127.0.0.1:3000 root@<this-host>
```

---

## 10. Known limitations

The section that determines how much of the above to rely on.

### Deployment

**Everything runs on one server.** The four-validator test runs four
validators on one host as one operating-system user. Until mainnet has
independent operators in different jurisdictions, "decentralised" describes
the design and not the deployment. See
[DECENTRALIZATION.md](DECENTRALIZATION.md) section 3, which is longer than
this paragraph.

**Mainnet has not launched.** It requires a Founder address generated off this
server, which is step A of the runbook and is the one step this software
cannot do for you.

### Protocol

**CometBFT's handshake does not verify the genesis hash.** A same-chain-id
fork reaches the transport layer. Consensus and operator tooling turn it away;
the P2P layer today does not. Phase 2 closes this.

**Storage challenges prove retrievability, not unique replication.** A
challenge proves the provider can produce a chunk, not that it stores it
independently rather than fetching it on demand. The twenty-minute response
window makes on-demand fetching awkward and is not a proof of replication.
Proof-of-replication schemes exist and are considerably more expensive; the
trade was made knowingly.

**The welcome attestor is trusted.** `x/welcome` verifies the signature. It
cannot verify the attestor is honest. A malicious attestor can issue
attestations for addresses it controls, up to its per-epoch cap. The cap
bounds it and on-chain visibility makes it detectable after the fact.

**No formal verification.** The economic invariants are enforced by 343 tests
and a runtime metric. They are not machine-checked proofs. The tests are good;
they are not the same thing.

**No independent security audit.** One implementation, no second
implementation to disagree with it and expose a consensus bug, no external
review.

### Phase 2 is unbuilt

Not started: the Rust `hashgram-node`, libp2p transport, OpenMLS end-to-end
encryption, the offline envelope store, signed social events,
content-addressed blob storage, the PostgreSQL indexer, the safety engine,
call discovery, client SDKs, and applications for any platform.

Any statement about the security of Hashgram messaging today is a statement
about a design, not about code. The chain-level identity registry that Phase 2
will build on does exist and is tested.

---

## 11. Honest assessment

The original specification was 116 sections and is realistically several
engineers for several months. Phase 1 is genuinely finished rather than
scaffolded: the chain runs, the economics are enforced by the state machine
and verified against a live chain, the operator tooling refuses unsafe
actions, the build is reproducible, and the security scanners are green after
the work they surfaced was actually done rather than suppressed.

What would most improve confidence, in order:

1. An independent security audit.
2. Validators run by other people, in other places.
3. A second implementation of the protocol.
4. Cross-machine reproducibility confirmed by a second builder.

None of those are things this session could produce, and the first three are
not things any amount of code can produce.
