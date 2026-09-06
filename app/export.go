package app

import (
	"encoding/json"
	"fmt"

	cmtproto "github.com/cometbft/cometbft/proto/tendermint/types"

	storetypes "cosmossdk.io/store/types"

	servertypes "github.com/cosmos/cosmos-sdk/server/types"
	sdk "github.com/cosmos/cosmos-sdk/types"
	slashingtypes "github.com/cosmos/cosmos-sdk/x/slashing/types"
	"github.com/cosmos/cosmos-sdk/x/staking"
	stakingtypes "github.com/cosmos/cosmos-sdk/x/staking/types"
)

// ExportAppStateAndValidators exports application state and the validator set.
//
// Used by `hashgramd export`, by `hashgramctl backup`, and by coordinated
// chain upgrades. Note that this is an operator action on a local node: it
// reads state, it cannot change the running network.
func (app *HashgramApp) ExportAppStateAndValidators(
	forZeroHeight bool,
	jailAllowedAddrs, modulesToExport []string,
) (servertypes.ExportedApp, error) {
	ctx := app.NewContextLegacy(true, cmtproto.Header{Height: app.LastBlockHeight()})

	// Export at last height + 1: that is the height at which CometBFT would
	// call InitChain on the resulting genesis.
	height := app.LastBlockHeight() + 1
	if forZeroHeight {
		height = 0
		if err := app.prepForZeroHeightGenesis(ctx, jailAllowedAddrs); err != nil {
			return servertypes.ExportedApp{}, err
		}
	}

	genState, err := app.ModuleManager.ExportGenesisForModules(ctx, app.appCodec, modulesToExport)
	if err != nil {
		return servertypes.ExportedApp{}, err
	}

	appState, err := json.MarshalIndent(genState, "", "  ")
	if err != nil {
		return servertypes.ExportedApp{}, err
	}

	validators, err := staking.WriteValidators(ctx, app.StakingKeeper)
	if err != nil {
		return servertypes.ExportedApp{}, err
	}

	return servertypes.ExportedApp{
		AppState:        appState,
		Validators:      validators,
		Height:          height,
		ConsensusParams: app.GetConsensusParams(ctx),
	}, nil
}

// prepForZeroHeightGenesis rewrites height-dependent state so the exported
// genesis can seed a fresh chain at height zero.
//
// Unlike the SDK reference implementation this returns an error instead of
// calling log.Fatal or panicking on operator input: `hashgramd export` must
// not take down a node that is also validating.
func (app *HashgramApp) prepForZeroHeightGenesis(ctx sdk.Context, jailAllowedAddrs []string) error {
	applyAllowedAddrs := len(jailAllowedAddrs) > 0

	allowedAddrs := make(map[string]bool, len(jailAllowedAddrs))
	for _, addr := range jailAllowedAddrs {
		if _, err := sdk.ValAddressFromBech32(addr); err != nil {
			return fmt.Errorf("invalid validator address %q in jail-allowed list: %w", addr, err)
		}
		allowedAddrs[addr] = true
	}

	valAddrCodec := app.StakingKeeper.ValidatorAddressCodec()

	// --- fee distribution -------------------------------------------------

	var iterErr error
	if err := app.StakingKeeper.IterateValidators(ctx, func(_ int64, val stakingtypes.ValidatorI) bool {
		valBz, err := valAddrCodec.StringToBytes(val.GetOperator())
		if err != nil {
			iterErr = err
			return true
		}
		// Ignore the "no commission to withdraw" case.
		_, _ = app.DistrKeeper.WithdrawValidatorCommission(ctx, valBz)
		return false
	}); err != nil {
		return err
	}
	if iterErr != nil {
		return iterErr
	}

	dels, err := app.StakingKeeper.GetAllDelegations(ctx)
	if err != nil {
		return err
	}
	for _, d := range dels {
		valAddr, err := sdk.ValAddressFromBech32(d.ValidatorAddress)
		if err != nil {
			return err
		}
		delAddr, err := sdk.AccAddressFromBech32(d.DelegatorAddress)
		if err != nil {
			return err
		}
		_, _ = app.DistrKeeper.WithdrawDelegationRewards(ctx, delAddr, valAddr)
	}

	app.DistrKeeper.DeleteAllValidatorSlashEvents(ctx)
	app.DistrKeeper.DeleteAllValidatorHistoricalRewards(ctx)

	// --- reinitialise validators and delegations at height 0 --------------

	height := ctx.BlockHeight()
	ctx = ctx.WithBlockHeight(0)

	if err := app.StakingKeeper.IterateValidators(ctx, func(_ int64, val stakingtypes.ValidatorI) bool {
		valBz, err := valAddrCodec.StringToBytes(val.GetOperator())
		if err != nil {
			iterErr = err
			return true
		}

		// Any unwithdrawn reward dust is donated to the community pool rather
		// than silently dropped, so that exported supply still balances.
		scraps, err := app.DistrKeeper.GetValidatorOutstandingRewardsCoins(ctx, valBz)
		if err != nil {
			iterErr = err
			return true
		}
		feePool, err := app.DistrKeeper.FeePool.Get(ctx)
		if err != nil {
			iterErr = err
			return true
		}
		feePool.CommunityPool = feePool.CommunityPool.Add(scraps...)
		if err := app.DistrKeeper.FeePool.Set(ctx, feePool); err != nil {
			iterErr = err
			return true
		}
		if err := app.DistrKeeper.Hooks().AfterValidatorCreated(ctx, valBz); err != nil {
			iterErr = err
			return true
		}
		return false
	}); err != nil {
		return err
	}
	if iterErr != nil {
		return iterErr
	}

	for _, d := range dels {
		valAddr, err := sdk.ValAddressFromBech32(d.ValidatorAddress)
		if err != nil {
			return err
		}
		delAddr, err := sdk.AccAddressFromBech32(d.DelegatorAddress)
		if err != nil {
			return err
		}
		if err := app.DistrKeeper.Hooks().BeforeDelegationCreated(ctx, delAddr, valAddr); err != nil {
			return fmt.Errorf("incrementing period for %s: %w", d.DelegatorAddress, err)
		}
		if err := app.DistrKeeper.Hooks().AfterDelegationModified(ctx, delAddr, valAddr); err != nil {
			return fmt.Errorf("creating delegation period record for %s: %w", d.DelegatorAddress, err)
		}
	}

	ctx = ctx.WithBlockHeight(height)

	// --- staking: reset creation heights ----------------------------------

	if err := app.StakingKeeper.IterateRedelegations(ctx, func(_ int64, red stakingtypes.Redelegation) bool {
		for i := range red.Entries {
			red.Entries[i].CreationHeight = 0
		}
		if err := app.StakingKeeper.SetRedelegation(ctx, red); err != nil {
			iterErr = err
			return true
		}
		return false
	}); err != nil {
		return fmt.Errorf("iterating redelegations: %w", err)
	}
	if iterErr != nil {
		return iterErr
	}

	if err := app.StakingKeeper.IterateUnbondingDelegations(ctx, func(_ int64, ubd stakingtypes.UnbondingDelegation) bool {
		for i := range ubd.Entries {
			ubd.Entries[i].CreationHeight = 0
		}
		if err := app.StakingKeeper.SetUnbondingDelegation(ctx, ubd); err != nil {
			iterErr = err
			return true
		}
		return false
	}); err != nil {
		return fmt.Errorf("iterating unbonding delegations: %w", err)
	}
	if iterErr != nil {
		return iterErr
	}

	// --- validators: reset bond heights, apply the jail allow-list --------

	store := ctx.KVStore(app.GetKey(stakingtypes.StoreKey))
	iter := storetypes.KVStoreReversePrefixIterator(store, stakingtypes.ValidatorsKey)
	for ; iter.Valid(); iter.Next() {
		addr := sdk.ValAddress(stakingtypes.AddressFromValidatorsKey(iter.Key()))
		validator, err := app.StakingKeeper.GetValidator(ctx, addr)
		if err != nil {
			_ = iter.Close()
			return fmt.Errorf("validator %s present in the index but not in state", addr)
		}

		validator.UnbondingHeight = 0
		if applyAllowedAddrs && !allowedAddrs[addr.String()] {
			validator.Jailed = true
		}

		if err := app.StakingKeeper.SetValidator(ctx, validator); err != nil {
			_ = iter.Close()
			return fmt.Errorf("setting validator %s: %w", addr, err)
		}
	}
	if err := iter.Close(); err != nil {
		return fmt.Errorf("closing validator iterator: %w", err)
	}

	if _, err := app.StakingKeeper.ApplyAndReturnValidatorSetUpdates(ctx); err != nil {
		return err
	}

	// --- slashing: clear downtime history from the previous chain ----------

	if err := app.SlashingKeeper.IterateValidatorSigningInfos(ctx,
		func(addr sdk.ConsAddress, info slashingtypes.ValidatorSigningInfo) bool {
			info.StartHeight = 0
			if err := app.SlashingKeeper.SetValidatorSigningInfo(ctx, addr, info); err != nil {
				iterErr = err
				return true
			}
			return false
		}); err != nil {
		return fmt.Errorf("iterating validator signing info: %w", err)
	}
	return iterErr
}
