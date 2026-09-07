// Command tokenomics-simulator projects Hashgram token flows over time.
//
// It imports the chain's own emission, tier and Founder-share functions
// rather than reimplementing them, so the curves it draws are the curves the
// state machine executes. Adoption assumptions are supplied by flags and are
// printed alongside every result.
//
// Usage:
//
//	tokenomics-simulator                          # baseline, table output
//	tokenomics-simulator -scenario optimistic
//	tokenomics-simulator -scenario reserve-drain -years 40
//	tokenomics-simulator -all                     # every preset
//	tokenomics-simulator -format csv > out.csv
//	tokenomics-simulator -format json | jq .notes
//
// Protocol constants (max supply, allocations, tier amounts, emission rate,
// Founder basis points) are deliberately not settable. Changing them would
// mean simulating a different chain.
package main

import (
	"flag"
	"fmt"
	"os"
	"strings"

	"github.com/hashgram/hashgram/app/params"
	"github.com/hashgram/hashgram/tools/tokenomics-simulator/model"
)

func main() {
	if err := run(); err != nil {
		fmt.Fprintf(os.Stderr, "error: %v\n", err)
		os.Exit(1)
	}
}

func run() error {
	var (
		scenarioName = flag.String("scenario", "baseline",
			"preset scenario: conservative, baseline, optimistic, reserve-drain")
		all    = flag.Bool("all", false, "run every preset scenario")
		list   = flag.Bool("list", false, "list the preset scenarios and exit")
		format = flag.String("format", "table", "output format: table, csv, json")
		years  = flag.Int("years", 0, "override the scenario horizon in years")

		usersYear1 = flag.Int64("users-year1", -1,
			"override users joining in year one")
		userGrowth = flag.Int64("user-growth-percent", -1000,
			"override year-over-year user growth, in percent")
		providersYear1 = flag.Int64("providers-year1", -1,
			"override the useful-service node count in year one")
		providerGrowth = flag.Int64("provider-growth-percent", -1000,
			"override year-over-year node growth, in percent")
		txPerUser = flag.Float64("tx-per-user-per-day", -1,
			"override fee-paying transactions per active user per day")
		avgTxFee = flag.Int64("avg-tx-fee-uhash", -1,
			"override the average gas fee per transaction, in uhash")
		activeUsers = flag.Int64("active-user-percent", -1,
			"override the share of joined users who transact, in percent")
	)

	flag.Usage = func() {
		fmt.Fprintf(flag.CommandLine.Output(),
			"tokenomics-simulator projects Hashgram token flows over time.\n\n"+
				"It imports the chain's emission, welcome tier and Founder share\n"+
				"functions directly, so the mechanics shown are the mechanics the\n"+
				"state machine runs. Adoption inputs are assumptions and are printed\n"+
				"with every result.\n\n"+
				"Protocol constants cannot be overridden here by design.\n\n"+
				"Flags:\n")
		flag.PrintDefaults()
	}
	flag.Parse()

	// Fail loudly if the binary's own tokenomics constants do not add up.
	// app/params runs this in init() too; repeating it here means the check
	// is visible in this tool's output rather than only as a panic.
	if err := params.ValidateSupplyInvariants(); err != nil {
		return fmt.Errorf("tokenomics constants are inconsistent: %w", err)
	}

	if *list {
		fmt.Println("Preset scenarios:")
		for _, s := range model.Presets() {
			fmt.Printf("\n  %-16s %d years\n", s.Name, s.Years)
			fmt.Printf("  %-16s %s\n", "", s.Description)
		}
		fmt.Println()
		return nil
	}

	var scenarios []model.Scenario
	if *all {
		scenarios = model.Presets()
	} else {
		s, err := model.PresetByName(*scenarioName)
		if err != nil {
			return err
		}
		scenarios = []model.Scenario{s}
	}

	// Apply overrides. The sentinel values are outside each field's valid
	// range so that "not supplied" is distinguishable from a real zero:
	// -users-year1=0 is a meaningful scenario and must not be ignored.
	for i := range scenarios {
		if *years > 0 {
			scenarios[i].Years = *years
		}
		if *usersYear1 >= 0 {
			scenarios[i].UsersYear1 = *usersYear1
		}
		if *userGrowth != -1000 {
			scenarios[i].UserGrowthPercent = *userGrowth
		}
		if *providersYear1 >= 0 {
			scenarios[i].ProvidersYear1 = *providersYear1
		}
		if *providerGrowth != -1000 {
			scenarios[i].ProviderGrowthPercent = *providerGrowth
		}
		if *txPerUser >= 0 {
			scenarios[i].TxPerUserPerDay = *txPerUser
		}
		if *avgTxFee >= 0 {
			scenarios[i].AvgTxFeeUhash = *avgTxFee
		}
		if *activeUsers >= 0 {
			scenarios[i].ActiveUserPercent = *activeUsers
		}
	}

	switch strings.ToLower(*format) {
	case "table":
		for _, s := range scenarios {
			res, err := model.Run(s)
			if err != nil {
				return fmt.Errorf("scenario %s: %w", s.Name, err)
			}
			if err := res.WriteTable(os.Stdout); err != nil {
				return err
			}
		}
		if len(scenarios) > 1 {
			return writeComparison(scenarios)
		}
		return nil

	case "csv":
		if len(scenarios) > 1 {
			return fmt.Errorf(
				"csv output writes one table and cannot represent %d scenarios; "+
					"run them one at a time", len(scenarios))
		}
		res, err := model.Run(scenarios[0])
		if err != nil {
			return err
		}
		return res.WriteCSV(os.Stdout)

	case "json":
		if len(scenarios) == 1 {
			res, err := model.Run(scenarios[0])
			if err != nil {
				return err
			}
			return res.WriteJSON(os.Stdout)
		}
		// Multiple scenarios: emit a JSON array by hand so each element is
		// the same shape a single run produces.
		fmt.Print("[\n")
		for i, s := range scenarios {
			res, err := model.Run(s)
			if err != nil {
				return fmt.Errorf("scenario %s: %w", s.Name, err)
			}
			if err := res.WriteJSON(os.Stdout); err != nil {
				return err
			}
			if i < len(scenarios)-1 {
				fmt.Print(",\n")
			}
		}
		fmt.Print("]\n")
		return nil

	default:
		return fmt.Errorf("unknown format %q; use table, csv or json", *format)
	}
}

// writeComparison prints the one table that matters when several scenarios
// have been run: how the conclusions differ.
func writeComparison(scenarios []model.Scenario) error {
	fmt.Print("\n")
	fmt.Print("=========================================================================\n")
	fmt.Print(" SCENARIO COMPARISON\n")
	fmt.Print("=========================================================================\n\n")
	fmt.Printf("  %-16s %6s %14s %16s %12s\n",
		"Scenario", "Years", "Reserve left", "Founder revenue", "Supply")
	fmt.Printf("  %-16s %6s %14s %16s %12s\n",
		"", "", "(% of 500M)", "(cumul. HASH)", "invariant")

	for _, s := range scenarios {
		res, err := model.Run(s)
		if err != nil {
			return fmt.Errorf("scenario %s: %w", s.Name, err)
		}
		last := res.Years[len(res.Years)-1]

		invariant := "BROKEN"
		if res.FinalLedger.Total().Equal(params.MaxSupplyBase()) {
			invariant = "held"
		}

		fmt.Printf("  %-16s %6d %14s %16s %12s\n",
			s.Name,
			s.Years,
			last.ReservePercent,
			model.FormatHash(last.CumulativeFounderRevenue),
			invariant,
		)
	}

	fmt.Print("\n")
	fmt.Print("  Reading the reserve column: the long-run figure is set almost\n")
	fmt.Print("  entirely by the emission rate and the horizon, not by adoption.\n")
	fmt.Print("  Taking 5 bps of what remains each epoch leaves the same small\n")
	fmt.Print("  percentage after twenty years whether the network has a hundred\n")
	fmt.Print("  nodes or fifty thousand, and it never reaches zero. Node count\n")
	fmt.Print("  moves the early years only: below twenty providers the per-provider\n")
	fmt.Print("  cap holds the draw down, and the undrawn remainder stays in the\n")
	fmt.Print("  reserve to fund later epochs, which is why the scenario with the\n")
	fmt.Print("  fewest nodes ends with the most reserve left.\n\n")
	fmt.Print("  Founder revenue varies by two orders of magnitude across these\n")
	fmt.Print("  scenarios because it is 1% of fee revenue, and fee revenue depends\n")
	fmt.Print("  entirely on the adoption assumptions. That column is the least\n")
	fmt.Print("  reliable number in the tool and should be read as a range, not a\n")
	fmt.Print("  forecast.\n\n")
	return nil
}
