// Package keeper implements x/welcome, the tiered joining reward.
//
// Creating a keypair earns nothing. A claim requires a signed eligibility
// attestation, a sequence number that has not been used, and a nonce that
// has not been seen, which is what makes the reward resistant to a script.
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
	"github.com/hashgram/hashgram/x/welcome/types"
)

// BankKeeper is the subset of x/bank that x/welcome needs.
//
// No MintCoins. The welcome pool is funded once at genesis out of the Growth
// allocation; the programme cannot create HASH, only distribute what it was
// given.
type BankKeeper interface {
	GetAllBalances(ctx context.Context, addr sdk.AccAddress) sdk.Coins
	SendCoinsFromModuleToAccount(ctx context.Context, senderModule string, recipient sdk.AccAddress, amt sdk.Coins) error
	BlockedAddr(addr sdk.AccAddress) bool
}

// AccountKeeper is the subset of x/auth that x/welcome needs.
type AccountKeeper interface {
	GetModuleAddress(name string) sdk.AccAddress
}

// NetworkKeeper supplies the domain-separated digest an attestation is signed
// over.
//
// Taking this as a dependency rather than reimplementing the digest means an
// attestation from a devnet or a fork can never verify on Mainnet, and the
// rule is enforced in one place.
type NetworkKeeper interface {
	SigningDigest(ctx context.Context, purpose hgparams.SigningPurpose, payload []byte) ([32]byte, error)
}

// Keeper manages the welcome programme.
type Keeper struct {
	cdc    codec.BinaryCodec
	logger log.Logger

	bankKeeper    BankKeeper
	accountKeeper AccountKeeper
	networkKeeper NetworkKeeper

	authority string

	Schema collections.Schema

	params       collections.Item[types.Params]
	nextSequence collections.Sequence
	claims       collections.Map[string, types.ClaimRecord]

	// usedNonces is keyed by (attestor address, nonce). This is the replay
	// guard: an attestation may be consumed once.
	usedNonces collections.KeySet[collections.Pair[string, uint64]]

	// attestorEpochCount is keyed by (attestor address, epoch) and bounds how
	// much damage a single compromised attestor can do.
	attestorEpochCount collections.Map[collections.Pair[string, uint64], uint64]

	totalPaid  collections.Item[types.TotalPaid]
	claimsPaid collections.Sequence

	poolAddress sdk.AccAddress
}

// NewKeeper constructs the x/welcome keeper.
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
		panic(fmt.Sprintf("x/welcome: invalid authority %q: %v", authority, err))
	}
	pool := ak.GetModuleAddress(types.ModuleName)
	if pool == nil {
		panic("x/welcome: the welcome module account has not been registered in maccPerms")
	}

	sb := collections.NewSchemaBuilder(storeService)

	k := Keeper{
		cdc:           cdc,
		logger:        logger.With("module", "x/"+types.ModuleName),
		bankKeeper:    bk,
		accountKeeper: ak,
		networkKeeper: nk,
		authority:     authority,
		poolAddress:   pool,
		params: collections.NewItem(
			sb, types.ParamsKey, "params",
			codec.CollValue[types.Params](cdc),
		),
		nextSequence: collections.NewSequence(sb, types.NextSequenceKey, "next_sequence"),
		claims: collections.NewMap(
			sb, types.ClaimKey, "claims",
			collections.StringKey, codec.CollValue[types.ClaimRecord](cdc),
		),
		usedNonces: collections.NewKeySet(
			sb, types.UsedNonceKey, "used_nonces",
			collections.PairKeyCodec(collections.StringKey, collections.Uint64Key),
		),
		attestorEpochCount: collections.NewMap(
			sb, types.AttestorEpochCountKey, "attestor_epoch_count",
			collections.PairKeyCodec(collections.StringKey, collections.Uint64Key),
			collections.Uint64Value,
		),
		totalPaid: collections.NewItem(
			sb, types.TotalPaidKey, "total_paid",
			codec.CollValue[types.TotalPaid](cdc),
		),
		claimsPaid: collections.NewSequence(sb, types.ClaimsPaidKey, "claims_paid"),
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

// PoolAddress returns the module account holding the welcome pool.
func (k Keeper) PoolAddress() sdk.AccAddress { return k.poolAddress }

// PoolRemaining returns the welcome pool balance.
func (k Keeper) PoolRemaining(ctx context.Context) sdk.Coins {
	return k.bankKeeper.GetAllBalances(ctx, k.poolAddress)
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

// UpdateParams applies a governance-approved parameter change.
func (k Keeper) UpdateParams(ctx context.Context, authority string, next types.Params) error {
	if authority != k.authority {
		return types.ErrInvalidAuthority.Wrapf("expected %s, got %s", k.authority, authority)
	}
	return k.SetParams(ctx, next)
}

// ---------------------------------------------------------------------------
// Sequence and totals
// ---------------------------------------------------------------------------

// NextSequence returns the sequence number the next successful claim receives.
func (k Keeper) NextSequence(ctx context.Context) (uint64, error) {
	seq, err := k.nextSequence.Peek(ctx)
	if err != nil {
		return 0, err
	}
	// A sequence of 0 means the store has never been written. Sequence
	// positions start at 1, so the first user is user number 1.
	if seq == 0 {
		return 1, nil
	}
	return seq, nil
}

// SetNextSequence sets the sequence counter, used by genesis.
func (k Keeper) SetNextSequence(ctx context.Context, seq uint64) error {
	return k.nextSequence.Set(ctx, seq)
}

func (k Keeper) advanceSequence(ctx context.Context) (uint64, error) {
	current, err := k.NextSequence(ctx)
	if err != nil {
		return 0, err
	}
	if err := k.nextSequence.Set(ctx, current+1); err != nil {
		return 0, err
	}
	return current, nil
}

// TotalPaid returns the cumulative amount paid out.
func (k Keeper) TotalPaid(ctx context.Context) (sdk.Coins, error) {
	tp, err := k.totalPaid.Get(ctx)
	if err != nil {
		if errors.Is(err, collections.ErrNotFound) {
			return sdk.NewCoins(), nil
		}
		return nil, err
	}
	return tp.Amount, nil
}

func (k Keeper) addTotalPaid(ctx context.Context, amount sdk.Coins) error {
	current, err := k.TotalPaid(ctx)
	if err != nil {
		return err
	}
	return k.totalPaid.Set(ctx, types.TotalPaid{Amount: current.Add(amount...)})
}

// ClaimsPaid returns how many rewards have been paid.
func (k Keeper) ClaimsPaid(ctx context.Context) (uint64, error) {
	return k.claimsPaid.Peek(ctx)
}

// SetClaimsPaid sets the paid-claim counter, used by genesis.
func (k Keeper) SetClaimsPaid(ctx context.Context, n uint64) error {
	return k.claimsPaid.Set(ctx, n)
}

// ---------------------------------------------------------------------------
// Claims
// ---------------------------------------------------------------------------

// HasClaimed reports whether an address has already received a reward.
func (k Keeper) HasClaimed(ctx context.Context, subject string) (bool, error) {
	return k.claims.Has(ctx, subject)
}

// GetClaim returns an address's claim record.
func (k Keeper) GetClaim(ctx context.Context, subject string) (types.ClaimRecord, bool, error) {
	rec, err := k.claims.Get(ctx, subject)
	if err != nil {
		if errors.Is(err, collections.ErrNotFound) {
			return types.ClaimRecord{}, false, nil
		}
		return types.ClaimRecord{}, false, err
	}
	return rec, true, nil
}

// SetClaim writes a claim record, used by genesis and by the claim handler.
func (k Keeper) SetClaim(ctx context.Context, rec types.ClaimRecord) error {
	return k.claims.Set(ctx, rec.Subject, rec)
}

// IterateClaims walks every claim record.
func (k Keeper) IterateClaims(ctx context.Context, fn func(types.ClaimRecord) (stop bool, err error)) error {
	return k.claims.Walk(ctx, nil, func(_ string, v types.ClaimRecord) (bool, error) {
		return fn(v)
	})
}

// ---------------------------------------------------------------------------
// Nonces and attestor caps
// ---------------------------------------------------------------------------

// IsNonceUsed reports whether an (attestor, nonce) pair has been consumed.
func (k Keeper) IsNonceUsed(ctx context.Context, attestor string, nonce uint64) (bool, error) {
	return k.usedNonces.Has(ctx, collections.Join(attestor, nonce))
}

// MarkNonceUsed consumes an (attestor, nonce) pair.
func (k Keeper) MarkNonceUsed(ctx context.Context, attestor string, nonce uint64) error {
	return k.usedNonces.Set(ctx, collections.Join(attestor, nonce))
}

// IterateUsedNonces walks every consumed nonce.
func (k Keeper) IterateUsedNonces(ctx context.Context, fn func(attestor string, nonce uint64) (stop bool, err error)) error {
	return k.usedNonces.Walk(ctx, nil, func(key collections.Pair[string, uint64]) (bool, error) {
		return fn(key.K1(), key.K2())
	})
}

// Epoch returns the epoch number for a height.
func (k Keeper) Epoch(height int64, epochBlocks uint64) uint64 {
	if epochBlocks == 0 || height < 0 {
		return 0
	}
	return uint64(height) / epochBlocks
}

// AttestorEpochCount returns how many claims an attestor has authorised in an
// epoch.
func (k Keeper) AttestorEpochCount(ctx context.Context, attestor string, epoch uint64) (uint64, error) {
	n, err := k.attestorEpochCount.Get(ctx, collections.Join(attestor, epoch))
	if err != nil {
		if errors.Is(err, collections.ErrNotFound) {
			return 0, nil
		}
		return 0, err
	}
	return n, nil
}

func (k Keeper) incrementAttestorEpochCount(ctx context.Context, attestor string, epoch uint64) error {
	n, err := k.AttestorEpochCount(ctx, attestor, epoch)
	if err != nil {
		return err
	}
	return k.attestorEpochCount.Set(ctx, collections.Join(attestor, epoch), n+1)
}
