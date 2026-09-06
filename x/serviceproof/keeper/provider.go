package keeper

import (
	"context"
	"fmt"

	"cosmossdk.io/collections"
	"cosmossdk.io/math"

	sdk "github.com/cosmos/cosmos-sdk/types"

	"github.com/hashgram/hashgram/x/serviceproof/types"
)

// RegisterProvider registers a useful-service node and escrows its bond.
func (k Keeper) RegisterProvider(ctx context.Context, msg *types.MsgRegisterProvider) error {
	params, err := k.GetParams(ctx)
	if err != nil {
		return err
	}
	if !params.Enabled {
		return types.ErrDisabled
	}

	exists, err := k.HasProvider(ctx, msg.Operator)
	if err != nil {
		return err
	}
	if exists {
		return types.ErrProviderExists.Wrapf("%s", msg.Operator)
	}

	if !msg.Bond.IsAllGTE(params.MinBond) {
		return types.ErrInsufficientBond.Wrapf(
			"bond %s is below the required minimum %s", msg.Bond, params.MinBond)
	}

	operator, err := sdk.AccAddressFromBech32(msg.Operator)
	if err != nil {
		return err
	}

	// The bond is escrowed in a dedicated module account, separate from the
	// reward reserve, so that "how much is left to pay out?" stays answerable
	// from a bank query.
	if err := k.bankKeeper.SendCoinsFromAccountToModule(ctx, operator, types.BondPoolName, msg.Bond); err != nil {
		return fmt.Errorf("escrowing bond %s: %w", msg.Bond, err)
	}

	rewardAddress := msg.RewardAddress
	if rewardAddress == "" {
		rewardAddress = msg.Operator
	}

	sdkCtx := sdk.UnwrapSDKContext(ctx)
	provider := types.Provider{
		Operator:             msg.Operator,
		RewardAddress:        rewardAddress,
		NodePubkey:           msg.NodePubkey,
		Roles:                msg.Roles,
		Bond:                 msg.Bond,
		DeclaredStorageBytes: msg.DeclaredStorageBytes,
		DeclaredBandwidthBps: msg.DeclaredBandwidthBps,
		RegisteredHeight:     sdkCtx.BlockHeight(),
		Moniker:              msg.Moniker,
	}
	if err := k.SetProvider(ctx, provider); err != nil {
		return err
	}

	sdkCtx.EventManager().EmitEvent(sdk.NewEvent(
		types.EventTypeProviderRegistered,
		sdk.NewAttribute(types.AttributeKeyProvider, msg.Operator),
		sdk.NewAttribute(types.AttributeKeyRewardTarget, rewardAddress),
		sdk.NewAttribute(types.AttributeKeyAmount, msg.Bond.String()),
	))

	k.logger.Info("useful-service provider registered",
		"operator", msg.Operator,
		"reward_address", rewardAddress,
		"roles", msg.Roles,
		"bond", msg.Bond.String(),
		"declared_storage_bytes", msg.DeclaredStorageBytes,
	)

	return nil
}

// UpdateProvider changes a provider's mutable fields and optionally tops up
// its bond.
func (k Keeper) UpdateProvider(ctx context.Context, msg *types.MsgUpdateProvider) error {
	provider, err := k.GetProvider(ctx, msg.Operator)
	if err != nil {
		return err
	}
	if provider.UnbondingHeight != 0 {
		return types.ErrUnbonding.Wrap("cannot update a provider that is unbonding")
	}

	if msg.RewardAddress != "" {
		provider.RewardAddress = msg.RewardAddress
	}
	if len(msg.Roles) > 0 {
		provider.Roles = msg.Roles
	}
	if msg.DeclaredStorageBytes > 0 {
		// Shrinking below what is already assigned would create assignments
		// the provider is no longer permitted to hold. Release assignments
		// first.
		assigned, err := k.AssignedBytes(ctx, msg.Operator)
		if err != nil {
			return err
		}
		if msg.DeclaredStorageBytes < assigned {
			return types.ErrCapacityExceeded.Wrapf(
				"cannot declare %d bytes while %d bytes are already assigned; release assignments first",
				msg.DeclaredStorageBytes, assigned)
		}
		provider.DeclaredStorageBytes = msg.DeclaredStorageBytes
	}
	if msg.DeclaredBandwidthBps > 0 {
		provider.DeclaredBandwidthBps = msg.DeclaredBandwidthBps
	}
	if msg.Moniker != "" {
		provider.Moniker = msg.Moniker
	}

	if !msg.AdditionalBond.IsZero() {
		operator, err := sdk.AccAddressFromBech32(msg.Operator)
		if err != nil {
			return err
		}
		if err := k.bankKeeper.SendCoinsFromAccountToModule(ctx, operator, types.BondPoolName, msg.AdditionalBond); err != nil {
			return fmt.Errorf("escrowing additional bond %s: %w", msg.AdditionalBond, err)
		}
		provider.Bond = provider.Bond.Add(msg.AdditionalBond...)
	}

	return k.SetProvider(ctx, provider)
}

// Unjail releases a provider whose jail period has elapsed.
//
// Unjailing resets the fraud score. The slash already happened; keeping the
// score would mean a second jailing on the next single failure, which turns
// one incident into a spiral.
func (k Keeper) Unjail(ctx context.Context, operator string) error {
	provider, err := k.GetProvider(ctx, operator)
	if err != nil {
		return err
	}
	if !provider.Jailed {
		return nil
	}

	sdkCtx := sdk.UnwrapSDKContext(ctx)
	if sdkCtx.BlockHeight() < provider.JailedUntilHeight {
		return types.ErrJailPeriodActive.Wrapf(
			"jailed until height %d, current height is %d",
			provider.JailedUntilHeight, sdkCtx.BlockHeight())
	}

	params, err := k.GetParams(ctx)
	if err != nil {
		return err
	}
	// A provider whose bond fell below the minimum after slashing must top up
	// before it can earn again.
	if !provider.Bond.IsAllGTE(params.MinBond) {
		return types.ErrInsufficientBond.Wrapf(
			"bond %s is below the minimum %s after slashing; top up with MsgUpdateProvider first",
			provider.Bond, params.MinBond)
	}

	provider.Jailed = false
	provider.JailedUntilHeight = 0
	provider.FraudScore = 0

	return k.SetProvider(ctx, provider)
}

// BeginUnbonding starts the bond withdrawal timer.
//
// The provider stops earning immediately but cannot touch the bond until the
// unbonding period elapses. The delay must outlast the window in which fraud
// could still be discovered; otherwise an operator could cheat, unbond, and
// walk away before the evidence lands.
func (k Keeper) BeginUnbonding(ctx context.Context, operator string) (int64, error) {
	provider, err := k.GetProvider(ctx, operator)
	if err != nil {
		return 0, err
	}
	if provider.UnbondingHeight != 0 {
		return 0, types.ErrUnbonding.Wrap("unbonding has already begun")
	}

	params, err := k.GetParams(ctx)
	if err != nil {
		return 0, err
	}

	sdkCtx := sdk.UnwrapSDKContext(ctx)
	provider.UnbondingHeight = sdkCtx.BlockHeight()
	if err := k.SetProvider(ctx, provider); err != nil {
		return 0, err
	}

	return provider.UnbondingHeight + params.UnbondingBlocks, nil
}

// WithdrawBond returns a bond whose unbonding period has completed.
func (k Keeper) WithdrawBond(ctx context.Context, operator string) (sdk.Coins, error) {
	provider, err := k.GetProvider(ctx, operator)
	if err != nil {
		return nil, err
	}
	if provider.UnbondingHeight == 0 {
		return nil, types.ErrNotUnbonding.Wrap("send MsgBeginUnbonding first")
	}
	if provider.Bond.IsZero() {
		return nil, types.ErrNoBond
	}

	params, err := k.GetParams(ctx)
	if err != nil {
		return nil, err
	}

	sdkCtx := sdk.UnwrapSDKContext(ctx)
	completion := provider.UnbondingHeight + params.UnbondingBlocks
	if sdkCtx.BlockHeight() < completion {
		return nil, types.ErrUnbondingIncomplete.Wrapf(
			"bond is withdrawable at height %d, current height is %d",
			completion, sdkCtx.BlockHeight())
	}

	addr, err := sdk.AccAddressFromBech32(operator)
	if err != nil {
		return nil, err
	}

	bond := provider.Bond
	if err := k.bankKeeper.SendCoinsFromModuleToAccount(ctx, types.BondPoolName, addr, bond); err != nil {
		return nil, fmt.Errorf("returning bond %s: %w", bond, err)
	}

	// The registration is removed once the bond is gone: a provider with no
	// bond has nothing at stake and must re-register to earn again.
	if err := k.providers.Remove(ctx, operator); err != nil {
		return nil, err
	}

	return bond, nil
}

// addFraudScore records a fraud report, increases the score, and jails and
// slashes the provider if the threshold is crossed.
//
// The provider is mutated in place; the caller persists it. Callers already
// hold the provider for other reasons, and re-reading it here would risk two
// writes racing within one message.
func (k Keeper) addFraudScore(
	ctx context.Context,
	provider *types.Provider,
	reason, detail string,
	score uint32,
	params types.Params,
) {
	seq, err := k.fraudSeq.Next(ctx)
	if err != nil {
		k.logger.Error("allocating a fraud report sequence failed", "error", err)
		return
	}

	sdkCtx := sdk.UnwrapSDKContext(ctx)
	report := types.FraudReport{
		Provider:   provider.Operator,
		Sequence:   seq,
		Height:     sdkCtx.BlockHeight(),
		Reason:     reason,
		Detail:     detail,
		ScoreAdded: score,
	}
	if err := k.fraudReports.Set(ctx, collections.Join(provider.Operator, seq), report); err != nil {
		k.logger.Error("recording a fraud report failed", "error", err)
	}

	provider.FraudScore += score

	k.logger.Info("provider fraud score increased",
		"operator", provider.Operator,
		"reason", reason,
		"detail", detail,
		"added", score,
		"score", provider.FraudScore,
		"threshold", params.FraudScoreJailThreshold,
	)

	if provider.FraudScore < params.FraudScoreJailThreshold || provider.Jailed {
		return
	}

	provider.Jailed = true
	provider.JailedUntilHeight = sdkCtx.BlockHeight() + params.JailDurationBlocks

	slashed := k.slashBond(ctx, provider, params)

	sdkCtx.EventManager().EmitEvent(sdk.NewEvent(
		types.EventTypeProviderJailed,
		sdk.NewAttribute(types.AttributeKeyProvider, provider.Operator),
		sdk.NewAttribute(types.AttributeKeyReason, reason),
		sdk.NewAttribute(types.AttributeKeyFraudScore, fmt.Sprintf("%d", provider.FraudScore)),
		sdk.NewAttribute(types.AttributeKeySlashed, slashed.String()),
	))
}

// slashBond takes a fraction of a provider's bond and returns it to the
// reward reserve.
//
// Slashed bond funds honest providers rather than being burned. Burning would
// shrink the supply, which is a change to the monetary base as a side effect
// of a disciplinary action; returning it to the reserve keeps the two
// separate.
func (k Keeper) slashBond(ctx context.Context, provider *types.Provider, params types.Params) sdk.Coins {
	if params.SlashFractionBps == 0 || provider.Bond.IsZero() {
		return sdk.NewCoins()
	}

	frac := math.NewIntFromUint64(uint64(params.SlashFractionBps))
	denom := math.NewIntFromUint64(uint64(types.BasisPointsMax))

	slash := make(sdk.Coins, 0, len(provider.Bond))
	for _, c := range provider.Bond {
		amt := c.Amount.Mul(frac).Quo(denom)
		if amt.IsPositive() {
			slash = append(slash, sdk.NewCoin(c.Denom, amt))
		}
	}
	slashed := sdk.NewCoins(slash...)
	if slashed.IsZero() {
		return slashed
	}

	if err := k.bankKeeper.SendCoinsFromModuleToModule(ctx, types.BondPoolName, types.ModuleName, slashed); err != nil {
		k.logger.Error("moving slashed bond to the reserve failed", "error", err)
		return sdk.NewCoins()
	}

	remaining, negative := provider.Bond.SafeSub(slashed...)
	if negative {
		k.logger.Error("slash exceeded bond; refusing to record a negative bond",
			"operator", provider.Operator, "bond", provider.Bond.String(), "slash", slashed.String())
		return sdk.NewCoins()
	}
	provider.Bond = remaining

	reserve, err := k.GetReserve(ctx)
	if err == nil {
		reserve.TotalSlashed = reserve.TotalSlashed.Add(slashed...)
		if err := k.SetReserve(ctx, reserve); err != nil {
			k.logger.Error("recording slashed total failed", "error", err)
		}
	}

	sdk.UnwrapSDKContext(ctx).EventManager().EmitEvent(sdk.NewEvent(
		types.EventTypeProviderSlashed,
		sdk.NewAttribute(types.AttributeKeyProvider, provider.Operator),
		sdk.NewAttribute(types.AttributeKeySlashed, slashed.String()),
	))

	return slashed
}

// FraudReports returns a provider's fraud history in order.
func (k Keeper) FraudReports(ctx context.Context, operator string) ([]types.FraudReport, error) {
	var out []types.FraudReport
	rng := collections.NewPrefixedPairRange[string, uint64](operator)
	err := k.fraudReports.Walk(ctx, rng, func(_ collections.Pair[string, uint64], v types.FraudReport) (bool, error) {
		out = append(out, v)
		return false, nil
	})
	return out, err
}

// AllFraudReports returns every fraud report, for genesis export.
func (k Keeper) AllFraudReports(ctx context.Context) ([]types.FraudReport, error) {
	var out []types.FraudReport
	err := k.fraudReports.Walk(ctx, nil, func(_ collections.Pair[string, uint64], v types.FraudReport) (bool, error) {
		out = append(out, v)
		return false, nil
	})
	return out, err
}

// SetFraudReport writes a fraud report, used by genesis.
func (k Keeper) SetFraudReport(ctx context.Context, r types.FraudReport) error {
	return k.fraudReports.Set(ctx, collections.Join(r.Provider, r.Sequence), r)
}
