#!/usr/bin/env bash
# ბეჭდავს ვალიდატორის ოპერატორის მნემონიკას და keyring-ის პაროლს.
# გაუშვი მხოლოდ შენ, SSH-ით, როცა ქაღალდი ხელში გაქვს.
# ეს გასაღები არის ცხელი: 1,000,000 HASH (900k სტეიკი + 100k საკომისიო/ხმა).
set -euo pipefail

TAKE=/root/HASHGRAM_TAKE_OFFLINE
JSON="$TAKE/operator-key.json"
PASS="$TAKE/operator-keyring-password.txt"

if [[ ! -f "$JSON" ]]; then
  echo "არ არის $JSON" >&2
  exit 1
fi

python3 - "$JSON" "$PASS" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
pw = open(sys.argv[2]).read().strip()
print()
print("=" * 74)
print("  OPERATOR (ცხელი გასაღები) — ჩაწერე ქაღალდზე და დახურე ტერმინალი")
print("=" * 74)
print()
print("  მისამართი     ", d.get("address", ""))
print("  სახელი        ", d.get("name", ""))
print()
print("  მნემონიკა")
print("  ---------")
words = (d.get("mnemonic") or "").split()
for i, w in enumerate(words, 1):
    print(f"    {i:2d}. {w}")
print()
print("  Keyring-ის პაროლი (საჭიროა gentx / vote / send-ისთვის ამ სერვერზე)")
print("  ", pw)
print()
print("=" * 74)
print("  ეს არ არის Founder-ის გასაღები. Founder ოფლაინ იქმნება.")
print("=" * 74)
print()
PY
