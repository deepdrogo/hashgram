package keeper

import (
	"context"

	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"

	"github.com/hashgram/hashgram/x/network/types"
)

var _ types.QueryServer = Querier{}

// Querier serves x/network queries.
type Querier struct {
	k Keeper
}

// NewQuerier wraps a keeper as a query server.
func NewQuerier(k Keeper) Querier { return Querier{k: k} }

// Info returns the on-chain network identity.
func (q Querier) Info(ctx context.Context, _ *types.QueryInfoRequest) (*types.QueryInfoResponse, error) {
	info, err := q.k.NetworkInfo(ctx)
	if err != nil {
		return nil, status.Error(codes.NotFound, err.Error())
	}
	return &types.QueryInfoResponse{Info: info}, nil
}

// ForkIsolation returns the peer-rejection policy in force.
func (q Querier) ForkIsolation(ctx context.Context, _ *types.QueryForkIsolationRequest) (*types.QueryForkIsolationResponse, error) {
	p, err := q.k.ForkIsolation(ctx)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	return &types.QueryForkIsolationResponse{Policy: p}, nil
}

// SigningDomain returns the exact signing domain for a purpose.
//
// An unknown purpose is an InvalidArgument, not a best-effort string: a client
// that misspells a purpose must fail here rather than produce signatures that
// nothing else will accept.
func (q Querier) SigningDomain(ctx context.Context, req *types.QuerySigningDomainRequest) (*types.QuerySigningDomainResponse, error) {
	if req == nil {
		return nil, status.Error(codes.InvalidArgument, "empty request")
	}

	purpose, err := types.ParseSigningPurpose(req.Purpose)
	if err != nil {
		return nil, status.Error(codes.InvalidArgument, err.Error())
	}

	info, err := q.k.NetworkInfo(ctx)
	if err != nil {
		return nil, status.Error(codes.NotFound, err.Error())
	}

	// Deliberately built from on-chain info with an empty genesis hash: the
	// signing domain does not include the genesis hash, only the protocol
	// version and network id. Including the genesis hash would make every
	// signature unverifiable by a client that had not yet fetched genesis.
	id := info.Identity("")

	return &types.QuerySigningDomainResponse{
		Domain:         id.SigningDomain(purpose),
		NetworkMagic:   info.NetworkMagic,
		PreimageLayout: types.PreimageLayout,
	}, nil
}
