package types

import (
	sdk "github.com/cosmos/cosmos-sdk/types"
)

var (
	_ sdk.Msg = (*MsgUpdateParams)(nil)
	_ sdk.Msg = (*MsgClaimFounderRevenue)(nil)
)

// ValidateBasic performs stateless validation of MsgUpdateParams.
func (m MsgUpdateParams) ValidateBasic() error {
	if _, err := sdk.AccAddressFromBech32(m.Authority); err != nil {
		return ErrInvalidAuthority.Wrapf("%q: %v", m.Authority, err)
	}
	return m.Params.Validate()
}

// ValidateBasic performs stateless validation of MsgClaimFounderRevenue.
func (m MsgClaimFounderRevenue) ValidateBasic() error {
	if _, err := sdk.AccAddressFromBech32(m.Sender); err != nil {
		return ErrInvalidBeneficiary.Wrapf("sender %q: %v", m.Sender, err)
	}
	return nil
}
