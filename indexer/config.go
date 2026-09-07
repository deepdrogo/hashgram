// Package indexer derives query tables from canonical Hashgram data.
//
// Nothing here is canonical. The chain is canonical for balances, names,
// identities and providers; signed, replicated social events are canonical
// for the social graph. PostgreSQL holds a rebuildable projection of both so
// that "Alice's timeline" or "everything tagged #reels" is one query rather
// than a scan. If the database is lost, `hashgram-indexer rebuild` recreates
// it; if it disagrees with the chain, it is wrong.
//
// The indexer never holds a key and never writes to the network. It reads
// the co-located chain node over its loopback RPC and REST, and the P2P node
// over its loopback API, and it serves a loopback read API of its own.
package indexer

import (
	"fmt"
	"os"
	"time"

	"github.com/pelletier/go-toml/v2"
)

// Config is /etc/hashgram/indexer.toml.
type Config struct {
	// DatabaseURL is a PostgreSQL connection string. The role it names must
	// own only the index database; see docs/SECURITY.md on least privilege.
	DatabaseURL string `toml:"database_url"`

	// ChainRPC is the CometBFT RPC of the co-located chain node.
	ChainRPC string `toml:"chain_rpc"`
	// ChainAPI is the REST gateway of the co-located chain node.
	ChainAPI string `toml:"chain_api"`
	// NodeAPI is the loopback API of the co-located hashgram-node.
	NodeAPI string `toml:"node_api"`

	// Listen is where the read API binds. Loopback by default; exposing it
	// is a reverse-proxy decision.
	Listen string `toml:"listen"`

	// PollIntervalStr bounds how often sources are polled, e.g. "3s".
	PollIntervalStr string `toml:"poll_interval"`
	// PollInterval is the parsed value.
	PollInterval time.Duration `toml:"-"`

	// StartHeight lets a fresh index skip ancient history. Zero indexes
	// from genesis.
	StartHeight int64 `toml:"start_height"`

	// TrustedAttestors are hex ed25519 keys whose safety verdicts the read
	// API enforces. Should match the node's trusted_attestors.
	TrustedAttestors []string `toml:"trusted_attestors"`
}

// Defaults fills unset fields.
func (c *Config) Defaults() {
	if c.ChainRPC == "" {
		c.ChainRPC = "http://127.0.0.1:26657"
	}
	if c.ChainAPI == "" {
		c.ChainAPI = "http://127.0.0.1:1317"
	}
	if c.NodeAPI == "" {
		c.NodeAPI = "http://127.0.0.1:26672"
	}
	if c.Listen == "" {
		c.Listen = "127.0.0.1:1318"
	}
	if c.PollIntervalStr != "" {
		if d, err := time.ParseDuration(c.PollIntervalStr); err == nil {
			c.PollInterval = d
		}
	}
	if c.PollInterval == 0 {
		c.PollInterval = 3 * time.Second
	}
}

// Validate refuses a configuration that cannot work.
func (c Config) Validate() error {
	if c.DatabaseURL == "" {
		return fmt.Errorf("database_url is required, e.g. postgres://hashgram_index@/hashgram_index?host=/var/run/postgresql")
	}
	if c.PollInterval < 500*time.Millisecond {
		return fmt.Errorf("poll_interval %s is too aggressive", c.PollInterval)
	}
	return nil
}

// Load reads and validates a configuration file.
func Load(path string) (Config, error) {
	var c Config
	raw, err := os.ReadFile(path)
	if err != nil {
		return c, fmt.Errorf("reading %s: %w", path, err)
	}
	if err := toml.Unmarshal(raw, &c); err != nil {
		return c, fmt.Errorf("parsing %s: %w", path, err)
	}
	c.Defaults()
	if err := c.Validate(); err != nil {
		return c, fmt.Errorf("%s: %w", path, err)
	}
	return c, nil
}
