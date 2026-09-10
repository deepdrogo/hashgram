#!/usr/bin/env bash
# Broadcast a Founder delegation signed on the offline laptop, then verify.
#
# Usage: founder-delegate-broadcast.sh /path/to/signed.json
set -euo pipefail

SIGNED="${1:-}"
HOME_DIR=/var/lib/hashgram/chain
KIT=/root/hashgram-founder-delegate-kit
FACTS="$KIT/SIGN_FACTS.txt"

die() { echo "ERROR: $*" >&2; exit 1; }
[[ -f "$SIGNED" ]] || die "usage: $0 signed.json"
[[ -f "$FACTS" ]] || die "$FACTS missing; run founder-delegate-prepare.sh first"

fact() { sed -n "s/^$1=//p" "$FACTS"; }
FOUNDER="$(fact founder_address)"; VALOPER="$(fact validator)"; AMOUNT="$(fact amount_uhash)"
CHAIN_ID="$(fact chain_id)"; ACCT="$(fact account_number)"; SEQ="$(fact sequence)"

# 1. The signed body must be the body we prepared, signed by the Founder.
python3 - "$SIGNED" "$FOUNDER" "$VALOPER" "$AMOUNT" <<'PY' || die "signed.json does not match the prepared delegation"
import json, sys
d = json.load(open(sys.argv[1]))
m = d["body"]["messages"]
assert len(m) == 1 and m[0]["@type"] == "/cosmos.staking.v1beta1.MsgDelegate", "unexpected message"
assert m[0]["delegator_address"] == sys.argv[2], "delegator is not the Founder"
assert m[0]["validator_address"] == sys.argv[3], "validator differs from the prepared one"
assert m[0]["amount"] == {"denom": "uhash", "amount": sys.argv[4]}, "amount differs"
assert len(d["signatures"]) == 1 and d["signatures"][0], "not signed"
PY

# 2. Cryptographic pre-check: the one signature must verify and belong to the
# Founder address. (The chain repeats this in CheckTx; this fails earlier and
# with a clearer message.) An empty keyring dir keeps the SDK from looking
# for a local key it must not find on this machine.
EMPTY_KR="$(mktemp -d)"; trap 'rm -rf "$EMPTY_KR"' EXIT
VALIDATION="$(hashgramd tx validate-signatures "$SIGNED" --home "$HOME_DIR" --chain-id "$CHAIN_ID" \
  --account-number "$ACCT" --sequence "$SEQ" --offline --keyring-backend test --keyring-dir "$EMPTY_KR" 2>&1 || true)"
echo "$VALIDATION"
grep -Eq "^\s*0: $FOUNDER\s+\[OK\]" <<<"$VALIDATION" || die "signature does not verify as $FOUNDER"

# 3. The sequence must still be what we signed for.
CUR_SEQ="$(hashgramd query auth account-info "$FOUNDER" --home "$HOME_DIR" --output json \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["info"].get("sequence","0"))')"
[[ "$CUR_SEQ" == "$SEQ" ]] || die "founder sequence is now $CUR_SEQ, signed for $SEQ; re-run prepare and sign again"

BEFORE="$(hashgramd query staking validator "$VALOPER" --home "$HOME_DIR" --output json \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["validator"]["tokens"])')"

echo "broadcasting..."
TX="$(hashgramd tx broadcast "$SIGNED" --home "$HOME_DIR" --broadcast-mode sync --output json \
  | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d.get("code",0)==0, d; print(d["txhash"])')"
echo "  txhash $TX"

for _ in $(seq 1 30); do
  if OUT="$(hashgramd query tx "$TX" --home "$HOME_DIR" --output json 2>/dev/null)"; then
    CODE="$(printf '%s' "$OUT" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("code",0))')"
    [[ "$CODE" == "0" ]] || die "tx failed with code $CODE: $(printf '%s' "$OUT" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("raw_log",""))')"
    break
  fi
  sleep 2
done
[[ -n "${CODE:-}" ]] || die "tx $TX not committed after 60s"

HEIGHT="$(printf '%s' "$OUT" | python3 -c 'import json,sys; print(json.load(sys.stdin)["height"])')"
AFTER="$(hashgramd query staking validator "$VALOPER" --home "$HOME_DIR" --output json \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["validator"]["tokens"])')"
DELEG="$(hashgramd query staking delegation "$FOUNDER" "$VALOPER" --home "$HOME_DIR" --output json \
  | python3 -c 'import json,sys; d=json.load(sys.stdin); d=d.get("delegation_response",d); print(d["balance"]["amount"])')"
DV="$(hashgramd query auth account "$FOUNDER" --home "$HOME_DIR" --output json \
  | python3 -c 'import json,sys; a=json.load(sys.stdin)["account"]; v=a.get("value",a); print(v["base_vesting_account"].get("delegated_vesting",[]))')"

{
  echo "founder_delegation_txhash=$TX"
  echo "founder_delegation_height=$HEIGHT"
  echo "founder_delegated_uhash=$DELEG"
} >> /etc/hashgram/LAUNCH_RECORD.txt

echo
echo "DELEGATED"
echo "  height              $HEIGHT"
echo "  validator tokens    $BEFORE -> $AFTER uhash"
echo "  founder delegation  $DELEG uhash"
echo "  delegated_vesting   $DV"
echo "  recorded in         /etc/hashgram/LAUNCH_RECORD.txt"
echo
echo "The signed.json is now spent (sequence used); it is safe to keep or delete."
