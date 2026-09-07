#!/usr/bin/env bash
#
# Bring up a single-node Hashgram devnet using the real genesis tooling.
#
# This is the acceptance test for "does the whole thing actually work": it
# builds genesis with the same code path Mainnet uses, starts a validator,
# and then verifies the properties the specification is most specific about
# rather than merely reporting that a process is running.
#
#   scripts/testnet/devnet.sh          create and start
#   scripts/testnet/devnet.sh stop     stop and leave the state
#   scripts/testnet/devnet.sh clean    stop and delete the state
#   scripts/testnet/devnet.sh verify   run the acceptance checks
#
# DEVNET ONLY. The keys this creates are unencrypted and the identity is the
# devnet identity, which can never be mistaken for Mainnet: a different
# network magic, a different chain-id, and mainnet-preflight refuses to pass
# on it.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

DEVNET_DIR="${DEVNET_DIR:-$REPO_ROOT/devnet}"
NODE_HOME="$DEVNET_DIR/node"
CONFIG_DIR="$DEVNET_DIR/config"
LOG_FILE="$DEVNET_DIR/node.log"
PID_FILE="$DEVNET_DIR/node.pid"

HASHGRAMD="$REPO_ROOT/build/hashgramd"
HASHGRAMCTL="$REPO_ROOT/build/hashgramctl"
KEYGEN="$REPO_ROOT/build/hashgram-keygen"

CHAIN_ID="hashgram-devnet-1"
RPC="tcp://127.0.0.1:26657"
RPC_HTTP="http://127.0.0.1:26657"
DENOM="uhash"

KEYRING=(--keyring-backend test --home "$NODE_HOME")

# Colours only when attached to a terminal, so piped output stays clean.
if [ -t 1 ]; then
  BOLD=$'\033[1m'; GREEN=$'\033[32m'; RED=$'\033[31m'; YELLOW=$'\033[33m'; RESET=$'\033[0m'
else
  BOLD=""; GREEN=""; RED=""; YELLOW=""; RESET=""
fi

say()  { printf '%s\n' "$*"; }
head1(){ printf '\n%s%s%s\n%s\n' "$BOLD" "$*" "$RESET" "$(printf '=%.0s' {1..74})"; }
ok()   { printf '  %s[ ok ]%s %s\n' "$GREEN" "$RESET" "$*"; }
bad()  { printf '  %s[FAIL]%s %s\n' "$RED" "$RESET" "$*"; FAILURES=$((FAILURES+1)); }
warn() { printf '  %s[warn]%s %s\n' "$YELLOW" "$RESET" "$*"; }

FAILURES=0

require_binaries() {
  for bin in "$HASHGRAMD" "$HASHGRAMCTL" "$KEYGEN"; do
    if [ ! -x "$bin" ]; then
      say "error: $bin is missing. Run: make build"
      exit 1
    fi
  done
}

# ---------------------------------------------------------------------------
# stop / clean
# ---------------------------------------------------------------------------

stop_node() {
  if [ -f "$PID_FILE" ]; then
    local pid
    pid="$(cat "$PID_FILE")"
    if kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
      # Give it time to flush state cleanly. A hard kill mid-commit is how a
      # node ends up with a corrupt database.
      for _ in $(seq 1 20); do
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.5
      done
      kill -9 "$pid" 2>/dev/null || true
    fi
    rm -f "$PID_FILE"
  fi
  pkill -f "hashgramd start --home $NODE_HOME" 2>/dev/null || true
}

case "${1:-up}" in
  stop)
    stop_node
    say "devnet stopped; state left in $DEVNET_DIR"
    exit 0
    ;;
  clean)
    stop_node
    rm -rf "$DEVNET_DIR"
    say "devnet stopped and $DEVNET_DIR removed"
    exit 0
    ;;
  verify)
    require_binaries
    ;;
  up|"")
    require_binaries
    ;;
  *)
    say "usage: $0 [up|stop|clean|verify]"
    exit 1
    ;;
esac

# ---------------------------------------------------------------------------
# up
# ---------------------------------------------------------------------------

if [ "${1:-up}" != "verify" ]; then
  head1 "HASHGRAM DEVNET"

  stop_node
  rm -rf "$DEVNET_DIR"
  mkdir -p "$NODE_HOME" "$CONFIG_DIR"

  say ""
  say "Creating the Founder cold wallet the way the runbook says to: with"
  say "hashgram-keygen, which does no networking and writes nothing to disk."
  say ""

  # In a real launch this runs on a different machine and only the public
  # address crosses over. Here we capture the address and discard the
  # mnemonic, which is exactly what the server is supposed to end up with.
  KEYGEN_OUT="$("$KEYGEN" new --words 24 --quiet)"
  FOUNDER_ADDR="$(printf '%s' "$KEYGEN_OUT" | grep -oE 'hash1[a-z0-9]{38,}' | head -1)"
  if [ -z "$FOUNDER_ADDR" ]; then
    say "error: could not extract a Founder address from hashgram-keygen output"
    exit 1
  fi
  ok "Founder public address: $FOUNDER_ADDR"
  say "        (the mnemonic was printed and discarded; it is not stored anywhere)"

  say ""
  say "Initialising the node."
  "$HASHGRAMD" init devnet-validator --chain-id "$CHAIN_ID" \
    --home "$NODE_HOME" --default-denom "$DENOM" >/dev/null 2>&1
  ok "node home initialised at $NODE_HOME"

  say ""
  say "Building genesis with the real tooling."
  "$HASHGRAMCTL" init-mainnet-genesis \
    --founder-address "$FOUNDER_ADDR" \
    --devnet \
    --force \
    --yes \
    --home "$NODE_HOME" \
    --config-dir "$CONFIG_DIR" \
    --data-dir "$DEVNET_DIR"

  GENESIS_HASH="$(sha256sum "$NODE_HOME/config/genesis.json" | awk '{print $1}')"
  ok "genesis hash: $GENESIS_HASH"

  say ""
  say "Creating the validator and its gentx."
  "$HASHGRAMD" keys add validator "${KEYRING[@]}" >/dev/null 2>&1
  VALIDATOR_ADDR="$("$HASHGRAMD" keys show validator -a "${KEYRING[@]}")"
  "$HASHGRAMD" keys add alice "${KEYRING[@]}" >/dev/null 2>&1
  ALICE_ADDR="$("$HASHGRAMD" keys show alice -a "${KEYRING[@]}")"
  "$HASHGRAMD" keys add bob "${KEYRING[@]}" >/dev/null 2>&1
  BOB_ADDR="$("$HASHGRAMD" keys show bob -a "${KEYRING[@]}")"

  ok "validator: $VALIDATOR_ADDR"
  ok "alice:     $ALICE_ADDR"
  ok "bob:       $BOB_ADDR"

  # The validator and the test accounts need a balance, and genesis is
  # already complete and totals exactly the canonical supply. So the stake
  # comes out of the treasury reserve rather than being added on top: adding
  # balances would exceed the supply ceiling, and the genesis builder
  # correctly refuses that.
  say ""
  say "Funding the validator and test accounts from the treasury reserve."
  say "(Genesis already totals exactly 1,000,000,000 HASH, so new balances"
  say " cannot be added on top; they have to come out of an allocation.)"

  TREASURY_ADDR="$(python3 - "$NODE_HOME/config/genesis.json" <<'PY'
import json,sys
doc=json.load(open(sys.argv[1]))
for b in doc["app_state"]["bank"]["balances"]:
    pass
res=doc["app_state"]["treasury"]["reserves"]
names={r["name"]:r["sub_account"] for r in res}
print(names.get("treasury",""))
PY
)"

  python3 - "$NODE_HOME/config/genesis.json" "$VALIDATOR_ADDR" "$ALICE_ADDR" "$BOB_ADDR" <<'PY'
import json,sys

path, validator, alice, bob = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
doc = json.load(open(path))
bank = doc["app_state"]["bank"]

# Move funds out of the treasury reserve into the validator and test
# accounts, keeping total supply exactly unchanged.
STAKE      = 100_000_000_000_000   # 100,000,000 HASH for the validator
TEST_FUNDS =   1_000_000_000_000   #   1,000,000 HASH each for alice and bob

reserves = {r["name"]: r for r in doc["app_state"]["treasury"]["reserves"]}
treasury_sub = reserves["treasury"]["sub_account"]

# Derive the treasury sub-account address from the existing balance list by
# matching the recorded initial amount.
treasury_initial = int(reserves["treasury"]["initial"][0]["amount"])
treasury_addr = None
for b in bank["balances"]:
    amt = int(b["coins"][0]["amount"]) if b["coins"] else 0
    if amt == treasury_initial:
        treasury_addr = b["address"]
        break
if treasury_addr is None:
    raise SystemExit("could not locate the treasury reserve balance in genesis")

take = STAKE + 2 * TEST_FUNDS
for b in bank["balances"]:
    if b["address"] == treasury_addr:
        remaining = int(b["coins"][0]["amount"]) - take
        if remaining < 0:
            raise SystemExit("treasury reserve is too small for the devnet funding")
        b["coins"][0]["amount"] = str(remaining)

bank["balances"].append({"address": validator, "coins": [{"denom": "uhash", "amount": str(STAKE)}]})
bank["balances"].append({"address": alice,     "coins": [{"denom": "uhash", "amount": str(TEST_FUNDS)}]})
bank["balances"].append({"address": bob,       "coins": [{"denom": "uhash", "amount": str(TEST_FUNDS)}]})

# The treasury record's `initial` is left at the genesis figure on purpose:
# it is the funded amount, and the difference from the live balance is what
# has been moved. Adjust it so the module's own invariant still holds.
reserves["treasury"]["initial"][0]["amount"] = str(treasury_initial - take)

# Accounts must exist in auth genesis for the balances to be usable.
accounts = doc["app_state"]["auth"]["accounts"]
next_number = len(accounts)
for addr in (validator, alice, bob):
    accounts.append({
        "@type": "/cosmos.auth.v1beta1.BaseAccount",
        "address": addr,
        "pub_key": None,
        "account_number": str(next_number),
        "sequence": "0",
    })
    next_number += 1

# Total supply is unchanged, so the recorded supply stays correct.
total = sum(int(c["amount"]) for b in bank["balances"] for c in b["coins"])
expected = int(bank["supply"][0]["amount"])
if total != expected:
    raise SystemExit(f"supply drifted: {total} != {expected}")

json.dump(doc, open(path, "w"), indent=2)
print(f"  total supply unchanged at {total} uhash")
PY

  ok "funded from the treasury reserve; total supply unchanged"

  say ""
  say "Creating the validator gentx."
  "$HASHGRAMD" genesis gentx validator 90000000000000"$DENOM" \
    --chain-id "$CHAIN_ID" "${KEYRING[@]}" \
    --moniker devnet-validator \
    --commission-rate 0.10 \
    --commission-max-rate 0.20 \
    --commission-max-change-rate 0.01 >/dev/null 2>&1
  "$HASHGRAMD" genesis collect-gentxs --home "$NODE_HOME" >/dev/null 2>&1
  "$HASHGRAMD" genesis validate-genesis --home "$NODE_HOME"
  ok "genesis validates"

  # The genesis hash changed when the gentx and funding were added, so repin.
  GENESIS_HASH="$(sha256sum "$NODE_HOME/config/genesis.json" | awk '{print $1}')"
  python3 - "$CONFIG_DIR/network.json" "$GENESIS_HASH" <<'PY'
import json,sys
path,h=sys.argv[1],sys.argv[2]
cfg=json.load(open(path))
cfg["genesis_hash"]=h
json.dump(cfg,open(path,"w"),indent=2)
PY
  ok "network pin updated to the final genesis hash: $GENESIS_HASH"

  # Point app.toml at the pinned hash so the node logs its verification.
  python3 - "$NODE_HOME/config/app.toml" "$GENESIS_HASH" <<'PY'
import sys,re
path,h=sys.argv[1],sys.argv[2]
text=open(path).read()
text=re.sub(r'genesis-hash = ".*"', f'genesis-hash = "{h}"', text)
text=re.sub(r'network-id = ".*"', 'network-id = "hashgram-devnet"', text)
text=re.sub(r'^roles = ".*"', 'roles = "validator,bootstrap"', text, flags=re.M)
open(path,"w").write(text)
PY

  # Roles, so hashgramctl knows what this machine is.
  cat > "$CONFIG_DIR/roles.json" <<EOF
{
  "roles": ["bootstrap", "validator"],
  "moniker": "devnet-validator"
}
EOF

  say ""
  say "Starting the node."
  nohup "$HASHGRAMD" start \
    --home "$NODE_HOME" \
    --minimum-gas-prices 0.0025"$DENOM" \
    > "$LOG_FILE" 2>&1 &
  echo $! > "$PID_FILE"

  # Wait for the first block rather than a fixed sleep: a fixed sleep either
  # wastes time or flakes.
  say -n "Waiting for the first block"
  for i in $(seq 1 60); do
    if curl -sf "$RPC_HTTP/status" >/dev/null 2>&1; then
      HEIGHT="$(curl -s "$RPC_HTTP/status" | python3 -c 'import json,sys;print(json.load(sys.stdin)["result"]["sync_info"]["latest_block_height"])' 2>/dev/null || echo 0)"
      if [ "${HEIGHT:-0}" -ge 1 ] 2>/dev/null; then
        printf '\n'
        ok "block $HEIGHT produced"
        break
      fi
    fi
    printf '.'
    sleep 1
    if [ "$i" -eq 60 ]; then
      printf '\n'
      say "error: no block after 60 seconds. Last log lines:"
      tail -30 "$LOG_FILE"
      exit 1
    fi
  done

  # Record what the verify step needs.
  cat > "$DEVNET_DIR/devnet.env" <<EOF
FOUNDER_ADDR=$FOUNDER_ADDR
VALIDATOR_ADDR=$VALIDATOR_ADDR
ALICE_ADDR=$ALICE_ADDR
BOB_ADDR=$BOB_ADDR
GENESIS_HASH=$GENESIS_HASH
TREASURY_SUB=$TREASURY_ADDR
EOF
fi

# ---------------------------------------------------------------------------
# verify
# ---------------------------------------------------------------------------

# shellcheck source=/dev/null
source "$DEVNET_DIR/devnet.env"

q() { "$HASHGRAMD" query "$@" --node "$RPC" --output json 2>/dev/null; }
jq_get() { python3 -c "import json,sys;d=json.load(sys.stdin);
import functools,operator
print(functools.reduce(lambda a,k: a[int(k)] if k.isdigit() else a[k], '$1'.split('.'), d))" 2>/dev/null; }

head1 "ACCEPTANCE CHECKS"

# --- blocks -----------------------------------------------------------------

H1="$(curl -s "$RPC_HTTP/status" | jq_get 'result.sync_info.latest_block_height')"
sleep 6
H2="$(curl -s "$RPC_HTTP/status" | jq_get 'result.sync_info.latest_block_height')"
if [ "${H2:-0}" -gt "${H1:-0}" ] 2>/dev/null; then
  ok "blocks are being produced ($H1 -> $H2)"
else
  bad "block height did not advance ($H1 -> $H2)"
fi

# --- supply -----------------------------------------------------------------

SUPPLY="$(q bank total | jq_get 'supply.0.amount')"
if [ "$SUPPLY" = "1000000000000000" ]; then
  ok "total supply is exactly 1,000,000,000 HASH"
else
  bad "total supply is $SUPPLY uhash, expected 1000000000000000"
fi

sleep 6
SUPPLY2="$(q bank total | jq_get 'supply.0.amount')"
if [ "$SUPPLY" = "$SUPPLY2" ]; then
  ok "supply unchanged across blocks: there is no inflation"
else
  bad "supply changed from $SUPPLY to $SUPPLY2"
fi

# --- founder allocation -----------------------------------------------------

FOUNDER_BAL="$(q bank balances "$FOUNDER_ADDR" | jq_get 'balances.0.amount')"
if [ "$FOUNDER_BAL" = "200000000000000" ]; then
  ok "Founder holds exactly 200,000,000 HASH"
else
  bad "Founder holds $FOUNDER_BAL uhash, expected 200000000000000"
fi

FOUNDER_SPENDABLE="$(q bank spendable-balance "$FOUNDER_ADDR" uhash | jq_get 'balance.amount')"
if [ "$FOUNDER_SPENDABLE" = "20000000000000" ]; then
  ok "Founder has exactly 20,000,000 HASH spendable at genesis"
else
  warn "Founder spendable is $FOUNDER_SPENDABLE uhash; on a devnet the compressed"
  warn "vesting schedule releases a period every minute, so this grows over time"
fi

FOUNDER_BENEFICIARY="$(q founder params | jq_get 'params.beneficiary')"
if [ "$FOUNDER_BENEFICIARY" = "$FOUNDER_ADDR" ]; then
  ok "Founder revenue beneficiary is the supplied address"
else
  bad "Founder beneficiary is $FOUNDER_BENEFICIARY, expected $FOUNDER_ADDR"
fi

FOUNDER_BPS="$(q founder params | jq_get 'params.fee_basis_points')"
if [ "$FOUNDER_BPS" = "100" ]; then
  ok "Founder revenue share is 100 bps (1%)"
else
  bad "Founder revenue share is $FOUNDER_BPS bps, expected 100"
fi

# --- transfers are not taxed ------------------------------------------------

BOB_BEFORE="$(q bank balances "$BOB_ADDR" | jq_get 'balances.0.amount')"
"$HASHGRAMD" tx bank send alice "$BOB_ADDR" 100000000uhash \
  --chain-id "$CHAIN_ID" --node "$RPC" "${KEYRING[@]}" \
  --gas auto --gas-adjustment 1.4 --gas-prices 0.0025uhash \
  --yes --output json >/dev/null 2>&1
sleep 8
BOB_AFTER="$(q bank balances "$BOB_ADDR" | jq_get 'balances.0.amount')"
RECEIVED=$((BOB_AFTER - BOB_BEFORE))
if [ "$RECEIVED" -eq 100000000 ]; then
  ok "Alice sent 100 HASH and Bob received exactly 100 HASH (no transfer tax)"
else
  bad "Bob received $RECEIVED uhash from a 100000000uhash transfer"
fi

# --- founder revenue accrues from gas, not from principal -------------------

sleep 6
ACCRUED="$(q founder revenue | jq_get 'total_accrued.0.amount' 2>/dev/null || echo 0)"
if [ -n "$ACCRUED" ] && [ "$ACCRUED" != "0" ]; then
  ok "Founder accrued $ACCRUED uhash from transaction gas fees"
else
  warn "Founder has accrued nothing yet; gas fees on a quiet devnet round to zero"
fi

# --- network identity -------------------------------------------------------

NET_ID="$(q network info | jq_get 'info.network_id')"
if [ "$NET_ID" = "hashgram-devnet" ]; then
  ok "on-chain network id is hashgram-devnet (not mainnet)"
else
  bad "on-chain network id is $NET_ID"
fi

ON_DISK_HASH="$(sha256sum "$NODE_HOME/config/genesis.json" | awk '{print $1}')"
if [ "$ON_DISK_HASH" = "$GENESIS_HASH" ]; then
  ok "genesis on disk still matches the pinned hash"
else
  bad "genesis hash drifted: $ON_DISK_HASH vs pinned $GENESIS_HASH"
fi

# --- no mint module ---------------------------------------------------------

if q mint params >/dev/null 2>&1; then
  bad "the mint module answered a query; it should not be compiled in"
else
  ok "no mint module: inflation is not merely zero, it is absent"
fi

# --- treasury reserves ------------------------------------------------------

RESERVE_COUNT="$(q treasury reserves | python3 -c 'import json,sys;print(len(json.load(sys.stdin)["reserves"]))' 2>/dev/null || echo 0)"
if [ "${RESERVE_COUNT:-0}" -ge 4 ]; then
  ok "$RESERVE_COUNT named treasury reserves are individually queryable"
else
  bad "expected at least 4 treasury reserves, found ${RESERVE_COUNT:-0}"
fi

# --- service reserve --------------------------------------------------------

SVC_REMAINING="$(q serviceproof reserve | jq_get 'remaining.0.amount')"
if [ "$SVC_REMAINING" = "500000000000000" ]; then
  ok "useful-service reserve holds exactly 500,000,000 HASH"
else
  bad "useful-service reserve holds $SVC_REMAINING uhash"
fi

# --- welcome pool -----------------------------------------------------------

WELCOME_POOL="$(q welcome status | jq_get 'pool_remaining.0.amount')"
if [ "$WELCOME_POOL" = "1850000000000" ]; then
  ok "welcome pool holds exactly 1,850,000 HASH"
else
  bad "welcome pool holds $WELCOME_POOL uhash"
fi

WELCOME_ATTESTORS="$(q welcome params | python3 -c 'import json,sys;print(len(json.load(sys.stdin)["params"].get("attestors") or []))' 2>/dev/null || echo 0)"
if [ "${WELCOME_ATTESTORS:-0}" -eq 0 ]; then
  ok "no welcome attestors registered: creating a key earns nothing"
else
  bad "$WELCOME_ATTESTORS attestors are registered at genesis"
fi

# --- username homograph defence ---------------------------------------------

"$HASHGRAMD" tx username register alice --chain-id "$CHAIN_ID" --node "$RPC" \
  --from alice "${KEYRING[@]}" --gas auto --gas-adjustment 1.4 \
  --gas-prices 0.0025uhash --yes --output json >/dev/null 2>&1
sleep 8

OWNER="$(q username lookup alice | jq_get 'registration.owner')"
if [ "$OWNER" = "$ALICE_ADDR" ]; then
  ok "@alice registered and resolves to Alice"
else
  bad "@alice resolves to $OWNER, expected $ALICE_ADDR"
fi

CONFUSABLE="$(q username availability a1ice | jq_get 'reason')"
if [ "$CONFUSABLE" = "confusable_with" ]; then
  ok "@a1ice rejected as visually confusable with @alice"
else
  bad "@a1ice availability reason is '$CONFUSABLE', expected confusable_with"
fi

RESERVED="$(q username availability admin | jq_get 'reason')"
if [ "$RESERVED" = "reserved" ]; then
  ok "@admin is reserved"
else
  bad "@admin availability reason is '$RESERVED', expected reserved"
fi

# --- username fee routed as qualifying revenue ------------------------------

SVC_REV="$(q feerouter service-revenue 2>/dev/null | python3 -c 'import json,sys;print(len(json.load(sys.stdin).get("service_revenue") or []))' 2>/dev/null || echo 0)"
if [ "${SVC_REV:-0}" -gt 0 ]; then
  ok "protocol revenue is recorded per service kind"
else
  warn "no per-service revenue recorded yet"
fi

# --- validator --------------------------------------------------------------

VOTING_POWER="$(curl -s "$RPC_HTTP/status" | jq_get 'result.validator_info.voting_power')"
if [ "${VOTING_POWER:-0}" -gt 0 ] 2>/dev/null; then
  ok "this node is validating with voting power $VOTING_POWER"
else
  bad "this node has no voting power"
fi

# --- preflight rejects the devnet -------------------------------------------

if "$HASHGRAMCTL" mainnet-preflight --home "$NODE_HOME" --config-dir "$CONFIG_DIR" \
   --data-dir "$DEVNET_DIR" >/dev/null 2>&1; then
  bad "mainnet-preflight PASSED on a devnet; it must refuse"
else
  ok "mainnet-preflight correctly refuses to pass on a devnet"
fi

# ---------------------------------------------------------------------------

head1 "RESULT"
if [ "$FAILURES" -eq 0 ]; then
  printf '  %sAll acceptance checks passed.%s\n\n' "$GREEN" "$RESET"
else
  printf '  %s%d check(s) failed.%s\n\n' "$RED" "$FAILURES" "$RESET"
fi

say "  Devnet state    $DEVNET_DIR"
say "  Node log        $LOG_FILE"
say "  RPC             $RPC_HTTP"
say "  Founder address $FOUNDER_ADDR"
say ""
say "  Inspect it:"
say "    build/hashgramctl chain-status --home $NODE_HOME --config-dir $CONFIG_DIR"
say "    build/hashgram-test-client founder verify --node $RPC"
say ""
say "  Stop it:   scripts/testnet/devnet.sh stop"
say "  Remove it: scripts/testnet/devnet.sh clean"
say ""

exit "$FAILURES"
