package keeper

import (
	sdk "github.com/cosmos/cosmos-sdk/types"

	"github.com/hashgram/hashgram/x/network/types"
)

// InitGenesis writes the network identity and fork-isolation policy.
//
// Three checks run here, all of which abort chain start rather than logging a
// warning:
//
//  1. The genesis state is internally valid, and a genesis claiming the
//     Mainnet network_id carries every other Mainnet identifier.
//  2. The declared chain_id matches the chain-id CometBFT was started with.
//  3. The identity has not already been written.
//
// Aborting is the right behaviour: a node that starts on the wrong network is
// worse than a node that does not start.
func (k Keeper) InitGenesis(ctx sdk.Context, gs types.GenesisState) error {
	if err := gs.Validate(); err != nil {
		return err
	}
	if err := k.AssertChainID(ctx, gs.Info.ChainId); err != nil {
		return err
	}
	if err := k.RequireUnset(ctx); err != nil {
		return err
	}

	if err := k.SetNetworkInfo(ctx, gs.Info); err != nil {
		return err
	}
	if err := k.SetForkIsolation(ctx, gs.ForkIsolation); err != nil {
		return err
	}

	// If this node was configured with a genesis hash, say so at start-up.
	// If it was not, say that too: an operator should be able to tell from
	// the logs whether their node is pinned to a genesis or trusting whatever
	// file happens to be on disk.
	if h := k.PinnedGenesisHash(); h != "" {
		k.Logger().Info("network identity established",
			"network", gs.Info.NetworkName,
			"network_id", gs.Info.NetworkId,
			"chain_id", gs.Info.ChainId,
			"magic", string(gs.Info.NetworkMagic),
			"protocol_major", gs.Info.ProtocolMajorVersion,
			"pinned_genesis_hash", h,
		)
	} else {
		k.Logger().Info("network identity established",
			"network", gs.Info.NetworkName,
			"network_id", gs.Info.NetworkId,
			"chain_id", gs.Info.ChainId,
			"magic", string(gs.Info.NetworkMagic),
			"protocol_major", gs.Info.ProtocolMajorVersion,
			"pinned_genesis_hash", "NOT SET - this node will refuse hashgram P2P peers",
		)
	}

	return nil
}

// ExportGenesis reads the network identity back out.
func (k Keeper) ExportGenesis(ctx sdk.Context) (*types.GenesisState, error) {
	info, err := k.NetworkInfo(ctx)
	if err != nil {
		return nil, err
	}
	policy, err := k.ForkIsolation(ctx)
	if err != nil {
		return nil, err
	}
	return &types.GenesisState{
		Info:          info,
		ForkIsolation: policy,
	}, nil
}
