package keeper

import (
	"context"

	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"

	sdk "github.com/cosmos/cosmos-sdk/types"

	"github.com/hashgram/hashgram/x/treasury/types"
)

// --- Msg server -------------------------------------------------------------

var _ types.MsgServer = msgServer{}

type msgServer struct{ k Keeper }

// NewMsgServerImpl returns the x/treasury message server.
func NewMsgServerImpl(k Keeper) types.MsgServer { return msgServer{k: k} }

func (s msgServer) Spend(ctx context.Context, msg *types.MsgSpend) (*types.MsgSpendResponse, error) {
	recipient, err := sdk.AccAddressFromBech32(msg.Recipient)
	if err != nil {
		return nil, err
	}
	if err := s.k.Spend(ctx, msg.Authority, msg.Reserve, recipient, msg.Amount, msg.Memo); err != nil {
		return nil, err
	}
	return &types.MsgSpendResponse{}, nil
}

// --- Query server -----------------------------------------------------------

var _ types.QueryServer = Querier{}

// Querier serves x/treasury queries.
type Querier struct{ k Keeper }

// NewQuerier wraps a keeper as a query server.
func NewQuerier(k Keeper) Querier { return Querier{k: k} }

func (q Querier) Reserves(ctx context.Context, _ *types.QueryReservesRequest) (*types.QueryReservesResponse, error) {
	var out []types.ReserveInfo
	err := q.k.IterateReserves(ctx, func(r types.Reserve) (bool, error) {
		out = append(out, types.ReserveInfo{
			Reserve: r,
			Address: types.ReserveAddress(r.Name).String(),
			Balance: q.k.Balance(ctx, r.Name),
		})
		return false, nil
	})
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	return &types.QueryReservesResponse{Reserves: out}, nil
}

func (q Querier) Reserve(ctx context.Context, req *types.QueryReserveRequest) (*types.QueryReserveResponse, error) {
	if req == nil || req.Name == "" {
		return nil, status.Error(codes.InvalidArgument, "name is required")
	}
	r, found, err := q.k.GetReserve(ctx, req.Name)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	info := types.ReserveInfo{}
	if found {
		info = types.ReserveInfo{
			Reserve: r,
			Address: types.ReserveAddress(r.Name).String(),
			Balance: q.k.Balance(ctx, r.Name),
		}
	}
	return &types.QueryReserveResponse{Found: found, Reserve: info}, nil
}

func (q Querier) Disbursements(ctx context.Context, req *types.QueryDisbursementsRequest) (*types.QueryDisbursementsResponse, error) {
	filter := ""
	if req != nil {
		filter = req.Reserve
	}
	list, err := q.k.Disbursements(ctx, filter)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	return &types.QueryDisbursementsResponse{Disbursements: list}, nil
}
