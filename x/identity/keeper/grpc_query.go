package keeper

import (
	"context"

	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"

	sdk "github.com/cosmos/cosmos-sdk/types"
	"github.com/cosmos/cosmos-sdk/types/query"

	"github.com/hashgram/hashgram/x/identity/types"
)

var _ types.QueryServer = Querier{}

// Querier serves x/identity queries.
type Querier struct {
	k Keeper
}

// NewQuerier wraps a keeper as a query server.
func NewQuerier(k Keeper) Querier { return Querier{k: k} }

func (q Querier) Identity(ctx context.Context, req *types.QueryIdentityRequest) (*types.QueryIdentityResponse, error) {
	if req == nil || req.Address == "" {
		return nil, status.Error(codes.InvalidArgument, "address is required")
	}
	if _, err := sdk.AccAddressFromBech32(req.Address); err != nil {
		return nil, status.Errorf(codes.InvalidArgument, "invalid address: %v", err)
	}

	id, found, err := q.k.GetIdentity(ctx, req.Address)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	var active int
	if found {
		active, err = q.k.ActiveDeviceCount(ctx, req.Address)
		if err != nil {
			return nil, status.Error(codes.Internal, err.Error())
		}
	}
	// Bounded by MaxDevicesPerIdentity, a small configured value, so the
	// narrowing to the proto's uint32 field cannot lose information.
	return &types.QueryIdentityResponse{
		Found:    found,
		Identity: id,
		// #nosec G115 -- bounded by MaxDevicesPerIdentity, enforced on every AddDevice.
		ActiveDevices: uint32(active),
	}, nil
}

func (q Querier) Devices(ctx context.Context, req *types.QueryDevicesRequest) (*types.QueryDevicesResponse, error) {
	if req == nil || req.Address == "" {
		return nil, status.Error(codes.InvalidArgument, "address is required")
	}
	devices, err := q.k.ListDevices(ctx, req.Address, req.IncludeRevoked)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	return &types.QueryDevicesResponse{Devices: devices}, nil
}

func (q Querier) Device(ctx context.Context, req *types.QueryDeviceRequest) (*types.QueryDeviceResponse, error) {
	if req == nil || req.Address == "" || req.DeviceId == "" {
		return nil, status.Error(codes.InvalidArgument, "address and device_id are required")
	}
	d, found, err := q.k.GetDevice(ctx, req.Address, req.DeviceId)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	return &types.QueryDeviceResponse{Found: found, Device: d}, nil
}

// ResolveDeviceKey finds the identity a device public key belongs to.
//
// This is the query a client makes when it receives a signed message or
// social event: it needs to establish which identity authorised the signing
// device, rather than trusting the sender's own claim about who they are.
func (q Querier) ResolveDeviceKey(ctx context.Context, req *types.QueryResolveDeviceKeyRequest) (*types.QueryResolveDeviceKeyResponse, error) {
	if req == nil || len(req.DevicePubkey) == 0 {
		return nil, status.Error(codes.InvalidArgument, "device_pubkey is required")
	}
	d, found, err := q.k.ResolveDeviceKey(ctx, req.DevicePubkey)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	return &types.QueryResolveDeviceKeyResponse{
		Found:       found,
		RootAddress: d.RootAddress,
		Device:      d,
	}, nil
}

func (q Querier) Recovery(ctx context.Context, req *types.QueryRecoveryRequest) (*types.QueryRecoveryResponse, error) {
	if req == nil || req.RootAddress == "" {
		return nil, status.Error(codes.InvalidArgument, "root_address is required")
	}
	r, found, err := q.k.GetRecovery(ctx, req.RootAddress)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}

	var threshold uint32
	if id, ok, err := q.k.GetIdentity(ctx, req.RootAddress); err == nil && ok {
		threshold = id.Recovery.Threshold
	}

	return &types.QueryRecoveryResponse{Found: found, Request: r, Threshold: threshold}, nil
}

func (q Querier) Identities(ctx context.Context, req *types.QueryIdentitiesRequest) (*types.QueryIdentitiesResponse, error) {
	if req == nil {
		req = &types.QueryIdentitiesRequest{}
	}
	ids, pageRes, err := query.CollectionPaginate(
		ctx, q.k.Identities(), req.Pagination,
		func(_ string, v types.RootIdentity) (types.RootIdentity, error) { return v, nil },
	)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	return &types.QueryIdentitiesResponse{Identities: ids, Pagination: pageRes}, nil
}
