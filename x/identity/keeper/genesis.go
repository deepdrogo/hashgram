package keeper

import (
	sdk "github.com/cosmos/cosmos-sdk/types"

	"github.com/hashgram/hashgram/x/identity/types"
)

// InitGenesis writes the identity configuration and any restored state.
func (k Keeper) InitGenesis(ctx sdk.Context, gs types.GenesisState) error {
	if err := gs.Validate(); err != nil {
		return err
	}
	if err := k.SetParams(ctx, gs.Params); err != nil {
		return err
	}
	for _, id := range gs.Identities {
		if err := k.SetIdentity(ctx, id); err != nil {
			return err
		}
	}
	for _, d := range gs.Devices {
		if err := k.SetDevice(ctx, d); err != nil {
			return err
		}
	}
	for _, r := range gs.RecoveryRequests {
		if err := k.SetRecovery(ctx, r); err != nil {
			return err
		}
	}

	k.Logger().Info("identity system configured",
		"max_devices_per_identity", gs.Params.MaxDevicesPerIdentity,
		"max_guardians", gs.Params.MaxGuardians,
		"min_recovery_delay_blocks", gs.Params.MinRecoveryDelayBlocks,
		"restored_identities", len(gs.Identities),
		"restored_devices", len(gs.Devices),
		"note", "only public keys are stored; no Hashgram server holds a user private key",
	)
	return nil
}

// ExportGenesis reads the identity state back out.
func (k Keeper) ExportGenesis(ctx sdk.Context) (*types.GenesisState, error) {
	params, err := k.GetParams(ctx)
	if err != nil {
		return nil, err
	}

	var ids []types.RootIdentity
	if err := k.IterateIdentities(ctx, func(id types.RootIdentity) (bool, error) {
		ids = append(ids, id)
		return false, nil
	}); err != nil {
		return nil, err
	}

	devices, err := k.AllDevices(ctx)
	if err != nil {
		return nil, err
	}
	recoveries, err := k.AllRecoveries(ctx)
	if err != nil {
		return nil, err
	}

	return &types.GenesisState{
		Params:           params,
		Identities:       ids,
		Devices:          devices,
		RecoveryRequests: recoveries,
	}, nil
}
