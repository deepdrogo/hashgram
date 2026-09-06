package keeper

import (
	sdk "github.com/cosmos/cosmos-sdk/types"

	"github.com/hashgram/hashgram/x/feerouter/types"
)

// InitGenesis writes the routing configuration and cumulative totals.
func (k Keeper) InitGenesis(ctx sdk.Context, gs types.GenesisState) error {
	if err := gs.Validate(); err != nil {
		return err
	}
	if err := k.SetParams(ctx, gs.Params); err != nil {
		return err
	}
	if err := k.SetTotals(ctx, gs.Totals); err != nil {
		return err
	}
	for _, sr := range gs.ServiceRevenue {
		if err := k.serviceRevenue.Set(ctx, int32(sr.Kind), sr); err != nil {
			return err
		}
	}
	return nil
}

// ExportGenesis reads the routing configuration and totals back out.
func (k Keeper) ExportGenesis(ctx sdk.Context) (*types.GenesisState, error) {
	params, err := k.GetParams(ctx)
	if err != nil {
		return nil, err
	}
	totals, err := k.GetTotals(ctx)
	if err != nil {
		return nil, err
	}
	serviceRevenue, err := k.AllServiceRevenue(ctx)
	if err != nil {
		return nil, err
	}
	return &types.GenesisState{
		Params:         params,
		Totals:         totals,
		ServiceRevenue: serviceRevenue,
	}, nil
}
