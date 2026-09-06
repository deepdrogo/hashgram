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

	"github.com/hashgram/hashgram/x/feerouter/types"
)

// BankKeeper is the subset of x/bank that x/feerouter needs.
//
// There is no MintCoins here. x/feerouter only ever moves coins that already
// exist between module accounts.
type BankKeeper interface {
	GetAllBalances(ctx context.Context, addr sdk.AccAddress) sdk.Coins
	SendCoinsFromAccountToModule(ctx context.Context, sender sdk.AccAddress, recipientModule string, amt sdk.Coins) error
	SendCoinsFromModuleToModule(ctx context.Context, senderModule, recipientModule string, amt sdk.Coins) error
}

// AccountKeeper is the subset of x/auth that x/feerouter needs.
type AccountKeeper interface {
	GetModuleAddress(name string) sdk.AccAddress
}

// FounderKeeper is the subset of x/founder that x/feerouter needs.
type FounderKeeper interface {
	// FounderCut returns the Founder share of an amount, or zero coins if no
	// beneficiary is configured. Returning zero rather than an error means an
	// unconfigured chain routes everything to validators instead of halting.
	FounderCut(ctx context.Context, amount sdk.Coins) (sdk.Coins, error)

	// Accrue moves an already-computed Founder share out of fromModule and
	// into the founder module account.
	Accrue(ctx context.Context, fromModule string, amount sdk.Coins, source string) error
}

// Keeper routes qualifying protocol fee revenue.
type Keeper struct {
	cdc    codec.BinaryCodec
	logger log.Logger

	bankKeeper    BankKeeper
	accountKeeper AccountKeeper
	founderKeeper FounderKeeper

	// feeCollectorName is the module account transaction gas fees land in.
	// x/distribution empties it every block; x/feerouter takes the Founder
	// share out of it first.
	feeCollectorName string

	// authority is the governance module account. Nothing else can change
	// this module's params.
	authority string

	Schema collections.Schema

	params         collections.Item[types.Params]
	totals         collections.Item[types.RevenueTotals]
	serviceRevenue collections.Map[int32, types.ServiceRevenue]

	revenuePool sdk.AccAddress
}

// NewKeeper constructs the x/feerouter keeper.
func NewKeeper(
	cdc codec.BinaryCodec,
	storeService storetypes.KVStoreService,
	ak AccountKeeper,
	bk BankKeeper,
	fk FounderKeeper,
	feeCollectorName string,
	authority string,
	logger log.Logger,
) Keeper {
	if _, err := sdk.AccAddressFromBech32(authority); err != nil {
		panic(fmt.Sprintf("x/feerouter: invalid authority %q: %v", authority, err))
	}
	pool := ak.GetModuleAddress(types.ModuleName)
	if pool == nil {
		panic("x/feerouter: the feerouter module account has not been registered in maccPerms")
	}
	if ak.GetModuleAddress(feeCollectorName) == nil {
		panic("x/feerouter: the fee collector module account has not been registered in maccPerms")
	}

	sb := collections.NewSchemaBuilder(storeService)

	k := Keeper{
		cdc:              cdc,
		logger:           logger.With("module", "x/"+types.ModuleName),
		bankKeeper:       bk,
		accountKeeper:    ak,
		founderKeeper:    fk,
		feeCollectorName: feeCollectorName,
		authority:        authority,
		revenuePool:      pool,
		params: collections.NewItem(
			sb, types.ParamsKey, "params",
			codec.CollValue[types.Params](cdc),
		),
		totals: collections.NewItem(
			sb, types.TotalsKey, "totals",
			codec.CollValue[types.RevenueTotals](cdc),
		),
		serviceRevenue: collections.NewMap(
			sb, types.ServiceRevenueKey, "service_revenue",
			collections.Int32Key, codec.CollValue[types.ServiceRevenue](cdc),
		),
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

// RevenuePool returns the module account that explicit service fees are paid
// into before being split.
func (k Keeper) RevenuePool() sdk.AccAddress { return k.revenuePool }

// PendingPool returns what is currently sitting in the revenue pool awaiting
// the next split.
func (k Keeper) PendingPool(ctx context.Context) sdk.Coins {
	return k.bankKeeper.GetAllBalances(ctx, k.revenuePool)
}

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

// GetTotals reads the cumulative revenue split.
func (k Keeper) GetTotals(ctx context.Context) (types.RevenueTotals, error) {
	t, err := k.totals.Get(ctx)
	if err != nil {
		if errors.Is(err, collections.ErrNotFound) {
			return types.EmptyTotals(), nil
		}
		return types.RevenueTotals{}, err
	}
	return t, nil
}

// SetTotals writes the cumulative revenue split.
func (k Keeper) SetTotals(ctx context.Context, t types.RevenueTotals) error {
	return k.totals.Set(ctx, t)
}

// GetServiceRevenue reads cumulative revenue for one service kind.
func (k Keeper) GetServiceRevenue(ctx context.Context, kind types.ServiceKind) (sdk.Coins, error) {
	sr, err := k.serviceRevenue.Get(ctx, int32(kind))
	if err != nil {
		if errors.Is(err, collections.ErrNotFound) {
			return sdk.NewCoins(), nil
		}
		return nil, err
	}
	return sr.Amount, nil
}

// AllServiceRevenue reads cumulative revenue for every service kind, ordered
// by kind so the output is deterministic.
func (k Keeper) AllServiceRevenue(ctx context.Context) ([]types.ServiceRevenue, error) {
	var out []types.ServiceRevenue
	err := k.serviceRevenue.Walk(ctx, nil, func(_ int32, v types.ServiceRevenue) (bool, error) {
		out = append(out, v)
		return false, nil
	})
	if err != nil {
		return nil, err
	}
	return out, nil
}

func (k Keeper) addServiceRevenue(ctx context.Context, kind types.ServiceKind, amount sdk.Coins) error {
	if amount.IsZero() {
		return nil
	}
	current, err := k.GetServiceRevenue(ctx, kind)
	if err != nil {
		return err
	}
	return k.serviceRevenue.Set(ctx, int32(kind), types.ServiceRevenue{
		Kind:   kind,
		Amount: current.Add(amount...),
	})
}
