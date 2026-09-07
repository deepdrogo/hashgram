package keeper

import (
	"context"

	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"

	sdk "github.com/cosmos/cosmos-sdk/types"
	"github.com/cosmos/cosmos-sdk/types/query"

	"github.com/hashgram/hashgram/x/serviceproof/types"
)

var _ types.QueryServer = Querier{}

// MaxEmissionProjectionEpochs bounds the EmissionSchedule query, so a client
// cannot ask a node to compute an unbounded projection.
const MaxEmissionProjectionEpochs = 400

// RecentEpochsInRewardsQuery is how many settled epochs the Rewards query
// returns alongside the open one.
const RecentEpochsInRewardsQuery = 10

// Querier serves x/serviceproof queries.
type Querier struct {
	k Keeper
}

// NewQuerier wraps a keeper as a query server.
func NewQuerier(k Keeper) Querier { return Querier{k: k} }

func (q Querier) Params(ctx context.Context, _ *types.QueryParamsRequest) (*types.QueryParamsResponse, error) {
	p, err := q.k.GetParams(ctx)
	if err != nil {
		return nil, status.Error(codes.NotFound, err.Error())
	}
	assigners, err := q.k.Assigners(ctx)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	return &types.QueryParamsResponse{Params: p, Assigners: assigners}, nil
}

func (q Querier) Reserve(ctx context.Context, _ *types.QueryReserveRequest) (*types.QueryReserveResponse, error) {
	r, err := q.k.GetReserve(ctx)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	return &types.QueryReserveResponse{
		Reserve:        r,
		Remaining:      q.k.ReserveRemaining(ctx),
		ReserveAccount: q.k.ReserveAddress().String(),
		Bonded:         q.k.BondedTotal(ctx),
	}, nil
}

func (q Querier) Provider(ctx context.Context, req *types.QueryProviderRequest) (*types.QueryProviderResponse, error) {
	if req == nil || req.Operator == "" {
		return nil, status.Error(codes.InvalidArgument, "operator is required")
	}
	p, err := q.k.GetProvider(ctx, req.Operator)
	if err != nil {
		return nil, status.Error(codes.NotFound, err.Error())
	}

	assignments, err := q.k.ProviderAssignments(ctx, req.Operator)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	var active, bytes uint64
	for _, a := range assignments {
		if a.Active {
			active++
			bytes += a.SizeBytes
		}
	}

	return &types.QueryProviderResponse{
		Provider:           p,
		ActiveAssignments:  active,
		TotalAssignedBytes: bytes,
	}, nil
}

func (q Querier) Providers(ctx context.Context, req *types.QueryProvidersRequest) (*types.QueryProvidersResponse, error) {
	if req == nil {
		req = &types.QueryProvidersRequest{}
	}
	providers, pageRes, err := query.CollectionPaginate(
		ctx, q.k.providers, req.Pagination,
		func(_ string, v types.Provider) (types.Provider, error) { return v, nil },
	)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	return &types.QueryProvidersResponse{Providers: providers, Pagination: pageRes}, nil
}

func (q Querier) CurrentEpoch(ctx context.Context, _ *types.QueryCurrentEpochRequest) (*types.QueryCurrentEpochResponse, error) {
	params, err := q.k.GetParams(ctx)
	if err != nil {
		return nil, status.Error(codes.NotFound, err.Error())
	}
	number, err := q.k.CurrentEpochNumber(ctx)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	epoch, err := q.k.GetEpoch(ctx, number)
	if err != nil {
		return nil, status.Error(codes.NotFound, err.Error())
	}
	start, err := q.k.EpochStartHeight(ctx)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}

	sdkCtx := sdk.UnwrapSDKContext(ctx)
	// #nosec G115 -- EpochBlocks is a validated positive block count.
	remaining := start + int64(params.EpochBlocks) - sdkCtx.BlockHeight()
	if remaining < 0 {
		remaining = 0
	}

	return &types.QueryCurrentEpochResponse{
		Epoch:           epoch,
		BlocksRemaining: remaining,
		ProjectedBudget: types.EmissionForEpoch(q.k.ReserveRemaining(ctx), params),
	}, nil
}

func (q Querier) Epoch(ctx context.Context, req *types.QueryEpochRequest) (*types.QueryEpochResponse, error) {
	if req == nil {
		return nil, status.Error(codes.InvalidArgument, "empty request")
	}
	e, err := q.k.GetEpoch(ctx, req.Number)
	if err != nil {
		return nil, status.Error(codes.NotFound, err.Error())
	}
	return &types.QueryEpochResponse{Epoch: e}, nil
}

func (q Querier) Rewards(ctx context.Context, req *types.QueryRewardsRequest) (*types.QueryRewardsResponse, error) {
	if req == nil || req.Operator == "" {
		return nil, status.Error(codes.InvalidArgument, "operator is required")
	}

	current, err := q.k.CurrentEpochNumber(ctx)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	credit, err := q.k.GetCredit(ctx, current, req.Operator)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	lifetime, err := q.k.LifetimePaid(ctx, req.Operator)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}

	params, err := q.k.GetParams(ctx)
	if err != nil {
		return nil, status.Error(codes.NotFound, err.Error())
	}
	effective, _, err := q.k.EffectiveCredit(ctx, current, credit, params)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	largest, _, err := q.k.LargestClientCredit(ctx, current, req.Operator)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}

	// Walk backwards from the epoch before the open one.
	var recent []types.ProviderEpochCredit
	// #nosec G115 -- the epoch counter advances once per EpochBlocks blocks,
	// so reaching 2^63 would take longer than the age of the universe.
	for e := int64(current) - 1; e >= 0 && len(recent) < RecentEpochsInRewardsQuery; e-- {
		c, err := q.k.GetCredit(ctx, uint64(e), req.Operator)
		if err != nil {
			continue
		}
		if c.RawTotal().IsZero() && c.Paid.IsZero() {
			continue
		}
		recent = append(recent, c)
	}

	return &types.QueryRewardsResponse{
		Current:             credit,
		EffectiveCredit:     effective,
		LargestClientCredit: largest,
		LifetimePaid:        lifetime,
		Recent:              recent,
	}, nil
}

func (q Querier) Assignments(ctx context.Context, req *types.QueryAssignmentsRequest) (*types.QueryAssignmentsResponse, error) {
	if req == nil || req.Provider == "" {
		return nil, status.Error(codes.InvalidArgument, "provider is required")
	}
	assignments, err := q.k.ProviderAssignments(ctx, req.Provider)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	return &types.QueryAssignmentsResponse{Assignments: assignments}, nil
}

func (q Querier) OpenChallenges(ctx context.Context, req *types.QueryOpenChallengesRequest) (*types.QueryOpenChallengesResponse, error) {
	if req == nil || req.Provider == "" {
		return nil, status.Error(codes.InvalidArgument, "provider is required")
	}
	challenges, err := q.k.OpenChallenges(ctx, req.Provider)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	return &types.QueryOpenChallengesResponse{Challenges: challenges}, nil
}

func (q Querier) FraudReports(ctx context.Context, req *types.QueryFraudReportsRequest) (*types.QueryFraudReportsResponse, error) {
	if req == nil || req.Provider == "" {
		return nil, status.Error(codes.InvalidArgument, "provider is required")
	}
	reports, err := q.k.FraudReports(ctx, req.Provider)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}
	params, err := q.k.GetParams(ctx)
	if err != nil {
		return nil, status.Error(codes.NotFound, err.Error())
	}

	var score uint32
	if p, err := q.k.GetProvider(ctx, req.Provider); err == nil {
		score = p.FraudScore
	}

	return &types.QueryFraudReportsResponse{
		Reports:       reports,
		CurrentScore:  score,
		JailThreshold: params.FraudScoreJailThreshold,
	}, nil
}

func (q Querier) EmissionSchedule(ctx context.Context, req *types.QueryEmissionScheduleRequest) (*types.QueryEmissionScheduleResponse, error) {
	params, err := q.k.GetParams(ctx)
	if err != nil {
		return nil, status.Error(codes.NotFound, err.Error())
	}

	n := 30
	if req != nil && req.Epochs > 0 {
		n = int(req.Epochs)
	}
	if n > MaxEmissionProjectionEpochs {
		n = MaxEmissionProjectionEpochs
	}

	current, err := q.k.CurrentEpochNumber(ctx)
	if err != nil {
		return nil, status.Error(codes.Internal, err.Error())
	}

	return &types.QueryEmissionScheduleResponse{
		Projections: types.ProjectEmission(q.k.ReserveRemaining(ctx), params, n, current),
	}, nil
}
