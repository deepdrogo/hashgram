package cmd

import (
	"fmt"
	"strings"

	"github.com/spf13/cobra"

	hgparams "github.com/hashgram/hashgram/app/params"
	"github.com/hashgram/hashgram/cmd/hashgramctl/internal/hgconfig"
	"github.com/hashgram/hashgram/cmd/hashgramctl/internal/hgrpc"
	"github.com/hashgram/hashgram/cmd/hashgramctl/internal/hgsys"
)

func cmdChainStatus() *cobra.Command {
	return &cobra.Command{
		Use:   "chain-status",
		Short: "Show the chain's identity, height, validators and supply",
		Long: `Show the chain's identity, height, validators and supply.

The supply line is worth checking after launch and periodically thereafter.
Hashgram has no minting module, so the total supply must equal 1,000,000,000
HASH forever. A different figure would mean something is badly wrong.`,
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			ctx, cancel := commandContext()
			defer cancel()

			o := newOut()

			network, netErr := hgconfig.LoadNetwork(paths)
			if netErr == nil {
				o.row("Network", network.NetworkName)
				o.row("Network id", network.NetworkID)
				o.row("Chain id", network.ChainID)
				o.row("Genesis hash", network.GenesisHash)
			}

			status, err := rpc.Status(ctx)
			if err != nil {
				return fmt.Errorf("the node RPC is unreachable: %w\n\n"+
					"Is the node running?  hashgramctl status", err)
			}

			o.row("Height", status.SyncInfo.LatestBlockHeight)
			o.row("Latest block", status.SyncInfo.LatestBlockTime)
			o.row("Latest block hash", shortHash(strings.ToLower(status.SyncInfo.LatestBlockHash)))
			o.row("App hash", shortHash(strings.ToLower(status.SyncInfo.LatestAppHash)))
			o.row("Earliest height", status.SyncInfo.EarliestHeight)
			o.row("Catching up", status.SyncInfo.CatchingUp)

			if net, err := rpc.NetInfo(ctx); err == nil {
				o.row("Peers", net.PeerCount())
			}

			// --- validators ---------------------------------------------

			var validators struct {
				Validators []struct {
					OperatorAddress string `json:"operator_address"`
					Jailed          bool   `json:"jailed"`
					Status          string `json:"status"`
					Tokens          string `json:"tokens"`
					Description     struct {
						Moniker string `json:"moniker"`
					} `json:"description"`
				} `json:"validators"`
			}
			if err := queryJSON(ctx, &validators, "query", "staking", "validators"); err == nil {
				bonded := 0
				jailed := 0
				for _, v := range validators.Validators {
					if v.Status == "BOND_STATUS_BONDED" {
						bonded++
					}
					if v.Jailed {
						jailed++
					}
				}
				o.row("Validators", fmt.Sprintf("%d total, %d bonded, %d jailed",
					len(validators.Validators), bonded, jailed))

				// The resilience milestone from the specification: four
				// appropriately distributed validators.
				switch {
				case bonded == 1:
					o.row("  resilience", "1 validator: this is a bootstrap single point of "+
						"availability, not a decentralised network")
				case bonded < 4:
					o.row("  resilience", fmt.Sprintf(
						"%d validators: below the 4-validator operational milestone", bonded))
				default:
					o.row("  resilience", fmt.Sprintf(
						"%d validators: at or above the 4-validator milestone", bonded))
				}
			}

			// --- supply --------------------------------------------------

			var supply struct {
				Supply []struct {
					Denom  string `json:"denom"`
					Amount string `json:"amount"`
				} `json:"supply"`
			}
			if err := queryJSON(ctx, &supply, "query", "bank", "total"); err == nil {
				for _, c := range supply.Supply {
					if c.Denom != hgparams.BaseCoinDenom {
						o.row("Supply ("+c.Denom+")", c.Amount)
						continue
					}
					o.row("Total supply", commasStr(c.Amount)+" "+c.Denom)

					expected := hgparams.MaxSupplyBase().String()
					if c.Amount == expected {
						o.row("  fixed supply", fmt.Sprintf(
							"correct: exactly %s HASH, and there is no minting module",
							commas(hgparams.MaxSupplyHash)))
					} else {
						o.row("  fixed supply", fmt.Sprintf(
							"UNEXPECTED: %s, expected %s. Hashgram has no minting module, so this "+
								"should be impossible", c.Amount, expected))
					}
				}
			}

			// --- founder -------------------------------------------------

			var founder struct {
				Params struct {
					Beneficiary    string `json:"beneficiary"`
					FeeBasisPoints uint32 `json:"fee_basis_points"`
				} `json:"params"`
				MaxFeeBasisPoints uint32 `json:"max_fee_basis_points"`
			}
			if err := queryJSON(ctx, &founder, "query", "founder", "params"); err == nil {
				o.section("Founder")
				o.row("  Beneficiary", orNone(founder.Params.Beneficiary))
				o.row("  Revenue share", fmt.Sprintf("%d bps (%.2f%%), ceiling %d bps",
					founder.Params.FeeBasisPoints,
					float64(founder.Params.FeeBasisPoints)/100,
					founder.MaxFeeBasisPoints))
			}

			o.section("Services")
			roles, _ := hgconfig.LoadRoles(paths)
			for _, unit := range unitsForRoles(roles) {
				state := hgsys.Inspect(ctx, unit)
				if state.Exists {
					o.row("  "+unit, state.Active)
				}
			}

			return o.flush()
		},
	}
}

func cmdWalletInfo() *cobra.Command {
	var address string

	cmd := &cobra.Command{
		Use:   "wallet-info [address]",
		Short: "Show an address's balance, vesting and delegations",
		Long: `Show an address's balance, vesting and delegations.

This is a read-only query and needs no key. It is how the Founder verifies
the 200,000,000 HASH allocation and its vesting schedule without bringing a
private key anywhere near this server:

  hashgramctl wallet-info hash1...

See docs/FOUNDER_LAUNCH_RUNBOOK.md Part D.`,
		Args: cobra.MaximumNArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			ctx, cancel := commandContext()
			defer cancel()

			if len(args) == 1 {
				address = args[0]
			}
			if address == "" {
				return fmt.Errorf("an address is required: hashgramctl wallet-info hash1...")
			}

			o := newOut()
			o.row("Address", address)

			var balances struct {
				Balances []struct {
					Denom  string `json:"denom"`
					Amount string `json:"amount"`
				} `json:"balances"`
			}
			if err := queryJSON(ctx, &balances, "query", "bank", "balances", address); err != nil {
				return err
			}
			if len(balances.Balances) == 0 {
				o.row("Balance", "0")
			}
			for _, b := range balances.Balances {
				o.row("Balance", commasStr(b.Amount)+" "+b.Denom)
				if b.Denom == hgparams.BaseCoinDenom {
					o.row("  in HASH", humanHash(b.Amount))
				}
			}

			var spendable struct {
				Balance struct {
					Denom  string `json:"denom"`
					Amount string `json:"amount"`
				} `json:"balance"`
			}
			if err := queryJSON(ctx, &spendable, "query", "bank", "spendable-balance",
				address, hgparams.BaseCoinDenom); err == nil && spendable.Balance.Amount != "" {
				o.row("Spendable now", commasStr(spendable.Balance.Amount)+" "+spendable.Balance.Denom)
				o.row("  in HASH", humanHash(spendable.Balance.Amount))
				o.raw("")
				o.raw("  The difference between balance and spendable is locked by vesting.")
			}

			// The account type reveals a vesting schedule.
			var account map[string]any
			if err := queryJSON(ctx, &account, "query", "auth", "account", address); err == nil {
				if t, ok := account["type"].(string); ok {
					o.row("Account type", t)
					if strings.Contains(t, "Vesting") {
						o.raw("")
						o.raw("  This is a vesting account. Full details:")
						o.raw("    hashgramd query auth account " + address + " -o json")
					}
				}
			}

			var delegations struct {
				DelegationResponses []struct {
					Delegation struct {
						ValidatorAddress string `json:"validator_address"`
					} `json:"delegation"`
					Balance struct {
						Amount string `json:"amount"`
					} `json:"balance"`
				} `json:"delegation_responses"`
			}
			if err := queryJSON(ctx, &delegations, "query", "staking", "delegations", address); err == nil {
				if len(delegations.DelegationResponses) > 0 {
					o.section("Delegations")
					for _, d := range delegations.DelegationResponses {
						o.row("  "+d.Delegation.ValidatorAddress, commasStr(d.Balance.Amount)+" uhash")
					}
				}
			}

			var names struct {
				Registrations []struct {
					Name string `json:"name"`
				} `json:"registrations"`
			}
			if err := queryJSON(ctx, &names, "query", "username", "reverse", address); err == nil {
				if len(names.Registrations) > 0 {
					var list []string
					for _, r := range names.Registrations {
						list = append(list, "@"+r.Name)
					}
					o.row("Usernames", strings.Join(list, " "))
				}
			}

			return o.flush()
		},
	}

	cmd.Flags().StringVar(&address, "address", "", "address to inspect")
	return cmd
}

func cmdRewards() *cobra.Command {
	var operator string

	cmd := &cobra.Command{
		Use:   "rewards [operator-address]",
		Short: "Show useful-service credit and earnings for this node",
		Long: `Show useful-service credit and earnings for this node.

  current            credit accrued in the open epoch, not yet paid
  effective_credit   what will actually be paid on, after the
                     per-counterparty concentration discount
  lifetime_paid      everything this node has earned

If effective_credit is much lower than the raw figures, most of the credit
came from a single counterparty and was discounted. That is the mechanism
that makes two nodes trading fake traffic unprofitable; see
docs/SERVICE_REWARDS.md.`,
		Args: cobra.MaximumNArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			ctx, cancel := commandContext()
			defer cancel()

			if len(args) == 1 {
				operator = args[0]
			}

			// Without an operator argument, ask the running P2P node: it
			// knows its own operator key and its pending evidence.
			var nodeView map[string]any
			if node := hgrpc.NewNodeAPI(flagNodeAPI); node.Reachable(ctx) {
				if v, err := node.Rewards(ctx); err == nil {
					nodeView = v
					if operator == "" {
						if op, ok := v["operator"].(string); ok {
							operator = op
						}
					}
				}
			}
			if operator == "" {
				return fmt.Errorf(
					"an operator address is required: hashgramctl rewards hash1...\n\n" +
						"This is the address that registered the provider, not the reward address.\n" +
						"When hashgram-node runs with an operator key it is found automatically.")
			}

			o := newOut()

			var rewards map[string]any
			if err := queryJSON(ctx, &rewards, "query", "serviceproof", "rewards", operator); err != nil {
				return fmt.Errorf("%w\n\nIs this node registered as a useful-service provider?\n"+
					"  hashgramd query serviceproof provider %s", err, operator)
			}

			if flagJSON {
				o.set("rewards", rewards)
				if nodeView != nil {
					o.set("node", nodeView)
				}
				return o.flush()
			}

			o.row("Operator", operator)
			if nodeView != nil {
				o.section("This node's agent")
				if v, ok := nodeView["reward_address"]; ok && v != "" {
					o.row("  reward address", v)
				}
				o.row("  epoch", nodeView["epoch"])
				o.row("  pending receipts", nodeView["pending_receipts"])
				o.row("  assigner", nodeView["is_assigner"])
				if st, ok := nodeView["stats"].(map[string]any); ok {
					for _, k := range []string{
						"receipts_accepted", "receipts_refused", "receipts_settled", "receipts_rejected",
						"challenges_answered", "challenges_failed", "assignments_made",
					} {
						o.row("  "+k, st[k])
					}
				}
			}
			if current, ok := rewards["current"].(map[string]any); ok {
				o.section("Open epoch")
				for _, k := range []string{
					"epoch", "storage_credit", "relay_credit", "retrieval_credit",
					"call_credit", "challenges_issued", "challenges_passed", "distinct_clients",
				} {
					if v, ok := current[k]; ok {
						o.row("  "+k, v)
					}
				}
			}
			if v, ok := rewards["effective_credit"]; ok {
				o.row("  effective credit", v)
			}
			if v, ok := rewards["largest_client_credit"]; ok {
				o.row("  largest counterparty", v)
			}
			if v, ok := rewards["lifetime_paid"]; ok {
				o.section("Lifetime")
				o.row("  paid", fmt.Sprintf("%v", v))
			}

			var reserve map[string]any
			if err := queryJSON(ctx, &reserve, "query", "serviceproof", "reserve"); err == nil {
				o.section("Reward reserve")
				if v, ok := reserve["remaining"]; ok {
					o.row("  remaining", fmt.Sprintf("%v", v))
				}
			}

			return o.flush()
		},
	}

	cmd.Flags().StringVar(&operator, "operator", "", "provider operator address")
	return cmd
}

func cmdStorage() *cobra.Command {
	var provider string

	cmd := &cobra.Command{
		Use:   "storage [provider-address]",
		Short: "Show storage assignments, challenges and local disk usage",
		Long: `Show storage assignments, challenges and local disk usage.

Note the distinction the chain draws: declared storage is advertising, and
assigned storage is what actually earns. A node with a terabyte declared and
nothing assigned earns nothing, by design.

Open challenges need answering before their deadline. A missed challenge
counts as a failure, not as a non-event.`,
		Args: cobra.MaximumNArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			ctx, cancel := commandContext()
			defer cancel()

			if len(args) == 1 {
				provider = args[0]
			}

			o := newOut()

			roles, _ := hgconfig.LoadRoles(paths)
			if roles.DeclaredStorageBytes > 0 {
				o.row("Declared storage", hgsys.HumanBytes(roles.DeclaredStorageBytes))
				o.raw("  (advertising: it caps what may be assigned, it is not what is paid)")
			}

			if free, total, err := hgsys.DiskUsage(paths.DataDir); err == nil {
				o.row("Local disk free", hgsys.HumanBytes(free))
				o.row("Local disk total", hgsys.HumanBytes(total))
			}
			if used, err := hgsys.DirSize(paths.NodeHome); err == nil {
				o.row("Node home size", hgsys.HumanBytes(used))
			}

			if node := hgrpc.NewNodeAPI(flagNodeAPI); node.Reachable(ctx) {
				if st, err := node.Status(ctx); err == nil {
					if st.Blobs != nil {
						o.section("Blob store (hashgram-node)")
						for _, k := range []string{"blobs_complete", "blobs_partial", "bytes_used", "quota_bytes", "degraded", "store_peers"} {
							o.row("  "+k, st.Blobs[k])
						}
						if d, ok := st.Blobs["degraded"].(float64); ok && d > 0 {
							o.raw("  DEGRADED: some blobs have fewer than 3 known replicas. One copy is one copy.")
						}
					}
					if st.Mailbox != nil {
						o.section("Mailbox store (hashgram-node)")
						for _, k := range []string{"envelopes", "mailboxes", "key_packages", "bytes"} {
							o.row("  "+k, st.Mailbox[k])
						}
					}
					if provider == "" {
						if rv, err := node.Rewards(ctx); err == nil {
							if op, ok := rv["operator"].(string); ok {
								provider = op
							}
						}
					}
				}
			}

			if provider == "" {
				o.blank()
				o.raw("Pass a provider address to see on-chain assignments and challenges.")
				return o.flush()
			}

			var assignments map[string]any
			if err := queryJSON(ctx, &assignments, "query", "serviceproof",
				"assignments", provider); err == nil {
				o.section("On-chain assignments")
				if list, ok := assignments["assignments"].([]any); ok {
					o.row("  count", len(list))
					var totalBytes uint64
					active := 0
					for _, item := range list {
						a, ok := item.(map[string]any)
						if !ok {
							continue
						}
						if isTrue(a["active"]) {
							active++
							totalBytes += parseUint(a["size_bytes"])
						}
					}
					o.row("  active", active)
					o.row("  assigned bytes", hgsys.HumanBytes(totalBytes))
				}
			}

			var challenges map[string]any
			if err := queryJSON(ctx, &challenges, "query", "serviceproof",
				"challenges", provider); err == nil {
				o.section("Open storage challenges")
				if list, ok := challenges["challenges"].([]any); ok {
					o.row("  open", len(list))
					if len(list) > 0 {
						o.raw("  These need answering before their deadline. A missed challenge")
						o.raw("  counts as a failure and adds to the fraud score.")
					}
				}
			}

			return o.flush()
		},
	}

	cmd.Flags().StringVar(&provider, "provider", "", "provider operator address")
	return cmd
}

func cmdValidator() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "validator",
		Short: "Show this machine's validator status and signing safety",
		Long: `Show this machine's validator status and signing safety.

The consensus key on this machine is the one thing that can cause a slashable
double-sign. Two rules, both absolute:

  - Never copy priv_validator_key.json to a second machine.
  - Never restore it from a backup onto a machine while the original is
    still running.

For production, move the key off the public-facing host entirely behind a
remote signer, with sentry nodes in front. See docs/SECURITY.md.`,
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			ctx, cancel := commandContext()
			defer cancel()

			o := newOut()

			status, err := rpc.Status(ctx)
			if err != nil {
				o.row("RPC", "unreachable: "+err.Error())
			} else {
				o.row("Consensus address", status.ValidatorInfo.Address)
				o.row("Voting power", status.VotingPower())
				o.row("In active set", status.IsValidator())
			}

			o.section("Local key files")
			o.row("  priv_validator_key.json", presence(paths.ValidatorKeyFile()))
			o.row("  priv_validator_state.json",
				presence(strings.Replace(paths.ValidatorKeyFile(), "key.json", "state.json", 1)))

			if fileExists(paths.ValidatorKeyFile()) {
				mode, err := hgsys.FileMode(paths.ValidatorKeyFile())
				if err == nil && mode&0o077 != 0 {
					o.row("  WARNING", fmt.Sprintf(
						"mode %04o allows group or world access to the consensus signing key; "+
							"run chmod 600", mode))
				}
			}

			var signing map[string]any
			if err := queryJSON(ctx, &signing, "query", "slashing", "signing-infos"); err == nil {
				if infos, ok := signing["info"].([]any); ok {
					o.section("Signing info")
					o.row("  validators tracked", len(infos))
				}
			}

			o.section("Signing safety")
			o.raw("  Never copy priv_validator_key.json to a second machine.")
			o.raw("  Never restore it onto a machine while the original is still running.")
			o.raw("  Two validators signing with one key is a double-sign: it is slashable")
			o.raw("  and it is the one operational mistake here that cannot be undone.")

			return o.flush()
		},
	}
	return cmd
}

func orNone(s string) string {
	if s == "" {
		return "NOT SET"
	}
	return s
}

func humanHash(baseAmount string) string {
	// Integer string division by 10^6, avoiding float rounding on figures
	// that can reach 10^15.
	if len(baseAmount) <= hgparams.CoinDecimals {
		return "0." + strings.Repeat("0", hgparams.CoinDecimals-len(baseAmount)) + baseAmount + " HASH"
	}
	split := len(baseAmount) - hgparams.CoinDecimals
	whole := baseAmount[:split]
	frac := strings.TrimRight(baseAmount[split:], "0")
	if frac == "" {
		return commasStr(whole) + " HASH"
	}
	return commasStr(whole) + "." + frac + " HASH"
}

func isTrue(v any) bool {
	b, ok := v.(bool)
	return ok && b
}

func parseUint(v any) uint64 {
	switch t := v.(type) {
	case string:
		var n uint64
		_, _ = fmt.Sscanf(t, "%d", &n)
		return n
	case float64:
		return uint64(t)
	}
	return 0
}
