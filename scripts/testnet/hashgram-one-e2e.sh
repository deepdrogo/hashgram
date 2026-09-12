#!/usr/bin/env bash
# Hashgram One end-to-end acceptance test on a local devnet.
#
# Starts a DEVNET ONLY mock chain gateway (identity registry, no consensus),
# one hashgram-node with the store/relay/media/bootstrap roles, and two
# clients (alice, bob) driven through `hashgram-client one ...` — the same
# SDK facade the desktop uses. Every step asserts on output. Nothing here
# touches Mainnet, /etc, systemd or ports below 1024.
#
# Usage: scripts/testnet/hashgram-one-e2e.sh [debug|release]
# Requires: the three binaries built in node/target/<profile>/, curl, python3.
set -euo pipefail

PROFILE=${1:-debug}
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
B="$ROOT/node/target/$PROFILE"
E=${HASHGRAM_ONE_E2E_DIR:-/tmp/hashgram-one-e2e}
GEN=1111111111111111111111111111111111111111111111111111111111111111
GW_PORT=31417
NODE_PORT=26790
API_PORT=26792
export HASHGRAM_PASSPHRASE=e2e-pass
export HASHGRAM_LIGHT_KDF=true
export RUST_LOG=${RUST_LOG:-warn}

PASS=0; FAIL=0
ok()   { PASS=$((PASS+1)); echo "  ok   $*"; }
fail() { FAIL=$((FAIL+1)); echo "  FAIL $*"; }
assert_contains() { # haystack needle label
  if grep -qF -- "$2" <<<"$1"; then ok "$3"; else fail "$3 (expected to contain: $2)"; echo "$1" | head -20; fi
}
assert_not_contains() {
  if grep -qF -- "$2" <<<"$1"; then fail "$3 (must not contain: $2)"; else ok "$3"; fi
}
cleanup() {
  [ -f "$E/gateway.pid" ] && kill "$(cat "$E/gateway.pid")" 2>/dev/null || true
  [ -f "$E/node.pid" ] && kill "$(cat "$E/node.pid")" 2>/dev/null || true
}
trap cleanup EXIT

for bin in hashgram-node hashgram-client mock-gateway; do
  [ -x "$B/$bin" ] || { echo "missing $B/$bin — build with: cd node && cargo build -p hashgram-node -p hashgram-client -p hashgram-devtools"; exit 2; }
done

rm -rf "$E"; mkdir -p "$E/node/data" "$E/alice" "$E/bob" "$E/logs"

echo "# starting mock gateway"
"$B/mock-gateway" --port $GW_PORT --block-secs 2 > "$E/logs/gateway.log" 2>&1 &
echo $! > "$E/gateway.pid"
for i in $(seq 1 20); do curl -sf 127.0.0.1:$GW_PORT/cosmos/base/tendermint/v1beta1/node_info >/dev/null 2>&1 && break; sleep 0.25; done

echo "# starting node"
cat > "$E/node/network.json" <<EOF
{"network_id":"hashgram-devnet","genesis_hash":"$GEN"}
EOF
cat > "$E/node/roles.json" <<EOF
{"roles":["relay","store","media","bootstrap"],"reward_address":"","declared_storage_bytes":1000000000}
EOF
cat > "$E/node/node.toml" <<EOF
listen_addr = "127.0.0.1"
listen_port = $NODE_PORT
transport = "tcp"
api_addr = "127.0.0.1:$API_PORT"
metrics_addr = "127.0.0.1:$((API_PORT-1))"
chain_rpc = "http://127.0.0.1:$((GW_PORT-1))"
chain_api = "http://127.0.0.1:$GW_PORT"
peerstore_path = "$E/node/data/peerstore.json"
data_dir = "$E/node/data"
bootstrap_peers = []
min_peers = 0
storage_quota_bytes = 1000000000
EOF
"$B/hashgram-node" run --home "$E/node/data" --config "$E/node/node.toml" > "$E/logs/node.log" 2>&1 &
echo $! > "$E/node.pid"
for i in $(seq 1 40); do curl -sf 127.0.0.1:$API_PORT/v1/status >/dev/null 2>&1 && break; sleep 0.25; done
PEER=$(curl -s 127.0.0.1:$API_PORT/v1/status | python3 -c 'import json,sys; print(json.load(sys.stdin)["swarm"]["peer_id"])')
[ -n "$PEER" ] || { echo "node did not start"; cat "$E/logs/node.log"; exit 1; }

echo "# creating identities"
for who in alice bob; do
  H="$E/$who"
  "$B/hashgram-client" --home "$H" configure --network devnet --genesis-hash $GEN --chain-api http://127.0.0.1:$GW_PORT --bootstrap /ip4/127.0.0.1/tcp/$NODE_PORT/p2p/$PEER >/dev/null
  "$B/hashgram-client" --home "$H" identity create --offline --device-id ${who}-1 > "$E/logs/$who-create.log" 2>&1
  J=$("$B/hashgram-client" --home "$H" --json identity show)
  ADDR=$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["address"])' "$J")
  PK=$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["device_pubkey"])' "$J")
  echo "$ADDR" > "$H/address"
  curl -sf -X POST 127.0.0.1:$GW_PORT/devnet/identity -H 'content-type: application/json' \
    -d "{\"address\":\"$ADDR\",\"username\":\"$who\",\"devices\":[{\"device_id\":\"${who}-1\",\"device_pubkey\":\"$PK\",\"label\":\"e2e\",\"platform\":\"linux\"}]}" >/dev/null
  "$B/hashgram-client" --home "$H" identity publish-keys >/dev/null
done
A() { "$B/hashgram-client" --home "$E/alice" "$@" 2>&1; }
Bb() { "$B/hashgram-client" --home "$E/bob" "$@" 2>&1; }
AJ() { "$B/hashgram-client" --home "$E/alice" --json "$@" 2>/dev/null; }
BJ() { "$B/hashgram-client" --home "$E/bob" --json "$@" 2>/dev/null; }
ALICE=$(cat "$E/alice/address"); BOB=$(cat "$E/bob/address")
jq() { python3 -c "import json,sys; d=json.load(sys.stdin); print($1)"; }

echo "# people"
assert_contains "$(A one people resolve @bob)" "username: \"bob\"" "resolve @bob"
assert_contains "$(A one people resolve bob@hashgram.io)" "$BOB" "resolve mail address form"

echo "# drive"
echo "contract v1" > "$E/contract.txt"
ENTRY=$(A one drive put "$E/contract.txt" / --name contract.txt | head -1 | awk '{print $1}')
assert_contains "$(A one drive ls /)" "contract.txt" "drive put + ls"
A one drive get /contract.txt "$E/back.txt" >/dev/null
assert_contains "$(cat "$E/back.txt")" "contract v1" "drive get round trip"

echo "# mail"
echo "inline note" > "$E/note.txt"
assert_contains "$(A one mail send --to @bob --subject 'Contract for review' --body 'see attached' --attach "$E/note.txt" --attach-drive-live "$ENTRY")" "sent" "mail send with inline + live drive attachment"
assert_contains "$(Bb one sync)" "1 mail, 1 drive" "bob sync receives mail + share"
MID=$(BJ one mail list requests | jq 'd[0]["id"]')
assert_contains "$(Bb one mail counts)" "requests      1 total     1 unread" "stranger's mail filed in Requests"
Bb one mail accept "$MID" >/dev/null
assert_contains "$(Bb one mail show "$MID")" "[0] note.txt" "message shows attachments"
Bb one mail attachment "$MID" 0 "$E/b-note.txt" >/dev/null; assert_contains "$(cat "$E/b-note.txt")" "inline note" "inline attachment decrypts"
Bb one mail attachment "$MID" 1 "$E/b-contract.txt" >/dev/null; assert_contains "$(cat "$E/b-contract.txt")" "contract v1" "drive attachment decrypts"
assert_contains "$(Bb one mail send --reply-to "$MID" --body 'Looks good')" "sent" "reply"
assert_contains "$(A one sync)" "1 mail" "alice receives reply"
SENT=$(AJ one mail list sent | jq 'd[0]["id"]')
assert_contains "$(AJ one mail show "$SENT" | jq 'len(d["delivered_to"])')" "1" "delivery receipt recorded on sent copy"
TID=$(AJ one mail list inbox | jq 'd[0]["thread_id"]')
assert_contains "$(A one mail thread "$TID")" "2 messages" "thread has both messages"
assert_contains "$(A one mail list inbox)" "Re: Contract for review" "reply from a previously-written-to sender lands in Inbox"
Bb one mail send --bcc @alice --subject 'bcc' --body x >/dev/null; A one sync >/dev/null
assert_contains "$(AJ one mail list inbox | jq '[x["bcc_copy"] for x in d if x["subject"]=="bcc"]')" "True" "bcc copy flagged"

echo "# live share update + revoke"
echo "contract v2" > "$E/contract.txt"
assert_contains "$(A one drive update "$ENTRY" "$E/contract.txt")" "version 2" "drive update creates v2"
assert_contains "$(A one drive versions "$ENTRY")" "v1" "previous version kept"
Bb one sync >/dev/null
assert_contains "$(Bb one drive shared-with-me)" "v2" "live share updated to v2 on bob"
SHARE=$(BJ one drive shared-with-me | jq 'bytes(d[0]["capability"]["share_id"]).hex()')
Bb one drive get-shared "$SHARE" "$E/b-v2.txt" >/dev/null; assert_contains "$(cat "$E/b-v2.txt")" "contract v2" "bob downloads v2 via capability"
assert_contains "$(Bb one drive save-shared "$SHARE" / && Bb one drive ls /)" "contract.txt" "save shared into own drive"
SID=$(AJ one drive shares | jq 'bytes(d[0]["share_id"]).hex()')
A one drive revoke "$SID" >/dev/null; Bb one sync >/dev/null
assert_contains "$(Bb one drive shared-with-me)" "(revoked)" "revocation visible to grantee"

echo "# contacts"
A one people request @bob --message hi >/dev/null; Bb one sync >/dev/null
assert_contains "$(Bb one people list incoming)" "pending_in" "incoming request"
Bb one people accept "$ALICE" >/dev/null; A one sync >/dev/null
assert_contains "$(A one people list friends)" "friend" "friendship established"

echo "# spaces"
SP=$(A one space create Team --description e2e)
A one space invite "$SP" "$BOB" --role member >/dev/null
A one space announce "$SP" Welcome 'first' >/dev/null
assert_contains "$(Bb one sync)" "space" "bob receives space events"
assert_contains "$(Bb one space list)" "role=2" "bob is Member"
Bb one space post "$SP" 'hello from bob' >/dev/null; A one sync >/dev/null
assert_contains "$(A one space content "$SP")" "hello from bob" "member post visible to owner"
assert_contains "$(Bb one space announce "$SP" Nope x || true)" "may not announce" "member cannot announce (rule enforced)"
A one space share-drive "$SP" "$ENTRY" --path Contracts >/dev/null; Bb one sync >/dev/null
assert_contains "$(Bb one space drive "$SP")" "Contracts/contract.txt" "space drive lists shared file"

echo "# circles"
CI=$(A one circle create Family --member "$BOB")
PID=$(A one circle poll "$CI" 'pizza or sushi' --option pizza --option sushi)
Bb one sync >/dev/null
Bb one circle vote "$CI" "$PID" --choice 1 >/dev/null; A one sync >/dev/null
assert_contains "$(A one circle posts "$CI")" "votes {1: 1}" "poll tally after vote"

echo "# devices"
assert_contains "$(A one devices reconcile)" "errors: []" "device reconcile clean"

echo
echo "hashgram-one e2e: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
