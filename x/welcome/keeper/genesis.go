package keeper

import (
	sdk "github.com/cosmos/cosmos-sdk/types"

	"github.com/hashgram/hashgram/x/welcome/types"
)

// InitGenesis writes the welcome configuration, sequence and claim history.
func (k Keeper) InitGenesis(ctx sdk.Context, gs types.GenesisState) error {
	if err := gs.Validate(); err != nil {
		return err
	}
	if err := k.SetParams(ctx, gs.Params); err != nil {
		return err
	}
	if err := k.SetNextSequence(ctx, gs.NextSequence); err != nil {
		return err
	}
	for _, c := range gs.Claims {
		if err := k.SetClaim(ctx, c); err != nil {
			return err
		}
	}
	for _, n := range gs.UsedNonces {
		if err := k.MarkNonceUsed(ctx, n.Attestor, n.Nonce); err != nil {
			return err
		}
	}
	if !gs.TotalPaid.Amount.IsZero() {
		if err := k.totalPaid.Set(ctx, gs.TotalPaid); err != nil {
			return err
		}
	}
	if gs.ClaimsPaid != 0 {
		if err := k.SetClaimsPaid(ctx, gs.ClaimsPaid); err != nil {
			return err
		}
	}

	if len(gs.Params.Attestors) == 0 {
		k.Logger().Info("welcome programme started with NO registered attestors",
			"effect", "no welcome reward can be claimed",
			"why", "creating a key must never by itself produce free HASH",
			"fix", "register an attestor through governance")
	} else {
		k.Logger().Info("welcome programme configured",
			"attestors", len(gs.Params.Attestors),
			"min_confidence_bps", gs.Params.MinConfidence,
			"next_sequence", gs.NextSequence,
			"max_pool", types.MaxPool().String())
	}

	return nil
}

// ExportGenesis reads the welcome state back out.
func (k Keeper) ExportGenesis(ctx sdk.Context) (*types.GenesisState, error) {
	params, err := k.GetParams(ctx)
	if err != nil {
		return nil, err
	}
	nextSeq, err := k.NextSequence(ctx)
	if err != nil {
		return nil, err
	}

	var claims []types.ClaimRecord
	if err := k.IterateClaims(ctx, func(rec types.ClaimRecord) (bool, error) {
		claims = append(claims, rec)
		return false, nil
	}); err != nil {
		return nil, err
	}

	var nonces []types.UsedNonce
	if err := k.IterateUsedNonces(ctx, func(attestor string, nonce uint64) (bool, error) {
		nonces = append(nonces, types.UsedNonce{Attestor: attestor, Nonce: nonce})
		return false, nil
	}); err != nil {
		return nil, err
	}

	totalPaid, err := k.TotalPaid(ctx)
	if err != nil {
		return nil, err
	}
	claimsPaid, err := k.ClaimsPaid(ctx)
	if err != nil {
		return nil, err
	}

	return &types.GenesisState{
		Params:       params,
		NextSequence: nextSeq,
		Claims:       claims,
		UsedNonces:   nonces,
		TotalPaid:    types.TotalPaid{Amount: totalPaid},
		ClaimsPaid:   claimsPaid,
	}, nil
}
