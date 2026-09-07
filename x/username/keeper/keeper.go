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

	feeroutertypes "github.com/hashgram/hashgram/x/feerouter/types"
	"github.com/hashgram/hashgram/x/username/types"
)

// FeeRouterKeeper is the subset of x/feerouter that x/username needs.
//
// Registration fees go through the router rather than straight to the fee
// collector, so that username revenue is recorded as qualifying protocol
// revenue and appears in the per-service breakdown. Sending it directly would
// make it invisible in the revenue accounting.
type FeeRouterKeeper interface {
	CollectServiceFee(ctx context.Context, payer sdk.AccAddress, amount sdk.Coins, kind feeroutertypes.ServiceKind) error
}

// Keeper manages the @username namespace.
type Keeper struct {
	cdc       codec.BinaryCodec
	logger    log.Logger
	feeRouter FeeRouterKeeper
	authority string

	Schema collections.Schema

	params        collections.Item[types.Params]
	registrations collections.Map[string, types.Registration]

	// skeletons maps a confusable-folded form to the name that claimed it,
	// so the collision check is an index lookup rather than a scan over every
	// registration. Without it, registration cost would grow with the size
	// of the namespace.
	skeletons collections.Map[string, string]

	// ownerIndex maps (owner, name) for reverse lookup.
	ownerIndex collections.KeySet[collections.Pair[string, string]]
}

// NewKeeper constructs the x/username keeper.
func NewKeeper(
	cdc codec.BinaryCodec,
	storeService storetypes.KVStoreService,
	fr FeeRouterKeeper,
	authority string,
	logger log.Logger,
) Keeper {
	if _, err := sdk.AccAddressFromBech32(authority); err != nil {
		panic(fmt.Sprintf("x/username: invalid authority %q: %v", authority, err))
	}

	sb := collections.NewSchemaBuilder(storeService)

	k := Keeper{
		cdc:       cdc,
		logger:    logger.With("module", "x/"+types.ModuleName),
		feeRouter: fr,
		authority: authority,
		params: collections.NewItem(sb, types.ParamsKey, "params",
			codec.CollValue[types.Params](cdc)),
		registrations: collections.NewMap(sb, types.RegistrationKey, "registrations",
			collections.StringKey, codec.CollValue[types.Registration](cdc)),
		skeletons: collections.NewMap(sb, types.SkeletonKey, "skeletons",
			collections.StringKey, collections.StringValue),
		ownerIndex: collections.NewKeySet(sb, types.OwnerIndexKey, "owner_index",
			collections.PairKeyCodec(collections.StringKey, collections.StringKey)),
	}

	schema, err := sb.Build()
	if err != nil {
		panic(err)
	}
	k.Schema = schema
	return k
}

// Authority returns the governance module address.
func (k Keeper) Authority() string { return k.authority }

// Logger returns the module logger.
func (k Keeper) Logger() log.Logger { return k.logger }

// SetParams writes the parameter set.
func (k Keeper) SetParams(ctx context.Context, p types.Params) error {
	if err := p.Validate(); err != nil {
		return err
	}
	return k.params.Set(ctx, p)
}

// GetParams reads the parameter set.
func (k Keeper) GetParams(ctx context.Context) (types.Params, error) {
	p, err := k.params.Get(ctx)
	if err != nil {
		if errors.Is(err, collections.ErrNotFound) {
			return types.Params{}, types.ErrParamsNotSet
		}
		return types.Params{}, err
	}
	return p, nil
}

// UpdateParams applies a governance-approved parameter change.
func (k Keeper) UpdateParams(ctx context.Context, authority string, next types.Params) error {
	if authority != k.authority {
		return types.ErrInvalidAuthority.Wrapf("expected %s, got %s", k.authority, authority)
	}
	return k.SetParams(ctx, next)
}

// GetRegistration reads a registration by normalised name.
func (k Keeper) GetRegistration(ctx context.Context, name string) (types.Registration, bool, error) {
	r, err := k.registrations.Get(ctx, name)
	if err != nil {
		if errors.Is(err, collections.ErrNotFound) {
			return types.Registration{}, false, nil
		}
		return types.Registration{}, false, err
	}
	return r, true, nil
}

// setRegistration writes a registration and both indexes.
func (k Keeper) setRegistration(ctx context.Context, r types.Registration) error {
	if err := k.registrations.Set(ctx, r.Name, r); err != nil {
		return err
	}
	if err := k.skeletons.Set(ctx, r.Skeleton, r.Name); err != nil {
		return err
	}
	return k.ownerIndex.Set(ctx, collections.Join(r.Owner, r.Name))
}

// SetRegistration writes a registration, used by genesis.
func (k Keeper) SetRegistration(ctx context.Context, r types.Registration) error {
	return k.setRegistration(ctx, r)
}

// removeRegistration deletes a registration and both indexes.
func (k Keeper) removeRegistration(ctx context.Context, r types.Registration) error {
	if err := k.registrations.Remove(ctx, r.Name); err != nil {
		return err
	}
	if err := k.skeletons.Remove(ctx, r.Skeleton); err != nil {
		return err
	}
	return k.ownerIndex.Remove(ctx, collections.Join(r.Owner, r.Name))
}

// SkeletonHolder returns the name that claimed a skeleton, if any.
func (k Keeper) SkeletonHolder(ctx context.Context, skeleton string) (string, bool, error) {
	name, err := k.skeletons.Get(ctx, skeleton)
	if err != nil {
		if errors.Is(err, collections.ErrNotFound) {
			return "", false, nil
		}
		return "", false, err
	}
	return name, true, nil
}

// OwnedNames returns every registration held by an address.
func (k Keeper) OwnedNames(ctx context.Context, owner string) ([]types.Registration, error) {
	var out []types.Registration
	rng := collections.NewPrefixedPairRange[string, string](owner)
	err := k.ownerIndex.Walk(ctx, rng, func(key collections.Pair[string, string]) (bool, error) {
		r, found, err := k.GetRegistration(ctx, key.K2())
		if err != nil {
			return true, err
		}
		if found {
			out = append(out, r)
		}
		return false, nil
	})
	return out, err
}

// IterateRegistrations walks every registration in name order.
func (k Keeper) IterateRegistrations(ctx context.Context, fn func(types.Registration) (stop bool, err error)) error {
	return k.registrations.Walk(ctx, nil, func(_ string, r types.Registration) (bool, error) {
		return fn(r)
	})
}

// Registrations exposes the underlying map for paginated queries.
func (k Keeper) Registrations() collections.Map[string, types.Registration] {
	return k.registrations
}
