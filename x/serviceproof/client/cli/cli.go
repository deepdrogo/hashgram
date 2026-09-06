package cli

import (
	"encoding/base64"
	"encoding/hex"
	"fmt"
	"strconv"
	"strings"

	"github.com/spf13/cobra"

	"github.com/cosmos/cosmos-sdk/client"
	"github.com/cosmos/cosmos-sdk/client/flags"
	"github.com/cosmos/cosmos-sdk/client/tx"
	sdk "github.com/cosmos/cosmos-sdk/types"

	"github.com/hashgram/hashgram/x/serviceproof/types"
)

// GetQueryCmd returns the x/serviceproof query commands.
func GetQueryCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:                        types.ModuleName,
		Short:                      "Query useful-service providers, evidence and rewards",
		DisableFlagParsing:         true,
		SuggestionsMinimumDistance: 2,
		RunE:                       client.ValidateCmd,
	}
	cmd.AddCommand(
		cmdParams(), cmdReserve(), cmdProvider(), cmdProviders(),
		cmdCurrentEpoch(), cmdEpoch(), cmdRewards(),
		cmdAssignments(), cmdChallenges(), cmdFraud(), cmdEmissionSchedule(),
	)
	return cmd
}

func queryCmd(use, short, long string, args cobra.PositionalArgs, run func(*cobra.Command, []string, client.Context) error) *cobra.Command {
	cmd := &cobra.Command{
		Use:   use,
		Short: short,
		Long:  long,
		Args:  args,
		RunE: func(cmd *cobra.Command, a []string) error {
			clientCtx, err := client.GetClientQueryContext(cmd)
			if err != nil {
				return err
			}
			return run(cmd, a, clientCtx)
		},
	}
	flags.AddQueryFlagsToCmd(cmd)
	return cmd
}

func cmdParams() *cobra.Command {
	return queryCmd("params", "Show the useful-service reward configuration", "",
		cobra.NoArgs,
		func(cmd *cobra.Command, _ []string, ctx client.Context) error {
			res, err := types.NewQueryClient(ctx).Params(cmd.Context(), &types.QueryParamsRequest{})
			if err != nil {
				return err
			}
			return ctx.PrintProto(res)
		})
}

func cmdReserve() *cobra.Command {
	return queryCmd("reserve", "Show the finite useful-service reward reserve",
		`Show the finite useful-service reward reserve.

  initial        what the reserve held at genesis: 500,000,000 HASH
  total_emitted  what has been paid out of it
  total_slashed  bond taken for proven fraud and returned to the reserve
  remaining      the reserve module account's live balance

There is no mint on this chain, so the reserve can only shrink as it pays out.
Cross-check remaining independently:

  hashgramd query bank balances $(hashgramd query serviceproof reserve -o json | jq -r .reserve_account)`,
		cobra.NoArgs,
		func(cmd *cobra.Command, _ []string, ctx client.Context) error {
			res, err := types.NewQueryClient(ctx).Reserve(cmd.Context(), &types.QueryReserveRequest{})
			if err != nil {
				return err
			}
			return ctx.PrintProto(res)
		})
}

func cmdProvider() *cobra.Command {
	return queryCmd("provider [operator]", "Show one provider's registration and standing",
		`Show one provider's registration and standing.

Note the difference between declared_storage_bytes, which is advertising, and
total_assigned_bytes, which is what actually accrues rewards.`,
		cobra.ExactArgs(1),
		func(cmd *cobra.Command, a []string, ctx client.Context) error {
			res, err := types.NewQueryClient(ctx).Provider(cmd.Context(),
				&types.QueryProviderRequest{Operator: a[0]})
			if err != nil {
				return err
			}
			return ctx.PrintProto(res)
		})
}

func cmdProviders() *cobra.Command {
	cmd := queryCmd("providers", "List registered providers", "",
		cobra.NoArgs,
		func(cmd *cobra.Command, _ []string, ctx client.Context) error {
			pageReq, err := client.ReadPageRequest(cmd.Flags())
			if err != nil {
				return err
			}
			res, err := types.NewQueryClient(ctx).Providers(cmd.Context(),
				&types.QueryProvidersRequest{Pagination: pageReq})
			if err != nil {
				return err
			}
			return ctx.PrintProto(res)
		})
	flags.AddPaginationFlagsToCmd(cmd, "providers")
	return cmd
}

func cmdCurrentEpoch() *cobra.Command {
	return queryCmd("current-epoch", "Show the open accounting period", "",
		cobra.NoArgs,
		func(cmd *cobra.Command, _ []string, ctx client.Context) error {
			res, err := types.NewQueryClient(ctx).CurrentEpoch(cmd.Context(),
				&types.QueryCurrentEpochRequest{})
			if err != nil {
				return err
			}
			return ctx.PrintProto(res)
		})
}

func cmdEpoch() *cobra.Command {
	return queryCmd("epoch [number]", "Show an epoch by number", "",
		cobra.ExactArgs(1),
		func(cmd *cobra.Command, a []string, ctx client.Context) error {
			n, err := strconv.ParseUint(a[0], 10, 64)
			if err != nil {
				return fmt.Errorf("invalid epoch number %q: %w", a[0], err)
			}
			res, err := types.NewQueryClient(ctx).Epoch(cmd.Context(), &types.QueryEpochRequest{Number: n})
			if err != nil {
				return err
			}
			return ctx.PrintProto(res)
		})
}

func cmdRewards() *cobra.Command {
	return queryCmd("rewards [operator]", "Show a provider's credit and earnings",
		`Show a provider's credit and earnings.

  current        credit accrued in the open epoch, not yet paid
  lifetime_paid  everything this provider has ever earned
  recent         the most recent settled epochs

Credit is broken out per role so that "this node was paid for relay work it
never did" is answerable from state rather than from one opaque number.`,
		cobra.ExactArgs(1),
		func(cmd *cobra.Command, a []string, ctx client.Context) error {
			res, err := types.NewQueryClient(ctx).Rewards(cmd.Context(),
				&types.QueryRewardsRequest{Operator: a[0]})
			if err != nil {
				return err
			}
			return ctx.PrintProto(res)
		})
}

func cmdAssignments() *cobra.Command {
	return queryCmd("assignments [provider]", "Show a provider's storage assignments", "",
		cobra.ExactArgs(1),
		func(cmd *cobra.Command, a []string, ctx client.Context) error {
			res, err := types.NewQueryClient(ctx).Assignments(cmd.Context(),
				&types.QueryAssignmentsRequest{Provider: a[0]})
			if err != nil {
				return err
			}
			return ctx.PrintProto(res)
		})
}

func cmdChallenges() *cobra.Command {
	return queryCmd("challenges [provider]", "Show storage challenges awaiting an answer",
		`Show storage challenges awaiting an answer.

A missed challenge counts as a failure, not as a non-event. A node that
ignores challenges stops earning and accumulates a fraud score.`,
		cobra.ExactArgs(1),
		func(cmd *cobra.Command, a []string, ctx client.Context) error {
			res, err := types.NewQueryClient(ctx).OpenChallenges(cmd.Context(),
				&types.QueryOpenChallengesRequest{Provider: a[0]})
			if err != nil {
				return err
			}
			return ctx.PrintProto(res)
		})
}

func cmdFraud() *cobra.Command {
	return queryCmd("fraud [provider]", "Show a provider's fraud history and score", "",
		cobra.ExactArgs(1),
		func(cmd *cobra.Command, a []string, ctx client.Context) error {
			res, err := types.NewQueryClient(ctx).FraudReports(cmd.Context(),
				&types.QueryFraudReportsRequest{Provider: a[0]})
			if err != nil {
				return err
			}
			return ctx.PrintProto(res)
		})
}

func cmdEmissionSchedule() *cobra.Command {
	cmd := queryCmd("emission-schedule", "Project the declining emission schedule forward",
		`Project the declining emission schedule forward.

Each epoch's budget is a fixed fraction of the reserve that *remains*, so
budgets fall geometrically, the sum converges to the initial reserve without
reaching it, and there is no cliff at which subsidy stops.`,
		cobra.NoArgs,
		func(cmd *cobra.Command, _ []string, ctx client.Context) error {
			n, _ := cmd.Flags().GetUint32("epochs")
			res, err := types.NewQueryClient(ctx).EmissionSchedule(cmd.Context(),
				&types.QueryEmissionScheduleRequest{Epochs: n})
			if err != nil {
				return err
			}
			return ctx.PrintProto(res)
		})
	cmd.Flags().Uint32("epochs", 30, "how many future epochs to project")
	return cmd
}

// ---------------------------------------------------------------------------
// Transactions
// ---------------------------------------------------------------------------

// GetTxCmd returns the x/serviceproof transaction commands.
func GetTxCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:                        types.ModuleName,
		Short:                      "Useful-service provider transactions",
		DisableFlagParsing:         true,
		SuggestionsMinimumDistance: 2,
		RunE:                       client.ValidateCmd,
	}
	cmd.AddCommand(
		cmdRegister(), cmdUnjail(), cmdBeginUnbonding(), cmdWithdrawBond(),
	)
	return cmd
}

func parseRoles(s string) ([]types.ServiceRole, error) {
	var out []types.ServiceRole
	for _, part := range strings.Split(s, ",") {
		switch strings.ToLower(strings.TrimSpace(part)) {
		case "storage":
			out = append(out, types.SERVICE_ROLE_STORAGE)
		case "relay":
			out = append(out, types.SERVICE_ROLE_RELAY)
		case "call":
			out = append(out, types.SERVICE_ROLE_CALL)
		case "media":
			out = append(out, types.SERVICE_ROLE_MEDIA)
		case "":
			continue
		default:
			return nil, fmt.Errorf("unknown role %q; valid roles: storage, relay, call, media", part)
		}
	}
	if len(out) == 0 {
		return nil, fmt.Errorf("at least one role is required")
	}
	return out, nil
}

func cmdRegister() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "register-provider [bond]",
		Short: "Register a useful-service node and post its bond",
		Long: `Register a useful-service node and post its bond.

The bond is what makes fraud expensive. Without it, the cost of being caught
would be the cost of generating a new keypair.

--reward-address should differ from the operator key when the node runs on a
rented server: the node then holds a key that can sign challenge responses but
not move the earnings. See docs/SECURITY.md on key separation.

--declared-storage is advertising, not entitlement. It caps how much data the
network may assign to you; you are paid on what is actually assigned and
survives challenges.

Example:

  hashgramd tx serviceproof register-provider 1000000000uhash \
    --roles relay,storage \
    --node-pubkey <base64> --node-key-type secp256k1 \
    --declared-storage 500000000000 \
    --reward-address hash1... \
    --from operator`,
		Args: cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			clientCtx, err := client.GetClientTxContext(cmd)
			if err != nil {
				return err
			}

			bond, err := sdk.ParseCoinsNormalized(args[0])
			if err != nil {
				return fmt.Errorf("invalid bond %q: %w", args[0], err)
			}

			rolesStr, _ := cmd.Flags().GetString("roles")
			roles, err := parseRoles(rolesStr)
			if err != nil {
				return err
			}

			pubStr, _ := cmd.Flags().GetString("node-pubkey")
			if pubStr == "" {
				return fmt.Errorf("--node-pubkey is required")
			}
			pub, err := decodeKey(pubStr)
			if err != nil {
				return err
			}

			keyTypeStr, _ := cmd.Flags().GetString("node-key-type")
			var keyType types.KeyType
			switch strings.ToLower(keyTypeStr) {
			case "secp256k1":
				keyType = types.KEY_TYPE_SECP256K1
			case "ed25519":
				keyType = types.KEY_TYPE_ED25519
			default:
				return fmt.Errorf("--node-key-type must be secp256k1 or ed25519")
			}

			rewardAddr, _ := cmd.Flags().GetString("reward-address")
			declaredStorage, _ := cmd.Flags().GetUint64("declared-storage")
			declaredBandwidth, _ := cmd.Flags().GetUint64("declared-bandwidth")
			moniker, _ := cmd.Flags().GetString("moniker")

			msg := &types.MsgRegisterProvider{
				Operator:             clientCtx.GetFromAddress().String(),
				RewardAddress:        rewardAddr,
				NodePubkey:           pub,
				NodeKeyType:          keyType,
				Roles:                roles,
				Bond:                 bond,
				DeclaredStorageBytes: declaredStorage,
				DeclaredBandwidthBps: declaredBandwidth,
				Moniker:              moniker,
			}
			return tx.GenerateOrBroadcastTxCLI(clientCtx, cmd.Flags(), msg)
		},
	}
	cmd.Flags().String("roles", "relay", "comma-separated roles: storage, relay, call, media")
	cmd.Flags().String("node-pubkey", "", "node public key, base64 or hex")
	cmd.Flags().String("node-key-type", "ed25519", "node key scheme: secp256k1 or ed25519")
	cmd.Flags().String("reward-address", "", "address that receives rewards (defaults to the operator)")
	cmd.Flags().Uint64("declared-storage", 0, "advertised storage in bytes")
	cmd.Flags().Uint64("declared-bandwidth", 0, "advertised relay bandwidth in bits per second")
	cmd.Flags().String("moniker", "", "human-readable label")
	flags.AddTxFlagsToCmd(cmd)
	return cmd
}

func decodeKey(s string) ([]byte, error) {
	if b, err := base64.StdEncoding.DecodeString(s); err == nil {
		return b, nil
	}
	b, err := hex.DecodeString(strings.TrimPrefix(s, "0x"))
	if err != nil {
		return nil, fmt.Errorf("key is neither valid base64 nor valid hex")
	}
	return b, nil
}

func simpleTxCmd(use, short, long string, build func(from string) sdk.Msg) *cobra.Command {
	cmd := &cobra.Command{
		Use:   use,
		Short: short,
		Long:  long,
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			clientCtx, err := client.GetClientTxContext(cmd)
			if err != nil {
				return err
			}
			return tx.GenerateOrBroadcastTxCLI(clientCtx, cmd.Flags(),
				build(clientCtx.GetFromAddress().String()))
		},
	}
	flags.AddTxFlagsToCmd(cmd)
	return cmd
}

func cmdUnjail() *cobra.Command {
	return simpleTxCmd("unjail", "Request release from jail after the jail period has elapsed",
		`Request release from jail after the jail period has elapsed.

Unjailing resets the fraud score: the slash already happened, and keeping the
score would mean a second jailing on the next single failure.

If the bond fell below the minimum after slashing, top it up with
'tx serviceproof update-provider --additional-bond' first.`,
		func(from string) sdk.Msg { return &types.MsgUnjail{Operator: from} })
}

func cmdBeginUnbonding() *cobra.Command {
	return simpleTxCmd("begin-unbonding", "Stop earning and start the bond withdrawal timer",
		`Stop earning and start the bond withdrawal timer.

Earning stops immediately; the bond stays locked for the unbonding period.
That delay must outlast the window in which fraud could still be discovered,
otherwise an operator could cheat, unbond, and walk away before the evidence
lands.`,
		func(from string) sdk.Msg { return &types.MsgBeginUnbonding{Operator: from} })
}

func cmdWithdrawBond() *cobra.Command {
	return simpleTxCmd("withdraw-bond", "Withdraw a bond whose unbonding period has completed",
		`Withdraw a bond whose unbonding period has completed.

This also removes the provider registration: a provider with no bond has
nothing at stake and must re-register to earn again.`,
		func(from string) sdk.Msg { return &types.MsgWithdrawBond{Operator: from} })
}
