# Hashgram

A decentralised network with a fixed supply, a finite reward reserve and no
central point of control.

This repository holds Hashgram Core: the blockchain, the operator tooling and
the genesis machinery. **Phase 1 is complete and runs.** The peer-to-peer
messaging, social and storage layers are Phase 2 and are not built yet; see
[Status](#status) for exactly what exists.

## What is different about it

Most of these are claims any chain can make. Each one below links to the code
that enforces it and the test that proves it, because a claim in a README is
worth nothing on its own.

**There is no inflation, and not because a parameter is set to zero.** The
`x/mint` module is not wired into the application at all. A parameter set to
zero can be changed by a governance proposal; a module that is absent requires
a new binary the validator set has to adopt. Every uhash that will ever exist
is created in the genesis block.
[`app/app.go`](app/app.go) ·
[`app/app_test.go`](app/app_test.go) asserts no module holds the minter
permission.

**The Founder's 1% is a share of protocol fee revenue, never a tax on
transfers.** If you send someone 100 HASH they receive exactly 100 HASH. The
Founder share applies only to fees the protocol itself has already collected,
and it is capped at 100 basis points by a compile-time constant that
governance cannot raise.
[`x/founder`](x/founder) · [`x/feerouter`](x/feerouter) ·
[`x/feerouter/keeper/route_test.go`](x/feerouter/keeper/route_test.go)

**Rewards for useful work, not for wasted electricity.** Storage providers are
paid on bytes the network assigned to them and challenges they answered. Relay
and call providers are paid on receipts the served client signed. A node that
reports its own traffic earns nothing, and a pair of nodes trading fake
receipts is discounted to 20% of what they claim.
[`x/serviceproof`](x/serviceproof)

**The reward reserve is finite and cannot be topped up.** 500,000,000 HASH at
genesis, spent on a declining schedule that takes a fixed fraction of what
remains each epoch. It asymptotes rather than hitting a cliff, and when it is
low, operators are funded by real service revenue instead of subsidy.
[`x/serviceproof/types/emission.go`](x/serviceproof/types/emission.go)

**A fork of this software is a different network, and the code says so.**
Network identity is five parts: name, network id, chain id, genesis hash and a
four-byte magic. Signatures are domain-separated by network, so an attestation
issued on one Hashgram network cannot be replayed on another.
[`x/network`](x/network) · [`app/params/network.go`](app/params/network.go)

**No master key, no kill switch, no override.** There is no administrative
transaction that can freeze an account, reverse a transfer, mint a coin or
change the supply. Treasury spending requires a governance proposal and leaves
a disbursement record.
[docs/DECENTRALIZATION.md](docs/DECENTRALIZATION.md)

## Status

| Component | State |
| --- | --- |
| Blockchain (Cosmos SDK v0.53, CometBFT v0.38) | Runs. Devnet and a four-validator testnet both pass. |
| Eight custom modules | Implemented and tested. |
| Genesis tooling, `hashgramctl`, `hashgram-keygen` | Complete. |
| Prometheus metrics, Grafana dashboards, alerts | Complete and verified against a running node. |
| CI: tests, staticcheck, gosec, gitleaks, govulncheck | Green. |
| Reproducible release build | Verified byte-identical. |
| Mainnet | **Not launched.** Requires a Founder address generated off this server. |
| P2P messaging, social, storage, calls (Rust) | **Not built.** Phase 2. |
| Windows, iOS, Android apps | Not built. Specifications only. |

## The chain

Eight modules on top of the standard Cosmos SDK set.

| Module | What it does |
| --- | --- |
| [`x/network`](x/network) | Network identity, genesis hash pinning, signature domain separation |
| [`x/founder`](x/founder) | The Founder revenue ledger and its compile-time fee ceiling |
| [`x/feerouter`](x/feerouter) | Splits protocol fee revenue; never touches transferred principal |
| [`x/welcome`](x/welcome) | Tiered joining reward, 50/5/1/0 HASH, capped at 1,850,000 HASH |
| [`x/serviceproof`](x/serviceproof) | Proof of Useful Service: receipts, storage challenges, bonds, fraud scoring |
| [`x/username`](x/username) | `@name` registry with confusable-character defence |
| [`x/identity`](x/identity) | Root identities, device certificates, social recovery. Public keys only. |
| [`x/treasury`](x/treasury) | Named genesis allocations, spendable only by governance |

Standard modules: `auth`, `bank`, `staking`, `slashing`, `distribution`,
`gov`, `evidence`, `vesting`, `upgrade`, `consensus`, `genutil`, `feegrant`,
`authz`. `x/mint` and `x/circuit` are deliberately excluded.

## Tokenomics

1,000,000,000 HASH, fixed forever. 1 HASH = 1,000,000 uhash.

| Allocation | HASH | Share |
| --- | --- | --- |
| Founder | 200,000,000 | 20% |
| Community / useful-service reserve | 500,000,000 | 50% |
| Treasury | 150,000,000 | 15% |
| Growth (includes the 1,850,000 welcome pool) | 50,000,000 | 5% |
| Developer grants | 50,000,000 | 5% |
| Liquidity | 50,000,000 | 5% |

The Founder allocation is 20,000,000 spendable at genesis and 180,000,000 in a
`PeriodicVestingAccount` released monthly over eight years.

Full detail, and the reasoning behind each number, in
[docs/TOKENOMICS.md](docs/TOKENOMICS.md).

## Build and run

Requires Go 1.26.8 and a Linux host. Ubuntu Server 24.04 LTS is what the
tooling targets and what it has been tested on.

```bash
make build          # binaries into build/
make test           # go test -race
make ci             # the whole pipeline: lint, gosec, gitleaks, govulncheck, tests
```

Bring up a single-node devnet and run the acceptance checks against it:

```bash
scripts/testnet/devnet.sh
```

That script builds a genesis with the real tooling, starts a chain, and then
verifies the claims made above against it: that a 100 HASH transfer delivers
100 HASH, that the Founder accrues exactly 1% of collected fees, that total
supply is exactly 1e15 uhash and does not move, that there is no mint module,
that a visually confusable username is refused, and that
`hashgramctl mainnet-preflight` refuses to pass on a devnet host.

Four validators, a killed validator, and fork isolation:

```bash
scripts/testnet/four-validator.sh
```

## Binaries

| Binary | Purpose |
| --- | --- |
| `hashgramd` | The node. Cosmos SDK application and CometBFT. |
| `hashgramctl` | Operator CLI: install, join, roles, status, backup, preflight. |
| `hashgram-test-client` | Developer client that exercises the protocol from outside a node. |
| `hashgram-keygen` | Offline key generation. No networking, no disk writes. |

## Documentation

Written from the code, not from intent. Where something is not implemented,
the document says so rather than describing it in the present tense.

**Start here**
- [ARCHITECTURE.md](docs/ARCHITECTURE.md) — how the pieces fit together
- [TOKENOMICS.md](docs/TOKENOMICS.md) — supply, allocations, emission, the Founder share
- [OPERATIONS.md](docs/OPERATIONS.md) — running a node day to day

**Running a node**
- [NODE_ROLES.md](docs/NODE_ROLES.md) — the eight roles and what each needs
- [MAINNET.md](docs/MAINNET.md) — genesis procedure and joining an existing network
- [DISASTER_RECOVERY.md](docs/DISASTER_RECOVERY.md) — backups, restores, key loss, compromise

**Security**
- [SECURITY.md](docs/SECURITY.md) — key handling, hardening, what is protected and how
- [THREAT_MODEL.md](docs/THREAT_MODEL.md) — what an attacker can and cannot do
- [DECENTRALIZATION.md](docs/DECENTRALIZATION.md) — an honest account of where control sits
- [LOGGING_POLICY.md](docs/LOGGING_POLICY.md) — what a node may and may not log

**Launch**
- [FOUNDER_LAUNCH_RUNBOOK.md](docs/FOUNDER_LAUNCH_RUNBOOK.md) — the full launch sequence

**Building a client**
- [PROMPT_WINDOWS_DESKTOP.md](docs/PROMPT_WINDOWS_DESKTOP.md) — the complete API surface and a Windows build specification
- [PROMPT_IOS_APP.md](docs/PROMPT_IOS_APP.md) — what differs on iOS
- [PROMPT_ANDROID_APP.md](docs/PROMPT_ANDROID_APP.md) — what differs on Android

**Status**
- [PHASE1_REPORT.md](docs/PHASE1_REPORT.md) — what was actually run and what it produced

## License

Apache License 2.0. See [LICENSE](LICENSE).
