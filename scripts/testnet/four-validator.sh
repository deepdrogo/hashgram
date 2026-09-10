#!/usr/bin/env bash
#
# Four-validator local testnet, and the resilience test that matters.
#
# The point is not to show four processes running. It is to answer one
# question with evidence: does the network keep producing blocks when a
# validator dies? A single-validator chain stops. A four-validator chain with
# no validator holding more than a third of the stake does not.
#
#   scripts/testnet/four-validator.sh          create, start, verify
#   scripts/testnet/four-validator.sh stop     stop, keep the state
#   scripts/testnet/four-validator.sh clean    stop and delete the state
#
# Hooks for the Phase 2 acceptance (scripts/testnet/phase2.sh):
#   EXTRA_GENESIS_ACCOUNTS="addr:amount addr:amount"  fund more accounts
#   EXTRA_GENESIS_PY=/path/to/edit.py                 python applied to genesis.json
#   SKIP_FORK_TESTS=1                                 stop after the resilience test
#
# The validators run on one host with distinct ports, which tests consensus
# and gossip but not network partitions or independent operators. That
# limitation is real and is stated in the output rather than glossed over.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

TESTNET_DIR="${TESTNET_DIR:-$REPO_ROOT/testnet-data}"
HASHGRAMD="$REPO_ROOT/build/hashgramd"
HASHGRAMCTL="$REPO_ROOT/build/hashgramctl"
CHAIN_ID="hashgram-devnet-1"
DENOM="uhash"
N=4

# Port layout: validator i uses 26640+10i for P2P and 26641+10i for RPC.
p2p_port()  { echo $((26640 + 10 * $1)); }
rpc_port()  { echo $((26641 + 10 * $1)); }
grpc_port() { echo $((26642 + 10 * $1)); }
api_port()  { echo $((26643 + 10 * $1)); }
prom_port() { echo $((26644 + 10 * $1)); }

home_of() { echo "$TESTNET_DIR/validator$1"; }
rpc_of()  { echo "http://127.0.0.1:$(rpc_port "$1")"; }

if [ -t 1 ]; then
  BOLD=$'\033[1m'; GREEN=$'\033[32m'; RED=$'\033[31m'; YELLOW=$'\033[33m'; RESET=$'\033[0m'
else
  BOLD=""; GREEN=""; RED=""; YELLOW=""; RESET=""
fi

say()   { printf '%s\n' "$*"; }
head1() { printf '\n%s%s%s\n%s\n' "$BOLD" "$*" "$RESET" "$(printf '=%.0s' {1..74})"; }
ok()    { printf '  %s[ ok ]%s %s\n' "$GREEN" "$RESET" "$*"; }
bad()   { printf '  %s[FAIL]%s %s\n' "$RED" "$RESET" "$*"; FAILURES=$((FAILURES+1)); }
warn()  { printf '  %s[warn]%s %s\n' "$YELLOW" "$RESET" "$*"; }

FAILURES=0

height_of() {
  curl -s --max-time 3 "$(rpc_of "$1")/status" 2>/dev/null \
    | python3 -c 'import json,sys;print(json.load(sys.stdin)["result"]["sync_info"]["latest_block_height"])' 2>/dev/null \
    || echo 0
}

stop_all() {
  for i in $(seq 0 $((N - 1))); do
    local pidfile="$(home_of "$i")/node.pid"
    if [ -f "$pidfile" ]; then
      local pid; pid="$(cat "$pidfile")"
      if kill -0 "$pid" 2>/dev/null; then
        kill "$pid" 2>/dev/null || true
        for _ in $(seq 1 20); do kill -0 "$pid" 2>/dev/null || break; sleep 0.5; done
        kill -9 "$pid" 2>/dev/null || true
      fi
      rm -f "$pidfile"
    fi
  done
  pkill -f "hashgramd start --home $TESTNET_DIR" 2>/dev/null || true
}

case "${1:-up}" in
  stop)  stop_all; say "stopped; state left in $TESTNET_DIR"; exit 0 ;;
  clean) stop_all; rm -rf "$TESTNET_DIR"; say "stopped and $TESTNET_DIR removed"; exit 0 ;;
  up|"") ;;
  *) say "usage: $0 [up|stop|clean]"; exit 1 ;;
esac

for required in "$HASHGRAMD" "$HASHGRAMCTL"; do
  if [ ! -x "$required" ]; then
    say "error: $required is missing. Run: make build"
    exit 1
  fi
done

head1 "FOUR-VALIDATOR TESTNET"

stop_all
rm -rf "$TESTNET_DIR"
mkdir -p "$TESTNET_DIR"

# ---------------------------------------------------------------------------
# Initialise
# ---------------------------------------------------------------------------

say ""
say "Initialising $N validators."

for i in $(seq 0 $((N - 1))); do
  home="$(home_of "$i")"
  "$HASHGRAMD" init "validator$i" --chain-id "$CHAIN_ID" \
    --home "$home" --default-denom "$DENOM" >/dev/null 2>&1
  "$HASHGRAMD" keys add "validator$i" --keyring-backend test --home "$home" >/dev/null 2>&1
  ok "validator$i initialised"
done

# Validator 0's genesis is the source of truth; the others receive a copy.
GENESIS_HOME="$(home_of 0)"

say ""
say "Building genesis with four equal validators."
say "Equal stake matters: with 25% each, no single validator can halt the"
say "chain by leaving, which is the property this test exists to demonstrate."

# Each validator gets stake plus a working balance. Equal stakes so that
# losing one leaves 75% of voting power, comfortably above the 66.7%
# CometBFT needs to commit a block.
STAKE=50000000000000      #  50,000,000 HASH staked per validator
SPARE=1000000000000       #   1,000,000 HASH spare per validator

for i in $(seq 0 $((N - 1))); do
  addr="$("$HASHGRAMD" keys show "validator$i" -a --keyring-backend test --home "$(home_of "$i")")"
  echo "$addr" > "$TESTNET_DIR/validator$i.addr"
  "$HASHGRAMD" genesis add-genesis-account "$addr" "$((STAKE + SPARE))$DENOM" \
    --home "$GENESIS_HOME" >/dev/null 2>&1
done
for entry in ${EXTRA_GENESIS_ACCOUNTS:-}; do
  "$HASHGRAMD" genesis add-genesis-account "${entry%%:*}" "${entry##*:}$DENOM" \
    --home "$GENESIS_HOME" >/dev/null 2>&1 && ok "extra genesis account ${entry%%:*}"
done
ok "four genesis accounts funded"

# Devnet denominations throughout, and short governance periods so a
# proposal can actually be exercised in a test run.
python3 - "$GENESIS_HOME/config/genesis.json" <<'PY'
import json,sys
p=sys.argv[1]
d=json.load(open(p))
s=d["app_state"]
s["staking"]["params"]["bond_denom"]="uhash"
s["staking"]["params"]["unbonding_time"]="600s"
# The expedited deposit must exceed the ordinary one; the SDK validates this
# at gentx time and refuses if they are equal, which is how this script's
# first draft failed.
s["gov"]["params"]["min_deposit"]=[{"denom":"uhash","amount":"10000000"}]
if "expedited_min_deposit" in s["gov"]["params"]:
    s["gov"]["params"]["expedited_min_deposit"]=[{"denom":"uhash","amount":"20000000"}]
s["gov"]["params"]["voting_period"]="120s"
s["gov"]["params"]["max_deposit_period"]="120s"
# The expedited voting period must be strictly shorter than the regular one.
# Shortening only the regular period leaves the default 24h expedited period
# longer than it, which the SDK rejects.
if "expedited_voting_period" in s["gov"]["params"]:
    s["gov"]["params"]["expedited_voting_period"]="60s"
json.dump(d,open(p,"w"),indent=2)
PY
ok "denominations and governance periods set for a test run"

if [ -n "${EXTRA_GENESIS_PY:-}" ] && [ -f "$EXTRA_GENESIS_PY" ]; then
  python3 "$EXTRA_GENESIS_PY" "$GENESIS_HOME/config/genesis.json"
  ok "extra genesis edits applied from $EXTRA_GENESIS_PY"
fi

# Each validator signs its own gentx in its own home, then all gentxs are
# collected into validator 0's genesis.
mkdir -p "$GENESIS_HOME/config/gentx"
for i in $(seq 0 $((N - 1))); do
  home="$(home_of "$i")"
  if [ "$i" -ne 0 ]; then
    cp "$GENESIS_HOME/config/genesis.json" "$home/config/genesis.json"
  fi
  "$HASHGRAMD" genesis gentx "validator$i" "$STAKE$DENOM" \
    --chain-id "$CHAIN_ID" --keyring-backend test --home "$home" \
    --moniker "validator$i" \
    --commission-rate 0.10 --commission-max-rate 0.20 \
    --commission-max-change-rate 0.01 \
    --ip 127.0.0.1 --p2p-port "$(p2p_port "$i")" >/dev/null 2>&1
  if [ "$i" -ne 0 ]; then
    cp "$home"/config/gentx/*.json "$GENESIS_HOME/config/gentx/"
  fi
done

"$HASHGRAMD" genesis collect-gentxs --home "$GENESIS_HOME" >/dev/null 2>&1
"$HASHGRAMD" genesis validate-genesis --home "$GENESIS_HOME"
ok "four gentxs collected, genesis validates"

GENESIS_HASH="$(sha256sum "$GENESIS_HOME/config/genesis.json" | awk '{print $1}')"
ok "genesis hash: $GENESIS_HASH"

# Every validator must have the identical genesis file. If they differ, they
# are on different networks and will refuse each other.
for i in $(seq 1 $((N - 1))); do
  cp "$GENESIS_HOME/config/genesis.json" "$(home_of "$i")/config/genesis.json"
done
ok "genesis distributed to all validators"

# ---------------------------------------------------------------------------
# Configure peers and ports
# ---------------------------------------------------------------------------

say ""
say "Wiring peers."

declare -a NODE_IDS
for i in $(seq 0 $((N - 1))); do
  NODE_IDS[$i]="$("$HASHGRAMD" comet show-node-id --home "$(home_of "$i")" 2>/dev/null)"
done

for i in $(seq 0 $((N - 1))); do
  home="$(home_of "$i")"
  peers=""
  for j in $(seq 0 $((N - 1))); do
    [ "$i" -eq "$j" ] && continue
    [ -n "$peers" ] && peers="$peers,"
    peers="$peers${NODE_IDS[$j]}@127.0.0.1:$(p2p_port "$j")"
  done

  python3 - "$home/config/config.toml" "$peers" "$(p2p_port "$i")" "$(rpc_port "$i")" "$(prom_port "$i")" <<'PY'
import re,sys
path,peers,p2p,rpc,prom = sys.argv[1:6]
t=open(path).read()
t=re.sub(r'^persistent_peers = ".*"', f'persistent_peers = "{peers}"', t, flags=re.M)
t=re.sub(r'^laddr = "tcp://0\.0\.0\.0:26656"', f'laddr = "tcp://0.0.0.0:{p2p}"', t, flags=re.M)
t=re.sub(r'^laddr = "tcp://127\.0\.0\.1:26657"', f'laddr = "tcp://127.0.0.1:{rpc}"', t, flags=re.M)
t=re.sub(r'^prometheus_listen_addr = ".*"', f'prometheus_listen_addr = "127.0.0.1:{prom}"', t, flags=re.M)
# All four validators share one host, so the same-IP guard has to be off.
# On a real network it must stay on: it is what stops one operator filling
# a node's peer slots from a single machine.
t=re.sub(r'^allow_duplicate_ip = false', 'allow_duplicate_ip = true', t, flags=re.M)
t=re.sub(r'^addr_book_strict = true', 'addr_book_strict = false', t, flags=re.M)
open(path,"w").write(t)
PY

  python3 - "$home/config/app.toml" "$(grpc_port "$i")" "$(api_port "$i")" "$GENESIS_HASH" <<'PY'
import re,sys
path,grpc,api,ghash = sys.argv[1:5]
t=open(path).read()
t=re.sub(r'^address = "127\.0\.0\.1:909[01]"', f'address = "127.0.0.1:{grpc}"', t, flags=re.M)
t=re.sub(r'^address = "tcp://127\.0\.0\.1:1317"', f'address = "tcp://127.0.0.1:{api}"', t, flags=re.M)
t=re.sub(r'genesis-hash = ".*"', f'genesis-hash = "{ghash}"', t)
t=re.sub(r'network-id = ".*"', 'network-id = "hashgram-devnet"', t)
open(path,"w").write(t)
PY
  ok "validator$i  p2p $(p2p_port "$i")  rpc $(rpc_port "$i")  id ${NODE_IDS[$i]:0:12}..."
done

# ---------------------------------------------------------------------------
# Start
# ---------------------------------------------------------------------------

say ""
say "Starting all four validators."

for i in $(seq 0 $((N - 1))); do
  home="$(home_of "$i")"
  nohup "$HASHGRAMD" start --home "$home" \
    --minimum-gas-prices "0.0025$DENOM" \
    > "$home/node.log" 2>&1 &
  echo $! > "$home/node.pid"
done

printf '  waiting for the first block'
for attempt in $(seq 1 90); do
  h="$(height_of 0)"
  if [ "${h:-0}" -ge 2 ] 2>/dev/null; then
    printf '\n'
    ok "block $h produced"
    break
  fi
  printf '.'
  sleep 1
  if [ "$attempt" -eq 90 ]; then
    printf '\n'
    bad "no blocks after 90 seconds"
    say ""
    say "validator0 log tail:"
    tail -25 "$(home_of 0)/node.log"
    exit 1
  fi
done

# ---------------------------------------------------------------------------
# Verify
# ---------------------------------------------------------------------------

head1 "ACCEPTANCE CHECKS"

q0() { "$HASHGRAMD" query "$@" --node "tcp://127.0.0.1:$(rpc_port 0)" --output json 2>/dev/null; }

# --- peer discovery ---------------------------------------------------------

sleep 5
ALL_PEERED=1
for i in $(seq 0 $((N - 1))); do
  peers="$(curl -s --max-time 3 "$(rpc_of "$i")/net_info" 2>/dev/null \
    | python3 -c 'import json,sys;print(json.load(sys.stdin)["result"]["n_peers"])' 2>/dev/null || echo 0)"
  if [ "${peers:-0}" -ge 3 ] 2>/dev/null; then
    ok "validator$i sees $peers peers"
  else
    bad "validator$i sees only ${peers:-0} peers, expected 3"
    ALL_PEERED=0
  fi
done
[ "$ALL_PEERED" -eq 1 ] && ok "peer discovery works: every validator found the other three"

# --- block propagation ------------------------------------------------------

sleep 6
declare -a HEIGHTS
SPREAD_OK=1
for i in $(seq 0 $((N - 1))); do
  HEIGHTS[$i]="$(height_of "$i")"
done
MIN=${HEIGHTS[0]}; MAX=${HEIGHTS[0]}
for i in $(seq 0 $((N - 1))); do
  [ "${HEIGHTS[$i]}" -lt "$MIN" ] && MIN=${HEIGHTS[$i]}
  [ "${HEIGHTS[$i]}" -gt "$MAX" ] && MAX=${HEIGHTS[$i]}
done
if [ $((MAX - MIN)) -le 2 ]; then
  ok "block propagation: all four within 2 blocks (${HEIGHTS[0]}, ${HEIGHTS[1]}, ${HEIGHTS[2]}, ${HEIGHTS[3]})"
else
  bad "heights spread by $((MAX - MIN)) blocks: ${HEIGHTS[*]}"
  SPREAD_OK=0
fi

# --- validator set ----------------------------------------------------------

BONDED="$(q0 staking validators | python3 -c '
import json,sys
d=json.load(sys.stdin)
print(sum(1 for v in d["validators"] if v["status"]=="BOND_STATUS_BONDED"))' 2>/dev/null || echo 0)"
if [ "${BONDED:-0}" -eq 4 ]; then
  ok "four bonded validators"
else
  bad "expected 4 bonded validators, found ${BONDED:-0}"
fi

# --- supply -----------------------------------------------------------------

SUPPLY="$(q0 bank total | python3 -c 'import json,sys;print(json.load(sys.stdin)["supply"][0]["amount"])' 2>/dev/null)"
sleep 6
SUPPLY2="$(q0 bank total | python3 -c 'import json,sys;print(json.load(sys.stdin)["supply"][0]["amount"])' 2>/dev/null)"
if [ "$SUPPLY" = "$SUPPLY2" ]; then
  ok "supply unchanged across blocks with four validators: no inflation"
else
  bad "supply changed from $SUPPLY to $SUPPLY2"
fi

# --- delegation -------------------------------------------------------------

V1_OPER="$(q0 staking validators | python3 -c '
import json,sys
d=json.load(sys.stdin)
vals=sorted(d["validators"], key=lambda v: v["operator_address"])
print(vals[0]["operator_address"])' 2>/dev/null)"

if [ -n "$V1_OPER" ]; then
  DELEGATOR_HOME="$(home_of 3)"
  BEFORE="$(q0 staking validators | python3 -c "
import json,sys
d=json.load(sys.stdin)
for v in d['validators']:
    if v['operator_address']=='$V1_OPER': print(v['tokens'])" 2>/dev/null)"

  "$HASHGRAMD" tx staking delegate "$V1_OPER" 1000000000"$DENOM" \
    --from validator3 --chain-id "$CHAIN_ID" \
    --node "tcp://127.0.0.1:$(rpc_port 0)" \
    --keyring-backend test --home "$DELEGATOR_HOME" \
    --gas auto --gas-adjustment 1.4 --gas-prices 0.0025"$DENOM" \
    --yes --output json >/dev/null 2>&1
  sleep 8

  AFTER="$(q0 staking validators | python3 -c "
import json,sys
d=json.load(sys.stdin)
for v in d['validators']:
    if v['operator_address']=='$V1_OPER': print(v['tokens'])" 2>/dev/null)"

  if [ -n "$BEFORE" ] && [ -n "$AFTER" ] && [ "$AFTER" -gt "$BEFORE" ] 2>/dev/null; then
    ok "delegation increased validator stake from $BEFORE to $AFTER"
  else
    bad "delegation did not change stake ($BEFORE -> $AFTER)"
  fi
fi

# --- THE RESILIENCE TEST ----------------------------------------------------

head1 "RESILIENCE: KILL A VALIDATOR"

say ""
say "Stopping validator3. With four equal validators, 75% of voting power"
say "remains, which is above the two-thirds CometBFT needs to commit. The"
say "chain should keep producing blocks."
say ""

KILL_PID="$(cat "$(home_of 3)/node.pid")"
kill "$KILL_PID" 2>/dev/null || true
for _ in $(seq 1 20); do kill -0 "$KILL_PID" 2>/dev/null || break; sleep 0.5; done
rm -f "$(home_of 3)/node.pid"
ok "validator3 stopped"

BEFORE_KILL="$(height_of 0)"
say ""
printf '  watching for 20 seconds'
for _ in $(seq 1 20); do printf '.'; sleep 1; done
printf '\n'
AFTER_KILL="$(height_of 0)"

PRODUCED=$((AFTER_KILL - BEFORE_KILL))
if [ "$PRODUCED" -ge 3 ]; then
  ok "chain continued: $PRODUCED blocks in 20 seconds ($BEFORE_KILL -> $AFTER_KILL)"
  ok "losing one of four validators does not stop the network"
else
  bad "only $PRODUCED blocks in 20 seconds after the kill ($BEFORE_KILL -> $AFTER_KILL)"
fi

# The surviving three must still agree with each other.
S1="$(height_of 0)"; S2="$(height_of 1)"; S3="$(height_of 2)"
SMIN=$S1; SMAX=$S1
for h in $S2 $S3; do
  [ "$h" -lt "$SMIN" ] && SMIN=$h
  [ "$h" -gt "$SMAX" ] && SMAX=$h
done
if [ $((SMAX - SMIN)) -le 2 ]; then
  ok "the three survivors remain in agreement ($S1, $S2, $S3)"
else
  bad "survivors diverged: $S1, $S2, $S3"
fi

# A transaction must still be processable with one validator down.
BOB="$(cat "$TESTNET_DIR/validator2.addr")"
TX_BEFORE="$(q0 bank balances "$BOB" | python3 -c 'import json,sys;b=json.load(sys.stdin)["balances"];print(b[0]["amount"] if b else 0)' 2>/dev/null)"
"$HASHGRAMD" tx bank send validator0 "$BOB" 5000000"$DENOM" \
  --chain-id "$CHAIN_ID" --node "tcp://127.0.0.1:$(rpc_port 0)" \
  --keyring-backend test --home "$(home_of 0)" \
  --gas auto --gas-adjustment 1.4 --gas-prices 0.0025"$DENOM" \
  --yes --output json >/dev/null 2>&1
# Poll rather than sleep a fixed time: on a two-core CI runner four validators
# on one host produce a block every 6-7 s, so a fixed 8 s wait is a coin flip.
# Ten blocks' worth is the bound; on a real host this returns in one block.
TX_AFTER="$TX_BEFORE"
for _ in $(seq 1 30); do
  TX_AFTER="$(q0 bank balances "$BOB" | python3 -c 'import json,sys;b=json.load(sys.stdin)["balances"];print(b[0]["amount"] if b else 0)' 2>/dev/null)"
  if [ "${TX_AFTER:-0}" -gt "${TX_BEFORE:-0}" ] 2>/dev/null; then break; fi
  sleep 2
done
if [ "$TX_AFTER" -gt "$TX_BEFORE" ] 2>/dev/null; then
  ok "transactions still process with one validator down"
else
  bad "transaction did not process ($TX_BEFORE -> $TX_AFTER)"
fi

# --- restart the killed validator and confirm it catches up -----------------

say ""
say "Restarting validator3 to confirm it rejoins and catches up."
home3="$(home_of 3)"
nohup "$HASHGRAMD" start --home "$home3" \
  --minimum-gas-prices "0.0025$DENOM" >> "$home3/node.log" 2>&1 &
echo $! > "$home3/node.pid"

printf '  waiting for validator3 to catch up'
TARGET="$(height_of 0)"
CAUGHT_UP=0
for _ in $(seq 1 60); do
  h3="$(height_of 3)"
  if [ "${h3:-0}" -ge "$TARGET" ] 2>/dev/null; then
    printf '\n'
    ok "validator3 rejoined and caught up to height $h3"
    CAUGHT_UP=1
    break
  fi
  printf '.'
  sleep 1
done
[ "$CAUGHT_UP" -eq 0 ] && { printf '\n'; bad "validator3 did not catch up to height $TARGET"; }

# --- FOREIGN FORK REJECTION -------------------------------------------------

if [ "${SKIP_FORK_TESTS:-0}" = "1" ]; then
  head1 "RESULT"
  if [ "$FAILURES" -eq 0 ]; then
    printf '  %sAll checks passed (fork isolation skipped by SKIP_FORK_TESTS).%s\n' "$GREEN" "$RESET"
  else
    printf '  %s%d check(s) failed.%s\n' "$RED" "$FAILURES" "$RESET"
  fi
  say "  State     $TESTNET_DIR"
  say "  RPC       $(rpc_of 0), $(rpc_of 1), $(rpc_of 2), $(rpc_of 3)"
  exit "$FAILURES"
fi

head1 "FORK ISOLATION"

say ""
say "Hashgram isolates foreign chains at three separate layers, and they"
say "are not interchangeable. This section tests each one, including the"
say "case where the weakest layer does not hold."
say ""
say "  1. CometBFT handshake  - compares chain_id only"
say "  2. Consensus           - validator set and app hash must match"
say "  3. Operator tooling    - hashgramctl pins the genesis hash"
say ""

# Builds an independent single-validator chain with its own genesis, points it
# at the real validators, and starts it. Ports are passed in so two forks can
# run without colliding.
#
# build_fork <name> <chain-id> <p2p> <rpc> <grpc> <api> <prom>
build_fork() {
  local name="$1" fchain="$2" p2p="$3" rpc="$4" grpc="$5" api="$6" prom="$7"
  local home="$TESTNET_DIR/$name"

  rm -rf "$home"
  "$HASHGRAMD" init "$name" --chain-id "$fchain" \
    --home "$home" --default-denom "$DENOM" >/dev/null 2>&1
  "$HASHGRAMD" keys add forker --keyring-backend test --home "$home" >/dev/null 2>&1
  local addr
  addr="$("$HASHGRAMD" keys show forker -a --keyring-backend test --home "$home")"

  python3 - "$home/config/genesis.json" <<'PY'
import json,sys
p=sys.argv[1]
d=json.load(open(p))
d["app_state"]["staking"]["params"]["bond_denom"]="uhash"
json.dump(d,open(p,"w"),indent=2)
PY
  "$HASHGRAMD" genesis add-genesis-account "$addr" 1000000000000"$DENOM" \
    --keyring-backend test --home "$home" >/dev/null 2>&1
  "$HASHGRAMD" genesis gentx forker 900000000000"$DENOM" --chain-id "$fchain" \
    --keyring-backend test --home "$home" \
    --ip 127.0.0.1 --p2p-port "$p2p" >/dev/null 2>&1
  "$HASHGRAMD" genesis collect-gentxs --home "$home" >/dev/null 2>&1

  local fpeers="${NODE_IDS[0]}@127.0.0.1:$(p2p_port 0),${NODE_IDS[1]}@127.0.0.1:$(p2p_port 1)"
  python3 - "$home/config/config.toml" "$fpeers" "$p2p" "$rpc" "$prom" <<'PY'
import re,sys
path,peers,p2p,rpc,prom=sys.argv[1:6]
t=open(path).read()
t=re.sub(r'^persistent_peers = ".*"', f'persistent_peers = "{peers}"', t, flags=re.M)
t=re.sub(r'^laddr = "tcp://0\.0\.0\.0:26656"', f'laddr = "tcp://0.0.0.0:{p2p}"', t, flags=re.M)
t=re.sub(r'^laddr = "tcp://127\.0\.0\.1:26657"', f'laddr = "tcp://127.0.0.1:{rpc}"', t, flags=re.M)
t=re.sub(r'^prometheus_listen_addr = ".*"', f'prometheus_listen_addr = "127.0.0.1:{prom}"', t, flags=re.M)
t=re.sub(r'^allow_duplicate_ip = false', 'allow_duplicate_ip = true', t, flags=re.M)
t=re.sub(r'^addr_book_strict = true', 'addr_book_strict = false', t, flags=re.M)
open(path,"w").write(t)
PY
  python3 - "$home/config/app.toml" "$grpc" "$api" <<'PY'
import re,sys
path,grpc,api=sys.argv[1:4]
t=open(path).read()
t=re.sub(r'^address = "127\.0\.0\.1:9090"', f'address = "127.0.0.1:{grpc}"', t, flags=re.M)
t=re.sub(r'^address = "tcp://127\.0\.0\.1:1317"', f'address = "tcp://127.0.0.1:{api}"', t, flags=re.M)
open(path,"w").write(t)
PY

  nohup "$HASHGRAMD" start --home "$home" \
    --minimum-gas-prices "0.0025$DENOM" > "$home/node.log" 2>&1 &
  local pid=$!
  echo $pid > "$home/node.pid"

  FORK_HOME="$home"
  FORK_PID="$pid"
  FORK_HASH="$(sha256sum "$home/config/genesis.json" | awk '{print $1}')"
}

stop_fork() {
  kill "$1" 2>/dev/null || true
  sleep 2
  kill -9 "$1" 2>/dev/null || true
}

peers_at() {
  curl -s --max-time 3 "http://127.0.0.1:$1/net_info" 2>/dev/null \
    | python3 -c 'import json,sys;print(json.load(sys.stdin)["result"]["n_peers"])' 2>/dev/null \
    || echo 0
}

app_hash_at() {
  curl -s --max-time 3 "http://127.0.0.1:$1/status" 2>/dev/null \
    | python3 -c 'import json,sys;print(json.load(sys.stdin)["result"]["sync_info"]["latest_app_hash"])' 2>/dev/null \
    || echo unknown
}

# --- Layer 1: a fork that also changes the chain id ------------------------

say "Layer 1. A fork under its own chain id, pointed at the real"
say "validators. CometBFT compares chain ids during the handshake, so this"
say "connection should never open."
say ""

build_fork "fork-alien" "hashgram-alienfork-1" 26700 26701 26702 26703 26704
ok "alien fork genesis hash: ${FORK_HASH:0:16}... (real: ${GENESIS_HASH:0:16}...)"
ALIEN_PID="$FORK_PID"
ALIEN_HOME="$FORK_HOME"

say "  giving it 20 seconds to try"
sleep 20

ALIEN_PEERS="$(peers_at 26701)"
if [ "${ALIEN_PEERS:-0}" -eq 0 ]; then
  ok "REJECTED at the handshake: the alien fork established 0 peers"
else
  bad "the alien fork established ${ALIEN_PEERS} peer(s); the chain id check failed"
fi

if grep -qiE "different network|wrong network|chain.?id" "$ALIEN_HOME/node.log" 2>/dev/null; then
  ok "the real validators name the reason in the alien fork's log"
else
  warn "no explicit chain id message logged; the peer count is the evidence"
fi

stop_fork "$ALIEN_PID"
rm -f "$ALIEN_HOME/node.pid"
ok "alien fork stopped"

# --- Layer 2: a fork that squats on the real chain id ----------------------

say ""
say "Layer 2. A harder case. This fork keeps the real chain id and changes"
say "only the genesis. CometBFT does not hash the genesis during the"
say "handshake, so the transport connection is expected to OPEN. What must"
say "hold is that it cannot join consensus or move the real chain."
say ""

REAL_HEIGHT_BEFORE="$(height_of 0)"
REAL_HASH_BEFORE="$(app_hash_at "$(rpc_port 0)")"

build_fork "fork-squatter" "$CHAIN_ID" 26710 26711 26712 26713 26714
if [ "$FORK_HASH" = "$GENESIS_HASH" ]; then
  bad "the squatting fork hashes identically to the testnet; the test is invalid"
else
  ok "squatting fork shares the chain id but not the genesis: ${FORK_HASH:0:16}..."
fi
SQUAT_PID="$FORK_PID"
SQUAT_HOME="$FORK_HOME"

say "  giving it 25 seconds to try"
sleep 25

SQUAT_PEERS="$(peers_at 26711)"
if [ "${SQUAT_PEERS:-0}" -gt 0 ]; then
  ok "as expected, the transport connected (${SQUAT_PEERS} peer(s)): chain id alone is not fork protection"
else
  ok "the transport did not connect (${SQUAT_PEERS} peers); stricter than required"
fi

# The decisive checks. The fork runs its own chain: its app hash at any height
# differs, and it can never make the real validators accept its blocks.
SQUAT_APP="$(app_hash_at 26711)"
REAL_APP="$(app_hash_at "$(rpc_port 0)")"
if [ "$SQUAT_APP" != "$REAL_APP" ] && [ "$SQUAT_APP" != "unknown" ]; then
  ok "the two chains never converge: app hash ${SQUAT_APP:0:16}... vs ${REAL_APP:0:16}..."
else
  bad "the fork's app hash matches the real chain; state was not isolated"
fi

# Every real validator must still agree with every other one.
REAL_APP_1="$(app_hash_at "$(rpc_port 1)")"
REAL_APP_2="$(app_hash_at "$(rpc_port 2)")"
REAL_APP_3="$(app_hash_at "$(rpc_port 3)")"
if [ "$REAL_APP" = "$REAL_APP_1" ] && [ "$REAL_APP" = "$REAL_APP_2" ] && [ "$REAL_APP" = "$REAL_APP_3" ]; then
  ok "all four real validators still share one app hash: the fork injected nothing"
else
  bad "the real validators disagree on app hash; the fork contaminated state"
fi

# And the real chain must have kept advancing on its own terms.
REAL_HEIGHT_AFTER="$(height_of 0)"
if [ "$REAL_HEIGHT_AFTER" -gt "$REAL_HEIGHT_BEFORE" ]; then
  ok "the real chain advanced ${REAL_HEIGHT_BEFORE} -> ${REAL_HEIGHT_AFTER} while the fork was connected"
else
  bad "the real chain stalled at ${REAL_HEIGHT_AFTER} while the fork was connected"
fi

# The fork's validator must not appear in the real validator set.
REAL_VALS="$(curl -s --max-time 3 "$(rpc_of 0)/validators" 2>/dev/null \
  | python3 -c 'import json,sys;print(len(json.load(sys.stdin)["result"]["validators"]))' 2>/dev/null || echo 0)"
if [ "${REAL_VALS:-0}" -eq 4 ]; then
  ok "the real validator set is still exactly 4: the fork's validator was never admitted"
else
  bad "the real validator set has ${REAL_VALS} members, expected 4"
fi

# --- Layer 3: operator tooling pins the genesis hash -----------------------

say ""
say "Layer 3. The layer that actually stops an operator from being fed a"
say "fork: hashgramctl refuses any genesis whose hash is not the pinned"
say "one, regardless of what chain id the file claims."
say ""

JOIN_HOME="$TESTNET_DIR/join-attempt"
rm -rf "$JOIN_HOME"
JOIN_OUT="$(
  "$HASHGRAMCTL" join-mainnet \
    --devnet \
    --genesis-file "$SQUAT_HOME/config/genesis.json" \
    --genesis-hash "$GENESIS_HASH" \
    --home "$JOIN_HOME" 2>&1
)" && JOIN_RC=0 || JOIN_RC=$?

if [ "$JOIN_RC" -ne 0 ]; then
  ok "hashgramctl refused the fork genesis against the pinned hash (exit ${JOIN_RC})"
  if printf '%s' "$JOIN_OUT" | grep -qiE 'hash|mismatch|does not match'; then
    ok "the refusal names the hash mismatch rather than failing vaguely"
  else
    warn "hashgramctl refused but did not mention the hash mismatch"
  fi
else
  bad "hashgramctl ACCEPTED a fork genesis under the real pinned hash"
fi

# The same command with the fork's own hash must also fail, because the pinned
# hash is what defines the network, not whatever the file happens to hash to.
rm -rf "$JOIN_HOME"
if "$HASHGRAMCTL" join-mainnet --devnet \
     --genesis-file "$SQUAT_HOME/config/genesis.json" \
     --genesis-hash "$FORK_HASH" \
     --home "$JOIN_HOME" >/dev/null 2>&1; then
  warn "a self-consistent fork genesis is accepted when its own hash is supplied; the pinned hash must come from an independent source"
else
  ok "even a self-consistent fork genesis is refused"
fi
rm -rf "$JOIN_HOME"

stop_fork "$SQUAT_PID"
rm -f "$SQUAT_HOME/node.pid"
ok "squatting fork stopped"

REAL_HEIGHT="$(height_of 0)"

# ---------------------------------------------------------------------------

head1 "RESULT"

if [ "$FAILURES" -eq 0 ]; then
  printf '  %sAll checks passed.%s\n' "$GREEN" "$RESET"
else
  printf '  %s%d check(s) failed.%s\n' "$RED" "$FAILURES" "$RESET"
fi

say ""
say "  What this demonstrated:"
say "    - four validators discover each other and agree on every block"
say "    - killing one leaves the chain producing blocks and processing"
say "      transactions, because 75% of voting power is above the two-thirds"
say "      CometBFT requires"
say "    - the killed validator rejoins and catches up"
say "    - a fork under its own chain id is refused at the CometBFT"
say "      handshake and never opens a connection"
say "    - a fork that squats on the real chain id does open a transport"
say "      connection, and still cannot join consensus, enter the validator"
say "      set, or move the real chain by a single block"
say "    - hashgramctl refuses a fork genesis against the pinned hash"
say ""
say "  What this did NOT demonstrate:"
say "    All four validators run on one host as one operating-system user."
say "    That tests consensus and gossip. It does not test network"
say "    partitions, independent operators, or geographic distribution, all"
say "    of which are what actually makes a network resilient. Four"
say "    validators on one VPS is still one VPS."
say ""
say "  A finding worth stating plainly:"
say "    CometBFT's handshake compares chain ids. It does not hash the"
say "    genesis file. A fork that keeps chain id ${CHAIN_ID}"
say "    therefore reaches the transport layer before consensus turns it"
say "    away. Nothing above is broken by that, but it is the reason the"
say "    pinned genesis hash in /etc/hashgram/network.json and the"
say "    genesis_hash field in the Hashgram P2P handshake exist, rather"
say "    than being treated as duplicated effort."
say ""
say "  State     $TESTNET_DIR"
say "  RPC       $(rpc_of 0), $(rpc_of 1), $(rpc_of 2), $(rpc_of 3)"
say "  Height    $REAL_HEIGHT"
say ""
say "  Stop:     scripts/testnet/four-validator.sh stop"
say "  Remove:   scripts/testnet/four-validator.sh clean"
say ""

exit "$FAILURES"
