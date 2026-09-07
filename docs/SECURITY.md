# Hashgram Security

What is protected, how, and what is not.

This describes measures that exist in the repository. Where something is
planned rather than built, it says so.

## Reporting a vulnerability

Do not open a public issue for a security bug. A consensus vulnerability
disclosed publicly before a patch is deployed is a vulnerability being
exploited.

Report privately, with enough detail to reproduce. Expect an acknowledgement,
a fix, and a coordinated disclosure once operators have had time to upgrade.

## Key domains

The single most important idea in this document: **Hashgram uses separate keys
for separate jobs, and compromising one does not yield the others.**

| Key | Lives | Can do | Cannot do |
| --- | --- | --- | --- |
| Founder cold wallet | Hardware wallet, offline | Spend the Founder allocation and revenue | Change protocol rules, mint, freeze anything |
| Validator consensus key | Node or remote signer | Sign blocks | Move funds |
| Validator operator key | Offline, not on the node | Stake, unstake, change commission | Sign blocks |
| Node P2P key | Node | Identify the node to peers | Move funds or sign blocks |
| Provider reward address | Wherever the operator chooses | Receive rewards | Operate the provider |
| User root identity key | User's device only | Authorise devices, rotate itself | Anything on another identity |
| User device key | One device | Send and receive messages | Authorise other devices |

Three consequences worth stating explicitly:

**A compromised validator cannot steal.** The consensus key signs blocks. It
cannot construct a transfer. An attacker who takes it can get you slashed for
double signing; they cannot take your stake.

**A compromised node cannot spend rewards.** `x/serviceproof` separates the
operator address from the reward address specifically so that a node running
on a rented server does not need a key that can also move the earnings.

**Hashgram never holds a user's private key.** `x/identity` stores public keys
only. There is no server-side key escrow, which is why account recovery is a
threshold of guardians the user chose rather than an administrative reset.

## Founder key handling

The Founder key controls 20% of the supply. It is generated **off this server
and off any server**, and only the public address is ever supplied to the
genesis tooling.

```bash
# On an offline machine. Not the VPS.
./hashgram-keygen new
```

`hashgram-keygen` has no networking code and writes nothing to disk. The
mnemonic is printed once, to the terminal.

Then, on the server, only the public address:

```bash
hashgramctl init-mainnet-genesis --founder-address hash1...
```

The tooling refuses to invent an address. It has no code path that generates
one, which means there is no code path by which this server could ever have
seen the private key.

### On BIP-39 checksums

`hashgram-keygen derive` recovers an address from a mnemonic, and it is the
right way to verify a written backup. But be precise about what BIP-39's
checksum catches: it detects most single-word errors and most typos. It does
**not** reliably detect two words being swapped, because a swap can still
produce a valid checksum.

So the verification that matters is comparing the **derived address** against
the one you recorded, not merely observing that the mnemonic was accepted.
The command's help text says this, because an earlier version overstated the
checksum's strength and a swapped-word test produced a silently valid, wrong
address.

Full procedure in [FOUNDER_LAUNCH_RUNBOOK.md](FOUNDER_LAUNCH_RUNBOOK.md).

## Validator key hardening

The consensus signing key, `priv_validator_key.json`, is the most sensitive
file on a validator. Two copies of it running at once is a double-signing
slash, and there is no recovery from that.

In increasing order of protection:

1. **File permissions.** 0600, owned by `hashgram-chain`.
   `hashgramctl mainnet-preflight` **fails the launch** if the key is readable
   by group or others.
2. **Sentry architecture.** The validator has no public P2P listener. It peers
   only with sentry nodes it controls, which absorb inbound connections. An
   attacker who cannot reach the validator cannot attack it directly.
3. **Remote signer.** `tmkms` on a separate machine, so the key never sits on
   the internet-facing host at all. Cosmos SDK v0.53 was chosen partly because
   it supports this.
4. **HSM.** `tmkms` with a YubiHSM or Ledger backend, so the key cannot be
   extracted even from the signer host.

For a validator with meaningful stake, 3 or 4 is the correct answer. For a
devnet, 1 is fine.

### Double signing

Only one process may ever hold a given consensus key. That includes: the same
key on a backup node "just in case", a restored snapshot started while the
original still runs, and a container orchestrator that helpfully restarts a
node elsewhere.

`priv_validator_state.json` tracks the last height and round signed and is the
mechanism that prevents a single node from double signing. It does not and
cannot protect against two nodes with the same key, because neither can see
the other's state file.

`hashgram_consensus_byzantine_validators` alerts on observed evidence. If it
fires for your own validator, stop the node immediately: every additional
block makes the slash worse.

## Host hardening

`scripts/install/bootstrap-ubuntu.sh` configures the host. What it does, and
why each piece is there:

### Service accounts

Five unprivileged users, one per role, each with its own data directory:

```text
hashgram-chain   /var/lib/hashgram/chain    0750
hashgram-node    /var/lib/hashgram/node     0750
hashgram-index   /var/lib/hashgram/index    0750
hashgram-safety  /var/lib/hashgram/safety   0750
hashgram-call    /var/lib/hashgram/call     0750
```

No shell, no home directory, no sudo. Compromising the relay does not give you
the validator's state, and compromising the safety engine does not give you
either: its unit lists the chain and node directories under
`InaccessiblePaths=`, because a content scanner has no business reading
consensus state or the envelope store.

### systemd confinement

Every unit in [`deploy/systemd/`](../deploy/systemd) carries:

| Directive | Effect |
| --- | --- |
| `ProtectSystem=strict` | The whole filesystem is read-only except `ReadWritePaths` |
| `ProtectHome=yes` | `/home`, `/root` and `/run/user` are inaccessible |
| `PrivateTmp=yes` | A private `/tmp`, so no shared-tmp attacks |
| `PrivateDevices=yes` | No raw device access |
| `NoNewPrivileges=yes` | setuid binaries cannot escalate |
| `CapabilityBoundingSet=` | Empty. No capabilities at all. |
| `AmbientCapabilities=` | Empty. |
| `SystemCallFilter=` | An allowlist; everything else returns EPERM |
| `SystemCallArchitectures=native` | Blocks the compat-syscall escape route |
| `MemoryDenyWriteExecute=yes` | No W+X pages, which blocks most shellcode |
| `LockPersonality=yes` | No `personality()` tricks |
| `RestrictNamespaces=yes` | Cannot create namespaces to escape |
| `RestrictSUIDSGID=yes` | Cannot create setuid files |
| `RestrictRealtime=yes` | Cannot starve the scheduler |
| `RestrictAddressFamilies=` | Only INET, INET6 and UNIX |
| `ProtectKernelModules/Tunables/Logs` | No kernel surface |
| `ProtectProc=invisible`, `ProcSubset=pid` | Cannot see other processes |
| `ProtectClock=yes` | Cannot change the system clock |
| `UMask=0077` | New files are private by default |
| `LimitNOFILE`, `TasksMax`, `MemoryMax` | Resource exhaustion is bounded |

`MemoryDenyWriteExecute` deserves a note: it is the single most effective
directive in that list against remote code execution, because most exploit
payloads need a writable-then-executable page. It works here because neither
Go nor the Rust node uses a JIT.

### Firewall

`ufw`, default deny inbound. Only what a role needs is opened:

| Port | Protocol | Opened for | Notes |
| --- | --- | --- | --- |
| SSH | TCP | Always | Detected from the running sshd, not assumed to be 22 |
| 26656 | TCP | All chain nodes | CometBFT consensus P2P |
| 26670 | TCP + UDP | `relay`, `store`, `media`, `bootstrap`, `call` | Hashgram P2P (QUIC on UDP, TCP fallback) |
| 3478, 5349 | TCP + UDP | `call` | TURN and TURN over TLS |
| 49152–65535 | UDP | `call` | TURN relay range |

**Never opened:** 26657 (CometBFT RPC), 1317 (REST), 9090 (gRPC), 26660
(metrics), 5432 (PostgreSQL), 9090/3000 (Prometheus and Grafana). These bind
to localhost and are reached over an SSH tunnel:

```bash
ssh -N -L 26657:127.0.0.1:26657 -L 3000:127.0.0.1:3000 operator@node
```

`mainnet-preflight` **fails** if the admin RPC is listening on a public
address. `scripts/install/monitoring.sh` checks the same for the monitoring
ports, and it rebinds them: the Debian packages bind Prometheus,
node-exporter and the Postgres exporter to every interface, which publishes an
operator's reconnaissance feed to the internet as a side effect of installing
a dashboard.

### PostgreSQL

Listens on localhost only, with a generated password. The indexer holds public
data that can be rebuilt from the chain, so it is not a confidentiality
boundary — but it is a write surface, and an exposed database is an exposed
database.

## Cryptography

Nothing is home-rolled. The primitives are the Cosmos SDK's and Go's standard
library. Where care was needed, it was in how they are composed.

| Purpose | Primitive |
| --- | --- |
| Account keys | secp256k1 |
| Validator consensus keys | ed25519 |
| Device and node keys | secp256k1 or ed25519 |
| Hashing | SHA-256 |
| Merkle trees (storage proofs) | SHA-256, domain-separated leaves and nodes |
| Addresses | Bech32 with the `hash` prefix |
| Key derivation | BIP-32/BIP-39/BIP-44, coin type 118 |
| Content addressing | BLAKE3 |
| Messaging | OpenMLS, RFC 9420 (`MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519`) |
| Client vault | Argon2id, XChaCha20-Poly1305 |
| Private media | XChaCha20-Poly1305 per blob, key shared inside MLS |
| TURN credentials | HMAC-SHA1 (`use-auth-secret`, the coturn standard) |

### Canonical signing preimages

Protobuf is not a canonical encoding. Field ordering, varint padding and
unknown fields all admit several encodings of the same logical message, so a
signature over "whatever proto produced" is a signature over something the
verifier may not reproduce.

Every signing preimage is hand-built with explicit length prefixes, in one
place: [`app/canonical`](../app/canonical).

The length prefixes are not decoration. Without them, `(method="ab",
extra="c")` and `(method="a", extra="bc")` produce identical bytes and a
signature for one verifies for the other. That is signature confusion, and it
is the reason four modules that each had their own copy of the framing logic
were consolidated into one implementation with tests: four copies is four
places for the framing to drift.

The package refuses to encode a field it cannot length-prefix unambiguously,
returning an error rather than a wrapped length, so callers fail closed.

### Domain separation

```text
digest = SHA-256( magic(4) || len(domain) || domain || len(payload) || payload )
```

The domain encodes both the network and the purpose. Consequences:

- A device certificate signature cannot be replayed as a service receipt.
- A devnet attestation cannot be replayed on mainnet.
- A signature from a fork of this software with a different magic does not
  verify here.

Nine purposes are defined: social event, device certificate, service receipt,
storage challenge, eligibility attestation, content attestation, bootstrap
record, peer handshake, node announcement.

### Merkle tree domain separation

Storage proofs hash leaves and internal nodes under different domain tags. A
tree that hashes both identically admits second-preimage attacks where an
internal node is presented as a leaf. There is a test for it.

## Anti-abuse

| Attack | Defence |
| --- | --- |
| Self-reported service work | Receipts must be signed by the served client, whose public key is inside the signed bytes. Self-traffic is detected and rejected. |
| Two nodes trading fake receipts | Concentration cap: at most 20% of a provider's credit may come from one counterparty. A ring of two collects a fifth of what it claims. `Validate` refuses to let governance disable this. |
| Receipt replay | Per-provider nonce tracking plus an expiry height. |
| Claiming storage without storing | Randomised challenges, with the chunk chosen by the previous block's app hash, which no single party can predict or steer. Failure accrues fraud score. |
| Sybil provider registration | A 1,000 HASH bond, slashable. |
| Sybil welcome claims | A signed eligibility attestation, an unused sequence, an unseen nonce, an expiry, and a per-attestor epoch cap. |
| Homograph username squatting | NFKC normalisation, Unicode-aware lowercasing, mixed-script rejection, and confusable folding to a skeleton. A name whose skeleton collides with an existing one is refused. |
| Free namespace exhaustion | A registration fee routed through `x/feerouter`, so a million names costs a million HASH. |
| Fork impersonation | Five-part network identity, a pinned genesis hash, and domain-separated signatures. |

## What is deliberately absent

Features whose absence is the security property:

- **No `x/mint`.** No inflation, and no code path that creates coins.
- **No `x/circuit`.** No authority can disable a message type. No kill switch.
- **No admin module.** No account can freeze a balance, reverse a transfer or
  seize funds. There is no such transaction to send.
- **No key escrow.** No Hashgram component holds a user's private key, which
  is why there is no way to reset an account for a user who lost theirs.
- **No plaintext logging.** Message content is never written to a log at any
  level in any build. Enforced by
  [`scripts/dev/check-logging.sh`](../scripts/dev/check-logging.sh), which
  self-tests its own patterns against deliberate violations so a broken check
  cannot pass silently. Policy in [LOGGING_POLICY.md](LOGGING_POLICY.md).

## Supply chain

The only defence a node operator has against running a tampered binary is
being able to build the published source and get the published bytes. If the
build is not reproducible, "verify the checksum" is advice nobody can act on.

```bash
scripts/dev/release.sh            # build with checksums
scripts/dev/release.sh --verify   # build twice, prove byte-identical
```

The release build sets `CGO_ENABLED=0`, `-trimpath` and `-buildvcs=false`, and
deliberately does **not** stamp the build date or git commit. Both are
tempting and both destroy reproducibility, which is the property that actually
protects an operator; the version and revision belong in the release notes.

`--verify` clears the build cache between the two builds, so the second one
genuinely recompiles rather than copying cached objects and matching
trivially.

Reproducibility depends on the Go toolchain version matching exactly, which is
why `go.mod` pins it, why `dist/BUILD_INFO` records it, and why CI fails if the
runner's toolchain disagrees with the pin.

### Dependency scanning

`scripts/dev/ci.sh vuln` runs `govulncheck` in binary mode against the built
`hashgramd`, which asks whether vulnerable code is in the artefact we would
ship rather than whether it is anywhere in the module graph.

Two findings are on a documented allowlist, each with the evidence:

- **GO-2024-2584**, slashing evasion in the Cosmos SDK, is a verified false
  positive. The advisory (ASA-2024-005) was patched in SDK 0.50.5 and 0.47.10;
  this tree is on v0.53.8. The Go vulnerability database entry carries a second
  affected range, "introduced 0.50.0", with no corresponding fixed event, so
  every release in that line matches it regardless of the patch.
- **GO-2026-5932**, `golang.org/x/crypto/openpgp` being unmaintained, has no
  fix: the advisory is "do not use this package". It is not imported by
  Hashgram. It reaches the binary through `cosmossdk.io/x/upgrade`, which
  depends on `hashicorp/go-getter` for its optional binary-download feature.
  Hashgram does not use automatic binary downloads; upgrades are applied by an
  operator installing a binary whose checksum they verified.

Everything else is fixed by upgrading rather than allowlisted. Ten
dependencies were upgraded to close thirteen symbol-level advisories.

## Pre-launch checks

```bash
hashgramctl mainnet-preflight
```

Refuses to pass, rather than warning, on:

- A devnet or test key present on a mainnet host
- A default or placeholder password
- The admin RPC listening on a public address
- `priv_validator_key.json` readable by group or others
- A chain id that does not match the pinned network
- A genesis hash that does not match the pin
- No Founder beneficiary configured
- Insufficient free disk
- An unsynchronised clock
- No firewall active

Each check states what is wrong and the command that fixes it. A check that
tells you it failed without telling you what to do is a check people learn to
skip.
