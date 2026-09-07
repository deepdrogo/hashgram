// Package cli provides the x/founder query and transaction commands.
//
// Claiming pending Founder revenue is permissionless: anyone may push it to
// the beneficiary. Changing the beneficiary requires governance.
package cli

import (
	"github.com/spf13/cobra"

	"github.com/cosmos/cosmos-sdk/client"
	"github.com/cosmos/cosmos-sdk/client/flags"
	"github.com/cosmos/cosmos-sdk/client/tx"

	"github.com/hashgram/hashgram/x/founder/types"
)

// GetQueryCmd returns the x/founder query commands.
func GetQueryCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:                        types.ModuleName,
		Short:                      "Query Founder revenue configuration and ledger",
		DisableFlagParsing:         true,
		SuggestionsMinimumDistance: 2,
		RunE:                       client.ValidateCmd,
	}
	cmd.AddCommand(cmdParams(), cmdRevenue(), cmdBeneficiaryHistory())
	return cmd
}

func cmdParams() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "params",
		Short: "Show the Founder beneficiary and revenue share",
		Long: `Show the Founder beneficiary and revenue share.

fee_basis_points is the Founder share of qualifying protocol fee revenue:
100 basis points is 1%. max_fee_basis_points is the ceiling compiled into the
binary serving this query; governance cannot exceed it.

This share applies to fees the protocol collected, not to transferred value.
Sending 100 HASH delivers 100 HASH.`,
		Args: cobra.NoArgs,
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

func cmdRevenue() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "revenue",
		Short: "Show accrued, paid and pending Founder revenue",
		Long: `Show accrued, paid and pending Founder revenue.

  total_accrued  every uhash ever credited to the Founder
  total_paid     every uhash ever pushed to a beneficiary address
  pending        currently held in the founder module account

pending is read from the module account's real bank balance, so it can be
cross-checked independently:

  hashgramd query bank balances $(hashgramd query founder revenue -o json | jq -r .module_account)`,
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			clientCtx, err := client.GetClientQueryContext(cmd)
			if err != nil {
				return err
			}
			res, err := types.NewQueryClient(clientCtx).Revenue(cmd.Context(), &types.QueryRevenueRequest{})
			if err != nil {
				return err
			}
			return clientCtx.PrintProto(res)
		},
	}
	flags.AddQueryFlagsToCmd(cmd)
	return cmd
}

func cmdBeneficiaryHistory() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "beneficiary-history",
		Short: "Show every recorded change of Founder beneficiary",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			clientCtx, err := client.GetClientQueryContext(cmd)
			if err != nil {
				return err
			}
			res, err := types.NewQueryClient(clientCtx).BeneficiaryHistory(cmd.Context(),
				&types.QueryBeneficiaryHistoryRequest{})
			if err != nil {
				return err
			}
			return clientCtx.PrintProto(res)
		},
	}
	flags.AddQueryFlagsToCmd(cmd)
	return cmd
}

// GetTxCmd returns the x/founder transaction commands.
func GetTxCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:                        types.ModuleName,
		Short:                      "Founder revenue transactions",
		DisableFlagParsing:         true,
		SuggestionsMinimumDistance: 2,
		RunE:                       client.ValidateCmd,
	}
	cmd.AddCommand(cmdClaim())
	return cmd
}

func cmdClaim() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "claim",
		Short: "Push pending Founder revenue to the configured beneficiary",
		Long: `Push pending Founder revenue to the configured beneficiary.

This transaction is permissionless. Any account may send it, it pays the gas,
and the revenue always goes to the beneficiary configured on chain. The
sender has no influence on the destination.

That is the point: the Founder never has to keep a private key on a Hashgram
server in order to receive revenue, and a compromised server can at most
trigger a payout to the correct address.`,
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			clientCtx, err := client.GetClientTxContext(cmd)
			if err != nil {
				return err
			}
			msg := &types.MsgClaimFounderRevenue{Sender: clientCtx.GetFromAddress().String()}
			return tx.GenerateOrBroadcastTxCLI(clientCtx, cmd.Flags(), msg)
		},
	}
	flags.AddTxFlagsToCmd(cmd)
	return cmd
}
