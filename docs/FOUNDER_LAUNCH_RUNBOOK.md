# Founder Launch Runbook

The complete sequence for launching Hashgram Mainnet, in order, with the
commands.

Read the whole thing before starting Part A. Several steps are irreversible,
and one of them — creating the Founder key — determines control of 20% of the
supply forever.

## A note on your seed phrase

**Nothing in this repository will ever ask for your seed phrase, and neither
will anyone legitimately helping you.**

`hashgram-keygen` prints a mnemonic once, to your terminal, on a machine you
control that is not connected to a network. Nothing else ever needs to see it:
not this server, not the genesis tooling, not a support channel, not a
document. The only thing that leaves the offline machine is a public address
beginning `hash1`.

If any tool, script, person or web page asks you to type or paste your seed
phrase, that is an attack. There is no exception to this and no situation in
which it is a reasonable request.

## Overview

```mermaid
flowchart TD
    A["Part A: Founder cold wallet<br/>OFFLINE machine, never a server"] --> B["Part B: Prepare hosts<br/>bootstrap + preflight"]
    B --> C["Part C: Preliminary genesis<br/>public address + launch accounts"]
    C --> D["Part D: Validator gentxs<br/>and finalize-genesis"]
    D --> E["Part E: Publish genesis and,<br/>separately, its hash"]
    E --> F["Part F: Operators join"]
    F --> G["Part G: Start and verify"]
    G --> H["Part H: First governance<br/>register the storage assigner"]
    H --> I["Part I: Network services<br/>hashgram-node roles, indexer"]
```

Estimated time: Part A takes an hour done carefully. Parts B through D take a
day. Part F depends on your operators. Do not compress Part A.

---

## Part A — Create the Founder cold wallet

**Where:** a machine that has never been a server and is not connected to a
network while you do this. A laptop with Wi-Fi switched off is acceptable. A
VPS is not, under any circumstances.

**Why offline:** this key controls 200,000,000 HASH. A key generated on a
server has been, however briefly, on a machine reachable from the internet and
running software you did not audit. There is no way to un-know that.

### A.1 Get the binary onto the offline machine

Build it on a connected machine, verify it, and carry it across on a USB
stick:

```bash
make build
sha256sum build/hashgram-keygen
```

Record that checksum. On the offline machine, verify it matches before
running anything.

### A.2 Confirm the machine is offline

```bash
ip link set wlan0 down     # or unplug the cable
ping -c 1 8.8.8.8          # must fail
```

`hashgram-keygen` contains no networking code and writes nothing to disk. That
is a property of the binary and it is not a reason to skip this step: defence
in depth means the tool being safe *and* the machine being offline.

### A.3 Generate

```bash
./hashgram-keygen new
```

It prints a warning, then a mnemonic, then the derived `hash1...` address.

### A.4 Write the mnemonic on paper

By hand. On paper. Twice.

**Never:** photograph it, type it into any computer, store it in a password
manager that syncs, put it in a note-taking app, email it to yourself, or read
it aloud near a phone.

Store the two copies in two separate physical locations. Consider a metal
backup plate if the amount matters to you, because paper burns and gets wet.

### A.5 Verify the backup by deriving the address

This is the step that catches a transcription error while it is still fixable:

```bash
./hashgram-keygen derive
```

Type the mnemonic from your **written copy**, not from the screen. Compare the
address it derives against the one printed in A.3.

**They must match exactly.** All of it, not the first and last few characters.

Be precise about what BIP-39's checksum does for you here: it catches most
single-word errors and most typos. It does **not** reliably catch two words
being swapped, because a swap can still produce a valid checksum. So the
mnemonic being *accepted* is not the verification. The **derived address
matching** is the verification.

### A.6 Record the public address

```text
Founder public address: hash1________________________________
```

Write it down separately from the mnemonic. You will type it on the server in
Part D, and it is the only thing from this part that goes anywhere near a
server.

### A.7 Consider a hardware wallet instead

Hashgram uses BIP-44 coin type 118, which is Cosmos's, so a Ledger works with
standard Cosmos tooling. If you have one, using it is better than a paper
mnemonic: the key is generated on the device and never exists in a form that
can be transcribed wrongly or photographed.

You still record the address. You still write down the device's own recovery
words, with the same discipline as A.4.

### Part A checklist

- [ ] Machine has never been a server and is offline now
- [ ] Binary checksum verified against the one recorded on the build machine
- [ ] Mnemonic written on paper, by hand, twice
- [ ] Two copies in two separate physical locations
- [ ] Mnemonic never photographed, typed, or stored digitally
- [ ] Backup verified by comparing the **derived address**, not just acceptance
- [ ] Public address recorded separately
- [ ] Offline machine's disk wiped, or the machine kept permanently offline

---

## Part B — Prepare the hosts

**Where:** each server that will run a node.

### B.1 Install

```bash
sudo ./scripts/install/bootstrap-ubuntu.sh
```

Creates the five per-role service users, sets up directories, installs
binaries and hardened systemd units, configures PostgreSQL for localhost only,
and sets a default-deny firewall.

The firewall step detects the port `sshd` is actually listening on rather than
assuming 22. Verify you can still open a second SSH session before closing the
first one.

### B.2 Initialise the node

```bash
hashgramctl init --moniker <name>
```

### B.3 Monitoring

```bash
sudo ./scripts/install/monitoring.sh
```

Note what it reports. On a stock Ubuntu host it will rebind Prometheus,
node-exporter and the Postgres exporter to localhost, because the Debian
packages bind them to every interface — which publishes an operator's
reconnaissance feed to the internet as a side effect of installing a
dashboard.

### B.4 Harden the validator, if this host is one

In increasing order of protection, and for a validator with meaningful stake
you want at least a remote signer:

1. Confirm `priv_validator_key.json` is mode 0600
2. Set up sentry nodes so the validator has no public P2P listener
3. Move the key to `tmkms` on a separate machine
4. Back `tmkms` with an HSM

Keep the **operator key off the node entirely**. It stakes and changes
commission; the consensus key signs blocks. Separate keys, separate machines.

### B.5 Preflight until it passes

```bash
hashgramctl mainnet-preflight
```

It refuses to pass on a devnet key, a default password, a publicly reachable
admin RPC, a validator key readable by group or others, a mismatched chain id
or genesis hash, no Founder beneficiary, insufficient disk, an unsynchronised
clock, or no firewall.

Every failure names the command that fixes it. **Do not proceed until it
passes.** This gate exists so that a devnet key cannot reach a live network,
and the devnet acceptance suite verifies the gate works before it is the only
thing standing between the two.

### Part B checklist

- [ ] bootstrap-ubuntu.sh run on every host
- [ ] Second SSH session confirmed working after the firewall step
- [ ] Node initialised
- [ ] Monitoring installed; nothing listening on a public address
- [ ] Validator key at 0600, remote signer configured if stake is meaningful
- [ ] Operator key is **not** on the node
- [ ] `mainnet-preflight` passes on every host

---

## Part C — Create the preliminary genesis

**Where:** the genesis machine. The address is the **public** one from A.6.

Genesis allocates the whole 1,000,000,000 HASH, so a validator has nothing
to bond from unless genesis gives it a balance. Those balances come out of
the Founder's unlocked 20,000,000 HASH — the Founder pays for the launch
validators from the Founder's own spendable money, the treasury is untouched,
and the total stays exactly 1,000,000,000.

### C.1 Each launch validator sends you an operator address

On each validator host, a **hot** operator key in `hashgramd`'s keyring:

```bash
hashgramd keys add operator            # or --keyring-backend file with a passphrase
hashgramd keys show operator -a        # hash1... — send this address to the genesis machine
```

Never the Founder cold key, never a key that exists on two machines.

### C.2 Create the preliminary genesis

```bash
hashgramctl init-mainnet-genesis \
  --founder-address hash1<founder-public-address> \
  --genesis-account hash1<validator-1-operator>=1000000HASH \
  --genesis-account hash1<validator-2-operator>=1000000HASH
```

Amounts take `HASH` or `uhash`. Give each validator its stake plus fee
change; what a validator does not bond stays spendable in its operator
account. The sum of all launch accounts must not exceed 20,000,000 HASH.

It prints the full distribution and asks for confirmation. Check:

```text
founder          199,000,000 - N  HASH   (180,000,000 vests; the rest is spendable)
launch-1           1,000,000 HASH
launch-2           1,000,000 HASH
serviceproof     500,000,000 HASH
welcome            1,850,000 HASH
treasury         150,000,000 HASH
dev_grants        50,000,000 HASH
liquidity         50,000,000 HASH
growth            48,150,000 HASH
TOTAL          1,000,000,000 HASH
```

And that the Founder address shown is character-for-character the one you
recorded. This is the last moment at which a wrong address is fixable.

The hash it prints is marked **PRELIMINARY**. It is not the network's
identity, because the file has no validators yet; Part D changes it.

### C.3 Distribute the preliminary file

Send `genesis.json` to every launch validator. Its preliminary hash lets them
confirm they received the same file:

```bash
sha256sum /var/lib/hashgram/chain/config/genesis.json
```

### On the initial validator set

This is a decision you make before the network exists, and it is listed as a
centralisation point in [DECENTRALIZATION.md](DECENTRALIZATION.md) because it
is one.

Aim for as many independent operators, in as many jurisdictions, with as even
a stake distribution as you can get. The network becomes permissionless
immediately after genesis — anyone can bond and join — but the starting set is
yours, and a starting set of one is a starting set of one.

### Part C checklist

- [ ] Founder address matches A.6 exactly, character for character
- [ ] Every launch validator's operator address is a hot key on its own host
- [ ] Distribution summary reviewed; total exactly 1,000,000,000 HASH
- [ ] Preliminary genesis distributed; every validator confirms its sha256
- [ ] Operators are genuinely independent, not one person with several servers
- [ ] No single operator holds a decisive share of the initial stake

---

## Part D — Validator gentxs and the final genesis

**Where:** each validator host, then the genesis machine. Once. Irreversibly.

### D.1 Each validator creates its gentx

Against the preliminary `genesis.json` from C.3, placed at
`<node-home>/config/genesis.json`, with the funded operator key:

```bash
hashgramd genesis gentx operator 900000000000uhash \
  --chain-id hashgram-1 \
  --moniker <name> \
  --commission-rate 0.10 --commission-max-rate 0.20 --commission-max-change-rate 0.01 \
  --ip <public-ip>
# writes config/gentx/gentx-<node-id>.json
```

The amount must be less than the launch account balance, leaving fee change.
Send the gentx file to the genesis machine.

### D.2 Finalise on the genesis machine

```bash
cp received-gentxs/*.json /var/lib/hashgram/chain/config/gentx/
hashgramctl finalize-genesis
```

It runs `collect-gentxs` and `validate-genesis`, refuses a genesis with no
validators, computes the hash over the final bytes and pins it in
`/etc/hashgram/network.json` and `app.toml`. It refuses to run twice without
`--force`, because finalising again would change the network's identity.

### D.3 Record the genesis hash

```text
GENESIS HASH  ________________________________________________________________
```

Write it down. Now, before doing anything else. Confirm it independently:

```bash
sha256sum /var/lib/hashgram/chain/config/genesis.json
hashgramctl network-info
```

### D.4 Confirm determinism, ideally with a second person

The same inputs (Founder address, launch accounts, genesis time, gentxs)
produce a byte-identical file. Have someone else rebuild it from the same
inputs and confirm they get the same hash. A genesis nobody else can
reproduce is a genesis everybody has to trust, and this is the cheapest
possible moment to remove that requirement.

Tested end to end on this tooling: preliminary genesis with one launch
account, gentx, `finalize-genesis`, `mainnet-preflight` (every check except
disk space on the development host), `hashgramd start` producing blocks on
`hashgram-1` with supply exactly 1,000,000,000,000,000 uhash and the Founder
holding exactly 200,000,000 minus the launch funding.

### Part D checklist

- [ ] A gentx from every launch validator, each with chain id `hashgram-1`
- [ ] `finalize-genesis` reported the expected validator count
- [ ] Genesis hash written down off the machine
- [ ] `network-info` and `sha256sum` agree with it
- [ ] Hash independently reproduced by a second person from the same inputs

---

## Part E — Publish

### E.1 Publish the genesis file

Anywhere operators can fetch it over HTTPS.

### E.2 Publish the hash **separately**

This is the step most often done wrong, so it is worth being explicit about
why it matters.

**A genesis file distributed alongside its own hash proves nothing.** Whoever
tampered with the file tampered with the hash sitting next to it. The hash has
to reach operators through a channel an attacker would have to compromise
separately.

Use more than one:

- Signed release notes on a tagged commit
- A published announcement on an account with history
- A third-party archive
- Directly, to each initial operator, through a channel you already share

Then any operator can cross-check two independent sources.

### Part E checklist

- [ ] Genesis file published
- [ ] Hash published through at least two channels **independent of the file**
- [ ] Each initial operator has the hash from a source they trust
- [ ] Instructions state that `--genesis-hash` is required

---

## Part F — Operators join

**Where:** every host other than the genesis machine.

```bash
hashgramctl join-mainnet \
  --genesis-url https://... \
  --genesis-hash <the hash from an independent source> \
  --peers <nodeid>@<host>:26656

hashgramctl configure-role relay,store    # optional
hashgramctl mainnet-preflight
hashgramctl start
```

`--genesis-hash` is required, and the command refuses any genesis that does
not match it. It also refuses to overwrite an existing pin without `--force`,
so a node already on one network cannot be silently moved to another.

### F.1 Every operator verifies the network they are on

```bash
hashgramctl network-info
```

Compare the pinned hash against the independently obtained one. Doing this
after joining, rather than trusting that the join worked, is the difference
between joining Hashgram and joining a fork with the same chain id.

### Part F checklist

- [ ] Every operator used `--genesis-hash` from an independent source
- [ ] Every operator ran `network-info` and confirmed the hash
- [ ] Every operator's `mainnet-preflight` passed
- [ ] Services started

---

## Part G — Start and verify

### G.1 Blocks are being produced

```bash
hashgramctl chain-status
hashgramctl status
```

### G.2 Supply is exactly right

```bash
hashgramctl chain-status
```

Total supply must be exactly `1000000000000000 uhash`. Not approximately.

```bash
hashgramd query bank total --denom uhash
```

### G.3 The Founder allocation is exactly right

```bash
hashgramctl wallet-info hash1...
```

- Total balance: 200,000,000 HASH minus the launch funding from C.2
- Spendable now: 20,000,000 HASH minus the launch funding
- Vesting: 180,000,000 HASH across 96 monthly periods, unchanged

With a single 1,000,000 HASH launch account that reads 199,000,000 total,
19,000,000 spendable, 180,000,000 vesting. If the spendable figure equals the
total, the vesting account was not created and something went wrong in
Part C. Stop and investigate before anyone transacts.

### G.4 The Founder revenue share is 1% and applies to fees only

```bash
hashgram-test-client founder verify --node tcp://127.0.0.1:26657
```

Reports the configured basis points (must be 100) and the realised share as a
fraction of collected revenue.

Then prove the transfer case directly. Send an amount between two accounts and
confirm the recipient receives all of it:

```bash
hashgram-test-client wallet send --from <a> --to <b> --amount 100
hashgram-test-client wallet balance <b>
```

100 HASH sent must be 100 HASH received.

### G.5 There is no mint module

```bash
hashgramd query --help | grep -c mint     # expect 0
```

And watch the metric:

```text
hashgram_supply_over_ceiling_uhash    must be 0, forever
```

### G.6 Monitoring is live

```bash
ssh -N -L 9090:127.0.0.1:9090 -L 3000:127.0.0.1:3000 operator@node
```

Confirm all three dashboards render with data, and that the alert rules
loaded:

```bash
scripts/dev/check-dashboards.sh --live
```

### G.7 A fork cannot join

Already tested by `scripts/testnet/four-validator.sh`, but worth
understanding for mainnet: a fork with a different chain id gets zero peers; a
fork sharing `hashgram-1` opens a transport connection and still cannot join
consensus or move the chain; and the layer that protects a person is the
pinned genesis hash in `join-mainnet`.

### Part G checklist

- [ ] Blocks producing steadily
- [ ] Total supply exactly 1,000,000,000,000,000 uhash
- [ ] Founder holds exactly 200,000,000 HASH minus the launch funding you chose
- [ ] Exactly 180,000,000 HASH vesting over 8 years; the remainder spendable
- [ ] Founder fee share is exactly 100 bps
- [ ] A transfer is untaxed: 100 sent is 100 received
- [ ] No mint module; `supply_over_ceiling` is zero
- [ ] All three dashboards render with data
- [ ] Alerts loaded, none firing
- [ ] Every operator confirmed their pinned genesis hash

---

## Part H — The first governance proposal

**Where:** any host with a funded key. Voting takes 7 days on Mainnet.

Genesis deliberately registers **no storage assigner** and **no welcome
attestor**, so nothing in the useful-service or welcome programmes can pay
anyone until governance says so. Storage rewards need one assigner; without
it every store node earns exactly zero for the bytes it holds
(`docs/SERVICE_REWARDS.md`).

### H.1 Decide who assigns

The assigner is a provider operator address — the key
`hashgramctl configure-role store` creates on a store node. On a launch where
you run the first store node, that is your own node's operator address:

```bash
hashgramctl rewards          # prints the operator address on a configured node
```

### H.2 Write the proposal

```bash
hashgramctl propose add-assigner hash1<operator> --out proposal-add-assigner.json
```

This reads the live `x/serviceproof` parameters and adds one address. Do not
write the file by hand: `MsgUpdateParams` replaces the whole parameter set,
and a hand-written file that names only `assigners` silently zeroes every
other parameter.

### H.3 Submit and vote

The deposit is 10,000 HASH, returned when the proposal passes. The Founder's
20,000,000 unlocked HASH can fund it, but the key that submits must be a hot
key on a connected machine — **never the Founder cold key.** Send the deposit
plus fees from the cold wallet to a hot key first.

```bash
hashgramd tx gov submit-proposal proposal-add-assigner.json \
  --from <hot-key> --chain-id hashgram-1 --gas auto --gas-adjustment 1.4 --fees 5000uhash
hashgramd query gov proposals --output json          # note the id
hashgramd tx gov vote <id> yes --from <validator-operator-key> --chain-id hashgram-1 --fees 5000uhash
```

Voting power is stake: each validator votes with its own operator key. Quorum
is 40% of bonded stake and the period is 7 days.

### H.4 Confirm

```bash
hashgramd query gov proposal <id> --output json | grep status      # PROPOSAL_STATUS_PASSED
hashgramd query serviceproof params --output json | grep -A2 assigners
```

Tested on a devnet with a 2-minute voting period: the proposal passed, the
assigner set gained the address, and every other parameter kept its value.

### Part H checklist

- [ ] Assigner address is a store node's operator key, not the Founder key
- [ ] Proposal file written by `hashgramctl propose`, not by hand
- [ ] Submitted from a hot key funded from the cold wallet
- [ ] Every validator voted
- [ ] `assigners` on chain lists the address; other parameters unchanged

The welcome programme stays paused until a welcome attestation service
exists and its key is registered the same way (`x/welcome` `MsgUpdateParams`
with an `attestors` entry). Creating a key earns nothing until then; say so
in the launch announcement.

---

## Part I — Network services

**Where:** every host that serves users. The chain alone is a ledger; the
messaging, social, media and call services are `hashgram-node`, installed
by `bootstrap-ubuntu.sh` and configured by role.

```bash
hashgramctl configure-role relay,store,media,bootstrap    # one node that does everything
hashgramctl restart
hashgramctl status              # P2P section: peers, roles, storage, rewards
hashgramctl health              # hashgramd, hashgram-node, coturn, indexer, safety
```

`configure-role` writes `/etc/hashgram/node.toml`, creates the operator key
and prints the address to fund with the provider bond (1,000 HASH) plus fee
change. The node registers itself as a provider on first start with a
funded key and answers storage challenges from then on.

For a second machine that should run the indexer and safety engine:

```bash
hashgramctl configure-role indexer,safety
hashgramctl restart
curl -s 127.0.0.1:1318/v1/status
```

`docs/NODE_ROLES.md` covers which roles to keep apart; the one rule to
remember is that the validator's host runs nothing else.

Publish your node's bootstrap multiaddr with the genesis hash so clients can
configure it:

```bash
hashgramctl status              # "Peer id" and "External addrs" under the P2P section
# the bootstrap multiaddr is <external-addr>/p2p/<peer-id>, for example
#   /ip4/203.0.113.10/udp/26670/quic-v1/p2p/12D3KooW...
```

### Part I checklist

- [ ] At least one node with `relay,store,bootstrap` reachable on 26670/udp and tcp
- [ ] Operator key funded; `hashgramctl rewards` shows the provider registered
- [ ] Bootstrap multiaddr and genesis hash published together with the genesis file
- [ ] `hashgram-client` on a laptop can `configure`, `identity create`, and `message send` to itself

---

## Final checklist

Everything in one place. All of it before announcing the network.

**Founder key**
- [ ] Generated on an offline machine that was never a server
- [ ] Mnemonic on paper, by hand, two copies, two locations
- [ ] Never photographed, typed into a connected machine, or stored digitally
- [ ] Backup verified by comparing the derived address
- [ ] Only the public address ever touched a server

**Hosts**
- [ ] bootstrap-ubuntu.sh on every host
- [ ] `mainnet-preflight` passes everywhere
- [ ] Nothing on a public address except the P2P port and SSH
- [ ] Validator key 0600, remote signer if stake is meaningful
- [ ] Operator key not on the node
- [ ] Monitoring installed and reachable only over SSH

**Genesis**
- [ ] Founder address verified character for character
- [ ] Launch accounts are validator hot keys, funded from the Founder's unlocked portion only
- [ ] Total exactly 1,000,000,000 HASH
- [ ] `finalize-genesis` run once; final hash written down
- [ ] Hash independently reproduced by a second person

**Publication**
- [ ] Genesis file published
- [ ] Hash published through at least two channels independent of the file

**Verification**
- [ ] Supply exactly 1e15 uhash
- [ ] Founder allocation and vesting correct
- [ ] Founder share 100 bps, transfers untaxed
- [ ] No mint module
- [ ] Dashboards and alerts working

**After launch**
- [ ] Storage assigner registered through governance (Part H)
- [ ] At least one `hashgram-node` serving relay, store and bootstrap (Part I)
- [ ] Bootstrap multiaddr published next to the genesis hash

**Recovery**
- [ ] Consensus and node keys backed up separately from `hashgramctl backup`
- [ ] Configuration backup automated
- [ ] Genesis hash recorded off the machine
- [ ] A restore drill completed on a spare machine

**Honesty**
- [ ] [DECENTRALIZATION.md](DECENTRALIZATION.md) section 3 read and understood
- [ ] [THREAT_MODEL.md](THREAT_MODEL.md) section 3 read and understood
- [ ] The launch announcement does not claim more than the deployment supports

---

## If something goes wrong during launch

**Before any operator joins:** you can discard the genesis and start Part D
again. Nothing is committed until blocks are produced and other people are
on the chain.

**After operators join but before real value moves:** coordinate a restart
with a corrected genesis. Awkward and embarrassing; entirely survivable.

**After the network is live:** you cannot change genesis. Everything becomes a
governance proposal or a coordinated upgrade. This is why Part D has a
confirmation prompt and why Part G verifies rather than assumes.

For anything after launch, see
[DISASTER_RECOVERY.md](DISASTER_RECOVERY.md).
