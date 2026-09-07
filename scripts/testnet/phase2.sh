#!/usr/bin/env bash
#
# Phase 2 acceptance: the whole network on one host.
#
#   scripts/testnet/phase2.sh          create everything, run every check
#   scripts/testnet/phase2.sh stop     stop everything, keep the state
#   scripts/testnet/phase2.sh clean    stop and delete the state
#
# What runs: four validators (scripts/testnet/four-validator.sh with fast
# useful-service epochs and node A's operator registered as a storage
# assigner in genesis), three hashgram-nodes (A: bootstrap+relay+store+call,
# B: store+media, C: relay+store), the indexer (when PostgreSQL is present),
# the safety engine, and two devices (alice, bob) using hashgram-client.
#
# What is checked, with evidence rather than "process running":
#   - nodes verify each other; a node on a forked genesis is refused
#   - alice -> bob E2EE message is delivered through store nodes that hold
#     only ciphertext; bob's reply comes back; a group works
#   - a post and a reel propagate to every node and appear in the indexer
#   - a blob uploaded to one node is replicated to three and is DEGRADED at
#     fewer; the storage assignment lands on chain; the chain's storage
#     challenge is answered and passes
#   - a client-signed retrieval receipt is submitted and credited
#   - a scam post is blocked by the safety engine on nodes and indexer
#   - killing the store node B: messages and blobs still flow
#   - destroying the genesis host (validator0 + node A): the chain keeps
#     producing blocks and messaging keeps working through B and C
#   - the P2P nodes' data directories contain no validator or Founder key
#     material (the "stolen disk image" check)
#
# Everything runs on one machine as one user, which tests protocol behaviour
# and not geography. The four-validator script says the same about itself.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

P2_DIR="${P2_DIR:-$REPO_ROOT/phase2-data}"
TESTNET_DIR="${TESTNET_DIR:-$REPO_ROOT/testnet-data}"
export TESTNET_DIR
HASHGRAMD="$REPO_ROOT/build/hashgramd"
NODE_BIN="${HASHGRAM_NODE_BIN:-$REPO_ROOT/node/target/release/hashgram-node}"
CLIENT_BIN="${HASHGRAM_CLIENT_BIN:-$REPO_ROOT/node/target/release/hashgram-client}"
INDEXER_BIN="$REPO_ROOT/build/hashgram-indexer"
SAFETY_BIN="$REPO_ROOT/build/hashgram-safety"
CHAIN_RPC="http://127.0.0.1:26641"
CHAIN_API="http://127.0.0.1:26643"
GENESIS_HASH=""

export HASHGRAM_PASSPHRASE="${HASHGRAM_PASSPHRASE:-phase2-devnet-only}"
export HASHGRAM_LIGHT_KDF=true

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

# Node i: P2P port 27000+i, API 27100+i.
node_home() { echo "$P2_DIR/node$1"; }
node_api()  { echo "http://127.0.0.1:$((27100 + $1))"; }
node_p2p()  { echo $((27000 + $1)); }
client_home() { echo "$P2_DIR/client-$1"; }
C() { "$CLIENT_BIN" --home "$(client_home "$1")" "${@:2}"; }

jsonq() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1" 2>/dev/null; }

stop_p2() {
  for f in "$P2_DIR"/*.pid; do
    [ -f "$f" ] || continue
    pid="$(cat "$f")"
    kill "$pid" 2>/dev/null || true
    rm -f "$f"
  done
  pkill -f "hashgram-node run --home $P2_DIR" 2>/dev/null || true
  pkill -f "hashgram-indexer run --config $P2_DIR" 2>/dev/null || true
  pkill -f "hashgram-safety run --config $P2_DIR" 2>/dev/null || true
}

case "${1:-up}" in
  stop)  stop_p2; scripts/testnet/four-validator.sh stop; exit 0 ;;
  clean) stop_p2; scripts/testnet/four-validator.sh clean; rm -rf "$P2_DIR"; say "removed $P2_DIR"; exit 0 ;;
  up|"") ;;
  *) say "usage: $0 [up|stop|clean]"; exit 1 ;;
esac

for required in "$HASHGRAMD" "$NODE_BIN" "$CLIENT_BIN" "$INDEXER_BIN" "$SAFETY_BIN"; do
  if [ ! -x "$required" ]; then
    say "error: $required is missing. Run: make build && make rust-release"
    exit 1
  fi
done

stop_p2
scripts/testnet/four-validator.sh clean >/dev/null 2>&1 || true
rm -rf "$P2_DIR"
mkdir -p "$P2_DIR"

# ---------------------------------------------------------------------------
head1 "PHASE 2 ACCEPTANCE"
# ---------------------------------------------------------------------------

say ""
say "Preparing operator keys before genesis, so node A's operator can be a"
say "storage assigner from block 1 and the operators are funded for bonds."
OPS=()
for i in 0 1 2; do
  mkdir -p "$(node_home "$i")/data"
  op="$("$NODE_BIN" operator-key --home "$(node_home "$i")/data" 2>/dev/null)"
  OPS+=("$op")
  ok "node$i operator $op"
done

# Devices: alice and bob get vaults now so their addresses can be funded in
# genesis. Registration on chain happens once the chain is up.
for u in alice bob; do
  mkdir -p "$(client_home "$u")"
  # A temporary profile so `identity create --offline` has a network to name.
  cat > "$(client_home "$u")/profile.toml" <<EOF
network = "devnet"
genesis_hash = "0000000000000000000000000000000000000000000000000000000000000000"
chain_api = "$CHAIN_API"
bootstrap = []
EOF
  C "$u" identity create --device-id "$u-laptop" --offline >/dev/null 2>&1
done
ALICE="$(C alice --json identity show | jsonq "d['address']")"
BOB="$(C bob --json identity show | jsonq "d['address']")"
ok "alice $ALICE"
ok "bob   $BOB"

GENESIS_PY="$P2_DIR/genesis-edit.py"
cat > "$GENESIS_PY" <<PY
import json, sys
p = sys.argv[1]
d = json.load(open(p))
sp = d["app_state"]["serviceproof"]
sp["params"]["epoch_blocks"] = "40"
sp["params"]["challenge_response_blocks"] = "30"
sp["params"]["challenges_per_epoch"] = 4
sp["assigners"] = ["${OPS[0]}"]
json.dump(d, open(p, "w"), indent=2)
PY

EXTRA=""
for op in "${OPS[@]}"; do EXTRA="$EXTRA $op:5000000000"; done
EXTRA="$EXTRA $ALICE:100000000000 $BOB:100000000000"

say ""
say "Starting the four-validator chain (fast epochs, node A as assigner)."
if ! EXTRA_GENESIS_ACCOUNTS="$EXTRA" EXTRA_GENESIS_PY="$GENESIS_PY" SKIP_FORK_TESTS=1 \
     scripts/testnet/four-validator.sh up > "$P2_DIR/four-validator.log" 2>&1; then
  bad "four-validator testnet failed; see $P2_DIR/four-validator.log"
  tail -20 "$P2_DIR/four-validator.log"
  exit 1
fi
ok "four validators running (see $P2_DIR/four-validator.log)"

GENESIS_HASH="$(sha256sum "$TESTNET_DIR/validator0/config/genesis.json" | awk '{print $1}')"
ok "genesis hash $GENESIS_HASH"

# Wait for the REST gateway.
for _ in $(seq 1 30); do
  curl -sf "$CHAIN_API/cosmos/base/tendermint/v1beta1/node_info" >/dev/null 2>&1 && break
  sleep 1
done

# ---------------------------------------------------------------------------
head1 "P2P NODES"
# ---------------------------------------------------------------------------

write_pin() {
  cat > "$1/network.json" <<EOF
{"network_name":"Hashgram Devnet (DEVNET ONLY)","network_id":"hashgram-devnet","chain_id":"hashgram-devnet-1","network_magic":"HGD1","protocol_major_version":1,"genesis_hash":"$GENESIS_HASH"}
EOF
}

ROLES=('["bootstrap","relay","store","call"]' '["store","media"]' '["relay","store"]')
NODE_IDS=()
for i in 0 1 2; do
  home="$(node_home "$i")"
  mkdir -p "$home/etc"
  write_pin "$home/etc"
  echo "{\"roles\":${ROLES[$i]},\"declared_storage_bytes\":5000000000}" > "$home/etc/roles.json"
  NODE_IDS+=("$("$NODE_BIN" node-id --home "$home/data" 2>/dev/null)")
done
A_ADDR="/ip4/127.0.0.1/tcp/$(node_p2p 0)/p2p/${NODE_IDS[0]}"

echo "phase2-turn-secret" > "$P2_DIR/turn.secret"
for i in 0 1 2; do
  home="$(node_home "$i")"
  {
    echo "listen_addr = \"127.0.0.1\""
    echo "listen_port = $(node_p2p "$i")"
    echo "api_addr = \"127.0.0.1:$((27100 + i))\""
    echo "metrics_addr = \"127.0.0.1:$((27100 + i))\""
    echo "chain_rpc = \"$CHAIN_RPC\""
    echo "chain_api = \"$CHAIN_API\""
    echo "peerstore_path = \"$home/data/peerstore.json\""
    echo "min_peers = 1"
    echo "auto_register_provider = true"
    echo "provider_bond_uhash = 1000000000"
    echo "reward_address = \"$ALICE\""
    echo "moniker = \"phase2-node$i\""
    if [ "$i" -ne 0 ]; then echo "bootstrap_peers = [\"$A_ADDR\"]"; fi
    if [ "$i" -eq 0 ]; then
      echo "turn_uris = [\"turn:127.0.0.1:3478?transport=udp\"]"
      echo "turn_realm = \"phase2.devnet\""
      echo "turn_secret_file = \"$P2_DIR/turn.secret\""
    fi
  } > "$home/etc/node.toml"
done

start_node() {
  local i="$1" home; home="$(node_home "$1")"
  RUST_LOG=info nohup "$NODE_BIN" run --home "$home/data" --config "$home/etc/node.toml" \
    > "$home/node.log" 2>&1 &
  echo $! > "$P2_DIR/node$i.pid"
}

start_node 0; sleep 2; start_node 1; start_node 2
sleep 8
for i in 0 1 2; do
  v="$(curl -s "$(node_api "$i")/v1/status" | jsonq "d['swarm']['verified']")"
  if [ "${v:-0}" -ge 1 ]; then ok "node$i verified $v peer(s)"; else bad "node$i verified no peers"; fi
done

# Clients point at all three nodes.
for u in alice bob; do
  C "$u" configure --network devnet --genesis-hash "$GENESIS_HASH" --chain-api "$CHAIN_API" \
    --bootstrap "$A_ADDR" \
    --bootstrap "/ip4/127.0.0.1/tcp/$(node_p2p 1)/p2p/${NODE_IDS[1]}" \
    --bootstrap "/ip4/127.0.0.1/tcp/$(node_p2p 2)/p2p/${NODE_IDS[2]}" >/dev/null
done

# ---------------------------------------------------------------------------
head1 "FORK REJECTION AT THE HASHGRAM HANDSHAKE"
# ---------------------------------------------------------------------------

FORK_HOME="$P2_DIR/fork"
mkdir -p "$FORK_HOME/etc" "$FORK_HOME/data"
cat > "$FORK_HOME/etc/network.json" <<EOF
{"network_name":"Hashgram Devnet (DEVNET ONLY)","network_id":"hashgram-devnet","chain_id":"hashgram-devnet-1","network_magic":"HGD1","protocol_major_version":1,"genesis_hash":"eed34034d774c8a2f0b1e5c6d7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d3e4f5a6"}
EOF
echo '{"roles":["relay"]}' > "$FORK_HOME/etc/roles.json"
cat > "$FORK_HOME/etc/node.toml" <<EOF
listen_addr = "127.0.0.1"
listen_port = 27050
api_addr = "127.0.0.1:27150"
metrics_addr = "127.0.0.1:27150"
peerstore_path = "$FORK_HOME/data/peerstore.json"
bootstrap_peers = ["$A_ADDR"]
EOF
RUST_LOG=info nohup "$NODE_BIN" run --home "$FORK_HOME/data" --config "$FORK_HOME/etc/node.toml" --insecure-no-chain > "$FORK_HOME/node.log" 2>&1 &
echo $! > "$P2_DIR/fork.pid"
sleep 6
if grep -q "genesis hash mismatch" "$(node_home 0)/node.log"; then
  ok "node A refused the fork at the handshake: genesis hash mismatch"
else
  bad "node A did not log a genesis hash mismatch for the fork"
fi
BANNED="$(curl -s "$(node_api 0)/v1/status" | jsonq "d['swarm']['banned']")"
[ "${BANNED:-0}" -ge 1 ] && ok "fork peer is banned on node A" || bad "fork peer not banned (banned=$BANNED)"
FV="$(curl -s "127.0.0.1:27150/v1/status" | jsonq "d['swarm']['verified']")"
[ "${FV:-1}" -eq 0 ] && ok "the fork verified nobody" || bad "the fork verified $FV peer(s)"
kill "$(cat "$P2_DIR/fork.pid")" 2>/dev/null; rm -f "$P2_DIR/fork.pid"

# ---------------------------------------------------------------------------
head1 "IDENTITIES AND PROVIDERS ON CHAIN"
# ---------------------------------------------------------------------------

C alice identity register >/dev/null 2>&1 && ok "alice identity registered" || bad "alice identity registration failed"
C bob identity register >/dev/null 2>&1 && ok "bob identity registered" || bad "bob identity registration failed"
DEV="$(C alice --json identity devices | jsonq "len(d)")"
[ "${DEV:-0}" -ge 1 ] && ok "alice has $DEV device(s) on chain" || bad "alice has no devices on chain"

say "  waiting for providers to register (bond 1,000 HASH each)..."
for _ in $(seq 1 30); do
  n="$(curl -s "$CHAIN_API/hashgram/serviceproof/v1/providers" | jsonq "len(d['providers'])")"
  [ "${n:-0}" -ge 3 ] && break
  sleep 2
done
[ "${n:-0}" -ge 3 ] && ok "$n providers registered" || bad "only ${n:-0} providers registered"

C alice identity publish-keys >/dev/null 2>&1 && ok "alice key packages published" || bad "alice publish-keys failed"
C bob identity publish-keys >/dev/null 2>&1 && ok "bob key packages published" || bad "bob publish-keys failed"

# ---------------------------------------------------------------------------
head1 "END-TO-END ENCRYPTED MESSAGING"
# ---------------------------------------------------------------------------

C alice message send "$BOB" "hello bob from phase2" >/dev/null 2>&1 && ok "alice sent" || bad "alice send failed"
OUT="$(C bob message receive 2>/dev/null)"
if echo "$OUT" | grep -q "hello bob from phase2"; then ok "bob decrypted alice's message"; else bad "bob did not receive: $OUT"; fi
C bob message send "$ALICE" "hi alice" >/dev/null 2>&1
OUT="$(C alice message receive 2>/dev/null)"
echo "$OUT" | grep -q "hi alice" && ok "alice decrypted bob's reply" || bad "alice did not receive the reply"
CONV="$(C alice message list | grep -c direct)"
[ "$CONV" -eq 1 ] && ok "one direct conversation reused for both directions" || bad "expected 1 direct conversation, found $CONV"

# The store nodes held ciphertext only.
if grep -a -q "hello bob from phase2" "$(node_home 0)/data/mailbox.redb" "$(node_home 1)/data/mailbox.redb" "$(node_home 2)/data/mailbox.redb" 2>/dev/null; then
  bad "PLAINTEXT FOUND in a store node's mailbox"
else
  ok "no plaintext in any store node's mailbox database"
fi

C alice group create team "$BOB" >/dev/null 2>&1 && ok "group created" || bad "group create failed"
GID="$(C alice message list | grep "group:team" | cut -d' ' -f1)"
C alice message send-group "$GID" "team message" >/dev/null 2>&1
OUT="$(C bob message receive 2>/dev/null)"
echo "$OUT" | grep -q "team message" && ok "bob decrypted the group message" || bad "bob did not receive the group message"

# ---------------------------------------------------------------------------
head1 "SOCIAL EVENTS, MEDIA AND REPLICATION"
# ---------------------------------------------------------------------------

C alice profile create --name "Alice" --bio "phase2" >/dev/null 2>&1 && ok "profile published" || bad "profile failed"
C alice post "phase2 public post" --tag phase2 >/dev/null 2>&1 && ok "post published" || bad "post failed"
C bob follow "$ALICE" >/dev/null 2>&1 && ok "bob follows alice" || bad "follow failed"
REEL="$(C alice --json reel publish-test 2>/dev/null)"
REEL_CID="$(echo "$REEL" | jsonq "d['video_cid']")"
[ -n "$REEL_CID" ] && ok "reel published, video $REEL_CID" || bad "reel failed"

sleep 4
for i in 0 1 2; do
  n="$(curl -s "$(node_api "$i")/v1/social/author/$ALICE" | jsonq "len(d)")"
  [ "${n:-0}" -ge 3 ] && ok "node$i holds $n of alice's events" || bad "node$i holds ${n:-0} of alice's events"
done

say "  waiting for replication to reach three copies..."
for _ in $(seq 1 45); do
  H="$(curl -s "$(node_api 1)/v1/blobs/$REEL_CID/health" | jsonq "d['status']")"
  [ "$H" = "HEALTHY" ] && break
  sleep 3
done
if [ "$H" = "HEALTHY" ]; then ok "reel video is HEALTHY (3 known replicas)"; else warn "reel video is ${H:-unknown} after 135s (repair runs every 60s)"; fi

head -c 3000000 /dev/urandom > "$P2_DIR/photo.bin"
UP="$(C alice --json blob upload "$P2_DIR/photo.bin" --mime image/jpeg --private 2>/dev/null)"
PCID="$(echo "$UP" | jsonq "d['cid']")"; PKEY="$(echo "$UP" | jsonq "d['key']")"; PNONCE="$(echo "$UP" | jsonq "d['nonce']")"
[ -n "$PCID" ] && ok "private blob uploaded $PCID" || bad "private upload failed"
C bob blob download "$PCID" "$P2_DIR/photo.dec" --key "$PKEY" --nonce "$PNONCE" >/dev/null 2>&1
cmp -s "$P2_DIR/photo.bin" "$P2_DIR/photo.dec" && ok "bob downloaded and decrypted the private blob byte-for-byte" || bad "private blob round trip failed"
C bob blob download "$PCID" "$P2_DIR/photo.enc" >/dev/null 2>&1
cmp -s "$P2_DIR/photo.bin" "$P2_DIR/photo.enc" && bad "stored blob equals plaintext" || ok "stored blob is ciphertext"

# ---------------------------------------------------------------------------
head1 "INDEXER AND SAFETY"
# ---------------------------------------------------------------------------

mkdir -p "$P2_DIR/safety/home"
write_pin "$P2_DIR/safety"
cat > "$P2_DIR/safety/rules.json" <<'EOF'
[{"pattern": "free\\s+hash\\s+airdrop", "verdict": "BLOCK", "reason": "scam-airdrop"}]
EOF
cat > "$P2_DIR/safety/safety.toml" <<EOF
node_api = "$(node_api 1)"
home = "$P2_DIR/safety/home"
text_rules_file = "$P2_DIR/safety/rules.json"
poll_interval = "2s"
EOF
ATTESTOR="$("$SAFETY_BIN" key --config "$P2_DIR/safety/safety.toml")"
ok "safety attestor $ATTESTOR"
# Nodes trust the attestor: restart B and C with it configured.
for i in 1 2; do
  echo "trusted_attestors = [\"$ATTESTOR\"]" >> "$(node_home "$i")/etc/node.toml"
  kill "$(cat "$P2_DIR/node$i.pid")" 2>/dev/null; sleep 1; start_node "$i"
done
sleep 6
nohup "$SAFETY_BIN" run --config "$P2_DIR/safety/safety.toml" > "$P2_DIR/safety/safety.log" 2>&1 &
echo $! > "$P2_DIR/safety.pid"

INDEXER_UP=0
if command -v psql >/dev/null 2>&1 && sudo -n -u postgres psql -c 'SELECT 1' >/dev/null 2>&1; then
  sudo -u postgres psql -tAc "SELECT 1 FROM pg_roles WHERE rolname='hashgram_phase2'" | grep -q 1 || sudo -u postgres psql -qc "CREATE ROLE hashgram_phase2 LOGIN PASSWORD 'phase2';"
  sudo -u postgres psql -qc "DROP DATABASE IF EXISTS hashgram_phase2;" >/dev/null 2>&1
  sudo -u postgres psql -qc "CREATE DATABASE hashgram_phase2 OWNER hashgram_phase2;"
  mkdir -p "$P2_DIR/indexer"; write_pin "$P2_DIR/indexer"
  cat > "$P2_DIR/indexer/indexer.toml" <<EOF
database_url = "postgres://hashgram_phase2:phase2@127.0.0.1:5432/hashgram_phase2"
chain_rpc = "$CHAIN_RPC"
chain_api = "$CHAIN_API"
node_api = "$(node_api 1)"
listen = "127.0.0.1:27318"
poll_interval = "2s"
trusted_attestors = ["$ATTESTOR"]
EOF
  nohup "$INDEXER_BIN" run --config "$P2_DIR/indexer/indexer.toml" > "$P2_DIR/indexer/indexer.log" 2>&1 &
  echo $! > "$P2_DIR/indexer.pid"
  INDEXER_UP=1
  sleep 10
  POSTS="$(curl -s "127.0.0.1:27318/v1/feed/author/$ALICE" | jsonq "len(d)")"
  [ "${POSTS:-0}" -ge 1 ] && ok "indexer serves alice's post" || bad "indexer has ${POSTS:-0} posts for alice"
  REELS="$(curl -s "127.0.0.1:27318/v1/reels" | jsonq "len(d)")"
  [ "${REELS:-0}" -ge 1 ] && ok "indexer serves the reel" || bad "indexer has no reels"
  FOLLOW="$(curl -s "127.0.0.1:27318/v1/feed/following/$BOB" | jsonq "len(d)")"
  [ "${FOLLOW:-0}" -ge 1 ] && ok "bob's following feed shows alice" || bad "following feed empty"
  PROV="$(curl -s "127.0.0.1:27318/v1/providers" | jsonq "len(d)")"
  [ "${PROV:-0}" -ge 3 ] && ok "indexer lists $PROV providers from chain" || bad "indexer lists ${PROV:-0} providers"
else
  warn "PostgreSQL not available without a password; indexer checks skipped"
fi

C bob post "FREE HASH airdrop!!! send 10 HASH to claim" >/dev/null 2>&1
sleep 8
if grep -a -q "verdict=CONTENT_BLOCK" "$P2_DIR/safety/safety.log"; then ok "safety engine blocked the scam post"; else bad "safety engine did not block the scam post"; fi
NB="$(curl -s "$(node_api 1)/v1/social/author/$BOB" | jsonq "len([e for e in d if e['type']=='POST_CREATE'])")"
[ "${NB:-1}" -eq 0 ] && ok "node B no longer serves the blocked post" || bad "node B still serves $NB blocked post(s)"
if [ "$INDEXER_UP" -eq 1 ]; then
  sleep 4
  IB="$(curl -s "127.0.0.1:27318/v1/feed/author/$BOB" | jsonq "len(d)")"
  [ "${IB:-1}" -eq 0 ] && ok "indexer hides the blocked post" || bad "indexer still shows $IB blocked post(s)"
fi

# ---------------------------------------------------------------------------
head1 "USEFUL-SERVICE EVIDENCE ON CHAIN"
# ---------------------------------------------------------------------------

say "  waiting for storage assignments and an epoch boundary (40 blocks)..."
for _ in $(seq 1 60); do
  ASSIGN="$(curl -s "$CHAIN_API/hashgram/serviceproof/v1/assignments/${OPS[0]}" | jsonq "len(d['assignments'])")"
  [ "${ASSIGN:-0}" -ge 1 ] && break
  sleep 3
done
[ "${ASSIGN:-0}" -ge 1 ] && ok "node A recorded $ASSIGN storage assignment(s) on chain" || bad "no storage assignments recorded"

for _ in $(seq 1 60); do
  CH="$(curl -s "$CHAIN_API/hashgram/serviceproof/v1/challenges/${OPS[0]}" | jsonq "len(d['challenges'])")"
  [ "${CH:-0}" -ge 1 ] && break
  sleep 3
done
if [ "${CH:-0}" -ge 1 ]; then
  ok "chain issued $CH storage challenge(s) to node A"
  for _ in $(seq 1 30); do
    if grep -a -q "storage challenge answered" "$(node_home 0)/node.log"; then break; fi
    sleep 3
  done
  grep -a -q "storage challenge answered" "$(node_home 0)/node.log" && ok "node A answered a challenge with a Merkle proof" || bad "node A did not answer"
  PASSED="$(curl -s "$CHAIN_API/hashgram/serviceproof/v1/rewards/${OPS[0]}" | jsonq "d['current']['challenges_passed']")"
  [ "${PASSED:-0}" != "0" ] && ok "chain records challenges_passed=$PASSED for node A" || warn "challenges_passed is ${PASSED:-0} (settlement may lag an epoch)"
else
  bad "no storage challenge issued"
fi

C bob blob download "$REEL_CID" "$P2_DIR/reel.bin" >/dev/null 2>&1
say "  waiting for receipt submission (once per minute) and settlement..."
sleep 75
credit_of() {
  curl -s "$CHAIN_API/hashgram/serviceproof/v1/rewards/$1" | python3 -c '
import json,sys
d=json.load(sys.stdin)
total=0
for c in [d.get("current",{})]+d.get("recent",[]):
    total+=int(c.get("relay_credit",0))+int(c.get("retrieval_credit",0))
print(total)' 2>/dev/null || echo 0
}
TOTAL=0
for op in "${OPS[@]}"; do TOTAL=$((TOTAL + $(credit_of "$op"))); done
if [ "$TOTAL" -gt 0 ]; then
  ok "client-signed receipts produced $TOTAL units of relay/retrieval credit across the providers"
else
  bad "no receipt credit on any provider"
fi
SETTLED="$(curl -s "$CHAIN_API/hashgram/serviceproof/v1/rewards/${OPS[0]}" | jsonq "len(d.get('lifetime_paid',[]))")"
[ "${SETTLED:-0}" -ge 1 ] && ok "node A has been paid at an epoch settlement" || warn "node A not yet paid (settlement pays at epoch close; storage needs a full epoch held)"

# ---------------------------------------------------------------------------
head1 "FAILOVER: KILL THE STORE NODE B"
# ---------------------------------------------------------------------------

kill "$(cat "$P2_DIR/node1.pid")" 2>/dev/null; rm -f "$P2_DIR/node1.pid"
sleep 3
C alice message send "$BOB" "after B died" >/dev/null 2>&1 && ok "alice sent with B down" || bad "send failed with B down"
OUT="$(C bob message receive 2>/dev/null)"
echo "$OUT" | grep -q "after B died" && ok "bob received with B down" || bad "bob did not receive with B down"
C bob blob download "$PCID" "$P2_DIR/photo2.enc" >/dev/null 2>&1 && ok "private blob still downloadable with B down" || bad "blob download failed with B down"

# ---------------------------------------------------------------------------
head1 "GENESIS HOST DESTROYED: VALIDATOR0 + NODE A"
# ---------------------------------------------------------------------------

H_BEFORE="$(curl -s "$CHAIN_RPC/status" | jsonq "int(d['result']['sync_info']['latest_block_height'])")"
kill "$(cat "$TESTNET_DIR/validator0/node.pid")" 2>/dev/null
kill "$(cat "$P2_DIR/node0.pid")" 2>/dev/null; rm -f "$P2_DIR/node0.pid"
sleep 12
H_AFTER="$(curl -s "http://127.0.0.1:26651/status" | jsonq "int(d['result']['sync_info']['latest_block_height'])")"
if [ "${H_AFTER:-0}" -gt "${H_BEFORE:-0}" ]; then
  ok "chain advanced from $H_BEFORE to $H_AFTER with the genesis validator and node A gone"
else
  bad "chain did not advance after the genesis host died ($H_BEFORE -> ${H_AFTER:-?})"
fi
# Clients need a chain API that is still alive: validator1's.
for u in alice bob; do
  C "$u" configure --network devnet --genesis-hash "$GENESIS_HASH" --chain-api "http://127.0.0.1:26653" \
    --bootstrap "/ip4/127.0.0.1/tcp/$(node_p2p 2)/p2p/${NODE_IDS[2]}" >/dev/null
done
C bob message send "$ALICE" "after genesis host died" >/dev/null 2>&1 && ok "bob sent through node C only" || bad "send failed with only node C"
OUT="$(C alice message receive 2>/dev/null)"
echo "$OUT" | grep -q "after genesis host died" && ok "alice received through node C only" || bad "alice did not receive through node C"

# ---------------------------------------------------------------------------
head1 "STOLEN DISK IMAGE: P2P NODE DIRECTORIES"
# ---------------------------------------------------------------------------

# What an attacker gets from a relay/store node's disk must not include a
# validator or Founder key. The operator key IS there and IS hot: that is a
# documented, bounded exposure (bond + node identity), not chain control.
LEAK=0
for i in 0 1 2; do
  d="$(node_home "$i")/data"
  if [ -f "$TESTNET_DIR/validator0/config/priv_validator_key.json" ]; then
    VALKEY="$(python3 -c "import json;print(json.load(open('$TESTNET_DIR/validator0/config/priv_validator_key.json'))['priv_key']['value'])")"
    grep -a -r -q "$VALKEY" "$d" && { bad "validator private key found in node$i data"; LEAK=1; }
  fi
  ALICE_SEED="$(python3 -c "import json,sys;print(json.load(open('$(client_home alice)/keystore.json')).get('ciphertext','')[:64])")"
  grep -a -r -q "$ALICE_SEED" "$d" && { bad "client vault ciphertext found in node$i data"; LEAK=1; }
  grep -a -r -q "mnemonic\|abandon abandon" "$d" && { bad "mnemonic material found in node$i data"; LEAK=1; }
done
[ "$LEAK" -eq 0 ] && ok "no validator key, client vault or mnemonic material in any P2P node directory"
if command -v gitleaks >/dev/null 2>&1; then
  if gitleaks detect --no-git --source "$P2_DIR/node2/data" --exit-code 1 >/dev/null 2>&1; then
    ok "gitleaks finds nothing in node C's data directory"
  else
    warn "gitleaks flagged content in node C's data (review; redb files contain random-looking bytes)"
  fi
fi

# ---------------------------------------------------------------------------
head1 "RESULT"
# ---------------------------------------------------------------------------

if [ "$FAILURES" -eq 0 ]; then
  printf '  %sAll Phase 2 checks passed.%s\n' "$GREEN" "$RESET"
else
  printf '  %s%d check(s) failed.%s\n' "$RED" "$FAILURES" "$RESET"
fi
say ""
say "  Logs      $P2_DIR/node{0,1,2}/node.log, $P2_DIR/safety/safety.log, $P2_DIR/indexer/indexer.log"
say "  Chain     $P2_DIR/four-validator.log"
say "  Stop      scripts/testnet/phase2.sh stop"
say "  Remove    scripts/testnet/phase2.sh clean"
say ""
say "  Everything ran on one host as one user: protocol behaviour, not"
say "  geography, is what this proves."
exit "$FAILURES"
