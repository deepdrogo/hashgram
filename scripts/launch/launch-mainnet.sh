#!/usr/bin/env bash
# Launch Hashgram Mainnet on this genesis VPS.
#
# Prerequisite: Founder key created OFFLINE. This script accepts only the
# public address. It never asks for a mnemonic.
#
#   scripts/launch/launch-mainnet.sh hash1<YOUR_FOUNDER_ADDRESS>
set -euo pipefail

FOUNDER="${1:-}"
HOME_DIR=/var/lib/hashgram/chain
TAKE=/root/HASHGRAM_TAKE_OFFLINE
OP_JSON="$TAKE/operator-key.json"
OP_PASS_FILE="$TAKE/operator-keyring-password.txt"
OP_ADDR_FILE="$TAKE/operator-address.txt"
export HASHGRAM_HOME="$HOME_DIR"

die() { echo "ERROR: $*" >&2; exit 1; }

[[ "$(id -u)" -eq 0 ]] || die "run as root"
[[ -n "$FOUNDER" ]] || die "usage: $0 hash1<FOUNDER>"
[[ "$FOUNDER" == hash1* ]] || die "founder address must start with hash1"
[[ ${#FOUNDER} -ge 38 ]] || die "founder address looks too short"
[[ -f "$OP_PASS_FILE" && -f "$OP_ADDR_FILE" ]] || die "operator password/address missing in $TAKE"
[[ ! -f "$HOME_DIR/config/genesis.json" ]] || die "genesis.json already exists — refusing to overwrite Mainnet identity"

OP_ADDR="$(tr -d '[:space:]' < "$OP_ADDR_FILE")"
[[ "$OP_ADDR" == hash1* ]] || die "operator address file is invalid"
[[ "$FOUNDER" != "$OP_ADDR" ]] || die "Founder address must NOT be the operator address"

PASS="$(tr -d '\n' < "$OP_PASS_FILE")"

echo
echo "======================================================================"
echo "  HASHGRAM MAINNET LAUNCH"
echo "======================================================================"
echo "  Founder (20% + 1% fees) : $FOUNDER"
echo "  Operator (1,000,000)    : $OP_ADDR"
echo "  Stake                   : 900,000 HASH"
echo "  Chain ID                : hashgram-1"
echo "  Public IP               : 186.241.19.230"
echo "======================================================================"
echo

hashgramctl -y init-mainnet-genesis \
  --founder-address "$FOUNDER" \
  --genesis-account "${OP_ADDR}=1000000HASH" \
  --moniker genesis-full

printf '%s\n' "$PASS" | hashgramd genesis gentx operator 900000000000uhash \
  --chain-id hashgram-1 \
  --home "$HOME_DIR" \
  --keyring-backend file \
  --moniker genesis-full \
  --commission-rate 0.10 \
  --commission-max-rate 0.20 \
  --commission-max-change-rate 0.01 \
  --ip 186.241.19.230 \
  -y

hashgramctl -y finalize-genesis | tee "$TAKE/finalize-genesis.log"

python3 -c '
import json
n = json.load(open("/etc/hashgram/network.json"))
h = n.get("genesis_hash") or n.get("GenesisHash") or ""
open("/root/HASHGRAM_TAKE_OFFLINE/GENESIS_HASH.txt","w").write(h + "\n")
print("PINNED_GENESIS_HASH=" + h)
'
chmod 644 "$TAKE/GENESIS_HASH.txt"

hashgramctl mainnet-preflight
hashgramctl start
sleep 8
hashgramctl chain-status || true

echo
echo "----- bank total (must be 1000000000000000 uhash) -----"
hashgramd query bank total --denom uhash --home "$HOME_DIR" || true
echo
echo "----- founder wallet (199,000,000 HASH; 19M spendable; 180M vesting) -----"
hashgramctl wallet-info "$FOUNDER" || true
echo
echo "----- founder 1% params (100 bps, ceiling 100) -----"
hashgram-test-client founder verify --node tcp://127.0.0.1:26657 || true

NODE_ID="$(hashgramd comet show-node-id --home "$HOME_DIR" 2>/dev/null || true)"
{
  echo "chain_id=hashgram-1"
  echo "founder_address=$FOUNDER"
  echo "operator_address=$OP_ADDR"
  echo "comet_node_id=$NODE_ID"
  echo "p2p=186.241.19.230:26656"
} > /etc/hashgram/launch-facts.txt
chmod 644 /etc/hashgram/launch-facts.txt

echo
echo "======================================================================"
echo "  CHAIN STARTED"
echo "======================================================================"
echo "  Write the GENESIS HASH from $TAKE/GENESIS_HASH.txt onto paper NOW."
echo "  Publish genesis.json and the hash on SEPARATE channels."
echo
echo "  Next (after blocks are producing):"
echo "    scripts/launch/post-launch.sh hash1<COLD_REWARD>"
echo "======================================================================"
