# Mainnet

The identity of Hashgram Mainnet, how its genesis is created, and how to join
it.

**Mainnet has not launched.** It requires a Founder address generated on a
machine that is not a server, which is a step nothing in this repository can
perform on your behalf. The full launch sequence is in
[FOUNDER_LAUNCH_RUNBOOK.md](FOUNDER_LAUNCH_RUNBOOK.md); this document covers
the mechanics.

## Network identity

Five parts. Four are compile-time constants; the fifth is computed at genesis
and then pinned.

| Part | Mainnet | Devnet |
| --- | --- | --- |
| Network name | `Hashgram Mainnet` | `Hashgram Devnet` |
| Network id | `hashgram-mainnet` | `hashgram-devnet` |
| Chain id | `hashgram-1` | `hashgram-devnet-1` |
| Network magic | 4 bytes, distinct per network | distinct |
| Genesis hash | computed at genesis | computed at genesis |
| Protocol major version | 1 | 1 |

The genesis hash is the only one that is the network's actual fingerprint. The
others are labels, and a fork can copy a label.

Signatures are domain-separated by network, so an attestation, receipt or
device certificate produced on devnet cannot be replayed on mainnet. This is
why the devnet acceptance suite includes a test that a signature made under
the devnet domain is rejected when presented against mainnet's.

## Creating genesis

Done once, by one person, before the network exists.

### What you need first

- The **Founder public address**, generated offline with `hashgram-keygen` on
  a machine that is not this server
- Gentxs from the initial validators, collected into the node's config
  directory
- A host that passes `hashgramctl mainnet-preflight`

### The command

```bash
hashgramctl init-mainnet-genesis --founder-address hash1...
```

It refuses to run without the address. There is no code path in the tooling
that generates one, which means there is no code path by which this server
could have seen the private key.

What it does:

1. Builds the complete genesis `AppState` deterministically
2. Creates the Founder account: 20,000,000 HASH spendable and 180,000,000 in a
   `PeriodicVestingAccount` over 96 monthly periods
3. Funds the useful-service reserve module account with 500,000,000 HASH
4. Funds the four treasury sub-accounts: treasury 150,000,000, growth
   50,000,000, dev grants 50,000,000, liquidity 50,000,000
5. Sets the Founder beneficiary and the 100 bps fee share
6. Writes the network identity into `x/network` genesis
7. Asserts total supply is exactly 1,000,000,000,000,000 uhash
8. Prints a summary and asks for confirmation
9. Computes the genesis hash, prints it, and pins it in the node's network
   configuration

Step 7 is not a formality. If the allocations do not sum to exactly the
canonical supply, the build fails rather than producing a genesis file whose
totals are wrong.

### Determinism

The same inputs produce a byte-identical genesis file, and therefore the same
hash. `genesis/mainnet_test.go` asserts it.

This matters because it is what lets a second person rebuild the genesis from
the same inputs and confirm the published hash independently. A genesis nobody
else can reproduce is a genesis everybody has to trust.

### Recording the hash

The hash is printed and pinned. **Publish it somewhere separate from the
genesis file itself.** A file distributed alongside its own hash proves
nothing: whoever tampered with one tampered with the other.

Signed release notes, a tagged commit, a social account, a third-party
archive — more than one channel, so an operator can cross-check.

## Joining

Since the 2026-09-10 launch the canonical genesis file, its SHA-256
(`e322bc2319f6e0173286fa526dab5a8ff8ad0797c7b80dd03e7c9d98621d5e4d`) and a
first-contact seed list are compiled into the binaries, in `app/params/mainnet/`.
This is the bitcoind model: the software you chose to run is the out-of-band
source for the genesis, and a fresh machine needs no arguments:

```bash
hashgramctl join-mainnet
```

You may still bring the genesis from elsewhere; it must hash to the built-in
value or the command refuses:

```bash
hashgramctl join-mainnet \
  --genesis-file ./genesis.json \
  --peers <nodeid>@<host>:26656            # extra persistent peers
  --p2p-peers /ip4/<host>/udp/26670/quic-v1/p2p/<peer-id>   # added to the built-in list
```

Discovery then has five layers, consulted in this order: the node's own
persisted peerstore/addrbook, the built-in seeds, `/dnsaddr` DNS seeds (none
published yet), anything passed by hand, and PEX/DHT gossip once the first
connection is up. A node that has ever been online never reads the built-in
lists again.

### Why --genesis-hash is required

Not optional convenience. Without it, joining a network means trusting whoever
handed you the file, and the whole point of a pinned hash is that you do not
have to.

The command:

- Refuses to proceed if the file's hash does not match
- Refuses if the chain id in the file does not match the network you named
- Refuses to overwrite an existing pin without `--force`, so a node already on
  one network cannot be silently moved to another
- Names the mismatch when it refuses, rather than failing vaguely

There is a test for the last point, because "GENESIS HASH MISMATCH" with the
expected and received values is actionable and "invalid input" is not.

### After joining

```bash
hashgramctl configure-role relay,store    # optional
hashgramctl start
hashgramctl status
```

## Verifying you are on the right network

```bash
hashgramctl network-info
```

Reports the network name, id, chain id and pinned genesis hash. Compare the
hash against your independent source.

This is worth doing after any restore, any config change, and any time the
height stops moving while peers look healthy. Two chains can share a chain id
and a TCP connection while being disjoint at consensus, and the symptom is
exactly that: peers connected, height frozen.

## What stops a fork, layer by layer

Tested by `scripts/testnet/four-validator.sh`, which tries both kinds of fork
and reports what actually happened rather than what was hoped for.

| Layer | What it checks | Result |
| --- | --- | --- |
| CometBFT handshake | Chain id only | A fork with a different chain id gets **zero peers** |
| CometBFT handshake | Genesis hash | **Not checked.** A same-chain-id fork opens a transport connection |
| Consensus | Validator set and app hash | The fork cannot inject a block or move the real chain |
| `hashgramctl join-mainnet` | Pinned genesis hash | Refuses the fork genesis |
| Hashgram P2P handshake | All five identity parts, genesis hash included | A same-chain-id fork is refused, banned and verifies nobody (`phase2.sh`) |

The second row is a real gap on the consensus transport, not a defence, and
it is why the pinned hash in `/etc/hashgram/network.json` and the
`genesis_hash` field in the Hashgram handshake exist rather than being
duplicated effort. Everything a client touches goes through the Hashgram
handshake; only validator-to-validator traffic goes through CometBFT's.

The test's own output states this. Verified results from a run:

```text
[ ok ] REJECTED at the handshake: the alien fork established 0 peers
[ ok ] as expected, the transport connected (2 peer(s)): chain id alone is not fork protection
[ ok ] the two chains never converge: app hash 8175B348... vs 5F7E9625...
[ ok ] all four real validators still share one app hash: the fork injected nothing
[ ok ] the real chain advanced 19 -> 26 while the fork was connected
[ ok ] the real validator set is still exactly 4: the fork's validator was never admitted
[ ok ] hashgramctl refused the fork genesis against the pinned hash (exit 1)
```

## Consensus parameters at genesis

| Parameter | Mainnet |
| --- | --- |
| Target block time | ~4 seconds |
| Max block bytes | 22 MB |
| Max gas | 200,000,000 |
| Unbonding period | 21 days |
| Max validators | 100 |
| Bond denomination | `uhash` |
| Slash for double signing | 5% |
| Slash for downtime | 0.01% |
| Downtime jail duration | 10 minutes |
| Signed blocks window | 10,000 |
| Min signed per window | 5% |
| Governance voting period | 14 days |
| Governance min deposit | 50,000 HASH |
| Expedited voting period | 24 hours |
| Expedited min deposit | 100,000 HASH |
| Quorum | 33.4% |
| Threshold | 50% |
| Veto threshold | 33.4% |

Devnet uses much shorter periods so that governance can be exercised in a test
run. Note that the SDK requires the expedited voting period to be strictly
shorter than the regular one and the expedited deposit to be strictly greater
— getting that backwards makes genesis validation fail, which is how the
four-validator script's parameter patch was caught during development.

Defined in [`genesis/params.go`](../genesis/params.go).

## Devnet

To exercise everything without a real Founder key:

```bash
scripts/testnet/devnet.sh
```

Builds a genesis with the real tooling using a devnet Founder key, starts a
chain, and runs 32 acceptance checks against it. Among them: supply exactly
1e15 uhash and unchanged; the Founder holding exactly 200,000,000 HASH with
20,000,000 spendable; a 100 HASH transfer delivering 100 HASH; the Founder
accruing exactly 1% of gas fees; the reserve holding exactly 500,000,000 HASH;
the welcome pool at exactly 1,850,000 HASH with no attestors; a confusable
username refused; and `mainnet-preflight` correctly refusing to pass on a
devnet host.

That last check matters: it proves the preflight gate works before it is the
only thing standing between a devnet key and a live network.

## The launch sequence

Summarised. Full detail, with a checklist, in
[FOUNDER_LAUNCH_RUNBOOK.md](FOUNDER_LAUNCH_RUNBOOK.md).

```text
A.  Generate the Founder cold wallet offline with hashgram-keygen
    Never on a server. Verify by comparing the derived address.
B.  Prepare hosts: bootstrap-ubuntu.sh, then mainnet-preflight until it passes
C.  Collect gentxs from the initial validators
D.  hashgramctl init-mainnet-genesis --founder-address hash1...
E.  Publish the genesis file and, separately, its hash
F.  Operators join with join-mainnet and the independently obtained hash
G.  Start, then verify: supply, Founder allocation, vesting, fee share
```

Step A is the one that cannot be delegated to this software, and step E is the
one most often done wrong.
