package keeper

import (
	"context"

	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"

	"github.com/cosmos/cosmos-sdk/runtime"
	sdk "github.com/cosmos/cosmos-sdk/types"
	"github.com/cosmos/cosmos-sdk/types/query"

	"github.com/hashgram/hashgram/x/welcome/types"
)

var _ types.QueryServer = Querier{}

// Querier serves x/welcome queries.
type Querier struct {
	k Keeper
}

// NewQuerier wraps a keeper as a query server.
func NewQuerier(k Keeper) Querier { return Querier{k: k} }

// Params returns the welcome configuration and attestor set.
func (q Querier) Params(ctx context.Context, _ *types.QueryParamsRequest) (*types.QueryParamsResponse, error) {
	p, err := q.k.GetParams(ctx)
	if err != nil {
		return nil, status.Error(codes.NotFound, err.Error())
	}
	return &types.QueryParamsResponse{Params: p}, nil
}

// Status returns the current sequence, tier, remaining pool and totals.
func (q Querier) Status(ctx context.Context, _ *types.QueryStatusRequest) (*types.QueryStatusResponse, error) {
	p, err := q.k.GetParams(ctx)
	if err != nil {
		return nil, status.Error(codes.NotFound, err.Error())
	}
	seq, err := q.k.NextSequence(ctx)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	totalPaid, err := q.k.TotalPaid(ctx)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	claimsPaid, err := q.k.ClaimsPaid(ctx)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}

	return &types.QueryStatusResponse{
		NextSequence:  seq,
		NextAmount:    types.TierAmount(seq),
		ClaimsPaid:    claimsPaid,
		TotalPaid:     totalPaid,
		PoolRemaining: q.k.PoolRemaining(ctx),
		PoolAccount:   q.k.PoolAddress().String(),
		Exhausted:     types.IsExhausted(seq),
		Enabled:       p.Enabled,
	}, nil
}

// Tiers returns the compile-time reward schedule.
func (q Querier) Tiers(_ context.Context, _ *types.QueryTiersRequest) (*types.QueryTiersResponse, error) {
	return &types.QueryTiersResponse{
		Tiers:   types.Tiers(),
		MaxPool: types.MaxPool(),
	}, nil
}

// Claim returns the claim record for an address.
func (q Querier) Claim(ctx context.Context, req *types.QueryClaimRequest) (*types.QueryClaimResponse, error) {
	if req == nil || req.Subject == "" {
		return nil, status.Error(codes.InvalidArgument, "subject is required")
	}
	if _, err := sdk.AccAddressFromBech32(req.Subject); err != nil {
		return nil, status.Errorf(codes.InvalidArgument, "invalid subject address: %v", err)
	}

	rec, found, err := q.k.GetClaim(ctx, req.Subject)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	return &types.QueryClaimResponse{Claimed: found, Record: rec}, nil
}

// Claims returns paginated claim records.
func (q Querier) Claims(ctx context.Context, req *types.QueryClaimsRequest) (*types.QueryClaimsResponse, error) {
	if req == nil {
		req = &types.QueryClaimsRequest{}
	}

	claims, pageRes, err := query.CollectionPaginate(
		ctx, q.k.claims, req.Pagination,
		func(_ string, v types.ClaimRecord) (types.ClaimRecord, error) { return v, nil },
	)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	return &types.QueryClaimsResponse{Claims: claims, Pagination: pageRes}, nil
}

// keep the runtime import referenced for build-tag stability across SDK
// versions where CollectionPaginate moved packages.
var _ = runtime.NewKVStoreService
