package cmd

import (
	"encoding/json"
	"fmt"
	"math/big"
	"strings"

	"github.com/spf13/cobra"

	hgparams "github.com/hashgram/hashgram/app/params"
	networktypes "github.com/hashgram/hashgram/x/network/types"
)

func parseBig(s string) (*big.Int, bool) {
	n := new(big.Int)
	_, ok := n.SetString(strings.TrimSpace(s), 10)
	return n, ok
}

func cmdStaking() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "staking",
		Short: "Delegate, and check delegations and rewards",
		RunE:  func(c *cobra.Command, _ []string) error { return c.Help() },
	}

	status := &cobra.Command{
		Use:   "status",
		Short: "Show the validator set and bonded stake",
		Args:  cobra.NoArgs,
		RunE: func(c *cobra.Command, _ []string) error {
			out, err := hashgramd(queryArgs("query", "staking", "validators")...)
			if err != nil {
				return err
			}

			var parsed struct {
				Validators []struct {
					OperatorAddress string `json:"operator_address"`
					Jailed          bool   `json:"jailed"`
					Status          string `json:"status"`
					Tokens          string `json:"tokens"`
					Description     struct {
						Moniker string `json:"moniker"`
					} `json:"description"`
					Commission struct {
						CommissionRates struct {
							Rate string `json:"rate"`
						} `json:"commission_rates"`
					} `json:"commission"`
				} `json:"validators"`
			}
			if err := json.Unmarshal([]byte(out), &parsed); err != nil {
				return err
			}

			w := c.OutOrStdout()
			fmt.Fprintf(w, "\n  %-20s %-16s %-8s %s\n", "MONIKER", "TOKENS", "JAILED", "STATUS")
			bonded := 0
			for _, v := range parsed.Validators {
				if v.Status == "BOND_STATUS_BONDED" {
					bonded++
				}
				fmt.Fprintf(w, "  %-20s %-16s %-8v %s\n",
					truncate(v.Description.Moniker, 20), v.Tokens, v.Jailed,
					strings.TrimPrefix(v.Status, "BOND_STATUS_"))
			}
			fmt.Fprintf(w, "\n  %d validators, %d bonded\n", len(parsed.Validators), bonded)

			switch {
			case bonded == 1:
				fmt.Fprintf(w, "\n  One validator. This is a bootstrap single point of availability,\n")
				fmt.Fprintf(w, "  not a decentralised network. Stopping it stops block production.\n")
			case bonded < 4:
				fmt.Fprintf(w, "\n  %d validators: below the four-validator operational milestone.\n", bonded)
			default:
				fmt.Fprintf(w, "\n  %d validators: at or above the four-validator milestone.\n", bonded)
			}
			fmt.Fprintln(w)
			return nil
		},
	}

	delegate := &cobra.Command{
		Use:   "delegate [validator-address] [amount]",
		Short: "Delegate stake to a validator",
		Args:  cobra.ExactArgs(2),
		RunE: func(c *cobra.Command, args []string) error {
			from, _ := c.Flags().GetString("from")
			if from == "" {
				return fmt.Errorf("--from is required")
			}
			out, err := hashgramd(txArgs(from, "tx", "staking", "delegate", args[0], args[1])...)
			if err != nil {
				return err
			}
			fmt.Fprintln(c.OutOrStdout(), out)
			return nil
		},
	}
	delegate.Flags().String("from", "", "delegating key name")

	rewards := &cobra.Command{
		Use:   "rewards [delegator-address]",
		Short: "Show staking rewards",
		Long: `Show staking rewards.

On Hashgram these come from transaction fees, not from inflation: there is no
minting module. A quiet chain therefore pays validators very little, which is
by design and is why useful-service rewards exist alongside staking.`,
		Args: cobra.ExactArgs(1),
		RunE: func(c *cobra.Command, args []string) error {
			out, err := hashgramd(queryArgs("query", "distribution", "rewards", args[0])...)
			if err != nil {
				return err
			}
			fmt.Fprintln(c.OutOrStdout(), out)
			fmt.Fprintf(c.OutOrStdout(),
				"\n  These rewards come from transaction fees. Hashgram has no minting module,\n"+
					"  so a quiet chain pays little; useful-service rewards exist alongside\n"+
					"  staking for exactly that reason.\n\n")
			return nil
		},
	}

	cmd.AddCommand(status, delegate, rewards)
	return cmd
}

func cmdFounder() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "founder",
		Short: "Verify the Founder allocation, vesting and revenue",
		Long: `Verify the Founder allocation, vesting and revenue.

Every command here is read-only and needs no key. This is how the Founder
checks their position without bringing a private key anywhere near a server.`,
		RunE: func(c *cobra.Command, _ []string) error { return c.Help() },
	}

	verify := &cobra.Command{
		Use:   "verify",
		Short: "Check the Founder configuration and allocation end to end",
		Long: `Check the Founder configuration and allocation end to end.

Answers, from chain state alone:

  Where are my 200,000,000 HASH?     the balance at the beneficiary address
  How much is locked?                 balance minus spendable
  Where does my 1% go?                the configured beneficiary
  How much has accrued and been paid? the revenue ledger

See docs/FOUNDER_LAUNCH_RUNBOOK.md Part D.`,
		Args: cobra.NoArgs,
		RunE: func(c *cobra.Command, _ []string) error {
			w := c.OutOrStdout()

			paramsOut, err := hashgramd(queryArgs("query", "founder", "params")...)
			if err != nil {
				return err
			}
			var params struct {
				Params struct {
					Beneficiary        string `json:"beneficiary"`
					FeeBasisPoints     uint32 `json:"fee_basis_points"`
					PayoutPeriodBlocks string `json:"payout_period_blocks"`
				} `json:"params"`
				MaxFeeBasisPoints uint32 `json:"max_fee_basis_points"`
			}
			if err := json.Unmarshal([]byte(paramsOut), &params); err != nil {
				return err
			}

			fmt.Fprintf(w, "\n  FOUNDER CONFIGURATION\n")
			fmt.Fprintf(w, "  Beneficiary       %s\n", orNone(params.Params.Beneficiary))
			fmt.Fprintf(w, "  Revenue share     %d bps (%.2f%%)\n",
				params.Params.FeeBasisPoints, float64(params.Params.FeeBasisPoints)/100)
			fmt.Fprintf(w, "  Ceiling           %d bps; governance cannot exceed this without a\n",
				params.MaxFeeBasisPoints)
			fmt.Fprintf(w, "                    new binary the validator set adopts\n")
			fmt.Fprintf(w, "  Payout period     every %s blocks\n", params.Params.PayoutPeriodBlocks)

			if params.Params.Beneficiary == "" {
				fmt.Fprintf(w, "\n  The beneficiary is NOT SET. The Founder share is not being taken at\n")
				fmt.Fprintf(w, "  all, and fixing it now requires a governance proposal.\n\n")
				return nil
			}

			addr := params.Params.Beneficiary

			fmt.Fprintf(w, "\n  ALLOCATION\n")
			balance := balanceOf(addr)
			fmt.Fprintf(w, "  Balance           %s uhash  (%s)\n", balance, humanHash(balance))

			spendableOut, err := hashgramd(queryArgs("query", "bank", "spendable-balance",
				addr, hgparams.BaseCoinDenom)...)
			if err == nil {
				var sp struct {
					Balance struct {
						Amount string `json:"amount"`
					} `json:"balance"`
				}
				if json.Unmarshal([]byte(spendableOut), &sp) == nil && sp.Balance.Amount != "" {
					fmt.Fprintf(w, "  Spendable now     %s uhash  (%s)\n",
						sp.Balance.Amount, humanHash(sp.Balance.Amount))
					locked := subtractStrings(balance, sp.Balance.Amount)
					fmt.Fprintf(w, "  Locked (vesting)  %s uhash  (%s)\n", locked, humanHash(locked))
				}
			}

			expected := hgparams.HashToBase(hgparams.AllocFounderHash).String()
			if balance == expected {
				fmt.Fprintf(w, "\n  Correct: exactly %s HASH, the documented Founder allocation.\n",
					commas(hgparams.AllocFounderHash))
			} else {
				fmt.Fprintf(w, "\n  Balance is %s uhash; the genesis allocation was %s uhash.\n",
					balance, expected)
				fmt.Fprintf(w, "  A difference is expected if funds have been moved since genesis.\n")
			}

			revenueOut, err := hashgramd(queryArgs("query", "founder", "revenue")...)
			if err == nil {
				var revenue struct {
					TotalAccrued []struct {
						Denom  string `json:"denom"`
						Amount string `json:"amount"`
					} `json:"total_accrued"`
					TotalPaid []struct {
						Denom  string `json:"denom"`
						Amount string `json:"amount"`
					} `json:"total_paid"`
					Pending []struct {
						Denom  string `json:"denom"`
						Amount string `json:"amount"`
					} `json:"pending"`
					ModuleAccount    string `json:"module_account"`
					LastPayoutHeight string `json:"last_payout_height"`
				}
				if json.Unmarshal([]byte(revenueOut), &revenue) == nil {
					fmt.Fprintf(w, "\n  REVENUE LEDGER\n")
					fmt.Fprintf(w, "  Total accrued     %s\n", coinList(revenue.TotalAccrued))
					fmt.Fprintf(w, "  Total paid        %s\n", coinList(revenue.TotalPaid))
					fmt.Fprintf(w, "  Pending           %s\n", coinList(revenue.Pending))
					fmt.Fprintf(w, "  Held at           %s\n", revenue.ModuleAccount)
					fmt.Fprintf(w, "  Last payout       height %s\n", revenue.LastPayoutHeight)
					fmt.Fprintf(w, "\n  Cross-check the pending figure without trusting this module:\n")
					fmt.Fprintf(w, "    hashgramd query bank balances %s\n", revenue.ModuleAccount)
				}
			}

			historyOut, err := hashgramd(queryArgs("query", "founder", "beneficiary-history")...)
			if err == nil {
				var history struct {
					Changes []struct {
						Height              string `json:"height"`
						PreviousBeneficiary string `json:"previous_beneficiary"`
						NewBeneficiary      string `json:"new_beneficiary"`
					} `json:"changes"`
				}
				if json.Unmarshal([]byte(historyOut), &history) == nil && len(history.Changes) > 0 {
					fmt.Fprintf(w, "\n  BENEFICIARY HISTORY\n")
					for _, ch := range history.Changes {
						from := ch.PreviousBeneficiary
						if from == "" {
							from = "(genesis)"
						}
						fmt.Fprintf(w, "  height %-10s %s -> %s\n", ch.Height, from, ch.NewBeneficiary)
					}
					fmt.Fprintf(w, "\n  Any change here required a governance proposal. A silent swap is not\n")
					fmt.Fprintf(w, "  possible: the full chain of beneficiaries is recorded.\n")
				}
			}

			fmt.Fprintf(w, "\n  The Founder private key is not required for any of the above, and is\n")
			fmt.Fprintf(w, "  not required to run the network. See docs/FOUNDER_LAUNCH_RUNBOOK.md.\n\n")
			return nil
		},
	}

	claim := &cobra.Command{
		Use:   "claim",
		Short: "Trigger a Founder revenue payout (any account may do this)",
		Long: `Trigger a Founder revenue payout.

Permissionless on purpose: any account can send this, it pays the gas, and
the revenue always goes to the beneficiary configured on chain. The sender
has no influence on the destination.

That is what makes "the Founder key is never needed on a server" true in
practice. A compromised server can at most trigger a payout to the correct
address.`,
		Args: cobra.NoArgs,
		RunE: func(c *cobra.Command, _ []string) error {
			from, _ := c.Flags().GetString("from")
			if from == "" {
				return fmt.Errorf("--from is required (any funded account will do)")
			}
			out, err := hashgramd(txArgs(from, "tx", "founder", "claim")...)
			if err != nil {
				return err
			}
			fmt.Fprintln(c.OutOrStdout(), out)
			return nil
		},
	}
	claim.Flags().String("from", "", "any funded key name; it pays the gas only")

	cmd.AddCommand(verify, claim)
	return cmd
}

func cmdSign() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "sign",
		Short: "Show the canonical signing preimage for a protocol object",
		Long: `Show the canonical signing preimage for a protocol object.

Client SDK authors need this. Every signed off-chain object in Hashgram is
signed over a hand-built, length-prefixed preimage wrapped in a
network-specific domain, and an SDK that gets the layout wrong produces
signatures that verify nowhere with no useful error.

  hashgram-test-client sign domains

prints every signing purpose and the exact domain string, taken from the
running chain rather than from documentation that could drift.`,
		RunE: func(c *cobra.Command, _ []string) error { return c.Help() },
	}

	domains := &cobra.Command{
		Use:   "domains",
		Short: "List every signing purpose and its exact domain string",
		Args:  cobra.NoArgs,
		RunE: func(c *cobra.Command, _ []string) error {
			w := c.OutOrStdout()

			fmt.Fprintf(w, "\n  SIGNING DOMAINS\n\n")
			fmt.Fprintf(w, "  Preimage layout:\n    %s\n\n", networktypes.PreimageLayout)
			fmt.Fprintf(w, "  The digest that is actually signed is SHA-256 over that preimage.\n\n")

			fmt.Fprintf(w, "  %-26s %s\n", "PURPOSE", "DOMAIN")
			for _, purpose := range networktypes.SigningPurposes() {
				out, err := hashgramd(queryArgs("query", "network",
					"signing-domain", string(purpose))...)
				if err != nil {
					fmt.Fprintf(w, "  %-26s (node unreachable)\n", purpose)
					continue
				}
				var parsed struct {
					Domain string `json:"domain"`
				}
				if json.Unmarshal([]byte(out), &parsed) == nil {
					fmt.Fprintf(w, "  %-26s %s\n", purpose, parsed.Domain)
				}
			}

			fmt.Fprintf(w, "\n  Every domain embeds the protocol version and the network id, so a\n")
			fmt.Fprintf(w, "  signature made on one Hashgram network does not verify on another,\n")
			fmt.Fprintf(w, "  and an object signed for one purpose does not verify as another.\n\n")
			return nil
		},
	}

	cmd.AddCommand(domains)
	return cmd
}

func coinList(coins []struct {
	Denom  string `json:"denom"`
	Amount string `json:"amount"`
}) string {
	if len(coins) == 0 {
		return "0"
	}
	parts := make([]string, 0, len(coins))
	for _, c := range coins {
		parts = append(parts, c.Amount+c.Denom)
	}
	return strings.Join(parts, ", ")
}

func orNone(s string) string {
	if s == "" {
		return "NOT SET"
	}
	return s
}

func truncate(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n-1] + "…"
}

func commas(n int64) string {
	s := fmt.Sprintf("%d", n)
	if len(s) <= 3 {
		return s
	}
	out := make([]byte, 0, len(s)+len(s)/3)
	lead := len(s) % 3
	if lead > 0 {
		out = append(out, s[:lead]...)
	}
	for i := lead; i < len(s); i += 3 {
		if len(out) > 0 {
			out = append(out, ',')
		}
		out = append(out, s[i:i+3]...)
	}
	return string(out)
}
