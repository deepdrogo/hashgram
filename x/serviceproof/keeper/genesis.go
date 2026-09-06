package keeper

import (
	sdk "github.com/cosmos/cosmos-sdk/types"

	"github.com/hashgram/hashgram/x/serviceproof/types"
)

// InitGenesis writes the reward configuration and any restored state.
func (k Keeper) InitGenesis(ctx sdk.Context, gs types.GenesisState) error {
	if err := gs.Validate(); err != nil {
		return err
	}
	if err := k.SetParams(ctx, gs.Params); err != nil {
		return err
	}
	if err := k.SetReserve(ctx, gs.Reserve); err != nil {
		return err
	}
	if err := k.SetAssigners(ctx, gs.Assigners); err != nil {
		return err
	}
	if err := k.SetCurrentEpochNumber(ctx, gs.CurrentEpoch); err != nil {
		return err
	}
	if err := k.SetEpochStartHeight(ctx, gs.EpochStartHeight); err != nil {
		return err
	}
	if err := k.SetNextChallengeID(ctx, gs.NextChallengeId); err != nil {
		return err
	}

	for _, p := range gs.Providers {
		if err := k.SetProvider(ctx, p); err != nil {
			return err
		}
	}
	for _, a := range gs.Assignments {
		if err := k.SetAssignment(ctx, a); err != nil {
			return err
		}
	}
	for _, c := range gs.Credits {
		if err := k.SetCredit(ctx, c); err != nil {
			return err
		}
	}
	for _, e := range gs.Epochs {
		if err := k.SetEpoch(ctx, e); err != nil {
			return err
		}
	}
	for _, r := range gs.FraudReports {
		if err := k.SetFraudReport(ctx, r); err != nil {
			return err
		}
	}
	for _, ch := range gs.OpenChallenges {
		if err := k.SetChallenge(ctx, ch); err != nil {
			return err
		}
	}

	if len(gs.Assigners) == 0 {
		k.Logger().Info("useful-service rewards started with NO registered storage assigners",
			"effect", "no storage can be assigned, so no storage rewards can be earned",
			"why", "a provider assigning work to itself would be self-reporting",
			"fix", "register an assigner through governance")
	}
	k.Logger().Info("useful-service reward system configured",
		"reserve", gs.Reserve.Initial.String(),
		"epoch_blocks", gs.Params.EpochBlocks,
		"emission_rate_bps", gs.Params.EmissionRateBps,
		"max_provider_share_bps", gs.Params.MaxProviderShareBps,
		"max_client_concentration_bps", gs.Params.MaxClientConcentrationBps,
		"min_bond", gs.Params.MinBond.String(),
	)

	return nil
}

// ExportGenesis reads the reward state back out.
func (k Keeper) ExportGenesis(ctx sdk.Context) (*types.GenesisState, error) {
	params, err := k.GetParams(ctx)
	if err != nil {
		return nil, err
	}
	reserve, err := k.GetReserve(ctx)
	if err != nil {
		return nil, err
	}
	assigners, err := k.Assigners(ctx)
	if err != nil {
		return nil, err
	}
	epoch, err := k.CurrentEpochNumber(ctx)
	if err != nil {
		return nil, err
	}
	start, err := k.EpochStartHeight(ctx)
	if err != nil {
		return nil, err
	}
	nextChallenge, err := k.NextChallengeID(ctx)
	if err != nil {
		return nil, err
	}

	var providers []types.Provider
	if err := k.IterateProviders(ctx, func(p types.Provider) (bool, error) {
		providers = append(providers, p)
		return false, nil
	}); err != nil {
		return nil, err
	}

	assignments, err := k.AllAssignments(ctx)
	if err != nil {
		return nil, err
	}

	var credits []types.ProviderEpochCredit
	if err := k.IterateAllCredits(ctx, func(c types.ProviderEpochCredit) (bool, error) {
		credits = append(credits, c)
		return false, nil
	}); err != nil {
		return nil, err
	}

	var epochs []types.Epoch
	if err := k.IterateEpochs(ctx, func(e types.Epoch) (bool, error) {
		epochs = append(epochs, e)
		return false, nil
	}); err != nil {
		return nil, err
	}

	fraud, err := k.AllFraudReports(ctx)
	if err != nil {
		return nil, err
	}
	open, err := k.AllChallenges(ctx)
	if err != nil {
		return nil, err
	}

	return &types.GenesisState{
		Params:           params,
		Reserve:          reserve,
		Assigners:        assigners,
		CurrentEpoch:     epoch,
		EpochStartHeight: start,
		Providers:        providers,
		Assignments:      assignments,
		Credits:          credits,
		Epochs:           epochs,
		FraudReports:     fraud,
		NextChallengeId:  nextChallenge,
		OpenChallenges:   open,
	}, nil
}
