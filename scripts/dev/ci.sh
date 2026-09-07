#!/usr/bin/env bash
#
# The full Hashgram CI pipeline, runnable locally.
#
#   scripts/dev/ci.sh              # everything
#   scripts/dev/ci.sh fast         # skip the slow stages (tests with -race, vuln)
#   scripts/dev/ci.sh <stage>      # one stage: fmt vet lint sec vuln secrets test policy
#
# The same script runs in GitHub Actions (.github/workflows/ci.yml), so a green
# local run means a green CI run. A pipeline that only exists inside a CI
# provider is a pipeline nobody runs before pushing.
#
# Exit code is the number of failed stages.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

export PATH="$PATH:/usr/local/go/bin:$(go env GOPATH 2>/dev/null)/bin:$HOME/go/bin"

RED=$'\033[31m'; GREEN=$'\033[32m'; YELLOW=$'\033[33m'; BOLD=$'\033[1m'; RESET=$'\033[0m'
FAILED_STAGES=()

stage() { printf '\n%s>>> %s%s\n' "$BOLD" "$*" "$RESET"; }
ok()    { printf '  %s[ ok ]%s %s\n' "$GREEN" "$RESET" "$*"; }
bad()   { printf '  %s[FAIL]%s %s\n' "$RED" "$RESET" "$*"; }
warn()  { printf '  %s[warn]%s %s\n' "$YELLOW" "$RESET" "$*"; }

fail_stage() { FAILED_STAGES+=("$1"); bad "$2"; }

# Generated code we do not author and cannot fix. Filtering here rather than
# disabling checks globally, so that a genuine deprecated-API use in our own
# code is still reported.
GOBIN_DIR="$REPO_ROOT/build"
GENERATED_RE='\.pb\.go|\.pb\.gw\.go|/proto/third_party/'

WANT="${1:-all}"
# want <stage> [fast]
#
# The second argument marks a stage as fast enough to run in `ci.sh fast`.
# Defaulted because stages that are never fast pass only one argument, and
# under `set -u` a bare $2 aborted the script the first time WANT was "fast" —
# which happened to be after gitleaks, so the failure looked like a gitleaks
# problem rather than a bug in this function.
want() {
  [ "$WANT" = "all" ] || [ "$WANT" = "$1" ] ||
    { [ "$WANT" = "fast" ] && [ "${2:-}" = "fast" ]; }
}

# ---------------------------------------------------------------------------

if want fmt fast; then
  stage "gofmt"
  UNFORMATTED="$(gofmt -l app cmd x genesis tools indexer safety 2>/dev/null | grep -vE "$GENERATED_RE" || true)"
  if [ -z "$UNFORMATTED" ]; then
    ok "all files formatted"
  else
    fail_stage fmt "unformatted files:"
    printf '        %s\n' $UNFORMATTED
    warn "fix with: gofmt -w ."
  fi
fi

# ---------------------------------------------------------------------------

if want vet fast; then
  stage "go vet"
  if go vet ./... >/tmp/ci-vet.log 2>&1; then
    ok "go vet clean"
  else
    fail_stage vet "go vet reported problems:"
    sed 's/^/        /' /tmp/ci-vet.log | head -40
  fi
fi

# ---------------------------------------------------------------------------

if want lint fast; then
  stage "staticcheck"
  if ! command -v staticcheck >/dev/null 2>&1; then
    warn "staticcheck not installed: go install honnef.co/go/tools/cmd/staticcheck@latest"
  else
    # JSON output so generated files can be filtered by path precisely rather
    # than by grepping human-readable lines.
    staticcheck -f json ./... >/tmp/ci-staticcheck.json 2>/tmp/ci-staticcheck.err || true

    REAL="$(python3 - <<'PY'
import json, re

generated = re.compile(r"\.pb\.go$|\.pb\.gw\.go$|/proto/third_party/")
findings = []

with open("/tmp/ci-staticcheck.json") as fh:
    for line in fh:
        line = line.strip()
        if not line:
            continue
        try:
            d = json.loads(line)
        except json.JSONDecodeError:
            continue
        loc = d.get("location", {})
        path = loc.get("file", "")
        if generated.search(path):
            continue
        findings.append(
            f'{path}:{loc.get("line")}:{loc.get("column")}: '
            f'{d.get("message")} ({d.get("code")})'
        )

for f in findings:
    print(f)
PY
)"

    if [ -z "$REAL" ]; then
      TOTAL="$(wc -l < /tmp/ci-staticcheck.json | tr -d ' ')"
      ok "staticcheck clean on authored code (${TOTAL} findings, all in generated files)"
    else
      fail_stage lint "staticcheck findings in authored code:"
      printf '%s\n' "$REAL" | sed 's/^/        /' | head -40
    fi
  fi
fi

# ---------------------------------------------------------------------------

if want sec fast; then
  stage "gosec"
  if ! command -v gosec >/dev/null 2>&1; then
    warn "gosec not installed: go install github.com/securego/gosec/v2/cmd/gosec@latest"
  else
    gosec -quiet -fmt json -out /tmp/ci-gosec.json -exclude-generated ./... >/dev/null 2>&1 || true

    GOSEC_OUT="$(python3 - <<'PY'
import json, os, re

path = "/tmp/ci-gosec.json"
if not os.path.exists(path):
    print("NOFILE")
    raise SystemExit

with open(path) as fh:
    d = json.load(fh)

generated = re.compile(r"\.pb\.go$|\.pb\.gw\.go$|/proto/third_party/")

# Rules that are meaningful in the state machine and not in the operator
# CLI, scoped by path rather than disabled globally.
#
# G204 subprocess launch, G304 file open from a variable, G702 command
# injection and G703 path traversal all describe a program acting on
# attacker-controlled input. In cmd/ the "attacker-controlled input" is a
# command-line flag typed by the operator, and the process already holds
# exactly that operator's privileges: hashgramctl exists in order to run
# systemctl and hashgramd and to read and write files under a --home the
# operator chose. There is no boundary being crossed, so the findings are
# noise there.
#
# They are emphatically not noise in app/ or x/. A subprocess launch or a
# variable file open inside the state machine would mean consensus code
# touching the host, which is a genuine alarm, so these rules stay enabled
# everywhere except the CLI tree.
#
# G301 directory permissions and G306 file permissions are also scoped here,
# for a different and more specific reason. hashgramctl runs as root under
# sudo, while the node runs as the unprivileged hashgram-chain user. A
# directory this tool creates at 0750 root:root is a directory the service
# user cannot traverse, so the node would fail to start. What protects the
# sensitive file is its own mode, not the directory's: hashgramd writes
# priv_validator_key.json at 0600, and `hashgramctl mainnet-preflight` fails
# the launch if that key is readable by group or others. The files this tool
# writes at 0644 are genesis.json, config.toml and the network pin, all of
# which are public data by design; genesis in particular is meant to be
# readable and hash-comparable by anyone.
cli_only = {"G204", "G304", "G702", "G703", "G301", "G306"}
cli_path = re.compile(r"/cmd/|/tools/")

# G404 is "use of weak random". In test code seeding deterministic fixtures,
# math/rand is the correct choice: crypto/rand would make the tests
# irreproducible. Filtered only in _test.go, never in production code.
def keep(issue):
    f = issue.get("file", "")
    if generated.search(f):
        return False
    rule = issue.get("rule_id")
    if f.endswith("_test.go") and rule == "G404":
        return False
    if rule in cli_only and cli_path.search(f):
        return False
    return True

issues = [i for i in d.get("Issues", []) if keep(i)]

by_sev = {}
for i in issues:
    by_sev.setdefault(i.get("severity", "?"), []).append(i)

if not issues:
    stats = d.get("Stats", {})
    print(f'CLEAN {stats.get("files", 0)} {stats.get("lines", 0)}')
else:
    for sev in ("HIGH", "MEDIUM", "LOW"):
        for i in by_sev.get(sev, []):
            print(f'{sev} {i["file"]}:{i["line"]}: {i["details"]} ({i["rule_id"]})')
PY
)"

    if [ "$GOSEC_OUT" = "NOFILE" ]; then
      warn "gosec produced no report"
    elif printf '%s' "$GOSEC_OUT" | grep -q '^CLEAN'; then
      set -- $GOSEC_OUT
      ok "gosec clean: $2 files, $3 lines scanned"
    elif printf '%s' "$GOSEC_OUT" | grep -q '^HIGH'; then
      fail_stage sec "gosec found high-severity issues:"
      printf '%s\n' "$GOSEC_OUT" | sed 's/^/        /' | head -30
    else
      warn "gosec found non-high-severity issues:"
      printf '%s\n' "$GOSEC_OUT" | sed 's/^/        /' | head -30
    fi
  fi
fi

# ---------------------------------------------------------------------------

if want secrets fast; then
  stage "gitleaks"
  if ! command -v gitleaks >/dev/null 2>&1; then
    warn "gitleaks not installed: go install github.com/zricethezav/gitleaks/v8@latest"
  else
    # Scan the working tree, not only git history: a secret staged but not yet
    # committed should be caught before it becomes permanent. Only files git
    # would commit are scanned (tracked plus untracked-and-not-ignored), so a
    # 30 GB Rust target directory or a devnet data directory does not turn a
    # five-second check into an hour. Ignored directories are covered by the
    # acceptance suites' own disk scans, not by this stage.
    SCAN_DIR="$(mktemp -d /tmp/ci-gitleaks-tree.XXXXXX)"
    git ls-files -z --cached --others --exclude-standard \
      | tar --null -T - -cf - 2>/dev/null | tar -C "$SCAN_DIR" -xf -
    cp -f .gitleaks.toml "$SCAN_DIR/" 2>/dev/null || true
    if gitleaks dir "$SCAN_DIR" --no-banner --redact \
         --report-format json --report-path /tmp/ci-gitleaks.json \
         >/tmp/ci-gitleaks.log 2>&1; then
      rm -rf "$SCAN_DIR"
      ok "no secrets found in the working tree"
    else
      COUNT="$(python3 -c '
import json
try:
    print(len(json.load(open("/tmp/ci-gitleaks.json"))))
except Exception:
    print("?")
' 2>/dev/null)"
      fail_stage secrets "gitleaks found ${COUNT} potential secret(s):"
      python3 - <<'PY' | sed 's/^/        /' | head -30
import json
try:
    for f in json.load(open("/tmp/ci-gitleaks.json")):
        print(f'{f.get("File")}:{f.get("StartLine")}: {f.get("Description")} [{f.get("RuleID")}]')
except Exception as exc:
    print(f"could not read the report: {exc}")
PY
      rm -rf "$SCAN_DIR"
    fi
  fi
fi

# ---------------------------------------------------------------------------

if want vuln; then
  stage "govulncheck"
  if ! command -v govulncheck >/dev/null 2>&1; then
    warn "govulncheck not installed: go install golang.org/x/vuln/cmd/govulncheck@latest"
  elif [ ! -x "$GOBIN_DIR/hashgramd" ] && [ ! -x build/hashgramd ]; then
    warn "build/hashgramd is missing; run make build first"
  else
    # Binary mode rather than source mode.
    #
    # Source mode builds the whole call graph of the Cosmos dependency tree
    # and was killed by the OOM reaper on a 8 GB host. Binary mode reads the
    # symbol table of the built artefact instead: it finishes in under two
    # seconds, and it answers a strictly better question, which is "is the
    # vulnerable code in the thing we are going to ship" rather than "is it
    # anywhere in the module graph".
    #
    # The tradeoff is that binary mode cannot distinguish a symbol that is
    # linked in from one that is actually called. The parser below therefore
    # separates symbol-level findings, where the vulnerable function is
    # present in the binary, from module-level ones, where only the dependency
    # is. Only the former fail the stage.
    if govulncheck -mode=binary -format json build/hashgramd \
         >/tmp/ci-vuln.json 2>/tmp/ci-vuln.err; then
      VULN_OUT="$(python3 - <<'PY'
import json

# Findings reviewed and accepted, with the evidence. An allowlist without a
# recorded reason becomes a way to ignore security findings, so each entry
# below states what was checked and how.
ACCEPTED = {
    # Verified false positive. ASA-2024-005 (GHSA-86h5-xcpx-cfqc) was patched
    # in Cosmos SDK 0.50.5 and 0.47.10; this tree is on v0.53.8. The Go vuln
    # database entry carries a second affected range, "introduced 0.50.0",
    # with no corresponding "fixed" event, so every SDK release in the 0.50+
    # line matches it regardless of the patch. The upstream advisory text is
    # unambiguous that 0.50.5 is patched.
    "GO-2024-2584": "false positive: patched in SDK 0.50.5, this tree is v0.53.8",

    # golang.org/x/crypto/openpgp is deprecated with no fix available: the
    # advisory is "do not use this package". It is not imported by Hashgram.
    # It reaches the binary through cosmossdk.io/x/upgrade, which depends on
    # hashicorp/go-getter for its optional binary-download feature, and
    # go-getter offers PGP verification of downloads. Hashgram does not use
    # automatic binary downloads: upgrades are applied by an operator
    # installing a binary whose checksum they verified, so this code is
    # linked but unreachable.
    "GO-2026-5932": "x/crypto/openpgp linked via x/upgrade -> go-getter; "
                    "Hashgram does not use automatic binary downloads",
}

def stream(path):
    """govulncheck emits concatenated JSON objects, not an array."""
    buf, depth, instr, esc = "", 0, False, False
    with open(path) as fh:
        for ch in fh.read():
            buf += ch
            if instr:
                if esc:
                    esc = False
                elif ch == "\\":
                    esc = True
                elif ch == '"':
                    instr = False
                continue
            if ch == '"':
                instr = True
            elif ch == "{":
                depth += 1
            elif ch == "}":
                depth -= 1
                if depth == 0:
                    try:
                        yield json.loads(buf)
                    except json.JSONDecodeError:
                        pass
                    buf = ""

summaries, findings = {}, {}
for d in stream("/tmp/ci-vuln.json"):
    osv = d.get("osv")
    if osv and osv.get("id"):
        summaries[osv["id"]] = osv.get("summary", "")
    f = d.get("finding")
    if not f:
        continue
    level = "symbol" if any(t.get("function") for t in f.get("trace", [])) else "module"
    prev = findings.get(f["osv"])
    if level == "symbol" or prev is None or prev[0] != "symbol":
        findings[f["osv"]] = (level, f.get("fixed_version") or "none")

symbol = {k: v for k, v in findings.items() if v[0] == "symbol"}
module_only = sum(1 for v in findings.values() if v[0] != "symbol")

unaccepted = {k: v for k, v in symbol.items() if k not in ACCEPTED}
accepted = {k: v for k, v in symbol.items() if k in ACCEPTED}

print(f"MODULEONLY {module_only}")
for osv, (_, fixed) in sorted(accepted.items()):
    print(f"ACCEPTED {osv} :: {ACCEPTED[osv]}")
for osv, (_, fixed) in sorted(unaccepted.items()):
    print(f"FAIL {osv} fixed_in={fixed} :: {summaries.get(osv, '')}")
PY
)"
      MODULE_ONLY="$(printf '%s' "$VULN_OUT" | awk '/^MODULEONLY/ {print $2}')"
      ACCEPTED_N="$(printf '%s' "$VULN_OUT" | grep -c '^ACCEPTED' || true)"

      if printf '%s' "$VULN_OUT" | grep -q '^FAIL '; then
        fail_stage vuln "vulnerable code present in build/hashgramd:"
        printf '%s\n' "$VULN_OUT" | grep '^FAIL ' | sed 's/^FAIL /        /'
      else
        ok "no unreviewed vulnerabilities in build/hashgramd"
        printf '        %s dependency advisories matched at module level only\n' \
          "${MODULE_ONLY:-0}"
        printf '        %s reviewed and accepted, with reasons in this script\n' \
          "${ACCEPTED_N:-0}"
        printf '%s\n' "$VULN_OUT" | grep '^ACCEPTED' | sed 's/^ACCEPTED /        - /'
      fi
    else
      warn "govulncheck could not complete:"
      sed 's/^/        /' /tmp/ci-vuln.err | head -10
    fi
  fi
fi

# ---------------------------------------------------------------------------

if want policy fast; then
  stage "policy checks"
  if scripts/dev/check-logging.sh >/tmp/ci-logging.log 2>&1; then
    ok "logging policy holds"
  else
    fail_stage policy "logging policy violated:"
    grep -E 'FAIL' /tmp/ci-logging.log | sed 's/^/        /' | head -20
  fi

  if scripts/dev/check-dashboards.sh >/tmp/ci-dash.log 2>&1; then
    ok "dashboards and alert rules valid"
  else
    fail_stage policy "dashboard or alert problem:"
    grep -E 'FAIL' /tmp/ci-dash.log | sed 's/^/        /' | head -20
  fi

  # Documentation drift is silent, and the failure mode is a reader following
  # an instruction that does not work and then distrusting everything else.
  if scripts/dev/check-docs.sh >/tmp/ci-docs.log 2>&1; then
    ok "documentation matches the code"
  else
    fail_stage policy "documentation does not match the code:"
    grep -E 'FAIL' /tmp/ci-docs.log | sed 's/^/        /' | head -20
  fi
fi

# ---------------------------------------------------------------------------

if want rust fast; then
  stage "rust: fmt, clippy, test"
  if ! command -v cargo >/dev/null 2>&1; then
    warn "cargo not installed; skipping the Rust workspace"
  else
    if (cd node && cargo fmt --all -- --check) >/tmp/ci-rustfmt.log 2>&1; then
      ok "rustfmt clean"
    else
      fail_stage rust "rustfmt found unformatted files:"
      grep -E '^Diff in' /tmp/ci-rustfmt.log | sed 's/^/        /' | head -20
      warn "fix with: cd node && cargo fmt --all"
    fi

    # The workspace denies panicking constructs in non-test code, so clippy
    # failing here is a real defect rather than a style opinion: an unwrap on
    # a malformed frame in a network daemon is a remote denial of service.
    if (cd node && cargo clippy --all-targets --all-features -- -D warnings) \
         >/tmp/ci-clippy.log 2>&1; then
      ok "clippy clean with the workspace deny list"
    else
      fail_stage rust "clippy findings:"
      grep -E '^(error|warning)' /tmp/ci-clippy.log | sed 's/^/        /' | head -25
    fi

    if (cd node && cargo test --all) >/tmp/ci-rusttest.log 2>&1; then
      COUNT="$(grep -ohE '[0-9]+ passed' /tmp/ci-rusttest.log \
        | awk '{s+=$1} END {print s+0}')"
      ok "all Rust tests pass (${COUNT} tests, including the decoder fuzz smoke)"
    else
      fail_stage rust "Rust tests failed:"
      grep -E '^(test .* FAILED|panicked|assertion)' /tmp/ci-rusttest.log \
        | sed 's/^/        /' | head -25
    fi

    # Known-vulnerability scan of Cargo.lock. Tolerated advisories are listed
    # with reasons in node/.cargo/audit.toml; anything else fails.
    if command -v cargo-audit >/dev/null 2>&1; then
      if (cd node && cargo audit) >/tmp/ci-cargo-audit.log 2>&1; then
        ok "cargo audit: no unlisted advisories"
      else
        fail_stage rust "cargo audit findings:"
        grep -E '^(Crate|ID|Title):' /tmp/ci-cargo-audit.log | sed 's/^/        /' | head -20
      fi
    else
      warn "cargo-audit not installed (cargo install cargo-audit); skipping the Rust advisory scan"
    fi
  fi
fi

# ---------------------------------------------------------------------------

if want fuzz; then
  stage "fuzz: Go native fuzzers (short)"
  for pkg in ./indexer ./safety ./x/serviceproof/types; do
    for fn in $(grep -ho 'func Fuzz[A-Za-z0-9_]*' "$pkg"/*_test.go 2>/dev/null | awk '{print $2}'); do
      if go test "$pkg" -run='^$' -fuzz="^${fn}\$" -fuzztime="${FUZZ_SECONDS:-10}s" >/tmp/ci-fuzz.log 2>&1; then
        ok "$pkg $fn"
      else
        fail_stage fuzz "$pkg $fn:"
        tail -20 /tmp/ci-fuzz.log | sed 's/^/        /'
      fi
    done
  done
fi

if want test; then
  stage "go test -race"
  if go test -race -timeout 20m ./... >/tmp/ci-test.log 2>&1; then
    PKGS="$(grep -c '^ok' /tmp/ci-test.log || true)"
    ok "all tests pass (${PKGS} packages)"
  else
    fail_stage test "tests failed:"
    grep -E '^(---|FAIL|\s+.*_test\.go)' /tmp/ci-test.log | sed 's/^/        /' | head -40
  fi
fi

if [ "$WANT" = "fast" ]; then
  stage "go test (no race)"
  if go test -timeout 10m ./... >/tmp/ci-test-fast.log 2>&1; then
    ok "all tests pass"
  else
    fail_stage test "tests failed:"
    grep -E '^(---|FAIL)' /tmp/ci-test-fast.log | sed 's/^/        /' | head -40
  fi
fi

# ---------------------------------------------------------------------------

printf '\n%s%s%s\n' "$BOLD" "==========================================================================" "$RESET"
if [ ${#FAILED_STAGES[@]} -eq 0 ]; then
  printf '  %sCI passed.%s\n\n' "$GREEN" "$RESET"
  exit 0
fi

printf '  %s%d stage(s) failed: %s%s\n\n' "$RED" "${#FAILED_STAGES[@]}" "${FAILED_STAGES[*]}" "$RESET"
exit "${#FAILED_STAGES[@]}"
