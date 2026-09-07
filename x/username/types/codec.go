package types

import (
	"github.com/cosmos/cosmos-sdk/codec"
	"github.com/cosmos/cosmos-sdk/codec/legacy"
	codectypes "github.com/cosmos/cosmos-sdk/codec/types"
	sdk "github.com/cosmos/cosmos-sdk/types"
	"github.com/cosmos/cosmos-sdk/types/msgservice"
)

// RegisterLegacyAminoCodec registers x/username messages on the amino codec.
func RegisterLegacyAminoCodec(cdc *codec.LegacyAmino) {
	legacy.RegisterAminoMsg(cdc, &MsgRegister{}, "hashgram/MsgRegisterUsername")
	legacy.RegisterAminoMsg(cdc, &MsgRenew{}, "hashgram/MsgRenewUsername")
	legacy.RegisterAminoMsg(cdc, &MsgTransfer{}, "hashgram/MsgTransferUsername")
	legacy.RegisterAminoMsg(cdc, &MsgSetTransferable{}, "hashgram/MsgSetTransferable")
	legacy.RegisterAminoMsg(cdc, &MsgRelease{}, "hashgram/MsgReleaseUsername")
	legacy.RegisterAminoMsg(cdc, &MsgUpdateParams{}, "hashgram/MsgUpdateNameParams")
}

// RegisterInterfaces registers x/username message implementations.
func RegisterInterfaces(registry codectypes.InterfaceRegistry) {
	registry.RegisterImplementations((*sdk.Msg)(nil),
		&MsgRegister{},
		&MsgRenew{},
		&MsgTransfer{},
		&MsgSetTransferable{},
		&MsgRelease{},
		&MsgUpdateParams{},
	)
	msgservice.RegisterMsgServiceDesc(registry, &_Msg_serviceDesc)
}

var (
	_ sdk.Msg = (*MsgRegister)(nil)
	_ sdk.Msg = (*MsgRenew)(nil)
	_ sdk.Msg = (*MsgTransfer)(nil)
	_ sdk.Msg = (*MsgSetTransferable)(nil)
	_ sdk.Msg = (*MsgRelease)(nil)
	_ sdk.Msg = (*MsgUpdateParams)(nil)
)

// MaxRequestedNameBytes bounds the raw requested string before normalisation.
//
// Normalisation can expand input (NFKC decomposition of a single code point
// can produce several), so the raw length has to be bounded independently of
// the normalised length limit.
const MaxRequestedNameBytes = 256

func validateRequestedName(name string) error {
	if name == "" {
		return ErrInvalidName.Wrap("name must not be empty")
	}
	if len(name) > MaxRequestedNameBytes {
		return ErrInvalidName.Wrapf(
			"requested name is %d bytes, maximum is %d", len(name), MaxRequestedNameBytes)
	}
	return nil
}

// ValidateBasic performs stateless validation.
func (m MsgRegister) ValidateBasic() error {
	if _, err := sdk.AccAddressFromBech32(m.Owner); err != nil {
		return ErrInvalidName.Wrapf("owner %q: %v", m.Owner, err)
	}
	return validateRequestedName(m.Name)
}

// ValidateBasic performs stateless validation.
func (m MsgRenew) ValidateBasic() error {
	if _, err := sdk.AccAddressFromBech32(m.Owner); err != nil {
		return ErrInvalidName.Wrapf("owner %q: %v", m.Owner, err)
	}
	return validateRequestedName(m.Name)
}

// ValidateBasic performs stateless validation.
func (m MsgTransfer) ValidateBasic() error {
	if _, err := sdk.AccAddressFromBech32(m.Owner); err != nil {
		return ErrInvalidName.Wrapf("owner %q: %v", m.Owner, err)
	}
	if _, err := sdk.AccAddressFromBech32(m.NewOwner); err != nil {
		return ErrInvalidName.Wrapf("new_owner %q: %v", m.NewOwner, err)
	}
	if m.Owner == m.NewOwner {
		return ErrInvalidName.Wrap("new_owner is the current owner")
	}
	return validateRequestedName(m.Name)
}

// ValidateBasic performs stateless validation.
func (m MsgSetTransferable) ValidateBasic() error {
	if _, err := sdk.AccAddressFromBech32(m.Owner); err != nil {
		return ErrInvalidName.Wrapf("owner %q: %v", m.Owner, err)
	}
	return validateRequestedName(m.Name)
}

// ValidateBasic performs stateless validation.
func (m MsgRelease) ValidateBasic() error {
	if _, err := sdk.AccAddressFromBech32(m.Owner); err != nil {
		return ErrInvalidName.Wrapf("owner %q: %v", m.Owner, err)
	}
	return validateRequestedName(m.Name)
}

// ValidateBasic performs stateless validation.
func (m MsgUpdateParams) ValidateBasic() error {
	if _, err := sdk.AccAddressFromBech32(m.Authority); err != nil {
		return ErrInvalidAuthority.Wrapf("%q: %v", m.Authority, err)
	}
	return m.Params.Validate()
}
