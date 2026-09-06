package keeper

import (
	sdk "github.com/cosmos/cosmos-sdk/types"

	"github.com/hashgram/hashgram/x/founder/types"
)

// InitGenesis writes the Founder configuration and ledger.
func (k Keeper) InitGenesis(ctx sdk.Context, gs types.GenesisState) error {
	if err := gs.Validate(); err != nil {
		return err
	}
	if err := k.SetParams(ctx, gs.Params); err != nil {
		return err
	}
	if err := k.SetLedger(ctx, gs.Ledger); err != nil {
		return err
	}

	if len(gs.BeneficiaryHistory) > 0 {
		// Restoring an exported chain: replay the audit trail verbatim.
		for _, ch := range gs.BeneficiaryHistory {
			seq, err := k.historySeq.Next(ctx)
			if err != nil {
				return err
			}
			if err := k.history.Set(ctx, seq, ch); err != nil {
				return err
			}
		}
	} else if err := k.RecordInitialBeneficiary(ctx, gs.Params.Beneficiary); err != nil {
		// Fresh chain: seed the audit trail with the genesis beneficiary so
		// the history starts at launch rather than at the first change.
		return err
	}

	if gs.Params.Beneficiary == "" {
		k.Logger().Info("founder revenue module started with NO beneficiary configured",
			"effect", "the founder share is not taken; all fee revenue goes to validators and delegators",
			"fix", "set GENESIS_FOUNDER_ADDRESS before running hashgramctl init-mainnet-genesis")
	} else {
		k.Logger().Info("founder revenue module configured",
			"beneficiary", gs.Params.Beneficiary,
			"fee_basis_points", gs.Params.FeeBasisPoints,
			"fee_percent", gs.Params.FeePercent(),
			"ceiling_basis_points", k.MaxFeeBasisPoints(),
			"payout_period_blocks", gs.Params.PayoutPeriodBlocks,
		)
	}

	return nil
}

// ExportGenesis reads the Founder configuration and ledger back out.
func (k Keeper) ExportGenesis(ctx sdk.Context) (*types.GenesisState, error) {
	params, err := k.GetParams(ctx)
	if err != nil {
		return nil, err
	}
	ledger, err := k.GetLedger(ctx)
	if err != nil {
		return nil, err
	}
	history, err := k.BeneficiaryHistory(ctx)
	if err != nil {
		return nil, err
	}
	return &types.GenesisState{
		Params:             params,
		Ledger:             ledger,
		BeneficiaryHistory: history,
	}, nil
}
