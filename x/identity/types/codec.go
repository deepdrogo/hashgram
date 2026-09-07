package types

import (
	"github.com/cosmos/cosmos-sdk/codec"
	"github.com/cosmos/cosmos-sdk/codec/legacy"
	codectypes "github.com/cosmos/cosmos-sdk/codec/types"
	sdk "github.com/cosmos/cosmos-sdk/types"
	"github.com/cosmos/cosmos-sdk/types/msgservice"
)

// RegisterLegacyAminoCodec registers x/identity messages on the amino codec.
func RegisterLegacyAminoCodec(cdc *codec.LegacyAmino) {
	legacy.RegisterAminoMsg(cdc, &MsgCreateIdentity{}, "hashgram/MsgCreateIdentity")
	legacy.RegisterAminoMsg(cdc, &MsgAddDevice{}, "hashgram/MsgAddDevice")
	legacy.RegisterAminoMsg(cdc, &MsgRevokeDevice{}, "hashgram/MsgRevokeDevice")
	legacy.RegisterAminoMsg(cdc, &MsgRotateRootKey{}, "hashgram/MsgRotateRootKey")
	legacy.RegisterAminoMsg(cdc, &MsgSetRecoveryConfig{}, "hashgram/MsgSetRecovery")
	legacy.RegisterAminoMsg(cdc, &MsgInitiateRecovery{}, "hashgram/MsgInitiateRecovery")
	legacy.RegisterAminoMsg(cdc, &MsgApproveRecovery{}, "hashgram/MsgApproveRecovery")
	legacy.RegisterAminoMsg(cdc, &MsgCancelRecovery{}, "hashgram/MsgCancelRecovery")
	legacy.RegisterAminoMsg(cdc, &MsgExecuteRecovery{}, "hashgram/MsgExecuteRecovery")
	legacy.RegisterAminoMsg(cdc, &MsgRevokeIdentity{}, "hashgram/MsgRevokeIdentity")
}

// RegisterInterfaces registers x/identity message implementations.
func RegisterInterfaces(registry codectypes.InterfaceRegistry) {
	registry.RegisterImplementations((*sdk.Msg)(nil),
		&MsgCreateIdentity{},
		&MsgAddDevice{},
		&MsgRevokeDevice{},
		&MsgRotateRootKey{},
		&MsgSetRecoveryConfig{},
		&MsgInitiateRecovery{},
		&MsgApproveRecovery{},
		&MsgCancelRecovery{},
		&MsgExecuteRecovery{},
		&MsgRevokeIdentity{},
	)
	msgservice.RegisterMsgServiceDesc(registry, &_Msg_serviceDesc)
}

var (
	_ sdk.Msg = (*MsgCreateIdentity)(nil)
	_ sdk.Msg = (*MsgAddDevice)(nil)
	_ sdk.Msg = (*MsgRevokeDevice)(nil)
	_ sdk.Msg = (*MsgRotateRootKey)(nil)
	_ sdk.Msg = (*MsgSetRecoveryConfig)(nil)
	_ sdk.Msg = (*MsgInitiateRecovery)(nil)
	_ sdk.Msg = (*MsgApproveRecovery)(nil)
	_ sdk.Msg = (*MsgCancelRecovery)(nil)
	_ sdk.Msg = (*MsgExecuteRecovery)(nil)
	_ sdk.Msg = (*MsgRevokeIdentity)(nil)
)

// ValidateBasic performs stateless validation.
func (m MsgCreateIdentity) ValidateBasic() error {
	if _, err := sdk.AccAddressFromBech32(m.Address); err != nil {
		return ErrIdentityNotFound.Wrapf("address %q: %v", m.Address, err)
	}
	if _, err := PubKeyFromBytes(m.RootPubkey, m.RootKeyType); err != nil {
		return err
	}
	// The recovery configuration is validated statefully against params in
	// the keeper; here only the shape is checked.
	if m.Recovery.Threshold > 0 && len(m.Recovery.Guardians) == 0 {
		return ErrInvalidRecovery.Wrap("threshold is set but no guardians are configured")
	}
	return nil
}

// ValidateBasic performs stateless validation.
func (m MsgAddDevice) ValidateBasic() error {
	if _, err := sdk.AccAddressFromBech32(m.Address); err != nil {
		return ErrIdentityNotFound.Wrapf("address %q: %v", m.Address, err)
	}
	// Length bounds come from params, so pass 0 to skip them here.
	return m.Certificate.ValidateBasic(0)
}

// ValidateBasic performs stateless validation.
func (m MsgRevokeDevice) ValidateBasic() error {
	if _, err := sdk.AccAddressFromBech32(m.Address); err != nil {
		return ErrIdentityNotFound.Wrapf("address %q: %v", m.Address, err)
	}
	return ValidateDeviceID(m.DeviceId, 0)
}

// ValidateBasic performs stateless validation.
func (m MsgRotateRootKey) ValidateBasic() error {
	if _, err := sdk.AccAddressFromBech32(m.Address); err != nil {
		return ErrIdentityNotFound.Wrapf("address %q: %v", m.Address, err)
	}
	if _, err := PubKeyFromBytes(m.NewRootPubkey, m.NewRootKeyType); err != nil {
		return err
	}
	if len(m.SignatureByOldRoot) != 64 {
		return ErrInvalidRotationSignature.Wrapf(
			"signature is %d bytes, expected 64", len(m.SignatureByOldRoot))
	}
	return nil
}

// ValidateBasic performs stateless validation.
func (m MsgSetRecoveryConfig) ValidateBasic() error {
	if _, err := sdk.AccAddressFromBech32(m.Address); err != nil {
		return ErrIdentityNotFound.Wrapf("address %q: %v", m.Address, err)
	}
	return nil
}

// ValidateBasic performs stateless validation.
func (m MsgInitiateRecovery) ValidateBasic() error {
	for name, addr := range map[string]string{
		"initiator":    m.Initiator,
		"root_address": m.RootAddress,
		"new_address":  m.NewAddress,
	} {
		if _, err := sdk.AccAddressFromBech32(addr); err != nil {
			return ErrInvalidRecovery.Wrapf("%s %q: %v", name, addr, err)
		}
	}
	_, err := PubKeyFromBytes(m.NewRootPubkey, m.NewRootKeyType)
	return err
}

// ValidateBasic performs stateless validation.
func (m MsgApproveRecovery) ValidateBasic() error {
	if _, err := sdk.AccAddressFromBech32(m.Guardian); err != nil {
		return ErrNotAGuardian.Wrapf("guardian %q: %v", m.Guardian, err)
	}
	_, err := sdk.AccAddressFromBech32(m.RootAddress)
	return err
}

// ValidateBasic performs stateless validation.
func (m MsgCancelRecovery) ValidateBasic() error {
	_, err := sdk.AccAddressFromBech32(m.Address)
	return err
}

// ValidateBasic performs stateless validation.
func (m MsgExecuteRecovery) ValidateBasic() error {
	if _, err := sdk.AccAddressFromBech32(m.Executor); err != nil {
		return ErrInvalidRecovery.Wrapf("executor %q: %v", m.Executor, err)
	}
	_, err := sdk.AccAddressFromBech32(m.RootAddress)
	return err
}

// ValidateBasic performs stateless validation.
func (m MsgRevokeIdentity) ValidateBasic() error {
	_, err := sdk.AccAddressFromBech32(m.Address)
	return err
}
