package indexer

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"

	"github.com/jackc/pgx/v5/pgxpool"
)

// httpJSON fetches JSON with a bounded body.
func httpJSON(ctx context.Context, client *http.Client, u string, out any) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, u, nil)
	if err != nil {
		return err
	}
	resp, err := client.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	body, err := io.ReadAll(io.LimitReader(resp.Body, 64<<20))
	if err != nil {
		return err
	}
	if resp.StatusCode/100 != 2 {
		return fmt.Errorf("%s: HTTP %d: %s", u, resp.StatusCode, truncate(string(body), 200))
	}
	return json.Unmarshal(body, out)
}

func truncate(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n] + "…"
}

// ChainIngester follows blocks and syncs the chain registries.
type ChainIngester struct {
	cfg  Config
	db   *pgxpool.Pool
	http *http.Client
	log  *slog.Logger
}

// NewChainIngester constructs one.
func NewChainIngester(cfg Config, db *pgxpool.Pool, log *slog.Logger) *ChainIngester {
	return &ChainIngester{cfg: cfg, db: db, http: &http.Client{Timeout: 20 * time.Second}, log: log}
}

type rpcStatus struct {
	Result struct {
		NodeInfo struct {
			Network string `json:"network"`
		} `json:"node_info"`
		SyncInfo struct {
			LatestBlockHeight string `json:"latest_block_height"`
		} `json:"sync_info"`
	} `json:"result"`
}

// Height returns the chain's latest height and chain id.
func (c *ChainIngester) Height(ctx context.Context) (int64, string, error) {
	var s rpcStatus
	if err := httpJSON(ctx, c.http, c.cfg.ChainRPC+"/status", &s); err != nil {
		return 0, "", err
	}
	h, _ := strconv.ParseInt(s.Result.SyncInfo.LatestBlockHeight, 10, 64)
	return h, s.Result.NodeInfo.Network, nil
}

// blockWithTxs is the REST GetBlockWithTxs response, decoded messages included.
type blockWithTxs struct {
	Txs []struct {
		Body struct {
			Messages []map[string]any `json:"messages"`
			Memo     string           `json:"memo"`
		} `json:"body"`
		AuthInfo struct {
			Fee struct {
				Amount []struct {
					Denom  string `json:"denom"`
					Amount string `json:"amount"`
				} `json:"amount"`
				GasLimit string `json:"gas_limit"`
			} `json:"fee"`
		} `json:"auth_info"`
	} `json:"txs"`
	Block struct {
		Header struct {
			Height          string    `json:"height"`
			Time            time.Time `json:"time"`
			ProposerAddress string    `json:"proposer_address"`
		} `json:"header"`
	} `json:"block"`
}

type blockResults struct {
	Result struct {
		TxsResults []struct {
			Code    int    `json:"code"`
			GasUsed string `json:"gas_used"`
			Events  []struct {
				Type       string `json:"type"`
				Attributes []struct {
					Key   string `json:"key"`
					Value string `json:"value"`
				} `json:"attributes"`
			} `json:"events"`
		} `json:"txs_results"`
	} `json:"result"`
}

type txHashes struct {
	Result struct {
		Block struct {
			Data struct {
				Txs []string `json:"txs"`
			} `json:"data"`
		} `json:"block"`
	} `json:"result"`
}

// IndexBlock ingests one height.
func (c *ChainIngester) IndexBlock(ctx context.Context, height int64) error {
	var b blockWithTxs
	if err := httpJSON(ctx, c.http, fmt.Sprintf("%s/cosmos/tx/v1beta1/txs/block/%d", c.cfg.ChainAPI, height), &b); err != nil {
		return fmt.Errorf("block %d txs: %w", height, err)
	}
	var results blockResults
	if err := httpJSON(ctx, c.http, fmt.Sprintf("%s/block_results?height=%d", c.cfg.ChainRPC, height), &results); err != nil {
		return fmt.Errorf("block %d results: %w", height, err)
	}
	var raw txHashes
	if err := httpJSON(ctx, c.http, fmt.Sprintf("%s/block?height=%d", c.cfg.ChainRPC, height), &raw); err != nil {
		return fmt.Errorf("block %d: %w", height, err)
	}

	tx, err := c.db.Begin(ctx)
	if err != nil {
		return err
	}
	defer tx.Rollback(ctx) //nolint:errcheck

	if _, err := tx.Exec(ctx,
		`INSERT INTO blocks(height, time, proposer, tx_count) VALUES ($1,$2,$3,$4)
		 ON CONFLICT (height) DO UPDATE SET time = EXCLUDED.time, tx_count = EXCLUDED.tx_count`,
		height, b.Block.Header.Time, b.Block.Header.ProposerAddress, len(b.Txs)); err != nil {
		return err
	}

	for i, t := range b.Txs {
		hash := ""
		if i < len(raw.Result.Block.Data.Txs) {
			hash = txHash(raw.Result.Block.Data.Txs[i])
		}
		if hash == "" {
			continue
		}
		code, gasUsed := 0, int64(0)
		var events []struct {
			Type       string `json:"type"`
			Attributes []struct {
				Key   string `json:"key"`
				Value string `json:"value"`
			} `json:"attributes"`
		}
		if i < len(results.Result.TxsResults) {
			r := results.Result.TxsResults[i]
			code = r.Code
			gasUsed, _ = strconv.ParseInt(r.GasUsed, 10, 64)
			events = r.Events
		}
		var msgTypes, signers []string
		for _, m := range t.Body.Messages {
			if ty, ok := m["@type"].(string); ok {
				msgTypes = append(msgTypes, ty)
			}
			for _, k := range []string{"from_address", "address", "owner", "operator", "submitter", "delegator_address", "claimant", "assigner"} {
				if v, ok := m[k].(string); ok && v != "" && !contains(signers, v) {
					signers = append(signers, v)
				}
			}
		}
		fee := "0"
		for _, a := range t.AuthInfo.Fee.Amount {
			if a.Denom == "uhash" {
				fee = a.Amount
			}
		}
		if _, err := tx.Exec(ctx,
			`INSERT INTO transactions(hash, height, index, code, gas_used, fee_uhash, memo, msg_types, signers)
			 VALUES ($1,$2,$3,$4,$5,$6::numeric,$7,$8,$9) ON CONFLICT (hash) DO NOTHING`,
			hash, height, i, code, gasUsed, fee, t.Body.Memo, nonNil(msgTypes), nonNil(signers)); err != nil {
			return err
		}
		if code != 0 {
			continue
		}
		for _, ev := range events {
			if ev.Type != "transfer" {
				continue
			}
			var sender, recipient, amount string
			for _, a := range ev.Attributes {
				switch a.Key {
				case "sender":
					sender = a.Value
				case "recipient":
					recipient = a.Value
				case "amount":
					amount = a.Value
				}
			}
			if sender == "" || recipient == "" {
				continue
			}
			for _, coin := range strings.Split(amount, ",") {
				if !strings.HasSuffix(coin, "uhash") {
					continue
				}
				n := strings.TrimSuffix(coin, "uhash")
				if _, err := strconv.ParseUint(n, 10, 64); err != nil {
					continue
				}
				if _, err := tx.Exec(ctx,
					`INSERT INTO transfers(height, txhash, sender, recipient, amount_uhash) VALUES ($1,$2,$3,$4,$5::numeric)`,
					height, hash, sender, recipient, n); err != nil {
					return err
				}
			}
		}
	}
	if err := tx.Commit(ctx); err != nil {
		return err
	}
	return setState(ctx, c.db, "chain_height", strconv.FormatInt(height, 10))
}

func contains(xs []string, x string) bool {
	for _, y := range xs {
		if y == x {
			return true
		}
	}
	return false
}

// txHash is the SHA-256 of the raw transaction bytes (base64 in the block),
// uppercase hex as CometBFT renders it.
func txHash(b64 string) string {
	raw, err := base64Decode(b64)
	if err != nil {
		return ""
	}
	return strings.ToUpper(sha256Hex(raw))
}

// Follow ingests new blocks until the context ends.
func (c *ChainIngester) Follow(ctx context.Context) {
	for {
		if ctx.Err() != nil {
			return
		}
		if err := c.step(ctx); err != nil {
			c.log.Warn("chain ingest", "error", err)
			select {
			case <-ctx.Done():
				return
			case <-time.After(c.cfg.PollInterval):
			}
			continue
		}
		select {
		case <-ctx.Done():
			return
		case <-time.After(c.cfg.PollInterval):
		}
	}
}

func (c *ChainIngester) step(ctx context.Context) error {
	latest, _, err := c.Height(ctx)
	if err != nil {
		return err
	}
	cur, err := getState(ctx, c.db, "chain_height")
	if err != nil {
		return err
	}
	next := c.cfg.StartHeight
	if next < 1 {
		next = 1
	}
	if cur != "" {
		if h, err := strconv.ParseInt(cur, 10, 64); err == nil && h+1 > next {
			next = h + 1
		}
	}
	// Bounded batch per step so a fresh index streams rather than stalls.
	for n := 0; next <= latest && n < 200; next, n = next+1, n+1 {
		if err := c.IndexBlock(ctx, next); err != nil {
			return err
		}
	}
	return nil
}

// SyncRegistries refreshes usernames, identities and providers from the
// chain's REST queries. Cheap enough to run every minute; the tables are
// small and the chain is the source of truth.
func (c *ChainIngester) SyncRegistries(ctx context.Context) error {
	if err := c.syncUsernames(ctx); err != nil {
		return fmt.Errorf("usernames: %w", err)
	}
	if err := c.syncIdentities(ctx); err != nil {
		return fmt.Errorf("identities: %w", err)
	}
	if err := c.syncProviders(ctx); err != nil {
		return fmt.Errorf("providers: %w", err)
	}
	return nil
}

func (c *ChainIngester) paginate(ctx context.Context, path, field string, fn func(item map[string]any) error) error {
	key := ""
	for page := 0; page < 1000; page++ {
		u := c.cfg.ChainAPI + path + "?pagination.limit=200"
		if key != "" {
			u += "&pagination.key=" + url.QueryEscape(key)
		}
		var v map[string]any
		if err := httpJSON(ctx, c.http, u, &v); err != nil {
			return err
		}
		items, _ := v[field].([]any)
		for _, it := range items {
			if m, ok := it.(map[string]any); ok {
				if err := fn(m); err != nil {
					return err
				}
			}
		}
		next := ""
		if p, ok := v["pagination"].(map[string]any); ok {
			if nk, ok := p["next_key"].(string); ok {
				next = nk
			}
		}
		if next == "" {
			return nil
		}
		key = next
	}
	return nil
}

func str(m map[string]any, k string) string {
	if v, ok := m[k].(string); ok {
		return v
	}
	if v, ok := m[k].(float64); ok {
		return strconv.FormatInt(int64(v), 10)
	}
	return ""
}

func (c *ChainIngester) syncUsernames(ctx context.Context) error {
	return c.paginate(ctx, "/hashgram/username/v1/registrations", "registrations", func(m map[string]any) error {
		name, owner := str(m, "name"), str(m, "owner")
		if name == "" || owner == "" {
			return nil
		}
		_, err := c.db.Exec(ctx,
			`INSERT INTO usernames(name, owner, synced_at) VALUES ($1,$2,now())
			 ON CONFLICT (name) DO UPDATE SET owner = EXCLUDED.owner, synced_at = now()`, name, owner)
		return err
	})
}

func (c *ChainIngester) syncIdentities(ctx context.Context) error {
	return c.paginate(ctx, "/hashgram/identity/v1/identities", "identities", func(m map[string]any) error {
		addr := str(m, "address")
		if addr == "" {
			return nil
		}
		rot, _ := strconv.Atoi(str(m, "rotation_count"))
		var devices map[string]any
		_ = httpJSON(ctx, c.http, c.cfg.ChainAPI+"/hashgram/identity/v1/devices/"+addr, &devices)
		devJSON, _ := json.Marshal(devices["devices"])
		if devJSON == nil {
			devJSON = []byte("[]")
		}
		_, err := c.db.Exec(ctx,
			`INSERT INTO identities(address, root_pubkey, rotation_count, devices, synced_at) VALUES ($1,$2,$3,$4,now())
			 ON CONFLICT (address) DO UPDATE SET root_pubkey = EXCLUDED.root_pubkey, rotation_count = EXCLUDED.rotation_count, devices = EXCLUDED.devices, synced_at = now()`,
			addr, str(m, "root_pubkey"), rot, string(devJSON))
		return err
	})
}

func (c *ChainIngester) syncProviders(ctx context.Context) error {
	return c.paginate(ctx, "/hashgram/serviceproof/v1/providers", "providers", func(m map[string]any) error {
		op := str(m, "operator")
		if op == "" {
			return nil
		}
		var roles []string
		if rs, ok := m["roles"].([]any); ok {
			for _, r := range rs {
				if s, ok := r.(string); ok {
					roles = append(roles, strings.ToLower(strings.TrimPrefix(s, "SERVICE_ROLE_")))
				}
			}
		}
		bond := "0"
		if bs, ok := m["bond"].([]any); ok {
			for _, b := range bs {
				if bm, ok := b.(map[string]any); ok && str(bm, "denom") == "uhash" {
					bond = str(bm, "amount")
				}
			}
		}
		declared, _ := strconv.ParseInt(str(m, "declared_storage_bytes"), 10, 64)
		fraud, _ := strconv.Atoi(str(m, "fraud_score"))
		jailed, _ := m["jailed"].(bool)
		_, err := c.db.Exec(ctx,
			`INSERT INTO providers(operator, reward_address, roles, bond_uhash, declared_storage, jailed, fraud_score, moniker, synced_at)
			 VALUES ($1,$2,$3,$4::numeric,$5,$6,$7,$8,now())
			 ON CONFLICT (operator) DO UPDATE SET reward_address = EXCLUDED.reward_address, roles = EXCLUDED.roles,
			   bond_uhash = EXCLUDED.bond_uhash, declared_storage = EXCLUDED.declared_storage, jailed = EXCLUDED.jailed,
			   fraud_score = EXCLUDED.fraud_score, moniker = EXCLUDED.moniker, synced_at = now()`,
			op, str(m, "reward_address"), roles, bond, declared, jailed, fraud, str(m, "moniker"))
		return err
	})
}
