package cmd

import (
	"encoding/json"
	"fmt"
	"os"
	"strings"

	"github.com/cosmos/cosmos-sdk/types/bech32"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"
	govtypes "github.com/cosmos/cosmos-sdk/x/gov/types"
	"github.com/spf13/cobra"

	hgparams "github.com/hashgram/hashgram/app/params"
)

// cmdPropose writes governance proposal files for the parameter changes an
// operator needs after genesis.
//
// hashgramctl signs nothing, so the output is a file for
// `hashgramd tx gov submit-proposal`. The reason the command exists at all
// is that MsgUpdateParams REPLACES the whole parameter set: a hand-written
// proposal that names only the field being changed silently resets every
// other field to zero. This command reads the live parameters first and
// changes exactly one thing.
func cmdPropose() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "propose",
		Short: "Write a governance proposal file (add-assigner, ...)",
		Long: `Write a governance proposal file for hashgramd tx gov submit-proposal.

The proposal carries the CURRENT on-chain parameters with one change applied,
because a parameter update replaces the whole set. Submit and vote with
hashgramd; hashgramctl holds no keys.`,
	}
	cmd.AddCommand(cmdProposeAddAssigner())
	return cmd
}

func cmdProposeAddAssigner() *cobra.Command {
	var (
		outPath string
		title   string
		summary string
		deposit string
	)
	cmd := &cobra.Command{
		Use:   "add-assigner <operator-address>",
		Short: "Register a storage assigner so stored bytes can earn",
		Long: `Write a proposal that adds a provider operator address to the storage
assigner set of x/serviceproof.

Until at least one assigner exists no storage assignment can be recorded, so
no store node earns for the bytes it holds. The operator address is the one
'hashgramctl configure-role store' created on the node that should assign;
'hashgramctl rewards' prints it.

Then, from a funded key:

  hashgramd tx gov submit-proposal proposal-add-assigner.json --from <key> --chain-id <id>
  hashgramd tx gov vote <proposal-id> yes --from <key> --chain-id <id>

The deposit is returned when the proposal passes.`,
		Args: cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			addr := strings.TrimSpace(args[0])
			if hrp, _, err := bech32.DecodeAndConvert(addr); err != nil || hrp != hgparams.Bech32PrefixAccAddr {
				return fmt.Errorf("%q is not a Hashgram account address (%s1...)", addr, hgparams.Bech32PrefixAccAddr)
			}

			ctx, cancel := commandContext()
			defer cancel()

			var current struct {
				Params    json.RawMessage `json:"params"`
				Assigners []string        `json:"assigners"`
			}
			if err := queryJSON(ctx, &current, "query", "serviceproof", "params"); err != nil {
				return fmt.Errorf("read current serviceproof params: %w", err)
			}
			if len(current.Params) == 0 {
				return fmt.Errorf("the node returned no serviceproof params; is the chain running?")
			}
			for _, a := range current.Assigners {
				if a == addr {
					return fmt.Errorf("%s is already an assigner", addr)
				}
			}
			assigners := append(append([]string{}, current.Assigners...), addr)

			authority, err := bech32.ConvertAndEncode(hgparams.Bech32PrefixAccAddr,
				authtypes.NewModuleAddress(govtypes.ModuleName))
			if err != nil {
				return err
			}

			if title == "" {
				title = "Register storage assigner " + addr
			}
			if summary == "" {
				summary = fmt.Sprintf("Adds %s to the x/serviceproof assigner set so the store nodes "+
					"it operates can record storage assignments and earn for challenged bytes. "+
					"All other useful-service parameters are unchanged.", addr)
			}

			msg := map[string]any{
				"@type":     "/hashgram.serviceproof.v1.MsgUpdateParams",
				"authority": authority,
				"params":    current.Params,
				"assigners": assigners,
			}
			proposal := map[string]any{
				"messages": []any{msg},
				"metadata": "",
				"deposit":  deposit,
				"title":    title,
				"summary":  summary,
			}
			raw, err := json.MarshalIndent(proposal, "", "  ")
			if err != nil {
				return err
			}
			if err := os.WriteFile(outPath, append(raw, '\n'), 0o644); err != nil {
				return err
			}

			o := newOut()
			o.section("Proposal written")
			o.row("file", outPath)
			o.row("adds assigner", addr)
			o.row("assigner set after", strings.Join(assigners, ", "))
			o.row("deposit", deposit)
			o.blank()
			o.raw("  Submit from a funded key, then vote:")
			o.raw(fmt.Sprintf("    hashgramd tx gov submit-proposal %s --from <key> --chain-id <id> --gas auto --gas-adjustment 1.4 --fees 5000uhash", outPath))
			o.raw("    hashgramd query gov proposals --output json      # find the id")
			o.raw("    hashgramd tx gov vote <id> yes --from <key> --chain-id <id> --fees 5000uhash")
			return o.flush()
		},
	}
	cmd.Flags().StringVar(&outPath, "out", "proposal-add-assigner.json", "where to write the proposal")
	cmd.Flags().StringVar(&title, "title", "", "proposal title (default: generated)")
	cmd.Flags().StringVar(&summary, "summary", "", "proposal summary (default: generated)")
	cmd.Flags().StringVar(&deposit, "deposit", "10000000000uhash",
		"initial deposit; Mainnet requires 10,000 HASH, refunded when the proposal passes")
	return cmd
}
