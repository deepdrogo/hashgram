#!/usr/bin/env bash
# Prepare an UNSIGNED Founder delegation and package an offline signing kit.
#
# The Founder key never touches this server. This script only builds the
# transaction bytes, records the account number / sequence the offline signer
# must use, and bundles the laptop binaries and instructions into a zip.
# Signing happens on the offline laptop; broadcasting happens afterwards with
# founder-delegate-broadcast.sh.
#
# Usage: founder-delegate-prepare.sh [AMOUNT_HASH]
#   AMOUNT_HASH   whole HASH to delegate (default 180000000 = the vesting part)
set -euo pipefail

AMOUNT_HASH="${1:-180000000}"
HOME_DIR=/var/lib/hashgram/chain
KIT=/root/hashgram-founder-delegate-kit
ZIP=/root/hashgram-founder-delegate-kit.zip
FACTS=/etc/hashgram/launch-facts.txt
CHAIN_ID=hashgram-1
GAS=250000
FEES=2500uhash

die() { echo "ERROR: $*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "missing $1"; }
need hashgramd; need python3; need zip

[[ "$AMOUNT_HASH" =~ ^[0-9]+$ ]] || die "amount must be whole HASH, got '$AMOUNT_HASH'"
[[ -f "$FACTS" ]] || die "$FACTS missing; was launch-mainnet.sh run?"
FOUNDER="$(sed -n 's/^founder_address=//p' "$FACTS")"
[[ "$FOUNDER" =~ ^hash1[0-9a-z]{38}$ ]] || die "bad founder address in $FACTS"

# The chain is the source of truth for the beneficiary; refuse if it differs.
CHAIN_FOUNDER="$(hashgramd query founder params --home "$HOME_DIR" --output json \
  | python3 -c 'import json,sys; p=json.load(sys.stdin); p=p.get("params",p); print(p.get("beneficiary") or p.get("beneficiary_address",""))')"
[[ -z "$CHAIN_FOUNDER" || "$CHAIN_FOUNDER" == "$FOUNDER" ]] \
  || die "founder beneficiary on chain ($CHAIN_FOUNDER) != $FACTS ($FOUNDER)"

# Exactly one validator exists today; refuse to guess if that changes.
mapfile -t VALOPERS < <(hashgramd query staking validators --home "$HOME_DIR" --output json \
  | python3 -c 'import json,sys; [print(v["operator_address"]) for v in json.load(sys.stdin)["validators"] if v["status"]=="BOND_STATUS_BONDED"]')
[[ ${#VALOPERS[@]} -eq 1 ]] || die "expected exactly 1 bonded validator, found ${#VALOPERS[@]}; pass the target explicitly (edit VALOPER)"
VALOPER="${VALOPERS[0]}"

read -r ACCT SEQ < <(hashgramd query auth account-info "$FOUNDER" --home "$HOME_DIR" --output json \
  | python3 -c 'import json,sys; i=json.load(sys.stdin)["info"]; print(i.get("account_number","0"), i.get("sequence","0"))')

BAL_UHASH="$(hashgramd query bank balance "$FOUNDER" uhash --home "$HOME_DIR" --output json \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["balance"]["amount"])')"
AMOUNT_UHASH="${AMOUNT_HASH}000000"
python3 - "$AMOUNT_UHASH" "$BAL_UHASH" <<'PY' || die "amount exceeds spendable+vesting balance minus fee headroom"
import sys; amt, bal = int(sys.argv[1]), int(sys.argv[2])
sys.exit(0 if amt + 1_000_000 <= bal else 1)
PY

rm -rf "$KIT"; mkdir -p "$KIT/bin"
hashgramd tx staking delegate "$VALOPER" "${AMOUNT_UHASH}uhash" \
  --from "$FOUNDER" --generate-only --chain-id "$CHAIN_ID" \
  --gas "$GAS" --fees "$FEES" --home "$HOME_DIR" --keyring-backend test \
  > "$KIT/unsigned.json" 2>/dev/null
python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); m=d["body"]["messages"][0]; assert m["@type"]=="/cosmos.staking.v1beta1.MsgDelegate"; assert m["delegator_address"]==sys.argv[2]; assert m["validator_address"]==sys.argv[3]; assert m["amount"]=={"denom":"uhash","amount":sys.argv[4]}; assert d["signatures"]==[]' \
  "$KIT/unsigned.json" "$FOUNDER" "$VALOPER" "$AMOUNT_UHASH" || die "unsigned.json does not contain the expected message"

# Laptop binaries: whatever cross-builds exist next to this script's output dir.
for f in /root/hashgram-founder-bins/*; do [[ -e "$f" ]] && cp -r "$f" "$KIT/bin/"; done
cp /usr/local/bin/hashgramd "$KIT/bin/hashgramd-linux-amd64"

cat > "$KIT/SIGN_FACTS.txt" <<EOF
chain_id=$CHAIN_ID
founder_address=$FOUNDER
validator=$VALOPER
amount_uhash=$AMOUNT_UHASH
amount_hash=$AMOUNT_HASH
account_number=$ACCT
sequence=$SEQ
gas=$GAS
fees=$FEES
unsigned_sha256=$(sha256sum "$KIT/unsigned.json" | cut -d' ' -f1)
prepared_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
EOF

cat > "$KIT/sign-OFFLINE.bat" <<EOF
@echo off
REM Run on the OFFLINE laptop, in the folder that contains this file.
REM Step 1 asks for the 24 Founder words. Step 2 signs. Step 3 wipes the key.
setlocal
set KR=%CD%\\keyring-tmp
if exist "%KR%" rmdir /s /q "%KR%"
echo === 1/3 import Founder key (24 words, offline) ===
bin\\windows-amd64\\hashgramd.exe keys add founder --recover --keyring-backend test --keyring-dir "%KR%" --coin-type 118 || goto :fail
echo === 2/3 check address, then sign ===
bin\\windows-amd64\\hashgramd.exe keys show founder -a --keyring-backend test --keyring-dir "%KR%"
echo expected: $FOUNDER
bin\\windows-amd64\\hashgramd.exe tx sign unsigned.json --from founder --offline --chain-id $CHAIN_ID --account-number $ACCT --sequence $SEQ --keyring-backend test --keyring-dir "%KR%" --output-document signed.json || goto :fail
echo === 3/3 wipe key material ===
rmdir /s /q "%KR%"
echo.
echo DONE: signed.json created. Copy ONLY signed.json to the server.
goto :eof
:fail
if exist "%KR%" rmdir /s /q "%KR%"
echo FAILED - nothing was signed. Key material wiped.
exit /b 1
EOF

cat > "$KIT/sign-OFFLINE.sh" <<EOF
#!/usr/bin/env bash
# Run on the OFFLINE laptop (macOS/Linux), in the folder that contains this file.
set -euo pipefail
cd "\$(dirname "\$0")"
case "\$(uname -s)-\$(uname -m)" in
  Darwin-arm64) BIN=bin/macos-arm64/hashgramd ;;
  Darwin-x86_64) BIN=bin/macos-intel/hashgramd ;;
  Linux-x86_64) BIN=bin/hashgramd-linux-amd64 ;;
  *) echo "unsupported platform"; exit 1 ;;
esac
chmod +x "\$BIN"
KR="\$PWD/keyring-tmp"; rm -rf "\$KR"
trap 'rm -rf "\$KR"' EXIT
echo "=== 1/3 import Founder key (24 words, offline) ==="
"\$BIN" keys add founder --recover --keyring-backend test --keyring-dir "\$KR" --coin-type 118
echo "=== 2/3 address check, then sign ==="
"\$BIN" keys show founder -a --keyring-backend test --keyring-dir "\$KR"
echo "expected: $FOUNDER"
"\$BIN" tx sign unsigned.json --from founder --offline --chain-id $CHAIN_ID --account-number $ACCT --sequence $SEQ --keyring-backend test --keyring-dir "\$KR" --output-document signed.json
echo "=== 3/3 key material wiped (trap) ==="
echo "DONE: signed.json created. Copy ONLY signed.json to the server."
EOF
chmod +x "$KIT/sign-OFFLINE.sh"

cat > "$KIT/README_KA.txt" <<EOF
HASHGRAM — Founder-ის დელეგირება (ოფლაინ ხელმოწერა)
=====================================================

რას აკეთებს: Founder-ის $AMOUNT_HASH HASH დელეგირდება შენს ვალიდატორზე.
  ვალიდატორი : $VALOPER
  Founder    : $FOUNDER
  ქსელი      : $CHAIN_ID   account_number=$ACCT   sequence=$SEQ

24 სიტყვა არასდროს ტოვებს ოფლაინ ლეპტოპს. სერვერზე მხოლოდ signed.json ბრუნდება.

ნაბიჯები (ლეპტოპი ინტერნეტის გარეშე):
  1. გახსენი ეს საქაღალდე.
  2. Windows: ორჯერ დააწკაპუნე sign-OFFLINE.bat
     macOS/Linux: ტერმინალში  ./sign-OFFLINE.sh
  3. მოთხოვნაზე შეიყვანე Founder-ის 24 სიტყვა (ბიპ39 პაროლი ცარიელი — Enter).
  4. სკრიპტი დაბეჭდავს მისამართს. უნდა იყოს ზუსტად:
        $FOUNDER
     თუ სხვაა — შეწყვიტე, სიტყვები არასწორია. არაფერი არ გაიგზავნება.
  5. შეიქმნება signed.json. სკრიპტი გასაღებს თვითონ შლის (keyring-tmp).
  6. USB-ით სერვერზე გადმოიტანე მხოლოდ signed.json და გაუშვი:
        sudo /home/hashgram/scripts/launch/founder-delegate-broadcast.sh /path/signed.json

შემოწმება ხელმოწერამდე (სურვილისამებრ): unsigned.json-ის sha256 უნდა იყოს
  $(sha256sum "$KIT/unsigned.json" | cut -d' ' -f1)

რისკები, რომ იცოდე:
  - დელეგირებული HASH ვალიდატორის ჯარიმებს იზიარებს: downtime 0.01%, ორმაგი
    ხელმოწერა 5%. ვალიდატორი შენია; priv_validator_key.json არასდროს გააორმაგო.
  - გამოტანას (unbond) 21 დღე სჭირდება. ვესტინგის მონეტები ისედაც ჩაკეტილია,
    ამიტომ მათი დელეგირება ლიკვიდურობას არ აკარგვინებს.
  - ეს ხმის უფლებას Founder-ის მისამართზე წერს: governance-ში შენი ხმა
    სტეიკის პროპორციულია, და ≥33.4% = ვეტო ნებისმიერ ცვლილებაზე.
EOF

( cd "$KIT" && sha256sum unsigned.json SIGN_FACTS.txt sign-OFFLINE.bat sign-OFFLINE.sh $(find bin -type f | sort) > SHA256SUMS.txt )
rm -f "$ZIP"; ( cd /root && zip -qr "$ZIP" "$(basename "$KIT")" )

echo
echo "PREPARED (nothing signed, nothing broadcast)"
echo "  founder          $FOUNDER"
echo "  validator        $VALOPER"
echo "  amount           $AMOUNT_HASH HASH"
echo "  account/sequence $ACCT / $SEQ"
echo "  kit              $ZIP  ($(du -h "$ZIP" | cut -f1))"
echo "  kit sha256       $(sha256sum "$ZIP" | cut -d' ' -f1)"
echo
echo "Next: move the zip to the offline laptop, run sign-OFFLINE, bring back signed.json,"
echo "      then: founder-delegate-broadcast.sh signed.json"
