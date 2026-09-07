// hashgram-indexer derives PostgreSQL query tables from the chain and from
// signed social events, and serves a loopback read API.
//
//	hashgram-indexer run     --config /etc/hashgram/indexer.toml
//	hashgram-indexer rebuild --config /etc/hashgram/indexer.toml
//	hashgram-indexer check   --config /etc/hashgram/indexer.toml
//
// The index is a cache. Destroy it and `rebuild` recreates it from canonical
// data; nothing the network relies on lives here.
package main

import (
	"context"
	"encoding/json"
	"fmt"
	"log/slog"
	"net/http"
	"os"
	"os/signal"
	"strings"
	"syscall"
	"time"

	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/spf13/cobra"

	hgparams "github.com/hashgram/hashgram/app/params"
	"github.com/hashgram/hashgram/indexer"
)

func main() {
	log := slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: slog.LevelInfo}))
	var configPath string

	root := &cobra.Command{
		Use:           "hashgram-indexer",
		Short:         "Hashgram PostgreSQL indexer",
		SilenceUsage:  true,
		SilenceErrors: true,
	}
	root.PersistentFlags().StringVar(&configPath, "config", "/etc/hashgram/indexer.toml", "configuration file")
	cobra.OnInitialize(func() { configPathGlobal = configPath })

	root.AddCommand(&cobra.Command{
		Use:   "run",
		Short: "Follow the chain and the node, serve the read API",
		RunE: func(cmd *cobra.Command, _ []string) error {
			cfg, err := indexer.Load(configPath)
			if err != nil {
				return err
			}
			return run(cmd.Context(), cfg, log)
		},
	})
	root.AddCommand(&cobra.Command{
		Use:   "rebuild",
		Short: "Clear every derived table and cursor; the next run refills them",
		RunE: func(cmd *cobra.Command, _ []string) error {
			cfg, err := indexer.Load(configPath)
			if err != nil {
				return err
			}
			db, err := connect(cmd.Context(), cfg)
			if err != nil {
				return err
			}
			defer db.Close()
			if err := indexer.Migrate(cmd.Context(), db); err != nil {
				return err
			}
			if err := indexer.Reset(cmd.Context(), db); err != nil {
				return err
			}
			fmt.Println("index cleared; start the indexer to rebuild from canonical data")
			return nil
		},
	})
	root.AddCommand(&cobra.Command{
		Use:   "check",
		Short: "Validate the configuration and database connection",
		RunE: func(cmd *cobra.Command, _ []string) error {
			cfg, err := indexer.Load(configPath)
			if err != nil {
				return err
			}
			db, err := connect(cmd.Context(), cfg)
			if err != nil {
				return err
			}
			defer db.Close()
			if err := indexer.Migrate(cmd.Context(), db); err != nil {
				return err
			}
			fmt.Println("configuration valid, database reachable, schema applied")
			return nil
		},
	})

	ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGINT, syscall.SIGTERM)
	defer stop()
	if err := root.ExecuteContext(ctx); err != nil {
		fmt.Fprintf(os.Stderr, "hashgram-indexer: %v\n", err)
		os.Exit(1)
	}
}

func connect(ctx context.Context, cfg indexer.Config) (*pgxpool.Pool, error) {
	pc, err := pgxpool.ParseConfig(cfg.DatabaseURL)
	if err != nil {
		return nil, fmt.Errorf("database_url: %w", err)
	}
	pc.MaxConns = 8
	db, err := pgxpool.NewWithConfig(ctx, pc)
	if err != nil {
		return nil, err
	}
	pingCtx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()
	if err := db.Ping(pingCtx); err != nil {
		return nil, fmt.Errorf("database: %w", err)
	}
	return db, nil
}

// pinnedIdentity reads /etc/hashgram/network.json next to the indexer config.
func pinnedIdentity(configPath string) (hgparams.NetworkIdentity, error) {
	dir := configPath[:strings.LastIndex(configPath, "/")+1]
	raw, err := os.ReadFile(dir + "network.json")
	if err != nil {
		return hgparams.NetworkIdentity{}, fmt.Errorf("reading the network pin: %w", err)
	}
	var pin struct {
		NetworkID   string `json:"network_id"`
		GenesisHash string `json:"genesis_hash"`
	}
	if err := json.Unmarshal(raw, &pin); err != nil {
		return hgparams.NetworkIdentity{}, err
	}
	switch pin.NetworkID {
	case "hashgram-mainnet":
		return hgparams.MainnetIdentity(pin.GenesisHash), nil
	case "hashgram-devnet":
		return hgparams.DevnetIdentity(pin.GenesisHash), nil
	default:
		return hgparams.NetworkIdentity{}, fmt.Errorf("network.json pins unknown network %q", pin.NetworkID)
	}
}

func run(ctx context.Context, cfg indexer.Config, log *slog.Logger) error {
	identity, err := pinnedIdentity(cfgPath(cfg))
	if err != nil {
		return err
	}
	db, err := connect(ctx, cfg)
	if err != nil {
		return err
	}
	defer db.Close()
	if err := indexer.Migrate(ctx, db); err != nil {
		return err
	}

	chain := indexer.NewChainIngester(cfg, db, log)
	if _, chainID, err := chain.Height(ctx); err != nil {
		log.Warn("chain not reachable yet", "error", err)
	} else if chainID != identity.ChainID {
		return fmt.Errorf("chain node reports chain-id %q but this machine is pinned to %q; refusing to index the wrong network", chainID, identity.ChainID)
	}
	social := indexer.NewSocialIngester(cfg, db, identity, log)
	api := indexer.NewAPI(db, log)

	log.Info("hashgram-indexer starting", "network", identity.NetworkID, "listen", cfg.Listen, "chain_api", cfg.ChainAPI, "node_api", cfg.NodeAPI)

	go chain.Follow(ctx)
	go social.Follow(ctx)
	go func() {
		t := time.NewTicker(60 * time.Second)
		defer t.Stop()
		for {
			if err := chain.SyncRegistries(ctx); err != nil {
				log.Warn("registry sync", "error", err)
			}
			select {
			case <-ctx.Done():
				return
			case <-t.C:
			}
		}
	}()
	// Safety verdicts are enforcement, so they are pulled at the poll
	// interval rather than with the slow registry sync.
	go func() {
		t := time.NewTicker(cfg.PollInterval)
		defer t.Stop()
		for {
			if err := social.SyncAttestations(ctx); err != nil {
				log.Debug("attestation sync", "error", err)
			}
			select {
			case <-ctx.Done():
				return
			case <-t.C:
			}
		}
	}()

	srv := &http.Server{
		Addr:              cfg.Listen,
		Handler:           api.Handler(),
		ReadHeaderTimeout: 5 * time.Second,
	}
	go func() {
		<-ctx.Done()
		shutdownCtx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		_ = srv.Shutdown(shutdownCtx)
	}()
	if err := srv.ListenAndServe(); err != nil && err != http.ErrServerClosed {
		return err
	}
	log.Info("hashgram-indexer stopped")
	return nil
}

// cfgPath recovers the config path for the pin lookup. The indexer takes
// the pin from the same directory as its own config, so the two cannot
// drift apart on a machine.
func cfgPath(cfg indexer.Config) string {
	if p := os.Getenv("HASHGRAM_INDEXER_CONFIG"); p != "" {
		return p
	}
	_ = cfg
	return configPathGlobal
}

var configPathGlobal = "/etc/hashgram/indexer.toml"
