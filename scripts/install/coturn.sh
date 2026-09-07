#!/usr/bin/env bash
#
# Configure coturn as a Hashgram call node.
#
#   sudo scripts/install/coturn.sh --realm calls.example.org [--external-ip 203.0.113.5]
#
# Writes /etc/turnserver.conf from deploy/coturn/turnserver.conf.template,
# generates the shared auth secret into /etc/hashgram/turn.secret
# (root:hashgram-node 0640), points node.toml at it, opens the TURN ports and
# enables the service. Idempotent: an existing secret is kept, so issued
# credentials stay valid across re-runs.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
REALM=""
EXTERNAL_IP=""
NODE_TOML="/etc/hashgram/node.toml"
SECRET_FILE="/etc/hashgram/turn.secret"

while [ $# -gt 0 ]; do
  case "$1" in
    --realm) REALM="$2"; shift 2 ;;
    --external-ip) EXTERNAL_IP="$2"; shift 2 ;;
    --node-toml) NODE_TOML="$2"; shift 2 ;;
    -h|--help) sed -n 2,14p "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [ "$(id -u)" -ne 0 ]; then echo "run as root" >&2; exit 1; fi
if [ -z "$REALM" ]; then echo "--realm is required (a DNS name or label clients will see)" >&2; exit 2; fi

if [ -z "$EXTERNAL_IP" ]; then
  EXTERNAL_IP="$(ip -4 route get 1.1.1.1 2>/dev/null | awk '{for(i=1;i<=NF;i++) if($i=="src") print $(i+1)}' | head -1)"
  echo "external IP not given; using $EXTERNAL_IP (override with --external-ip if this host is behind NAT)"
fi

if ! command -v turnserver >/dev/null 2>&1; then
  apt-get install -y -qq --no-install-recommends coturn
fi

install -d -m 0750 -o root -g hashgram-node /etc/hashgram 2>/dev/null || install -d -m 0755 /etc/hashgram
if [ ! -s "$SECRET_FILE" ]; then
  umask 077
  head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' > "$SECRET_FILE"
  echo >> "$SECRET_FILE"
  umask 022
  echo "generated TURN auth secret at $SECRET_FILE"
fi
if getent group hashgram-node >/dev/null; then
  chown root:hashgram-node "$SECRET_FILE"
else
  chown root:root "$SECRET_FILE"
fi
chmod 0640 "$SECRET_FILE"
SECRET="$(tr -d ' \n' < "$SECRET_FILE")"

install -d -m 0755 -o turnserver -g turnserver /var/log/turnserver 2>/dev/null || true
sed -e "s|__REALM__|$REALM|g" -e "s|__SECRET__|$SECRET|g" -e "s|__EXTERNAL_IP__|$EXTERNAL_IP|g" \
  "$REPO_ROOT/deploy/coturn/turnserver.conf.template" > /etc/turnserver.conf
chmod 0640 /etc/turnserver.conf
chown root:turnserver /etc/turnserver.conf 2>/dev/null || true

# Debian ships coturn disabled behind this file.
if [ -f /etc/default/coturn ]; then
  sed -i 's/^#\?TURNSERVER_ENABLED=.*/TURNSERVER_ENABLED=1/' /etc/default/coturn
fi

if command -v ufw >/dev/null 2>&1 && ufw status | grep -q "Status: active"; then
  ufw allow 3478/tcp comment 'TURN' >/dev/null
  ufw allow 3478/udp comment 'TURN' >/dev/null
  ufw allow 5349/tcp comment 'TURN over TLS' >/dev/null
  ufw allow 5349/udp comment 'TURN over DTLS' >/dev/null
  ufw allow 49152:65535/udp comment 'TURN relay range' >/dev/null
fi

systemctl enable coturn >/dev/null 2>&1 || true
systemctl restart coturn

# Point the node at the secret and the URIs so it announces them.
if [ -f "$NODE_TOML" ]; then
  python3 - "$NODE_TOML" "$EXTERNAL_IP" "$REALM" "$SECRET_FILE" <<'PY'
import re, sys
path, ip, realm, secret = sys.argv[1:5]
text = open(path).read()
def setkey(t, key, value):
    if re.search(rf'^{key}\s*=', t, re.M):
        return re.sub(rf'^{key}\s*=.*$', f'{key} = {value}', t, flags=re.M)
    return t.rstrip('\n') + f'\n{key} = {value}\n'
text = setkey(text, 'turn_uris', f'["turn:{ip}:3478?transport=udp", "turn:{ip}:3478?transport=tcp", "turns:{ip}:5349?transport=tcp"]')
text = setkey(text, 'turn_realm', f'"{realm}"')
text = setkey(text, 'turn_secret_file', f'"{secret}"')
open(path, 'w').write(text)
PY
  echo "node.toml updated: turn_uris, turn_realm, turn_secret_file"
fi

echo
echo "coturn configured for realm $REALM at $EXTERNAL_IP."
echo "Add the call role and restart the node:  hashgramctl configure-role <existing>,call --restart"
echo "Then verify:                              hashgram-client call discover"
