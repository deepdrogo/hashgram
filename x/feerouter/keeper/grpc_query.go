package keeper

import (
	"context"

	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"

	"github.com/hashgram/hashgram/x/feerouter/types"
)

var _ types.QueryServer = Querier{}

// Querier serves x/feerouter queries.
type Querier struct {
	k Keeper
}

// NewQuerier wraps a keeper as a query server.
func NewQuerier(k Keeper) Querier { return Querier{k: k} }

// Params returns the routing configuration and the revenue pool state.
func (q Querier) Params(ctx context.Context, _ *types.QueryParamsRequest) (*types.QueryParamsResponse, error) {
	p, err := q.k.GetParams(ctx)
	if err != nil {
		return nil, status.Error(codes.NotFound, err.Error())
	}
	return &types.QueryParamsResponse{
		Params:      p,
		RevenuePool: q.k.RevenuePool().String(),
		Pending:     q.k.PendingPool(ctx),
	}, nil
}

// Totals returns the cumulative qualifying revenue and its split.
func (q Querier) Totals(ctx context.Context, _ *types.QueryTotalsRequest) (*types.QueryTotalsResponse, error) {
	t, err := q.k.GetTotals(ctx)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	return &types.QueryTotalsResponse{Totals: t}, nil
}

// ServiceRevenue returns cumulative qualifying revenue per service kind.
func (q Querier) ServiceRevenue(ctx context.Context, _ *types.QueryServiceRevenueRequest) (*types.QueryServiceRevenueResponse, error) {
	sr, err := q.k.AllServiceRevenue(ctx)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	return &types.QueryServiceRevenueResponse{ServiceRevenue: sr}, nil
}
