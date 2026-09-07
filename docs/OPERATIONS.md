# Operations

Running a Hashgram node day to day.

Every command here exists. The list came from `hashgramctl --help` on a built
binary, not from a design document.

## Installing

```bash
sudo ./scripts/install/bootstrap-ubuntu.sh
```

Checks the Ubuntu version, installs dependencies, creates the five per-role
service users, sets up directories, installs binaries and hardened systemd
units, configures PostgreSQL for localhost only, and sets a default-deny
firewall.

The firewall step detects the port `sshd` is actually listening on rather than
assuming 22, because a script that locks you out of your own server is worse
than no script.

Options:

```bash
sudo ./scripts/install/bootstrap-ubuntu.sh --binaries-only   # skip host setup
sudo ./scripts/install/bootstrap-ubuntu.sh --no-firewall     # manage ufw yourself
```

Then the monitoring stack:

```bash
sudo ./scripts/install/monitoring.sh
sudo ./scripts/install/monitoring.sh --check    # validate config, change nothing
```

### Reinstalling binaries only

To replace the binaries and systemd units without touching users, directories
or the firewall — which is what you want when upgrading:

```bash
sudo hashgramctl install
```

Equivalent to `bootstrap-ubuntu.sh --binaries-only`, and the form to reach for
after building a new release. It does not restart services; do that
deliberately with `hashgramctl restart` once you have checked the version.

## Joining a network

You need three things from a source you trust, and the genesis hash must come
from somewhere **independent** of the genesis file itself. A file and its own
hash prove nothing together.

```bash
hashgramctl join-mainnet \
  --genesis-url https://... \
  --genesis-hash <sha256> \
  --peers <nodeid>@<host>:26656

hashgramctl configure-role relay,store
hashgramctl start
```

`--genesis-hash` is required, not optional convenience. Without it, joining a
network means trusting whoever handed you the file. The command refuses any
genesis whose hash does not match, and it also checks that the chain id in the
file matches the network you named.

For a devnet, add `--devnet`.

### Where the node home is

`hashgramd.service` runs as `hashgram-chain` from `/var/lib/hashgram/chain`.
On a host where `bootstrap-ubuntu.sh` created that directory, every
`hashgramctl` command operates on it by default (unless `HASHGRAM_HOME` or
`--home` says otherwise), and the commands that write into it as root —
`init`, `init-mainnet-genesis`, `finalize-genesis`, `join-mainnet` — hand the
files to the service account afterwards. On a developer machine without that
directory the default is `~/.hashgram`. `hashgramctl status` prints which one
is in use; if it is not the one the service reads, nothing else will make
sense.

`hashgramd` itself does not know about this default: pass `--home
/var/lib/hashgram/chain` to it explicitly (keys, gentx, queries) or export
`HASHGRAM_HOME=/var/lib/hashgram/chain` in the operator's shell.

## Daily commands

```bash
hashgramctl status         # one screen: height, peers, roles, sync, disk
hashgramctl health         # exits non-zero if unhealthy; for cron and monitoring
hashgramctl chain-status   # chain identity, height, validators, supply
hashgramctl node-info      # this node's identity, roles and build
hashgramctl peers          # connected peers, consensus and Hashgram P2P
hashgramctl storage        # assignments, challenges, blob health (store nodes)
hashgramctl rewards        # provider registration, credit, payouts
hashgramctl logs           # service logs
hashgramctl logs -f        # follow
```

Governance actions an operator may need — today, registering a storage
assigner — are written by `hashgramctl propose <what>` and submitted with
`hashgramd tx gov`; see `docs/FOUNDER_LAUNCH_RUNBOOK.md` Part H.

`health` is the one to wire into monitoring. It exits non-zero rather than
printing something a script has to parse.

## Service control

```bash
hashgramctl start          # start the services this machine's roles need
hashgramctl stop
hashgramctl restart
```

These drive systemd. `systemctl start hashgramd` works too, but `hashgramctl
start` knows which services this machine's roles require and starts exactly
those.

## Checking the money

The commands that let anyone verify the tokenomics claims against a running
chain:

```bash
hashgramctl chain-status               # total supply, which must be exactly 1e15 uhash
hashgramctl wallet-info <address>      # balance, vesting schedule, delegations
hashgramctl rewards                    # this node's useful-service credit and earnings
hashgramctl validator                  # signing status and safety
hashgramctl storage                    # assignments, challenges, local disk
```

And from outside the node, with the developer client:

```bash
hashgram-test-client founder verify    # the Founder share, configured and realised
hashgram-test-client wallet balance <address>
hashgram-test-client staking status
hashgram-test-client sign domains      # the signature domain strings
```

`founder verify` reports the configured basis points alongside the realised
share as a fraction of collected revenue, so the 1% figure can be checked
against what the chain actually did rather than against what it is configured
to do.

## Monitoring

Prometheus and Grafana bind to localhost. Reach them over SSH:

```bash
ssh -N -L 9090:127.0.0.1:9090 -L 3000:127.0.0.1:3000 operator@node
```

Then Prometheus at `http://localhost:9090` and Grafana at
`http://localhost:3000`. Change the Grafana admin password on first login.

### Dashboards

Three, provisioned from disk under the **Hashgram** folder:

**Chain and Consensus** — height, blocks per second, peers, validator count,
missing validators, byzantine evidence, block interval quantiles, consensus
rounds, blocks behind last signature, P2P bandwidth by peer and by message
type, mempool, per-module block timing.

**Economics and Invariants** — supply against its ceiling, the realised
Founder share in basis points, the fee routing split, the service reserve and
its depletion curve, the epoch budget, providers by status, storage
assignments and open challenges, bonded stake, welcome pool consumption and
current tier.

**Host and Database** — CPU, memory, disk by mount with a seven-day
projection, disk I/O, network, the chain process's own memory and goroutines,
clock offset, PostgreSQL.

### Two metric traps worth knowing

Both were found by scraping a running node rather than reading documentation.

**`cometbft_p2p_peers` does not exist until a node's first peer event.** It is
a lazily-created Prometheus child, so an alert comparing it to zero **cannot
fire** on a node that has never had a peer. There is a separate
`absent()` alert for that state, and the dashboard panel reads "never peered"
rather than zero.

**CometBFT exports no missed-blocks counter.** Validator signing health is
measured as the gap between `cometbft_consensus_latest_block_height` and
`cometbft_consensus_validator_last_signed_height`. A rising gap means blocks
are being missed and jailing is approaching.

### The alert that matters most

`HashgramSupplyExceedsCeiling`. It should be impossible: there is no mint
module. If it ever fires, treat the binary as suspect before treating the
metric as suspect, stop accepting deposits, and compare the binary's checksum
against the published release.

24 rules in total, in
[`deploy/monitoring/rules/hashgram-alerts.yml`](../deploy/monitoring/rules/hashgram-alerts.yml).
Each states what an operator should check, because an alert that fires without
saying what to do is one people learn to silence.

### Verifying the dashboards still work

```bash
scripts/dev/check-dashboards.sh          # parse and PromQL syntax
scripts/dev/check-dashboards.sh --live   # also query a running Prometheus
```

A panel with a broken query renders as "No data", which looks identical to a
healthy zero. That is the failure this script exists to catch, and it has
already caught one: the welcome tier panel used bare PromQL comparisons, which
filter series out rather than yielding zero, so adding the three tier terms
produced an empty vector and the panel read "No data" at every sequence below
10,000.

## Backups

```bash
hashgramctl backup                      # configuration and light state
hashgramctl backup --include-chain-db    # everything, much larger and slower
hashgramctl restore <archive>
```

**What the archive deliberately excludes:** `priv_validator_key.json`, the
node key, and the keyring. Private keys do not belong in an archive that gets
copied to a second machine, and a restore that recreated a consensus key on a
new host would be a double-signing accident waiting to happen.

Back those up separately, once, by hand, and store them offline. The full
procedure is in [DISASTER_RECOVERY.md](DISASTER_RECOVERY.md).

## Upgrading

```bash
hashgramctl update                       # installed versions and known on-chain upgrades
```

An on-chain upgrade proposal names a height. Before it:

1. Read the proposal and check what changes.
2. Verify the new binary's checksum against the published release.
3. Install it but do not restart yet.
4. At the upgrade height, the node halts on its own. Restart with the new
   binary.

Hashgram does **not** use automatic binary downloads. `x/upgrade` supports
them via `go-getter`, and that path is deliberately unused: automatic download
means the upgrade mechanism decides what code your validator runs. An operator
installing a binary whose checksum they verified is the whole point.

## Pre-launch checks

```bash
hashgramctl mainnet-preflight
```

Refuses to pass — not warns — on a devnet key on a mainnet host, a default
password, a publicly reachable admin RPC, a validator key readable by group or
others, a chain id that disagrees with the pin, a genesis hash that disagrees
with the pin, no Founder beneficiary, insufficient disk, an unsynchronised
clock, or no firewall.

Every failure names the command that fixes it.

## Log levels

Default is `info`. Raising to `debug` is a legitimate operator action:

```bash
sudo sed -i 's/^log_level = .*/log_level = "debug"/' \
  /var/lib/hashgram/chain/config/config.toml
hashgramctl restart
```

Safe because `debug` contains no message plaintext to reveal. That is a
property of the code, not a request that operators avoid debug logging: no log
call at any level writes message content, and
[`scripts/dev/check-logging.sh`](../scripts/dev/check-logging.sh) enforces it
in CI. Policy in [LOGGING_POLICY.md](LOGGING_POLICY.md).

Set journal retention so a compromised host yields a bounded amount of
metadata:

```ini
# /etc/systemd/journald.conf
[Journal]
SystemMaxUse=2G
MaxRetentionSec=2week
```

## Common problems

### The node is not producing or receiving blocks

```bash
hashgramctl status
hashgramctl peers
hashgramctl logs | tail -50
```

Zero peers is the usual cause. Check that 26656 is open and that
`persistent_peers` still resolves — a peer that changed address is a peer you
no longer have.

### Peers connect and the height does not move

You are probably on a different genesis from the network. Two chains can share
a chain id and a TCP connection while being disjoint at consensus:

```bash
hashgramctl network-info    # compare the pinned hash against the network's
```

If they differ, you joined a fork. Re-join with the correct
`--genesis-hash`.

### The validator is missing blocks

```bash
hashgramctl validator
```

Check that the remote signer is reachable, that the consensus key is where the
node expects it, and that CPU and disk are not saturated — the
`Host and Database` dashboard next to `cometbft_state_block_processing_time`
answers the last one directly.

### Disk filling

```bash
hashgramctl status          # includes free space
```

Set pruning in `app.toml`, or grow the volume. The
`HashgramDiskFillingFast` alert extrapolates six hours forward and fires if
the filesystem runs out within a week, which is meant to give you time to act
outside an incident.

Do not delete files under `/var/lib/hashgram` by hand.

### A provider got jailed

```bash
hashgramctl storage         # challenge history
hashgramctl rewards         # fraud score and earnings
```

**Check the disks first.** A failing disk looks exactly like fraud to the
challenge system, and the most common cause of a failed challenge on an honest
node is hardware. The fraud score decays by 10 per epoch, so one bad day does
not permanently condemn a node, but four failures inside the decay window
jail it and slash 5% of bond.

## Running the acceptance suites

All three scripts are safe to run on a development host and clean up after
themselves.

```bash
scripts/testnet/devnet.sh              # single node, economic acceptance checks
scripts/testnet/devnet.sh stop
scripts/testnet/devnet.sh clean

scripts/testnet/four-validator.sh      # four validators, resilience, fork isolation
scripts/testnet/four-validator.sh clean

scripts/testnet/phase2.sh              # + 3 P2P nodes, indexer, safety, two clients
scripts/testnet/phase2.sh stop
scripts/testnet/phase2.sh clean
```

The four-validator script kills a validator mid-run to prove the chain
continues on 75% of voting power, restarts it to prove it rejoins, and then
tries to join two different forks to demonstrate which layer stops each one.

The Phase 2 script needs PostgreSQL on the host (it creates a throwaway
role and database) and takes about ten minutes. It proves messages decrypt
only for their recipients, media replicates and repairs, forks are refused at
the Hashgram handshake, the safety engine's verdict is enforced, storage
challenges are answered, and the network keeps working after the genesis
host is destroyed. Its output is reproduced in `docs/FINAL_REPORT.md` §7.

## CI, locally

```bash
make ci          # everything: fmt, vet, staticcheck, gosec, gitleaks, govulncheck, policy, tests
make ci-fast     # skips the slow stages
make check-policy
```

The same script runs in GitHub Actions, so a green local run means a green CI
run. A pipeline that only exists inside a CI provider is one nobody runs
before pushing.

## Building a release

```bash
scripts/dev/release.sh                 # binaries with checksums into dist/
scripts/dev/release.sh --verify        # build twice, prove byte-identical
scripts/dev/release.sh --check dist/   # verify an existing dist
```

`--verify` clears the build cache between builds so the second one genuinely
recompiles. Reproducibility is what makes a published checksum worth anything
to the operator who downloads it.
