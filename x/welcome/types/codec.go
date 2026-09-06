package types

import (
	"github.com/cosmos/cosmos-sdk/codec"
	"github.com/cosmos/cosmos-sdk/codec/legacy"
	codectypes "github.com/cosmos/cosmos-sdk/codec/types"
	sdk "github.com/cosmos/cosmos-sdk/types"
	"github.com/cosmos/cosmos-sdk/types/msgservice"
)

// RegisterLegacyAminoCodec registers x/welcome messages on the amino codec.
func RegisterLegacyAminoCodec(cdc *codec.LegacyAmino) {
	legacy.RegisterAminoMsg(cdc, &MsgClaimWelcome{}, "hashgram/MsgClaimWelcome")
	legacy.RegisterAminoMsg(cdc, &MsgUpdateParams{}, "hashgram/MsgUpdateWelcomeParams")
}

// RegisterInterfaces registers x/welcome message implementations.
func RegisterInterfaces(registry codectypes.InterfaceRegistry) {
	registry.RegisterImplementations((*sdk.Msg)(nil),
		&MsgClaimWelcome{},
		&MsgUpdateParams{},
	)
	msgservice.RegisterMsgServiceDesc(registry, &_Msg_serviceDesc)
}

var (
	_ sdk.Msg = (*MsgClaimWelcome)(nil)
	_ sdk.Msg = (*MsgUpdateParams)(nil)
)

// ValidateBasic performs stateless validation of MsgClaimWelcome.
func (m MsgClaimWelcome) ValidateBasic() error {
	if _, err := sdk.AccAddressFromBech32(m.Sender); err != nil {
		return ErrInvalidAttestation.Wrapf("sender %q: %v", m.Sender, err)
	}
	return m.Attestation.ValidateBasic()
}

// ValidateBasic performs stateless validation of MsgUpdateParams.
func (m MsgUpdateParams) ValidateBasic() error {
	if _, err := sdk.AccAddressFromBech32(m.Authority); err != nil {
		return ErrInvalidAuthority.Wrapf("%q: %v", m.Authority, err)
	}
	return m.Params.Validate()
}
