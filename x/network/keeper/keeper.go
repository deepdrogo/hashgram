package keeper

import (
	"context"
	"errors"
	"fmt"

	"cosmossdk.io/collections"
	storetypes "cosmossdk.io/core/store"
	"cosmossdk.io/log"

	"github.com/cosmos/cosmos-sdk/codec"
	sdk "github.com/cosmos/cosmos-sdk/types"

	hgparams "github.com/hashgram/hashgram/app/params"
	"github.com/hashgram/hashgram/x/network/types"
)

// Keeper owns the immutable network identity.
//
// There is no authority field and no setter reachable from a message handler.
// The identity is written by InitGenesis and read thereafter. Changing it
// requires a store migration in a coordinated binary upgrade, which is the
// only mechanism in Hashgram that can change the state machine at all.
type Keeper struct {
	cdc    codec.BinaryCodec
	logger log.Logger

	Schema collections.Schema

	// info is the NetworkInfo singleton.
	info collections.Item[types.NetworkInfo]

	// forkIsolation is the ForkIsolationPolicy singleton.
	forkIsolation collections.Item[types.ForkIsolationPolicy]

	// genesisHash is supplied by node configuration, not by chain state: the
	// hash of the genesis file cannot be a field inside that file. It is
	// process-local and never written to the store, so two nodes disagreeing
	// about it cannot cause a consensus fault; they simply refuse to peer.
	genesisHash string
}

// NewKeeper constructs the x/network keeper.
func NewKeeper(
	cdc codec.BinaryCodec,
	storeService storetypes.KVStoreService,
	logger log.Logger,
) Keeper {
	sb := collections.NewSchemaBuilder(storeService)

	k := Keeper{
		cdc:    cdc,
		logger: logger.With("module", "x/"+types.ModuleName),
		info: collections.NewItem(
			sb, types.NetworkInfoKey, "network_info",
			codec.CollValue[types.NetworkInfo](cdc),
		),
		forkIsolation: collections.NewItem(
			sb, types.ForkIsolationKey, "fork_isolation",
			codec.CollValue[types.ForkIsolationPolicy](cdc),
		),
	}

	schema, err := sb.Build()
	if err != nil {
		panic(err)
	}
	k.Schema = schema
	return k
}

// SetPinnedGenesisHash records the genesis hash this node was configured with.
//
// Called once during application construction from app.toml. It affects only
// this process's peer-acceptance decisions and node-info output.
func (k *Keeper) SetPinnedGenesisHash(hash string) error {
	if hash == "" {
		k.genesisHash = ""
		return nil
	}
	if err := hgparams.ValidateGenesisHash(hash); err != nil {
		return fmt.Errorf("configured genesis-hash is malformed: %w", err)
	}
	k.genesisHash = hash
	return nil
}

// PinnedGenesisHash returns the configured genesis hash, or "" if unset.
func (k Keeper) PinnedGenesisHash() string { return k.genesisHash }

// Logger returns the module logger.
func (k Keeper) Logger() log.Logger { return k.logger }

// SetNetworkInfo writes the network identity.
//
// Unexported-by-convention: only InitGenesis and store migrations call this.
// It is exported for the migration package but guarded by RequireUnset.
func (k Keeper) SetNetworkInfo(ctx context.Context, info types.NetworkInfo) error {
	if err := info.Validate(); err != nil {
		return err
	}
	return k.info.Set(ctx, info)
}

// NetworkInfo reads the network identity.
func (k Keeper) NetworkInfo(ctx context.Context) (types.NetworkInfo, error) {
	info, err := k.info.Get(ctx)
	if err != nil {
		if errorsIsNotFound(err) {
			return types.NetworkInfo{}, types.ErrNetworkInfoNotSet
		}
		return types.NetworkInfo{}, err
	}
	return info, nil
}

// RequireUnset returns an error if the network identity has already been
// written. InitGenesis uses it so that a second InitGenesis on a populated
// store fails loudly instead of silently rewriting the network's identity.
func (k Keeper) RequireUnset(ctx context.Context) error {
	has, err := k.info.Has(ctx)
	if err != nil {
		return err
	}
	if has {
		return types.ErrNetworkInfoImmutable
	}
	return nil
}

// SetForkIsolation writes the fork-isolation policy.
func (k Keeper) SetForkIsolation(ctx context.Context, p types.ForkIsolationPolicy) error {
	return k.forkIsolation.Set(ctx, p)
}

// ForkIsolation reads the fork-isolation policy. A missing policy is treated
// as the strictest policy, so a state-corruption or migration gap fails
// closed rather than open.
func (k Keeper) ForkIsolation(ctx context.Context) (types.ForkIsolationPolicy, error) {
	p, err := k.forkIsolation.Get(ctx)
	if err != nil {
		if errorsIsNotFound(err) {
			return types.StrictForkIsolation(), nil
		}
		return types.ForkIsolationPolicy{}, err
	}
	return p, nil
}

// Identity returns the full network identity, combining on-chain info with
// this node's configured genesis hash.
func (k Keeper) Identity(ctx context.Context) (hgparams.NetworkIdentity, error) {
	info, err := k.NetworkInfo(ctx)
	if err != nil {
		return hgparams.NetworkIdentity{}, err
	}
	return info.Identity(k.genesisHash), nil
}

// SigningDomain returns the domain-separation string for a purpose.
func (k Keeper) SigningDomain(ctx context.Context, purpose hgparams.SigningPurpose) (string, error) {
	id, err := k.Identity(ctx)
	if err != nil {
		return "", err
	}
	return id.SigningDomain(purpose), nil
}

// SigningDigest returns the digest that must be signed for a purpose and
// payload on this network.
//
// Every module that verifies a signature over off-chain data (service
// receipts, device certificates, eligibility attestations, content
// attestations) obtains its digest here, so that domain separation is applied
// in exactly one place.
func (k Keeper) SigningDigest(ctx context.Context, purpose hgparams.SigningPurpose, payload []byte) ([32]byte, error) {
	id, err := k.Identity(ctx)
	if err != nil {
		return [32]byte{}, err
	}
	return id.SigningDigest(purpose, payload), nil
}

// AssertChainID checks that the on-chain chain_id matches the chain-id
// CometBFT is actually running.
//
// This is what turns "somebody gave me a genesis.json" into a checkable
// claim: a genesis whose x/network chain_id has been edited to say
// "hashgram-1" while the CometBFT chain-id says something else will fail here
// at InitChain, before the chain produces a block.
func (k Keeper) AssertChainID(ctx sdk.Context, declared string) error {
	if actual := ctx.ChainID(); actual != declared {
		return types.ErrChainIDMismatch.Wrapf(
			"genesis declares chain_id %q but CometBFT chain-id is %q", declared, actual)
	}
	return nil
}

func errorsIsNotFound(err error) bool {
	return errors.Is(err, collections.ErrNotFound)
}
