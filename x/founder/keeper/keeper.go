// Package keeper implements the x/founder revenue ledger.
//
// The Founder share is taken from protocol fee revenue the chain has already
// collected, never from transferred principal. Accrued, paid and pending are
// tracked separately so that a payout failure is visible rather than being
// absorbed into a single total.
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
	"github.com/hashgram/hashgram/x/founder/types"
)

// BankKeeper is the subset of x/bank that x/founder needs.
//
// Notably absent: MintCoins. x/founder cannot create HASH; it can only move
// coins that other modules routed into its account.
type BankKeeper interface {
	GetAllBalances(ctx context.Context, addr sdk.AccAddress) sdk.Coins
	SendCoinsFromModuleToAccount(ctx context.Context, senderModule string, recipient sdk.AccAddress, amt sdk.Coins) error
	SendCoinsFromModuleToModule(ctx context.Context, senderModule, recipientModule string, amt sdk.Coins) error
	BlockedAddr(addr sdk.AccAddress) bool
}

// AccountKeeper is the subset of x/auth that x/founder needs.
type AccountKeeper interface {
	GetModuleAddress(name string) sdk.AccAddress
}

// Keeper manages the Founder beneficiary, revenue accrual and payout.
type Keeper struct {
	cdc          codec.BinaryCodec
	logger       log.Logger
	bankKeeper   BankKeeper
	accountKeepr AccountKeeper

	// authority is the only address permitted to change params. It is the
	// governance module account. There is no second authority, and in
	// particular the beneficiary has no privileged role: being paid by the
	// protocol does not confer the power to reconfigure it.
	authority string

	Schema collections.Schema

	params        collections.Item[types.Params]
	ledger        collections.Item[types.RevenueLedger]
	history       collections.Map[uint64, types.BeneficiaryChange]
	historySeq    collections.Sequence
	moduleAddress sdk.AccAddress
}

// NewKeeper constructs the x/founder keeper.
func NewKeeper(
	cdc codec.BinaryCodec,
	storeService storetypes.KVStoreService,
	ak AccountKeeper,
	bk BankKeeper,
	authority string,
	logger log.Logger,
) Keeper {
	if _, err := sdk.AccAddressFromBech32(authority); err != nil {
		panic(fmt.Sprintf("x/founder: invalid authority %q: %v", authority, err))
	}

	moduleAddr := ak.GetModuleAddress(types.ModuleName)
	if moduleAddr == nil {
		panic("x/founder: the founder module account has not been registered in maccPerms")
	}

	sb := collections.NewSchemaBuilder(storeService)

	k := Keeper{
		cdc:           cdc,
		logger:        logger.With("module", "x/"+types.ModuleName),
		bankKeeper:    bk,
		accountKeepr:  ak,
		authority:     authority,
		moduleAddress: moduleAddr,
		params: collections.NewItem(
			sb, types.ParamsKey, "params",
			codec.CollValue[types.Params](cdc),
		),
		ledger: collections.NewItem(
			sb, types.LedgerKey, "ledger",
			codec.CollValue[types.RevenueLedger](cdc),
		),
		history: collections.NewMap(
			sb, types.BeneficiaryHistoryKey, "beneficiary_history",
			collections.Uint64Key, codec.CollValue[types.BeneficiaryChange](cdc),
		),
		historySeq: collections.NewSequence(
			sb, types.BeneficiaryHistorySeqKey, "beneficiary_history_seq",
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

// ModuleAddress returns the address that holds accrued-but-unpaid revenue.
func (k Keeper) ModuleAddress() sdk.AccAddress { return k.moduleAddress }

// ---------------------------------------------------------------------------
// Params
// ---------------------------------------------------------------------------

// SetParams writes the parameter set after validating it.
//
// Validation is applied here as well as in ValidateBasic so that no code path,
// including genesis and migrations, can install a fee above the ceiling.
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

// FeeBasisPoints returns the configured Founder share in basis points, or 0
// if routing has not been configured.
//
// Returning zero rather than an error on a missing param set means a
// misconfigured chain under-pays the Founder instead of halting block
// production. That is the correct direction for a liveness-critical path in
// BeginBlock.
func (k Keeper) FeeBasisPoints(ctx context.Context) uint32 {
	p, err := k.GetParams(ctx)
	if err != nil {
		return 0
	}
	return p.FeeBasisPoints
}

// MaxFeeBasisPoints returns the compile-time ceiling in this binary.
func (Keeper) MaxFeeBasisPoints() uint32 { return hgparams.MaxFounderFeeBasisPoints }

// Beneficiary returns the configured beneficiary address.
func (k Keeper) Beneficiary(ctx context.Context) (sdk.AccAddress, error) {
	p, err := k.GetParams(ctx)
	if err != nil {
		return nil, err
	}
	if p.Beneficiary == "" {
		return nil, types.ErrBeneficiaryNotSet
	}
	addr, err := sdk.AccAddressFromBech32(p.Beneficiary)
	if err != nil {
		return nil, types.ErrInvalidBeneficiary.Wrapf("%q: %v", p.Beneficiary, err)
	}
	return addr, nil
}

// ---------------------------------------------------------------------------
// Ledger
// ---------------------------------------------------------------------------

// GetLedger reads the revenue ledger, returning a zero ledger if unset.
func (k Keeper) GetLedger(ctx context.Context) (types.RevenueLedger, error) {
	l, err := k.ledger.Get(ctx)
	if err != nil {
		if errors.Is(err, collections.ErrNotFound) {
			return types.RevenueLedger{
				TotalAccrued: sdk.NewCoins(),
				TotalPaid:    sdk.NewCoins(),
			}, nil
		}
		return types.RevenueLedger{}, err
	}
	return l, nil
}

// SetLedger writes the revenue ledger.
func (k Keeper) SetLedger(ctx context.Context, l types.RevenueLedger) error {
	return k.ledger.Set(ctx, l)
}

// Pending returns the balance of the founder module account: revenue that has
// accrued but not yet been paid out.
func (k Keeper) Pending(ctx context.Context) sdk.Coins {
	return k.bankKeeper.GetAllBalances(ctx, k.moduleAddress)
}

// ---------------------------------------------------------------------------
// Beneficiary history
// ---------------------------------------------------------------------------

func (k Keeper) appendBeneficiaryChange(ctx context.Context, prev, next string) error {
	seq, err := k.historySeq.Next(ctx)
	if err != nil {
		return err
	}
	sdkCtx := sdk.UnwrapSDKContext(ctx)
	return k.history.Set(ctx, seq, types.BeneficiaryChange{
		Height:              sdkCtx.BlockHeight(),
		PreviousBeneficiary: prev,
		NewBeneficiary:      next,
	})
}

// BeneficiaryHistory returns every recorded beneficiary change, in order.
func (k Keeper) BeneficiaryHistory(ctx context.Context) ([]types.BeneficiaryChange, error) {
	var out []types.BeneficiaryChange
	err := k.history.Walk(ctx, nil, func(_ uint64, v types.BeneficiaryChange) (bool, error) {
		out = append(out, v)
		return false, nil
	})
	if err != nil {
		return nil, err
	}
	return out, nil
}

// RecordInitialBeneficiary logs the genesis beneficiary as history entry zero,
// so the audit trail starts at launch rather than at the first change.
func (k Keeper) RecordInitialBeneficiary(ctx context.Context, beneficiary string) error {
	if beneficiary == "" {
		return nil
	}
	return k.appendBeneficiaryChange(ctx, "", beneficiary)
}
