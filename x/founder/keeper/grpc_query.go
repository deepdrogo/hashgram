package keeper

import (
	"context"

	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"

	"github.com/hashgram/hashgram/x/founder/types"
)

var _ types.QueryServer = Querier{}

// Querier serves x/founder queries.
type Querier struct {
	k Keeper
}

// NewQuerier wraps a keeper as a query server.
func NewQuerier(k Keeper) Querier { return Querier{k: k} }

// Params returns the beneficiary and revenue share configuration.
func (q Querier) Params(ctx context.Context, _ *types.QueryParamsRequest) (*types.QueryParamsResponse, error) {
	p, err := q.k.GetParams(ctx)
	if err != nil {
		return nil, status.Error(codes.NotFound, err.Error())
	}
	return &types.QueryParamsResponse{
		Params:            p,
		MaxFeeBasisPoints: q.k.MaxFeeBasisPoints(),
	}, nil
}

// Revenue returns accrued, paid and pending Founder revenue.
//
// This is the query behind "where are my 1% earnings?". `pending` is read from
// the module account's actual bank balance rather than from a counter, so it
// cannot drift away from reality.
func (q Querier) Revenue(ctx context.Context, _ *types.QueryRevenueRequest) (*types.QueryRevenueResponse, error) {
	ledger, err := q.k.GetLedger(ctx)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	return &types.QueryRevenueResponse{
		TotalAccrued:          ledger.TotalAccrued,
		TotalPaid:             ledger.TotalPaid,
		Pending:               q.k.Pending(ctx),
		ModuleAccount:         q.k.ModuleAddress().String(),
		LastPayoutHeight:      ledger.LastPayoutHeight,
		LastPayoutBeneficiary: ledger.LastPayoutBeneficiary,
	}, nil
}

// BeneficiaryHistory returns every recorded beneficiary change.
func (q Querier) BeneficiaryHistory(ctx context.Context, _ *types.QueryBeneficiaryHistoryRequest) (*types.QueryBeneficiaryHistoryResponse, error) {
	changes, err := q.k.BeneficiaryHistory(ctx)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	return &types.QueryBeneficiaryHistoryResponse{Changes: changes}, nil
}
