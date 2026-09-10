#!/usr/bin/env bash
# Hashgram Founder (and rewards-cold) key.
# Run ONLY on an offline machine. Do not run this on the Hashgram server.
#
# Usage:
#   1. Copy only these two files onto USB from the server:
#        /usr/local/bin/hashgram-keygen
#        scripts/launch/make-founder-key-OFFLINE.sh
#   2. On the laptop: disable Wi-Fi / unplug the cable.
#   3. ./make-founder-key-OFFLINE.sh
#   4. Write the 24 words on paper, twice, in two places.
#   5. Send ONLY the public address hash1... to the server.
set -euo pipefail

if [[ -f /etc/hashgram/roles.json ]] || [[ -d /var/lib/hashgram/chain/config ]]; then
  echo "STOP: this is the Hashgram server. The Founder key is not created here." >&2
  echo "Copy hashgram-keygen to an offline machine and run it there." >&2
  exit 1
fi

BIN=""
for c in ./hashgram-keygen hashgram-keygen /usr/local/bin/hashgram-keygen; do
  if command -v "$c" >/dev/null 2>&1 || [[ -x "$c" ]]; then
    BIN="$c"
    break
  fi
done
if [[ -z "$BIN" ]]; then
  echo "hashgram-keygen not found. Put it in this directory." >&2
  exit 1
fi

echo
echo "======================================================================"
echo "  HASHGRAM — offline key"
echo "======================================================================"
echo
echo "  This prints 24 words once. They are the key to 200,000,000 HASH"
echo "  and the 1% protocol-fee beneficiary."
echo
echo "  Do not photograph. Do not put in a synced password manager."
echo "  Paper. Twice. Two locations."
echo
echo "  Disconnect the network now. Then confirm."
echo

"$BIN" new

echo
echo "Next:"
echo "  1. From the PAPER (not the screen) run:  $BIN derive"
echo "  2. The address must match exactly, character by character."
echo "  3. Send ONLY the hash1... address to the server."
echo "  4. Close the terminal and clear scrollback."
echo
