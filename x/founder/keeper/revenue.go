package keeper

import (
	"context"
	"fmt"

	sdk "github.com/cosmos/cosmos-sdk/types"

	"github.com/hashgram/hashgram/x/founder/types"
)

// Accrue credits Founder revenue.
//
// The coins must already be sitting in `fromModule`'s account; this moves them
// into the founder module account and records the accrual. It creates nothing:
// x/founder holds no Minter permission and calls no mint function.
//
// Called by x/feerouter once per block after it has computed the Founder share
// of qualifying revenue. Nothing else should call it, which is why the caller
// must name the source module and the source service kind: an unexplained
// accrual would defeat the point of the ledger.
func (k Keeper) Accrue(ctx context.Context, fromModule string, amount sdk.Coins, source string) error {
	if amount.IsZero() {
		return nil
	}
	if !amount.IsValid() || amount.IsAnyNegative() {
		return fmt.Errorf("x/founder: refusing to accrue invalid amount %q", amount)
	}

	// If no beneficiary is configured, do not accrue. The coins stay where
	// they are and flow to validators and delegators through the normal
	// distribution path. Accruing into an account nobody can withdraw from
	// would silently strand real value.
	params, err := k.GetParams(ctx)
	if err != nil || params.Beneficiary == "" {
		k.logger.Debug("skipping founder accrual: no beneficiary configured",
			"amount", amount.String(), "source", source)
		return nil
	}

	if err := k.bankKeeper.SendCoinsFromModuleToModule(ctx, fromModule, types.ModuleName, amount); err != nil {
		return fmt.Errorf("x/founder: moving %s from %s: %w", amount, fromModule, err)
	}

	ledger, err := k.GetLedger(ctx)
	if err != nil {
		return err
	}
	ledger.TotalAccrued = ledger.TotalAccrued.Add(amount...)
	if err := k.SetLedger(ctx, ledger); err != nil {
		return err
	}

	sdkCtx := sdk.UnwrapSDKContext(ctx)
	sdkCtx.EventManager().EmitEvent(sdk.NewEvent(
		types.EventTypeRevenueAccrued,
		sdk.NewAttribute(types.AttributeKeyAmount, amount.String()),
		sdk.NewAttribute(types.AttributeKeySource, source),
	))

	return nil
}

// Payout pushes all pending revenue to the configured beneficiary.
//
// Returns the amount paid, which is empty when there is nothing pending. The
// trigger string distinguishes an automatic EndBlock payout from an explicit
// MsgClaimFounderRevenue in the emitted event.
func (k Keeper) Payout(ctx context.Context, trigger string) (sdk.Coins, error) {
	beneficiary, err := k.Beneficiary(ctx)
	if err != nil {
		return nil, err
	}

	// A beneficiary that x/bank refuses to credit would make every payout
	// fail. Catch it here with a clear message rather than surfacing a bank
	// error from inside a block.
	if k.bankKeeper.BlockedAddr(beneficiary) {
		return nil, types.ErrInvalidBeneficiary.Wrapf(
			"beneficiary %s is a blocked address and cannot receive funds", beneficiary)
	}

	pending := k.Pending(ctx)
	if pending.IsZero() {
		return sdk.NewCoins(), nil
	}

	if err := k.bankKeeper.SendCoinsFromModuleToAccount(ctx, types.ModuleName, beneficiary, pending); err != nil {
		return nil, fmt.Errorf("x/founder: paying %s to %s: %w", pending, beneficiary, err)
	}

	sdkCtx := sdk.UnwrapSDKContext(ctx)

	ledger, err := k.GetLedger(ctx)
	if err != nil {
		return nil, err
	}
	ledger.TotalPaid = ledger.TotalPaid.Add(pending...)
	ledger.LastPayoutHeight = sdkCtx.BlockHeight()
	ledger.LastPayoutBeneficiary = beneficiary.String()
	if err := k.SetLedger(ctx, ledger); err != nil {
		return nil, err
	}

	sdkCtx.EventManager().EmitEvent(sdk.NewEvent(
		types.EventTypeRevenuePaid,
		sdk.NewAttribute(types.AttributeKeyAmount, pending.String()),
		sdk.NewAttribute(types.AttributeKeyBeneficiary, beneficiary.String()),
		sdk.NewAttribute(types.AttributeKeyTrigger, trigger),
	))

	k.logger.Info("founder revenue paid",
		"amount", pending.String(),
		"beneficiary", beneficiary.String(),
		"trigger", trigger,
		"height", sdkCtx.BlockHeight(),
	)

	return pending, nil
}

// EndBlocker performs the periodic automatic payout.
//
// It never returns an error to the caller for a payout failure. A block must
// not fail to finalise because the Founder could not be paid: that would turn
// a payment problem into a liveness problem for the whole network. Failures
// are logged and the revenue stays pending, recoverable by the next period or
// by anyone sending MsgClaimFounderRevenue.
func (k Keeper) EndBlocker(ctx sdk.Context) error {
	params, err := k.GetParams(ctx)
	if err != nil {
		// No params means the module was never configured. Nothing to do.
		return nil
	}
	if params.PayoutPeriodBlocks == 0 || params.Beneficiary == "" {
		return nil
	}

	height := ctx.BlockHeight()
	if height <= 0 || uint64(height)%params.PayoutPeriodBlocks != 0 {
		return nil
	}

	pending := k.Pending(ctx)
	if pending.IsZero() {
		return nil
	}
	if !params.MinPayout.IsZero() && !pending.IsAllGTE(params.MinPayout) {
		k.logger.Debug("founder payout below threshold, waiting",
			"pending", pending.String(), "min_payout", params.MinPayout.String())
		return nil
	}

	if _, err := k.Payout(ctx, types.TriggerAutomatic); err != nil {
		k.logger.Error("automatic founder payout failed; revenue remains pending",
			"error", err, "pending", pending.String(), "height", height)
	}
	return nil
}

// UpdateParams applies a governance-approved parameter change.
//
// A beneficiary change is appended to the audit history so that a silent swap
// is not possible: the full chain of beneficiaries is queryable.
func (k Keeper) UpdateParams(ctx context.Context, authority string, next types.Params) error {
	if authority != k.authority {
		return types.ErrInvalidAuthority.Wrapf(
			"expected %s, got %s", k.authority, authority)
	}
	if err := next.Validate(); err != nil {
		return err
	}

	current, err := k.GetParams(ctx)
	if err != nil && !isNotSet(err) {
		return err
	}

	if current.Beneficiary != next.Beneficiary {
		// Pay out to the outgoing beneficiary before the switch. Revenue that
		// accrued under the old configuration belongs to the old address;
		// carrying it across a governance change would silently reassign it.
		if current.Beneficiary != "" && !k.Pending(ctx).IsZero() {
			if _, err := k.Payout(ctx, types.TriggerAutomatic); err != nil {
				return fmt.Errorf("settling revenue with the outgoing beneficiary: %w", err)
			}
		}
		if err := k.appendBeneficiaryChange(ctx, current.Beneficiary, next.Beneficiary); err != nil {
			return err
		}

		sdkCtx := sdk.UnwrapSDKContext(ctx)
		sdkCtx.EventManager().EmitEvent(sdk.NewEvent(
			types.EventTypeBeneficiarySet,
			sdk.NewAttribute(types.AttributeKeyPreviousBeneficiary, current.Beneficiary),
			sdk.NewAttribute(types.AttributeKeyBeneficiary, next.Beneficiary),
		))
	}

	return k.SetParams(ctx, next)
}

func isNotSet(err error) bool {
	return types.ErrParamsNotSet.Is(err)
}
