#!/usr/bin/env bash
#
# Regenerate Go types from the Hashgram protobuf definitions.
#
# Requires:
#   buf                      github.com/bufbuild/buf/cmd/buf
#   protoc-gen-gocosmos      github.com/cosmos/gogoproto/protoc-gen-gocosmos
#   protoc-gen-grpc-gateway  github.com/grpc-ecosystem/grpc-gateway/protoc-gen-grpc-gateway
#
# Install them with: make proto-tools
#
# Generated files are committed to the repository. That is deliberate: a
# consensus-critical wire format should not depend on a code generator staying
# reproducible across machines and years. Regenerating must produce no diff;
# if it does, the wire format changed, which is a state-machine breaking
# change requiring a coordinated upgrade.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

for tool in buf protoc-gen-gocosmos protoc-gen-grpc-gateway; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "error: $tool not found in PATH" >&2
    echo "       run: make proto-tools" >&2
    exit 1
  fi
done

echo ">>> linting hashgram protobuf definitions"
(cd proto && buf lint 2>&1 | grep -v '^WARN' | grep -v '^$' | grep -v 'deprecated' || true)
(cd proto && buf lint 2>/dev/null >/dev/null) || {
  echo "error: buf lint failed" >&2
  exit 1
}

echo ">>> generating Go types"
cd proto
# One file at a time: gocosmos emits a single Go file per proto file, and buf's
# directory mode has mis-ordered imports when a directory mixes service and
# message definitions.
while IFS= read -r -d '' file; do
  printf '    %s\n' "$file"
  buf generate --template buf.gen.gogo.yaml "$file"
done < <(find ./hashgram -name '*.proto' -print0 | sort -z)
cd "$REPO_ROOT"

# gocosmos writes into a tree mirroring the go_package option, i.e.
# ./github.com/hashgram/hashgram/x/... . Move it into place.
for base in proto/github.com github.com; do
  if [ -d "$base/hashgram/hashgram" ]; then
    cp -r "$base/hashgram/hashgram/." .
    rm -rf "$base"
  fi
done

echo ">>> tidying module"
go mod tidy >/dev/null 2>&1

echo ">>> generated files"
find x -name '*.pb.go' -o -name '*.pb.gw.go' | sort | sed 's/^/    /'

# ---------------------------------------------------------------------------
# Off-chain wire protocol (hashgram.p2p.v1, hashgram.chat.v1): plain Go
# protobuf for the indexer and safety engine. The Rust side is generated at
# build time by prost from the same files.
# ---------------------------------------------------------------------------
if command -v protoc-gen-go >/dev/null 2>&1; then
  protoc -I proto --go_out=. --go_opt=module=github.com/hashgram/hashgram \
    proto/hashgram/p2p/v1/*.proto proto/hashgram/chat/v1/chat.proto
  echo "generated pkg/p2ppb and pkg/chatpb"
else
  echo "protoc-gen-go not installed; skipping pkg/p2ppb (go install google.golang.org/protobuf/cmd/protoc-gen-go@latest)"
fi
