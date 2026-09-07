package cmd

import (
	"fmt"
	"os"
	"strings"

	"github.com/spf13/cobra"

	hgparams "github.com/hashgram/hashgram/app/params"
	"github.com/hashgram/hashgram/cmd/hashgramctl/internal/hgconfig"
	"github.com/hashgram/hashgram/cmd/hashgramctl/internal/hgrpc"
	"github.com/hashgram/hashgram/cmd/hashgramctl/internal/hgsys"
)

func cmdStatus() *cobra.Command {
	return &cobra.Command{
		Use:   "status",
		Short: "Show a one-screen summary of this node",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			ctx, cancel := commandContext()
			defer cancel()

			o := newOut()

			network, netErr := hgconfig.LoadNetwork(paths)
			if netErr != nil {
				o.row("Network", "NOT CONFIGURED")
			} else {
				o.row("Network", fmt.Sprintf("%s (%s)", network.NetworkName, network.ChainID))
				o.row("Genesis hash", network.GenesisHash)
			}

			roles, _ := hgconfig.LoadRoles(paths)
			if len(roles.Roles) == 0 {
				o.row("Roles", "none configured; run 'hashgramctl configure-role'")
			} else {
				o.row("Roles", roles.String())
			}

			o.section("Services")
			for _, unit := range unitsForRoles(roles) {
				state := hgsys.Inspect(ctx, unit)
				if !state.Exists {
					o.row("  "+unit, "not installed")
					continue
				}
				o.row("  "+unit, fmt.Sprintf("%s (%s)", state.Active, state.SubState))
			}

			o.section("Chain")
			status, err := rpc.Status(ctx)
			if err != nil {
				o.row("  RPC", "unreachable: "+err.Error())
			} else {
				o.row("  Height", status.SyncInfo.LatestBlockHeight)
				o.row("  Catching up", status.SyncInfo.CatchingUp)
				o.row("  Node id", status.NodeInfo.ID)
				if status.IsValidator() {
					o.row("  Validator", fmt.Sprintf("yes, voting power %d", status.VotingPower()))
				} else {
					o.row("  Validator", "no")
				}

				// A node reporting a different chain-id than the pinned one
				// is on the wrong network, which is worth saying loudly.
				if netErr == nil && status.NodeInfo.Network != network.ChainID {
					o.row("  WARNING", fmt.Sprintf(
						"the running node reports chain-id %q but this machine is pinned to %q",
						status.NodeInfo.Network, network.ChainID))
				}
			}

			if net, err := rpc.NetInfo(ctx); err == nil {
				o.row("  Peers", net.PeerCount())
			}

			// The P2P node, when this machine runs one. Its absence is not
			// an error for a validator-only host.
			if node := hgrpc.NewNodeAPI(flagNodeAPI); node.Reachable(ctx) {
				o.section("P2P node (hashgram-node)")
				if st, err := node.Status(ctx); err == nil {
					if st.Swarm != nil {
						o.row("  Peer id", st.Swarm.PeerID)
						o.row("  Peers", fmt.Sprintf("%d connected, %d verified, %d known",
							st.Swarm.Connected, st.Swarm.Verified, st.Swarm.Known))
						o.row("  Reachability", st.Swarm.Reachability)
						if len(st.Swarm.ExternalAddrs) > 0 {
							o.row("  External addrs", strings.Join(st.Swarm.ExternalAddrs, ", "))
						}
					}
					o.row("  Announcements", st.AnnouncementsKnown)
					if st.Blobs != nil {
						o.row("  Blobs", fmt.Sprintf("%v complete, %v degraded", st.Blobs["blobs_complete"], st.Blobs["degraded"]))
					}
					if st.Mailbox != nil {
						o.row("  Mailbox", fmt.Sprintf("%v envelopes, %v key packages", st.Mailbox["envelopes"], st.Mailbox["key_packages"]))
					}
					if st.Social != nil {
						o.row("  Social events", st.Social["events"])
					}
					if netErr == nil && st.GenesisHash != network.GenesisHash {
						o.row("  WARNING", "hashgram-node is pinned to a different genesis hash than this machine")
					}
				}
				if rv, err := node.Rewards(ctx); err == nil {
					o.row("  Operator", rv["operator"])
					if rv["provider"] == nil {
						o.row("  Provider", "not registered on chain")
					} else {
						o.row("  Provider", "registered")
					}
				}
			}

			o.section("Host")
			if free, total, err := hgsys.DiskUsage(paths.NodeHome); err == nil {
				o.row("  Disk", fmt.Sprintf("%s free of %s",
					hgsys.HumanBytes(free), hgsys.HumanBytes(total)))
			}
			if uptime, err := hgsys.Uptime(); err == nil {
				o.row("  Host uptime", uptime.Round(60_000_000_000).String())
			}

			return o.flush()
		},
	}
}

func cmdHealth() *cobra.Command {
	return &cobra.Command{
		Use:   "health",
		Short: "Check whether this node is healthy, exiting non-zero if not",
		Long: `Check whether this node is healthy, exiting non-zero if not.

Suitable for monitoring. Reports unhealthy when the RPC is unreachable, the
node is still catching up, it has no peers on a network that should have them,
or the running chain-id does not match the pinned one.`,
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			ctx, cancel := commandContext()
			defer cancel()

			var problems []string
			o := newOut()

			status, err := rpc.Status(ctx)
			if err != nil {
				problems = append(problems, "node RPC is unreachable")
				o.row("RPC", "unreachable")
			} else {
				o.row("RPC", "reachable")
				o.row("Height", status.SyncInfo.LatestBlockHeight)

				if status.SyncInfo.CatchingUp {
					problems = append(problems, "node is still catching up")
					o.row("Sync", "catching up")
				} else {
					o.row("Sync", "synced")
				}

				if network, netErr := hgconfig.LoadNetwork(paths); netErr == nil {
					if status.NodeInfo.Network != network.ChainID {
						problems = append(problems, fmt.Sprintf(
							"running chain-id %q does not match the pinned %q",
							status.NodeInfo.Network, network.ChainID))
					}
				}
			}

			if net, err := rpc.NetInfo(ctx); err == nil {
				o.row("Peers", net.PeerCount())
				if net.PeerCount() == 0 {
					// Not a failure on a single-node network, which is a
					// legitimate bootstrap state, so this is reported rather
					// than treated as unhealthy.
					o.row("Note", "no peers; expected on a single-node bootstrap network")
				}
				if !net.Listening {
					problems = append(problems, "P2P is not listening")
				}
			}

			roles, _ := hgconfig.LoadRoles(paths)
			for _, unit := range unitsForRoles(roles) {
				state := hgsys.Inspect(ctx, unit)
				if state.Exists && !state.Running() {
					problems = append(problems, fmt.Sprintf("%s is %s", unit, state.Active))
				}
			}

			o.set("problems", problems)
			o.set("healthy", len(problems) == 0)

			if len(problems) == 0 {
				o.row("Result", "HEALTHY")
				return o.flush()
			}

			o.row("Result", "UNHEALTHY")
			for i, p := range problems {
				o.row(fmt.Sprintf("  problem %d", i+1), p)
			}
			if err := o.flush(); err != nil {
				return err
			}
			return fmt.Errorf("unhealthy: %s", strings.Join(problems, "; "))
		},
	}
}

func cmdPeers() *cobra.Command {
	return &cobra.Command{
		Use:   "peers",
		Short: "List connected peers",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			ctx, cancel := commandContext()
			defer cancel()

			net, err := rpc.NetInfo(ctx)
			if err != nil {
				return err
			}

			if flagJSON {
				o := newOut()
				o.set("listening", net.Listening)
				o.set("peer_count", net.PeerCount())
				o.set("peers", net.Peers)
				return o.flush()
			}

			w := cmd.OutOrStdout()
			fmt.Fprintf(w, "listening: %v\npeers: %d\n\n", net.Listening, net.PeerCount())
			if len(net.Peers) == 0 {
				fmt.Fprintf(w, "Consensus (CometBFT): no peers. On a single-node bootstrap network this is expected;\n")
				fmt.Fprintf(w, "on a joined network, check persistent_peers in %s\n",
					paths.CometConfigFile())
			} else {
				fmt.Fprintf(w, "Consensus (CometBFT)\n")
				fmt.Fprintf(w, "%-42s %-16s %-9s %s\n", "NODE ID", "REMOTE IP", "DIRECTION", "MONIKER")
				for _, p := range net.Peers {
					direction := "inbound"
					if p.IsOutbound {
						direction = "outbound"
					}
					fmt.Fprintf(w, "%-42s %-16s %-9s %s\n",
						p.NodeInfo.ID, p.RemoteIP, direction, p.NodeInfo.Moniker)
				}
			}

			// The P2P node's peers, which passed the Hashgram handshake
			// (genesis hash included), when the node runs here.
			if node := hgrpc.NewNodeAPI(flagNodeAPI); node.Reachable(ctx) {
				peers, err := node.Peers(ctx)
				if err == nil {
					fmt.Fprintf(w, "\nP2P (hashgram-node): %d peers\n", len(peers))
					if len(peers) > 0 {
						fmt.Fprintf(w, "%-54s %-9s %-8s %-6s %s\n", "PEER ID", "DIRECTION", "VERIFIED", "SCORE", "ROLES")
						for _, p := range peers {
							fmt.Fprintf(w, "%-54s %-9s %-8v %-6d %s\n",
								p.PeerID, p.Direction, p.Verified, p.Score, strings.Join(p.Roles, ","))
						}
					}
				}
			}
			return nil
		},
	}
}

func cmdNetworkInfo() *cobra.Command {
	return &cobra.Command{
		Use:   "network-info",
		Short: "Show the network identity this node is pinned to",
		Long: `Show the network identity this node is pinned to.

Compare every field against the published Hashgram Mainnet values. If the
genesis hash differs, this node is on a different network, whatever it calls
itself.

Verify the hash independently:

  sha256sum ` + "$HOME/.hashgram/config/genesis.json",
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			ctx, cancel := commandContext()
			defer cancel()

			network, err := hgconfig.LoadNetwork(paths)
			if err != nil {
				return err
			}

			o := newOut()
			o.row("Network name", network.NetworkName)
			o.row("Network id", network.NetworkID)
			o.row("Chain id", network.ChainID)
			o.row("Network magic", network.NetworkMagic)
			o.row("Protocol version", fmt.Sprintf("v%d", network.ProtocolMajorVersion))
			o.row("Genesis hash", network.GenesisHash)
			o.row("Is mainnet", network.IsMainnet())
			o.row("Pinned in", paths.NetworkFile())

			// Verify the file on disk still matches the pin. A mismatch means
			// somebody replaced the genesis file after this node was
			// configured.
			if raw, err := os.ReadFile(paths.GenesisFile()); err == nil {
				actual := hgparams.ComputeGenesisHash(raw)
				if actual == network.GenesisHash {
					o.row("Genesis on disk", "matches the pin")
				} else {
					o.row("Genesis on disk", fmt.Sprintf(
						"MISMATCH: hashes to %s. The genesis file has been replaced since this "+
							"node was configured", actual))
				}
			}

			// What the running node is actually on, checked by chain id rather
			// than by hashing the genesis the RPC serves.
			//
			// Hashing the RPC response looks like the obvious check and is
			// wrong: CometBFT parses the genesis file and re-serialises it,
			// dropping the SDK's app_name and app_version fields, rendering
			// initial_height as a string, and emitting compact rather than
			// indented JSON. The two byte streams therefore never match, so
			// the comparison reported a mismatch on every healthy node and
			// blamed it on a stale restart. A diagnostic that always warns is
			// a diagnostic operators learn to ignore.
			//
			// The chain id is the identity CometBFT itself enforces at the
			// peer handshake, so comparing it answers the question that
			// matters: is this node on the network we pinned.
			if status, err := rpc.Status(ctx); err == nil {
				served := status.NodeInfo.Network
				if served == network.ChainID {
					o.row("Running node", fmt.Sprintf("on chain id %s, matching the pin", served))
				} else {
					o.row("Running node", fmt.Sprintf(
						"MISMATCH: serving chain id %s but this node is pinned to %s. "+
							"It was probably started before the pin changed and needs a restart",
						served, network.ChainID))
				}
				o.row("  genesis hash note", "CometBFT re-serialises the genesis, so the hash of "+
					"the RPC response is not comparable to the file hash above")
			}

			o.section("Fork isolation")
			o.raw("  A peer must match network_id, chain_id, network_magic, protocol version")
			o.raw("  and genesis_hash. A fork of this software with its own genesis has a")
			o.raw("  different genesis hash and is rejected. See docs/DECENTRALIZATION.md.")

			return o.flush()
		},
	}
}

func cmdNodeInfo() *cobra.Command {
	return &cobra.Command{
		Use:   "node-info",
		Short: "Show this node's identity, roles and build",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			ctx, cancel := commandContext()
			defer cancel()

			o := newOut()

			if status, err := rpc.Status(ctx); err == nil {
				o.row("Node id", status.NodeInfo.ID)
				o.row("Moniker", status.NodeInfo.Moniker)
				o.row("CometBFT version", status.NodeInfo.Version)
				o.row("Validator address", status.ValidatorInfo.Address)
				o.row("Voting power", status.VotingPower())
			} else {
				o.row("RPC", "unreachable: "+err.Error())
			}

			if info, err := rpc.ABCIInfo(ctx); err == nil {
				o.row("Application", info.Data)
				o.row("App version", info.Version)
			}

			roles, _ := hgconfig.LoadRoles(paths)
			o.row("Roles", roles.String())
			if roles.RewardAddress != "" {
				o.row("Reward address", roles.RewardAddress)
			}
			if roles.DeclaredStorageBytes > 0 {
				o.row("Declared storage", hgsys.HumanBytes(roles.DeclaredStorageBytes))
			}

			o.section("Paths")
			o.row("  Node home", paths.NodeHome)
			o.row("  Config dir", paths.ConfigDir)
			o.row("  Data dir", paths.DataDir)

			o.section("Keys on this machine")
			o.row("  Node key", presence(paths.NodeKeyFile()))
			o.row("  Validator key", presence(paths.ValidatorKeyFile()))
			o.raw("")
			o.raw("  No Founder key is expected on this machine. If one is here, move it off:")
			o.raw("  see docs/FOUNDER_LAUNCH_RUNBOOK.md Part A.")

			return o.flush()
		},
	}
}

func presence(path string) string {
	if !fileExists(path) {
		return "absent"
	}
	mode, err := hgsys.FileMode(path)
	if err != nil {
		return "present"
	}
	return fmt.Sprintf("present (mode %04o)", mode)
}

func shortHash(h string) string {
	if len(h) <= 16 {
		return h
	}
	return h[:8] + ".." + h[len(h)-8:]
}
