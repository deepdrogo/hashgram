package cli

import (
	"fmt"
	"strings"

	"github.com/spf13/cobra"

	"github.com/cosmos/cosmos-sdk/client"
	"github.com/cosmos/cosmos-sdk/client/flags"

	"github.com/hashgram/hashgram/x/network/types"
)

// GetQueryCmd returns the x/network query commands.
func GetQueryCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:                        types.ModuleName,
		Short:                      "Query the Hashgram network identity",
		DisableFlagParsing:         true,
		SuggestionsMinimumDistance: 2,
		RunE:                       client.ValidateCmd,
	}

	cmd.AddCommand(
		cmdInfo(),
		cmdForkIsolation(),
		cmdSigningDomain(),
	)
	return cmd
}

func cmdInfo() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "info",
		Short: "Show the on-chain network identity",
		Long: `Show the on-chain network identity.

To verify you are talking to Hashgram Mainnet, all of the following must hold:

  network_id             hashgram-mainnet
  chain_id               hashgram-1
  network_magic          HGM1
  protocol_major_version 1

and, separately, the genesis file must hash to the published Mainnet genesis
hash. This query cannot prove that for you: the hash of the genesis file is
not a value inside the genesis file. Check it yourself:

  sha256sum ~/.hashgram/config/genesis.json`,
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			clientCtx, err := client.GetClientQueryContext(cmd)
			if err != nil {
				return err
			}
			res, err := types.NewQueryClient(clientCtx).Info(cmd.Context(), &types.QueryInfoRequest{})
			if err != nil {
				return err
			}
			return clientCtx.PrintProto(res)
		},
	}
	flags.AddQueryFlagsToCmd(cmd)
	return cmd
}

func cmdForkIsolation() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "fork-isolation",
		Short: "Show which peer-rejection checks are in force",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			clientCtx, err := client.GetClientQueryContext(cmd)
			if err != nil {
				return err
			}
			res, err := types.NewQueryClient(clientCtx).ForkIsolation(cmd.Context(), &types.QueryForkIsolationRequest{})
			if err != nil {
				return err
			}
			return clientCtx.PrintProto(res)
		},
	}
	flags.AddQueryFlagsToCmd(cmd)
	return cmd
}

func cmdSigningDomain() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "signing-domain [purpose]",
		Short: "Show the signature domain-separation string for a purpose",
		Long: fmt.Sprintf(`Show the signature domain-separation string for a purpose.

Client SDKs must reproduce this string byte for byte, and must hash the
preimage layout the response reports. Getting either wrong shows up as
signatures that verify nowhere.

Valid purposes: %s`, strings.Join(types.SigningPurposeStrings(), ", ")),
		Args: cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			clientCtx, err := client.GetClientQueryContext(cmd)
			if err != nil {
				return err
			}
			res, err := types.NewQueryClient(clientCtx).SigningDomain(cmd.Context(),
				&types.QuerySigningDomainRequest{Purpose: args[0]})
			if err != nil {
				return err
			}
			return clientCtx.PrintProto(res)
		},
	}
	flags.AddQueryFlagsToCmd(cmd)
	return cmd
}
