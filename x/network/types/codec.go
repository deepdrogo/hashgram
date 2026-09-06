package types

import (
	"github.com/cosmos/cosmos-sdk/codec"
	codectypes "github.com/cosmos/cosmos-sdk/codec/types"
)

// RegisterLegacyAminoCodec registers x/network types on the amino codec.
//
// x/network has no messages: the network identity is written once at genesis
// and there is no transaction that can change it. There is therefore nothing
// to register, and this is a no-op rather than an oversight.
func RegisterLegacyAminoCodec(_ *codec.LegacyAmino) {}

// RegisterInterfaces registers x/network implementations on the interface
// registry. As above, x/network exposes no messages.
func RegisterInterfaces(_ codectypes.InterfaceRegistry) {}
