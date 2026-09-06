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

# CGO is required for the default cometbft-db backend (goleveldb is pure Go but
# rocksdb/pebble paths and libsecp256k1 are not). We build with CGO on.
export CGO_ENABLED := 1

LDFLAGS = -X github.com/cosmos/cosmos-sdk/version.Name=hashgram \
          -X github.com/cosmos/cosmos-sdk/version.AppName=hashgramd \
          -X github.com/cosmos/cosmos-sdk/version.Version=$(VERSION) \
          -X github.com/cosmos/cosmos-sdk/version.Commit=$(COMMIT) \
          -X github.com/hashgram/hashgram/app.BuildDate=$(BUILD_DATE)

BUILD_FLAGS = -mod=readonly -ldflags '$(LDFLAGS)' -trimpath

BINARIES = hashgramd hashgramctl hashgram-test-client hashgram-keygen

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
# Release
# ---------------------------------------------------------------------------

.PHONY: release
release:
	@bash scripts/release/build-release.sh

.PHONY: version
version:
	@echo "version    $(VERSION)"
	@echo "commit     $(COMMIT)"
	@echo "build date $(BUILD_DATE)"
	@echo "go         $$($(GO) version | awk '{print $$3}')"
