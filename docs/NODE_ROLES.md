# Node Roles

One machine can serve one role or several. `hashgramctl configure-role` sets
them, and `hashgramctl start` starts only the services those roles need.

```bash
hashgramctl configure-role validator
hashgramctl configure-role relay,store,media
hashgramctl node-info            # what this machine is configured for
```

## The eight roles

| Role | Service | Purpose | Earns |
| --- | --- | --- | --- |
| `validator` | `hashgramd` | Signs blocks | block fees and staking rewards |
| `relay` | `hashgram-node` | Forwards encrypted envelopes and gossip, serves circuit relay | client-signed relay receipts |
| `store` | `hashgram-node` | Holds mailboxes, key packages and blobs; answers storage challenges | storage assignments × challenges, retrieval receipts |
| `media` | `hashgram-node` | Serves media manifests and chunks | retrieval receipts |
| `indexer` | `hashgram-indexer` | PostgreSQL index of chain and public social data | nothing (an operator service) |
| `bootstrap` | `hashgram-node` | Helps new nodes find peers, serves circuit relay | relay receipts |
| `call` | `hashgram-node` + `coturn` (+ LiveKit) | TURN credentials, announcements, optional SFU | call receipts |
| `safety` | `hashgram-safety` | Reviews public content, signs verdicts | nothing (an operator service) |

Earning roles need a provider operator key and a bonded registration; see
`docs/SERVICE_REWARDS.md`. `hashgramctl configure-role` creates the key and
prints the address to fund.

A machine with **no** role still runs `hashgramd` as a full node: it follows
the chain, serves local queries and relays transactions. That is the default
after `hashgramctl init`, and it is the right starting point.

## validator

Signs blocks. The only role with slashing risk, and the one that needs the
most care.

**Requirements**

| | |
| --- | --- |
| CPU | 4 cores |
| RAM | 8 GB |
| Disk | 500 GB SSD, growing with chain history |
| Network | 100 Mbit, low jitter matters more than bandwidth |
| Uptime | As close to continuous as you can manage |

**What it needs to be safe**

- `priv_validator_key.json` at mode 0600. `mainnet-preflight` fails the launch
  if it is readable by group or others.
- Only one process holding the consensus key, ever. Two is a double-signing
  slash with no recovery.
- No public P2P listener for a validator with meaningful stake: peer with
  sentry nodes you control instead.
- A remote signer (`tmkms`) so the key is not on the internet-facing host at
  all. Cosmos SDK v0.53 was chosen partly for this support.
- The operator key, which stakes and changes commission, kept **off** the
  node. It is a separate key domain from the consensus key for exactly this
  reason.

**Sentry architecture.** Two or three sentry nodes with public P2P, and the
validator peering only with them over private addresses. An attacker who
cannot reach the validator cannot attack it directly, and the sentries absorb
inbound connection load.

```text
   internet ──▶ sentry 1 ──┐
   internet ──▶ sentry 2 ──┼──▶ validator (no public listener)
   internet ──▶ sentry 3 ──┘
```

**Monitor**

```bash
hashgramctl validator          # signing status and safety checks
```

The alerts that matter: `HashgramValidatorNotSigning`,
`HashgramByzantineValidators`, `HashgramMissingValidators`. Note that CometBFT
exports no missed-blocks counter, so signing health is measured as the gap
between the chain height and this validator's last signed height.

**Do not combine** with `safety`, which is a content-scanning workload with a
much larger attack surface, or with anything CPU-hungry: a validator that
misses precommits because a neighbour process is busy gets jailed.

## relay

Forwards encrypted envelopes between peers and holds them for offline
recipients.

**Requirements:** 2 cores, 4 GB RAM, 100 GB SSD, and bandwidth — this is the
role where bandwidth is the binding constraint. 1 Gbit if you want meaningful
earnings.

**Paid on** receipts signed by the clients it served, per GiB forwarded. A
relay that reports its own traffic earns nothing, and at most 20% of its
credit may come from any single counterparty.

**Sees:** who connected, when, and how much data moved. **Cannot see:**
message content, which is end-to-end encrypted. Metadata is still traffic
analysis material; see [THREAT_MODEL.md](THREAT_MODEL.md).

## store

Stores encrypted blobs and answers retrieval requests.

**Requirements:** 2 cores, 4 GB RAM, and as much disk as you intend to be
paid for. Disk reliability matters more than disk speed: a failing disk looks
exactly like fraud to the challenge system.

**Paid on** bytes the network **assigned** to it, held across epochs, scaled
by challenge success. Declared capacity is advertising: an operator who
declares a petabyte and is assigned nothing earns nothing.

**Challenges.** Roughly four per epoch per provider. The challenged chunk is
derived from the previous block's app hash, so it cannot be predicted. A
failed challenge adds 25 to the fraud score; 100 jails and slashes 5% of bond.

**Before registering,** verify your disks. Jailing costs bond, and the most
common cause of a failed challenge on an honest node is hardware.

## media

Serves media manifests and chunks for public content: images, video, reels.

Similar to `store` but read-heavy and latency-sensitive. Paid on client-signed
retrieval receipts, per GiB served. Combines naturally with `store`.

## indexer

Maintains a PostgreSQL index of public chain and social data, so clients can
run queries the chain cannot answer efficiently.

**Requirements:** 4 cores, 8 GB RAM, 200 GB SSD.

**Not consensus-critical.** If the database dies the chain is unaffected, and
the index can be rebuilt from chain state:

```bash
hashgramctl indexer rebuild
```

PostgreSQL listens on localhost only. The index holds public data that can be
regenerated, so it is not a confidentiality boundary — but it is a write
surface, and an exposed database is an exposed database.

## bootstrap

Answers "who else is on this network" for nodes that have just started.

**Requirements:** modest — 2 cores, 2 GB RAM. What it needs is a **stable
address**, because its value is entirely in being findable.

Run one if you want to reduce the network's dependence on bootstrap peers
chosen by whoever built the release. That dependence is listed as a
centralisation point in [DECENTRALIZATION.md](DECENTRALIZATION.md), and more
independent bootstrap operators is the fix.

## call

TURN relay for voice and video, so calls work between peers behind NAT.

**Requirements:** 2 cores, 4 GB RAM, and bandwidth. Every relayed call stream
crosses this machine twice.

**Ports:** 3478 and 5349 (TCP and UDP), plus 49152–65535 UDP for the relay
range. This is the only role that needs a wide port range open, which is worth
weighing before combining it with a validator.

Uses `coturn`, reconfigured with a Hashgram realm and fresh credentials.

**Cannot see** call media: it is end-to-end encrypted between participants.
It relays bytes it cannot read.

## safety

Scans **public** content — posts, channels, media — against configured
providers, and publishes signed attestations on chain.

**It never touches private messages.** This is enforced structurally, not by
policy: the systemd unit lists the chain and node data directories under
`InaccessiblePaths=`, so the process cannot read the envelope store even if
its code tried to.

**Requirements:** 4 cores, 8 GB RAM, and outbound access to whichever scanning
providers are configured.

**Do not combine with `validator`.** It has the largest attack surface of any
role — it processes untrusted content and talks to third-party services — and
a validator's consensus key should not share a host with that.

## Combining roles

**Sensible**

```text
relay + store + media          one storage and bandwidth node
store + indexer                storage plus queryability
validator alone                the correct answer for meaningful stake
bootstrap + relay              a stable, useful peer
```

**Avoid**

```text
validator + safety             largest attack surface next to the signing key
validator + call               a wide UDP range next to the signing key
validator + anything CPU-heavy missed precommits lead to jailing
```

## Per-role service users

Each role runs as its own unprivileged user with its own data directory, so
compromising one does not yield the others:

```text
hashgram-chain   /var/lib/hashgram/chain    validator, full node
hashgram-node    /var/lib/hashgram/node     relay, store, media, bootstrap
hashgram-index   /var/lib/hashgram/index    indexer
hashgram-safety  /var/lib/hashgram/safety   safety
hashgram-call    /var/lib/hashgram/call     call
```

No shells, no home directories, no sudo. Set up by
`scripts/install/bootstrap-ubuntu.sh` and confined by the unit files in
[`deploy/systemd/`](../deploy/systemd).

## Firewall by role

`ufw` defaults to deny inbound. Only what a role needs is opened:

| Role | Opened |
| --- | --- |
| Any chain node | 26656/tcp |
| relay, store, bootstrap | 26670/tcp and 26670/udp |
| call | 3478, 5349 (tcp+udp), 49152–65535/udp |
| indexer | nothing; PostgreSQL is localhost only |
| safety | nothing inbound |

**Never opened:** 26657 (RPC), 1317 (REST), 9090 (gRPC), 26660 (metrics), 5432
(PostgreSQL), 9090/3000 (Prometheus, Grafana). Reach them over SSH:

```bash
ssh -N -L 26657:127.0.0.1:26657 -L 3000:127.0.0.1:3000 operator@node
```

`mainnet-preflight` fails if the admin RPC is publicly reachable.

## Changing roles

```bash
hashgramctl configure-role relay,store
hashgramctl restart
```

Adding a role starts the services it needs. Removing one stops them and leaves
the data in place, so a role can be re-enabled without resyncing.

Removing `store` while you hold storage assignments does **not** release them.
Release them first, or you will fail the challenges that keep arriving and
accrue fraud score on a node that is no longer trying to serve:

```bash
hashgramctl storage             # what is assigned
# release assignments, wait for the epoch to settle, then remove the role
```

## Limitations

- A role earns only after the operator key is funded and the provider is
  registered; `hashgramctl rewards` shows both. Storage credit also needs a
  registered assigner on chain (`docs/SERVICE_REWARDS.md`), which Mainnet
  genesis does not include.
- `call` with an SFU encrypts media to the SFU, not through it. 1:1 calls are
  peer-to-peer. Call receipts are not yet produced by the reference client,
  so the `call` role earns nothing today beyond relay traffic.
- `safety` implements the hash, text and HTTP-model stages; OCR and video
  frame extraction are hook points, not built.
- Roles have been run together and apart on one host. Behaviour across real
  NATs and separate operators has not yet been observed in production.
