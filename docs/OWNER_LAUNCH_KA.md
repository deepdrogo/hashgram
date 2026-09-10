# Hashgram — owner launch card

Server: `186.241.19.230`. Full runbook: [LAUNCH_HANDOVER_KA.md](LAUNCH_HANDOVER_KA.md).

## Already done on this host

- `hashgramctl init --moniker genesis-full`
- `hashgramctl configure-role validator`
- Validator operator key created
- Operator address: `hash127zemcfnxd3jrldpjzzgcckek4dswyw044sdzj`
- CometBFT node id: `2f6629254d6568e48a09aff20ba01a61a26c47f0`
- P2P: `186.241.19.230:26656`
- Admin RPC: `127.0.0.1:26657` (not public)
- Mainnet genesis: **not created**. Waiting for the Founder public address.
- Founder mnemonic: **not on this server and not in chat**

## Four keys (do not mix them)

| Id | Name | Where | Controls | What you take |
| --- | --- | --- | --- | --- |
| A | Founder (cold) | Your offline laptop | 199,000,000 HASH (19M spendable + 180M vesting) and the **1% protocol-fee beneficiary** | 24 words on paper. Server gets only `hash1...` |
| B | Validator operator (hot) | This server, already created | 1,000,000 HASH; gentx, votes, sends | 24 words + keyring password on paper |
| C | Consensus / `priv_validator` | This server, created by init | Block signing | Never copy to a second validator |
| D | Rewards cold | Offline, a second key | store / relay / media rewards | 24 words on paper. Server gets only `hash1...` |

The 1% is **not** a transfer tax. A 100 HASH send delivers 100 HASH.
The 1% is cut only from protocol fee revenue (gas, username, module service fees)
and is credited to A automatically. Anyone may send `MsgClaimFounderRevenue`;
payout still goes to A. The chain also pays about every 7200 blocks (~8 hours).

To sign your own transactions you need A's 19,000,000 spendable HASH and a
desktop / `hashgram-test-client` that signs **locally**.

## Take off this server today

```bash
/home/hashgram/scripts/launch/show-operator-backup.sh
```

Write down the 24 words, the keyring password, and the address
`hash127zemcfnxd3jrldpjzzgcckek4dswyw044sdzj`. Verify with
`hashgram-keygen derive` from the paper, not the screen.

Then:

```bash
shred -u /root/HASHGRAM_TAKE_OFFLINE/operator-key.json
```

Keep `operator-keyring-password.txt` until launch scripts have run, then shred it.

Never copy:

- `/var/lib/hashgram/chain/config/priv_validator_key.json`
- `/var/lib/hashgram/chain/config/node_key.json`
- The Founder words (they are not here)

## Create the Founder key offline

On your laptop, not this VPS:

```bash
scp root@186.241.19.230:/usr/local/bin/hashgram-keygen .
# disconnect the network; ping 8.8.8.8 must fail
chmod +x hashgram-keygen
./hashgram-keygen new
./hashgram-keygen derive
```

Paper, twice, two places. Then create a **second** key the same way (D).
Send only the two `hash1...` addresses back. Never the words.

A Ledger (coin type 118, Cosmos) is better than paper for 200M HASH.

## Start Mainnet (one command)

```bash
cd /home/hashgram
scripts/launch/launch-mainnet.sh hash1<YOUR_FOUNDER>
```

That runs: preliminary genesis (you = 199M + 1% beneficiary) →
operator funded 1,000,000 HASH → gentx 900,000 HASH →
`finalize-genesis` (write the **final GENESIS HASH** off the server) →
preflight → start → verify.

Expected:

```text
bank total uhash              = 1000000000000000
wallet-info founder           = 199,000,000 / 19M spendable / 180M vesting
founder verify                = 100 bps, ceiling 100, beneficiary = you
```

If Spendable == Balance, vesting was not created. Stop.

Publish `genesis.json` on one channel and the hash on two **other** channels.

## After blocks: assigner and earning roles

```bash
scripts/launch/post-launch.sh hash1<YOUR_REWARDS_COLD>
```

Adds relay/store/media/bootstrap, funds the node operator with 1,100 HASH,
submits and votes the first assigner proposal (7-day voting period).
Store nodes earn nothing until that proposal passes.
The 1% founder fee does not depend on this step.

## Sign from a desktop

26657 and 1317 are localhost. Opening them publicly fails preflight.

SSH tunnel (works today):

```bash
ssh -N -L 26657:127.0.0.1:26657 -L 1317:127.0.0.1:1317 root@186.241.19.230
```

Desktop RPC `http://127.0.0.1:26657`, REST `http://127.0.0.1:1317`.

Or run a local node with `join-mainnet` and the published genesis hash.
Or put HTTPS in front yourself. Do not bind 26657 to `0.0.0.0`.

The Founder mnemonic is loaded only on your computer, encrypted with Argon2id.

Transactions you will sign: `MsgSend`, staking, username, identity,
`MsgClaimFounderRevenue` (optional), `MsgVote` (operator is easier; it holds the stake).

## Second VPS

See [LAUNCH_HANDOVER_KA.md](LAUNCH_HANDOVER_KA.md) section 5.
Peer: `2f6629254d6568e48a09aff20ba01a61a26c47f0@186.241.19.230:26656`.
Never copy `priv_validator_key.json`.

## Desktop app

Spec: [PROMPT_WINDOWS_DESKTOP.md](PROMPT_WINDOWS_DESKTOP.md).
Copy-paste AI prompt: [PROMPT_DESKTOP_AI.md](PROMPT_DESKTOP_AI.md).

Build with **Tauri + Rust + hashgram-sdk**. UniFFI/C# bindings do not exist yet.
Stage 1 = wallet / identity / staking / founder 1% screen / supply.
Stage 2 = messenger / feed / media / calls, only through the SDK.

The app cannot talk to Mainnet until section "Start Mainnet" has finished.

## Honest limits

One validator: losing this server loses the network.
Missing: mobile apps, UniFFI, WebRTC media stack, SFU E2EE, push,
welcome attestation, independent audit.
Full list: [LAUNCH_HANDOVER_KA.md](LAUNCH_HANDOVER_KA.md) section 8,
[DECENTRALIZATION.md](DECENTRALIZATION.md).

## Your next three actions

1. SSH: `show-operator-backup.sh` → paper → shred the JSON.
2. Offline: Founder + rewards keys. Send back only two `hash1...` addresses.
3. Say the Founder address here. `launch-mainnet.sh` will run and return the GENESIS HASH.
