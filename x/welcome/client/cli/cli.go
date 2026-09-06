package cli

import (
	"encoding/base64"
	"encoding/json"
	"fmt"
	"os"

	"github.com/spf13/cobra"

	"github.com/cosmos/cosmos-sdk/client"
	"github.com/cosmos/cosmos-sdk/client/flags"
	"github.com/cosmos/cosmos-sdk/client/tx"

	"github.com/hashgram/hashgram/x/welcome/types"
)

// GetQueryCmd returns the x/welcome query commands.
func GetQueryCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:                        types.ModuleName,
		Short:                      "Query the welcome reward programme",
		DisableFlagParsing:         true,
		SuggestionsMinimumDistance: 2,
		RunE:                       client.ValidateCmd,
	}
	cmd.AddCommand(cmdParams(), cmdStatus(), cmdTiers(), cmdClaim(), cmdClaims())
	return cmd
}

func cmdParams() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "params",
		Short: "Show the welcome configuration and registered attestor set",
		Long: `Show the welcome configuration and registered attestor set.

If attestors is empty, no welcome reward can be claimed by anyone. That is
the intended default: creating a private key must never by itself produce
free HASH.`,
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

func cmdStatus() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "status",
		Short: "Show the next sequence, tier amount, remaining pool and totals",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			clientCtx, err := client.GetClientQueryContext(cmd)
			if err != nil {
				return err
			}
			res, err := types.NewQueryClient(clientCtx).Status(cmd.Context(), &types.QueryStatusRequest{})
			if err != nil {
				return err
			}
			return clientCtx.PrintProto(res)
		},
	}
	flags.AddQueryFlagsToCmd(cmd)
	return cmd
}

func cmdTiers() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "tiers",
		Short: "Show the welcome reward schedule",
		Long: `Show the welcome reward schedule.

              1 ..    10,000  ->  50 HASH
         10,001 ..   100,000  ->   5 HASH
        100,001 .. 1,000,000  ->   1 HASH
      1,000,001 and beyond    ->   0 HASH

These are compile-time constants, not governance parameters. Changing them
requires a new binary that the validator set adopts.`,
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			clientCtx, err := client.GetClientQueryContext(cmd)
			if err != nil {
				return err
			}
			res, err := types.NewQueryClient(clientCtx).Tiers(cmd.Context(), &types.QueryTiersRequest{})
			if err != nil {
				return err
			}
			return clientCtx.PrintProto(res)
		},
	}
	flags.AddQueryFlagsToCmd(cmd)
	return cmd
}

func cmdClaim() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "claim [address]",
		Short: "Show whether an address has received a welcome reward",
		Args:  cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			clientCtx, err := client.GetClientQueryContext(cmd)
			if err != nil {
				return err
			}
			res, err := types.NewQueryClient(clientCtx).Claim(cmd.Context(),
				&types.QueryClaimRequest{Subject: args[0]})
			if err != nil {
				return err
			}
			return clientCtx.PrintProto(res)
		},
	}
	flags.AddQueryFlagsToCmd(cmd)
	return cmd
}

func cmdClaims() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "claims",
		Short: "List welcome claim records",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			clientCtx, err := client.GetClientQueryContext(cmd)
			if err != nil {
				return err
			}
			pageReq, err := client.ReadPageRequest(cmd.Flags())
			if err != nil {
				return err
			}
			res, err := types.NewQueryClient(clientCtx).Claims(cmd.Context(),
				&types.QueryClaimsRequest{Pagination: pageReq})
			if err != nil {
				return err
			}
			return clientCtx.PrintProto(res)
		},
	}
	flags.AddPaginationFlagsToCmd(cmd, "claims")
	flags.AddQueryFlagsToCmd(cmd)
	return cmd
}

// GetTxCmd returns the x/welcome transaction commands.
func GetTxCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:                        types.ModuleName,
		Short:                      "Welcome reward transactions",
		DisableFlagParsing:         true,
		SuggestionsMinimumDistance: 2,
		RunE:                       client.ValidateCmd,
	}
	cmd.AddCommand(cmdSubmitClaim())
	return cmd
}

// attestationFile is the JSON shape an attestor produces and a client submits.
//
// A file rather than a pile of flags: an attestation is a signed object, and
// hand-assembling one from command-line arguments invites a mismatch between
// what was signed and what is submitted.
type attestationFile struct {
	Subject      string `json:"subject"`
	Attestor     string `json:"attestor"`
	Nonce        uint64 `json:"nonce"`
	ExpiryHeight int64  `json:"expiry_height"`
	Method       string `json:"method"`
	Confidence   uint32 `json:"confidence"`
	Signature    string `json:"signature"` // standard base64
}

func cmdSubmitClaim() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "claim [attestation.json]",
		Short: "Submit a signed eligibility attestation and pay its reward",
		Long: `Submit a signed eligibility attestation and pay its reward.

The reward goes to the attestation's subject, never to the transaction
sender. That separation is required in practice: a brand-new user has a zero
balance and cannot pay gas, so somebody else must be able to submit the claim
on their behalf.

The attestation file looks like:

  {
    "subject":       "hash1...",
    "attestor":      "hash1...",
    "nonce":         1,
    "expiry_height": 12345,
    "method":        "device-attestation",
    "confidence":    9000,
    "signature":     "<base64 of the 64-byte signature>"
  }

The signature is over the network-domain-separated digest of the canonical
attestation encoding. Use 'hashgram-test-client welcome sign-attestation' to
produce one, or reproduce the encoding from
docs/CLIENT_CONNECTIVITY_SPEC.md.`,
		Args: cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			clientCtx, err := client.GetClientTxContext(cmd)
			if err != nil {
				return err
			}

			raw, err := os.ReadFile(args[0])
			if err != nil {
				return fmt.Errorf("reading attestation file: %w", err)
			}
			var af attestationFile
			if err := json.Unmarshal(raw, &af); err != nil {
				return fmt.Errorf("parsing attestation file: %w", err)
			}
			sig, err := base64.StdEncoding.DecodeString(af.Signature)
			if err != nil {
				return fmt.Errorf("decoding signature (expected standard base64): %w", err)
			}

			msg := &types.MsgClaimWelcome{
				Sender: clientCtx.GetFromAddress().String(),
				Attestation: types.EligibilityAttestation{
					Subject:      af.Subject,
					Attestor:     af.Attestor,
					Nonce:        af.Nonce,
					ExpiryHeight: af.ExpiryHeight,
					Method:       af.Method,
					Confidence:   af.Confidence,
					Signature:    sig,
				},
			}
			return tx.GenerateOrBroadcastTxCLI(clientCtx, cmd.Flags(), msg)
		},
	}
	flags.AddTxFlagsToCmd(cmd)
	return cmd
}
