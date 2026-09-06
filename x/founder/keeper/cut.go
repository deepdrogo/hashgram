package keeper

import (
	"context"

	sdk "github.com/cosmos/cosmos-sdk/types"
)

// FounderCut returns the Founder share of an amount of qualifying revenue.
//
// It returns zero coins, not an error, when the module is unconfigured or the
// share is zero. That choice matters: this is called from BeginBlock, and a
// chain whose Founder beneficiary was never set should route everything to
// validators rather than stop producing blocks.
//
// The arithmetic is integer-only and truncates toward zero, so any remainder
// stays with validators and delegators. See types.Params.FounderCut.
func (k Keeper) FounderCut(ctx context.Context, amount sdk.Coins) (sdk.Coins, error) {
	if amount.IsZero() {
		return sdk.NewCoins(), nil
	}

	params, err := k.GetParams(ctx)
	if err != nil {
		// Unconfigured: take nothing.
		return sdk.NewCoins(), nil
	}
	if params.Beneficiary == "" || params.FeeBasisPoints == 0 {
		return sdk.NewCoins(), nil
	}

	return params.FounderCut(amount), nil
}
