#!/usr/bin/env bash
#
# Install and configure Prometheus, Grafana and the exporters for a Hashgram
# node.
#
#   scripts/install/monitoring.sh                 # install and start
#   scripts/install/monitoring.sh --no-grafana    # Prometheus and exporters only
#   scripts/install/monitoring.sh --check         # validate config, change nothing
#
# Everything binds to localhost. Prometheus holds a detailed picture of the
# node's internals and Grafana holds a credential for it; publishing either
# hands an attacker a reconnaissance feed and an extra login page. Reach them
# over SSH instead:
#
#   ssh -N -L 9090:127.0.0.1:9090 -L 3000:127.0.0.1:3000 operator@node
#
# This script does not open any firewall port, and it will tell you if it
# finds one of these services already listening on a public address.
#
# See docs/OPERATIONS.md and docs/SECURITY.md.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
MON_SRC="$REPO_ROOT/deploy/monitoring"

PROM_CONFIG_DIR=/etc/prometheus
PROM_RULES_DIR=/etc/prometheus/rules
GRAFANA_PROV_DIR=/etc/grafana/provisioning
GRAFANA_DASH_DIR=/var/lib/grafana/dashboards/hashgram

WITH_GRAFANA=1
CHECK_ONLY=0

RED=$'\033[31m'; GREEN=$'\033[32m'; YELLOW=$'\033[33m'; BOLD=$'\033[1m'; RESET=$'\033[0m'
say()  { printf '%s\n' "$*"; }
ok()   { printf '  %s[ ok ]%s %s\n' "$GREEN" "$RESET" "$*"; }
bad()  { printf '  %s[FAIL]%s %s\n' "$RED" "$RESET" "$*"; FAILURES=$((FAILURES + 1)); }
warn() { printf '  %s[warn]%s %s\n' "$YELLOW" "$RESET" "$*"; }
head1() { printf '\n%s%s%s\n%s\n' "$BOLD" "$*" "$RESET" "=========================================================================="; }

FAILURES=0

while [ $# -gt 0 ]; do
  case "$1" in
    --no-grafana) WITH_GRAFANA=0 ;;
    --check)      CHECK_ONLY=1 ;;
    -h|--help)    sed -n '2,30p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *)            say "unknown argument: $1"; exit 1 ;;
  esac
  shift
done

if [ "$CHECK_ONLY" -eq 0 ] && [ "$(id -u)" -ne 0 ]; then
  say "This script installs system configuration and must run as root."
  exit 1
fi

# ---------------------------------------------------------------------------

head1 "VALIDATING CONFIGURATION"

# Validate before touching anything on the system. Installing a config that
# Prometheus then refuses to load leaves monitoring down, which is the worst
# possible time to be blind.
if ! command -v promtool >/dev/null 2>&1; then
  warn "promtool is not installed; skipping validation. Install the prometheus package first."
else
  if promtool check rules "$MON_SRC/rules/hashgram-alerts.yml" >/tmp/promtool-rules.log 2>&1; then
    ok "alert rules valid: $(grep -oE '[0-9]+ rules found' /tmp/promtool-rules.log | head -1)"
  else
    bad "alert rules are invalid:"
    sed 's/^/        /' /tmp/promtool-rules.log
  fi
fi

# The dashboards must at least be parseable JSON. A dashboard with a trailing
# comma fails silently in Grafana's provisioner and shows up as a missing
# dashboard with a log line nobody reads.
for f in "$MON_SRC"/grafana/dashboards/*.json; do
  if python3 -c "import json,sys; json.load(open(sys.argv[1]))" "$f" 2>/dev/null; then
    ok "dashboard parses: $(basename "$f")"
  else
    bad "dashboard is not valid JSON: $(basename "$f")"
  fi
done

# Every dashboard must reference the datasource uid the provisioning file
# creates, or its panels will render empty.
for f in "$MON_SRC"/grafana/dashboards/*.json; do
  if grep -q 'hashgram-prometheus' "$f"; then
    ok "dashboard references the provisioned datasource: $(basename "$f")"
  else
    bad "$(basename "$f") does not reference the hashgram-prometheus datasource uid"
  fi
done

if [ "$CHECK_ONLY" -eq 1 ]; then
  head1 "RESULT"
  if [ "$FAILURES" -eq 0 ]; then
    say "  All checks passed. Nothing was changed."
  else
    say "  ${FAILURES} check(s) failed. Nothing was changed."
  fi
  exit "$FAILURES"
fi

if [ "$FAILURES" -ne 0 ]; then
  head1 "ABORTED"
  say "  Refusing to install a configuration that does not validate."
  exit "$FAILURES"
fi

# ---------------------------------------------------------------------------

head1 "INSTALLING PACKAGES"

PACKAGES="prometheus prometheus-node-exporter prometheus-postgres-exporter"
if [ "$WITH_GRAFANA" -eq 1 ]; then
  PACKAGES="$PACKAGES grafana"
fi

MISSING=""
for p in $PACKAGES; do
  if ! dpkg -s "$p" >/dev/null 2>&1; then
    MISSING="$MISSING $p"
  fi
done

if [ -n "$MISSING" ]; then
  say "  installing:$MISSING"
  DEBIAN_FRONTEND=noninteractive apt-get update -qq
  # shellcheck disable=SC2086
  DEBIAN_FRONTEND=noninteractive apt-get install -y -qq $MISSING >/dev/null
fi

for p in $PACKAGES; do
  if dpkg -s "$p" >/dev/null 2>&1; then
    ok "$p $(dpkg-query -W -f='${Version}' "$p" 2>/dev/null)"
  else
    bad "$p failed to install"
  fi
done

# ---------------------------------------------------------------------------

head1 "PROMETHEUS"

install -d -m 0755 "$PROM_RULES_DIR"

# Keep whatever was there before. The Debian package ships a working default
# and an operator may have added targets of their own.
if [ -f "$PROM_CONFIG_DIR/prometheus.yml" ] && \
   ! grep -q 'hashgramd' "$PROM_CONFIG_DIR/prometheus.yml"; then
  cp -a "$PROM_CONFIG_DIR/prometheus.yml" \
        "$PROM_CONFIG_DIR/prometheus.yml.pre-hashgram.$(date +%Y%m%d%H%M%S)"
  ok "existing prometheus.yml backed up"
fi

install -m 0644 "$MON_SRC/prometheus.yml" "$PROM_CONFIG_DIR/prometheus.yml"
install -m 0644 "$MON_SRC/rules/hashgram-alerts.yml" "$PROM_RULES_DIR/hashgram-alerts.yml"

# Label every series with this host, so that metrics remain attributable if
# several nodes are ever federated into one Prometheus.
HOSTNAME_SHORT="$(hostname -s 2>/dev/null || hostname)"
sed -i "s/hashgram_node: \"localhost\"/hashgram_node: \"${HOSTNAME_SHORT}\"/" \
  "$PROM_CONFIG_DIR/prometheus.yml"
ok "external label hashgram_node=${HOSTNAME_SHORT}"

if promtool check config "$PROM_CONFIG_DIR/prometheus.yml" >/tmp/promtool-config.log 2>&1; then
  ok "installed prometheus.yml validates"
else
  bad "installed prometheus.yml does not validate:"
  sed 's/^/        /' /tmp/promtool-config.log
fi

systemctl enable prometheus >/dev/null 2>&1 || true
systemctl restart prometheus
sleep 3
if systemctl is-active --quiet prometheus; then
  ok "prometheus is running"
else
  bad "prometheus failed to start: journalctl -u prometheus -n 50"
fi

systemctl enable prometheus-node-exporter >/dev/null 2>&1 || true
systemctl restart prometheus-node-exporter
if systemctl is-active --quiet prometheus-node-exporter; then
  ok "node exporter is running"
else
  bad "node exporter failed to start"
fi

# The Postgres exporter needs a connection string it does not ship with. It is
# left disabled rather than started with a guessed one, because a failing
# exporter that looks configured is worse than one that is plainly absent.
if [ ! -f /etc/default/prometheus-postgres-exporter ] || \
   ! grep -q 'DATA_SOURCE_NAME' /etc/default/prometheus-postgres-exporter 2>/dev/null; then
  warn "postgres exporter needs DATA_SOURCE_NAME in /etc/default/prometheus-postgres-exporter"
  warn "  it is left stopped until the phase 2 indexer database exists"
fi

# ---------------------------------------------------------------------------

if [ "$WITH_GRAFANA" -eq 1 ]; then
  head1 "GRAFANA"

  install -d -m 0755 "$GRAFANA_PROV_DIR/datasources" "$GRAFANA_PROV_DIR/dashboards"
  install -m 0644 "$MON_SRC/grafana/provisioning/datasources/hashgram.yaml" \
    "$GRAFANA_PROV_DIR/datasources/hashgram.yaml"
  install -m 0644 "$MON_SRC/grafana/provisioning/dashboards/hashgram.yaml" \
    "$GRAFANA_PROV_DIR/dashboards/hashgram.yaml"
  ok "provisioning installed"

  install -d -m 0755 "$GRAFANA_DASH_DIR"
  for f in "$MON_SRC"/grafana/dashboards/*.json; do
    install -m 0644 "$f" "$GRAFANA_DASH_DIR/$(basename "$f")"
    ok "dashboard installed: $(basename "$f")"
  done
  chown -R grafana:grafana /var/lib/grafana/dashboards 2>/dev/null || true

  # Bind Grafana to localhost. The package default is 0.0.0.0, which would put
  # a login page on the public internet as a side effect of installing a
  # dashboard.
  if [ -f /etc/grafana/grafana.ini ]; then
    if grep -qE '^\s*http_addr\s*=' /etc/grafana/grafana.ini; then
      sed -i 's/^\s*http_addr\s*=.*/http_addr = 127.0.0.1/' /etc/grafana/grafana.ini
    else
      sed -i '/^\[server\]/a http_addr = 127.0.0.1' /etc/grafana/grafana.ini
    fi
    ok "grafana bound to 127.0.0.1"
  fi

  systemctl enable grafana-server >/dev/null 2>&1 || true
  systemctl restart grafana-server
  sleep 8
  if systemctl is-active --quiet grafana-server; then
    ok "grafana is running"
  else
    bad "grafana failed to start: journalctl -u grafana-server -n 50"
  fi
fi

# ---------------------------------------------------------------------------

head1 "BINDING TO LOCALHOST"

# The Debian packages bind to every interface. On a host with a public IP that
# publishes Prometheus, node-exporter and the Postgres exporter to the
# internet as a side effect of installing them.
#
# The firewall should also be blocking these ports, and on this project's
# reference host it is. That is not a reason to skip this: a firewall rule
# added carelessly later, or a host moved to a network without one, should not
# turn a monitoring endpoint into a public reconnaissance feed. Bind and
# filter, not either one alone.
#
# Each package reads ARGS from /etc/default/<name>, so the address is set
# there rather than by editing a unit file, which an apt upgrade would
# overwrite.
bind_localhost() {
  local svc="$1" port="$2" defaults="/etc/default/$1"

  if ! dpkg -s "$svc" >/dev/null 2>&1; then
    return 0
  fi

  local flag="--web.listen-address=127.0.0.1:${port}"

  if [ -f "$defaults" ] && grep -q -- "$flag" "$defaults"; then
    ok "$svc already bound to 127.0.0.1:${port}"
    return 0
  fi

  touch "$defaults"

  if grep -qE '^\s*ARGS=' "$defaults"; then
    # Drop any existing listen-address, then append ours, so re-running the
    # script does not accumulate flags.
    sed -i -E "s|--web\.listen-address=[^ \"']*||g" "$defaults"
    sed -i -E "s|^(\s*ARGS=\")(.*)(\")|\1\2 ${flag}\3|" "$defaults"
  else
    printf 'ARGS="%s"\n' "$flag" >> "$defaults"
  fi

  # Collapse the double spaces the substitution above can leave behind.
  sed -i -E 's/  +/ /g; s/ARGS=" /ARGS="/' "$defaults"

  if grep -q -- "$flag" "$defaults"; then
    ok "$svc bound to 127.0.0.1:${port}"
  else
    bad "could not set the listen address for $svc in $defaults"
  fi
}

bind_localhost prometheus 9090
bind_localhost prometheus-node-exporter 9100
bind_localhost prometheus-postgres-exporter 9187

systemctl restart prometheus prometheus-node-exporter 2>/dev/null || true

# The Postgres exporter has no database to talk to until the phase 2 indexer
# exists. Leaving it running would produce a permanently failing service and a
# permanently red alert, which teaches operators to ignore red alerts.
#
# The Debian default file ships DATA_SOURCE_NAME='' uncommented, so a check for
# the variable's presence matches an empty setting and concludes the exporter
# is configured. It then runs, fails to connect, reports pg_up 0 forever, and
# the alert fires permanently. The pattern below requires a non-empty value.
pg_dsn_configured() {
  [ -f /etc/default/prometheus-postgres-exporter ] || return 1
  grep -qE "^\s*DATA_SOURCE_NAME\s*=\s*['\"]?[^'\"[:space:]]+" \
    /etc/default/prometheus-postgres-exporter 2>/dev/null
}

if dpkg -s prometheus-postgres-exporter >/dev/null 2>&1; then
  if pg_dsn_configured; then
    systemctl enable prometheus-postgres-exporter >/dev/null 2>&1 || true
    systemctl restart prometheus-postgres-exporter 2>/dev/null || true
    ok "postgres exporter restarted with a configured data source"
  else
    systemctl stop prometheus-postgres-exporter 2>/dev/null || true
    systemctl disable prometheus-postgres-exporter >/dev/null 2>&1 || true
    ok "postgres exporter stopped: DATA_SOURCE_NAME is empty"
    say "      It would otherwise report pg_up 0 forever and keep the Postgres"
    say "      alert permanently red, which is how operators learn to ignore"
    say "      alerts. Enable it when the phase 2 indexer database exists:"
    say "        DATA_SOURCE_NAME='user=prometheus host=/run/postgresql dbname=hashgram_index'"
  fi
fi

sleep 3

# ---------------------------------------------------------------------------

head1 "EXPOSURE CHECK"

# The point of this section is to catch the mistake this script is most likely
# to enable: a monitoring stack reachable from the internet.
for entry in "9090 prometheus" "3000 grafana" "9100 node-exporter" "9187 postgres-exporter"; do
  set -- $entry
  port="$1"; name="$2"
  listen="$(ss -ltnH "sport = :$port" 2>/dev/null | awk '{print $4}' | head -1)"
  if [ -z "$listen" ]; then
    say "  $name ($port): not listening"
  elif printf '%s' "$listen" | grep -qE '^(127\.0\.0\.1|\[::1\]):'; then
    ok "$name ($port) is bound to localhost only"
  else
    bad "$name is listening on ${listen}, which may be reachable from the internet"
    warn "  set --web.listen-address=127.0.0.1:${port} in /etc/default/${name}"
  fi
done

if command -v ufw >/dev/null 2>&1 && ufw status 2>/dev/null | grep -q '^Status: active'; then
  if ufw status 2>/dev/null | grep -qE '(9090|3000|9100|9187)'; then
    bad "a firewall rule exposes a monitoring port; remove it"
  else
    ok "no firewall rule exposes a monitoring port"
  fi
fi

# ---------------------------------------------------------------------------

head1 "CHAIN NODE PREREQUISITE"

# The chain metrics only exist if the node was told to publish them. Checking
# here saves an operator from a dashboard full of No data and no explanation.
NODE_CONFIG=/var/lib/hashgram/chain/config/config.toml
if [ -f "$NODE_CONFIG" ]; then
  if grep -qE '^\s*prometheus\s*=\s*true' "$NODE_CONFIG"; then
    ok "the node has prometheus = true"
  else
    warn "the node has prometheus = false in $NODE_CONFIG"
    warn "  set it to true and restart, or every chain panel will read No data:"
    warn "    sed -i 's/^prometheus = false/prometheus = true/' $NODE_CONFIG"
    warn "    hashgramctl restart"
  fi
else
  warn "no node config at $NODE_CONFIG; skipping (expected before hashgramctl init)"
fi

if curl -s --max-time 3 http://127.0.0.1:26660/metrics >/dev/null 2>&1; then
  COUNT="$(curl -s --max-time 3 http://127.0.0.1:26660/metrics | grep -c '^hashgram_' || true)"
  ok "the node is publishing metrics, including ${COUNT} hashgram_* series"
else
  warn "127.0.0.1:26660 is not answering; the node may not be running yet"
fi

# ---------------------------------------------------------------------------

head1 "RESULT"

if [ "$FAILURES" -eq 0 ]; then
  printf '  %sMonitoring installed.%s\n' "$GREEN" "$RESET"
else
  printf '  %s%d problem(s) found.%s\n' "$RED" "$FAILURES" "$RESET"
fi

cat <<'EOF'

  Reach the interfaces over SSH, not over the internet:

    ssh -N -L 9090:127.0.0.1:9090 -L 3000:127.0.0.1:3000 operator@this-host

  then open http://localhost:9090 for Prometheus and http://localhost:3000
  for Grafana. Change the Grafana admin password on first login.

  Dashboards, under the Hashgram folder:

    Chain and Consensus        liveness, rounds, peers, validator signing
    Economics and Invariants   supply ceiling, Founder 1%, reserve, welcome
    Host and Database          CPU, memory, disk projection, PostgreSQL

  The alert to care about above all others is HashgramSupplyExceedsCeiling.
  It should be impossible: there is no mint module. If it ever fires, treat
  the binary as suspect before treating the metric as suspect.

  Logging policy: docs/LOGGING_POLICY.md. Message plaintext is never logged
  at any level, and scripts/dev/check-logging.sh enforces that in CI.

EOF

exit "$FAILURES"
