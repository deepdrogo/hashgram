# Hashgram launch scripts

These scripts finish Mainnet on the genesis VPS. They never create or
accept a Founder mnemonic.

| Script | Where | What |
| --- | --- | --- |
| `make-founder-key-OFFLINE.sh` | Offline laptop | Prints the Founder 24 words once |
| `show-operator-backup.sh` | This server, you only | Prints the hot operator mnemonic so you can write it on paper |
| `launch-mainnet.sh` | This server | genesis → gentx → finalize → start → verify 1% |
| `post-launch.sh` | This server, after blocks | assigner proposal, earning roles, fund node operator |
| `founder-delegate-prepare.sh [HASH]` | This server | Builds an UNSIGNED `MsgDelegate` Founder → validator, packages an offline signing kit (zip with `hashgramd` for Windows/macOS/Linux + `sign-OFFLINE.*`) |
| `sign-OFFLINE.bat` / `.sh` | Offline laptop (inside the kit) | Imports the 24 words into a temp keyring, signs, wipes the keyring; produces `signed.json` |
| `founder-delegate-broadcast.sh signed.json` | This server | Verifies the signature is the Founder's and the body is the prepared one, broadcasts, records in `LAUNCH_RECORD.txt` |

Why delegate: voting power in governance is staked HASH. With ≥ 33.4 % of the
bonded stake the Founder holds a veto over every parameter change and upgrade,
and the 24 words stay offline because the transaction is signed there. Vesting
coins may be delegated; they stay locked either way. Delegated coins share the
validator's slashing risk (downtime 0.01 %, double-sign 5 %) and need 21 days
to unbond.

## Already done on this host

- `hashgramctl init --moniker genesis-full`
- `hashgramctl configure-role validator`
- Operator key created (address in `/root/HASHGRAM_TAKE_OFFLINE/operator-address.txt`)
- CometBFT node id: `2f6629254d6568e48a09aff20ba01a61a26c47f0`
- P2P advertised as `186.241.19.230:26656`
- Admin RPC bound to `127.0.0.1:26657` (preflight requires this)

## You still have to do

1. Offline: create Founder key. Send only `hash1...` back.
2. `scripts/launch/launch-mainnet.sh hash1...`
3. Write the printed GENESIS HASH on paper, off this server.
4. Offline: create a second cold key for node rewards.
5. `scripts/launch/post-launch.sh hash1<reward>`
6. `scripts/launch/founder-delegate-prepare.sh` → zip to the offline laptop →
   `sign-OFFLINE` → `signed.json` back → `founder-delegate-broadcast.sh signed.json`
