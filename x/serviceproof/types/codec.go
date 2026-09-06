package types

import (
	"github.com/cosmos/cosmos-sdk/codec"
	"github.com/cosmos/cosmos-sdk/codec/legacy"
	codectypes "github.com/cosmos/cosmos-sdk/codec/types"
	sdk "github.com/cosmos/cosmos-sdk/types"
	"github.com/cosmos/cosmos-sdk/types/msgservice"
)

// RegisterLegacyAminoCodec registers x/serviceproof messages on the amino
// codec.
func RegisterLegacyAminoCodec(cdc *codec.LegacyAmino) {
	legacy.RegisterAminoMsg(cdc, &MsgRegisterProvider{}, "hashgram/MsgRegisterProvider")
	legacy.RegisterAminoMsg(cdc, &MsgUpdateProvider{}, "hashgram/MsgUpdateProvider")
	legacy.RegisterAminoMsg(cdc, &MsgSubmitReceipts{}, "hashgram/MsgSubmitReceipts")
	legacy.RegisterAminoMsg(cdc, &MsgAnswerChallenge{}, "hashgram/MsgAnswerChallenge")
	legacy.RegisterAminoMsg(cdc, &MsgUnjail{}, "hashgram/MsgUnjailProvider")
	legacy.RegisterAminoMsg(cdc, &MsgBeginUnbonding{}, "hashgram/MsgBeginUnbonding")
	legacy.RegisterAminoMsg(cdc, &MsgWithdrawBond{}, "hashgram/MsgWithdrawBond")
	legacy.RegisterAminoMsg(cdc, &MsgAssignStorage{}, "hashgram/MsgAssignStorage")
	legacy.RegisterAminoMsg(cdc, &MsgReleaseStorage{}, "hashgram/MsgReleaseStorage")
	legacy.RegisterAminoMsg(cdc, &MsgUpdateParams{}, "hashgram/MsgUpdateSvcParams")
}

// RegisterInterfaces registers x/serviceproof message implementations.
func RegisterInterfaces(registry codectypes.InterfaceRegistry) {
	registry.RegisterImplementations((*sdk.Msg)(nil),
		&MsgRegisterProvider{},
		&MsgUpdateProvider{},
		&MsgSubmitReceipts{},
		&MsgAnswerChallenge{},
		&MsgUnjail{},
		&MsgBeginUnbonding{},
		&MsgWithdrawBond{},
		&MsgAssignStorage{},
		&MsgReleaseStorage{},
		&MsgUpdateParams{},
	)
	msgservice.RegisterMsgServiceDesc(registry, &_Msg_serviceDesc)
}

var (
	_ sdk.Msg = (*MsgRegisterProvider)(nil)
	_ sdk.Msg = (*MsgUpdateProvider)(nil)
	_ sdk.Msg = (*MsgSubmitReceipts)(nil)
	_ sdk.Msg = (*MsgAnswerChallenge)(nil)
	_ sdk.Msg = (*MsgUnjail)(nil)
	_ sdk.Msg = (*MsgBeginUnbonding)(nil)
	_ sdk.Msg = (*MsgWithdrawBond)(nil)
	_ sdk.Msg = (*MsgAssignStorage)(nil)
	_ sdk.Msg = (*MsgReleaseStorage)(nil)
	_ sdk.Msg = (*MsgUpdateParams)(nil)
)

// MaxReceiptsPerMessage bounds a receipt batch.
//
// Batching is necessary for a busy relay, but an unbounded batch is a way to
// force unbounded signature verification inside one transaction. Gas would
// eventually stop it; an explicit limit stops it with a comprehensible error.
const MaxReceiptsPerMessage = 256

// MaxMonikerLength bounds the human-readable provider label.
const MaxMonikerLength = 70

// ValidateBasic performs stateless validation.
func (m MsgRegisterProvider) ValidateBasic() error {
	if _, err := sdk.AccAddressFromBech32(m.Operator); err != nil {
		return ErrProviderNotFound.Wrapf("operator %q: %v", m.Operator, err)
	}
	if m.RewardAddress != "" {
		if _, err := sdk.AccAddressFromBech32(m.RewardAddress); err != nil {
			return ErrProviderNotFound.Wrapf("reward_address %q: %v", m.RewardAddress, err)
		}
	}
	if len(m.Roles) == 0 {
		return ErrInvalidRole.Wrap("a provider must offer at least one role")
	}
	seen := make(map[ServiceRole]bool, len(m.Roles))
	for _, r := range m.Roles {
		if err := ValidateRole(r); err != nil {
			return err
		}
		if seen[r] {
			return ErrInvalidRole.Wrapf("role %s is listed twice", r)
		}
		seen[r] = true
	}
	if !m.Bond.IsValid() || m.Bond.IsAnyNegative() {
		return ErrInsufficientBond.Wrapf("bond %q is invalid", m.Bond)
	}
	if _, err := PubKeyFromBytes(m.NodePubkey, m.NodeKeyType); err != nil {
		return err
	}
	if len(m.Moniker) > MaxMonikerLength {
		return ErrInvalidParams.Wrapf("moniker is %d bytes, maximum is %d", len(m.Moniker), MaxMonikerLength)
	}
	return nil
}

// ValidateBasic performs stateless validation.
func (m MsgUpdateProvider) ValidateBasic() error {
	if _, err := sdk.AccAddressFromBech32(m.Operator); err != nil {
		return ErrProviderNotFound.Wrapf("operator %q: %v", m.Operator, err)
	}
	if m.RewardAddress != "" {
		if _, err := sdk.AccAddressFromBech32(m.RewardAddress); err != nil {
			return ErrProviderNotFound.Wrapf("reward_address %q: %v", m.RewardAddress, err)
		}
	}
	for _, r := range m.Roles {
		if err := ValidateRole(r); err != nil {
			return err
		}
	}
	if !m.AdditionalBond.IsValid() || m.AdditionalBond.IsAnyNegative() {
		return ErrInsufficientBond.Wrapf("additional_bond %q is invalid", m.AdditionalBond)
	}
	if len(m.Moniker) > MaxMonikerLength {
		return ErrInvalidParams.Wrapf("moniker is %d bytes, maximum is %d", len(m.Moniker), MaxMonikerLength)
	}
	return nil
}

// ValidateBasic performs stateless validation.
func (m MsgSubmitReceipts) ValidateBasic() error {
	if _, err := sdk.AccAddressFromBech32(m.Submitter); err != nil {
		return ErrInvalidReceipt.Wrapf("submitter %q: %v", m.Submitter, err)
	}
	if len(m.Receipts) == 0 {
		return ErrInvalidReceipt.Wrap("no receipts submitted")
	}
	if len(m.Receipts) > MaxReceiptsPerMessage {
		return ErrInvalidReceipt.Wrapf(
			"%d receipts submitted, maximum is %d", len(m.Receipts), MaxReceiptsPerMessage)
	}
	for i, r := range m.Receipts {
		if err := r.ValidateBasic(); err != nil {
			return ErrInvalidReceipt.Wrapf("receipts[%d]: %v", i, err)
		}
	}
	return nil
}

// ValidateBasic performs stateless validation.
func (m MsgAnswerChallenge) ValidateBasic() error {
	if _, err := sdk.AccAddressFromBech32(m.Operator); err != nil {
		return ErrProviderNotFound.Wrapf("operator %q: %v", m.Operator, err)
	}
	if len(m.Response.ChunkHash) != ChunkHashSize {
		return ErrInvalidMerkleProof.Wrapf(
			"chunk_hash is %d bytes, expected %d", len(m.Response.ChunkHash), ChunkHashSize)
	}
	if len(m.Response.MerklePath) > MaxMerklePathLength {
		return ErrInvalidMerkleProof.Wrapf(
			"merkle_path has %d elements, maximum is %d", len(m.Response.MerklePath), MaxMerklePathLength)
	}
	for i, p := range m.Response.MerklePath {
		if len(p) != ChunkHashSize {
			return ErrInvalidMerkleProof.Wrapf(
				"merkle_path[%d] is %d bytes, expected %d", i, len(p), ChunkHashSize)
		}
	}
	if len(m.Response.NodeSignature) != 64 {
		return ErrInvalidSignature.Wrapf(
			"node_signature is %d bytes, expected 64", len(m.Response.NodeSignature))
	}
	return nil
}

// ValidateBasic performs stateless validation.
func (m MsgUnjail) ValidateBasic() error {
	_, err := sdk.AccAddressFromBech32(m.Operator)
	return err
}

// ValidateBasic performs stateless validation.
func (m MsgBeginUnbonding) ValidateBasic() error {
	_, err := sdk.AccAddressFromBech32(m.Operator)
	return err
}

// ValidateBasic performs stateless validation.
func (m MsgWithdrawBond) ValidateBasic() error {
	_, err := sdk.AccAddressFromBech32(m.Operator)
	return err
}

// ValidateBasic performs stateless validation.
func (m MsgAssignStorage) ValidateBasic() error {
	if _, err := sdk.AccAddressFromBech32(m.Assigner); err != nil {
		return ErrNotAnAssigner.Wrapf("assigner %q: %v", m.Assigner, err)
	}
	return m.Assignment.Validate()
}

// ValidateBasic performs stateless validation.
func (m MsgReleaseStorage) ValidateBasic() error {
	if _, err := sdk.AccAddressFromBech32(m.Assigner); err != nil {
		return ErrNotAnAssigner.Wrapf("assigner %q: %v", m.Assigner, err)
	}
	if _, err := sdk.AccAddressFromBech32(m.Provider); err != nil {
		return ErrProviderNotFound.Wrapf("provider %q: %v", m.Provider, err)
	}
	if len(m.BlobId) == 0 {
		return ErrInvalidAssignment.Wrap("blob_id must not be empty")
	}
	return nil
}

// ValidateBasic performs stateless validation.
func (m MsgUpdateParams) ValidateBasic() error {
	if _, err := sdk.AccAddressFromBech32(m.Authority); err != nil {
		return ErrInvalidAuthority.Wrapf("%q: %v", m.Authority, err)
	}
	for i, a := range m.Assigners {
		if _, err := sdk.AccAddressFromBech32(a); err != nil {
			return ErrNotAnAssigner.Wrapf("assigners[%d] %q: %v", i, a, err)
		}
	}
	return m.Params.Validate()
}
