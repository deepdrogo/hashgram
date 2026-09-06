package params

import (
	"sync"

	sdk "github.com/cosmos/cosmos-sdk/types"
)

var setSDKConfigOnce sync.Once

// SetSDKConfig installs the Hashgram bech32 prefixes and HD coin type into the
// process-global Cosmos SDK config and seals it.
//
// This must run before any address is parsed or rendered, which in practice
// means before cobra command construction in every binary. It is idempotent
// and safe to call from tests: the SDK config is sealed on first call, and
// calling Seal twice panics, hence the sync.Once.
func SetSDKConfig() {
	setSDKConfigOnce.Do(func() {
		cfg := sdk.GetConfig()
		cfg.SetBech32PrefixForAccount(Bech32PrefixAccAddr, Bech32PrefixAccPub)
		cfg.SetBech32PrefixForValidator(Bech32PrefixValAddr, Bech32PrefixValPub)
		cfg.SetBech32PrefixForConsensusNode(Bech32PrefixConsAddr, Bech32PrefixConsPub)
		cfg.SetCoinType(Bip44CoinType)
		cfg.Seal()
	})
}
