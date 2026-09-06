package types

import (
	sdk "github.com/cosmos/cosmos-sdk/types"
)

// DefaultGenesis returns the x/founder genesis with no beneficiary set.
func DefaultGenesis() *GenesisState {
	return &GenesisState{
		Params:             DefaultParams(),
		Ledger:             RevenueLedger{TotalAccrued: sdk.NewCoins(), TotalPaid: sdk.NewCoins()},
		BeneficiaryHistory: nil,
	}
}

// NewGenesisState builds genesis with an explicit beneficiary address.
//
// Used by `hashgramctl init-mainnet-genesis`, which obtains the address from
// the operator and never generates one.
func NewGenesisState(beneficiary string) *GenesisState {
	gs := DefaultGenesis()
	gs.Params.Beneficiary = beneficiary
	return gs
}

// Validate checks the genesis state.
func (gs GenesisState) Validate() error {
	if err := gs.Params.Validate(); err != nil {
		return err
	}

	if !gs.Ledger.TotalAccrued.IsValid() {
		return ErrParamsNotSet.Wrapf("ledger total_accrued %q is not a valid coin set", gs.Ledger.TotalAccrued)
	}
	if !gs.Ledger.TotalPaid.IsValid() {
		return ErrParamsNotSet.Wrapf("ledger total_paid %q is not a valid coin set", gs.Ledger.TotalPaid)
	}

	// Paying out more than was ever accrued would mean the ledger describes
	// coins that never entered the module.
	if !gs.Ledger.TotalAccrued.IsAllGTE(gs.Ledger.TotalPaid) {
		return ErrParamsNotSet.Wrapf(
			"ledger total_paid %q exceeds total_accrued %q",
			gs.Ledger.TotalPaid, gs.Ledger.TotalAccrued)
	}

	for i, ch := range gs.BeneficiaryHistory {
		if ch.NewBeneficiary != "" {
			if _, err := sdk.AccAddressFromBech32(ch.NewBeneficiary); err != nil {
				return ErrInvalidBeneficiary.Wrapf("history[%d].new_beneficiary: %v", i, err)
			}
		}
		if ch.PreviousBeneficiary != "" {
			if _, err := sdk.AccAddressFromBech32(ch.PreviousBeneficiary); err != nil {
				return ErrInvalidBeneficiary.Wrapf("history[%d].previous_beneficiary: %v", i, err)
			}
		}
	}

	return nil
}
