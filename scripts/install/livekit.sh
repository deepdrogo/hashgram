#!/usr/bin/env bash
#
# Install LiveKit as the SFU for group calls on a Hashgram call node.
#
#   sudo scripts/install/livekit.sh --domain sfu.example.org [--version v1.8.4]
#
# What it does:
#   - downloads the livekit-server release for this architecture and verifies
#     it against the release's published SHA-256 checksums
#   - writes /etc/livekit/config.yaml with a generated API key and secret
#     (root:hashgram-node 0640, so the node can mint room tokens for devices)
#   - installs a hardened systemd unit, opens the media ports, starts it
#   - points node.toml at the SFU so it is announced under the call role
#
# The SFU is optional. 1:1 calls work peer-to-peer with TURN alone; the SFU is
# for group calls and live streams. It terminates media, so it sees decrypted
# audio and video for the rooms it hosts — the same trust level as any SFU
# anywhere. Clients should show which node is hosting a group call.
#
# LiveKit needs a TLS-terminating reverse proxy in front of :7880 for
# browsers; native apps can use wss:// through the same proxy. This script
# does not configure TLS: put Caddy or nginx in front, or run it on a host
# that already has one. The URL announced is wss://<domain>.

set -euo pipefail

DOMAIN=""
VERSION="v1.8.4"
NODE_TOML="/etc/hashgram/node.toml"

while [ $# -gt 0 ]; do
  case "$1" in
    --domain) DOMAIN="$2"; shift 2 ;;
    --version) VERSION="$2"; shift 2 ;;
    --node-toml) NODE_TOML="$2"; shift 2 ;;
    -h|--help) sed -n 2,24p "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
if [ "$(id -u)" -ne 0 ]; then echo "run as root" >&2; exit 1; fi
if [ -z "$DOMAIN" ]; then echo "--domain is required (the public name clients will connect to)" >&2; exit 2; fi

ARCH="$(uname -m)"
case "$ARCH" in
  x86_64) LK_ARCH="amd64" ;;
  aarch64) LK_ARCH="arm64" ;;
  *) echo "unsupported architecture $ARCH" >&2; exit 1 ;;
esac

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
BASE="https://github.com/livekit/livekit/releases/download/${VERSION}"
TARBALL="livekit_${VERSION#v}_linux_${LK_ARCH}.tar.gz"
echo "downloading ${BASE}/${TARBALL}"
curl -fsSL -o "$TMP/$TARBALL" "${BASE}/${TARBALL}"
curl -fsSL -o "$TMP/checksums.txt" "${BASE}/livekit_${VERSION#v}_checksums.txt"
EXPECTED="$(grep " ${TARBALL}\$" "$TMP/checksums.txt" | awk '{print $1}')"
if [ -z "$EXPECTED" ]; then echo "no checksum for $TARBALL in the release" >&2; exit 1; fi
ACTUAL="$(sha256sum "$TMP/$TARBALL" | awk '{print $1}')"
if [ "$EXPECTED" != "$ACTUAL" ]; then
  echo "CHECKSUM MISMATCH for $TARBALL: expected $EXPECTED got $ACTUAL" >&2
  exit 1
fi
echo "checksum verified"
tar -xzf "$TMP/$TARBALL" -C "$TMP" livekit-server
install -m 0755 "$TMP/livekit-server" /usr/local/bin/livekit-server

id -u livekit >/dev/null 2>&1 || useradd --system --no-create-home --shell /usr/sbin/nologin livekit
install -d -m 0750 -o root -g livekit /etc/livekit

if [ ! -f /etc/livekit/config.yaml ]; then
  API_KEY="HG$(head -c 9 /dev/urandom | base64 | tr -dc 'A-Za-z0-9' | head -c 12)"
  API_SECRET="$(head -c 32 /dev/urandom | base64 | tr -d '\n')"
  umask 027
  cat > /etc/livekit/config.yaml <<EOF
# LiveKit SFU for Hashgram. Written by scripts/install/livekit.sh.
port: 7880
bind_addresses:
  - "127.0.0.1"
rtc:
  tcp_port: 7881
  port_range_start: 50000
  port_range_end: 60000
  use_external_ip: true
keys:
  ${API_KEY}: ${API_SECRET}
logging:
  level: info
room:
  auto_create: true
  empty_timeout: 300
  max_participants: 50
EOF
  umask 022
  echo "wrote /etc/livekit/config.yaml with a generated API key"
fi
chown root:livekit /etc/livekit/config.yaml
chmod 0640 /etc/livekit/config.yaml
# The node mints room tokens, so it needs to read the key too.
if getent group hashgram-node >/dev/null; then
  setfacl -m g:hashgram-node:r /etc/livekit/config.yaml 2>/dev/null || true
fi

cat > /etc/systemd/system/livekit-server.service <<'EOF'
[Unit]
Description=LiveKit SFU (Hashgram call node)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=livekit
Group=livekit
ExecStart=/usr/local/bin/livekit-server --config /etc/livekit/config.yaml
Restart=on-failure
RestartSec=5
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
PrivateDevices=true
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectControlGroups=true
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX
LimitNOFILE=65535
MemoryMax=4G

[Install]
WantedBy=multi-user.target
EOF

if command -v ufw >/dev/null 2>&1 && ufw status | grep -q "Status: active"; then
  ufw allow 7881/tcp comment 'LiveKit RTC TCP' >/dev/null
  ufw allow 50000:60000/udp comment 'LiveKit RTC UDP' >/dev/null
fi

systemctl daemon-reload
systemctl enable --now livekit-server

if [ -f "$NODE_TOML" ]; then
  python3 - "$NODE_TOML" "$DOMAIN" <<'PY'
import re, sys
path, domain = sys.argv[1:3]
text = open(path).read()
line = f'sfu_url = "wss://{domain}"'
if re.search(r'^sfu_url\s*=', text, re.M):
    text = re.sub(r'^sfu_url\s*=.*$', line, text, flags=re.M)
else:
    text = text.rstrip('\n') + '\n' + line + '\n'
open(path, 'w').write(text)
PY
  echo "node.toml updated: sfu_url = wss://$DOMAIN"
fi

echo
echo "LiveKit installed. Put a TLS reverse proxy for $DOMAIN in front of 127.0.0.1:7880,"
echo "add the call role if not present (hashgramctl configure-role <existing>,call --restart),"
echo "and verify with: hashgram-client call discover"
