package keeper

import (
	"context"

	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"

	sdk "github.com/cosmos/cosmos-sdk/types"
	"github.com/cosmos/cosmos-sdk/types/query"

	"github.com/hashgram/hashgram/x/username/types"
)

var _ types.QueryServer = Querier{}

// Querier serves x/username queries.
type Querier struct {
	k Keeper
}

// NewQuerier wraps a keeper as a query server.
func NewQuerier(k Keeper) Querier { return Querier{k: k} }

func (q Querier) Params(ctx context.Context, _ *types.QueryParamsRequest) (*types.QueryParamsResponse, error) {
	p, err := q.k.GetParams(ctx)
	if err != nil {
		return nil, status.Error(codes.NotFound, err.Error())
	}
	return &types.QueryParamsResponse{Params: p}, nil
}

func (q Querier) Lookup(ctx context.Context, req *types.QueryLookupRequest) (*types.QueryLookupResponse, error) {
	if req == nil || req.Name == "" {
		return nil, status.Error(codes.InvalidArgument, "name is required")
	}

	reg, found, err := q.k.Lookup(ctx, req.Name)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	return &types.QueryLookupResponse{
		Found:        found,
		Registration: reg,
		Normalized:   types.Normalize(req.Name),
	}, nil
}

func (q Querier) ReverseLookup(ctx context.Context, req *types.QueryReverseLookupRequest) (*types.QueryReverseLookupResponse, error) {
	if req == nil || req.Owner == "" {
		return nil, status.Error(codes.InvalidArgument, "owner is required")
	}
	if _, err := sdk.AccAddressFromBech32(req.Owner); err != nil {
		return nil, status.Errorf(codes.InvalidArgument, "invalid owner address: %v", err)
	}

	regs, err := q.k.OwnedNames(ctx, req.Owner)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	return &types.QueryReverseLookupResponse{Registrations: regs}, nil
}

func (q Querier) Availability(ctx context.Context, req *types.QueryAvailabilityRequest) (*types.QueryAvailabilityResponse, error) {
	if req == nil || req.Name == "" {
		return nil, status.Error(codes.InvalidArgument, "name is required")
	}

	a, err := q.k.CheckAvailability(ctx, req.Name)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	return &types.QueryAvailabilityResponse{
		Available:       a.Available,
		Normalized:      a.Normalized,
		Skeleton:        a.Skeleton,
		Reason:          a.Reason,
		ConflictingName: a.ConflictingName,
	}, nil
}

func (q Querier) Registrations(ctx context.Context, req *types.QueryRegistrationsRequest) (*types.QueryRegistrationsResponse, error) {
	if req == nil {
		req = &types.QueryRegistrationsRequest{}
	}
	regs, pageRes, err := query.CollectionPaginate(
		ctx, q.k.Registrations(), req.Pagination,
		func(_ string, v types.Registration) (types.Registration, error) { return v, nil },
	)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	return &types.QueryRegistrationsResponse{Registrations: regs, Pagination: pageRes}, nil
}
