#!/usr/bin/env bash
#
# Enforce the Hashgram logging policy: message plaintext, private keys and
# seed phrases are never written to a log, at any level, in any build.
#
#   scripts/dev/check-logging.sh
#
# A policy document that nothing checks is a policy that will be violated by
# the third person who touches the messaging code, and the violation will be
# discovered by whoever reads the logs of a production node. This script is
# the enforcement, and it runs in CI.
#
# It is a grep, so it is neither complete nor authoritative: it catches the
# patterns that have historically leaked plaintext in messaging systems. Code
# review remains the real defence. What it does guarantee is that the obvious
# mistakes cannot be merged silently.
#
# See docs/LOGGING_POLICY.md.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

RED=$'\033[31m'; GREEN=$'\033[32m'; YELLOW=$'\033[33m'; BOLD=$'\033[1m'; RESET=$'\033[0m'
FAILURES=0

say()  { printf '%s\n' "$*"; }
ok()   { printf '  %s[ ok ]%s %s\n' "$GREEN" "$RESET" "$*"; }
bad()  { printf '  %s[FAIL]%s %s\n' "$RED" "$RESET" "$*"; FAILURES=$((FAILURES + 1)); }
warn() { printf '  %s[warn]%s %s\n' "$YELLOW" "$RESET" "$*"; }

printf '%sHASHGRAM LOGGING POLICY CHECK%s\n' "$BOLD" "$RESET"
printf '=========================================================================\n'

# Directories that hold real code. Excluding vendored protos and generated
# files, which we do not control and which contain no logging.
SEARCH_PATHS=(app cmd x genesis tools node sdk apps services)
EXISTING=()
for p in "${SEARCH_PATHS[@]}"; do
  [ -d "$p" ] && EXISTING+=("$p")
done

# Generated protobuf code contains field names like Plaintext in String()
# methods, which is not a logging call. Excluded by filename.
EXCLUDE=(
  --glob '!**/*.pb.go'
  --glob '!**/*.pb.gw.go'
  --glob '!**/*_test.go'
  --glob '!**/testdata/**'
  --glob '!**/node_modules/**'
  --glob '!**/target/**'
  --glob '!**/dist/**'
  --glob '!**/*.test.ts'
  --glob '!**/*.test.tsx'
)

# ---------------------------------------------------------------------------
# 1. Plaintext, ciphertext bodies and message content in log calls.
#
# The pattern targets a logging call on the same line as a field name that
# would carry message content. Matching per-line rather than per-statement
# keeps it readable and catches the realistic case, which is someone adding a
# field to an existing log line.
# ---------------------------------------------------------------------------
say ""
say "1. Message content in log calls"

CONTENT_PATTERN='(Logger|logger|log)\s*(\(\))?\.\s*(Debug|Info|Warn|Error|Trace|Printf|Println|Print|Fatal|Panic)[a-zA-Z]*\s*\(.*(plaintext|plainText|PlainText|cleartext|clearText|messageBody|message_body|msgBody|decrypted|Decrypted)'

if [ ${#EXISTING[@]} -gt 0 ] && \
   rg --pcre2 -n "${EXCLUDE[@]}" "$CONTENT_PATTERN" "${EXISTING[@]}" 2>/dev/null; then
  bad "a log call references message plaintext (matches above)"
else
  ok "no log call references plaintext, cleartext or a decrypted body"
fi

# ---------------------------------------------------------------------------
# 2. Private keys, seeds and mnemonics in log calls.
#
# hashgram-keygen handles mnemonics and prints them to stdout on purpose. That
# is a deliberate interactive output for a tool that runs offline, not
# logging, so it is excluded by path and called out here so the exclusion is
# visible rather than hidden in a glob.
# ---------------------------------------------------------------------------
say ""
say "2. Key material in log calls"

KEY_PATTERN='(Logger|logger|log)\s*(\(\))?\.\s*(Debug|Info|Warn|Error|Trace|Printf|Println|Print|Fatal|Panic)[a-zA-Z]*\s*\(.*(mnemonic|Mnemonic|seedPhrase|seed_phrase|privKey|privateKey|PrivateKey|privValidatorKey|secretKey|SecretKey)'

if [ ${#EXISTING[@]} -gt 0 ] && \
   rg --pcre2 -n "${EXCLUDE[@]}" --glob '!cmd/hashgram-keygen/**' \
      "$KEY_PATTERN" "${EXISTING[@]}" 2>/dev/null; then
  bad "a log call references key material (matches above)"
else
  ok "no log call references a mnemonic, seed phrase or private key"
fi

# ---------------------------------------------------------------------------
# 2b. Rust `tracing` macros and TypeScript `console` calls (SDK, node,
#     desktop). Same idea as 1 and 2 for the other two languages: a log macro
#     on one line with a field that would carry content or key material. The
#     desktop additionally treats mail subjects, file names and passphrases as
#     content (docs/PRIVACY_MODEL.md).
# ---------------------------------------------------------------------------
say ""
say "2b. Rust tracing / TypeScript console calls with content or key material"

RUST_LOG_PATTERN='\b(trace|debug|info|warn|error)!\s*\(.*\b(plaintext|cleartext|decrypted|mnemonic|seed_phrase|passphrase|private_key|secret_key|root_key|device_seed|object_key|manifest_key|subject|body_text|body_html|file_name)\b'
TS_LOG_PATTERN='console\.(log|info|warn|error|debug|trace)\s*\(.*\b(mnemonic|passphrase|words|subject|bodyText|body_text|privateKey|secret)\b'

if [ ${#EXISTING[@]} -gt 0 ] && \
   rg --pcre2 -n "${EXCLUDE[@]}" --glob '*.rs' "$RUST_LOG_PATTERN" "${EXISTING[@]}" 2>/dev/null; then
  bad "a Rust tracing call references content or key material (matches above)"
else
  ok "no Rust tracing call references content or key material"
fi
if [ ${#EXISTING[@]} -gt 0 ] && \
   rg --pcre2 -n "${EXCLUDE[@]}" --glob '*.ts' --glob '*.tsx' "$TS_LOG_PATTERN" "${EXISTING[@]}" 2>/dev/null; then
  bad "a console call references content or key material (matches above)"
else
  ok "no console call references content or key material"
fi

# ---------------------------------------------------------------------------
# 3. Whole-struct logging of types that carry content.
#
# Logging a struct with %v or %+v prints every field, so a struct that gains a
# body field later starts leaking without anyone editing the log line. This is
# the failure mode that a per-field policy does not catch.
# ---------------------------------------------------------------------------
say ""
say "3. Whole-struct logging with %v or %+v"

# The keyword may appear on either side of the verb: `log("envelope %+v", e)`
# puts it before, `log("delivering %+v", envelope)` puts it after. An earlier
# version of this pattern only looked after the verb and missed the first form,
# which the self-test below caught.
STRUCT_KEYWORDS='(Envelope|envelope|Message|message|Payload|payload|Ciphertext|ciphertext)'
STRUCT_PATTERN="(Logger|logger|log)\s*(\(\))?\.\s*(Debug|Info|Warn|Error|Trace)[a-zA-Z]*f?\s*\((.*${STRUCT_KEYWORDS}.*%\+?v|.*%\+?v.*${STRUCT_KEYWORDS})"

if [ ${#EXISTING[@]} -gt 0 ] && \
   rg --pcre2 -n "${EXCLUDE[@]}" "$STRUCT_PATTERN" "${EXISTING[@]}" 2>/dev/null; then
  warn "a log call formats a message or envelope struct with %v"
  warn "  log identifiers and sizes, never whole structs; see docs/LOGGING_POLICY.md"
  FAILURES=$((FAILURES + 1))
else
  ok "no log call formats a message or envelope struct wholesale"
fi

# ---------------------------------------------------------------------------
# 4. Metrics must not carry user identity as a label.
#
# A Prometheus label with a user address in it creates one time series per
# user. That is a cardinality problem and a privacy problem at once: the
# metrics endpoint becomes a list of who uses the node.
# ---------------------------------------------------------------------------
say ""
say "4. User identity as a metric label"

LABEL_PATTERN='(NewLabel|WithLabelValues|prometheus\.Labels)\s*\(.*("address"|"user"|"sender"|"recipient"|"account")'

if [ ${#EXISTING[@]} -gt 0 ] && \
   rg --pcre2 -n "${EXCLUDE[@]}" "$LABEL_PATTERN" "${EXISTING[@]}" 2>/dev/null; then
  bad "a metric uses a user identifier as a label (matches above)"
else
  ok "no metric labels series by address, user, sender or recipient"
fi

# ---------------------------------------------------------------------------
# 5. Chain events must not carry message content.
#
# Events are written into the block and are permanent and public. An event
# attribute holding message content would publish it to every node forever,
# which is a far worse leak than a log line on one machine.
# ---------------------------------------------------------------------------
say ""
say "5. Message content in chain events"

EVENT_PATTERN='NewAttribute\s*\(.*(plaintext|plainText|cleartext|messageBody|message_body|decrypted)'

if [ ${#EXISTING[@]} -gt 0 ] && \
   rg --pcre2 -n "${EXCLUDE[@]}" "$EVENT_PATTERN" "${EXISTING[@]}" 2>/dev/null; then
  bad "a chain event attribute carries message content (matches above)"
else
  ok "no chain event attribute carries message content"
fi

# ---------------------------------------------------------------------------
# 6. Debug logging left permanently enabled.
# ---------------------------------------------------------------------------
say ""
say "6. Log level defaults"

if rg -n --glob '!**/*_test.go' 'log_level\s*=\s*"debug"' deploy/ scripts/ 2>/dev/null; then
  bad "a shipped configuration sets log_level to debug"
else
  ok "no shipped configuration defaults to debug logging"
fi

# ---------------------------------------------------------------------------

# ---------------------------------------------------------------------------
# 7. Self-test.
#
# The checks above all pass on a clean tree, which is also what they would do
# if every pattern were broken. This section writes a file containing one
# deliberate violation of each rule and asserts that the corresponding pattern
# fires. It caught a real bug during development: the whole-struct pattern
# only looked for the keyword after the format verb, so it missed
# log("envelope %+v", e).
# ---------------------------------------------------------------------------
say ""
say "7. Self-test: each pattern must fire on a known violation"

FIXTURE_DIR="$(mktemp -d)"
trap 'rm -rf "$FIXTURE_DIR"' EXIT

cat > "$FIXTURE_DIR/violations.go" <<'FIXTURE'
package fixture

// Deliberate violations, one per rule. Never compiled; only grepped.

func content(e envelope) {
	logger.Info("delivering", "plaintext", string(e.Plaintext))
}

func keys(m string) {
	logger.Debug("recovering wallet", "mnemonic", m)
}

func wholeStructAfter(e envelope) {
	logger.Errorf("delivering %+v", e.Message)
}

func wholeStructBefore(e envelope) {
	logger.Errorf("envelope %+v", e)
}

func labels(addr string) {
	gauge.WithLabelValues("address", addr).Set(1)
}

func events(e envelope) {
	sdk.NewAttribute("plaintext", string(e.Plaintext))
}
FIXTURE

selftest() {
  local label="$1" pattern="$2"
  if rg --pcre2 -q --pcre2 "$pattern" "$FIXTURE_DIR/violations.go" 2>/dev/null; then
    ok "pattern fires on a known violation: $label"
  else
    bad "pattern does NOT fire on a known violation: $label. The check above is not protecting anything."
  fi
}

cat > "$FIXTURE_DIR/violations.rs" <<'FIXTURE'
fn bad(m: &Mail) { tracing::info!(subject = %m.subject, "delivering"); }
fn worse(p: &str) { warn!("unlock failed for passphrase {p}"); }
FIXTURE
cat > "$FIXTURE_DIR/violations.ts" <<'FIXTURE'
console.log("restore", mnemonic);
FIXTURE
if rg --pcre2 -q "$RUST_LOG_PATTERN" "$FIXTURE_DIR/violations.rs" 2>/dev/null; then
  ok "pattern fires on a known violation: rust tracing"
else
  bad "pattern does NOT fire on a known violation: rust tracing"
fi
if rg --pcre2 -q "$TS_LOG_PATTERN" "$FIXTURE_DIR/violations.ts" 2>/dev/null; then
  ok "pattern fires on a known violation: console"
else
  bad "pattern does NOT fire on a known violation: console"
fi

selftest "message content"        "$CONTENT_PATTERN"
selftest "key material"           "$KEY_PATTERN"
selftest "whole-struct logging"   "$STRUCT_PATTERN"
selftest "identity metric labels" "$LABEL_PATTERN"
selftest "event content"          "$EVENT_PATTERN"

# And the inverse: a clean file must not match, or the checks would fail on
# every tree and would be turned off within a week.
cat > "$FIXTURE_DIR/clean.go" <<'FIXTURE'
package fixture

func good(e envelope) {
	logger.Info("delivering envelope", "id", e.ID, "size_bytes", len(e.Ciphertext))
	sdk.NewAttribute("envelope_id", e.ID)
	gauge.WithLabelValues("status", "delivered").Set(1)
}
FIXTURE

CLEAN_MATCHED=0
for pattern in "$CONTENT_PATTERN" "$KEY_PATTERN" "$STRUCT_PATTERN" "$LABEL_PATTERN" "$EVENT_PATTERN"; do
  if rg --pcre2 -q "$pattern" "$FIXTURE_DIR/clean.go" 2>/dev/null; then
    CLEAN_MATCHED=1
  fi
done
if [ "$CLEAN_MATCHED" -eq 0 ]; then
  ok "no pattern fires on compliant logging (identifiers and sizes only)"
else
  bad "a pattern fires on compliant logging; it would be silenced rather than fixed"
fi

# ---------------------------------------------------------------------------

say ""
printf '=========================================================================\n'
if [ "$FAILURES" -eq 0 ]; then
  printf '  %sLogging policy holds.%s\n' "$GREEN" "$RESET"
  say ""
  say "  What this proves: none of the patterns that have historically leaked"
  say "  message plaintext are present in the tree."
  say ""
  say "  What it does not prove: that plaintext cannot leak. A grep cannot see"
  say "  through a variable rename or a helper function. When the phase 2"
  say "  messaging code lands, the envelope types should carry no exported"
  say "  plaintext field at all, so that logging one is not expressible rather"
  say "  than merely discouraged."
  say ""
else
  printf '  %s%d violation(s).%s See docs/LOGGING_POLICY.md.\n' "$RED" "$FAILURES" "$RESET"
  say ""
fi

exit "$FAILURES"
