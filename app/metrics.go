package app

import (
	"fmt"
	"math/big"

	"cosmossdk.io/math"

	sdk "github.com/cosmos/cosmos-sdk/types"
	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promauto"

	hgparams "github.com/hashgram/hashgram/app/params"
	serviceprooftypes "github.com/hashgram/hashgram/x/serviceproof/types"
)

// Hashgram-specific Prometheus metrics.
//
// CometBFT already exposes consensus, mempool and P2P metrics, and the SDK
// exposes transaction counts and per-module block timings. What neither can
// know is whether Hashgram's own economic invariants still hold, so that is
// what this file adds. See docs/OPERATIONS.md for the dashboards and alerts
// built on these series.
//
// Gauges are registered directly with the Prometheus default registry rather
// than through the SDK's telemetry package. The SDK's API takes float32,
// whose 24-bit mantissa cannot represent a uhash amount above about 1.7e7
// exactly; amounts here reach 1e15, so total supply and its ceiling would
// both round to the same float32 and a supply breach would be invisible in
// the one metric that exists to detect it. CometBFT serves
// prometheus.DefaultGatherer, so metrics registered here appear on the same
// endpoint as the CometBFT ones with no extra listener.
//
// Two further constraints shaped this:
//
//  1. These are emitted from EndBlock, inside consensus. A slow metric is a
//     slow block for the whole network. Cheap scalar reads run every block;
//     anything that iterates state runs on an interval.
//
//  2. Nothing here may fail a block. Errors are returned for logging and
//     never propagate into the block result. Turning an observability fault
//     into a liveness fault would be a poor trade.
//
// Note that these gauges are process-global, as all Prometheus metrics are.
// A test binary that constructs several apps writes to one set of series.
const (
	// metricsScanInterval is how often the metrics that iterate state are
	// recomputed, in blocks. At the 4 second target that is about every six
	// minutes: well inside any useful scrape interval, and cheap enough that
	// the scan cost does not show up in block times.
	metricsScanInterval = 100

	metricNamespace = "hashgram"
)

var promFactory = promauto.With(prometheus.DefaultRegisterer)

// Supply gauges.
//
// gaugeSupplyExcess is exported as its own series rather than left to be
// computed in PromQL so that the alert rule is a comparison against zero and
// cannot be got subtly wrong. It is also computed with exact integer
// arithmetic before conversion, so a one-uhash breach is still visible even
// though the absolute totals are floats.
var (
	gaugeSupplyTotal = promFactory.NewGauge(prometheus.GaugeOpts{
		Namespace: metricNamespace,
		Name:      "supply_total_uhash",
		Help:      "Total uhash in existence. Fixed at genesis; must never rise.",
	})
	gaugeSupplyCeiling = promFactory.NewGauge(prometheus.GaugeOpts{
		Namespace: metricNamespace,
		Name:      "supply_ceiling_uhash",
		Help:      "Compile-time maximum supply in uhash.",
	})
	gaugeSupplyExcess = promFactory.NewGauge(prometheus.GaugeOpts{
		Namespace: metricNamespace,
		Name:      "supply_over_ceiling_uhash",
		Help: "Amount by which total supply exceeds the ceiling. " +
			"Any value above zero means coins were created and is a critical fault.",
	})
)

// Useful-service reward gauges.
var (
	gaugeServiceReserve = promFactory.NewGauge(prometheus.GaugeOpts{
		Namespace: metricNamespace,
		Name:      "service_reserve_remaining_uhash",
		Help:      "Uhash left in the finite useful-service reward reserve.",
	})
	gaugeServiceReserveRatio = promFactory.NewGauge(prometheus.GaugeOpts{
		Namespace: metricNamespace,
		Name:      "service_reserve_remaining_ratio",
		Help:      "Reserve remaining as a fraction of its genesis size, 0 to 1.",
	})
	gaugeServiceEmitted = promFactory.NewGauge(prometheus.GaugeOpts{
		Namespace: metricNamespace,
		Name:      "service_emitted_total_uhash",
		Help:      "Cumulative uhash paid out of the reserve since genesis.",
	})
	gaugeServiceEpoch = promFactory.NewGauge(prometheus.GaugeOpts{
		Namespace: metricNamespace,
		Name:      "service_epoch",
		Help:      "Current useful-service settlement epoch number.",
	})
	gaugeServiceEpochBudget = promFactory.NewGauge(prometheus.GaugeOpts{
		Namespace: metricNamespace,
		Name:      "service_epoch_budget_uhash",
		Help: "Reward budget the schedule allocates for the next settlement. " +
			"A ceiling, not a guaranteed payout: the per-provider cap can leave " +
			"part of it undrawn, and the remainder stays in the reserve.",
	})
	gaugeServiceBonded = promFactory.NewGauge(prometheus.GaugeOpts{
		Namespace: metricNamespace,
		Name:      "service_bonded_total_uhash",
		Help:      "Total uhash posted as provider bonds.",
	})
	gaugeServiceProviders = promFactory.NewGaugeVec(prometheus.GaugeOpts{
		Namespace: metricNamespace,
		Name:      "service_providers",
		Help:      "Registered useful-service providers by status.",
	}, []string{"status"})
	gaugeServiceOpenChallenges = promFactory.NewGauge(prometheus.GaugeOpts{
		Namespace: metricNamespace,
		Name:      "service_open_challenges",
		Help:      "Storage challenges issued and not yet answered.",
	})
	gaugeServiceAssignments = promFactory.NewGauge(prometheus.GaugeOpts{
		Namespace: metricNamespace,
		Name:      "service_storage_assignments",
		Help:      "Active storage assignments across all providers.",
	})
	gaugeServiceAssignedBytes = promFactory.NewGauge(prometheus.GaugeOpts{
		Namespace: metricNamespace,
		Name:      "service_assigned_bytes",
		Help:      "Total bytes assigned to storage providers.",
	})
)

// Founder revenue gauges.
//
// Published because "the Founder receives 1% of protocol fee revenue and
// nothing else" is a claim that should be checkable by anyone running a node,
// not only by reading the source. Accrued, paid and pending are separate
// series so a payout failure shows up as pending growing without paid
// following it.
var (
	gaugeFounderAccrued = promFactory.NewGauge(prometheus.GaugeOpts{
		Namespace: metricNamespace,
		Name:      "founder_revenue_accrued_uhash",
		Help:      "Cumulative uhash credited to the Founder revenue share.",
	})
	gaugeFounderPaid = promFactory.NewGauge(prometheus.GaugeOpts{
		Namespace: metricNamespace,
		Name:      "founder_revenue_paid_uhash",
		Help:      "Cumulative uhash actually paid to the Founder beneficiary.",
	})
	gaugeFounderPending = promFactory.NewGauge(prometheus.GaugeOpts{
		Namespace: metricNamespace,
		Name:      "founder_revenue_pending_uhash",
		Help: "Accrued but unpaid uhash. Sustained growth here without " +
			"founder_revenue_paid_uhash rising means payouts are failing.",
	})
	gaugeFounderFeeBps = promFactory.NewGauge(prometheus.GaugeOpts{
		Namespace: metricNamespace,
		Name:      "founder_fee_basis_points",
		Help: "The Founder share of qualifying protocol revenue, in basis points. " +
			"Cannot exceed the compile-time ceiling; alert if it changes at all.",
	})
)

// Fee routing gauges.
var (
	gaugeRevenueQualifying = promFactory.NewGauge(prometheus.GaugeOpts{
		Namespace: metricNamespace,
		Name:      "revenue_qualifying_total_uhash",
		Help:      "Cumulative protocol fee revenue routed through the fee router.",
	})
	gaugeRevenueFounder = promFactory.NewGauge(prometheus.GaugeOpts{
		Namespace: metricNamespace,
		Name:      "revenue_founder_share_uhash",
		Help:      "The Founder portion of routed revenue.",
	})
	gaugeRevenueValidator = promFactory.NewGauge(prometheus.GaugeOpts{
		Namespace: metricNamespace,
		Name:      "revenue_validator_share_uhash",
		Help:      "The validator and delegator portion of routed revenue.",
	})
)

// Welcome reward gauges.
var (
	gaugeWelcomeSequence = promFactory.NewGauge(prometheus.GaugeOpts{
		Namespace: metricNamespace,
		Name:      "welcome_next_sequence",
		Help:      "Next Welcome claim sequence number, which determines the tier.",
	})
	gaugeWelcomePaid = promFactory.NewGauge(prometheus.GaugeOpts{
		Namespace: metricNamespace,
		Name:      "welcome_paid_total_uhash",
		Help:      "Cumulative uhash paid out as Welcome rewards.",
	})
	gaugeWelcomeRemaining = promFactory.NewGauge(prometheus.GaugeOpts{
		Namespace: metricNamespace,
		Name:      "welcome_pool_remaining_uhash",
		Help:      "Uhash left in the capped Welcome pool.",
	})
)

// providerStatuses is the full set of labels gaugeServiceProviders reports.
//
// Declared explicitly so the children can be created at startup. A Prometheus
// GaugeVec child does not appear in /metrics until it has been set once, which
// means an alert like "jailed providers above zero" silently evaluates against
// a missing series on a fresh node. CometBFT has the same behaviour with
// cometbft_p2p_peers, which is absent until the first peer connects, and it is
// a genuine trap when writing alert rules.
var providerStatuses = []string{"active", "jailed", "unbonding"}

func init() {
	for _, status := range providerStatuses {
		gaugeServiceProviders.WithLabelValues(status).Set(0)
	}
}

// RecordMetrics publishes Hashgram's own gauges.
//
// Called from EndBlocker. Returns errors for the caller to log; it does not
// return them as a block failure.
func (app *HashgramApp) RecordMetrics(ctx sdk.Context) []error {
	var errs []error
	note := func(what string, err error) {
		if err != nil {
			errs = append(errs, fmt.Errorf("%s: %w", what, err))
		}
	}

	note("supply", app.recordSupplyMetrics(ctx))
	note("service reserve", app.recordServiceScalarMetrics(ctx))
	note("founder revenue", app.recordFounderMetrics(ctx))
	note("fee routing", app.recordRevenueMetrics(ctx))
	note("welcome", app.recordWelcomeMetrics(ctx))

	if ctx.BlockHeight()%metricsScanInterval == 0 {
		note("provider scan", app.recordProviderMetrics(ctx))
		note("storage scan", app.recordStorageMetrics(ctx))
	}

	return errs
}

// recordSupplyMetrics publishes total supply against the compile-time
// ceiling. This is the metric to alert on above all others: Hashgram claims
// no coin can be created after genesis, and this is that claim as a number.
func (app *HashgramApp) recordSupplyMetrics(ctx sdk.Context) error {
	supply := app.BankKeeper.GetSupply(ctx, hgparams.BaseCoinDenom).Amount
	ceiling := hgparams.MaxSupplyBase()

	gaugeSupplyTotal.Set(toFloat(supply))
	gaugeSupplyCeiling.Set(toFloat(ceiling))

	// Computed with exact integers, then converted, so that a small breach is
	// not lost to floating-point rounding of two large numbers.
	excess := supply.Sub(ceiling)
	if excess.IsNegative() {
		excess = math.ZeroInt()
	}
	gaugeSupplyExcess.Set(toFloat(excess))

	return nil
}

// recordServiceScalarMetrics publishes the reward state readable without
// iterating.
func (app *HashgramApp) recordServiceScalarMetrics(ctx sdk.Context) error {
	reserveCoins := app.ServiceProofKeeper.ReserveRemaining(ctx)
	reserve := reserveCoins.AmountOf(hgparams.BaseCoinDenom)

	gaugeServiceReserve.Set(toFloat(reserve))
	gaugeServiceBonded.Set(toFloat(
		app.ServiceProofKeeper.BondedTotal(ctx).AmountOf(hgparams.BaseCoinDenom)))

	// A fraction rather than an amount, because "3% of the reserve is left"
	// is the form an operator can act on without doing arithmetic.
	genesis := hgparams.HashToBase(hgparams.AllocServiceReserveHash)
	if genesis.IsPositive() {
		gaugeServiceReserveRatio.Set(toFloat(reserve) / toFloat(genesis))
	}

	state, err := app.ServiceProofKeeper.GetReserve(ctx)
	if err != nil {
		return err
	}
	gaugeServiceEmitted.Set(toFloat(state.TotalEmitted.AmountOf(hgparams.BaseCoinDenom)))

	epochNum, err := app.ServiceProofKeeper.CurrentEpochNumber(ctx)
	if err != nil {
		return err
	}
	gaugeServiceEpoch.Set(float64(epochNum))

	// The budget the schedule would allocate now. Published so the declining
	// curve is directly visible rather than having to be inferred from
	// payouts, which are also shaped by the per-provider cap.
	params, err := app.ServiceProofKeeper.GetParams(ctx)
	if err != nil {
		return err
	}
	budget := serviceprooftypes.EmissionForEpoch(reserveCoins, params)
	gaugeServiceEpochBudget.Set(toFloat(budget.AmountOf(hgparams.BaseCoinDenom)))

	return nil
}

func (app *HashgramApp) recordFounderMetrics(ctx sdk.Context) error {
	ledger, err := app.FounderKeeper.GetLedger(ctx)
	if err != nil {
		return err
	}
	gaugeFounderAccrued.Set(toFloat(ledger.TotalAccrued.AmountOf(hgparams.BaseCoinDenom)))
	gaugeFounderPaid.Set(toFloat(ledger.TotalPaid.AmountOf(hgparams.BaseCoinDenom)))
	gaugeFounderPending.Set(toFloat(
		app.FounderKeeper.Pending(ctx).AmountOf(hgparams.BaseCoinDenom)))

	params, err := app.FounderKeeper.GetParams(ctx)
	if err != nil {
		return err
	}
	// Exported so an alert can fire if this ever moves. It cannot exceed the
	// compile-time ceiling, but governance can lower it, and a silent change
	// to the Founder share is exactly what a network should be able to notice.
	gaugeFounderFeeBps.Set(float64(params.FeeBasisPoints))

	return nil
}

func (app *HashgramApp) recordRevenueMetrics(ctx sdk.Context) error {
	totals, err := app.FeeRouterKeeper.GetTotals(ctx)
	if err != nil {
		return err
	}
	gaugeRevenueQualifying.Set(toFloat(totals.TotalQualifying.AmountOf(hgparams.BaseCoinDenom)))
	gaugeRevenueFounder.Set(toFloat(totals.FounderShare.AmountOf(hgparams.BaseCoinDenom)))
	gaugeRevenueValidator.Set(toFloat(totals.ValidatorShare.AmountOf(hgparams.BaseCoinDenom)))
	return nil
}

func (app *HashgramApp) recordWelcomeMetrics(ctx sdk.Context) error {
	seq, err := app.WelcomeKeeper.NextSequence(ctx)
	if err != nil {
		return err
	}
	gaugeWelcomeSequence.Set(float64(seq))

	paid, err := app.WelcomeKeeper.TotalPaid(ctx)
	if err != nil {
		return err
	}
	paidAmount := paid.AmountOf(hgparams.BaseCoinDenom)
	gaugeWelcomePaid.Set(toFloat(paidAmount))

	remaining := hgparams.HashToBase(hgparams.WelcomePoolHash).Sub(paidAmount)
	if remaining.IsNegative() {
		remaining = math.ZeroInt()
	}
	gaugeWelcomeRemaining.Set(toFloat(remaining))

	return nil
}

// recordProviderMetrics counts registered providers by state.
//
// Labelled by status rather than split into separate metrics so a dashboard
// can sum across states without knowing the full set in advance.
func (app *HashgramApp) recordProviderMetrics(ctx sdk.Context) error {
	counts := make(map[string]float64, len(providerStatuses))
	for _, status := range providerStatuses {
		counts[status] = 0
	}

	err := app.ServiceProofKeeper.IterateProviders(ctx,
		func(p serviceprooftypes.Provider) (bool, error) {
			switch {
			case p.Jailed:
				counts["jailed"]++
			case p.UnbondingHeight > 0:
				counts["unbonding"]++
			default:
				counts["active"]++
			}
			return false, nil
		})
	if err != nil {
		return err
	}

	for status, count := range counts {
		gaugeServiceProviders.WithLabelValues(status).Set(count)
	}
	return nil
}

func (app *HashgramApp) recordStorageMetrics(ctx sdk.Context) error {
	assignments, err := app.ServiceProofKeeper.AllAssignments(ctx)
	if err != nil {
		return err
	}

	var assignedBytes uint64
	for _, a := range assignments {
		assignedBytes += a.SizeBytes
	}
	gaugeServiceAssignments.Set(float64(len(assignments)))
	gaugeServiceAssignedBytes.Set(float64(assignedBytes))

	challenges, err := app.ServiceProofKeeper.AllChallenges(ctx)
	if err != nil {
		return err
	}
	var open float64
	for _, c := range challenges {
		if !c.Answered {
			open++
		}
	}
	gaugeServiceOpenChallenges.Set(open)

	return nil
}

// toFloat converts a coin amount to float64 for Prometheus.
//
// Via big.Float rather than Int64() because Int64() panics on an amount that
// does not fit, and a metrics function must not be able to halt a node. The
// float64 mantissa is exact up to 2^53, which is about 9e15 uhash, comfortably
// above the 1e15 ceiling, so ordinary amounts are represented exactly.
func toFloat(amount math.Int) float64 {
	if amount.IsNil() {
		return 0
	}
	f, _ := new(big.Float).SetInt(amount.BigInt()).Float64()
	return f
}
