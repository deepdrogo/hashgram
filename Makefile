# ---------------------------------------------------------------------------
# Hashgram Core
# ---------------------------------------------------------------------------

SHELL := /usr/bin/env bash

VERSION      ?= $(shell git describe --tags --always --dirty 2>/dev/null || echo "dev")
COMMIT       ?= $(shell git rev-parse --short HEAD 2>/dev/null || echo "unknown")
BUILD_DATE   ?= $(shell date -u +%Y-%m-%dT%H:%M:%SZ)
GO           ?= go
GOBIN        ?= $(CURDIR)/build
GOFLAGS      ?=

# CGO is off by default, which was verified rather than assumed: a
# CGO_ENABLED=0 build of hashgramd was run as a single-node devnet and produced
# blocks. The default database backend is goleveldb and the default secp256k1
# implementation is btcec, both pure Go, so nothing in the default
# configuration needs a C toolchain.
#
# Turning CGO off buys two things worth having. The binary is static, so it
# does not depend on the glibc version of whatever host built it. And the build
# is far easier to make reproducible, which is the only reason a published
# checksum is worth anything to an operator.
#
# Set CGO_ENABLED=1 if you build with the rocksdb or libsecp256k1 build tags.
export CGO_ENABLED ?= 0

LDFLAGS = -X github.com/cosmos/cosmos-sdk/version.Name=hashgram \
          -X github.com/cosmos/cosmos-sdk/version.AppName=hashgramd \
          -X github.com/cosmos/cosmos-sdk/version.Version=$(VERSION) \
          -X github.com/cosmos/cosmos-sdk/version.Commit=$(COMMIT) \
          -X github.com/hashgram/hashgram/app.BuildDate=$(BUILD_DATE)

BUILD_FLAGS = -mod=readonly -ldflags '$(LDFLAGS)' -trimpath

BINARIES = hashgramd hashgramctl hashgram-test-client hashgram-keygen hashgram-indexer hashgram-safety

.PHONY: all
all: lint test build

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------

.PHONY: build
build: $(addprefix build-,$(BINARIES))

.PHONY: $(addprefix build-,$(BINARIES))
$(addprefix build-,$(BINARIES)): build-%:
	@mkdir -p $(GOBIN)
	@echo ">>> building $*"
	@$(GO) build $(BUILD_FLAGS) -o $(GOBIN)/$* ./cmd/$*

# Developer tools live under tools/ rather than cmd/ because they are not
# part of a node deployment: nothing in the release archive needs them.
TOOLS = tokenomics-simulator

.PHONY: tools
tools: $(addprefix build-tool-,$(TOOLS))

.PHONY: $(addprefix build-tool-,$(TOOLS))
$(addprefix build-tool-,$(TOOLS)): build-tool-%:
	@mkdir -p $(GOBIN)
	@echo ">>> building tool $*"
	@$(GO) build $(BUILD_FLAGS) -o $(GOBIN)/$* ./tools/$*

.PHONY: install
install:
	@echo ">>> installing $(BINARIES) into /usr/local/bin"
	@for b in $(BINARIES); do \
		$(GO) build $(BUILD_FLAGS) -o /usr/local/bin/$$b ./cmd/$$b || exit 1; \
		echo "    /usr/local/bin/$$b"; \
	done

.PHONY: clean
clean:
	rm -rf $(GOBIN) coverage.out coverage.html

# ---------------------------------------------------------------------------
# Test
# ---------------------------------------------------------------------------

.PHONY: test
test:
	$(GO) test -race -timeout 20m ./...

# Policy and configuration checks that are not Go tests.
#
# check-logging asserts that message plaintext and key material are absent
# from every log call, metric label and chain event, and self-tests its own
# patterns against deliberate violations.
#
# check-dashboards asserts that every PromQL expression in the shipped
# dashboards parses. Pass --live to also query a running Prometheus, which is
# the only way to tell a broken query from a healthy zero.
.PHONY: check-policy
check-policy:
	@scripts/dev/check-logging.sh
	@scripts/dev/check-dashboards.sh
	@scripts/dev/check-docs.sh

.PHONY: check-policy-live
check-policy-live:
	@scripts/dev/check-logging.sh
	@scripts/dev/check-dashboards.sh --live
	@scripts/dev/check-docs.sh

# The whole CI pipeline, exactly as it runs in GitHub Actions.
# ---------------------------------------------------------------------------
# Rust workspace (Phase 2)
# ---------------------------------------------------------------------------

.PHONY: rust
rust:
	@cd node && cargo build --all

.PHONY: rust-test
rust-test:
	@cd node && cargo test --all

.PHONY: rust-lint
rust-lint:
	@cd node && cargo fmt --all -- --check
	@cd node && cargo clippy --all-targets --all-features -- -D warnings

# Regenerates the cross-language signing vectors from the Go implementation.
# The Rust crate must reproduce these byte for byte; a signature is only
# verifiable if both sides build the same preimage.
.PHONY: signing-vectors
signing-vectors:
	@$(GO) run ./tools/signing-vectors > node/testdata/signing-vectors.json
	@echo ">>> node/testdata/signing-vectors.json regenerated"

.PHONY: ci
ci:
	@scripts/dev/ci.sh

.PHONY: ci-fast
ci-fast:
	@scripts/dev/ci.sh fast

# ---------------------------------------------------------------------------
# Release
# ---------------------------------------------------------------------------

# Reproducible release binaries with checksums. Unlike `make build`, this does
# not stamp the build date or commit: either would make the output
# unreproducible, and an operator who cannot reproduce a checksum cannot
# verify a download.
.PHONY: release
release:
	@scripts/dev/release.sh

.PHONY: release-verify
release-verify:
	@scripts/dev/release.sh --verify

.PHONY: test-short
test-short:
	$(GO) test -short -timeout 10m ./...

.PHONY: test-cover
test-cover:
	$(GO) test -coverprofile=coverage.out -covermode=atomic ./...
	$(GO) tool cover -func=coverage.out | tail -1

.PHONY: test-fuzz
test-fuzz:
	@echo ">>> running fuzz targets for $${FUZZTIME:-30s} each"
	@bash scripts/dev/fuzz.sh

# Integration tests are tagged so that `make test` stays fast.
.PHONY: test-integration
test-integration:
	$(GO) test -tags=integration -timeout 30m ./tests/integration/...

# ---------------------------------------------------------------------------
# Lint / vet / security
# ---------------------------------------------------------------------------

.PHONY: lint
lint: vet
	@command -v staticcheck >/dev/null 2>&1 && staticcheck ./... || \
		echo "staticcheck not installed; run 'make tools'"

.PHONY: vet
vet:
	$(GO) vet ./...

.PHONY: fmt
fmt:
	$(GO) fmt ./...
	@command -v gofumpt >/dev/null 2>&1 && gofumpt -l -w . || true

.PHONY: sec
sec:
	@command -v gosec >/dev/null 2>&1 && gosec -quiet ./... || \
		echo "gosec not installed; run 'make tools'"
	@command -v gitleaks >/dev/null 2>&1 && gitleaks detect --no-banner --redact || \
		echo "gitleaks not installed; run 'make tools'"

.PHONY: tools
tools:
	$(GO) install honnef.co/go/tools/cmd/staticcheck@latest
	$(GO) install github.com/securego/gosec/v2/cmd/gosec@latest
	$(GO) install mvdan.cc/gofumpt@latest

# ---------------------------------------------------------------------------
# Protobuf
# ---------------------------------------------------------------------------

.PHONY: proto-gen
proto-gen:
	@bash scripts/dev/protocgen.sh

.PHONY: proto-lint
proto-lint:
	@cd proto && buf lint

# ---------------------------------------------------------------------------
# Local networks
# ---------------------------------------------------------------------------

.PHONY: devnet
devnet: build
	@bash scripts/testnet/devnet.sh

.PHONY: devnet-stop
devnet-stop:
	@bash scripts/testnet/devnet.sh stop

.PHONY: four-validator
four-validator: build
	@bash scripts/testnet/four-validator.sh

.PHONY: four-validator-stop
four-validator-stop:
	@bash scripts/testnet/four-validator.sh stop

# ---------------------------------------------------------------------------
# Version
# ---------------------------------------------------------------------------

.PHONY: version
version:
	@echo "version    $(VERSION)"
	@echo "commit     $(COMMIT)"
	@echo "build date $(BUILD_DATE)"
	@echo "go         $$($(GO) version | awk '{print $$3}')"
