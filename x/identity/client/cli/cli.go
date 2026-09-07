package cli

import (
	"encoding/base64"
	"encoding/hex"
	"fmt"
	"strings"

	"github.com/spf13/cobra"

	"github.com/cosmos/cosmos-sdk/client"
	"github.com/cosmos/cosmos-sdk/client/flags"

	"github.com/hashgram/hashgram/x/identity/types"
)

// GetQueryCmd returns the x/identity query commands.
func GetQueryCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:                        types.ModuleName,
		Short:                      "Query decentralised identities and their devices",
		DisableFlagParsing:         true,
		SuggestionsMinimumDistance: 2,
		RunE:                       client.ValidateCmd,
	}
	cmd.AddCommand(cmdIdentity(), cmdDevices(), cmdDevice(), cmdResolveKey(), cmdRecovery())
	return cmd
}

func cmdIdentity() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "identity [address]",
		Short: "Show a root identity",
		Long: `Show a root identity.

Only public keys appear in the response. The chain never receives, stores or
transports a private key, and no Hashgram server holds one.`,
		Args: cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			ctx, err := client.GetClientQueryContext(cmd)
			if err != nil {
				return err
			}
			res, err := types.NewQueryClient(ctx).Identity(cmd.Context(),
				&types.QueryIdentityRequest{Address: args[0]})
			if err != nil {
				return err
			}
			return ctx.PrintProto(res)
		},
	}
	flags.AddQueryFlagsToCmd(cmd)
	return cmd
}

func cmdDevices() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "devices [address]",
		Short: "List an identity's devices",
		Long: `List an identity's devices.

Pass --include-revoked when checking whether a historical signature was valid
at the time it was made: a revoked device's events remain attributable, and
the record is kept for exactly that reason.`,
		Args: cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			ctx, err := client.GetClientQueryContext(cmd)
			if err != nil {
				return err
			}
			includeRevoked, _ := cmd.Flags().GetBool("include-revoked")
			res, err := types.NewQueryClient(ctx).Devices(cmd.Context(),
				&types.QueryDevicesRequest{Address: args[0], IncludeRevoked: includeRevoked})
			if err != nil {
				return err
			}
			return ctx.PrintProto(res)
		},
	}
	cmd.Flags().Bool("include-revoked", false, "include revoked devices")
	flags.AddQueryFlagsToCmd(cmd)
	return cmd
}

func cmdDevice() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "device [address] [device-id]",
		Short: "Show one device and its root-signed certificate",
		Args:  cobra.ExactArgs(2),
		RunE: func(cmd *cobra.Command, args []string) error {
			ctx, err := client.GetClientQueryContext(cmd)
			if err != nil {
				return err
			}
			res, err := types.NewQueryClient(ctx).Device(cmd.Context(),
				&types.QueryDeviceRequest{Address: args[0], DeviceId: args[1]})
			if err != nil {
				return err
			}
			return ctx.PrintProto(res)
		},
	}
	flags.AddQueryFlagsToCmd(cmd)
	return cmd
}

func cmdResolveKey() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "resolve-device-key [pubkey]",
		Short: "Find the identity a device public key belongs to",
		Long: `Find the identity a device public key belongs to.

This is the query a client makes on receiving a signed message or social
event: it establishes which identity authorised the signing device, rather
than trusting the sender's own claim about who they are.

The key may be given as base64 or hex.`,
		Args: cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			ctx, err := client.GetClientQueryContext(cmd)
			if err != nil {
				return err
			}
			key, err := decodeKey(args[0])
			if err != nil {
				return err
			}
			res, err := types.NewQueryClient(ctx).ResolveDeviceKey(cmd.Context(),
				&types.QueryResolveDeviceKeyRequest{DevicePubkey: key})
			if err != nil {
				return err
			}
			return ctx.PrintProto(res)
		},
	}
	flags.AddQueryFlagsToCmd(cmd)
	return cmd
}

func cmdRecovery() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "recovery [address]",
		Short: "Show an identity's in-progress recovery request",
		Long: `Show an identity's in-progress recovery request.

If you did not initiate this, cancel it before executable_height. The delay
exists precisely so that a user whose guardians have been socially engineered
can notice and refuse.`,
		Args: cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			ctx, err := client.GetClientQueryContext(cmd)
			if err != nil {
				return err
			}
			res, err := types.NewQueryClient(ctx).Recovery(cmd.Context(),
				&types.QueryRecoveryRequest{RootAddress: args[0]})
			if err != nil {
				return err
			}
			return ctx.PrintProto(res)
		},
	}
	flags.AddQueryFlagsToCmd(cmd)
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
