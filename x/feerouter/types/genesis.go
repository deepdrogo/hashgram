package types

import (
	sdk "github.com/cosmos/cosmos-sdk/types"
)

// DefaultParams returns the launch routing configuration.
func DefaultParams() Params {
	return Params{Enabled: true}
}

// Validate checks the parameter set.
func (p Params) Validate() error { return nil }

// DefaultGenesis returns the x/feerouter genesis state.
func DefaultGenesis() *GenesisState {
	return &GenesisState{
		Params:         DefaultParams(),
		Totals:         EmptyTotals(),
		ServiceRevenue: nil,
	}
}

// EmptyTotals returns a zeroed RevenueTotals with non-nil coin slices, so
// that arithmetic on a fresh chain does not have to special-case nil.
func EmptyTotals() RevenueTotals {
	return RevenueTotals{
		TotalQualifying: sdk.NewCoins(),
		FounderShare:    sdk.NewCoins(),
		ValidatorShare:  sdk.NewCoins(),
		TreasuryShare:   sdk.NewCoins(),
	}
}

// Validate checks the genesis state.
func (gs GenesisState) Validate() error {
	if err := gs.Params.Validate(); err != nil {
		return err
	}
	if err := gs.Totals.Validate(); err != nil {
		return err
	}

	seen := make(map[ServiceKind]bool, len(gs.ServiceRevenue))
	for i, sr := range gs.ServiceRevenue {
		if sr.Kind == SERVICE_KIND_UNSPECIFIED {
			return ErrInvalidServiceKind.Wrapf("service_revenue[%d] has an unspecified kind", i)
		}
		if seen[sr.Kind] {
			return ErrInvalidServiceKind.Wrapf("service_revenue has duplicate entries for %s", sr.Kind)
		}
		seen[sr.Kind] = true

		if !sr.Amount.IsValid() || sr.Amount.IsAnyNegative() {
			return ErrInvalidFeeAmount.Wrapf("service_revenue[%d] amount %q is invalid", i, sr.Amount)
		}
	}
	return nil
}

// Validate checks that the recorded shares sum back to the qualifying total.
//
// This is the accounting identity the module exists to preserve:
//
//	total_qualifying == founder_share + validator_share + treasury_share
//
// It is checked at genesis and asserted on every block that routes revenue.
// A violation means the split arithmetic created or destroyed value.
func (t RevenueTotals) Validate() error {
	for _, c := range []struct {
		name  string
		coins sdk.Coins
	}{
		{"total_qualifying", t.TotalQualifying},
		{"founder_share", t.FounderShare},
		{"validator_share", t.ValidatorShare},
		{"treasury_share", t.TreasuryShare},
	} {
		if !c.coins.IsValid() || c.coins.IsAnyNegative() {
			return ErrInvalidFeeAmount.Wrapf("%s %q is invalid", c.name, c.coins)
		}
	}

	sum := t.FounderShare.Add(t.ValidatorShare...).Add(t.TreasuryShare...)
	if !sum.Equal(t.TotalQualifying) {
		return ErrSplitDoesNotBalance.Wrapf(
			"founder(%s) + validator(%s) + treasury(%s) = %s, but total_qualifying is %s",
			t.FounderShare, t.ValidatorShare, t.TreasuryShare, sum, t.TotalQualifying)
	}
	return nil
}

// ValidateServiceKind rejects the unspecified kind.
func ValidateServiceKind(k ServiceKind) error {
	if k == SERVICE_KIND_UNSPECIFIED {
		return ErrInvalidServiceKind.Wrap("service kind must be specified")
	}
	if _, ok := ServiceKind_name[int32(k)]; !ok {
		return ErrInvalidServiceKind.Wrapf("unknown service kind %d", k)
	}
	return nil
}
