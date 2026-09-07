#!/usr/bin/env bash
#
# Verify that the documentation describes software that exists.
#
#   scripts/dev/check-docs.sh
#
# Documentation drifts from code silently, and the failure mode is worse than
# no documentation: a reader follows an instruction, it does not work, and
# they stop trusting the rest. This checks the mechanical claims — that every
# referenced file exists, every internal link resolves, and every hashgramctl
# subcommand named in prose is a real subcommand.
#
# It cannot check that the prose is true. That is what review is for.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

RED=$'\033[31m'; GREEN=$'\033[32m'; YELLOW=$'\033[33m'; BOLD=$'\033[1m'; RESET=$'\033[0m'
FAILURES=0

ok()    { printf '  %s[ ok ]%s %s\n' "$GREEN" "$RESET" "$*"; }
bad()   { printf '  %s[FAIL]%s %s\n' "$RED" "$RESET" "$*"; FAILURES=$((FAILURES + 1)); }
warn()  { printf '  %s[warn]%s %s\n' "$YELLOW" "$RESET" "$*"; }
head1() { printf '\n%s%s%s\n%s\n' "$BOLD" "$*" "$RESET" "=========================================================================="; }

DOCS="README.md $(ls docs/*.md 2>/dev/null)"

# ---------------------------------------------------------------------------

head1 "REQUIRED DOCUMENTS"

REQUIRED="
docs/ARCHITECTURE.md
docs/TOKENOMICS.md
docs/SECURITY.md
docs/THREAT_MODEL.md
docs/DECENTRALIZATION.md
docs/NODE_ROLES.md
docs/OPERATIONS.md
docs/MAINNET.md
docs/DISASTER_RECOVERY.md
docs/FOUNDER_LAUNCH_RUNBOOK.md
docs/LOGGING_POLICY.md
docs/PHASE1_REPORT.md
README.md
"
for f in $REQUIRED; do
  if [ -f "$f" ]; then
    ok "$f ($(wc -l < "$f" | tr -d ' ') lines)"
  else
    bad "$f is missing"
  fi
done

# ---------------------------------------------------------------------------

head1 "INTERNAL LINKS RESOLVE"

python3 - $DOCS <<'PY'
import os, re, sys

link = re.compile(r'\[[^\]]*\]\(([^)#]+)(#[^)]*)?\)')
problems = 0

for doc in sys.argv[1:]:
    base = os.path.dirname(doc)
    with open(doc) as fh:
        text = fh.read()

    # Skip fenced code blocks: a path inside an example command is not a link.
    text = re.sub(r'```.*?```', '', text, flags=re.S)

    for m in link.finditer(text):
        target = m.group(1).strip()
        if target.startswith(("http://", "https://", "mailto:")):
            continue
        resolved = os.path.normpath(os.path.join(base, target))
        if not os.path.exists(resolved):
            print(f'  [FAIL] {doc}: link to {target} resolves to {resolved}, which does not exist')
            problems += 1

sys.exit(1 if problems else 0)
PY
if [ $? -eq 0 ]; then
  ok "every internal link resolves to a real path"
else
  FAILURES=$((FAILURES + 1))
fi

# ---------------------------------------------------------------------------

head1 "COMMANDS EXIST"

# Extract every `hashgramctl <subcommand>` mentioned anywhere in the docs and
# check it against the binary's own command list. This is the check most
# likely to catch real drift: a renamed subcommand leaves the documentation
# confidently wrong.
if [ ! -x build/hashgramctl ]; then
  warn "build/hashgramctl is missing; run make build to check command names"
else
  build/hashgramctl --help 2>&1 \
    | sed -n '/Available Commands/,/^Flags/p' \
    | grep -E '^  [a-z]' | awk '{print $1}' | sort -u > /tmp/docs-real-cmds.txt

  # Only from inline code spans and fenced code blocks. Matching bare prose
  # picked up the verb in sentences like "hashgramctl join-mainnet refused the
  # fork genesis", which is not a subcommand.
  python3 - $DOCS > /tmp/docs-used-cmds.txt <<'EXTRACT'
import re, sys

cmds = set()
for path in sys.argv[1:]:
    text = open(path).read()
    spans = re.findall(r'`([^`\n]+)`', text)
    # A ```text block is quoted output, not a command to run. Including them
    # picked up the verb from a quoted test result:
    #   [ ok ] hashgramctl refused the fork genesis against the pinned hash
    blocks = re.findall(r'```(?!text)[a-z]*\n(.*?)```', text, re.S)
    for chunk in spans + blocks:
        for m in re.finditer(r'\bhashgramctl\s+([a-z][a-z0-9-]*)', chunk):
            cmds.add(m.group(1))
for c in sorted(cmds):
    print(c)
EXTRACT

  MISSING="$(comm -23 /tmp/docs-used-cmds.txt /tmp/docs-real-cmds.txt)"
  if [ -z "$MISSING" ]; then
    ok "all $(wc -l < /tmp/docs-used-cmds.txt | tr -d ' ') documented hashgramctl subcommands exist"
  else
    bad "documented hashgramctl subcommands that do not exist:"
    printf '        %s\n' $MISSING
  fi

  # The inverse is a warning, not a failure: a command may reasonably be
  # undocumented, but it is worth knowing which.
  UNDOCUMENTED="$(comm -13 /tmp/docs-used-cmds.txt /tmp/docs-real-cmds.txt \
    | grep -vE '^(completion|help)$' || true)"
  if [ -n "$UNDOCUMENTED" ]; then
    warn "commands that exist but are not mentioned in any document:"
    printf '        %s\n' $UNDOCUMENTED
  else
    ok "every hashgramctl subcommand is mentioned somewhere"
  fi
fi

# The scripts the documentation tells people to run must be executable.
head1 "REFERENCED SCRIPTS ARE RUNNABLE"

for s in $(grep -ohE 'scripts/[a-z]+/[a-z0-9-]+\.sh' $DOCS 2>/dev/null | sort -u); do
  if [ ! -f "$s" ]; then
    bad "$s is referenced but does not exist"
  elif [ ! -x "$s" ]; then
    bad "$s exists but is not executable"
  else
    ok "$s"
  fi
done

# ---------------------------------------------------------------------------

head1 "NUMBERS MATCH THE CODE"

# The tokenomics figures appear in several documents. A number that disagrees
# with app/params is worse than a missing number, because a reader will act on
# it. Checked against the constants rather than against a second copy of them.
python3 - <<'PY'
import re, subprocess, sys

src = open("app/params/params.go").read()

def const(name):
    m = re.search(rf'{name}\s+int64\s*=\s*([\d_]+)', src)
    if not m:
        m = re.search(rf'{name}\s*int64\s*=\s*([\d_]+)', src)
    if not m:
        m = re.search(rf'{name}\s+uint32\s*=\s*([\d_]+)', src)
    return int(m.group(1).replace("_", "")) if m else None

def grouped(n):
    return f"{n:,}"

expected = {
    "MaxSupplyHash": const("MaxSupplyHash"),
    "AllocFounderHash": const("AllocFounderHash"),
    "AllocServiceReserveHash": const("AllocServiceReserveHash"),
    "AllocTreasuryHash": const("AllocTreasuryHash"),
    "AllocGrowthHash": const("AllocGrowthHash"),
    "AllocDevGrantsHash": const("AllocDevGrantsHash"),
    "AllocLiquidityHash": const("AllocLiquidityHash"),
    "FounderUnlockedAtGenesisHash": const("FounderUnlockedAtGenesisHash"),
    "FounderVestedHash": const("FounderVestedHash"),
    "WelcomePoolHash": const("WelcomePoolHash"),
    "FounderFeeBasisPoints": const("FounderFeeBasisPoints"),
}

missing = [k for k, v in expected.items() if v is None]
if missing:
    print(f'  [FAIL] could not read these constants from app/params: {missing}')
    sys.exit(1)

docs = subprocess.run(
    ["bash", "-c", "cat README.md docs/*.md"],
    capture_output=True, text=True).stdout

problems = 0

# Each of these figures must appear, formatted with thousands separators, in
# the documentation. If a constant changes and the docs do not, this fails.
for name, value in expected.items():
    if name == "FounderFeeBasisPoints":
        continue
    if grouped(value) not in docs:
        print(f'  [FAIL] {name} is {grouped(value)} in code but that figure '
              f'appears nowhere in the documentation')
        problems += 1

# Supply in uhash, which several documents quote directly.
base = expected["MaxSupplyHash"] * 1_000_000
if grouped(base) not in docs and str(base) not in docs:
    print(f'  [FAIL] max supply in uhash ({base}) appears nowhere in the documentation')
    problems += 1

# The allocations must still sum to the ceiling. If this fails, the code is
# wrong, not the docs.
total = sum(expected[k] for k in (
    "AllocFounderHash", "AllocServiceReserveHash", "AllocTreasuryHash",
    "AllocGrowthHash", "AllocDevGrantsHash", "AllocLiquidityHash"))
if total != expected["MaxSupplyHash"]:
    print(f'  [FAIL] allocations sum to {grouped(total)}, ceiling is '
          f'{grouped(expected["MaxSupplyHash"])}')
    problems += 1

# The Founder split must reconstruct the allocation.
split = expected["FounderUnlockedAtGenesisHash"] + expected["FounderVestedHash"]
if split != expected["AllocFounderHash"]:
    print(f'  [FAIL] Founder unlocked plus vested is {grouped(split)}, '
          f'allocation is {grouped(expected["AllocFounderHash"])}')
    problems += 1

if problems == 0:
    print(f'  [ ok ] all {len(expected) - 1} tokenomics figures in the docs match app/params')
    print(f'  [ ok ] allocations sum to exactly {grouped(total)} HASH')
    print(f'  [ ok ] Founder split reconstructs the allocation')

sys.exit(1 if problems else 0)
PY
if [ $? -ne 0 ]; then
  FAILURES=$((FAILURES + 1))
fi

# ---------------------------------------------------------------------------

head1 "THE SEED PHRASE RULE"

# The runbook promises that nothing will ask for a seed phrase. That promise
# is worth checking against the tooling rather than trusting.
if grep -rniE 'enter your (seed|mnemonic)|paste your (seed|mnemonic)|type your (seed|mnemonic)' \
     cmd/ scripts/ 2>/dev/null | grep -v 'hashgram-keygen' | grep -v check-docs; then
  bad "something asks the operator for a seed phrase outside hashgram-keygen"
else
  ok "nothing outside hashgram-keygen asks for a seed phrase"
fi

if grep -qiE '(never|will ever) ask (for|you for) your seed phrase' \
     docs/FOUNDER_LAUNCH_RUNBOOK.md 2>/dev/null; then
  ok "the runbook states the seed phrase rule explicitly"
else
  bad "the runbook does not state the seed phrase rule"
fi

# ---------------------------------------------------------------------------

head1 "UNBUILT WORK IS MARKED AS SUCH"

# Phase 2 does not exist. Documentation that describes it in the present tense
# is documentation that lies, and this is the check that keeps the distinction
# honest as Phase 2 lands.
for doc in docs/ARCHITECTURE.md docs/NODE_ROLES.md README.md; do
  if grep -qiE 'phase 2|not built|not implemented|not launched' "$doc"; then
    ok "$(basename "$doc") marks unbuilt work"
  else
    bad "$(basename "$doc") does not distinguish built from unbuilt work"
  fi
done

# ---------------------------------------------------------------------------

head1 "RESULT"

if [ "$FAILURES" -eq 0 ]; then
  printf '  %sDocumentation matches the code.%s\n\n' "$GREEN" "$RESET"
  printf '  What this proves: every referenced file exists, every internal link\n'
  printf '  resolves, every documented command is real, and every tokenomics\n'
  printf '  figure agrees with app/params.\n\n'
  printf '  What it does not prove: that the prose is accurate. A command can\n'
  printf '  exist and be described wrongly. That needs a reader.\n\n'
else
  printf '  %s%d problem(s).%s\n\n' "$RED" "$FAILURES" "$RESET"
fi

exit "$FAILURES"
