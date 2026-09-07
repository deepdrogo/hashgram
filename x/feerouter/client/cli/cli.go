// Package cli provides the read-only x/feerouter query commands, including
// the cumulative revenue split that demonstrates the realised Founder share.
package cli

import (
	"github.com/spf13/cobra"

	"github.com/cosmos/cosmos-sdk/client"
	"github.com/cosmos/cosmos-sdk/client/flags"

	"github.com/hashgram/hashgram/x/feerouter/types"
)

// GetQueryCmd returns the x/feerouter query commands.
func GetQueryCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:                        types.ModuleName,
		Short:                      "Query protocol fee revenue routing",
		DisableFlagParsing:         true,
		SuggestionsMinimumDistance: 2,
		RunE:                       client.ValidateCmd,
	}
	cmd.AddCommand(cmdParams(), cmdTotals(), cmdServiceRevenue())
	return cmd
}

func cmdParams() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "params",
		Short: "Show fee routing configuration and the pending revenue pool",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			clientCtx, err := client.GetClientQueryContext(cmd)
			if err != nil {
				return err
			}
			res, err := types.NewQueryClient(clientCtx).Params(cmd.Context(), &types.QueryParamsRequest{})
			if err != nil {
				return err
			}
			return clientCtx.PrintProto(res)
		},
	}
	flags.AddQueryFlagsToCmd(cmd)
	return cmd
}

func cmdTotals() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "totals",
		Short: "Show cumulative qualifying protocol revenue and its split",
		Long: `Show cumulative qualifying protocol revenue and its split.

The following identity always holds and is asserted on every block that
routes revenue:

  total_qualifying = founder_share + validator_share + treasury_share

"Qualifying" means fee revenue the protocol charged for a service: transaction
gas, username registration, and similar. It never includes value that users
transferred to each other.`,
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			clientCtx, err := client.GetClientQueryContext(cmd)
			if err != nil {
				return err
			}
			res, err := types.NewQueryClient(clientCtx).Totals(cmd.Context(), &types.QueryTotalsRequest{})
			if err != nil {
				return err
			}
			return clientCtx.PrintProto(res)
		},
	}
	flags.AddQueryFlagsToCmd(cmd)
	return cmd
}

func cmdServiceRevenue() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "service-revenue",
		Short: "Show cumulative qualifying revenue per service kind",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			clientCtx, err := client.GetClientQueryContext(cmd)
			if err != nil {
				return err
			}
			res, err := types.NewQueryClient(clientCtx).ServiceRevenue(cmd.Context(),
				&types.QueryServiceRevenueRequest{})
			if err != nil {
				return err
			}
			return clientCtx.PrintProto(res)
		},
	}
	flags.AddQueryFlagsToCmd(cmd)
	return cmd
}
