// Package cli provides the x/username commands. The availability query
// reports confusable collisions as unavailable, not merely taken names.
package cli

import (
	"strconv"

	"github.com/spf13/cobra"

	"github.com/cosmos/cosmos-sdk/client"
	"github.com/cosmos/cosmos-sdk/client/flags"
	"github.com/cosmos/cosmos-sdk/client/tx"

	"github.com/hashgram/hashgram/x/username/types"
)

// GetQueryCmd returns the x/username query commands.
func GetQueryCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:                        types.ModuleName,
		Short:                      "Query the @username namespace",
		DisableFlagParsing:         true,
		SuggestionsMinimumDistance: 2,
		RunE:                       client.ValidateCmd,
	}
	cmd.AddCommand(cmdParams(), cmdLookup(), cmdReverse(), cmdAvailability(), cmdList())
	return cmd
}

func cmdParams() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "params",
		Short: "Show the username configuration and reserved names",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			ctx, err := client.GetClientQueryContext(cmd)
			if err != nil {
				return err
			}
			res, err := types.NewQueryClient(ctx).Params(cmd.Context(), &types.QueryParamsRequest{})
			if err != nil {
				return err
			}
			return ctx.PrintProto(res)
		},
	}
	flags.AddQueryFlagsToCmd(cmd)
	return cmd
}

func cmdLookup() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "lookup [@name]",
		Short: "Resolve a username to its owner",
		Long: `Resolve a username to its owner.

The leading '@' is optional. The name is normalised before lookup, so
"@Alice", "alice" and the fullwidth form all resolve to the same
registration; the response reports the normalised form actually used.

A name that has expired past its grace period reports found=false, because
resolving a lapsed name to its former owner would be actively misleading.`,
		Args: cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			ctx, err := client.GetClientQueryContext(cmd)
			if err != nil {
				return err
			}
			res, err := types.NewQueryClient(ctx).Lookup(cmd.Context(),
				&types.QueryLookupRequest{Name: args[0]})
			if err != nil {
				return err
			}
			return ctx.PrintProto(res)
		},
	}
	flags.AddQueryFlagsToCmd(cmd)
	return cmd
}

func cmdReverse() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "reverse [address]",
		Short: "List the usernames an address owns",
		Args:  cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			ctx, err := client.GetClientQueryContext(cmd)
			if err != nil {
				return err
			}
			res, err := types.NewQueryClient(ctx).ReverseLookup(cmd.Context(),
				&types.QueryReverseLookupRequest{Owner: args[0]})
			if err != nil {
				return err
			}
			return ctx.PrintProto(res)
		},
	}
	flags.AddQueryFlagsToCmd(cmd)
	return cmd
}

func cmdAvailability() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "availability [@name]",
		Short: "Check whether a username can be registered, and if not, why",
		Long: `Check whether a username can be registered, and if not, why.

Check this before registering: the reason field distinguishes "taken" from
"reserved" from "confusable_with", and in the confusable case
conflicting_name names the existing registration your request looks like.
Registration fees are not refunded for a rejected request.

Possible reasons:
  taken                  already registered
  reserved               on the governance-managed reserved list
  confusable_with        folds to the same skeleton as an existing name
  mixed_script           mixes scripts, e.g. Latin with Cyrillic
  non_ascii_not_allowed  outside the ASCII namespace
  too_short / too_long   outside the configured length bounds
  invalid                contains a disallowed character`,
		Args: cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			ctx, err := client.GetClientQueryContext(cmd)
			if err != nil {
				return err
			}
			res, err := types.NewQueryClient(ctx).Availability(cmd.Context(),
				&types.QueryAvailabilityRequest{Name: args[0]})
			if err != nil {
				return err
			}
			return ctx.PrintProto(res)
		},
	}
	flags.AddQueryFlagsToCmd(cmd)
	return cmd
}

func cmdList() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "list",
		Short: "List registrations",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			ctx, err := client.GetClientQueryContext(cmd)
			if err != nil {
				return err
			}
			pageReq, err := client.ReadPageRequest(cmd.Flags())
			if err != nil {
				return err
			}
			res, err := types.NewQueryClient(ctx).Registrations(cmd.Context(),
				&types.QueryRegistrationsRequest{Pagination: pageReq})
			if err != nil {
				return err
			}
			return ctx.PrintProto(res)
		},
	}
	flags.AddPaginationFlagsToCmd(cmd, "registrations")
	flags.AddQueryFlagsToCmd(cmd)
	return cmd
}

// GetTxCmd returns the x/username transaction commands.
func GetTxCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:                        types.ModuleName,
		Short:                      "Username transactions",
		DisableFlagParsing:         true,
		SuggestionsMinimumDistance: 2,
		RunE:                       client.ValidateCmd,
	}
	cmd.AddCommand(cmdRegister(), cmdRenew(), cmdTransfer(), cmdLock(), cmdRelease())
	return cmd
}

func cmdRegister() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "register [@name]",
		Short: "Register a username",
		Long: `Register a username.

Check 'query username availability' first: a rejected registration still
costs gas, and the registration fee is only charged on success.`,
		Args: cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			ctx, err := client.GetClientTxContext(cmd)
			if err != nil {
				return err
			}
			msg := &types.MsgRegister{Owner: ctx.GetFromAddress().String(), Name: args[0]}
			return tx.GenerateOrBroadcastTxCLI(ctx, cmd.Flags(), msg)
		},
	}
	flags.AddTxFlagsToCmd(cmd)
	return cmd
}

func cmdRenew() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "renew [@name]",
		Short: "Renew a username registration",
		Long: `Renew a username registration.

Renewing early does not forfeit remaining time: the new expiry extends from
the later of the current expiry and the current height.`,
		Args: cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			ctx, err := client.GetClientTxContext(cmd)
			if err != nil {
				return err
			}
			msg := &types.MsgRenew{Owner: ctx.GetFromAddress().String(), Name: args[0]}
			return tx.GenerateOrBroadcastTxCLI(ctx, cmd.Flags(), msg)
		},
	}
	flags.AddTxFlagsToCmd(cmd)
	return cmd
}

func cmdTransfer() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "transfer [@name] [new-owner]",
		Short: "Transfer a username to another address",
		Args:  cobra.ExactArgs(2),
		RunE: func(cmd *cobra.Command, args []string) error {
			ctx, err := client.GetClientTxContext(cmd)
			if err != nil {
				return err
			}
			msg := &types.MsgTransfer{
				Owner:    ctx.GetFromAddress().String(),
				Name:     args[0],
				NewOwner: args[1],
			}
			return tx.GenerateOrBroadcastTxCLI(ctx, cmd.Flags(), msg)
		},
	}
	flags.AddTxFlagsToCmd(cmd)
	return cmd
}

func cmdLock() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "set-transferable [@name] [true|false]",
		Short: "Lock or unlock a username against transfer",
		Long: `Lock or unlock a username against transfer.

Locking is worth doing for a name that matters. An attacker who steals your
key cannot move a locked name, and unlocking is itself a transaction you can
see on chain.`,
		Args: cobra.ExactArgs(2),
		RunE: func(cmd *cobra.Command, args []string) error {
			ctx, err := client.GetClientTxContext(cmd)
			if err != nil {
				return err
			}
			transferable, err := strconv.ParseBool(args[1])
			if err != nil {
				return err
			}
			msg := &types.MsgSetTransferable{
				Owner:        ctx.GetFromAddress().String(),
				Name:         args[0],
				Transferable: transferable,
			}
			return tx.GenerateOrBroadcastTxCLI(ctx, cmd.Flags(), msg)
		},
	}
	flags.AddTxFlagsToCmd(cmd)
	return cmd
}

func cmdRelease() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "release [@name]",
		Short: "Give up a username, returning it to circulation",
		Args:  cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			ctx, err := client.GetClientTxContext(cmd)
			if err != nil {
				return err
			}
			msg := &types.MsgRelease{Owner: ctx.GetFromAddress().String(), Name: args[0]}
			return tx.GenerateOrBroadcastTxCLI(ctx, cmd.Flags(), msg)
		},
	}
	flags.AddTxFlagsToCmd(cmd)
	return cmd
}
