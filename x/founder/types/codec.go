package types

import (
	"github.com/cosmos/cosmos-sdk/codec"
	"github.com/cosmos/cosmos-sdk/codec/legacy"
	codectypes "github.com/cosmos/cosmos-sdk/codec/types"
	sdk "github.com/cosmos/cosmos-sdk/types"
	"github.com/cosmos/cosmos-sdk/types/msgservice"
)

// RegisterLegacyAminoCodec registers x/founder messages for amino signing,
// which hardware wallets still use.
//
// This matters for the Founder specifically: the beneficiary is expected to be
// a hardware wallet or multisig, and amino ledger signing must work for it.
func RegisterLegacyAminoCodec(cdc *codec.LegacyAmino) {
	legacy.RegisterAminoMsg(cdc, &MsgUpdateParams{}, "hashgram/MsgUpdateFounderParams")
	legacy.RegisterAminoMsg(cdc, &MsgClaimFounderRevenue{}, "hashgram/MsgClaimFounderRevenue")
}

// RegisterInterfaces registers x/founder message implementations.
func RegisterInterfaces(registry codectypes.InterfaceRegistry) {
	registry.RegisterImplementations((*sdk.Msg)(nil),
		&MsgUpdateParams{},
		&MsgClaimFounderRevenue{},
	)
	msgservice.RegisterMsgServiceDesc(registry, &_Msg_serviceDesc)
}
