package indexer

import (
	"context"
	"errors"
	"net/http"
	"strconv"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"
)

// Network read endpoints: balances, validators, leaderboards, network stats.
//
// Privacy: the only address -> name relation exposed here is the public
// on-chain username registration (usernames.owner). Nothing is inferred
// from social data, transfers or profiles.
//
// Pagination: ranked lists use `?cursor=<rank>` and return
// {"items": [...], "next_cursor": "<rank>"}; `next_cursor` is omitted on the
// last page. `limit` defaults to 50 and is capped at 200.

const (
	leaderboardDefaultLimit = 50
	leaderboardMaxLimit     = 200
)

// rankedPage is the envelope of every ranked list.
type rankedPage struct {
	Items      any    `json:"items"`
	NextCursor string `json:"next_cursor,omitempty"`
}

// cursorOf reads the rank cursor: rows with rank > cursor are returned.
func cursorOf(r *http.Request) int64 {
	return parseCursor(r.URL.Query().Get("cursor"))
}

// parseCursor turns a cursor string into a rank offset; anything that is
// not a non-negative integer is the start of the list.
func parseCursor(s string) int64 {
	n, err := strconv.ParseInt(strings.TrimSpace(s), 10, 64)
	if err != nil || n < 0 {
		return 0
	}
	return n
}

// nextCursor is the cursor for the page after one ending at lastRank, or ""
// when the page was short (no further rows).
func nextCursor(lastRank int64, got, limit int) string {
	if got < limit || got == 0 {
		return ""
	}
	return strconv.FormatInt(lastRank, 10)
}

// normalizeRole maps a role filter to the lowercase form stored in
// providers.roles ("SERVICE_ROLE_STORAGE" and "storage" both become "storage").
func normalizeRole(s string) string {
	s = strings.ToLower(strings.TrimSpace(s))
	return strings.TrimPrefix(s, "service_role_")
}

// blockTimeSeconds estimates the average block interval from the span of
// n consecutive block timestamps. Fewer than two blocks yield 0.
func blockTimeSeconds(n int64, first, last time.Time) float64 {
	if n < 2 || !last.After(first) {
		return 0
	}
	return last.Sub(first).Seconds() / float64(n-1)
}

// --- holders -----------------------------------------------------------------

type holderRow struct {
	Rank     int64  `json:"rank"`
	Address  string `json:"address"`
	Amount   string `json:"amount"`
	Kind     string `json:"kind"`
	Label    string `json:"label,omitempty"`
	Username string `json:"username,omitempty"`
	Verified bool   `json:"verified"`
}

func (a *API) leaderboardHolders(w http.ResponseWriter, r *http.Request) {
	limit, cursor := limitOf(r, leaderboardDefaultLimit, leaderboardMaxLimit), cursorOf(r)
	rows, err := a.db.Query(r.Context(), `
SELECT b.rank, b.address, b.amount::text, COALESCE(u.name, '')
FROM (SELECT row_number() OVER (ORDER BY amount DESC, address) AS rank, address, amount
      FROM balances WHERE amount > 0) b
LEFT JOIN LATERAL (SELECT name FROM usernames WHERE owner = b.address ORDER BY name LIMIT 1) u ON true
WHERE b.rank > $1 ORDER BY b.rank LIMIT $2`, cursor, limit)
	if err != nil {
		writeErr(w, 500, err.Error())
		return
	}
	defer rows.Close()
	out := []holderRow{}
	var last int64
	for rows.Next() {
		var h holderRow
		if err := rows.Scan(&h.Rank, &h.Address, &h.Amount, &h.Username); err != nil {
			writeErr(w, 500, err.Error())
			return
		}
		h.Kind, h.Label = accountKind(h.Address)
		h.Verified = h.Username != ""
		last = h.Rank
		out = append(out, h)
	}
	if err := rows.Err(); err != nil {
		writeErr(w, 500, err.Error())
		return
	}
	writeJSON(w, 200, rankedPage{Items: out, NextCursor: nextCursor(last, len(out), limit)})
}

// --- validators ---------------------------------------------------------------

type validatorAPIRow struct {
	Rank           int64  `json:"rank"`
	Operator       string `json:"operator"`
	Moniker        string `json:"moniker"`
	Tokens         string `json:"tokens"`
	CommissionRate string `json:"commission_rate"`
	Status         string `json:"status"`
	Jailed         bool   `json:"jailed"`
	UpdatedHeight  int64  `json:"updated_height"`
}

const validatorSelect = `
SELECT rank, operator, moniker, tokens::text, commission_rate, status, jailed, updated_height
FROM (SELECT row_number() OVER (ORDER BY tokens DESC, operator) AS rank, operator, moniker, tokens, commission_rate, status, jailed, updated_height
      FROM validators) v`

func (a *API) queryValidators(ctx context.Context, where string, args ...any) ([]validatorAPIRow, error) {
	rows, err := a.db.Query(ctx, validatorSelect+where, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	out := []validatorAPIRow{}
	for rows.Next() {
		var v validatorAPIRow
		if err := rows.Scan(&v.Rank, &v.Operator, &v.Moniker, &v.Tokens, &v.CommissionRate, &v.Status, &v.Jailed, &v.UpdatedHeight); err != nil {
			return nil, err
		}
		out = append(out, v)
	}
	return out, rows.Err()
}

func (a *API) rankedValidators(w http.ResponseWriter, r *http.Request) {
	limit, cursor := limitOf(r, leaderboardDefaultLimit, leaderboardMaxLimit), cursorOf(r)
	out, err := a.queryValidators(r.Context(), " WHERE rank > $1 ORDER BY rank LIMIT $2", cursor, limit)
	if err != nil {
		writeErr(w, 500, err.Error())
		return
	}
	var last int64
	if len(out) > 0 {
		last = out[len(out)-1].Rank
	}
	writeJSON(w, 200, rankedPage{Items: out, NextCursor: nextCursor(last, len(out), limit)})
}

func (a *API) leaderboardValidators(w http.ResponseWriter, r *http.Request) { a.rankedValidators(w, r) }

func (a *API) validators(w http.ResponseWriter, r *http.Request) { a.rankedValidators(w, r) }

func (a *API) validator(w http.ResponseWriter, r *http.Request) {
	out, err := a.queryValidators(r.Context(), " WHERE operator = $1", r.PathValue("operator"))
	if err != nil {
		writeErr(w, 500, err.Error())
		return
	}
	if len(out) == 0 {
		writeErr(w, 404, "not found")
		return
	}
	writeJSON(w, 200, out[0])
}

// --- providers / earners ----------------------------------------------------------

type providerRankRow struct {
	Rank            int64    `json:"rank"`
	Operator        string   `json:"operator"`
	Moniker         string   `json:"moniker"`
	Roles           []string `json:"roles"`
	TotalPaid       string   `json:"total_paid"`
	Bond            string   `json:"bond_uhash"`
	DeclaredStorage int64    `json:"declared_storage_bytes"`
	Jailed          bool     `json:"jailed"`
	FraudScore      int      `json:"fraud_score"`
}

// leaderboardProviders serves both /v1/leaderboards/providers and
// /v1/leaderboards/earners: providers by lifetime earnings, then bond.
func (a *API) leaderboardProviders(w http.ResponseWriter, r *http.Request) {
	limit, cursor := limitOf(r, leaderboardDefaultLimit, leaderboardMaxLimit), cursorOf(r)
	role := normalizeRole(r.URL.Query().Get("role"))
	rows, err := a.db.Query(r.Context(), `
SELECT rank, operator, moniker, roles, total_paid::text, bond_uhash::text, declared_storage, jailed, fraud_score
FROM (SELECT row_number() OVER (ORDER BY total_paid DESC, bond_uhash DESC, operator) AS rank,
             operator, moniker, roles, total_paid, bond_uhash, declared_storage, jailed, fraud_score
      FROM providers WHERE $3::text = '' OR $3::text = ANY(roles)) p
WHERE rank > $1 ORDER BY rank LIMIT $2`, cursor, limit, role)
	if err != nil {
		writeErr(w, 500, err.Error())
		return
	}
	defer rows.Close()
	out := []providerRankRow{}
	var last int64
	for rows.Next() {
		var p providerRankRow
		if err := rows.Scan(&p.Rank, &p.Operator, &p.Moniker, &p.Roles, &p.TotalPaid, &p.Bond, &p.DeclaredStorage, &p.Jailed, &p.FraudScore); err != nil {
			writeErr(w, 500, err.Error())
			return
		}
		if p.Roles == nil {
			p.Roles = []string{}
		}
		last = p.Rank
		out = append(out, p)
	}
	if err := rows.Err(); err != nil {
		writeErr(w, 500, err.Error())
		return
	}
	writeJSON(w, 200, rankedPage{Items: out, NextCursor: nextCursor(last, len(out), limit)})
}

// --- network stats -------------------------------------------------------------

// networkStats is one document of headline figures. Every field is derived
// from the projection; a query failure leaves its field at the zero value
// rather than failing the whole response, and is logged.
func (a *API) networkStats(w http.ResponseWriter, r *http.Request) {
	ctx := r.Context()
	out := map[string]any{}
	warn := func(what string, err error) {
		if err != nil && !errors.Is(err, pgx.ErrNoRows) {
			a.log.Debug("network stats", "field", what, "error", err)
		}
	}

	indexed, err := getStateInt(ctx, a.db, "chain_height")
	warn("height", err)
	latest, err := getStateInt(ctx, a.db, stateChainLatest)
	warn("chain_latest", err)
	if latest < indexed {
		latest = indexed
	}
	out["height"] = indexed
	out["chain_latest_height"] = latest
	out["indexer_lag"] = latest - indexed

	var n int64
	var first, last time.Time
	err = a.db.QueryRow(ctx, `SELECT count(*), COALESCE(min(time), to_timestamp(0)), COALESCE(max(time), to_timestamp(0))
		FROM (SELECT time FROM blocks ORDER BY height DESC LIMIT 101) t`).Scan(&n, &first, &last)
	warn("block_time", err)
	out["block_time_seconds"] = blockTimeSeconds(n, first, last)
	if err == nil && n > 0 {
		out["latest_block_time"] = last
	}

	var tx24 int64
	warn("tx_count_24h", a.db.QueryRow(ctx, `SELECT COALESCE(sum(tx_count), 0) FROM blocks WHERE time >= now() - interval '24 hours'`).Scan(&tx24))
	out["tx_count_24h"] = tx24

	var accounts int64
	var supply string
	warn("balances", a.db.QueryRow(ctx, `SELECT count(*) FILTER (WHERE amount > 0), COALESCE(sum(amount), 0)::text FROM balances`).Scan(&accounts, &supply))
	out["accounts_with_balance"] = accounts
	out["total_supply"] = supply
	balHeight, err := getStateInt(ctx, a.db, stateBalancesHeight)
	warn("balances_height", err)
	partial, err := getState(ctx, a.db, stateBalancesPartial)
	warn("balances_partial", err)
	out["balances_height"] = balHeight
	out["balances_partial"] = partial != ""

	var vTotal, vBonded, vJailed int64
	var bonded string
	warn("validators", a.db.QueryRow(ctx, `SELECT count(*), count(*) FILTER (WHERE status = 'bonded'), count(*) FILTER (WHERE jailed),
		COALESCE(sum(tokens) FILTER (WHERE status = 'bonded'), 0)::text FROM validators`).Scan(&vTotal, &vBonded, &vJailed, &bonded))
	out["bonded_tokens"] = bonded
	out["validators"] = map[string]any{"total": vTotal, "bonded": vBonded, "jailed": vJailed}

	var pTotal int64
	warn("providers", a.db.QueryRow(ctx, `SELECT count(*) FROM providers`).Scan(&pTotal))
	byRole := map[string]int64{}
	if rows, err := a.db.Query(ctx, `SELECT r, count(*) FROM providers, unnest(roles) AS r GROUP BY r`); err == nil {
		for rows.Next() {
			var role string
			var cnt int64
			if rows.Scan(&role, &cnt) == nil {
				byRole[role] = cnt
			}
		}
		rows.Close()
	} else {
		warn("providers_by_role", err)
	}
	out["providers"] = map[string]any{"total": pTotal, "by_role": byRole}

	counts := map[string]int64{}
	for t, q := range map[string]string{
		"usernames":  "SELECT count(*) FROM usernames",
		"identities": "SELECT count(*) FROM identities",
		"posts":      "SELECT count(*) FROM posts WHERE NOT deleted",
		"profiles":   "SELECT count(*) FROM profiles",
	} {
		var c int64
		if err := a.db.QueryRow(ctx, q).Scan(&c); err == nil {
			counts[t] = c
		} else {
			warn(t, err)
		}
	}
	out["usernames"] = counts["usernames"]
	out["identities"] = counts["identities"]
	out["social"] = map[string]any{"posts": counts["posts"], "profiles": counts["profiles"]}

	writeJSON(w, 200, out)
}
