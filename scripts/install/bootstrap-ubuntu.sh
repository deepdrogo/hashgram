#!/usr/bin/env bash
#
# Prepare an Ubuntu Server host to run Hashgram.
#
# Creates the service accounts, directories, systemd units and firewall
# rules, installs the binaries, and locks PostgreSQL to loopback.
#
#   sudo ./scripts/install/bootstrap-ubuntu.sh              full install
#   sudo ./scripts/install/bootstrap-ubuntu.sh --binaries-only  rebuild only
#   sudo ./scripts/install/bootstrap-ubuntu.sh --no-firewall    skip ufw
#
# Idempotent: safe to re-run. It will not overwrite an existing node home,
# an existing network pin, or an existing validator key.
#
# Design notes worth reading before changing anything:
#
#   One service account per role, not one for everything. A relay node that
#   is compromised should not be able to read the consensus key, and separate
#   accounts are what makes that true rather than aspirational.
#
#   Default-deny firewall with only the P2P ports open. Administrative
#   surfaces stay on loopback; exposing read RPC publicly is a deliberate
#   reverse-proxy decision, not a default.
#
#   PostgreSQL on loopback only. The index holds nothing canonical, but it
#   does hold a queryable view of social activity, and an open database port
#   is an open database port.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

BINARIES_ONLY=0
SKIP_FIREWALL=0
SKIP_POSTGRES=0

for arg in "$@"; do
  case "$arg" in
    --binaries-only) BINARIES_ONLY=1 ;;
    --no-firewall)   SKIP_FIREWALL=1 ;;
    --no-postgres)   SKIP_POSTGRES=1 ;;
    -h|--help)
      sed -n '2,30p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *) echo "unknown argument: $arg" >&2; exit 1 ;;
  esac
done

if [ -t 1 ]; then
  BOLD=$'\033[1m'; GREEN=$'\033[32m'; YELLOW=$'\033[33m'; RESET=$'\033[0m'
else
  BOLD=""; GREEN=""; YELLOW=""; RESET=""
fi

step() { printf '\n%s==> %s%s\n' "$BOLD" "$*" "$RESET"; }
ok()   { printf '    %s%s%s\n' "$GREEN" "$*" "$RESET"; }
note() { printf '    %s\n' "$*"; }
warn() { printf '    %s%s%s\n' "$YELLOW" "$*" "$RESET"; }

# ---------------------------------------------------------------------------
# Preconditions
# ---------------------------------------------------------------------------

if [ "$(id -u)" -ne 0 ]; then
  echo "error: this script must run as root (use sudo)" >&2
  exit 1
fi

step "Checking the host"

if [ ! -f /etc/os-release ]; then
  echo "error: /etc/os-release is missing; this script targets Ubuntu Server" >&2
  exit 1
fi
# shellcheck source=/dev/null
. /etc/os-release

if [ "${ID:-}" != "ubuntu" ]; then
  echo "error: this script targets Ubuntu, found ID=${ID:-unknown}" >&2
  echo "       Hashgram runs on other distributions, but the service accounts," >&2
  echo "       firewall and PostgreSQL paths here are Ubuntu-specific." >&2
  exit 1
fi

MAJOR="${VERSION_ID%%.*}"
if [ "$MAJOR" -lt 22 ]; then
  echo "error: Ubuntu $VERSION_ID is too old. 22.04 or newer is required," >&2
  echo "       principally for the systemd version that supports the hardening" >&2
  echo "       directives in deploy/systemd (ProtectProc, ProcSubset)." >&2
  exit 1
fi
ok "Ubuntu $VERSION_ID"

ARCH="$(dpkg --print-architecture)"
if [ "$ARCH" != "amd64" ] && [ "$ARCH" != "arm64" ]; then
  warn "architecture $ARCH is untested; amd64 and arm64 are supported"
fi
ok "architecture $ARCH"

CORES="$(nproc)"
MEM_KB="$(awk '/MemTotal/ {print $2}' /proc/meminfo)"
MEM_GB=$((MEM_KB / 1024 / 1024))
DISK_GB="$(df -BG --output=avail / | tail -1 | tr -dc '0-9')"

ok "${CORES} vCPU, ${MEM_GB} GB RAM, ${DISK_GB} GB free on /"

if [ "$MEM_GB" -lt 4 ]; then
  warn "under 4 GB of RAM. A validator will struggle; consider a larger host."
fi
if [ "$DISK_GB" -lt 50 ]; then
  warn "under 50 GB free. mainnet-preflight will fail on this."
fi

# ---------------------------------------------------------------------------
# Service accounts
# ---------------------------------------------------------------------------

CHAIN_USER=hashgram-chain
NODE_USER=hashgram-node
INDEX_USER=hashgram-index
SAFETY_USER=hashgram-safety
CALL_USER=hashgram-call

DATA_DIR=/var/lib/hashgram
CONFIG_DIR=/etc/hashgram

create_service_user() {
  local user="$1" home="$2"
  if id "$user" >/dev/null 2>&1; then
    note "$user already exists"
    return
  fi
  # System account, no login shell, no password. A service account that can
  # log in is an account somebody can log in as.
  useradd --system --no-create-home --home-dir "$home" \
    --shell /usr/sbin/nologin "$user"
  ok "created $user"
}

if [ "$BINARIES_ONLY" -eq 0 ]; then
  step "Creating service accounts"
  note "One account per role, so a compromised relay cannot read the"
  note "consensus key. This is what makes role separation real."
  create_service_user "$CHAIN_USER"  "$DATA_DIR/chain"
  create_service_user "$NODE_USER"   "$DATA_DIR/node"
  create_service_user "$INDEX_USER"  "$DATA_DIR/index"
  create_service_user "$SAFETY_USER" "$DATA_DIR/safety"
  create_service_user "$CALL_USER"   "$DATA_DIR/call"

  step "Creating directories"

  install -d -m 0755 -o root -g root "$CONFIG_DIR"
  install -d -m 0750 -o "$CHAIN_USER"  -g "$CHAIN_USER"  "$DATA_DIR/chain"
  install -d -m 0750 -o "$NODE_USER"   -g "$NODE_USER"   "$DATA_DIR/node"
  install -d -m 0750 -o "$INDEX_USER"  -g "$INDEX_USER"  "$DATA_DIR/index"
  install -d -m 0750 -o "$SAFETY_USER" -g "$SAFETY_USER" "$DATA_DIR/safety"
  install -d -m 0750 -o "$CALL_USER"   -g "$CALL_USER"   "$DATA_DIR/call"
  install -d -m 0700 -o root -g root "$DATA_DIR/backups"

  ok "$CONFIG_DIR and $DATA_DIR/{chain,node,index,safety,call,backups}"
  note "Each data directory is 0750 and owned by its own service account."
  note "Backups are 0700 and root-owned: they can contain the node key."

  step "Installing dependencies"
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq
  apt-get install -y -qq --no-install-recommends \
    ca-certificates curl jq tar gzip python3 \
    ufw \
    systemd-timesyncd \
    >/dev/null
  ok "base packages installed"

  # An unsynchronised clock on a validator produces blocks its peers reject,
  # and the symptom looks like a network fault.
  systemctl enable --now systemd-timesyncd >/dev/null 2>&1 || true
  if timedatectl show --property=NTPSynchronized --value 2>/dev/null | grep -q yes; then
    ok "system clock is NTP synchronised"
  else
    warn "clock not yet synchronised; it may take a minute. Check with: timedatectl"
  fi
fi

# ---------------------------------------------------------------------------
# Binaries
# ---------------------------------------------------------------------------

step "Installing binaries"

if [ -d "$REPO_ROOT/build" ] && [ -x "$REPO_ROOT/build/hashgramd" ]; then
  SOURCE_DIR="$REPO_ROOT/build"
elif command -v go >/dev/null 2>&1; then
  note "no prebuilt binaries; building from source"
  ( cd "$REPO_ROOT" && make build )
  SOURCE_DIR="$REPO_ROOT/build"
else
  echo "error: no binaries in $REPO_ROOT/build and Go is not installed." >&2
  echo "       Either run 'make build' first, or install Go 1.26 or newer." >&2
  exit 1
fi

for bin in hashgramd hashgramctl hashgram-test-client hashgram-keygen hashgram-indexer hashgram-safety; do
  if [ -x "$SOURCE_DIR/$bin" ]; then
    install -m 0755 -o root -g root "$SOURCE_DIR/$bin" "/usr/local/bin/$bin"
    ok "/usr/local/bin/$bin"
  else
    warn "$bin not found in $SOURCE_DIR; skipped"
  fi
done

# The Rust node stack: hashgram-node (P2P) and hashgram-client (developer
# client). Prebuilt release binaries are used when present; otherwise the
# workspace is built with a pinned toolchain installed through rustup for
# root only, so the build does not depend on whatever cargo a user has.
RUST_SOURCE=""
if [ -x "$REPO_ROOT/node/target/release/hashgram-node" ]; then
  RUST_SOURCE="$REPO_ROOT/node/target/release"
elif [ -x "$REPO_ROOT/build/hashgram-node" ]; then
  RUST_SOURCE="$REPO_ROOT/build"
else
  note "no prebuilt hashgram-node; building the Rust workspace (this takes a while on a small VPS)"
  export RUSTUP_HOME="${RUSTUP_HOME:-/root/.rustup}" CARGO_HOME="${CARGO_HOME:-/root/.cargo}"
  if ! command -v cargo >/dev/null 2>&1 && [ ! -x "$CARGO_HOME/bin/cargo" ]; then
    apt-get install -y -qq --no-install-recommends build-essential pkg-config >/dev/null
    RUST_PIN="$(sed -n 's/^rust-version = "\(.*\)"/\1/p' "$REPO_ROOT/node/Cargo.toml")"
    curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain "${RUST_PIN:-stable}" >/dev/null
  fi
  export PATH="$CARGO_HOME/bin:$PATH"
  ( cd "$REPO_ROOT/node" && cargo build --release --locked -p hashgram-node -p hashgram-client )
  RUST_SOURCE="$REPO_ROOT/node/target/release"
fi
for bin in hashgram-node hashgram-client; do
  if [ -x "$RUST_SOURCE/$bin" ]; then
    install -m 0755 -o root -g root "$RUST_SOURCE/$bin" "/usr/local/bin/$bin"
    ok "/usr/local/bin/$bin"
  else
    warn "$bin not found in $RUST_SOURCE; the P2P roles will not start"
  fi
done

if [ "$BINARIES_ONLY" -eq 1 ]; then
  step "Done"
  note "Binaries reinstalled. Restart the services to pick them up:"
  note "  hashgramctl restart"
  exit 0
fi

# ---------------------------------------------------------------------------
# systemd units
# ---------------------------------------------------------------------------

step "Installing systemd units"

UNIT_SOURCE="$REPO_ROOT/deploy/systemd"
if [ ! -d "$UNIT_SOURCE" ]; then
  echo "error: $UNIT_SOURCE is missing" >&2
  exit 1
fi

for unit in "$UNIT_SOURCE"/*.service; do
  [ -f "$unit" ] || continue
  name="$(basename "$unit")"
  install -m 0644 -o root -g root "$unit" "/etc/systemd/system/$name"
  ok "$name"
done

systemctl daemon-reload
ok "systemd reloaded"

note ""
note "The units apply ProtectSystem=strict, NoNewPrivileges, an empty"
note "capability bounding set, a syscall allow-list, MemoryDenyWriteExecute"
note "and bounded memory. Verify with:"
note "  systemd-analyze security hashgramd.service"

# ---------------------------------------------------------------------------
# Firewall
# ---------------------------------------------------------------------------

if [ "$SKIP_FIREWALL" -eq 0 ]; then
  step "Configuring the firewall"

  note "Default deny inbound. Only the ports that genuinely must be reachable"
  note "from the internet are opened; administrative surfaces stay on loopback."

  ufw --force reset >/dev/null 2>&1 || true
  ufw default deny incoming >/dev/null
  ufw default allow outgoing >/dev/null

  # SSH first and explicitly. Enabling a default-deny firewall without an SSH
  # rule locks the operator out of the machine they are configuring.
  SSH_PORT="$(awk '/^Port /{print $2; exit}' /etc/ssh/sshd_config 2>/dev/null || true)"
  SSH_PORT="${SSH_PORT:-22}"
  ufw allow "$SSH_PORT"/tcp comment 'SSH' >/dev/null
  ok "allow $SSH_PORT/tcp  SSH"

  # CometBFT P2P. Must be reachable or the node cannot participate in
  # consensus gossip.
  ufw allow 26656/tcp comment 'Hashgram consensus P2P' >/dev/null
  ok "allow 26656/tcp  consensus P2P"

  # Hashgram P2P (phase 2 Rust node): QUIC over UDP and TCP fallback.
  ufw allow 26670/tcp comment 'Hashgram node P2P (TCP)' >/dev/null
  ufw allow 26670/udp comment 'Hashgram node P2P (QUIC)' >/dev/null
  ok "allow 26670/tcp+udp  Hashgram P2P"

  # TURN for calls. Opened only if coturn is present, since an open TURN
  # port with no server behind it is noise in the logs.
  if systemctl list-unit-files coturn.service >/dev/null 2>&1; then
    ufw allow 3478/tcp comment 'TURN' >/dev/null
    ufw allow 3478/udp comment 'TURN' >/dev/null
    ufw allow 5349/tcp comment 'TURN over TLS' >/dev/null
    ufw allow 5349/udp comment 'TURN over DTLS' >/dev/null
    ufw allow 49152:65535/udp comment 'TURN relay range' >/dev/null
    ok "allow 3478, 5349, 49152-65535/udp  TURN"
  fi

  ufw --force enable >/dev/null
  ok "firewall active, default deny inbound"

  note ""
  note "Deliberately NOT opened:"
  note "  26657  CometBFT RPC        administrative; loopback only"
  note "  9091   application gRPC     loopback only"
  note "  26672  hashgram-node API    loopback only"
  note "  1318   indexer read API     loopback only"
  note "  1317   application REST     loopback only"
  note "  26660  Prometheus metrics   loopback only"
  note "  5432   PostgreSQL           loopback only"
  note ""
  note "To expose public read RPC, put nginx in front of 127.0.0.1:26657 with"
  note "the write and administrative endpoints blocked. See docs/SECURITY.md."
else
  warn "firewall configuration skipped (--no-firewall)"
fi

# ---------------------------------------------------------------------------
# PostgreSQL
# ---------------------------------------------------------------------------

if [ "$SKIP_POSTGRES" -eq 0 ] && command -v psql >/dev/null 2>&1; then
  step "Locking PostgreSQL to loopback"

  PG_CONF="$(sudo -u postgres psql -tAc 'SHOW config_file;' 2>/dev/null || true)"
  if [ -n "$PG_CONF" ] && [ -f "$PG_CONF" ]; then
    CURRENT="$(sudo -u postgres psql -tAc 'SHOW listen_addresses;' 2>/dev/null || echo '')"
    if [ "$CURRENT" = "localhost" ] || [ "$CURRENT" = "127.0.0.1" ]; then
      ok "already listening on $CURRENT only"
    else
      sed -i "s/^#\?listen_addresses.*/listen_addresses = 'localhost'/" "$PG_CONF"
      systemctl restart postgresql
      ok "listen_addresses set to localhost and PostgreSQL restarted"
    fi
  else
    warn "could not locate postgresql.conf; check listen_addresses manually"
  fi

  note "The index holds nothing canonical, but it does hold a queryable view"
  note "of social activity. An open database port is an open database port."
fi

# ---------------------------------------------------------------------------
# Configuration templates for the node stack
# ---------------------------------------------------------------------------

step "Writing configuration templates"

if [ ! -f "$CONFIG_DIR/node.toml" ]; then
  cat > "$CONFIG_DIR/node.toml" <<'NODETOML'
# hashgram-node configuration. The network pin comes from network.json and
# the roles from roles.json in this directory; both are written by
# hashgramctl. Everything here is tuning. See docs/NODE_ROLES.md.

listen_addr = "0.0.0.0"
listen_port = 26670
api_addr = "127.0.0.1:26672"
metrics_addr = "127.0.0.1:26671"
chain_rpc = "http://127.0.0.1:26657"
chain_api = "http://127.0.0.1:1317"
peerstore_path = "/var/lib/hashgram/node/peerstore.json"

# Bootstrap peers with /p2p/<peer-id>. hashgramctl join-mainnet fills this
# from the peers you give it; add independent operators' nodes here too.
bootstrap_peers = []

# Useful-service rewards. The operator key lives in
# /var/lib/hashgram/node/operator.key (hashgram-node operator-key); the
# reward address should be a key that is NOT on this machine.
auto_register_provider = false
provider_bond_uhash = 1000000000
reward_address = ""

# Safety attestors whose verdicts this node enforces (hex ed25519 keys).
trusted_attestors = []
NODETOML
  chmod 0644 "$CONFIG_DIR/node.toml"
  ok "$CONFIG_DIR/node.toml"
else
  note "$CONFIG_DIR/node.toml exists; left untouched"
fi

if [ ! -f "$CONFIG_DIR/safety.toml" ]; then
  cat > "$CONFIG_DIR/safety.toml" <<'SAFETYTOML'
# Hashgram Safety Engine. Reviews PUBLIC content only. See docs/MODERATION.md.
node_api = "http://127.0.0.1:26672"
home = "/var/lib/hashgram/safety"
policy = "hashgram-public-v1"
poll_interval = "5s"
# One BLAKE3 hex per line, optional reason code.
hash_list_file = "/etc/hashgram/safety-hashes.txt"
# JSON array of {"pattern": "...", "verdict": "BLOCK|RESTRICT|QUARANTINE", "reason": "..."}.
text_rules_file = "/etc/hashgram/safety-rules.json"
# Optional external classifier; see safety/pipeline.go for the interface.
model_url = ""
model_send_media = false
model_min_confidence = 0.8
max_media_bytes = 67108864
publish_allow = false
SAFETYTOML
  chmod 0644 "$CONFIG_DIR/safety.toml"
  [ -f "$CONFIG_DIR/safety-rules.json" ] || echo '[]' > "$CONFIG_DIR/safety-rules.json"
  [ -f "$CONFIG_DIR/safety-hashes.txt" ] || printf '# BLAKE3 hex of plaintext media to block, one per line, optional reason code\n' > "$CONFIG_DIR/safety-hashes.txt"
  ok "$CONFIG_DIR/safety.toml (empty rule set; add rules before enabling the safety role)"
else
  note "$CONFIG_DIR/safety.toml exists; left untouched"
fi

# ---------------------------------------------------------------------------
# Indexer database
# ---------------------------------------------------------------------------

if [ "$SKIP_POSTGRES" -eq 0 ] && command -v psql >/dev/null 2>&1; then
  step "Preparing the indexer database"
  if [ ! -f "$CONFIG_DIR/indexer.toml" ]; then
    # A random password, generated here and stored only in indexer.toml,
    # which only root and the indexer's account can read. Not a default.
    INDEX_PW="$(head -c 24 /dev/urandom | base64 | tr -d '/+=\n' | head -c 32)"
    if ! sudo -u postgres psql -tAc "SELECT 1 FROM pg_roles WHERE rolname='$INDEX_USER'" | grep -q 1; then
      sudo -u postgres psql -qc "CREATE ROLE \"$INDEX_USER\" LOGIN PASSWORD '$INDEX_PW';"
    else
      sudo -u postgres psql -qc "ALTER ROLE \"$INDEX_USER\" PASSWORD '$INDEX_PW';"
    fi
    if ! sudo -u postgres psql -tAc "SELECT 1 FROM pg_database WHERE datname='hashgram_index'" | grep -q 1; then
      sudo -u postgres psql -qc "CREATE DATABASE hashgram_index OWNER \"$INDEX_USER\";"
    fi
    umask 027
    cat > "$CONFIG_DIR/indexer.toml" <<INDEXTOML
# Hashgram indexer. The database is a rebuildable cache; see docs/OPERATIONS.md.
database_url = "postgres://${INDEX_USER}:${INDEX_PW}@127.0.0.1:5432/hashgram_index"
chain_rpc = "http://127.0.0.1:26657"
chain_api = "http://127.0.0.1:1317"
node_api = "http://127.0.0.1:26672"
listen = "127.0.0.1:1318"
poll_interval = "3s"
trusted_attestors = []
INDEXTOML
    umask 022
    chown root:"$INDEX_USER" "$CONFIG_DIR/indexer.toml"
    chmod 0640 "$CONFIG_DIR/indexer.toml"
    ok "database hashgram_index and role $INDEX_USER; credentials in $CONFIG_DIR/indexer.toml (root:$INDEX_USER 0640)"
  else
    note "$CONFIG_DIR/indexer.toml exists; left untouched"
  fi
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------

step "Host prepared"

cat <<'NEXT'

    NEXT STEPS

    Creating a new network (genesis operator only):

      hashgramctl init                 initialise the node home and keys
      hashgramctl configure-role validator
      hashgramd keys add operator --home /var/lib/hashgram/chain     # hot key; NOT the Founder key
      hashgramctl init-mainnet-genesis --founder-address hash1... \
        --genesis-account <operator-address>=1000000HASH           # from the Founder's unlocked 20M
      hashgramd genesis gentx operator 900000000000uhash --chain-id hashgram-1 \
        --home /var/lib/hashgram/chain --moniker <name> \
        --commission-rate 0.10 --commission-max-rate 0.20 --commission-max-change-rate 0.01
      hashgramctl finalize-genesis     # pins the FINAL genesis hash
      hashgramctl mainnet-preflight
      hashgramctl start

    Joining an existing network:

      hashgramctl init
      hashgramctl join-mainnet          (genesis, hash and seed nodes are built in)
      hashgramctl configure-role relay,store --declared-storage 500000000000 --reward-address hash1...
      hashgramctl start

    Useful-service rewards need a funded operator account (1,000 HASH bond):
    configure-role prints the operator address to fund, then set
    auto_register_provider = true in /etc/hashgram/node.toml.

    Calls (optional):  sudo scripts/install/coturn.sh --realm <name>
    Group calls SFU:   sudo scripts/install/livekit.sh --domain <name>

    Then:

      hashgramctl status
      hashgramctl chain-status
      hashgramctl health

    BEFORE YOU CREATE A NETWORK

    The Founder address must be created on a machine that is NOT this
    server. Copy hashgram-keygen to that machine, run it there, and bring
    back only the public hash1... address:

      hashgram-keygen new

    Never type the Founder mnemonic on this server. Nothing about running
    the network requires it. See docs/FOUNDER_LAUNCH_RUNBOOK.md.

NEXT
