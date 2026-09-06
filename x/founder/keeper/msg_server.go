package keeper

import (
	"context"

	"github.com/hashgram/hashgram/x/founder/types"
)

var _ types.MsgServer = msgServer{}

type msgServer struct {
	k Keeper
}

// NewMsgServerImpl returns the x/founder message server.
func NewMsgServerImpl(k Keeper) types.MsgServer { return msgServer{k: k} }

// UpdateParams handles a governance-approved parameter change.
func (s msgServer) UpdateParams(ctx context.Context, msg *types.MsgUpdateParams) (*types.MsgUpdateParamsResponse, error) {
	if err := s.k.UpdateParams(ctx, msg.Authority, msg.Params); err != nil {
		return nil, err
	}
	return &types.MsgUpdateParamsResponse{}, nil
}

// ClaimFounderRevenue pushes pending revenue to the configured beneficiary.
//
// The sender is irrelevant to the destination: funds always go to the
// configured beneficiary. That is what allows the Founder to keep their
// private key entirely off Hashgram infrastructure while still being able to
// have revenue delivered, and it means a compromised server cannot redirect
// the payout, only trigger it.
func (s msgServer) ClaimFounderRevenue(ctx context.Context, msg *types.MsgClaimFounderRevenue) (*types.MsgClaimFounderRevenueResponse, error) {
	beneficiary, err := s.k.Beneficiary(ctx)
	if err != nil {
		return nil, err
	}

	paid, err := s.k.Payout(ctx, types.TriggerClaim)
	if err != nil {
		return nil, err
	}
	if paid.IsZero() {
		return nil, types.ErrNothingToClaim
	}

	return &types.MsgClaimFounderRevenueResponse{
		Paid:        paid,
		Beneficiary: beneficiary.String(),
	}, nil
}
