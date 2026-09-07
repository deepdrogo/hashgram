package keeper

import (
	"context"

	sdk "github.com/cosmos/cosmos-sdk/types"

	"github.com/hashgram/hashgram/x/username/types"
)

var _ types.MsgServer = msgServer{}

type msgServer struct {
	k Keeper
}

// NewMsgServerImpl returns the x/username message server.
func NewMsgServerImpl(k Keeper) types.MsgServer { return msgServer{k: k} }

func (s msgServer) Register(ctx context.Context, msg *types.MsgRegister) (*types.MsgRegisterResponse, error) {
	owner, err := sdk.AccAddressFromBech32(msg.Owner)
	if err != nil {
		return nil, err
	}
	reg, err := s.k.Register(ctx, owner, msg.Name)
	if err != nil {
		return nil, err
	}
	return &types.MsgRegisterResponse{Name: reg.Name, ExpiryHeight: reg.ExpiryHeight}, nil
}

func (s msgServer) Renew(ctx context.Context, msg *types.MsgRenew) (*types.MsgRenewResponse, error) {
	owner, err := sdk.AccAddressFromBech32(msg.Owner)
	if err != nil {
		return nil, err
	}
	expiry, err := s.k.Renew(ctx, owner, msg.Name)
	if err != nil {
		return nil, err
	}
	return &types.MsgRenewResponse{ExpiryHeight: expiry}, nil
}

func (s msgServer) Transfer(ctx context.Context, msg *types.MsgTransfer) (*types.MsgTransferResponse, error) {
	owner, err := sdk.AccAddressFromBech32(msg.Owner)
	if err != nil {
		return nil, err
	}
	newOwner, err := sdk.AccAddressFromBech32(msg.NewOwner)
	if err != nil {
		return nil, err
	}
	if err := s.k.Transfer(ctx, owner, msg.Name, newOwner); err != nil {
		return nil, err
	}
	return &types.MsgTransferResponse{}, nil
}

func (s msgServer) SetTransferable(ctx context.Context, msg *types.MsgSetTransferable) (*types.MsgSetTransferableResponse, error) {
	owner, err := sdk.AccAddressFromBech32(msg.Owner)
	if err != nil {
		return nil, err
	}
	if err := s.k.SetTransferable(ctx, owner, msg.Name, msg.Transferable); err != nil {
		return nil, err
	}
	return &types.MsgSetTransferableResponse{}, nil
}

func (s msgServer) Release(ctx context.Context, msg *types.MsgRelease) (*types.MsgReleaseResponse, error) {
	owner, err := sdk.AccAddressFromBech32(msg.Owner)
	if err != nil {
		return nil, err
	}
	if err := s.k.Release(ctx, owner, msg.Name); err != nil {
		return nil, err
	}
	return &types.MsgReleaseResponse{}, nil
}

func (s msgServer) UpdateParams(ctx context.Context, msg *types.MsgUpdateParams) (*types.MsgUpdateParamsResponse, error) {
	if err := s.k.UpdateParams(ctx, msg.Authority, msg.Params); err != nil {
		return nil, err
	}
	return &types.MsgUpdateParamsResponse{}, nil
}
