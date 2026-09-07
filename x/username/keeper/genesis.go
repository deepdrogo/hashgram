package keeper

import (
	sdk "github.com/cosmos/cosmos-sdk/types"

	"github.com/hashgram/hashgram/x/username/types"
)

// InitGenesis writes the username configuration and any restored
// registrations.
func (k Keeper) InitGenesis(ctx sdk.Context, gs types.GenesisState) error {
	if err := gs.Validate(); err != nil {
		return err
	}
	if err := k.SetParams(ctx, gs.Params); err != nil {
		return err
	}
	for _, r := range gs.Registrations {
		if err := k.SetRegistration(ctx, r); err != nil {
			return err
		}
	}

	k.Logger().Info("username namespace configured",
		"registration_fee", gs.Params.RegistrationFee.String(),
		"period_blocks", gs.Params.RegistrationPeriodBlocks,
		"min_length", gs.Params.MinLength,
		"max_length", gs.Params.MaxLength,
		"reserved", len(gs.Params.ReservedNames),
		"non_ascii_allowed", gs.Params.AllowNonAscii,
		"restored_registrations", len(gs.Registrations),
	)
	return nil
}

// ExportGenesis reads the username state back out.
func (k Keeper) ExportGenesis(ctx sdk.Context) (*types.GenesisState, error) {
	params, err := k.GetParams(ctx)
	if err != nil {
		return nil, err
	}

	var regs []types.Registration
	if err := k.IterateRegistrations(ctx, func(r types.Registration) (bool, error) {
		regs = append(regs, r)
		return false, nil
	}); err != nil {
		return nil, err
	}

	return &types.GenesisState{Params: params, Registrations: regs}, nil
}
