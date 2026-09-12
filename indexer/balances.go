package indexer

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"math/big"
	"os"
	"sort"
	"strconv"
	"strings"

	"github.com/jackc/pgx/v5"
)

// Balances projection.
//
// `balances` is a projection of x/bank: seeded from the genesis bank
// balances and then folded forward with every `coin_spent` and
// `coin_received` event the chain emits, in begin/end-block (CometBFT 0.38:
// finalize_block_events) as well as in transaction results. Failed
// transactions are included because the SDK still charges their fee and
// reports the ante-handler events on the failed result.
//
// The chain is the source of truth. `hashgram_supply` in `stats` is the sum
// of this table; if it disagrees with the bank supply the projection has
// drifted and `hashgram-indexer rebuild` is the fix.

const (
	stateBalancesSeeded  = "balances_seeded"  // "1" once genesis balances are loaded
	stateBalancesHeight  = "balances_height"  // last height folded into balances
	stateBalancesPartial = "balances_partial" // "1" when early block_results were unavailable
	stateChainLatest     = "chain_latest"     // latest height the chain node reported
	statHashgramSupply   = "hashgram_supply"
)

const bondDenom = "uhash"

// eventAttr is one ABCI event attribute as CometBFT renders it.
type eventAttr struct {
	Key   string `json:"key"`
	Value string `json:"value"`
}

// abciEvent is one ABCI event.
type abciEvent struct {
	Type       string      `json:"type"`
	Attributes []eventAttr `json:"attributes"`
}

func (e abciEvent) attr(key string) string {
	for _, a := range e.Attributes {
		if a.Key == key {
			return a.Value
		}
	}
	return ""
}

// uhashAmount extracts the uhash quantity from a Cosmos coins string such as
// "12345uhash" or "5000uhash,7ibc/ABC". The second result is false when no
// uhash coin is present or the uhash coin is malformed. Other denominations
// are ignored, including ones that merely end in "uhash".
func uhashAmount(coins string) (*big.Int, bool) {
	coins = strings.TrimSpace(coins)
	if coins == "" {
		return nil, false
	}
	for _, coin := range strings.Split(coins, ",") {
		coin = strings.TrimSpace(coin)
		i := 0
		for i < len(coin) && coin[i] >= '0' && coin[i] <= '9' {
			i++
		}
		if i == 0 || coin[i:] != bondDenom {
			continue
		}
		n, ok := new(big.Int).SetString(coin[:i], 10)
		if !ok || n.Sign() < 0 {
			return nil, false
		}
		return n, true
	}
	return nil, false
}

// applyBalanceEvent folds a coin_spent or coin_received event into deltas.
// It returns true when the event changed a balance.
func applyBalanceEvent(deltas map[string]*big.Int, ev abciEvent) bool {
	var addr string
	var sign int
	switch ev.Type {
	case "coin_spent":
		addr, sign = ev.attr("spender"), -1
	case "coin_received":
		addr, sign = ev.attr("receiver"), 1
	default:
		return false
	}
	if addr == "" {
		return false
	}
	amt, ok := uhashAmount(ev.attr("amount"))
	if !ok || amt.Sign() == 0 {
		return false
	}
	if sign < 0 {
		amt = new(big.Int).Neg(amt)
	}
	if cur, ok := deltas[addr]; ok {
		cur.Add(cur, amt)
	} else {
		deltas[addr] = amt
	}
	return true
}

// collectBalanceDeltas sums every balance-changing event of one block:
// begin/end-block (or finalize_block) events and every transaction's events.
func collectBalanceDeltas(r *blockResults) map[string]*big.Int {
	deltas := map[string]*big.Int{}
	for _, ev := range r.Result.BeginBlockEvents {
		applyBalanceEvent(deltas, ev)
	}
	for _, t := range r.Result.TxsResults {
		for _, ev := range t.Events {
			applyBalanceEvent(deltas, ev)
		}
	}
	for _, ev := range r.Result.EndBlockEvents {
		applyBalanceEvent(deltas, ev)
	}
	for _, ev := range r.Result.FinalizeBlockEvents {
		applyBalanceEvent(deltas, ev)
	}
	return deltas
}

// sortedAddrs returns the keys of deltas in a deterministic order so that
// the write pattern, and any deadlock, is reproducible.
func sortedAddrs(deltas map[string]*big.Int) []string {
	out := make([]string, 0, len(deltas))
	for a := range deltas {
		out = append(out, a)
	}
	sort.Strings(out)
	return out
}

// restCoin is a cosmos.base.v1beta1.Coin as REST and genesis JSON render it.
type restCoin struct {
	Denom  string `json:"denom"`
	Amount string `json:"amount"`
}

// sumUhash adds up the uhash coins of a list, ignoring other denominations.
// An empty list is a valid zero; a malformed uhash amount yields false.
func sumUhash(coins []restCoin) (*big.Int, bool) {
	total := new(big.Int)
	for _, coin := range coins {
		if coin.Denom != bondDenom {
			continue
		}
		n, ok := new(big.Int).SetString(strings.TrimSpace(coin.Amount), 10)
		if !ok || n.Sign() < 0 {
			return nil, false
		}
		total.Add(total, n)
	}
	return total, true
}

// genesisBalance is one bank balance from genesis, uhash only.
type genesisBalance struct {
	Address string
	Amount  *big.Int
}

// parseGenesisBalances reads app_state.bank.balances from a genesis document.
// It accepts either the bare genesis file or the CometBFT `/genesis` RPC
// envelope ({"result":{"genesis":{...}}}).
func parseGenesisBalances(doc []byte) ([]genesisBalance, error) {
	var envelope struct {
		Result *struct {
			Genesis json.RawMessage `json:"genesis"`
		} `json:"result"`
		AppState *struct {
			Bank struct {
				Balances []struct {
					Address string     `json:"address"`
					Coins   []restCoin `json:"coins"`
				} `json:"balances"`
			} `json:"bank"`
		} `json:"app_state"`
	}
	if err := json.Unmarshal(doc, &envelope); err != nil {
		return nil, fmt.Errorf("genesis: %w", err)
	}
	if envelope.AppState == nil {
		if envelope.Result == nil || len(envelope.Result.Genesis) == 0 {
			return nil, errors.New("genesis: no app_state and no result.genesis")
		}
		return parseGenesisBalances(envelope.Result.Genesis)
	}
	out := make([]genesisBalance, 0, len(envelope.AppState.Bank.Balances))
	for _, b := range envelope.AppState.Bank.Balances {
		if b.Address == "" {
			continue
		}
		total, ok := sumUhash(b.Coins)
		if !ok {
			return nil, fmt.Errorf("genesis: bad uhash amount for %s", b.Address)
		}
		out = append(out, genesisBalance{Address: b.Address, Amount: total})
	}
	return out, nil
}

// fetchGenesis returns the genesis document: the configured file, else
// `GET /genesis` from the RPC, else the reassembled `/genesis_chunked`.
func (c *ChainIngester) fetchGenesis(ctx context.Context) ([]byte, error) {
	if c.cfg.GenesisPath != "" {
		raw, err := os.ReadFile(c.cfg.GenesisPath)
		if err != nil {
			return nil, fmt.Errorf("genesis_path: %w", err)
		}
		return raw, nil
	}
	var whole struct {
		Result struct {
			Genesis json.RawMessage `json:"genesis"`
		} `json:"result"`
	}
	err := httpJSON(ctx, c.http, c.cfg.ChainRPC+"/genesis", &whole)
	if err == nil && len(whole.Result.Genesis) > 0 {
		return whole.Result.Genesis, nil
	}
	// CometBFT refuses /genesis for large documents; reassemble the chunks.
	var first struct {
		Result struct {
			Total string `json:"total"`
			Data  []byte `json:"data"`
		} `json:"result"`
	}
	if cerr := httpJSON(ctx, c.http, c.cfg.ChainRPC+"/genesis_chunked?chunk=0", &first); cerr != nil {
		if err == nil {
			err = errors.New("empty /genesis response")
		}
		return nil, fmt.Errorf("/genesis: %v; /genesis_chunked: %w", err, cerr)
	}
	total, err := strconv.Atoi(first.Result.Total)
	if err != nil || total < 1 {
		return nil, fmt.Errorf("unexpected genesis chunk total %q", first.Result.Total)
	}
	out := append([]byte(nil), first.Result.Data...)
	for i := 1; i < total; i++ {
		var next struct {
			Result struct {
				Data []byte `json:"data"`
			} `json:"result"`
		}
		if err := httpJSON(ctx, c.http, fmt.Sprintf("%s/genesis_chunked?chunk=%d", c.cfg.ChainRPC, i), &next); err != nil {
			return nil, err
		}
		out = append(out, next.Result.Data...)
	}
	return out, nil
}

// balanceState is the ingester's cached view of the projection cursors. The
// Follow loop is the only writer, so an in-memory copy is safe and saves a
// state query per block.
type balanceState struct {
	loaded bool
	seeded bool
	height int64
}

func (c *ChainIngester) loadBalanceState(ctx context.Context) error {
	if c.bal.loaded {
		return nil
	}
	seeded, err := getState(ctx, c.db, stateBalancesSeeded)
	if err != nil {
		return err
	}
	h, err := getStateInt(ctx, c.db, stateBalancesHeight)
	if err != nil {
		return err
	}
	c.bal = balanceState{loaded: true, seeded: seeded != "", height: h}
	return nil
}

// seedBalances loads the genesis bank balances once.
func (c *ChainIngester) seedBalances(ctx context.Context) error {
	if err := c.loadBalanceState(ctx); err != nil {
		return err
	}
	if c.bal.seeded {
		return nil
	}
	doc, err := c.fetchGenesis(ctx)
	if err != nil {
		return err
	}
	bals, err := parseGenesisBalances(doc)
	if err != nil {
		return err
	}
	tx, err := c.db.Begin(ctx)
	if err != nil {
		return err
	}
	defer tx.Rollback(ctx) //nolint:errcheck
	if _, err := tx.Exec(ctx, "DELETE FROM balances"); err != nil {
		return err
	}
	for _, b := range bals {
		if _, err := tx.Exec(ctx,
			`INSERT INTO balances(address, amount, updated_height) VALUES ($1, $2::numeric, 0)
			 ON CONFLICT (address) DO UPDATE SET amount = balances.amount + EXCLUDED.amount`,
			b.Address, b.Amount.String()); err != nil {
			return err
		}
	}
	if err := setState(ctx, tx, stateBalancesHeight, "0"); err != nil {
		return err
	}
	if err := setState(ctx, tx, stateBalancesSeeded, "1"); err != nil {
		return err
	}
	if err := tx.Commit(ctx); err != nil {
		return err
	}
	c.bal = balanceState{loaded: true, seeded: true, height: 0}
	c.log.Info("balances seeded from genesis", "accounts", len(bals))
	return nil
}

// applyBalancesTx folds one block's deltas into balances inside tx and
// advances the balances cursor in the same transaction, so a crash can
// neither skip nor double-apply a block.
func (c *ChainIngester) applyBalancesTx(ctx context.Context, tx pgx.Tx, height int64, r *blockResults) error {
	deltas := collectBalanceDeltas(r)
	for _, addr := range sortedAddrs(deltas) {
		d := deltas[addr]
		if d.Sign() == 0 {
			continue
		}
		if _, err := tx.Exec(ctx,
			`INSERT INTO balances(address, amount, updated_height) VALUES ($1, $2::numeric, $3)
			 ON CONFLICT (address) DO UPDATE SET amount = balances.amount + EXCLUDED.amount, updated_height = EXCLUDED.updated_height`,
			addr, d.String(), height); err != nil {
			return fmt.Errorf("balance %s: %w", addr, err)
		}
	}
	return setState(ctx, tx, stateBalancesHeight, strconv.FormatInt(height, 10))
}

// catchUpBalances advances the projection towards the indexed chain height
// when it lags, which happens on the first run of a binary with this
// projection against an older index, or after a crash between blocks. At
// most `limit` blocks are processed per call. Returns true when any block
// was applied.
func (c *ChainIngester) catchUpBalances(ctx context.Context, indexed int64, limit int) (bool, error) {
	if err := c.loadBalanceState(ctx); err != nil {
		return false, err
	}
	if !c.bal.seeded {
		return false, nil
	}
	changed := false
	for n := 0; c.bal.height < indexed && n < limit; n++ {
		h := c.bal.height + 1
		var results blockResults
		if err := httpJSON(ctx, c.http, fmt.Sprintf("%s/block_results?height=%d", c.cfg.ChainRPC, h), &results); err != nil {
			// A pruned or state-synced node cannot serve ancient block
			// results. Skip to the first block this index actually holds
			// and say so, rather than retrying forever.
			var minIndexed int64
			if qerr := c.db.QueryRow(ctx, "SELECT COALESCE(min(height), 0) FROM blocks").Scan(&minIndexed); qerr == nil && minIndexed > h {
				c.log.Warn("balances: block results unavailable; projection is partial", "height", h, "resume_from", minIndexed, "error", err)
				if err := setState(ctx, c.db, stateBalancesPartial, "1"); err != nil {
					return changed, err
				}
				if err := setState(ctx, c.db, stateBalancesHeight, strconv.FormatInt(minIndexed-1, 10)); err != nil {
					return changed, err
				}
				c.bal.height = minIndexed - 1
				continue
			}
			return changed, fmt.Errorf("block %d results: %w", h, err)
		}
		tx, err := c.db.Begin(ctx)
		if err != nil {
			return changed, err
		}
		if err := c.applyBalancesTx(ctx, tx, h, &results); err != nil {
			_ = tx.Rollback(ctx)
			return changed, err
		}
		if err := tx.Commit(ctx); err != nil {
			return changed, err
		}
		c.bal.height = h
		changed = true
	}
	return changed, nil
}

// updateSupplyStat records sum(balances) as hashgram_supply for sanity
// checks against the bank supply.
func (c *ChainIngester) updateSupplyStat(ctx context.Context) error {
	_, err := c.db.Exec(ctx,
		`INSERT INTO stats(key, value, updated_height)
		 SELECT $1::text, COALESCE(sum(amount), 0)::text, $2::bigint FROM balances
		 ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_height = EXCLUDED.updated_height`,
		statHashgramSupply, c.bal.height)
	return err
}
