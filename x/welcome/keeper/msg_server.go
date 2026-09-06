package keeper

import (
	"context"

	"github.com/hashgram/hashgram/x/welcome/types"
)

var _ types.MsgServer = msgServer{}

type msgServer struct {
	k Keeper
}

// NewMsgServerImpl returns the x/welcome message server.
func NewMsgServerImpl(k Keeper) types.MsgServer { return msgServer{k: k} }

// ClaimWelcome pays a welcome reward to the attestation's subject.
//
// The reward goes to msg.Attestation.Subject, never to msg.Sender. Separating
// them is a practical necessity, not a nicety: a brand-new user has a zero
// balance and cannot pay gas, so somebody else has to be able to submit the
// claim for them.
func (s msgServer) ClaimWelcome(ctx context.Context, msg *types.MsgClaimWelcome) (*types.MsgClaimWelcomeResponse, error) {
	record, err := s.k.Claim(ctx, msg.Attestation)
	if err != nil {
		return nil, err
	}
	return &types.MsgClaimWelcomeResponse{
		Subject:  record.Subject,
		Sequence: record.Sequence,
		Amount:   record.Amount,
	}, nil
}

// UpdateParams handles a governance-approved parameter change.
func (s msgServer) UpdateParams(ctx context.Context, msg *types.MsgUpdateParams) (*types.MsgUpdateParamsResponse, error) {
	if err := s.k.UpdateParams(ctx, msg.Authority, msg.Params); err != nil {
		return nil, err
	}
	return &types.MsgUpdateParamsResponse{}, nil
}
