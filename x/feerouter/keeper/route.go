package keeper

import (
	"context"
	"fmt"

	sdk "github.com/cosmos/cosmos-sdk/types"

	"github.com/hashgram/hashgram/x/feerouter/types"
)

// CollectServiceFee charges a payer an explicit protocol service fee.
//
// The fee is moved into the revenue pool and split on the next block's
// BeginBlock, together with that block's gas fees. Deferring the split is not
// laziness: splitting immediately would deposit the validator remainder into
// the fee collector, where the next block's gas pass would tax it a second
// time.
//
// Other modules call this. For example x/username charges a registration fee
// here rather than transferring to the fee collector directly, so that
// username revenue is recorded as qualifying protocol revenue and appears in
// the per-service breakdown.
func (k Keeper) CollectServiceFee(
	ctx context.Context,
	payer sdk.AccAddress,
	amount sdk.Coins,
	kind types.ServiceKind,
) error {
	if amount.IsZero() {
		return nil
	}
	if !amount.IsValid() || amount.IsAnyNegative() {
		return types.ErrInvalidFeeAmount.Wrapf("%q", amount)
	}
	if err := types.ValidateServiceKind(kind); err != nil {
		return err
	}

	if err := k.bankKeeper.SendCoinsFromAccountToModule(ctx, payer, types.ModuleName, amount); err != nil {
		return fmt.Errorf("x/feerouter: collecting %s service fee %s from %s: %w", kind, amount, payer, err)
	}

	sdkCtx := sdk.UnwrapSDKContext(ctx)
	sdkCtx.EventManager().EmitEvent(sdk.NewEvent(
		types.EventTypeServiceFee,
		sdk.NewAttribute(types.AttributeKeyServiceKind, kind.String()),
		sdk.NewAttribute(types.AttributeKeyPayer, payer.String()),
		sdk.NewAttribute(types.AttributeKeyAmount, amount.String()),
	))

	return nil
}

// BeginBlocker splits the previous block's qualifying protocol fee revenue.
//
// It must run before x/distribution's BeginBlocker, which empties the fee
// collector. The ordering is set explicitly in app.go.
//
// Two passes, in this order:
//
//  1. Gas. Take the Founder share of whatever the fee collector holds and
//     move it to the founder module account. The remainder stays put and
//     x/distribution pays it to validators and delegators this same block.
//
//  2. Service fees. Take the Founder share of the revenue pool, then push the
//     remainder into the fee collector so it joins this block's distribution.
//
// Doing gas first is what prevents double taxation: by the time pass 2 adds
// the service remainder to the fee collector, pass 1 has already measured it.
//
// Errors are logged, not returned. A block must not fail to begin because fee
// routing hit a problem; that would convert an accounting fault into a
// liveness fault for the entire network. Unrouted revenue stays where it is
// and is picked up by the next block.
func (k Keeper) BeginBlocker(ctx sdk.Context) error {
	params, err := k.GetParams(ctx)
	if err != nil {
		return nil // module not configured; nothing to route
	}
	if !params.Enabled {
		return nil
	}

	if err := k.routeGasFees(ctx); err != nil {
		k.logger.Error("routing gas fees failed; revenue remains with the fee collector", "error", err)
	}
	if err := k.routeServiceFees(ctx); err != nil {
		k.logger.Error("routing service fees failed; revenue remains in the pool", "error", err)
	}
	return nil
}

// routeGasFees implements pass 1.
func (k Keeper) routeGasFees(ctx sdk.Context) error {
	feeCollector := k.accountKeeper.GetModuleAddress(k.feeCollectorName)
	qualifying := k.bankKeeper.GetAllBalances(ctx, feeCollector)
	if qualifying.IsZero() {
		return nil
	}

	cut, err := k.founderKeeper.FounderCut(ctx, qualifying)
	if err != nil {
		return err
	}

	if !cut.IsZero() {
		if err := k.founderKeeper.Accrue(ctx, k.feeCollectorName, cut, types.SERVICE_KIND_GAS.String()); err != nil {
			return err
		}
	}

	return k.record(ctx, types.SERVICE_KIND_GAS, qualifying, cut)
}

// routeServiceFees implements pass 2.
func (k Keeper) routeServiceFees(ctx sdk.Context) error {
	qualifying := k.PendingPool(ctx)
	if qualifying.IsZero() {
		return nil
	}

	cut, err := k.founderKeeper.FounderCut(ctx, qualifying)
	if err != nil {
		return err
	}

	if !cut.IsZero() {
		if err := k.founderKeeper.Accrue(ctx, types.ModuleName, cut, "service_fees"); err != nil {
			return err
		}
	}

	// Whatever the founder accrual did or did not take, sweep the rest of the
	// pool to the fee collector. Reading the balance again rather than
	// computing qualifying-cut means a partial accrual cannot leave coins
	// stranded in the pool with the totals already recorded as swept.
	remainder := k.PendingPool(ctx)
	if !remainder.IsZero() {
		if err := k.bankKeeper.SendCoinsFromModuleToModule(ctx, types.ModuleName, k.feeCollectorName, remainder); err != nil {
			return fmt.Errorf("sweeping %s to the fee collector: %w", remainder, err)
		}
	}

	// The actual founder share is qualifying minus what was swept, which
	// equals cut when accrual succeeded and zero when it was skipped.
	actualCut, negative := qualifying.SafeSub(remainder...)
	if negative {
		return types.ErrSplitDoesNotBalance.Wrapf(
			"pool remainder %s exceeds qualifying %s", remainder, qualifying)
	}

	return k.record(ctx, types.SERVICE_KIND_OTHER, qualifying, actualCut)
}

// record updates the cumulative totals and the per-service breakdown, then
// re-asserts the accounting identity.
func (k Keeper) record(ctx sdk.Context, kind types.ServiceKind, qualifying, founderShare sdk.Coins) error {
	validatorShare, negative := qualifying.SafeSub(founderShare...)
	if negative {
		return types.ErrSplitDoesNotBalance.Wrapf(
			"founder share %s exceeds qualifying revenue %s", founderShare, qualifying)
	}

	totals, err := k.GetTotals(ctx)
	if err != nil {
		return err
	}
	totals.TotalQualifying = totals.TotalQualifying.Add(qualifying...)
	totals.FounderShare = totals.FounderShare.Add(founderShare...)
	totals.ValidatorShare = totals.ValidatorShare.Add(validatorShare...)

	// The identity total == founder + validator + treasury must hold after
	// every update. If it does not, the split arithmetic is wrong and we
	// refuse to persist a set of books that does not balance.
	if err := totals.Validate(); err != nil {
		return err
	}
	if err := k.SetTotals(ctx, totals); err != nil {
		return err
	}
	if err := k.addServiceRevenue(ctx, kind, qualifying); err != nil {
		return err
	}

	ctx.EventManager().EmitEvent(sdk.NewEvent(
		types.EventTypeRevenueRouted,
		sdk.NewAttribute(types.AttributeKeyServiceKind, kind.String()),
		sdk.NewAttribute(types.AttributeKeyQualifying, qualifying.String()),
		sdk.NewAttribute(types.AttributeKeyFounderShare, founderShare.String()),
		sdk.NewAttribute(types.AttributeKeyValidatorShare, validatorShare.String()),
	))

	return nil
}

// Ensure the context type used by the keeper interfaces is the plain
// context.Context expected by collections, while BeginBlocker takes an
// sdk.Context for event emission.
var _ = context.Context(nil)
