// hashgram-safety is the Hashgram Safety Engine.
//
//	hashgram-safety run    --config /etc/hashgram/safety.toml
//	hashgram-safety key    --config /etc/hashgram/safety.toml     print the attestor public key
//	hashgram-safety attest --config ... --event <hex> --verdict BLOCK --reason manual-review
//	hashgram-safety attest --config ... --cid <hex>   --verdict UNBLOCK --reason appeal-upheld
//
// It reviews PUBLIC content only. Its systemd unit has no access to the
// node's data directory, and there is no code path here that reads an
// encrypted envelope.
package main

import (
	"context"
	"crypto/ed25519"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"log/slog"
	"os"
	"os/signal"
	"path/filepath"
	"syscall"

	"github.com/spf13/cobra"

	hgparams "github.com/hashgram/hashgram/app/params"
	"github.com/hashgram/hashgram/pkg/p2ppb"
	"github.com/hashgram/hashgram/safety"
)

func main() {
	log := slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: slog.LevelInfo}))
	var configPath string

	root := &cobra.Command{Use: "hashgram-safety", Short: "Hashgram Safety Engine (public content only)", SilenceUsage: true, SilenceErrors: true}
	root.PersistentFlags().StringVar(&configPath, "config", "/etc/hashgram/safety.toml", "configuration file")

	load := func() (safety.Config, hgparams.NetworkIdentity, error) {
		cfg, err := safety.LoadConfig(configPath)
		if err != nil {
			return cfg, hgparams.NetworkIdentity{}, err
		}
		raw, err := os.ReadFile(filepath.Join(filepath.Dir(configPath), "network.json"))
		if err != nil {
			return cfg, hgparams.NetworkIdentity{}, fmt.Errorf("network pin: %w", err)
		}
		var pin struct {
			NetworkID   string `json:"network_id"`
			GenesisHash string `json:"genesis_hash"`
		}
		if err := json.Unmarshal(raw, &pin); err != nil {
			return cfg, hgparams.NetworkIdentity{}, err
		}
		switch pin.NetworkID {
		case "hashgram-mainnet":
			return cfg, hgparams.MainnetIdentity(pin.GenesisHash), nil
		case "hashgram-devnet":
			return cfg, hgparams.DevnetIdentity(pin.GenesisHash), nil
		}
		return cfg, hgparams.NetworkIdentity{}, fmt.Errorf("unknown network %q", pin.NetworkID)
	}

	root.AddCommand(&cobra.Command{
		Use:   "run",
		Short: "Review public content as it arrives and publish signed verdicts",
		RunE: func(cmd *cobra.Command, _ []string) error {
			cfg, id, err := load()
			if err != nil {
				return err
			}
			key, err := safety.LoadOrCreateKey(cfg.Home)
			if err != nil {
				return err
			}
			eng, err := safety.NewEngine(cfg, id, key, log)
			if err != nil {
				return err
			}
			log.Info("hashgram-safety starting", "network", id.NetworkID, "attestor", eng.AttestorPubkey(), "node_api", cfg.NodeAPI)
			log.Info("add the attestor key to trusted_attestors in node.toml and indexer.toml on nodes that should enforce these verdicts")
			eng.Run(cmd.Context())
			log.Info("hashgram-safety stopped", "reviewed", eng.Reviewed, "published", eng.Published, "errors", eng.Errors)
			return nil
		},
	})
	root.AddCommand(&cobra.Command{
		Use:   "key",
		Short: "Print the attestor public key (hex), creating the key if absent",
		RunE: func(_ *cobra.Command, _ []string) error {
			cfg, _, err := load()
			if err != nil {
				return err
			}
			key, err := safety.LoadOrCreateKey(cfg.Home)
			if err != nil {
				return err
			}
			fmt.Println(hex.EncodeToString(key.Public().(ed25519.PublicKey)))
			return nil
		},
	})

	var eventID, cid, verdict, reason string
	attest := &cobra.Command{
		Use:   "attest",
		Short: "Publish a manual verdict about a public event or blob",
		RunE: func(cmd *cobra.Command, _ []string) error {
			cfg, id, err := load()
			if err != nil {
				return err
			}
			key, err := safety.LoadOrCreateKey(cfg.Home)
			if err != nil {
				return err
			}
			eng, err := safety.NewEngine(cfg, id, key, log)
			if err != nil {
				return err
			}
			a := &p2ppb.ContentAttestation{Policy: cfg.Policy, ReasonCode: reason, Timestamp: safety.UnixNow()}
			switch verdict {
			case "ALLOW", "QUARANTINE", "RESTRICT", "BLOCK":
				a.Verdict = safety.ParseVerdict(verdict).Proto()
			case "UNBLOCK":
				a.Verdict = p2ppb.Verdict_CONTENT_UNBLOCK
			default:
				return fmt.Errorf("verdict must be ALLOW, QUARANTINE, RESTRICT, BLOCK or UNBLOCK")
			}
			if (eventID == "") == (cid == "") {
				return fmt.Errorf("exactly one of --event or --cid is required")
			}
			if eventID != "" {
				if a.EventId, err = hex.DecodeString(eventID); err != nil {
					return err
				}
			} else if a.Cid, err = hex.DecodeString(cid); err != nil {
				return err
			}
			if err := eng.Publish(cmd.Context(), a); err != nil {
				return err
			}
			fmt.Printf("%s published for %s%s\n", verdict, eventID, cid)
			return nil
		},
	}
	attest.Flags().StringVar(&eventID, "event", "", "hex social event id")
	attest.Flags().StringVar(&cid, "cid", "", "hex blob cid")
	attest.Flags().StringVar(&verdict, "verdict", "", "ALLOW|QUARANTINE|RESTRICT|BLOCK|UNBLOCK")
	attest.Flags().StringVar(&reason, "reason", "manual-review", "machine-readable reason code")
	root.AddCommand(attest)

	ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGINT, syscall.SIGTERM)
	defer stop()
	if err := root.ExecuteContext(ctx); err != nil {
		fmt.Fprintf(os.Stderr, "hashgram-safety: %v\n", err)
		os.Exit(1)
	}
}
