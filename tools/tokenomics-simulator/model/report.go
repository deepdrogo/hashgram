package model

import (
	"encoding/csv"
	"encoding/json"
	"fmt"
	"io"
	"strconv"
	"strings"

	"cosmossdk.io/math"

	hgparams "github.com/hashgram/hashgram/app/params"
)

// formatHashFromBase renders a uhash amount as whole HASH with thousands
// separators, truncating the fractional part.
//
// Whole HASH is the right unit for a multi-year report: six decimal places of
// uhash on a nine-figure number is noise that makes the table unreadable.
func formatHashFromBase(base math.Int) string {
	if base.IsNil() {
		return "0"
	}
	whole := base.Quo(math.NewInt(hgparams.MicroUnit))
	return addThousands(whole.String())
}

// FormatHash renders a uhash amount as whole HASH with thousands separators.
// Exported for callers building their own summaries.
func FormatHash(base math.Int) string { return formatHashFromBase(base) }

// formatFractionalHash renders a small uhash amount with its decimal places
// intact.
//
// Needed for per-transaction fees, which are a few thousand uhash: rendering
// those as whole HASH shows "0", which reads as though the scenario assumed
// no fee at all.
func formatFractionalHash(base int64) string {
	whole := base / hgparams.MicroUnit
	frac := base % hgparams.MicroUnit
	if frac == 0 {
		return addThousands(strconv.FormatInt(whole, 10))
	}
	s := fmt.Sprintf("%d.%06d", whole, frac)
	return strings.TrimRight(s, "0")
}

// formatCount renders an integer with thousands separators.
func formatCount(n int64) string {
	return addThousands(strconv.FormatInt(n, 10))
}

func addThousands(s string) string {
	neg := strings.HasPrefix(s, "-")
	if neg {
		s = s[1:]
	}
	if len(s) <= 3 {
		if neg {
			return "-" + s
		}
		return s
	}

	var b strings.Builder
	lead := len(s) % 3
	if lead > 0 {
		b.WriteString(s[:lead])
	}
	for i := lead; i < len(s); i += 3 {
		if b.Len() > 0 {
			b.WriteByte(',')
		}
		b.WriteString(s[i : i+3])
	}
	if neg {
		return "-" + b.String()
	}
	return b.String()
}

// WriteTable renders the human-readable report.
func (r *Result) WriteTable(w io.Writer) error {
	p := func(format string, args ...any) {
		fmt.Fprintf(w, format, args...)
	}

	p("\n")
	p("=========================================================================\n")
	p(" HASHGRAM TOKENOMICS SIMULATION: %s\n", strings.ToUpper(r.Scenario.Name))
	p("=========================================================================\n")
	p("\n%s\n", wrap(r.Scenario.Description, 72, ""))

	p("\nPROTOCOL CONSTANTS (imported from the chain, not configurable here)\n")
	p("-------------------------------------------------------------------------\n")
	for _, kv := range ProtocolConstants() {
		p("  %-26s %s\n", kv[0], kv[1])
	}

	p("\nSCENARIO ASSUMPTIONS (inputs, not protocol facts)\n")
	p("-------------------------------------------------------------------------\n")
	for _, kv := range r.Scenario.Assumptions() {
		p("  %-30s %s\n", kv[0], kv[1])
	}

	p("\nPER-YEAR FLOWS, in whole HASH\n")
	p("-------------------------------------------------------------------------\n")
	p("  %4s %12s %8s %12s %12s %12s %12s\n",
		"Year", "Users", "Nodes", "Vested", "Welcome", "Emission", "Fee rev.")
	for _, y := range r.Years {
		p("  %4d %12s %8s %12s %12s %12s %12s\n",
			y.Year,
			formatCount(y.CumulativeUsers),
			formatCount(y.Providers),
			formatHashFromBase(y.FounderVested),
			formatHashFromBase(y.WelcomePaid),
			formatHashFromBase(y.ServiceEmission),
			formatHashFromBase(y.GasRevenue.Add(y.ServiceFeeRevenue)),
		)
	}

	p("\nFOUNDER ECONOMICS, in whole HASH\n")
	p("-------------------------------------------------------------------------\n")
	p("  %4s %14s %14s %16s %14s\n",
		"Year", "Vested", "Revenue 1%", "Cumul. revenue", "Liquid total")
	for _, y := range r.Years {
		p("  %4d %14s %14s %16s %14s\n",
			y.Year,
			formatHashFromBase(y.FounderVested),
			formatHashFromBase(y.FounderRevenue),
			formatHashFromBase(y.CumulativeFounderRevenue),
			formatHashFromBase(y.Balances[BucketFounderLiquid]),
		)
	}

	p("\nRESERVE AND POOL DEPLETION, in whole HASH\n")
	p("-------------------------------------------------------------------------\n")
	p("  %4s %16s %9s %14s %14s\n",
		"Year", "Service reserve", "of orig.", "Growth pool", "Treasury")
	for _, y := range r.Years {
		p("  %4d %16s %9s %14s %14s\n",
			y.Year,
			formatHashFromBase(y.ReserveRemaining),
			y.ReservePercent,
			formatHashFromBase(y.Balances[BucketGrowth]),
			formatHashFromBase(y.Balances[BucketTreasury]),
		)
	}

	p("\nSUPPLY DISTRIBUTION AT END OF RUN, in whole HASH\n")
	p("-------------------------------------------------------------------------\n")
	final := r.FinalLedger.Snapshot()
	total := r.FinalLedger.Total()
	for _, name := range Buckets() {
		p("  %-20s %18s  %8s\n",
			name,
			formatHashFromBase(final[name]),
			percentOf(final[name], total),
		)
	}
	p("  %-20s %18s  %8s\n", "TOTAL", formatHashFromBase(total), "100.00%")

	p("\nSUPPLY INVARIANT\n")
	p("-------------------------------------------------------------------------\n")
	p("  Genesis supply   %s uhash\n", hgparams.MaxSupplyBase())
	p("  Final supply     %s uhash\n", total)
	if total.Equal(hgparams.MaxSupplyBase()) {
		p("  Result           UNCHANGED across %d epochs. No coin was created.\n",
			r.Scenario.Years*EpochsPerYear)
	} else {
		p("  Result           BROKEN. Delta %s uhash.\n", total.Sub(hgparams.MaxSupplyBase()))
	}

	p("\nWHAT THIS RUN SHOWS\n")
	p("-------------------------------------------------------------------------\n")
	for i, note := range r.Notes {
		p("  %d. %s\n\n", i+1, wrap(note, 68, "     "))
	}

	p("HOW TO READ THIS\n")
	p("-------------------------------------------------------------------------\n")
	p("%s\n", wrap(
		"The protocol constants above are compiled into the chain and are "+
			"imported by this tool, so the emission curve, the welcome tiers and "+
			"the Founder basis points shown here are the ones the state machine "+
			"executes. The scenario assumptions are guesses about human "+
			"behaviour. Treat the shape of the curves as informative and the "+
			"absolute revenue figures as illustrative, and re-run with different "+
			"assumptions before relying on any single number.", 72, "  "))
	p("\n")

	return nil
}

// WriteCSV emits one row per year, for plotting or spreadsheet work.
//
// Amounts are in uhash, unformatted, because a spreadsheet should receive
// exact integers rather than a rendering with separators in it.
func (r *Result) WriteCSV(w io.Writer) error {
	cw := csv.NewWriter(w)
	defer cw.Flush()

	header := []string{
		"year", "new_users", "cumulative_users", "active_users", "providers",
		"founder_vested_uhash", "welcome_paid_uhash",
		"service_emission_uhash", "emission_forfeited_uhash",
		"gas_revenue_uhash", "service_fee_revenue_uhash",
		"founder_revenue_uhash", "validator_revenue_uhash",
		"cumulative_founder_revenue_uhash", "cumulative_emission_uhash",
		"liquidity_released_uhash", "treasury_spent_uhash", "dev_grants_spent_uhash",
		"welcome_claims", "circulating_uhash", "total_supply_uhash",
	}
	for _, b := range Buckets() {
		header = append(header, "bucket_"+b+"_uhash")
	}
	if err := cw.Write(header); err != nil {
		return err
	}

	for _, y := range r.Years {
		row := []string{
			strconv.Itoa(y.Year),
			strconv.FormatInt(y.NewUsers, 10),
			strconv.FormatInt(y.CumulativeUsers, 10),
			strconv.FormatInt(y.ActiveUsers, 10),
			strconv.FormatInt(y.Providers, 10),
			y.FounderVested.String(),
			y.WelcomePaid.String(),
			y.ServiceEmission.String(),
			y.EmissionForfeited.String(),
			y.GasRevenue.String(),
			y.ServiceFeeRevenue.String(),
			y.FounderRevenue.String(),
			y.ValidatorRevenue.String(),
			y.CumulativeFounderRevenue.String(),
			y.CumulativeEmission.String(),
			y.LiquidityReleased.String(),
			y.TreasurySpent.String(),
			y.DevGrantsSpent.String(),
			strconv.FormatInt(y.WelcomeClaims, 10),
			y.Circulating.String(),
			r.FinalLedger.Total().String(),
		}
		for _, b := range Buckets() {
			row = append(row, y.Balances[b].String())
		}
		if err := cw.Write(row); err != nil {
			return err
		}
	}
	return cw.Error()
}

// jsonYear is the wire shape for one year, with uhash as strings so that
// large integers survive JSON parsers that use float64 for numbers.
type jsonYear struct {
	Year                     int               `json:"year"`
	NewUsers                 int64             `json:"new_users"`
	CumulativeUsers          int64             `json:"cumulative_users"`
	ActiveUsers              int64             `json:"active_users"`
	Providers                int64             `json:"providers"`
	FounderVestedUhash       string            `json:"founder_vested_uhash"`
	WelcomePaidUhash         string            `json:"welcome_paid_uhash"`
	ServiceEmissionUhash     string            `json:"service_emission_uhash"`
	EmissionForfeitedUhash   string            `json:"emission_forfeited_uhash"`
	GasRevenueUhash          string            `json:"gas_revenue_uhash"`
	ServiceFeeRevenueUhash   string            `json:"service_fee_revenue_uhash"`
	FounderRevenueUhash      string            `json:"founder_revenue_uhash"`
	ValidatorRevenueUhash    string            `json:"validator_revenue_uhash"`
	CumulativeFounderRevenue string            `json:"cumulative_founder_revenue_uhash"`
	CumulativeEmission       string            `json:"cumulative_emission_uhash"`
	WelcomeClaims            int64             `json:"welcome_claims"`
	CirculatingUhash         string            `json:"circulating_uhash"`
	ReserveRemainingUhash    string            `json:"reserve_remaining_uhash"`
	ReservePercentOfGenesis  string            `json:"reserve_percent_of_genesis"`
	Balances                 map[string]string `json:"balances_uhash"`
}

// WriteJSON emits the whole result, for downstream tooling.
func (r *Result) WriteJSON(w io.Writer) error {
	years := make([]jsonYear, 0, len(r.Years))
	for _, y := range r.Years {
		balances := make(map[string]string, len(y.Balances))
		for k, v := range y.Balances {
			balances[k] = v.String()
		}
		years = append(years, jsonYear{
			Year:                     y.Year,
			NewUsers:                 y.NewUsers,
			CumulativeUsers:          y.CumulativeUsers,
			ActiveUsers:              y.ActiveUsers,
			Providers:                y.Providers,
			FounderVestedUhash:       y.FounderVested.String(),
			WelcomePaidUhash:         y.WelcomePaid.String(),
			ServiceEmissionUhash:     y.ServiceEmission.String(),
			EmissionForfeitedUhash:   y.EmissionForfeited.String(),
			GasRevenueUhash:          y.GasRevenue.String(),
			ServiceFeeRevenueUhash:   y.ServiceFeeRevenue.String(),
			FounderRevenueUhash:      y.FounderRevenue.String(),
			ValidatorRevenueUhash:    y.ValidatorRevenue.String(),
			CumulativeFounderRevenue: y.CumulativeFounderRevenue.String(),
			CumulativeEmission:       y.CumulativeEmission.String(),
			WelcomeClaims:            y.WelcomeClaims,
			CirculatingUhash:         y.Circulating.String(),
			ReserveRemainingUhash:    y.ReserveRemaining.String(),
			ReservePercentOfGenesis:  y.ReservePercent,
			Balances:                 balances,
		})
	}

	assumptions := make(map[string]string, 16)
	for _, kv := range r.Scenario.Assumptions() {
		assumptions[kv[0]] = kv[1]
	}
	constants := make(map[string]string, 8)
	for _, kv := range ProtocolConstants() {
		constants[kv[0]] = kv[1]
	}

	out := struct {
		Scenario           string            `json:"scenario"`
		Description        string            `json:"description"`
		Years              int               `json:"years"`
		EpochsPerYear      int               `json:"epochs_per_year"`
		ProtocolConstants  map[string]string `json:"protocol_constants"`
		Assumptions        map[string]string `json:"assumptions"`
		GenesisSupplyUhash string            `json:"genesis_supply_uhash"`
		FinalSupplyUhash   string            `json:"final_supply_uhash"`
		SupplyUnchanged    bool              `json:"supply_unchanged"`
		Notes              []string          `json:"notes"`
		Results            []jsonYear        `json:"years_detail"`
	}{
		Scenario:           r.Scenario.Name,
		Description:        r.Scenario.Description,
		Years:              r.Scenario.Years,
		EpochsPerYear:      EpochsPerYear,
		ProtocolConstants:  constants,
		Assumptions:        assumptions,
		GenesisSupplyUhash: hgparams.MaxSupplyBase().String(),
		FinalSupplyUhash:   r.FinalLedger.Total().String(),
		SupplyUnchanged:    r.FinalLedger.Total().Equal(hgparams.MaxSupplyBase()),
		Notes:              r.Notes,
		Results:            years,
	}

	enc := json.NewEncoder(w)
	enc.SetIndent("", "  ")
	return enc.Encode(out)
}

// wrap breaks text at a column, indenting continuation lines.
func wrap(text string, width int, indent string) string {
	words := strings.Fields(text)
	if len(words) == 0 {
		return ""
	}

	var (
		lines []string
		line  = words[0]
	)
	for _, word := range words[1:] {
		if len(line)+1+len(word) > width {
			lines = append(lines, line)
			line = word
			continue
		}
		line += " " + word
	}
	lines = append(lines, line)

	for i := 1; i < len(lines); i++ {
		lines[i] = indent + lines[i]
	}
	return strings.Join(lines, "\n")
}
