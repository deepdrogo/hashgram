package keeper

import (
	"context"

	"github.com/hashgram/hashgram/x/serviceproof/types"
)

var _ types.MsgServer = msgServer{}

type msgServer struct {
	k Keeper
}

// NewMsgServerImpl returns the x/serviceproof message server.
func NewMsgServerImpl(k Keeper) types.MsgServer { return msgServer{k: k} }

func (s msgServer) RegisterProvider(ctx context.Context, msg *types.MsgRegisterProvider) (*types.MsgRegisterProviderResponse, error) {
	if err := s.k.RegisterProvider(ctx, msg); err != nil {
		return nil, err
	}
	return &types.MsgRegisterProviderResponse{}, nil
}

func (s msgServer) UpdateProvider(ctx context.Context, msg *types.MsgUpdateProvider) (*types.MsgUpdateProviderResponse, error) {
	if err := s.k.UpdateProvider(ctx, msg); err != nil {
		return nil, err
	}
	return &types.MsgUpdateProviderResponse{}, nil
}

func (s msgServer) SubmitReceipts(ctx context.Context, msg *types.MsgSubmitReceipts) (*types.MsgSubmitReceiptsResponse, error) {
	accepted, rejected, reasons, err := s.k.SubmitReceipts(ctx, msg.Receipts)
	if err != nil {
		return nil, err
	}
	return &types.MsgSubmitReceiptsResponse{
		Accepted:         accepted,
		Rejected:         rejected,
		RejectionReasons: reasons,
	}, nil
}

func (s msgServer) AnswerChallenge(ctx context.Context, msg *types.MsgAnswerChallenge) (*types.MsgAnswerChallengeResponse, error) {
	passed, err := s.k.AnswerChallenge(ctx, msg.Operator, msg.Response)
	if err != nil {
		return nil, err
	}
	return &types.MsgAnswerChallengeResponse{Passed: passed}, nil
}

func (s msgServer) Unjail(ctx context.Context, msg *types.MsgUnjail) (*types.MsgUnjailResponse, error) {
	if err := s.k.Unjail(ctx, msg.Operator); err != nil {
		return nil, err
	}
	return &types.MsgUnjailResponse{}, nil
}

func (s msgServer) BeginUnbonding(ctx context.Context, msg *types.MsgBeginUnbonding) (*types.MsgBeginUnbondingResponse, error) {
	completion, err := s.k.BeginUnbonding(ctx, msg.Operator)
	if err != nil {
		return nil, err
	}
	return &types.MsgBeginUnbondingResponse{CompletionHeight: completion}, nil
}

func (s msgServer) WithdrawBond(ctx context.Context, msg *types.MsgWithdrawBond) (*types.MsgWithdrawBondResponse, error) {
	returned, err := s.k.WithdrawBond(ctx, msg.Operator)
	if err != nil {
		return nil, err
	}
	return &types.MsgWithdrawBondResponse{Returned: returned}, nil
}

func (s msgServer) AssignStorage(ctx context.Context, msg *types.MsgAssignStorage) (*types.MsgAssignStorageResponse, error) {
	if err := s.k.AssignStorage(ctx, msg.Assigner, msg.Assignment); err != nil {
		return nil, err
	}
	return &types.MsgAssignStorageResponse{}, nil
}

func (s msgServer) ReleaseStorage(ctx context.Context, msg *types.MsgReleaseStorage) (*types.MsgReleaseStorageResponse, error) {
	if err := s.k.ReleaseStorage(ctx, msg.Assigner, msg.BlobId, msg.Provider, msg.ReplicaIndex); err != nil {
		return nil, err
	}
	return &types.MsgReleaseStorageResponse{}, nil
}

func (s msgServer) UpdateParams(ctx context.Context, msg *types.MsgUpdateParams) (*types.MsgUpdateParamsResponse, error) {
	if err := s.k.UpdateParams(ctx, msg.Authority, msg.Params, msg.Assigners); err != nil {
		return nil, err
	}
	return &types.MsgUpdateParamsResponse{}, nil
}
