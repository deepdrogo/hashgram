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

	"github.com/hashgram/hashgram/x/treasury/types"
)

// BankKeeper is the subset of x/bank that x/treasury needs.
//
// No MintCoins. Reserves are module accounts funded once at genesis; a spend
// is a transfer out of one.
type BankKeeper interface {
	GetAllBalances(ctx context.Context, addr sdk.AccAddress) sdk.Coins
	SendCoinsFromModuleToAccount(ctx context.Context, senderModule string, recipient sdk.AccAddress, amt sdk.Coins) error
	BlockedAddr(addr sdk.AccAddress) bool
}

// Keeper holds the named genesis allocations.
type Keeper struct {
	cdc        codec.BinaryCodec
	logger     log.Logger
	bankKeeper BankKeeper

	// authority is the governance module account, and the only party that can
	// move these funds. There is no founder key, admin key or operator key
	// with authority here.
	authority string

	Schema collections.Schema

	reserves        collections.Map[string, types.Reserve]
	disbursements   collections.Map[uint64, types.Disbursement]
	disbursementSeq collections.Sequence
}

// NewKeeper constructs the x/treasury keeper.
func NewKeeper(
	cdc codec.BinaryCodec,
	storeService storetypes.KVStoreService,
	bk BankKeeper,
	authority string,
	logger log.Logger,
) Keeper {
	if _, err := sdk.AccAddressFromBech32(authority); err != nil {
		panic(fmt.Sprintf("x/treasury: invalid authority %q: %v", authority, err))
	}

	sb := collections.NewSchemaBuilder(storeService)

	k := Keeper{
		cdc:        cdc,
		logger:     logger.With("module", "x/"+types.ModuleName),
		bankKeeper: bk,
		authority:  authority,
		reserves: collections.NewMap(sb, types.ReserveKey, "reserves",
			collections.StringKey, codec.CollValue[types.Reserve](cdc)),
		disbursements: collections.NewMap(sb, types.DisbursementKey, "disbursements",
			collections.Uint64Key, codec.CollValue[types.Disbursement](cdc)),
		disbursementSeq: collections.NewSequence(sb, types.DisbursementSeqKey, "disbursement_seq"),
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

// GetReserve reads a reserve record.
func (k Keeper) GetReserve(ctx context.Context, name string) (types.Reserve, bool, error) {
	r, err := k.reserves.Get(ctx, name)
	if err != nil {
		if errors.Is(err, collections.ErrNotFound) {
			return types.Reserve{}, false, nil
		}
		return types.Reserve{}, false, err
	}
	return r, true, nil
}

// SetReserve writes a reserve record.
func (k Keeper) SetReserve(ctx context.Context, r types.Reserve) error {
	if err := r.Validate(); err != nil {
		return err
	}
	return k.reserves.Set(ctx, r.Name, r)
}

// Balance returns a reserve's live on-chain balance.
//
// Read from the bank rather than from a counter, so it cannot drift away from
// reality, and so it is cross-checkable with a plain bank query.
func (k Keeper) Balance(ctx context.Context, reserve string) sdk.Coins {
	return k.bankKeeper.GetAllBalances(ctx, types.ReserveAddress(reserve))
}

// IterateReserves walks every reserve in name order.
func (k Keeper) IterateReserves(ctx context.Context, fn func(types.Reserve) (stop bool, err error)) error {
	return k.reserves.Walk(ctx, nil, func(_ string, r types.Reserve) (bool, error) {
		return fn(r)
	})
}

// Spend disburses from a reserve. Governance only.
func (k Keeper) Spend(ctx context.Context, authority, reserve string, recipient sdk.AccAddress, amount sdk.Coins, memo string) error {
	if authority != k.authority {
		return types.ErrInvalidAuthority.Wrapf("expected %s, got %s", k.authority, authority)
	}

	r, found, err := k.GetReserve(ctx, reserve)
	if err != nil {
		return err
	}
	if !found {
		return types.ErrReserveNotFound.Wrapf("%q", reserve)
	}

	if k.bankKeeper.BlockedAddr(recipient) {
		return types.ErrBlockedRecipient.Wrapf("%s", recipient)
	}

	balance := k.Balance(ctx, reserve)
	if !balance.IsAllGTE(amount) {
		return types.ErrInsufficientReserve.Wrapf(
			"reserve %q holds %s, cannot disburse %s", reserve, balance, amount)
	}

	if err := k.bankKeeper.SendCoinsFromModuleToAccount(ctx, r.SubAccount, recipient, amount); err != nil {
		return fmt.Errorf("x/treasury: disbursing %s from %q: %w", amount, reserve, err)
	}

	r.Spent = r.Spent.Add(amount...)
	if err := k.SetReserve(ctx, r); err != nil {
		return err
	}

	seq, err := k.disbursementSeq.Next(ctx)
	if err != nil {
		return err
	}
	sdkCtx := sdk.UnwrapSDKContext(ctx)
	record := types.Disbursement{
		Sequence:  seq,
		Reserve:   reserve,
		Recipient: recipient.String(),
		Amount:    amount,
		Height:    sdkCtx.BlockHeight(),
		Memo:      memo,
	}
	if err := k.disbursements.Set(ctx, seq, record); err != nil {
		return err
	}

	remaining := k.Balance(ctx, reserve)

	sdkCtx.EventManager().EmitEvent(sdk.NewEvent(
		types.EventTypeDisbursed,
		sdk.NewAttribute(types.AttributeKeyReserve, reserve),
		sdk.NewAttribute(types.AttributeKeyRecipient, recipient.String()),
		sdk.NewAttribute(types.AttributeKeyAmount, amount.String()),
		sdk.NewAttribute(types.AttributeKeyMemo, memo),
		sdk.NewAttribute(types.AttributeKeyRemaining, remaining.String()),
	))

	k.logger.Info("treasury disbursement",
		"reserve", reserve,
		"recipient", recipient.String(),
		"amount", amount.String(),
		"remaining", remaining.String(),
		"memo", memo,
	)

	return nil
}

// Disbursements returns the spend history, optionally filtered to one reserve.
func (k Keeper) Disbursements(ctx context.Context, reserve string) ([]types.Disbursement, error) {
	var out []types.Disbursement
	err := k.disbursements.Walk(ctx, nil, func(_ uint64, d types.Disbursement) (bool, error) {
		if reserve == "" || d.Reserve == reserve {
			out = append(out, d)
		}
		return false, nil
	})
	return out, err
}

// SetDisbursement writes a disbursement record, used by genesis.
func (k Keeper) SetDisbursement(ctx context.Context, d types.Disbursement) error {
	return k.disbursements.Set(ctx, d.Sequence, d)
}

// SetDisbursementSeq sets the sequence counter, used by genesis.
func (k Keeper) SetDisbursementSeq(ctx context.Context, n uint64) error {
	return k.disbursementSeq.Set(ctx, n)
}

// InitGenesis writes the reserve set and spend history.
func (k Keeper) InitGenesis(ctx sdk.Context, gs types.GenesisState) error {
	if err := gs.Validate(); err != nil {
		return err
	}
	for _, r := range gs.Reserves {
		if err := k.SetReserve(ctx, r); err != nil {
			return err
		}
	}
	var maxSeq uint64
	for _, d := range gs.Disbursements {
		if err := k.SetDisbursement(ctx, d); err != nil {
			return err
		}
		if d.Sequence >= maxSeq {
			maxSeq = d.Sequence + 1
		}
	}
	if maxSeq > 0 {
		if err := k.SetDisbursementSeq(ctx, maxSeq); err != nil {
			return err
		}
	}

	for _, r := range gs.Reserves {
		k.Logger().Info("treasury reserve established",
			"name", r.Name,
			"initial", r.Initial.String(),
			"address", types.ReserveAddress(r.Name).String(),
			"spendable_by", "x/gov only")
	}
	return nil
}

// ExportGenesis reads the treasury state back out.
func (k Keeper) ExportGenesis(ctx sdk.Context) (*types.GenesisState, error) {
	var reserves []types.Reserve
	if err := k.IterateReserves(ctx, func(r types.Reserve) (bool, error) {
		reserves = append(reserves, r)
		return false, nil
	}); err != nil {
		return nil, err
	}
	disbursements, err := k.Disbursements(ctx, "")
	if err != nil {
		return nil, err
	}
	return &types.GenesisState{Reserves: reserves, Disbursements: disbursements}, nil
}
