// Package keeper implements x/serviceproof, the Proof of Useful Service
// reward system.
//
// Nothing is rewarded for merely existing. Relay and call work is paid
// against receipts the served client signed; storage is paid against bytes
// the network assigned and challenges the provider answered. Rewards come
// from a finite genesis reserve that is never topped up.
package keeper

import (
	"context"
	"errors"
	"fmt"

	"cosmossdk.io/collections"
	storetypes "cosmossdk.io/core/store"
	"cosmossdk.io/log"
	"cosmossdk.io/math"

	"github.com/cosmos/cosmos-sdk/codec"
	sdk "github.com/cosmos/cosmos-sdk/types"

	hgparams "github.com/hashgram/hashgram/app/params"
	"github.com/hashgram/hashgram/x/serviceproof/types"
)

// BankKeeper is the subset of x/bank that x/serviceproof needs.
//
// No MintCoins. The reserve is a module account funded once at genesis;
// rewards are transfers out of it. That is what makes "never mint past the
// canonical supply" structural rather than a policy someone has to remember.
type BankKeeper interface {
	GetAllBalances(ctx context.Context, addr sdk.AccAddress) sdk.Coins
	SendCoinsFromAccountToModule(ctx context.Context, sender sdk.AccAddress, recipientModule string, amt sdk.Coins) error
	SendCoinsFromModuleToAccount(ctx context.Context, senderModule string, recipient sdk.AccAddress, amt sdk.Coins) error
	SendCoinsFromModuleToModule(ctx context.Context, senderModule, recipientModule string, amt sdk.Coins) error
	BlockedAddr(addr sdk.AccAddress) bool
}

// AccountKeeper is the subset of x/auth that x/serviceproof needs.
type AccountKeeper interface {
	GetModuleAddress(name string) sdk.AccAddress
}

// NetworkKeeper supplies domain-separated digests for receipts and challenge
// responses, so evidence from one network cannot be replayed on another.
type NetworkKeeper interface {
	SigningDigest(ctx context.Context, purpose hgparams.SigningPurpose, payload []byte) ([32]byte, error)
}

// Keeper manages providers, evidence, epochs and reward settlement.
type Keeper struct {
	cdc    codec.BinaryCodec
	logger log.Logger

	bankKeeper    BankKeeper
	accountKeeper AccountKeeper
	networkKeeper NetworkKeeper

	authority string

	Schema collections.Schema

	params    collections.Item[types.Params]
	reserve   collections.Item[types.ReserveState]
	assigners collections.KeySet[string]

	currentEpoch     collections.Item[uint64]
	epochStartHeight collections.Item[int64]

	providers collections.Map[string, types.Provider]
	epochs    collections.Map[uint64, types.Epoch]

	// credits is keyed by (epoch, operator).
	credits collections.Map[collections.Pair[uint64, string], types.ProviderEpochCredit]

	// clientCredit is keyed by (epoch, operator, client address) and records
	// how much credit each counterparty contributed. It is what the
	// concentration cap is computed from at settlement.
	clientCredit collections.Map[collections.Triple[uint64, string, string], math.Int]

	// assignments is keyed by (provider, blobID, replicaIndex).
	assignments collections.Map[collections.Triple[string, []byte, uint32], types.StorageAssignment]

	challenges      collections.Map[uint64, types.StorageChallenge]
	nextChallengeID collections.Sequence

	// receiptNonces is keyed by (provider, client address, nonce).
	receiptNonces collections.KeySet[collections.Triple[string, string, uint64]]

	fraudReports collections.Map[collections.Pair[string, uint64], types.FraudReport]
	fraudSeq     collections.Sequence

	lifetimePaid collections.Map[string, types.TotalPaid]

	reserveAddress  sdk.AccAddress
	bondPoolAddress sdk.AccAddress
}

// NewKeeper constructs the x/serviceproof keeper.
func NewKeeper(
	cdc codec.BinaryCodec,
	storeService storetypes.KVStoreService,
	ak AccountKeeper,
	bk BankKeeper,
	nk NetworkKeeper,
	authority string,
	logger log.Logger,
) Keeper {
	if _, err := sdk.AccAddressFromBech32(authority); err != nil {
		panic(fmt.Sprintf("x/serviceproof: invalid authority %q: %v", authority, err))
	}
	reserveAddr := ak.GetModuleAddress(types.ModuleName)
	if reserveAddr == nil {
		panic("x/serviceproof: the serviceproof module account has not been registered in maccPerms")
	}
	bondAddr := ak.GetModuleAddress(types.BondPoolName)
	if bondAddr == nil {
		panic("x/serviceproof: the serviceproof_bond module account has not been registered in maccPerms")
	}

	sb := collections.NewSchemaBuilder(storeService)

	k := Keeper{
		cdc:             cdc,
		logger:          logger.With("module", "x/"+types.ModuleName),
		bankKeeper:      bk,
		accountKeeper:   ak,
		networkKeeper:   nk,
		authority:       authority,
		reserveAddress:  reserveAddr,
		bondPoolAddress: bondAddr,

		params: collections.NewItem(sb, types.ParamsKey, "params",
			codec.CollValue[types.Params](cdc)),
		reserve: collections.NewItem(sb, types.ReserveKey, "reserve",
			codec.CollValue[types.ReserveState](cdc)),
		assigners: collections.NewKeySet(sb, types.AssignersKey, "assigners",
			collections.StringKey),

		currentEpoch: collections.NewItem(sb, types.CurrentEpochKey, "current_epoch",
			collections.Uint64Value),
		epochStartHeight: collections.NewItem(sb, types.EpochStartKey, "epoch_start_height",
			collections.Int64Value),

		providers: collections.NewMap(sb, types.ProviderKey, "providers",
			collections.StringKey, codec.CollValue[types.Provider](cdc)),
		epochs: collections.NewMap(sb, types.EpochKey, "epochs",
			collections.Uint64Key, codec.CollValue[types.Epoch](cdc)),

		credits: collections.NewMap(sb, types.CreditKey, "credits",
			collections.PairKeyCodec(collections.Uint64Key, collections.StringKey),
			codec.CollValue[types.ProviderEpochCredit](cdc)),

		clientCredit: collections.NewMap(sb, types.ClientCreditKey, "client_credit",
			collections.TripleKeyCodec(collections.Uint64Key, collections.StringKey, collections.StringKey),
			sdk.IntValue),

		assignments: collections.NewMap(sb, types.AssignmentKey, "assignments",
			collections.TripleKeyCodec(collections.StringKey, collections.BytesKey, collections.Uint32Key),
			codec.CollValue[types.StorageAssignment](cdc)),

		challenges: collections.NewMap(sb, types.ChallengeKey, "challenges",
			collections.Uint64Key, codec.CollValue[types.StorageChallenge](cdc)),
		nextChallengeID: collections.NewSequence(sb, types.NextChallengeIDKey, "next_challenge_id"),

		receiptNonces: collections.NewKeySet(sb, types.ReceiptNonceKey, "receipt_nonces",
			collections.TripleKeyCodec(collections.StringKey, collections.StringKey, collections.Uint64Key)),

		fraudReports: collections.NewMap(sb, types.FraudReportKey, "fraud_reports",
			collections.PairKeyCodec(collections.StringKey, collections.Uint64Key),
			codec.CollValue[types.FraudReport](cdc)),
		fraudSeq: collections.NewSequence(sb, types.FraudSeqKey, "fraud_seq"),

		lifetimePaid: collections.NewMap(sb, types.LifetimePaidKey, "lifetime_paid",
			collections.StringKey, codec.CollValue[types.TotalPaid](cdc)),
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

// ReserveAddress returns the module account holding the reward reserve.
func (k Keeper) ReserveAddress() sdk.AccAddress { return k.reserveAddress }

// BondPoolAddress returns the module account holding provider bonds.
func (k Keeper) BondPoolAddress() sdk.AccAddress { return k.bondPoolAddress }

// ReserveRemaining returns the live reserve balance.
func (k Keeper) ReserveRemaining(ctx context.Context) sdk.Coins {
	return k.bankKeeper.GetAllBalances(ctx, k.reserveAddress)
}

// BondedTotal returns the total posted bond.
func (k Keeper) BondedTotal(ctx context.Context) sdk.Coins {
	return k.bankKeeper.GetAllBalances(ctx, k.bondPoolAddress)
}

// ---------------------------------------------------------------------------
// Params
// ---------------------------------------------------------------------------

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

// UpdateParams applies a governance-approved parameter change and replaces
// the assigner set.
func (k Keeper) UpdateParams(ctx context.Context, authority string, next types.Params, assigners []string) error {
	if authority != k.authority {
		return types.ErrInvalidAuthority.Wrapf("expected %s, got %s", k.authority, authority)
	}
	if err := k.SetParams(ctx, next); err != nil {
		return err
	}
	return k.SetAssigners(ctx, assigners)
}

// ---------------------------------------------------------------------------
// Assigners
// ---------------------------------------------------------------------------

// SetAssigners replaces the registered assigner set.
func (k Keeper) SetAssigners(ctx context.Context, assigners []string) error {
	if err := k.assigners.Clear(ctx, nil); err != nil {
		return err
	}
	for _, a := range assigners {
		if _, err := sdk.AccAddressFromBech32(a); err != nil {
			return types.ErrNotAnAssigner.Wrapf("%q: %v", a, err)
		}
		if err := k.assigners.Set(ctx, a); err != nil {
			return err
		}
	}
	return nil
}

// IsAssigner reports whether an address may record storage assignments.
func (k Keeper) IsAssigner(ctx context.Context, addr string) (bool, error) {
	return k.assigners.Has(ctx, addr)
}

// Assigners returns the registered assigner set.
func (k Keeper) Assigners(ctx context.Context) ([]string, error) {
	var out []string
	err := k.assigners.Walk(ctx, nil, func(a string) (bool, error) {
		out = append(out, a)
		return false, nil
	})
	return out, err
}

// ---------------------------------------------------------------------------
// Reserve
// ---------------------------------------------------------------------------

// GetReserve reads the reserve accounting record.
func (k Keeper) GetReserve(ctx context.Context) (types.ReserveState, error) {
	r, err := k.reserve.Get(ctx)
	if err != nil {
		if errors.Is(err, collections.ErrNotFound) {
			return types.ReserveState{
				Initial:      types.InitialReserve(),
				TotalEmitted: sdk.NewCoins(),
				TotalSlashed: sdk.NewCoins(),
			}, nil
		}
		return types.ReserveState{}, err
	}
	return r, nil
}

// SetReserve writes the reserve accounting record.
func (k Keeper) SetReserve(ctx context.Context, r types.ReserveState) error {
	if err := r.Validate(); err != nil {
		return err
	}
	return k.reserve.Set(ctx, r)
}

// ---------------------------------------------------------------------------
// Providers
// ---------------------------------------------------------------------------

// GetProvider reads a provider.
func (k Keeper) GetProvider(ctx context.Context, operator string) (types.Provider, error) {
	p, err := k.providers.Get(ctx, operator)
	if err != nil {
		if errors.Is(err, collections.ErrNotFound) {
			return types.Provider{}, types.ErrProviderNotFound.Wrapf("%s", operator)
		}
		return types.Provider{}, err
	}
	return p, nil
}

// HasProvider reports whether a provider is registered.
func (k Keeper) HasProvider(ctx context.Context, operator string) (bool, error) {
	return k.providers.Has(ctx, operator)
}

// SetProvider writes a provider.
func (k Keeper) SetProvider(ctx context.Context, p types.Provider) error {
	return k.providers.Set(ctx, p.Operator, p)
}

// IterateProviders walks every provider in address order.
func (k Keeper) IterateProviders(ctx context.Context, fn func(types.Provider) (stop bool, err error)) error {
	return k.providers.Walk(ctx, nil, func(_ string, v types.Provider) (bool, error) {
		return fn(v)
	})
}

// LifetimePaid returns everything a provider has earned.
func (k Keeper) LifetimePaid(ctx context.Context, operator string) (sdk.Coins, error) {
	tp, err := k.lifetimePaid.Get(ctx, operator)
	if err != nil {
		if errors.Is(err, collections.ErrNotFound) {
			return sdk.NewCoins(), nil
		}
		return nil, err
	}
	return tp.Amount, nil
}

func (k Keeper) addLifetimePaid(ctx context.Context, operator string, amount sdk.Coins) error {
	current, err := k.LifetimePaid(ctx, operator)
	if err != nil {
		return err
	}
	return k.lifetimePaid.Set(ctx, operator, types.TotalPaid{Amount: current.Add(amount...)})
}

// ---------------------------------------------------------------------------
// Epochs
// ---------------------------------------------------------------------------

// CurrentEpochNumber returns the open epoch number.
func (k Keeper) CurrentEpochNumber(ctx context.Context) (uint64, error) {
	n, err := k.currentEpoch.Get(ctx)
	if err != nil {
		if errors.Is(err, collections.ErrNotFound) {
			return 0, nil
		}
		return 0, err
	}
	return n, nil
}

// SetCurrentEpochNumber sets the open epoch number.
func (k Keeper) SetCurrentEpochNumber(ctx context.Context, n uint64) error {
	return k.currentEpoch.Set(ctx, n)
}

// EpochStartHeight returns when the open epoch began.
func (k Keeper) EpochStartHeight(ctx context.Context) (int64, error) {
	h, err := k.epochStartHeight.Get(ctx)
	if err != nil {
		if errors.Is(err, collections.ErrNotFound) {
			return 0, nil
		}
		return 0, err
	}
	return h, nil
}

// SetEpochStartHeight sets when the open epoch began.
func (k Keeper) SetEpochStartHeight(ctx context.Context, h int64) error {
	return k.epochStartHeight.Set(ctx, h)
}

// GetEpoch reads an epoch record.
func (k Keeper) GetEpoch(ctx context.Context, n uint64) (types.Epoch, error) {
	e, err := k.epochs.Get(ctx, n)
	if err != nil {
		if errors.Is(err, collections.ErrNotFound) {
			return types.Epoch{}, types.ErrEpochNotFound.Wrapf("epoch %d", n)
		}
		return types.Epoch{}, err
	}
	return e, nil
}

// SetEpoch writes an epoch record.
func (k Keeper) SetEpoch(ctx context.Context, e types.Epoch) error {
	return k.epochs.Set(ctx, e.Number, e)
}

// IterateEpochs walks every epoch record.
func (k Keeper) IterateEpochs(ctx context.Context, fn func(types.Epoch) (stop bool, err error)) error {
	return k.epochs.Walk(ctx, nil, func(_ uint64, v types.Epoch) (bool, error) {
		return fn(v)
	})
}

// ---------------------------------------------------------------------------
// Credit
// ---------------------------------------------------------------------------

// GetCredit reads a provider's credit for an epoch, returning a zeroed record
// if none exists yet.
func (k Keeper) GetCredit(ctx context.Context, epoch uint64, operator string) (types.ProviderEpochCredit, error) {
	c, err := k.credits.Get(ctx, collections.Join(epoch, operator))
	if err != nil {
		if errors.Is(err, collections.ErrNotFound) {
			return types.NewCredit(operator, epoch), nil
		}
		return types.ProviderEpochCredit{}, err
	}
	return c, nil
}

// SetCredit writes a provider's credit for an epoch.
func (k Keeper) SetCredit(ctx context.Context, c types.ProviderEpochCredit) error {
	return k.credits.Set(ctx, collections.Join(c.Epoch, c.Operator), c)
}

// IterateEpochCredits walks every provider's credit for one epoch.
func (k Keeper) IterateEpochCredits(ctx context.Context, epoch uint64, fn func(types.ProviderEpochCredit) (stop bool, err error)) error {
	rng := collections.NewPrefixedPairRange[uint64, string](epoch)
	return k.credits.Walk(ctx, rng, func(_ collections.Pair[uint64, string], v types.ProviderEpochCredit) (bool, error) {
		return fn(v)
	})
}

// IterateAllCredits walks every credit record.
func (k Keeper) IterateAllCredits(ctx context.Context, fn func(types.ProviderEpochCredit) (stop bool, err error)) error {
	return k.credits.Walk(ctx, nil, func(_ collections.Pair[uint64, string], v types.ProviderEpochCredit) (bool, error) {
		return fn(v)
	})
}

// addClientCredit records how much credit a specific counterparty
// contributed, which is what the concentration cap reads at settlement.
func (k Keeper) addClientCredit(ctx context.Context, epoch uint64, operator, client string, amount math.Int) error {
	key := collections.Join3(epoch, operator, client)
	current, err := k.clientCredit.Get(ctx, key)
	if err != nil {
		if !errors.Is(err, collections.ErrNotFound) {
			return err
		}
		current = math.ZeroInt()
	}
	return k.clientCredit.Set(ctx, key, current.Add(amount))
}

// LargestClientCredit returns the biggest single-counterparty contribution to
// a provider's credit in an epoch, and how many distinct counterparties there
// were.
func (k Keeper) LargestClientCredit(ctx context.Context, epoch uint64, operator string) (largest math.Int, distinct uint64, err error) {
	largest = math.ZeroInt()
	rng := collections.NewSuperPrefixedTripleRange[uint64, string, string](epoch, operator)
	err = k.clientCredit.Walk(ctx, rng, func(_ collections.Triple[uint64, string, string], v math.Int) (bool, error) {
		distinct++
		if v.GT(largest) {
			largest = v
		}
		return false, nil
	})
	return largest, distinct, err
}
