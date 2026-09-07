package cmd

import (
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"strings"

	"github.com/spf13/cobra"

	hgparams "github.com/hashgram/hashgram/app/params"
)

// hashgramd delegation.
//
// The test client shells out to hashgramd for signing and broadcasting rather
// than reimplementing transaction construction. Two reasons: a second
// implementation of tx building is a second place for a signing bug to live,
// and delegating means this tool demonstrates the same path a real client
// would take through the documented CLI.
func hashgramd(args ...string) (string, error) {
	bin := os.Getenv("HASHGRAMD_BIN")
	if bin == "" {
		found, err := exec.LookPath("hashgramd")
		if err != nil {
			for _, candidate := range []string{"/usr/local/bin/hashgramd", "build/hashgramd"} {
				if _, statErr := os.Stat(candidate); statErr == nil {
					found = candidate
					break
				}
			}
		}
		if found == "" {
			return "", fmt.Errorf(
				"hashgramd not found on PATH; set HASHGRAMD_BIN or run 'make install'")
		}
		bin = found
	}

	full := args
	if flagHome != "" {
		full = append([]string{"--home", flagHome}, full...)
	}

	out, err := exec.Command(bin, full...).CombinedOutput()
	text := strings.TrimSpace(string(out))
	if err != nil {
		return text, fmt.Errorf("hashgramd %s: %v\n%s", strings.Join(args, " "), err, text)
	}
	return text, nil
}

func queryArgs(args ...string) []string {
	return append(args, "--node", flagNode, "--output", "json")
}

func txArgs(from string, args ...string) []string {
	out := append(args,
		"--from", from,
		"--node", flagNode,
		"--keyring-backend", flagKeyring,
		"--gas", "auto",
		"--gas-adjustment", "1.4",
		"--gas-prices", "0.0025"+hgparams.BaseCoinDenom,
		"--output", "json",
		"--yes",
	)
	if flagChainID != "" {
		out = append(out, "--chain-id", flagChainID)
	}
	return out
}

func cmdWallet() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "wallet",
		Short: "Create keys, check balances, send HASH and view history",
		RunE:  func(c *cobra.Command, _ []string) error { return c.Help() },
	}

	cmd.AddCommand(
		walletCreate(),
		walletList(),
		walletBalance(),
		walletSend(),
		walletHistory(),
	)
	return cmd
}

func walletCreate() *cobra.Command {
	return &cobra.Command{
		Use:   "create [name]",
		Short: "Create a development key",
		Long: `Create a development key.

With the default 'test' keyring backend the key is stored UNENCRYPTED on
disk. That is appropriate for a devnet and appropriate for nothing else.

Never use this for the Founder key or for any address holding real value.
Use hashgram-keygen on a machine that is not a server.`,
		Args: cobra.ExactArgs(1),
		RunE: func(c *cobra.Command, args []string) error {
			if flagKeyring == "test" {
				fmt.Fprintf(c.OutOrStdout(),
					"note: the 'test' keyring stores this key unencrypted on disk.\n"+
						"      Development only. Do not put real value behind it.\n\n")
			}
			out, err := hashgramd("keys", "add", args[0],
				"--keyring-backend", flagKeyring, "--output", "json")
			if err != nil {
				return err
			}
			fmt.Fprintln(c.OutOrStdout(), out)
			return nil
		},
	}
}

func walletList() *cobra.Command {
	return &cobra.Command{
		Use:   "list",
		Short: "List development keys",
		Args:  cobra.NoArgs,
		RunE: func(c *cobra.Command, _ []string) error {
			out, err := hashgramd("keys", "list",
				"--keyring-backend", flagKeyring, "--output", "json")
			if err != nil {
				return err
			}
			fmt.Fprintln(c.OutOrStdout(), out)
			return nil
		},
	}
}

func walletBalance() *cobra.Command {
	return &cobra.Command{
		Use:   "balance [address]",
		Short: "Show an address's balance and how much of it is spendable",
		Long: `Show an address's balance and how much of it is spendable.

For a vesting account these differ: the balance includes locked coins, and
the spendable figure does not. That is how the Founder allocation is
verified without any key: 200,000,000 HASH held, 20,000,000 spendable at
genesis.`,
		Args: cobra.ExactArgs(1),
		RunE: func(c *cobra.Command, args []string) error {
			address := args[0]

			balances, err := hashgramd(queryArgs("query", "bank", "balances", address)...)
			if err != nil {
				return err
			}

			var parsed struct {
				Balances []struct {
					Denom  string `json:"denom"`
					Amount string `json:"amount"`
				} `json:"balances"`
			}
			if err := json.Unmarshal([]byte(balances), &parsed); err != nil {
				return err
			}

			w := c.OutOrStdout()
			fmt.Fprintf(w, "\n  Address     %s\n", address)
			if len(parsed.Balances) == 0 {
				fmt.Fprintf(w, "  Balance     0\n\n")
				return nil
			}
			for _, b := range parsed.Balances {
				fmt.Fprintf(w, "  Balance     %s %s", b.Amount, b.Denom)
				if b.Denom == hgparams.BaseCoinDenom {
					fmt.Fprintf(w, "  (%s)", humanHash(b.Amount))
				}
				fmt.Fprintln(w)
			}

			spendable, err := hashgramd(queryArgs("query", "bank", "spendable-balance",
				address, hgparams.BaseCoinDenom)...)
			if err == nil {
				var sp struct {
					Balance struct {
						Amount string `json:"amount"`
					} `json:"balance"`
				}
				if json.Unmarshal([]byte(spendable), &sp) == nil && sp.Balance.Amount != "" {
					fmt.Fprintf(w, "  Spendable   %s %s  (%s)\n",
						sp.Balance.Amount, hgparams.BaseCoinDenom, humanHash(sp.Balance.Amount))
					fmt.Fprintf(w, "\n  The difference between balance and spendable is locked by vesting.\n")
				}
			}
			fmt.Fprintln(w)
			return nil
		},
	}
}

func walletSend() *cobra.Command {
	var from string

	cmd := &cobra.Command{
		Use:   "send [to-address] [amount]",
		Short: "Send HASH and show that the recipient receives the full amount",
		Long: `Send HASH and show that the recipient receives the full amount.

This is the transaction the specification is most specific about: if you send
100 HASH, the recipient receives 100 HASH. The Founder revenue share applies
to protocol fee revenue, not to transferred value, so nothing is deducted
from the principal.

  hashgram-test-client wallet send hash1... 100000000uhash --from alice

The command prints the recipient's balance before and after, so the property
is visible rather than asserted.`,
		Args: cobra.ExactArgs(2),
		RunE: func(c *cobra.Command, args []string) error {
			to, amount := args[0], args[1]
			if from == "" {
				return fmt.Errorf("--from is required")
			}

			w := c.OutOrStdout()

			before := balanceOf(to)
			fmt.Fprintf(w, "\n  Recipient before   %s uhash\n", before)

			out, err := hashgramd(txArgs(from, "tx", "bank", "send",
				resolveKey(from), to, amount)...)
			if err != nil {
				return err
			}

			var res struct {
				TxHash string `json:"txhash"`
				Code   int    `json:"code"`
				RawLog string `json:"raw_log"`
			}
			if json.Unmarshal([]byte(out), &res) == nil {
				fmt.Fprintf(w, "  Transaction        %s\n", res.TxHash)
				if res.Code != 0 {
					return fmt.Errorf("transaction failed with code %d: %s", res.Code, res.RawLog)
				}
			}

			// Wait for inclusion so the after-balance is meaningful rather
			// than racing the block.
			if res.TxHash != "" {
				if _, err := hashgramd(queryArgs("query", "wait-tx", res.TxHash)...); err != nil {
					fmt.Fprintf(w, "  (could not wait for inclusion: %v)\n", err)
				}
			}

			after := balanceOf(to)
			fmt.Fprintf(w, "  Recipient after    %s uhash\n", after)

			delta := subtractStrings(after, before)
			fmt.Fprintf(w, "  Received           %s uhash\n\n", delta)

			sent := strings.TrimSuffix(amount, hgparams.BaseCoinDenom)
			if delta == sent {
				fmt.Fprintf(w, "  The recipient received exactly what was sent. No transfer tax was\n")
				fmt.Fprintf(w, "  applied: the Founder revenue share is a share of protocol fee\n")
				fmt.Fprintf(w, "  revenue, not of transferred value.\n\n")
			} else {
				fmt.Fprintf(w, "  NOTE: received %s but %s was sent. If these differ, something is\n", delta, sent)
				fmt.Fprintf(w, "  wrong: Hashgram takes nothing from transferred principal.\n\n")
			}
			return nil
		},
	}

	cmd.Flags().StringVar(&from, "from", "", "sending key name or address")
	return cmd
}

func walletHistory() *cobra.Command {
	return &cobra.Command{
		Use:   "history [address]",
		Short: "Show an address's transaction history",
		Args:  cobra.ExactArgs(1),
		RunE: func(c *cobra.Command, args []string) error {
			out, err := hashgramd(queryArgs("query", "txs",
				"--query", fmt.Sprintf("message.sender='%s'", args[0]),
				"--limit", "50")...)
			if err != nil {
				return err
			}
			fmt.Fprintln(c.OutOrStdout(), out)
			return nil
		},
	}
}

// resolveKey passes a key name straight through; hashgramd's --from accepts
// either a name or an address, so no lookup is needed.
func resolveKey(name string) string { return name }

func balanceOf(address string) string {
	out, err := hashgramd(queryArgs("query", "bank", "balances", address)...)
	if err != nil {
		return "0"
	}
	var parsed struct {
		Balances []struct {
			Denom  string `json:"denom"`
			Amount string `json:"amount"`
		} `json:"balances"`
	}
	if json.Unmarshal([]byte(out), &parsed) != nil {
		return "0"
	}
	for _, b := range parsed.Balances {
		if b.Denom == hgparams.BaseCoinDenom {
			return b.Amount
		}
	}
	return "0"
}

// subtractStrings subtracts two decimal integer strings without going
// through float64, which cannot represent the whole uhash supply exactly.
func subtractStrings(a, b string) string {
	x, ok1 := parseBig(a)
	y, ok2 := parseBig(b)
	if !ok1 || !ok2 {
		return "?"
	}
	return x.Sub(x, y).String()
}

func humanHash(baseAmount string) string {
	if len(baseAmount) <= hgparams.CoinDecimals {
		return "0." + strings.Repeat("0", hgparams.CoinDecimals-len(baseAmount)) + baseAmount + " HASH"
	}
	split := len(baseAmount) - hgparams.CoinDecimals
	whole := baseAmount[:split]
	frac := strings.TrimRight(baseAmount[split:], "0")
	if frac == "" {
		return whole + " HASH"
	}
	return whole + "." + frac + " HASH"
}
