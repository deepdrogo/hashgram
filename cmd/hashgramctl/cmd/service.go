package cmd

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"

	"github.com/spf13/cobra"

	"github.com/hashgram/hashgram/cmd/hashgramctl/internal/hgconfig"
	"github.com/hashgram/hashgram/cmd/hashgramctl/internal/hgsys"
)

// unitsForRoles returns the systemd units a role set implies.
//
// Roles map to units rather than the other way round, so that an operator
// changing roles does not have to know which units exist. The chain node runs
// for every role because every role needs a verified view of the chain: a
// relay that cannot check the network identity cannot safely relay.
func unitsForRoles(roles hgconfig.Roles) []string {
	units := []string{hgsys.UnitChain}

	if roles.Has(hgconfig.RoleRelay) || roles.Has(hgconfig.RoleStore) ||
		roles.Has(hgconfig.RoleMedia) || roles.Has(hgconfig.RoleBootstrap) {
		units = append(units, hgsys.UnitNode)
	}
	if roles.Has(hgconfig.RoleIndexer) {
		units = append(units, hgsys.UnitIndexer)
	}
	if roles.Has(hgconfig.RoleSafety) {
		units = append(units, hgsys.UnitSafety)
	}
	if roles.Has(hgconfig.RoleCall) {
		units = append(units, hgsys.UnitCall)
	}
	return units
}

func cmdStart() *cobra.Command {
	return &cobra.Command{
		Use:   "start",
		Short: "Start the Hashgram services for this machine's roles",
		Long: `Start the Hashgram services for this machine's roles.

Which services start depends on the configured roles. The chain node starts
for every role, because every role needs a verified view of the chain: a
relay that cannot check the network identity cannot safely relay.`,
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			ctx, cancel := commandContext()
			defer cancel()

			roles, err := hgconfig.LoadRoles(paths)
			if err != nil {
				return err
			}

			o := newOut()
			for _, unit := range unitsForRoles(roles) {
				state := hgsys.Inspect(ctx, unit)
				if !state.Exists {
					o.row(unit, "not installed (skipped)")
					continue
				}
				if state.Running() {
					o.row(unit, "already running")
					continue
				}
				if err := hgsys.Start(ctx, unit); err != nil {
					o.row(unit, "FAILED: "+err.Error())
					continue
				}
				o.row(unit, "started")
			}
			return o.flush()
		},
	}
}

func cmdStop() *cobra.Command {
	return &cobra.Command{
		Use:   "stop",
		Short: "Stop the Hashgram services on this machine",
		Long: `Stop the Hashgram services on this machine.

Stopping a validator stops signing. If this is the only validator, the chain
stops producing blocks until it comes back. That is a real consequence of
running a single-validator network and is documented in
docs/DECENTRALIZATION.md.`,
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			ctx, cancel := commandContext()
			defer cancel()

			roles, err := hgconfig.LoadRoles(paths)
			if err != nil {
				return err
			}

			units := unitsForRoles(roles)
			// Stopped in reverse dependency order so the chain node, which
			// the others read from, goes last.
			o := newOut()
			for i := len(units) - 1; i >= 0; i-- {
				unit := units[i]
				state := hgsys.Inspect(ctx, unit)
				if !state.Exists || !state.Running() {
					o.row(unit, "not running")
					continue
				}
				if err := hgsys.Stop(ctx, unit); err != nil {
					o.row(unit, "FAILED: "+err.Error())
					continue
				}
				o.row(unit, "stopped")
			}
			return o.flush()
		},
	}
}

func cmdRestart() *cobra.Command {
	return &cobra.Command{
		Use:   "restart",
		Short: "Restart the Hashgram services on this machine",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			ctx, cancel := commandContext()
			defer cancel()

			roles, err := hgconfig.LoadRoles(paths)
			if err != nil {
				return err
			}

			o := newOut()
			for _, unit := range unitsForRoles(roles) {
				state := hgsys.Inspect(ctx, unit)
				if !state.Exists {
					o.row(unit, "not installed (skipped)")
					continue
				}
				if err := hgsys.Restart(ctx, unit); err != nil {
					o.row(unit, "FAILED: "+err.Error())
					continue
				}
				o.row(unit, "restarted")
			}
			return o.flush()
		},
	}
}

func cmdLogs() *cobra.Command {
	var (
		unit   string
		lines  int
		follow bool
	)

	cmd := &cobra.Command{
		Use:   "logs",
		Short: "Show service logs",
		Long: `Show service logs.

Hashgram never logs private message plaintext, so these logs are safe to
share when asking for help. See docs/SECURITY.md on the logging policy.`,
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			// No timeout: --follow is meant to run until interrupted.
			return hgsys.Journal(cmd.Context(), unit, lines, follow)
		},
	}

	cmd.Flags().StringVar(&unit, "unit", hgsys.UnitChain, "systemd unit to read")
	cmd.Flags().IntVarP(&lines, "lines", "n", 200, "how many lines to show")
	cmd.Flags().BoolVarP(&follow, "follow", "f", false, "stream new lines")

	return cmd
}

func cmdConfigureRole() *cobra.Command {
	var (
		rewardAddress   string
		declaredStorage uint64
		declaredBps     uint64
		moniker         string
		restart         bool
	)

	cmd := &cobra.Command{
		Use:   "configure-role [roles]",
		Short: "Set which node roles this machine serves",
		Long: `Set which node roles this machine serves.

  hashgramctl configure-role validator,bootstrap
  hashgramctl configure-role relay,store
  hashgramctl configure-role media
  hashgramctl configure-role indexer,safety

Valid roles: ` + hgconfig.RoleNames() + `

--reward-address should differ from any key held on this machine. A node that
holds a key which can also move its own earnings turns a node compromise into
a theft. See docs/SECURITY.md on key separation.

--declared-storage is advertising, not entitlement. It caps how much data the
network may assign to this node; payment follows what is actually assigned and
survives storage challenges.`,
		Args: cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			ctx, cancel := commandContext()
			defer cancel()

			roles, err := hgconfig.ParseRoles(args[0])
			if err != nil {
				return err
			}

			existing, err := hgconfig.LoadRoles(paths)
			if err != nil {
				return err
			}

			cfg := hgconfig.Roles{
				Roles:                roles,
				RewardAddress:        firstNonEmpty(rewardAddress, existing.RewardAddress),
				DeclaredStorageBytes: firstNonZero(declaredStorage, existing.DeclaredStorageBytes),
				DeclaredBandwidthBps: firstNonZero(declaredBps, existing.DeclaredBandwidthBps),
				Moniker:              firstNonEmpty(moniker, existing.Moniker),
			}

			if err := hgconfig.SaveRoles(paths, cfg); err != nil {
				return err
			}
			if err := writeAppRoles(cfg.String()); err != nil {
				return err
			}

			o := newOut()
			o.row("Roles", cfg.String())
			o.row("Config file", paths.RolesFile())
			if cfg.RewardAddress != "" {
				o.row("Reward address", cfg.RewardAddress)
				_ = writeNodeTomlString("reward_address", cfg.RewardAddress)
			} else if hasEarningRole(cfg) {
				o.row("Reward address", "not set; earnings will go to the operator address")
			}

			// Earning roles need a provider operator key. It is generated by
			// hashgram-node as its own service account, so the key file is
			// owned by the account that uses it and nobody else.
			if hasEarningRole(cfg) {
				if op, err := ensureOperatorKey(ctx); err != nil {
					o.row("Operator key", "not created: "+err.Error())
				} else {
					o.row("Operator address", op)
					o.raw("  Fund it with at least 1,000 HASH for the provider bond plus fees, then set")
					o.raw("  auto_register_provider = true in /etc/hashgram/node.toml and restart. The")
					o.raw("  operator key is a hot key on this machine; the reward address should not be.")
				}
			}
			if cfg.DeclaredStorageBytes > 0 {
				o.row("Declared storage", hgsys.HumanBytes(cfg.DeclaredStorageBytes))
			}
			o.section("Services for these roles")
			for _, unit := range unitsForRoles(cfg) {
				state := hgsys.Inspect(ctx, unit)
				status := "not installed"
				if state.Exists {
					status = state.Active
				}
				o.row("  "+unit, status)
			}

			if restart {
				o.blank()
				for _, unit := range unitsForRoles(cfg) {
					if state := hgsys.Inspect(ctx, unit); state.Exists {
						if err := hgsys.Restart(ctx, unit); err != nil {
							o.row("  restart "+unit, "FAILED: "+err.Error())
							continue
						}
						o.row("  restart "+unit, "done")
					}
				}
			} else {
				o.blank()
				o.raw("Run 'hashgramctl restart' to apply.")
			}

			return o.flush()
		},
	}

	cmd.Flags().StringVar(&rewardAddress, "reward-address", "",
		"address that receives useful-service rewards")
	cmd.Flags().Uint64Var(&declaredStorage, "declared-storage", 0,
		"advertised storage in bytes")
	cmd.Flags().Uint64Var(&declaredBps, "declared-bandwidth", 0,
		"advertised relay bandwidth in bits per second")
	cmd.Flags().StringVar(&moniker, "moniker", "", "human-readable node label")
	cmd.Flags().BoolVar(&restart, "restart", false, "restart the affected services now")

	return cmd
}

// ensureOperatorKey runs `hashgram-node operator-key` as the node's service
// account and returns the operator address. Idempotent.
func ensureOperatorKey(ctx context.Context) (string, error) {
	bin, err := exec.LookPath("hashgram-node")
	if err != nil {
		return "", fmt.Errorf("hashgram-node is not installed")
	}
	home := filepath.Join(paths.DataDir, "node")
	var c *exec.Cmd
	if os.Geteuid() == 0 {
		c = exec.CommandContext(ctx, "runuser", "-u", "hashgram-node", "--", bin, "operator-key", "--home", home)
	} else {
		c = exec.CommandContext(ctx, bin, "operator-key", "--home", home)
	}
	out, err := c.Output()
	if err != nil {
		return "", err
	}
	return strings.TrimSpace(string(out)), nil
}

// writeNodeTomlString sets a string key in node.toml.
func writeNodeTomlString(key, value string) error {
	path := paths.ConfigDir + "/node.toml"
	raw, err := os.ReadFile(path)
	if err != nil {
		return err
	}
	line := fmt.Sprintf("%s = %q", key, value)
	lines := strings.Split(string(raw), "\n")
	replaced := false
	for i, l := range lines {
		t := strings.TrimSpace(l)
		if strings.HasPrefix(t, key+" ") || strings.HasPrefix(t, key+"=") {
			lines[i] = line
			replaced = true
		}
	}
	if !replaced {
		lines = append(lines, line)
	}
	return os.WriteFile(path, []byte(strings.TrimRight(strings.Join(lines, "\n"), "\n")+"\n"), 0o644)
}

func hasEarningRole(r hgconfig.Roles) bool {
	for _, role := range []hgconfig.Role{
		hgconfig.RoleRelay, hgconfig.RoleStore, hgconfig.RoleMedia, hgconfig.RoleCall,
	} {
		if r.Has(role) {
			return true
		}
	}
	return false
}

// writeAppRoles records the role list in app.toml's [hashgram] section, so
// that hashgramd itself can report what this machine is configured for.
func writeAppRoles(roles string) error {
	path := paths.AppConfigFile()
	raw, err := os.ReadFile(path)
	if err != nil {
		if os.IsNotExist(err) {
			// The node has not been initialised yet; the role file is
			// authoritative and app.toml will pick it up at init.
			return nil
		}
		return err
	}

	lines := strings.Split(string(raw), "\n")
	for i, line := range lines {
		trimmed := strings.TrimSpace(line)
		if strings.HasPrefix(trimmed, "roles ") || strings.HasPrefix(trimmed, "roles=") {
			lines[i] = fmt.Sprintf("roles = %q", roles)
			return os.WriteFile(path, []byte(strings.Join(lines, "\n")), 0o644)
		}
	}
	return nil
}

func cmdInstall() *cobra.Command {
	return &cobra.Command{
		Use:   "install",
		Short: "Install or reinstall the Hashgram binaries and systemd units",
		Long: `Install or reinstall the Hashgram binaries and systemd units.

This is a convenience wrapper. The authoritative installer is
scripts/install/bootstrap-ubuntu.sh, which also creates the service accounts,
directories and firewall rules. Run that first on a fresh machine; this
command is for reinstalling binaries after a rebuild.`,
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			script := findRepoFile("scripts/install/bootstrap-ubuntu.sh")
			if script == "" {
				return fmt.Errorf(
					"scripts/install/bootstrap-ubuntu.sh not found.\n\n" +
						"Run this from a checkout of the Hashgram repository, or run the script " +
						"directly:\n\n  sudo ./scripts/install/bootstrap-ubuntu.sh")
			}

			fmt.Fprintf(cmd.OutOrStdout(), "running %s --binaries-only\n\n", script)
			c := exec.CommandContext(cmd.Context(), "bash", script, "--binaries-only")
			c.Stdout = cmd.OutOrStdout()
			c.Stderr = cmd.ErrOrStderr()
			return c.Run()
		},
	}
}

func cmdInit() *cobra.Command {
	var moniker string

	cmd := &cobra.Command{
		Use:   "init",
		Short: "Initialise the node home, keys and configuration",
		Long: `Initialise the node home, keys and configuration.

This creates the node home, the P2P node key and a default configuration. It
does NOT create a genesis file: use 'init-mainnet-genesis' to create a network
or 'join-mainnet' to join one.

Safe to run on an existing node home: it will not overwrite an existing
validator key, because doing so would destroy the identity the chain knows
this validator by.`,
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			ctx, cancel := commandContext()
			defer cancel()

			if fileExists(paths.ValidatorKeyFile()) {
				fmt.Fprintf(cmd.OutOrStdout(),
					"validator key already exists at %s; leaving it untouched\n",
					paths.ValidatorKeyFile())
			}

			// hashgramd init writes a default genesis as a side effect. It is
			// removed afterwards so an operator cannot accidentally start a
			// chain on it: a default genesis is a devnet genesis.
			out, err := runHashgramd(ctx, "init", moniker, "--default-denom", "uhash")
			if err != nil && !strings.Contains(out, "already exists") {
				return err
			}

			genesisPath := paths.GenesisFile()
			if fileExists(genesisPath) {
				network, netErr := hgconfig.LoadNetwork(paths)
				if netErr != nil || network.GenesisHash == "" {
					if err := os.Remove(genesisPath); err == nil {
						fmt.Fprintf(cmd.OutOrStdout(),
							"removed the placeholder genesis written by init: a default genesis is a\n"+
								"devnet genesis, and leaving it would let a start command launch the\n"+
								"wrong network. Use init-mainnet-genesis or join-mainnet next.\n")
					}
				}
			}

			o := newOut()
			o.row("Node home", paths.NodeHome)
			o.row("Node key", paths.NodeKeyFile())
			o.row("Validator key", paths.ValidatorKeyFile())
			o.row("Config", paths.CometConfigFile())
			o.blank()
			o.raw("Next:")
			o.raw("  hashgramctl init-mainnet-genesis --founder-address hash1...   (create a network)")
			o.raw("  hashgramctl join-mainnet --genesis-url ... --genesis-hash ... (join one)")
			return o.flush()
		},
	}

	cmd.Flags().StringVar(&moniker, "moniker", "hashgram-node", "node moniker")
	return cmd
}

func cmdUpdate() *cobra.Command {
	return &cobra.Command{
		Use:   "update",
		Short: "Show the installed versions and known on-chain upgrades",
		Long: `Show the installed versions and known on-chain upgrades.

This command deliberately does not download or install anything. Automatic
binary updates on a validator are a way to have somebody else's compromised
release infrastructure sign your blocks. Fetch a release, verify its
signature and checksum yourself, then run 'make install'.

See docs/OPERATIONS.md on the upgrade procedure.`,
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			ctx, cancel := commandContext()
			defer cancel()

			o := newOut()

			version, err := runHashgramd(ctx, "version")
			if err != nil {
				o.row("hashgramd", "not runnable: "+err.Error())
			} else {
				o.row("hashgramd", strings.TrimSpace(version))
			}

			var plan struct {
				Plan struct {
					Name   string `json:"name"`
					Height string `json:"height"`
					Info   string `json:"info"`
				} `json:"plan"`
			}
			if err := queryJSON(ctx, &plan, "query", "upgrade", "plan"); err != nil {
				o.row("Scheduled upgrade", "none, or the node is not reachable")
			} else if plan.Plan.Name == "" {
				o.row("Scheduled upgrade", "none")
			} else {
				o.row("Scheduled upgrade", plan.Plan.Name)
				o.row("  at height", plan.Plan.Height)
				o.row("  info", plan.Plan.Info)
			}

			o.blank()
			o.raw("Upgrades are applied by governance proposal and require operators to run")
			o.raw("the new binary. This command never downloads anything: automatic updates")
			o.raw("on a validator mean somebody else's release infrastructure can sign your")
			o.raw("blocks. Verify signatures and checksums yourself.")
			return o.flush()
		},
	}
}

// findRepoFile locates a repository-relative file from the working directory
// upward, so hashgramctl works from anywhere inside a checkout.
func findRepoFile(rel string) string {
	dir, err := os.Getwd()
	if err != nil {
		return ""
	}
	for i := 0; i < 6; i++ {
		candidate := filepath.Join(dir, rel)
		if fileExists(candidate) {
			return candidate
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			break
		}
		dir = parent
	}
	for _, base := range []string{"/home/hashgram", "/opt/hashgram", "/usr/local/share/hashgram"} {
		candidate := filepath.Join(base, rel)
		if fileExists(candidate) {
			return candidate
		}
	}
	return ""
}

func firstNonEmpty(values ...string) string {
	for _, v := range values {
		if v != "" {
			return v
		}
	}
	return ""
}

func firstNonZero(values ...uint64) uint64 {
	for _, v := range values {
		if v != 0 {
			return v
		}
	}
	return 0
}
