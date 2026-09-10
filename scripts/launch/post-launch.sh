#!/usr/bin/env bash
# After Mainnet is producing blocks: register the storage assigner,
# enable earning roles on this first host, fund the node operator.
#
#   scripts/launch/post-launch.sh hash1<COLD_REWARD_ADDRESS>
#
# The reward address is a second offline key (NOT Founder, NOT operator).
# Useful-service HASH (storage/relay) is paid there. The 1% founder fee
# is independent and already goes to the Founder address set at genesis.
set -euo pipefail

REWARD="${1:-}"
HOME_DIR=/var/lib/hashgram/chain
TAKE=/root/HASHGRAM_TAKE_OFFLINE
OP_PASS_FILE="$TAKE/operator-keyring-password.txt"
export HASHGRAM_HOME="$HOME_DIR"

die() { echo "ERROR: $*" >&2; exit 1; }
[[ "$(id -u)" -eq 0 ]] || die "run as root"
[[ "$REWARD" == hash1* ]] || die "usage: $0 hash1<COLD_REWARD>"
[[ -f "$OP_PASS_FILE" ]] || die "operator password missing"
PASS="$(tr -d '\n' < "$OP_PASS_FILE")"

hashgramctl configure-role validator,relay,store,media,bootstrap \
  --declared-storage 200000000000 \
  --reward-address "$REWARD"

NODE_OP="$(runuser -u hashgram-node -- hashgram-node operator-key --home /var/lib/hashgram/node | tr -d '[:space:]')"
[[ "$NODE_OP" == hash1* ]] || die "could not read node operator address"
echo "NODE_OPERATOR=$NODE_OP"
echo "$NODE_OP" > /etc/hashgram/node-operator-address.txt
chmod 644 /etc/hashgram/node-operator-address.txt

# Broadcast is sync: it returns on CheckTx, before the block. Each tx below
# must be committed before the next one is simulated, or the sequence is stale.
wait_tx() {
  local hash="$1" i
  for i in $(seq 1 30); do
    if hashgramd query tx "$hash" --home "$HOME_DIR" --output json >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
  done
  die "tx $hash not committed after 60s"
}
tx_hash() { python3 -c 'import json,sys; print(json.load(sys.stdin)["txhash"])'; }

FUNDED="$(hashgramd query bank balances "$NODE_OP" --home "$HOME_DIR" --output json \
  | python3 -c 'import json,sys; b=json.load(sys.stdin)["balances"]; print(sum(int(x["amount"]) for x in b if x["denom"]=="uhash"))')"
if [[ "$FUNDED" -lt 1100000000 ]]; then
  H="$(printf '%s\n' "$PASS" | hashgramd tx bank send operator "$NODE_OP" 1100000000uhash \
    --chain-id hashgram-1 --home "$HOME_DIR" --keyring-backend file \
    --fees 5000uhash --gas auto --gas-adjustment 1.4 --output json -y | tx_hash)"
  echo "fund tx $H"; wait_tx "$H"
else
  echo "node operator already funded ($FUNDED uhash)"
fi

hashgramctl propose add-assigner "$NODE_OP" --out /root/proposal-add-assigner.json
H="$(printf '%s\n' "$PASS" | hashgramd tx gov submit-proposal /root/proposal-add-assigner.json \
  --from operator --chain-id hashgram-1 --home "$HOME_DIR" --keyring-backend file \
  --gas auto --gas-adjustment 1.4 --fees 5000uhash --output json -y | tx_hash)"
echo "proposal tx $H"; wait_tx "$H"
PROP_ID="$(hashgramd query gov proposals --output json --home "$HOME_DIR" | python3 -c '
import json, sys
d = json.load(sys.stdin)
props = d.get("proposals") or []
if isinstance(props, dict):
    props = [props]
if not props:
    raise SystemExit("no proposals")
last = props[-1]
print(last.get("id") or last.get("proposal_id") or "")
')"
[[ -n "$PROP_ID" ]] || die "could not read proposal id"
echo "PROPOSAL_ID=$PROP_ID"

H="$(printf '%s\n' "$PASS" | hashgramd tx gov vote "$PROP_ID" yes \
  --from operator --chain-id hashgram-1 --home "$HOME_DIR" --keyring-backend file \
  --fees 5000uhash --output json -y | tx_hash)"
echo "vote tx $H"; wait_tx "$H"

sed -i 's/^auto_register_provider = false/auto_register_provider = true/' /etc/hashgram/node.toml
hashgramctl restart
sleep 5
hashgramctl status || true
hashgramctl rewards || true

echo
echo "Assigner proposal $PROP_ID is voting (7 days, 40% quorum)."
echo "With one validator your vote is 100%. After it passes:"
echo "  hashgramd query serviceproof params --output json"
echo "Store nodes earn only after the assigner is on-chain."
