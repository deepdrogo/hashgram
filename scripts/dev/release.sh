#!/usr/bin/env bash
#
# Build reproducible release binaries with checksums.
#
#   scripts/dev/release.sh                # build into dist/
#   scripts/dev/release.sh --verify       # build twice and prove byte-identical
#   scripts/dev/release.sh --check <dir>  # verify checksums of an existing dist
#
# Why this matters more here than for ordinary software: the only defence a
# node operator has against running a tampered binary is being able to build
# the published source and get the published bytes. If the build is not
# reproducible, "verify the checksum" is advice nobody can act on, and every
# operator is trusting whoever produced the download.
#
# What makes it reproducible:
#
#   -trimpath          strips the build machine's directory layout from the
#                      binary, which is otherwise the largest source of
#                      divergence between two machines
#   -buildvcs=false    omits git state, which differs between a clean checkout
#                      and a working tree
#   CGO_ENABLED=0      no linkage against the build host's libc
#   fixed -ldflags     the version stamp is an input, not a timestamp
#   GOFLAGS=-mod=readonly  no silent dependency resolution mid-build
#
# What is deliberately NOT stamped: build time and git commit. Both are
# tempting and both destroy reproducibility, which is the property that
# actually protects an operator. The version and the source revision belong in
# the release notes and in the tag, not baked into bytes nobody else can
# reproduce.
#
# See docs/SECURITY.md.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

export PATH="$PATH:/usr/local/go/bin"

DIST="${DIST:-$REPO_ROOT/dist}"
BINARIES="hashgramd hashgramctl hashgram-test-client hashgram-keygen"

# The version is an explicit input so that two people building the same
# release get the same bytes. Defaults to the git tag when there is one.
VERSION="${VERSION:-$(git describe --tags --exact-match 2>/dev/null || echo dev)}"

RED=$'\033[31m'; GREEN=$'\033[32m'; YELLOW=$'\033[33m'; BOLD=$'\033[1m'; RESET=$'\033[0m'
FAILURES=0

say()   { printf '%s\n' "$*"; }
ok()    { printf '  %s[ ok ]%s %s\n' "$GREEN" "$RESET" "$*"; }
bad()   { printf '  %s[FAIL]%s %s\n' "$RED" "$RESET" "$*"; FAILURES=$((FAILURES + 1)); }
warn()  { printf '  %s[warn]%s %s\n' "$YELLOW" "$RESET" "$*"; }
head1() { printf '\n%s%s%s\n%s\n' "$BOLD" "$*" "$RESET" "=========================================================================="; }

# The version stamp goes into the Cosmos SDK's version package, which is where
# `hashgramd version` and `hashgramctl node-info` read it from. app.Version is
# a function that wraps it, not a variable, so -X against app.Version would be
# silently ignored and every release would report "dev".
#
# app.BuildDate and version.Commit are deliberately left unset. The Makefile
# stamps both for development builds, and either one makes the output
# unreproducible: two people building the same tag would get different bytes,
# and the published checksum would be unverifiable. BuildInfo reports them as
# empty on a release binary, which is honest.
LDFLAGS_RELEASE="-s -w \
-X github.com/cosmos/cosmos-sdk/version.Name=hashgram \
-X github.com/cosmos/cosmos-sdk/version.AppName=hashgramd \
-X github.com/cosmos/cosmos-sdk/version.Version=${VERSION}"

build_into() {
  local out="$1"
  mkdir -p "$out"

  for b in $BINARIES; do
    CGO_ENABLED=0 \
    GOFLAGS=-mod=readonly \
    go build \
      -trimpath \
      -buildvcs=false \
      -ldflags "$LDFLAGS_RELEASE" \
      -o "$out/$b" \
      "./cmd/$b" || return 1
  done
}

checksums_for() {
  local dir="$1"
  ( cd "$dir" && sha256sum $BINARIES | sort -k2 )
}

case "${1:-build}" in
# -------------------------------------------------------------------------
--check)
  DIR="${2:-$DIST}"
  head1 "VERIFYING CHECKSUMS IN $DIR"

  if [ ! -f "$DIR/SHA256SUMS" ]; then
    bad "no SHA256SUMS in $DIR"
    exit 1
  fi
  if ( cd "$DIR" && sha256sum --quiet --check SHA256SUMS ); then
    ok "every binary matches its recorded checksum"
  else
    bad "a binary does not match its checksum"
  fi
  exit "$FAILURES"
  ;;

# -------------------------------------------------------------------------
--verify)
  head1 "REPRODUCIBILITY CHECK"

  say ""
  say "Building twice into separate directories and comparing. A difference"
  say "means an operator cannot verify a published checksum by rebuilding,"
  say "which makes the checksum decorative."
  say ""

  A="$(mktemp -d)"
  B="$(mktemp -d)"
  trap 'rm -rf "$A" "$B"' EXIT

  say "  first build..."
  if ! build_into "$A"; then
    bad "the first build failed"
    exit 1
  fi

  # Clear the build cache between runs so the second build genuinely
  # recompiles. Without this the second build copies cached objects and would
  # match trivially, proving nothing.
  say "  clearing the build cache..."
  go clean -cache >/dev/null 2>&1

  say "  second build..."
  if ! build_into "$B"; then
    bad "the second build failed"
    exit 1
  fi

  say ""
  for b in $BINARIES; do
    if cmp -s "$A/$b" "$B/$b"; then
      ok "$b is byte-identical across builds ($(stat -c%s "$A/$b") bytes)"
    else
      bad "$b differs between two builds of the same source"
      warn "  compare with: cmp -l $A/$b $B/$b | head"
    fi
  done

  say ""
  say "  Checksums:"
  checksums_for "$A" | sed 's/^/    /'

  head1 "RESULT"
  if [ "$FAILURES" -eq 0 ]; then
    printf '  %sThe build is reproducible on this machine.%s\n' "$GREEN" "$RESET"
    say ""
    say "  What this proves: two builds from the same source on the same host"
    say "  produce identical bytes, so the build does not embed timestamps,"
    say "  paths or git state."
    say ""
    say "  What it does not prove: that a different machine produces the same"
    say "  bytes. That depends on the Go toolchain version matching exactly,"
    say "  which is why go.mod pins it and why the release notes must state it."
    say "  Cross-machine reproducibility should be confirmed by a second"
    say "  builder before a release is published."
    say ""
  else
    printf '  %s%d binary(ies) are not reproducible.%s\n\n' "$RED" "$FAILURES" "$RESET"
  fi
  exit "$FAILURES"
  ;;

# -------------------------------------------------------------------------
build|"")
  head1 "RELEASE BUILD"

  say ""
  say "  Version      $VERSION"
  say "  Go           $(go version | awk '{print $3}')"
  say "  Output       $DIST"
  say ""

  if [ "$VERSION" = "dev" ]; then
    warn "no exact git tag; building as 'dev'"
    warn "  a published release must be built from a tagged commit so that the"
    warn "  version in the binary and the version in the tag cannot disagree"
  fi

  rm -rf "$DIST"
  if ! build_into "$DIST"; then
    bad "the build failed"
    exit 1
  fi

  for b in $BINARIES; do
    ok "$b  $(stat -c%s "$DIST/$b") bytes"
  done

  checksums_for "$DIST" > "$DIST/SHA256SUMS"
  ok "SHA256SUMS written"

  # Record what the bytes depend on. A checksum with no statement of the
  # toolchain that produced it cannot be reproduced by anyone.
  {
    echo "Hashgram release build"
    echo
    echo "version:      $VERSION"
    echo "go:           $(go version | awk '{print $3}')"
    echo "go toolchain: $(awk '/^toolchain /{print $2}' go.mod 2>/dev/null || echo unset)"
    echo "go directive: $(awk '/^go /{print $2; exit}' go.mod)"
    echo "flags:        CGO_ENABLED=0 -trimpath -buildvcs=false -mod=readonly"
    echo "ldflags:      $LDFLAGS_RELEASE"
    echo
    echo "Reproduce with:"
    echo "  git checkout $VERSION"
    echo "  VERSION=$VERSION scripts/dev/release.sh"
    echo "  sha256sum -c dist/SHA256SUMS"
    echo
    echo "Build time and git commit are deliberately not stamped into the"
    echo "binaries: either would make the output unreproducible, and an"
    echo "unreproducible binary cannot be verified by the operator running it."
  } > "$DIST/BUILD_INFO"
  ok "BUILD_INFO written"

  # A release binary that cannot run is worse than no release.
  head1 "SMOKE TEST"
  for b in $BINARIES; do
    if "$DIST/$b" version >/dev/null 2>&1 || "$DIST/$b" --help >/dev/null 2>&1; then
      ok "$b runs"
    else
      bad "$b does not run"
    fi
  done

  head1 "CHECKSUMS"
  sed 's/^/  /' "$DIST/SHA256SUMS"

  head1 "RESULT"
  if [ "$FAILURES" -eq 0 ]; then
    printf '  %sRelease built into %s%s\n\n' "$GREEN" "$DIST" "$RESET"
    say "  Confirm reproducibility before publishing:"
    say "    scripts/dev/release.sh --verify"
    say ""
    say "  Operators verify a download with:"
    say "    sha256sum -c SHA256SUMS"
    say ""
  else
    printf '  %s%d problem(s).%s\n\n' "$RED" "$FAILURES" "$RESET"
  fi
  exit "$FAILURES"
  ;;

*)
  say "usage: $0 [build|--verify|--check <dir>]"
  exit 1
  ;;
esac
