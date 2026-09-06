package keeper

import (
	"context"
	"fmt"

	"cosmossdk.io/math"

	sdk "github.com/cosmos/cosmos-sdk/types"

	"github.com/hashgram/hashgram/x/serviceproof/types"
)

// BeginBlocker advances epochs and settles the one that just closed.
//
// Errors are logged rather than returned. Reward settlement must never fail a
// block: turning an accounting problem into a liveness problem for the whole
// network would be a far worse outcome than a delayed payout. Unsettled work
// stays in state and is retried.
func (k Keeper) BeginBlocker(ctx sdk.Context) error {
	params, err := k.GetParams(ctx)
	if err != nil {
		return nil // module not configured
	}
	if !params.Enabled {
		return nil
	}

	if err := k.ExpireChallenges(ctx, params); err != nil {
		k.logger.Error("expiring storage challenges failed", "error", err)
	}

	start, err := k.EpochStartHeight(ctx)
	if err != nil {
		k.logger.Error("reading epoch start height failed", "error", err)
		return nil
	}
	height := ctx.BlockHeight()

	// Initialise the first epoch on the first block.
	if start == 0 {
		if err := k.SetEpochStartHeight(ctx, height); err != nil {
			k.logger.Error("initialising epoch start height failed", "error", err)
			return nil
		}
		epoch, err := k.CurrentEpochNumber(ctx)
		if err != nil {
			return nil
		}
		if err := k.openEpoch(ctx, epoch, height, params); err != nil {
			k.logger.Error("opening the first epoch failed", "error", err)
		}
		return nil
	}

	if height-start < int64(params.EpochBlocks) {
		return nil
	}

	epoch, err := k.CurrentEpochNumber(ctx)
	if err != nil {
		k.logger.Error("reading current epoch failed", "error", err)
		return nil
	}

	if err := k.SettleEpoch(ctx, epoch, height-1, params); err != nil {
		k.logger.Error("epoch settlement failed; work remains unsettled and will be retried",
			"epoch", epoch, "error", err)
		return nil
	}

	next := epoch + 1
	if err := k.SetCurrentEpochNumber(ctx, next); err != nil {
		k.logger.Error("advancing the epoch counter failed", "error", err)
		return nil
	}
	if err := k.SetEpochStartHeight(ctx, height); err != nil {
		k.logger.Error("setting the new epoch start height failed", "error", err)
		return nil
	}
	if err := k.openEpoch(ctx, next, height, params); err != nil {
		k.logger.Error("opening the next epoch failed", "epoch", next, "error", err)
	}
	return nil
}

// openEpoch records a new open epoch and issues its storage challenges.
func (k Keeper) openEpoch(ctx sdk.Context, number uint64, startHeight int64, params types.Params) error {
	if err := k.SetEpoch(ctx, types.Epoch{
		Number:      number,
		StartHeight: startHeight,
		Budget:      sdk.NewCoins(),
		Distributed: sdk.NewCoins(),
		TotalCredit: math.ZeroInt(),
	}); err != nil {
		return err
	}
	if err := k.decayFraudScores(ctx, params); err != nil {
		return err
	}
	return k.IssueChallenges(ctx, number, params)
}

// SettleEpoch computes and pays one epoch's rewards.
//
// The sequence is:
//
//  1. accrue storage credit for the closing epoch, scaled by challenge
//     success, from assignments rather than declarations
//  2. apply the per-counterparty concentration discount to every provider's
//     credit, which is what makes two nodes trading fake traffic unprofitable
//  3. compute the epoch budget as a declining fraction of the reserve that
//     remains
//  4. pay each provider pro rata, capped at max_provider_share_bps
//  5. assert the finite-reserve invariant and record the result
//
// Unpaid budget is not burned. It stays in the reserve and funds later
// epochs, so an early network with few providers does not waste the
// community's allocation.
func (k Keeper) SettleEpoch(ctx sdk.Context, epoch uint64, endHeight int64, params types.Params) error {
	if err := k.AccrueStorageCredit(ctx, epoch, params); err != nil {
		return fmt.Errorf("accruing storage credit: %w", err)
	}

	// --- 2. discounted credit per provider ---

	type share struct {
		operator string
		credit   math.Int
	}
	var shares []share
	totalCredit := math.ZeroInt()

	if err := k.IterateEpochCredits(ctx, epoch, func(c types.ProviderEpochCredit) (bool, error) {
		effective, distinct, err := k.EffectiveCredit(ctx, epoch, c, params)
		if err != nil {
			return true, err
		}

		c.DistinctClients = distinct
		if err := k.SetCredit(ctx, c); err != nil {
			return true, err
		}

		if !effective.IsPositive() {
			return false, nil
		}
		shares = append(shares, share{operator: c.Operator, credit: effective})
		totalCredit = totalCredit.Add(effective)
		return false, nil
	}); err != nil {
		return fmt.Errorf("collecting epoch credit: %w", err)
	}

	// --- 3. budget ---

	remaining := k.ReserveRemaining(ctx)
	budget := types.EmissionForEpoch(remaining, params)

	e, err := k.GetEpoch(ctx, epoch)
	if err != nil {
		e = types.Epoch{Number: epoch, StartHeight: 0, TotalCredit: math.ZeroInt()}
	}
	e.EndHeight = endHeight
	e.Budget = budget
	e.TotalCredit = totalCredit
	e.Distributed = sdk.NewCoins()
	e.Settled = true

	if !totalCredit.IsPositive() || budget.IsZero() {
		// Nobody did verifiable work, or the reserve is empty. Either way the
		// budget stays where it is.
		return k.SetEpoch(ctx, e)
	}

	// --- 4. pay ---

	providerCap := types.ProviderRewardCap(budget, params)
	distributed := sdk.NewCoins()

	for _, s := range shares {
		reward := types.ProRata(budget, s.credit, totalCredit)
		if reward.IsZero() {
			continue
		}
		// Cap one provider's take, so the subsidy cannot fund
		// centralisation. The capped-away amount stays in the reserve.
		for i, c := range reward {
			if lim := providerCap.AmountOf(c.Denom); lim.IsPositive() && c.Amount.GT(lim) {
				reward[i] = sdk.NewCoin(c.Denom, lim)
			}
		}
		reward = sdk.NewCoins(reward...)
		if reward.IsZero() {
			continue
		}

		provider, err := k.GetProvider(ctx, s.operator)
		if err != nil {
			continue
		}
		rewardAddr, err := sdk.AccAddressFromBech32(provider.RewardAddress)
		if err != nil {
			k.logger.Error("provider has an unparseable reward address; skipping payout",
				"operator", s.operator, "reward_address", provider.RewardAddress)
			continue
		}
		if k.bankKeeper.BlockedAddr(rewardAddr) {
			k.logger.Error("provider reward address is blocked; skipping payout",
				"operator", s.operator, "reward_address", provider.RewardAddress)
			continue
		}

		if err := k.bankKeeper.SendCoinsFromModuleToAccount(ctx, types.ModuleName, rewardAddr, reward); err != nil {
			k.logger.Error("paying a service reward failed; provider skipped this epoch",
				"operator", s.operator, "amount", reward.String(), "error", err)
			continue
		}

		distributed = distributed.Add(reward...)

		credit, err := k.GetCredit(ctx, epoch, s.operator)
		if err == nil {
			credit.Paid = reward
			if err := k.SetCredit(ctx, credit); err != nil {
				return err
			}
		}
		if err := k.addLifetimePaid(ctx, s.operator, reward); err != nil {
			return err
		}

		ctx.EventManager().EmitEvent(sdk.NewEvent(
			types.EventTypeRewardPaid,
			sdk.NewAttribute(types.AttributeKeyProvider, s.operator),
			sdk.NewAttribute(types.AttributeKeyRewardTarget, provider.RewardAddress),
			sdk.NewAttribute(types.AttributeKeyEpoch, fmt.Sprintf("%d", epoch)),
			sdk.NewAttribute(types.AttributeKeyAmount, reward.String()),
			sdk.NewAttribute(types.AttributeKeyCredit, s.credit.String()),
		))
	}

	// --- 5. invariant and record ---

	reserve, err := k.GetReserve(ctx)
	if err != nil {
		return err
	}
	reserve.TotalEmitted = reserve.TotalEmitted.Add(distributed...)

	// The finite-reserve guarantee. It cannot be violated by construction
	// either, since rewards are transfers out of a module account and there
	// is no mint on this chain, but a books-do-not-balance condition is worth
	// refusing to persist rather than logging.
	if err := reserve.Validate(); err != nil {
		return fmt.Errorf("reserve invariant violated at settlement: %w", err)
	}
	if err := k.SetReserve(ctx, reserve); err != nil {
		return err
	}

	e.Distributed = distributed
	if err := k.SetEpoch(ctx, e); err != nil {
		return err
	}

	ctx.EventManager().EmitEvent(sdk.NewEvent(
		types.EventTypeEpochSettled,
		sdk.NewAttribute(types.AttributeKeyEpoch, fmt.Sprintf("%d", epoch)),
		sdk.NewAttribute(types.AttributeKeyBudget, budget.String()),
		sdk.NewAttribute(types.AttributeKeyDistributed, distributed.String()),
		sdk.NewAttribute(types.AttributeKeyCredit, totalCredit.String()),
	))

	k.logger.Info("service epoch settled",
		"epoch", epoch,
		"providers", len(shares),
		"budget", budget.String(),
		"distributed", distributed.String(),
		"total_credit", totalCredit.String(),
	)

	return nil
}

// EffectiveCredit applies the per-counterparty concentration discount and
// returns the credit a provider will actually be paid on, plus how many
// distinct counterparties contributed.
//
// Exported because an operator whose raw credit is high but whose payout is
// low deserves to see why; the Rewards query surfaces both figures.
//
// This is the defence against two nodes trading fake traffic. No signature
// check can stop them: each can legitimately sign for the other. What stops
// them is that credit sourced overwhelmingly from a single counterparty is
// capped at max_client_concentration_bps of the provider's total.
//
// Concretely, with the default 20% cap:
//
//   - An honest relay serving many clients, none of them dominant, keeps all
//     its credit.
//   - A two-node ring where each node's credit comes entirely from the other
//     keeps 20% of it, so the ring must spend five times the resources to
//     earn what it claims, which is worse than doing the real work.
//
// Storage credit is exempt, because it comes from chain-issued challenges
// against chain-recorded assignments rather than from a counterparty at all.
func (k Keeper) EffectiveCredit(
	ctx context.Context,
	epoch uint64,
	c types.ProviderEpochCredit,
	params types.Params,
) (math.Int, uint64, error) {
	counterpartyCredit := c.RelayCredit.Add(c.RetrievalCredit).Add(c.CallCredit)

	largest, distinct, err := k.LargestClientCredit(ctx, epoch, c.Operator)
	if err != nil {
		return math.ZeroInt(), 0, err
	}

	effectiveCounterparty := counterpartyCredit
	if counterpartyCredit.IsPositive() && largest.IsPositive() {
		capBps := math.NewIntFromUint64(uint64(params.MaxClientConcentrationBps))
		denom := math.NewIntFromUint64(uint64(types.BasisPointsMax))

		// allowed = total * cap_bps / 10000, i.e. the most that a single
		// counterparty may contribute.
		allowedPerClient := counterpartyCredit.Mul(capBps).Quo(denom)

		if largest.GT(allowedPerClient) {
			// Scale the whole counterparty credit down by the ratio of what
			// was allowed to what the dominant client actually contributed.
			// A ring where largest == total ends up with exactly cap_bps of
			// its claimed credit.
			effectiveCounterparty = counterpartyCredit.Mul(allowedPerClient).Quo(largest)
		}
	}

	total := c.StorageCredit
	if total.IsNil() {
		total = math.ZeroInt()
	}
	return total.Add(effectiveCounterparty), distinct, nil
}

// decayFraudScores reduces every provider's fraud score at epoch rollover, so
// that one bad day does not permanently condemn an otherwise honest operator.
func (k Keeper) decayFraudScores(ctx context.Context, params types.Params) error {
	if params.FraudScoreDecayPerEpoch == 0 {
		return nil
	}
	return k.IterateProviders(ctx, func(p types.Provider) (bool, error) {
		if p.FraudScore == 0 {
			return false, nil
		}
		if p.FraudScore <= params.FraudScoreDecayPerEpoch {
			p.FraudScore = 0
		} else {
			p.FraudScore -= params.FraudScoreDecayPerEpoch
		}
		return false, k.SetProvider(ctx, p)
	})
}
