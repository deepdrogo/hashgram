#!/usr/bin/env bash
#
# Verify that every PromQL expression in the shipped dashboards and alert
# rules actually parses, and that the ones which should have data do.
#
#   scripts/dev/check-dashboards.sh            # parse-only, no Prometheus needed
#   scripts/dev/check-dashboards.sh --live     # also query a running Prometheus
#
# A Grafana panel whose expression is wrong renders as "No data", which looks
# identical to a healthy zero. That is the failure this script exists to
# catch, and it has already caught a real one: the Welcome tier panel used
# bare comparisons, which filter series out rather than yielding zero, so
# adding the three tier terms produced an empty vector and the panel read
# No data at every sequence below 10,000.
#
# See docs/OPERATIONS.md.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

PROM_URL="${PROM_URL:-http://127.0.0.1:9090}"
LIVE=0
[ "${1:-}" = "--live" ] && LIVE=1

RED=$'\033[31m'; GREEN=$'\033[32m'; YELLOW=$'\033[33m'; BOLD=$'\033[1m'; RESET=$'\033[0m'
FAILURES=0

ok()   { printf '  %s[ ok ]%s %s\n' "$GREEN" "$RESET" "$*"; }
bad()  { printf '  %s[FAIL]%s %s\n' "$RED" "$RESET" "$*"; FAILURES=$((FAILURES + 1)); }
warn() { printf '  %s[warn]%s %s\n' "$YELLOW" "$RESET" "$*"; }
head1() { printf '\n%s%s%s\n%s\n' "$BOLD" "$*" "$RESET" "=========================================================================="; }

DASH_DIR="deploy/monitoring/grafana/dashboards"
RULES="deploy/monitoring/rules/hashgram-alerts.yml"

# ---------------------------------------------------------------------------

head1 "DASHBOARD STRUCTURE"

for f in "$DASH_DIR"/*.json; do
  if python3 -c "import json,sys; json.load(open(sys.argv[1]))" "$f" 2>/dev/null; then
    ok "valid JSON: $(basename "$f")"
  else
    bad "invalid JSON: $(basename "$f")"
    continue
  fi
done

# Every panel that queries data must name the provisioned datasource uid, or
# Grafana falls back to the default and the dashboard breaks on any host where
# a different datasource is default.
python3 - "$DASH_DIR" <<'PY'
import json, os, sys

dash_dir = sys.argv[1]
problems = 0

for name in sorted(os.listdir(dash_dir)):
    if not name.endswith(".json"):
        continue
    d = json.load(open(os.path.join(dash_dir, name)))

    for panel in d.get("panels", []):
        if panel.get("type") == "row":
            continue
        title = panel.get("title", "<untitled>")
        ds = panel.get("datasource") or {}
        if ds.get("uid") != "hashgram-prometheus":
            print(f'  [FAIL] {name}: panel "{title}" does not use the hashgram-prometheus datasource')
            problems += 1
        if not panel.get("targets"):
            print(f'  [FAIL] {name}: panel "{title}" has no query')
            problems += 1

    if not d.get("uid"):
        print(f'  [FAIL] {name}: no uid, so it cannot be linked to stably')
        problems += 1
    if not d.get("description"):
        print(f'  [FAIL] {name}: no description')
        problems += 1

sys.exit(1 if problems else 0)
PY
if [ $? -eq 0 ]; then
  ok "every panel has a query and names the provisioned datasource"
else
  FAILURES=$((FAILURES + 1))
fi

# ---------------------------------------------------------------------------

head1 "PROMQL SYNTAX"

# promtool has no expression-check subcommand, so each expression is wrapped
# in a throwaway rules file, which is exactly the check we want.
if ! command -v promtool >/dev/null 2>&1; then
  warn "promtool not installed; skipping syntax check"
else
  TMPDIR_CHECK="$(mktemp -d)"
  trap 'rm -rf "$TMPDIR_CHECK"' EXIT

  python3 - "$DASH_DIR" "$TMPDIR_CHECK" <<'PY'
import json, os, sys

dash_dir, out_dir = sys.argv[1], sys.argv[2]
exprs = []

for name in sorted(os.listdir(dash_dir)):
    if not name.endswith(".json"):
        continue
    d = json.load(open(os.path.join(dash_dir, name)))
    for panel in d.get("panels", []):
        for i, t in enumerate(panel.get("targets", [])):
            e = t.get("expr")
            if e:
                exprs.append((f'{name}:{panel.get("title","?")}:{i}', e))

# Template variables such as $chain_id are not valid PromQL. Substituting a
# literal keeps the rest of the expression under test rather than skipping
# these panels entirely, which are the most complex ones.
with open(os.path.join(out_dir, "exprs.yml"), "w") as fh:
    fh.write("groups:\n  - name: dashboard-exprs\n    rules:\n")
    for i, (label, e) in enumerate(exprs):
        safe = e.replace("$chain_id", "placeholder")
        fh.write(f'      - record: check_{i}\n')
        fh.write(f'        expr: {json.dumps(safe)}\n')

with open(os.path.join(out_dir, "labels.txt"), "w") as fh:
    for label, _ in exprs:
        fh.write(label + "\n")
PY

  COUNT="$(python3 -c "print(sum(1 for _ in open('$TMPDIR_CHECK/labels.txt')))" 2>/dev/null || echo 0)"

  if promtool check rules "$TMPDIR_CHECK/exprs.yml" >"$TMPDIR_CHECK/out.log" 2>&1; then
    ok "all ${COUNT} dashboard expressions parse as valid PromQL"
  else
    bad "a dashboard expression is not valid PromQL:"
    sed 's/^/        /' "$TMPDIR_CHECK/out.log"
  fi

  if promtool check rules "$RULES" >"$TMPDIR_CHECK/rules.log" 2>&1; then
    ok "alert rules valid: $(grep -oE '[0-9]+ rules found' "$TMPDIR_CHECK/rules.log" | head -1)"
  else
    bad "alert rules are not valid:"
    sed 's/^/        /' "$TMPDIR_CHECK/rules.log"
  fi
fi

# ---------------------------------------------------------------------------

head1 "LIVE DATA"

if [ "$LIVE" -eq 0 ]; then
  printf '  Skipped. Pass --live to query %s.\n' "$PROM_URL"
  printf '  Syntax alone does not prove a panel shows anything: a valid query\n'
  printf '  against a metric that does not exist returns No data.\n'
else
  if ! curl -s --max-time 5 "$PROM_URL/-/ready" >/dev/null 2>&1; then
    bad "Prometheus is not ready at $PROM_URL"
  else
    PROM_URL="$PROM_URL" python3 - <<'PY'
import json, os, sys, urllib.parse, urllib.request

prom = os.environ["PROM_URL"]

def query(expr):
    url = prom + "/api/v1/query?" + urllib.parse.urlencode({"query": expr})
    try:
        d = json.load(urllib.request.urlopen(url, timeout=10))
    except Exception as exc:
        return None, str(exc)
    if d.get("status") != "success":
        return None, d.get("error", "query failed")
    result = d["data"]["result"]
    if not result:
        return None, "no data"
    return result[0]["value"][1], None

# Expressions that must return a value on any running node, and the ones that
# legitimately may not. The second list is as important as the first: an
# expression that is absent for a known structural reason must not be reported
# as a failure, or the check gets ignored.
required = {
    "chain height": "cometbft_consensus_latest_block_height",
    "validators": "cometbft_consensus_validators",
    "block interval p99":
        "histogram_quantile(0.99, sum by(le) "
        "(rate(cometbft_consensus_block_interval_seconds_bucket[5m])))",
    "supply total": "hashgram_supply_total_uhash",
    "supply over ceiling": "hashgram_supply_over_ceiling_uhash",
    "founder fee bps": "hashgram_founder_fee_basis_points",
    "reserve ratio": "hashgram_service_reserve_remaining_ratio",
    "providers by status": "hashgram_service_providers",
    "welcome next sequence": "hashgram_welcome_next_sequence",
    "welcome tier":
        "(hashgram_welcome_next_sequence <= bool 10000) * 50 "
        "+ ((hashgram_welcome_next_sequence > bool 10000) "
        "* (hashgram_welcome_next_sequence <= bool 100000)) * 5 "
        "+ ((hashgram_welcome_next_sequence > bool 100000) "
        "* (hashgram_welcome_next_sequence <= bool 1000000)) * 1",
    "realised founder bps":
        "10000 * hashgram_revenue_founder_share_uhash "
        "/ clamp_min(hashgram_revenue_qualifying_total_uhash, 1)",
    "cpu percent":
        '100 * (1 - avg(rate(node_cpu_seconds_total{mode="idle"}[5m])))',
    "memory percent":
        "100 * (1 - node_memory_MemAvailable_bytes / node_memory_MemTotal_bytes)",
    "disk percent":
        '100 * (1 - node_filesystem_avail_bytes{fstype!~"tmpfs|squashfs|overlay"} '
        '/ node_filesystem_size_bytes{fstype!~"tmpfs|squashfs|overlay"})',
    "clock offset": "node_timex_offset_seconds",
}

# May legitimately be absent, with the reason. These are checked so that the
# reason is recorded rather than rediscovered.
conditional = {
    "peers": (
        "cometbft_p2p_peers",
        "CometBFT creates this series on the first peer event, so it is "
        "absent on a node that has never peered",
    ),
    "p2p bandwidth by peer": (
        "sum by(peer_id) (rate(cometbft_p2p_peer_receive_bytes_total[5m]))",
        "per-peer series exist only while a peer is connected",
    ),
    "blocks behind signature": (
        "cometbft_consensus_latest_block_height - on(chain_id) group_left() "
        "max by(chain_id) (cometbft_consensus_validator_last_signed_height)",
        "the last-signed-height series exists only on a validator",
    ),
    "postgres up": (
        "pg_up",
        "the Postgres exporter is stopped until the phase 2 indexer exists",
    ),
}

failures = 0

for name, expr in sorted(required.items()):
    value, err = query(expr)
    if err:
        print(f"  [FAIL] {name}: {err}")
        failures += 1
    else:
        try:
            shown = f"{float(value):.6g}"
        except (TypeError, ValueError):
            shown = str(value)
        print(f"  [ ok ] {name} = {shown}")

for name, (expr, reason) in sorted(conditional.items()):
    value, err = query(expr)
    if err:
        print(f"  [warn] {name}: absent. {reason}")
    else:
        try:
            shown = f"{float(value):.6g}"
        except (TypeError, ValueError):
            shown = str(value)
        print(f"  [ ok ] {name} = {shown}")

sys.exit(1 if failures else 0)
PY
    if [ $? -ne 0 ]; then
      FAILURES=$((FAILURES + 1))
    fi
  fi
fi

# ---------------------------------------------------------------------------

head1 "RESULT"

if [ "$FAILURES" -eq 0 ]; then
  printf '  %sDashboards and alerts check out.%s\n\n' "$GREEN" "$RESET"
else
  printf '  %s%d problem(s).%s\n\n' "$RED" "$FAILURES" "$RESET"
fi

exit "$FAILURES"
