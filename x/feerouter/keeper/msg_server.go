package keeper

import (
	"context"

	"github.com/hashgram/hashgram/x/feerouter/types"
)

var _ types.MsgServer = msgServer{}

type msgServer struct {
	k Keeper
}

// NewMsgServerImpl returns the x/feerouter message server.
func NewMsgServerImpl(k Keeper) types.MsgServer { return msgServer{k: k} }

// UpdateParams handles a governance-approved parameter change.
func (s msgServer) UpdateParams(ctx context.Context, msg *types.MsgUpdateParams) (*types.MsgUpdateParamsResponse, error) {
	if err := s.k.UpdateParams(ctx, msg.Authority, msg.Params); err != nil {
		return nil, err
	}
	return &types.MsgUpdateParamsResponse{}, nil
}
