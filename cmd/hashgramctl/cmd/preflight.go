package cmd

import (
	"context"
	"fmt"
	"os"
	"strings"

	"github.com/spf13/cobra"

	hgparams "github.com/hashgram/hashgram/app/params"
	"github.com/hashgram/hashgram/cmd/hashgramctl/internal/hgconfig"
	"github.com/hashgram/hashgram/cmd/hashgramctl/internal/hgsys"
)

// checkResult is one preflight finding.
type checkResult struct {
	Name     string `json:"name"`
	Status   string `json:"status"` // PASS, FAIL, WARN
	Detail   string `json:"detail"`
	Critical bool   `json:"critical"`
}

const (
	statusPass = "PASS"
	statusFail = "FAIL"
	statusWarn = "WARN"
)

// MinFreeDiskBytes is the free space a Mainnet node needs before launch.
//
// 50 GiB is not a guess at final chain size; it is the point below which a
// node will run out of disk during normal operation before an operator
// notices. A node that halts because the disk filled is an availability
// failure that also risks state corruption.
const MinFreeDiskBytes uint64 = 50 * 1024 * 1024 * 1024

func cmdMainnetPreflight() *cobra.Command {
	var skipWarnings bool

	cmd := &cobra.Command{
		Use:   "mainnet-preflight",
		Short: "Run the pre-launch safety checks",
		Long: `Run the pre-launch safety checks.

Every check either PASSes, WARNs, or FAILs. A FAIL on a critical check means
do not launch: the command exits non-zero and says why.

The checks that matter most, and why:

  development credentials   A DEVNET key or a devnet chain-id on what is
                            supposed to be Mainnet means the network identity
                            is wrong, and every later assumption is void.

  genesis hash pinned       Without it this node cannot verify which network
                            it is on, and will refuse Hashgram P2P peers.

  administrative RPC        CometBFT RPC and the app's gRPC bind loopback by
                            default. Bound to 0.0.0.0 they expose node
                            administration to the internet.

  validator key permissions priv_validator_key.json readable by other users
                            on the machine is a double-sign waiting to happen.

  clock synchronisation     A validator with a wrong clock proposes blocks its
                            peers reject, and the symptom looks like a
                            network fault rather than a clock fault.

  founder beneficiary       A Mainnet genesis with no Founder beneficiary
                            means the Founder share is not being taken at
                            all; that is recoverable only by governance.`,
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			ctx, cancel := commandContext()
			defer cancel()

			var results []checkResult
			add := func(r checkResult) { results = append(results, r) }

			// --- network identity ---------------------------------------

			network, netErr := hgconfig.LoadNetwork(paths)
			if netErr != nil {
				add(checkResult{"network configuration", statusFail, netErr.Error(), true})
			} else if err := network.Validate(); err != nil {
				add(checkResult{"network configuration", statusFail, err.Error(), true})
			} else {
				add(checkResult{"network configuration", statusPass,
					fmt.Sprintf("%s (%s)", network.NetworkName, network.ChainID), true})
			}

			add(checkGenesisHash(network, netErr == nil))
			add(checkDevelopmentCredentials(network, netErr == nil))
			add(checkChainID(network, netErr == nil))

			// --- keys and permissions -----------------------------------

			results = append(results, checkValidatorKey()...)
			add(checkNodeKey())

			// --- exposure ------------------------------------------------

			results = append(results, checkAdminExposure(ctx)...)
			add(checkFirewall(ctx))

			// --- host ----------------------------------------------------

			add(checkDisk())
			add(checkClock(ctx))

			// --- chain configuration -------------------------------------

			add(checkFounderBeneficiary())
			add(checkValidatorGentx())

			// --- report ---------------------------------------------------

			return report(cmd, results, skipWarnings)
		},
	}

	cmd.Flags().BoolVar(&skipWarnings, "ignore-warnings", false,
		"exit zero even if there are warnings (critical failures still exit non-zero)")

	return cmd
}

func checkGenesisHash(n hgconfig.Network, loaded bool) checkResult {
	if !loaded {
		return checkResult{"genesis hash pinned", statusFail,
			"no network configuration, so no genesis hash is pinned", true}
	}
	if n.GenesisHash == "" {
		return checkResult{"genesis hash pinned", statusFail,
			"genesis_hash is empty; this node cannot verify which network it is on " +
				"and will refuse Hashgram P2P peers", true}
	}

	raw, err := os.ReadFile(paths.GenesisFile())
	if err != nil {
		return checkResult{"genesis hash pinned", statusFail,
			fmt.Sprintf("cannot read %s: %v", paths.GenesisFile(), err), true}
	}
	actual := hgparams.ComputeGenesisHash(raw)
	if actual != n.GenesisHash {
		return checkResult{"genesis hash pinned", statusFail,
			fmt.Sprintf("the genesis file on disk hashes to %s but this node is pinned to %s; "+
				"one of them is wrong and starting would put this node on the wrong network",
				actual, n.GenesisHash), true}
	}
	return checkResult{"genesis hash pinned", statusPass, n.GenesisHash, true}
}

// checkDevelopmentCredentials is §111 and §112: production Mainnet tooling
// must reject obvious development credentials.
func checkDevelopmentCredentials(n hgconfig.Network, loaded bool) checkResult {
	if !loaded {
		return checkResult{"development credentials", statusWarn,
			"skipped: no network configuration to inspect", false}
	}

	if n.Identity().IsDevnet() {
		return checkResult{"development credentials", statusFail,
			fmt.Sprintf("this node is configured for %s, which is a DEVNET. "+
				"It is not Hashgram Mainnet and must not be operated as though it were",
				n.NetworkID), true}
	}

	// The devnet magic on something claiming to be Mainnet is a
	// misconfiguration serious enough to stop a launch.
	if n.NetworkMagic == string(hgparams.NetworkMagicDevnet[:]) {
		return checkResult{"development credentials", statusFail,
			"the pinned network magic is the DEVNET magic", true}
	}

	// A genesis file containing the DEVNET marker string.
	if raw, err := os.ReadFile(paths.GenesisFile()); err == nil {
		if strings.Contains(string(raw), "DEVNET ONLY") {
			return checkResult{"development credentials", statusFail,
				"the genesis file contains the DEVNET ONLY marker", true}
		}
		if strings.Contains(string(raw), hgparams.ChainIDDevnet) {
			return checkResult{"development credentials", statusFail,
				fmt.Sprintf("the genesis file references the devnet chain-id %q",
					hgparams.ChainIDDevnet), true}
		}
	}

	return checkResult{"development credentials", statusPass,
		"no development identifiers found", true}
}

func checkChainID(n hgconfig.Network, loaded bool) checkResult {
	if !loaded {
		return checkResult{"chain id", statusFail, "no network configuration", true}
	}
	if n.ChainID != hgparams.ChainIDMainnet {
		return checkResult{"chain id", statusFail,
			fmt.Sprintf("chain-id is %q, Hashgram Mainnet is %q",
				n.ChainID, hgparams.ChainIDMainnet), true}
	}
	if n.NetworkID != hgparams.NetworkIDMainnet {
		return checkResult{"chain id", statusFail,
			fmt.Sprintf("network-id is %q, Hashgram Mainnet is %q",
				n.NetworkID, hgparams.NetworkIDMainnet), true}
	}
	return checkResult{"chain id", statusPass, n.ChainID, true}
}

func checkValidatorKey() []checkResult {
	path := paths.ValidatorKeyFile()
	roles, _ := hgconfig.LoadRoles(paths)

	if !fileExists(path) {
		if roles.Has(hgconfig.RoleValidator) {
			return []checkResult{{"validator key", statusFail,
				fmt.Sprintf("this machine is configured as a validator but %s does not exist", path),
				true}}
		}
		return []checkResult{{"validator key", statusPass,
			"absent, and this machine is not configured as a validator", false}}
	}

	mode, err := hgsys.FileMode(path)
	if err != nil {
		return []checkResult{{"validator key permissions", statusFail, err.Error(), true}}
	}

	// 0600 or stricter. Group or world readability on a consensus signing key
	// means any other account on the machine can copy it and double-sign.
	if mode&0o077 != 0 {
		return []checkResult{{"validator key permissions", statusFail,
			fmt.Sprintf("%s is mode %04o; it must not be readable by group or others. "+
				"Fix with: chmod 600 %s", path, mode, path), true}}
	}

	return []checkResult{{"validator key permissions", statusPass,
		fmt.Sprintf("%s is mode %04o", path, mode), true}}
}

func checkNodeKey() checkResult {
	path := paths.NodeKeyFile()
	if !fileExists(path) {
		return checkResult{"node key", statusFail,
			fmt.Sprintf("%s does not exist; run 'hashgramctl init' first", path), true}
	}
	mode, err := hgsys.FileMode(path)
	if err != nil {
		return checkResult{"node key", statusFail, err.Error(), true}
	}
	if mode&0o077 != 0 {
		return checkResult{"node key permissions", statusWarn,
			fmt.Sprintf("%s is mode %04o; 600 is preferred", path, mode), false}
	}
	return checkResult{"node key permissions", statusPass, fmt.Sprintf("mode %04o", mode), false}
}

// adminPorts are the ports that must not be reachable from outside the host.
var adminPorts = map[string]string{
	"26657": "CometBFT RPC (includes administrative endpoints)",
	"9090":  "application gRPC",
	"1317":  "application REST API",
	"26660": "Prometheus metrics",
	"5432":  "PostgreSQL",
}

func checkAdminExposure(ctx context.Context) []checkResult {
	sockets, err := hgsys.ListeningSockets(ctx)
	if err != nil {
		return []checkResult{{"administrative RPC exposure", statusWarn, err.Error(), false}}
	}

	var findings []checkResult
	exposed := false

	for _, socket := range sockets {
		host, port, ok := splitHostPort(socket)
		if !ok {
			continue
		}
		description, watched := adminPorts[port]
		if !watched {
			continue
		}
		if isLoopback(host) {
			continue
		}
		exposed = true
		findings = append(findings, checkResult{
			Name:   "administrative RPC exposure",
			Status: statusFail,
			Detail: fmt.Sprintf("%s is listening on %s, which is not loopback. "+
				"Bind it to 127.0.0.1 and put a reverse proxy in front if you want public "+
				"read access", description, socket),
			Critical: true,
		})
	}

	if !exposed {
		findings = append(findings, checkResult{"administrative RPC exposure", statusPass,
			"no administrative port is bound outside loopback", true})
	}
	return findings
}

func checkFirewall(ctx context.Context) checkResult {
	active, detail := hgsys.FirewallActive(ctx)
	if !active {
		return checkResult{"firewall", statusWarn,
			"ufw is not active. Hashgram expects default-deny with only the required " +
				"ports open; see scripts/install/bootstrap-ubuntu.sh", false}
	}
	first := strings.SplitN(detail, "\n", 2)[0]
	return checkResult{"firewall", statusPass, first, false}
}

func checkDisk() checkResult {
	free, total, err := hgsys.DiskUsage(paths.NodeHome)
	if err != nil {
		return checkResult{"disk space", statusWarn,
			fmt.Sprintf("could not inspect %s: %v", paths.NodeHome, err), false}
	}
	detail := fmt.Sprintf("%s free of %s at %s",
		hgsys.HumanBytes(free), hgsys.HumanBytes(total), paths.NodeHome)

	if free < MinFreeDiskBytes {
		return checkResult{"disk space", statusFail,
			fmt.Sprintf("%s; at least %s is required. A node that halts because the disk "+
				"filled is an availability failure that also risks state corruption",
				detail, hgsys.HumanBytes(MinFreeDiskBytes)), true}
	}
	return checkResult{"disk space", statusPass, detail, true}
}

func checkClock(ctx context.Context) checkResult {
	synced, detail := hgsys.ClockSynchronised(ctx)
	if !synced {
		return checkResult{"clock synchronisation", statusFail,
			detail + ". A validator with a wrong clock proposes blocks its peers reject, " +
				"and the symptom looks like a network fault. Enable systemd-timesyncd or chrony",
			true}
	}
	return checkResult{"clock synchronisation", statusPass, detail, true}
}

// checkFounderBeneficiary reads the genesis file rather than the running
// chain, because preflight runs before the chain starts.
func checkFounderBeneficiary() checkResult {
	raw, err := os.ReadFile(paths.GenesisFile())
	if err != nil {
		return checkResult{"founder beneficiary", statusFail,
			fmt.Sprintf("cannot read %s: %v", paths.GenesisFile(), err), true}
	}

	// A substring check rather than a full decode: preflight must work even
	// if the genesis contains a module this binary does not know about,
	// which is exactly the situation during an upgrade.
	text := string(raw)
	if !strings.Contains(text, `"founder"`) {
		return checkResult{"founder beneficiary", statusFail,
			"the genesis file has no founder module state", true}
	}
	if strings.Contains(text, `"beneficiary": ""`) || strings.Contains(text, `"beneficiary":""`) {
		return checkResult{"founder beneficiary", statusFail,
			"the Founder beneficiary is empty. The Founder revenue share will not be " +
				"taken at all, and fixing it after launch requires a governance proposal", true}
	}
	return checkResult{"founder beneficiary", statusPass, "configured", true}
}

func checkValidatorGentx() checkResult {
	roles, _ := hgconfig.LoadRoles(paths)
	if !roles.Has(hgconfig.RoleValidator) {
		return checkResult{"initial validator", statusPass,
			"this machine is not configured as a validator", false}
	}

	raw, err := os.ReadFile(paths.GenesisFile())
	if err != nil {
		return checkResult{"initial validator", statusFail, err.Error(), true}
	}
	if strings.Contains(string(raw), `"gen_txs": []`) ||
		strings.Contains(string(raw), `"gen_txs":[]`) {
		return checkResult{"initial validator", statusFail,
			"the genesis file contains no validator gentxs, so the chain has no validator " +
				"set and will not produce blocks. Run 'hashgramd genesis gentx' and " +
				"'hashgramd genesis collect-gentxs'", true}
	}
	return checkResult{"initial validator", statusPass, "gentxs present in genesis", true}
}

func report(cmd *cobra.Command, results []checkResult, ignoreWarnings bool) error {
	var failed, warned int
	for _, r := range results {
		if r.Status == statusFail {
			failed++
		}
		if r.Status == statusWarn {
			warned++
		}
	}

	if flagJSON {
		o := newOut()
		o.set("checks", results)
		o.set("failed", failed)
		o.set("warnings", warned)
		o.set("result", overall(failed, warned))
		if err := o.flush(); err != nil {
			return err
		}
	} else {
		w := cmd.OutOrStdout()
		fmt.Fprintf(w, "\n%s\n", strings.Repeat("=", 74))
		fmt.Fprintf(w, "  HASHGRAM MAINNET PREFLIGHT\n")
		fmt.Fprintf(w, "%s\n\n", strings.Repeat("=", 74))

		for _, r := range results {
			fmt.Fprintf(w, "  [%s] %s\n", r.Status, r.Name)
			if r.Detail != "" {
				for _, line := range wrap(r.Detail, 66) {
					fmt.Fprintf(w, "         %s\n", line)
				}
			}
		}

		fmt.Fprintf(w, "\n%s\n", strings.Repeat("-", 74))
		fmt.Fprintf(w, "  %d checks, %d failed, %d warnings\n", len(results), failed, warned)
		fmt.Fprintf(w, "  RESULT: %s\n", overall(failed, warned))
		fmt.Fprintf(w, "%s\n\n", strings.Repeat("-", 74))

		if failed > 0 {
			fmt.Fprintf(w, "  Do not launch. Fix the failures above and run this again.\n\n")
		} else if warned > 0 {
			fmt.Fprintf(w, "  Launch is possible, but read the warnings first.\n\n")
		} else {
			fmt.Fprintf(w, "  Ready to launch:  hashgramctl start\n\n")
		}
	}

	if failed > 0 {
		return fmt.Errorf("preflight FAILED: %d critical check(s) did not pass", failed)
	}
	if warned > 0 && !ignoreWarnings {
		return fmt.Errorf("preflight completed with %d warning(s); "+
			"pass --ignore-warnings to exit zero anyway", warned)
	}
	return nil
}

func overall(failed, warned int) string {
	switch {
	case failed > 0:
		return statusFail
	case warned > 0:
		return statusWarn
	default:
		return statusPass
	}
}

func wrap(s string, width int) []string {
	words := strings.Fields(s)
	if len(words) == 0 {
		return nil
	}
	var lines []string
	line := words[0]
	for _, word := range words[1:] {
		if len(line)+1+len(word) > width {
			lines = append(lines, line)
			line = word
			continue
		}
		line += " " + word
	}
	return append(lines, line)
}

func splitHostPort(socket string) (host, port string, ok bool) {
	idx := strings.LastIndex(socket, ":")
	if idx < 0 {
		return "", "", false
	}
	return socket[:idx], socket[idx+1:], true
}

func isLoopback(host string) bool {
	host = strings.Trim(host, "[]")
	switch host {
	case "127.0.0.1", "::1", "localhost":
		return true
	}
	return strings.HasPrefix(host, "127.")
}
