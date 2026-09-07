package keeper

import (
	"context"

	"github.com/hashgram/hashgram/x/identity/types"
)

var _ types.MsgServer = msgServer{}

type msgServer struct {
	k Keeper
}

// NewMsgServerImpl returns the x/identity message server.
func NewMsgServerImpl(k Keeper) types.MsgServer { return msgServer{k: k} }

func (s msgServer) CreateIdentity(ctx context.Context, msg *types.MsgCreateIdentity) (*types.MsgCreateIdentityResponse, error) {
	if err := s.k.CreateIdentity(ctx, msg); err != nil {
		return nil, err
	}
	return &types.MsgCreateIdentityResponse{}, nil
}

func (s msgServer) AddDevice(ctx context.Context, msg *types.MsgAddDevice) (*types.MsgAddDeviceResponse, error) {
	if err := s.k.AddDevice(ctx, msg.Address, msg.Certificate, msg.Label, msg.Platform); err != nil {
		return nil, err
	}
	return &types.MsgAddDeviceResponse{}, nil
}

func (s msgServer) RevokeDevice(ctx context.Context, msg *types.MsgRevokeDevice) (*types.MsgRevokeDeviceResponse, error) {
	if err := s.k.RevokeDevice(ctx, msg.Address, msg.DeviceId); err != nil {
		return nil, err
	}
	return &types.MsgRevokeDeviceResponse{}, nil
}

func (s msgServer) RotateRootKey(ctx context.Context, msg *types.MsgRotateRootKey) (*types.MsgRotateRootKeyResponse, error) {
	if _, err := s.k.RotateRootKey(ctx, msg); err != nil {
		return nil, err
	}
	return &types.MsgRotateRootKeyResponse{}, nil
}

func (s msgServer) SetRecoveryConfig(ctx context.Context, msg *types.MsgSetRecoveryConfig) (*types.MsgSetRecoveryConfigResponse, error) {
	if err := s.k.SetRecoveryConfig(ctx, msg.Address, msg.Recovery); err != nil {
		return nil, err
	}
	return &types.MsgSetRecoveryConfigResponse{}, nil
}

func (s msgServer) InitiateRecovery(ctx context.Context, msg *types.MsgInitiateRecovery) (*types.MsgInitiateRecoveryResponse, error) {
	executable, err := s.k.InitiateRecovery(ctx, msg)
	if err != nil {
		return nil, err
	}
	return &types.MsgInitiateRecoveryResponse{ExecutableHeight: executable}, nil
}

func (s msgServer) ApproveRecovery(ctx context.Context, msg *types.MsgApproveRecovery) (*types.MsgApproveRecoveryResponse, error) {
	approvals, threshold, err := s.k.ApproveRecovery(ctx, msg.Guardian, msg.RootAddress)
	if err != nil {
		return nil, err
	}
	// Both are bounded by the guardian list, which MaxGuardians caps at a
	// small value, so narrowing to the proto's uint32 fields is exact.
	return &types.MsgApproveRecoveryResponse{
		// #nosec G115 -- both bounded by the guardian list, which MaxGuardians caps.
		Approvals: uint32(approvals),
		// #nosec G115 -- as above.
		Threshold: uint32(threshold),
	}, nil
}

func (s msgServer) CancelRecovery(ctx context.Context, msg *types.MsgCancelRecovery) (*types.MsgCancelRecoveryResponse, error) {
	if err := s.k.CancelRecovery(ctx, msg.Address); err != nil {
		return nil, err
	}
	return &types.MsgCancelRecoveryResponse{}, nil
}

func (s msgServer) ExecuteRecovery(ctx context.Context, msg *types.MsgExecuteRecovery) (*types.MsgExecuteRecoveryResponse, error) {
	if err := s.k.ExecuteRecovery(ctx, msg.RootAddress); err != nil {
		return nil, err
	}
	return &types.MsgExecuteRecoveryResponse{}, nil
}

func (s msgServer) RevokeIdentity(ctx context.Context, msg *types.MsgRevokeIdentity) (*types.MsgRevokeIdentityResponse, error) {
	if err := s.k.RevokeIdentity(ctx, msg.Address); err != nil {
		return nil, err
	}
	return &types.MsgRevokeIdentityResponse{}, nil
}
