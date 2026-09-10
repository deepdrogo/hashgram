# Hashgram launch scripts

These scripts finish Mainnet on the genesis VPS. They never create or
accept a Founder mnemonic.

| Script | Where | What |
| --- | --- | --- |
| `make-founder-key-OFFLINE.sh` | Offline laptop | Prints the Founder 24 words once |
| `show-operator-backup.sh` | This server, you only | Prints the hot operator mnemonic so you can write it on paper |
| `launch-mainnet.sh` | This server | genesis → gentx → finalize → start → verify 1% |
| `post-launch.sh` | This server, after blocks | assigner proposal, earning roles, fund node operator |

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
