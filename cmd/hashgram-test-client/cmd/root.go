// Package cmd implements the hashgram-test-client command tree.
//
// This is a developer tool that exercises the protocol from outside a node,
// so that claims such as a transfer being untaxed are demonstrated against a
// running chain rather than only in unit tests.
package cmd

import (
	"github.com/spf13/cobra"
)

// Global flags.
var (
	flagNode    string
	flagChainID string
	flagHome    string
	flagKeyring string
)

// NewRootCmd builds the hashgram-test-client root command.
func NewRootCmd() *cobra.Command {
	root := &cobra.Command{
		Use:   "hashgram-test-client",
		Short: "Developer client for Hashgram",
		Long: `Developer client for Hashgram.

This tool exists to prove the protocol works from outside the node: that a
wallet can be created and used, that staking and rewards are queryable, and
that the signed off-chain object formats can be produced correctly by a third
party rather than only by the code that verifies them.

That last point is the reason it is a separate binary. A signing format that
only the verifying implementation can produce is a format nobody else can
implement. Every canonical encoding here goes through the same exported
functions the state machine uses, so a client SDK that reproduces what this
tool does will interoperate.

This is a developer tool. It is not the Hashgram wallet, it is not hardened
for end users, and it must not be used to hold the Founder key: use
hashgram-keygen on a machine that is not a server for that.

  wallet      create keys, check balances, send HASH, view history
  staking     delegate, check delegations and rewards
  founder     verify the Founder allocation, vesting and revenue
  serviceproof  sign service receipts and answer storage challenges
  welcome     produce a signed eligibility attestation
  identity    build a root-signed device certificate
  sign        show the canonical signing preimage for any protocol object`,
		SilenceUsage: true,
	}

	root.PersistentFlags().StringVar(&flagNode, "node", "tcp://127.0.0.1:26657",
		"node RPC endpoint")
	root.PersistentFlags().StringVar(&flagChainID, "chain-id", "",
		"chain id (read from the node if empty)")
	root.PersistentFlags().StringVar(&flagHome, "home", "",
		"client home directory for the test keyring")
	root.PersistentFlags().StringVar(&flagKeyring, "keyring-backend", "test",
		"keyring backend; 'test' is unencrypted and is for development only")

	root.AddCommand(
		cmdWallet(),
		cmdStaking(),
		cmdFounder(),
		cmdSign(),
	)

	return root
}
