# Disaster Recovery

What to do when something is lost, broken or stolen.

Ordered by how bad it is. The first section is the one to read before anything
goes wrong, because two of the scenarios below have no recovery and the only
defence is preparation.

## What cannot be recovered

Be clear about this first, so the rest is read in the right light.

| Lost | Recovery |
| --- | --- |
| Founder mnemonic, with no backup | **None.** 200,000,000 HASH is unreachable forever. |
| A user's device with no other device and no guardians | **None.** No key escrow exists. |
| Stake slashed for double signing | **None.** The slash is consensus. |

Everything else on this page is recoverable.

## Before anything goes wrong

### Back up the keys, once, by hand

`hashgramctl backup` deliberately **excludes** private keys. That is not an
oversight: a consensus key inside an archive that gets copied to a second
machine is a double-signing accident waiting to be triggered by a restore.

So the keys are backed up separately, once, deliberately:

```bash
# On the validator, as root.
cp /var/lib/hashgram/chain/config/priv_validator_key.json  /secure/offline/
cp /var/lib/hashgram/chain/config/node_key.json            /secure/offline/
```

Store them encrypted, offline, in two physical locations. Not on the server.
Not in the same place as each other if you can avoid it.

**The Founder mnemonic** is written on paper, stored in two separate physical
locations, and never photographed, never typed into a computer that has ever
been online, and never stored in a password manager that syncs.

Verify a written backup by deriving the address from it and comparing:

```bash
./hashgram-keygen derive     # on an offline machine
```

Compare the **derived address** against the one you recorded. Do not rely on
the mnemonic merely being accepted: BIP-39's checksum catches most single-word
errors and typos but does **not** reliably catch two words being swapped,
because a swap can produce a valid checksum. An earlier version of this tool's
help text overstated that, and a swapped-word test produced a silently valid,
wrong address.

### Back up configuration regularly

```bash
hashgramctl backup
```

Archives configuration and light state. Fast, small, and safe to automate:

```bash
# /etc/cron.d/hashgram-backup
0 3 * * * root /usr/local/bin/hashgramctl backup >/dev/null 2>&1
```

Chain data is excluded by default because it is large and because it can be
resynced from the network. Include it only if resync time matters more than
archive size:

```bash
hashgramctl backup --include-chain-db
```

### Write down the genesis hash

```bash
hashgramctl network-info
```

Record the pinned genesis hash somewhere separate from the node. After any
restore, this is how you confirm you came back on the right network rather
than a fork.

## Scenario: the node will not start

```bash
hashgramctl logs | tail -50
systemctl status hashgramd
```

**Common causes, in the order they occur in practice**

*Disk full.* Prune or grow the volume. Do not delete files under
`/var/lib/hashgram` by hand.

*Corrupted database.* The chain database can be rebuilt from the network:

```bash
hashgramctl stop
mv /var/lib/hashgram/chain/data /var/lib/hashgram/chain/data.broken
mkdir -p /var/lib/hashgram/chain/data
cp /var/lib/hashgram/chain/data.broken/priv_validator_state.json \
   /var/lib/hashgram/chain/data/
hashgramctl start
```

Copying `priv_validator_state.json` forward is not optional on a validator.
Losing it means the node does not know the last height it signed, and it may
sign a height it has already signed. That is a double-signing slash.

If the file is genuinely gone, do **not** start the validator. Wait past the
current unbonding-relevant window, or start as a non-validating full node
until you are certain.

*Wrong genesis.* Compare `hashgramctl network-info` against your recorded
hash.

## Scenario: peers connect but the height does not move

You are on a different chain from the network. Two chains can share a chain id
and a TCP connection while being disjoint at consensus, and this is exactly
what that looks like.

```bash
hashgramctl network-info      # compare the pinned hash against your record
```

If it differs, rejoin with the correct hash:

```bash
hashgramctl stop
hashgramctl join-mainnet --genesis-url <url> --genesis-hash <correct> --force
hashgramctl start
```

`--force` is required to overwrite an existing pin, so that a node cannot be
silently moved between networks.

## Scenario: the validator is missing blocks

```bash
hashgramctl validator
```

Check, in order: is the remote signer reachable; is the consensus key where
the node expects it; is the node synced; are CPU and disk saturated.

The `Host and Database` dashboard next to
`cometbft_state_block_processing_time` answers the last question directly. A
validator that misses precommits because a neighbour process is busy gets
jailed for something that is not a Hashgram problem.

Missing blocks accrues downtime slashing (0.01%) and jailing after falling
below 5% signed in a 10,000-block window. Unjail once healthy:

```bash
hashgramd tx slashing unjail --from <operator-key>
```

Note that the operator key should not be on the node. Sign that transaction
from wherever the operator key actually lives.

## Scenario: the validator was slashed for double signing

**Stop the node immediately.** Every additional block makes it worse.

```bash
hashgramctl stop
```

The slash (5%) and the jailing cannot be reversed. What can still be done is
finding the second process, because if it is still running you are still
double signing:

- Another node with the same `priv_validator_key.json`
- A restored snapshot started while the original was running
- A container orchestrator that restarted the node elsewhere
- A remote signer serving two nodes

Then: generate a **new** consensus key, and never reuse the old one anywhere.

`priv_validator_state.json` prevents a single node from double signing. It
cannot protect against two nodes with the same key, because neither can see
the other's state file. That is why the operational rule is absolute: one
process, one consensus key, ever.

## Scenario: a node was compromised

Assume everything on that host is known to the attacker.

### If it was a validator

```bash
hashgramctl stop
```

1. **Assume the consensus key is stolen.** Generate a new one and migrate.
2. **Do not restore the old key on a new host.** The attacker may be using it,
   and two signers is a slash.
3. Rebuild the host from scratch. Do not clean it in place; you cannot prove
   you found everything.
4. Rotate SSH keys, and anything else that machine could reach.
5. Check whether the operator key was ever on that host. If it was, move the
   stake with a key that was not.

### If it was a relay, store or media node

Less severe, because the key domains are separate:

- The node's P2P key is stolen. Generate a new one; the node's identity
  changes, which costs reputation and nothing else.
- The provider's reward address is **not** stolen unless you put its key on
  the node, which is why `x/serviceproof` separates operator from reward
  address in the first place.
- Encrypted blobs the node held are exposed, but they are encrypted with keys
  the node never had.

```bash
hashgramctl stop
# rebuild the host, then:
hashgramctl init --reset-node-key
hashgramctl configure-role relay,store
hashgramctl start
```

### What a host attacker does not get

Because of the key domain separation in
[SECURITY.md](SECURITY.md#key-domains): no user private keys, no message
plaintext, no other node's keys, no Founder key, and no ability to forge
blocks the rest of the network accepts.

## Scenario: the whole server is lost

Provider failure, seizure, or an unrecoverable host.

```bash
# On a new machine:
sudo ./scripts/install/bootstrap-ubuntu.sh
hashgramctl restore /path/to/backup.tar.gz
```

Then, and this is the part people skip:

```bash
hashgramctl network-info      # confirm the genesis hash matches your record
```

If the old machine was a validator, **do not** restore the old consensus key
unless you are certain the old machine is permanently gone. "Certain" means
the disk is destroyed or the provider has confirmed termination, not "it seems
to be down". If in any doubt, generate a new consensus key: a jailed validator
recovers, a slashed one does not.

## Scenario: PostgreSQL is lost

The least serious scenario on this page. The index holds public data derived
from the chain and is not consensus-critical:

```bash
sudo -u postgres createdb hashgram_index
hashgramctl indexer rebuild
```

The chain is unaffected while the database is down.

## Scenario: the Founder key is lost

If a backup exists, restore from it on an offline machine and verify by
comparing the derived address.

If no backup exists, the 200,000,000 HASH is unreachable forever. There is no
recovery mechanism, no administrative reset, and no code path that could
create one — which is the same property that means nobody else can seize it.

The revenue share continues to accrue to whatever beneficiary address is
configured. If that address is also unreachable, governance can change the
beneficiary; the accrued balance in the module account is not lost, only
undeliverable until then.

## Scenario: a user lost their device

This is `x/identity`'s job and it has to have been set up in advance.

**With another authorised device:** the remaining device authorises a
replacement and revokes the lost one. No recovery process needed.

**With guardians configured:** the threshold of guardians approves recovery,
a mandatory delay elapses, and then a new root key takes over. The delay
exists so that a user whose guardians are being socially engineered has time
to cancel.

**With neither:** the account is unreachable. Permanently. There is no key
escrow, which is precisely why no server can be compelled to hand over a
user's account.

Configure guardians before you need them:

```bash
hashgramd tx identity set-recovery-config \
  --guardians <addr1>,<addr2>,<addr3> \
  --threshold 2 \
  --from <root-key>
```

## Recovery drill

Untested backups are not backups. Run this quarterly on a spare machine, not
on the production host:

```bash
hashgramctl backup
# on a clean machine:
sudo ./scripts/install/bootstrap-ubuntu.sh
hashgramctl restore /path/to/backup.tar.gz
hashgramctl network-info        # does the pinned hash match your record?
hashgramctl start
hashgramctl status              # does it sync?
```

If any step surprises you, that is the point of the drill, and it is much
better to be surprised now.
