# Vendored protobuf dependencies

These files are not authored by Hashgram. They are vendored so that
`make proto-gen` works with no network access and so that the wire format is
pinned by file content rather than by a registry tag.

| Path | Upstream | Version |
| --- | --- | --- |
| `gogoproto/gogo.proto` | `github.com/cosmos/gogoproto` | v1.7.2 |
| `google/api/annotations.proto` | `github.com/grpc-ecosystem/grpc-gateway` (`third_party/googleapis`) | v1.16.0 |
| `google/api/http.proto` | `github.com/grpc-ecosystem/grpc-gateway` (`third_party/googleapis`) | v1.16.0 |
| `cosmos_proto/cosmos.proto` | `github.com/cosmos/cosmos-proto` | v1.0.0-beta.5 |
| `amino/amino.proto` | `github.com/cosmos/cosmos-sdk` | v0.53.8 |
| `cosmos/msg/v1/msg.proto` | `github.com/cosmos/cosmos-sdk` | v0.53.8 |
| `cosmos/base/v1beta1/coin.proto` | `github.com/cosmos/cosmos-sdk` | v0.53.8 |
| `cosmos/base/query/v1beta1/pagination.proto` | `github.com/cosmos/cosmos-sdk` | v0.53.8 |
| `cosmos/query/v1/query.proto` | `github.com/cosmos/cosmos-sdk` | v0.53.8 |

To refresh after a Cosmos SDK bump, run `scripts/dev/vendor-protos.sh`, which
copies from the Go module cache at the versions in `go.mod` and then re-runs
code generation. Regenerating must produce no diff in `x/**/*.pb.go`; if it
does, the wire format changed and that is a state-machine breaking change.

Licences: gogoproto and cosmos-proto are BSD-3-Clause, googleapis and the
Cosmos SDK files are Apache-2.0.
