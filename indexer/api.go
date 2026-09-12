package indexer

import (
	"context"
	"encoding/json"
	"errors"
	"log/slog"
	"net/http"
	"strconv"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
)

// API is the loopback read API. Every handler is a query over derived
// tables; nothing here writes, and nothing here is canonical.
//
// Safety: content blocked by a trusted attestor is never returned. With
// `?safe=1`, content marked sensitive by its author or restricted by a
// trusted attestor is excluded too. This is the "Safe Mode" the applications
// offer, implemented as a filter over the same data.
type API struct {
	db  *pgxpool.Pool
	log *slog.Logger
}

// NewAPI constructs one.
func NewAPI(db *pgxpool.Pool, log *slog.Logger) *API {
	return &API{db: db, log: log}
}

// Handler returns the router.
func (a *API) Handler() http.Handler {
	return withTimeout(a.routes(), 15*time.Second)
}

// routes is the route table; Handler wraps it with the request timeout.
func (a *API) routes() *http.ServeMux {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /v1/health", a.health)
	mux.HandleFunc("GET /v1/stats", a.stats)
	mux.HandleFunc("GET /v1/feed/chronological", a.feedChronological)
	mux.HandleFunc("GET /v1/feed/following/{address}", a.feedFollowing)
	mux.HandleFunc("GET /v1/feed/author/{address}", a.feedAuthor)
	mux.HandleFunc("GET /v1/feed/hashtag/{tag}", a.feedHashtag)
	mux.HandleFunc("GET /v1/feed/channel/{id}", a.feedChannel)
	mux.HandleFunc("GET /v1/reels", a.reels)
	mux.HandleFunc("GET /v1/reels/author/{address}", a.reelsByAuthor)
	mux.HandleFunc("GET /v1/posts/{id}", a.post)
	mux.HandleFunc("GET /v1/profiles/{address}", a.profile)
	mux.HandleFunc("GET /v1/profiles/{address}/followers", a.followers)
	mux.HandleFunc("GET /v1/profiles/{address}/following", a.following)
	mux.HandleFunc("GET /v1/stories/{address}", a.stories)
	mux.HandleFunc("GET /v1/channels", a.channels)
	mux.HandleFunc("GET /v1/search/users", a.searchUsers)
	mux.HandleFunc("GET /v1/search/hashtags", a.searchHashtags)
	mux.HandleFunc("GET /v1/accounts/{address}/transfers", a.transfers)
	mux.HandleFunc("GET /v1/accounts/{address}/transactions", a.transactions)
	mux.HandleFunc("GET /v1/providers", a.providers)
	mux.HandleFunc("GET /v1/blocks/latest", a.latestBlocks)
	mux.HandleFunc("GET /v1/txs/{hash}", a.tx)
	// Network: balances, validators, leaderboards (api_network.go).
	mux.HandleFunc("GET /v1/leaderboards/holders", a.leaderboardHolders)
	mux.HandleFunc("GET /v1/leaderboards/validators", a.leaderboardValidators)
	mux.HandleFunc("GET /v1/leaderboards/providers", a.leaderboardProviders)
	mux.HandleFunc("GET /v1/leaderboards/earners", a.leaderboardProviders)
	mux.HandleFunc("GET /v1/validators", a.validators)
	mux.HandleFunc("GET /v1/validators/{operator}", a.validator)
	mux.HandleFunc("GET /v1/network/stats", a.networkStats)
	return mux
}

func withTimeout(h http.Handler, d time.Duration) http.Handler {
	return http.TimeoutHandler(h, d, `{"error":"timeout"}`)
}

func writeJSON(w http.ResponseWriter, code int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	_ = json.NewEncoder(w).Encode(v)
}

func writeErr(w http.ResponseWriter, code int, msg string) {
	writeJSON(w, code, map[string]string{"error": msg})
}

func limitOf(r *http.Request, def, max int) int {
	n, err := strconv.Atoi(r.URL.Query().Get("limit"))
	if err != nil || n <= 0 {
		return def
	}
	if n > max {
		return max
	}
	return n
}

func beforeOf(r *http.Request) int64 {
	n, err := strconv.ParseInt(r.URL.Query().Get("before"), 10, 64)
	if err != nil || n <= 0 {
		return 1 << 62
	}
	return n
}

func safeMode(r *http.Request) bool {
	v := r.URL.Query().Get("safe")
	return v == "1" || v == "true"
}

// likeEscaper neutralises LIKE metacharacters in user input so that a
// prefix search for "a_b" matches the literal string and not "a" + any
// character + "b". Queries using it must declare ESCAPE '\'.
var likeEscaper = strings.NewReplacer(`\`, `\\`, `%`, `\%`, `_`, `\_`)

// escapeLike escapes %, _ and \ for use inside a LIKE pattern.
func escapeLike(s string) string {
	return likeEscaper.Replace(s)
}

// safetyClause excludes blocked content always and sensitive/restricted
// content in safe mode. `col` is the id column of the content table.
func safetyClause(col string, safe bool, sensitiveCol string) string {
	c := " AND NOT EXISTS (SELECT 1 FROM attestations at WHERE at.subject = " + col +
		" AND at.trusted AND at.verdict = 'CONTENT_BLOCK' AND at.ts >= COALESCE((SELECT max(ts) FROM attestations u WHERE u.subject = at.subject AND u.trusted AND u.verdict = 'CONTENT_UNBLOCK'), 0))"
	if safe {
		if sensitiveCol != "" {
			c += " AND NOT " + sensitiveCol
		}
		c += " AND NOT EXISTS (SELECT 1 FROM attestations at WHERE at.subject = " + col +
			" AND at.trusted AND at.verdict = 'CONTENT_RESTRICT')"
	}
	return c
}

type postRow struct {
	ID        string          `json:"id"`
	Author    string          `json:"author"`
	Text      string          `json:"text"`
	Hashtags  []string        `json:"hashtags"`
	Channel   string          `json:"channel"`
	ReplyTo   string          `json:"reply_to"`
	Sensitive bool            `json:"sensitive"`
	Media     json.RawMessage `json:"media"`
	Ts        int64           `json:"timestamp"`
	EditedTs  *int64          `json:"edited_ts,omitempty"`
	Deleted   bool            `json:"deleted"`
	Reactions int64           `json:"reactions"`
	Comments  int64           `json:"comments"`
	Reposts   int64           `json:"reposts"`
}

const postSelect = `
SELECT p.id, p.author, p.text, p.hashtags, p.channel, p.reply_to, p.sensitive, p.media, p.ts, p.edited_ts, p.deleted,
  (SELECT count(*) FROM reactions r WHERE r.target = p.id),
  (SELECT count(*) FROM comments c WHERE c.post = p.id),
  (SELECT count(*) FROM reposts rp WHERE rp.post = p.id)
FROM posts p WHERE NOT p.deleted AND p.ts < $1`

func (a *API) queryPosts(ctx context.Context, where string, args ...any) ([]postRow, error) {
	rows, err := a.db.Query(ctx, postSelect+where+" ORDER BY p.ts DESC LIMIT $2", args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	out := []postRow{}
	for rows.Next() {
		var p postRow
		var media []byte
		if err := rows.Scan(&p.ID, &p.Author, &p.Text, &p.Hashtags, &p.Channel, &p.ReplyTo, &p.Sensitive, &media, &p.Ts, &p.EditedTs, &p.Deleted, &p.Reactions, &p.Comments, &p.Reposts); err != nil {
			return nil, err
		}
		p.Media = media
		if p.Hashtags == nil {
			p.Hashtags = []string{}
		}
		out = append(out, p)
	}
	return out, rows.Err()
}

func (a *API) health(w http.ResponseWriter, r *http.Request) {
	var height string
	err := a.db.QueryRow(r.Context(), "SELECT value FROM index_state WHERE key = 'chain_height'").Scan(&height)
	if err != nil && !errors.Is(err, pgx.ErrNoRows) {
		writeErr(w, 503, "database: "+err.Error())
		return
	}
	writeJSON(w, 200, map[string]any{"status": "ok", "chain_height": height})
}

func (a *API) stats(w http.ResponseWriter, r *http.Request) {
	out := map[string]any{}
	for _, t := range []string{"blocks", "transactions", "transfers", "usernames", "identities", "providers", "validators", "balances", "social_events", "posts", "reels", "follows", "channels", "attestations"} {
		var n int64
		if err := a.db.QueryRow(r.Context(), "SELECT count(*) FROM "+t).Scan(&n); err == nil {
			out[t] = n
		}
	}
	if h, err := getState(r.Context(), a.db, "chain_height"); err == nil {
		out["chain_height"] = h
	}
	// Sanity figure: sum(balances) as the ingester last computed it. Compare
	// with the bank supply; a mismatch means the projection has drifted.
	var supply string
	if err := a.db.QueryRow(r.Context(), "SELECT value FROM stats WHERE key = $1", statHashgramSupply).Scan(&supply); err == nil {
		out[statHashgramSupply] = supply
	}
	writeJSON(w, 200, out)
}

func (a *API) feedChronological(w http.ResponseWriter, r *http.Request) {
	posts, err := a.queryPosts(r.Context(), safetyClause("p.id", safeMode(r), "p.sensitive"), beforeOf(r), limitOf(r, 50, 200))
	if err != nil {
		writeErr(w, 500, err.Error())
		return
	}
	writeJSON(w, 200, posts)
}

func (a *API) feedFollowing(w http.ResponseWriter, r *http.Request) {
	addr := r.PathValue("address")
	posts, err := a.queryPosts(r.Context(),
		" AND (p.author = $3 OR p.author IN (SELECT target FROM follows WHERE follower = $3))"+safetyClause("p.id", safeMode(r), "p.sensitive"),
		beforeOf(r), limitOf(r, 50, 200), addr)
	if err != nil {
		writeErr(w, 500, err.Error())
		return
	}
	writeJSON(w, 200, posts)
}

func (a *API) feedAuthor(w http.ResponseWriter, r *http.Request) {
	posts, err := a.queryPosts(r.Context(), " AND p.author = $3"+safetyClause("p.id", safeMode(r), "p.sensitive"), beforeOf(r), limitOf(r, 50, 200), r.PathValue("address"))
	if err != nil {
		writeErr(w, 500, err.Error())
		return
	}
	writeJSON(w, 200, posts)
}

func (a *API) feedHashtag(w http.ResponseWriter, r *http.Request) {
	tag := strings.ToLower(strings.TrimPrefix(r.PathValue("tag"), "#"))
	posts, err := a.queryPosts(r.Context(), " AND $3 = ANY(p.hashtags)"+safetyClause("p.id", safeMode(r), "p.sensitive"), beforeOf(r), limitOf(r, 50, 200), tag)
	if err != nil {
		writeErr(w, 500, err.Error())
		return
	}
	writeJSON(w, 200, posts)
}

func (a *API) feedChannel(w http.ResponseWriter, r *http.Request) {
	posts, err := a.queryPosts(r.Context(), " AND p.channel = $3"+safetyClause("p.id", safeMode(r), "p.sensitive"), beforeOf(r), limitOf(r, 50, 200), r.PathValue("id"))
	if err != nil {
		writeErr(w, 500, err.Error())
		return
	}
	writeJSON(w, 200, posts)
}

type reelRow struct {
	ID            string   `json:"id"`
	Author        string   `json:"author"`
	Caption       string   `json:"caption"`
	Hashtags      []string `json:"hashtags"`
	VideoCID      string   `json:"video_cid"`
	VideoMime     string   `json:"video_mime"`
	DurationMs    int      `json:"duration_ms"`
	ThumbnailCID  string   `json:"thumbnail_cid"`
	Sensitive     bool     `json:"sensitive"`
	MinAge        int      `json:"min_age"`
	AllowComments bool     `json:"allow_comments"`
	Ts            int64    `json:"timestamp"`
	Reactions     int64    `json:"reactions"`
	Comments      int64    `json:"comments"`
}

func (a *API) queryReels(ctx context.Context, where string, args ...any) ([]reelRow, error) {
	rows, err := a.db.Query(ctx, `
SELECT r.id, r.author, r.caption, r.hashtags, r.video_cid, r.video_mime, r.duration_ms, r.thumbnail_cid, r.sensitive, r.min_age, r.allow_comments, r.ts,
  (SELECT count(*) FROM reactions x WHERE x.target = r.id),
  (SELECT count(*) FROM comments c WHERE c.post = r.id)
FROM reels r WHERE NOT r.deleted AND r.ts < $1`+where+" ORDER BY r.ts DESC LIMIT $2", args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	out := []reelRow{}
	for rows.Next() {
		var x reelRow
		if err := rows.Scan(&x.ID, &x.Author, &x.Caption, &x.Hashtags, &x.VideoCID, &x.VideoMime, &x.DurationMs, &x.ThumbnailCID, &x.Sensitive, &x.MinAge, &x.AllowComments, &x.Ts, &x.Reactions, &x.Comments); err != nil {
			return nil, err
		}
		if x.Hashtags == nil {
			x.Hashtags = []string{}
		}
		out = append(out, x)
	}
	return out, rows.Err()
}

func (a *API) reels(w http.ResponseWriter, r *http.Request) {
	where := safetyClause("r.id", safeMode(r), "r.sensitive")
	// Age gate: ?max_age=N hides reels whose declared minimum age exceeds N.
	args := []any{beforeOf(r), limitOf(r, 30, 100)}
	if v, err := strconv.Atoi(r.URL.Query().Get("max_age")); err == nil && v > 0 {
		where += " AND r.min_age <= $3"
		args = append(args, v)
	}
	if tag := strings.ToLower(r.URL.Query().Get("tag")); tag != "" {
		where += " AND $" + strconv.Itoa(len(args)+1) + " = ANY(r.hashtags)"
		args = append(args, tag)
	}
	reels, err := a.queryReels(r.Context(), where, args...)
	if err != nil {
		writeErr(w, 500, err.Error())
		return
	}
	writeJSON(w, 200, reels)
}

func (a *API) reelsByAuthor(w http.ResponseWriter, r *http.Request) {
	reels, err := a.queryReels(r.Context(), " AND r.author = $3"+safetyClause("r.id", safeMode(r), "r.sensitive"), beforeOf(r), limitOf(r, 30, 100), r.PathValue("address"))
	if err != nil {
		writeErr(w, 500, err.Error())
		return
	}
	writeJSON(w, 200, reels)
}

func (a *API) post(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	posts, err := a.queryPosts(r.Context(), " AND p.id = $3"+safetyClause("p.id", false, ""), int64(1)<<62, 1, id)
	if err != nil {
		writeErr(w, 500, err.Error())
		return
	}
	if len(posts) == 0 {
		// Maybe a reel.
		reels, err := a.queryReels(r.Context(), " AND r.id = $3"+safetyClause("r.id", false, ""), int64(1)<<62, 1, id)
		if err != nil || len(reels) == 0 {
			writeErr(w, 404, "not found")
			return
		}
		writeJSON(w, 200, map[string]any{"reel": reels[0], "comments": a.commentsOf(r.Context(), id)})
		return
	}
	writeJSON(w, 200, map[string]any{"post": posts[0], "comments": a.commentsOf(r.Context(), id), "reactions": a.reactionsOf(r.Context(), id)})
}

type commentRow struct {
	ID     string `json:"id"`
	Author string `json:"author"`
	Text   string `json:"text"`
	Parent string `json:"parent"`
	Ts     int64  `json:"timestamp"`
}

func (a *API) commentsOf(ctx context.Context, post string) []commentRow {
	rows, err := a.db.Query(ctx, `SELECT id, author, text, parent, ts FROM comments WHERE post = $1`+safetyClause("id", false, "")+` ORDER BY ts LIMIT 500`, post)
	if err != nil {
		return nil
	}
	defer rows.Close()
	out := []commentRow{}
	for rows.Next() {
		var c commentRow
		if rows.Scan(&c.ID, &c.Author, &c.Text, &c.Parent, &c.Ts) == nil {
			out = append(out, c)
		}
	}
	return out
}

func (a *API) reactionsOf(ctx context.Context, target string) map[string]int64 {
	rows, err := a.db.Query(ctx, `SELECT reaction, count(*) FROM reactions WHERE target = $1 GROUP BY reaction`, target)
	if err != nil {
		return nil
	}
	defer rows.Close()
	out := map[string]int64{}
	for rows.Next() {
		var k string
		var n int64
		if rows.Scan(&k, &n) == nil {
			out[k] = n
		}
	}
	return out
}

func (a *API) profile(w http.ResponseWriter, r *http.Request) {
	addr := r.PathValue("address")
	out := map[string]any{"address": addr}
	var name, bio, avatar, banner, website string
	var attrs []byte
	var updated int64
	err := a.db.QueryRow(r.Context(), `SELECT display_name, bio, avatar_cid, banner_cid, website, attributes, updated_ts FROM profiles WHERE address = $1`, addr).
		Scan(&name, &bio, &avatar, &banner, &website, &attrs, &updated)
	if err == nil {
		out["display_name"], out["bio"], out["avatar_cid"], out["banner_cid"], out["website"], out["attributes"], out["updated_ts"] = name, bio, avatar, banner, website, json.RawMessage(attrs), updated
	}
	var username string
	if err := a.db.QueryRow(r.Context(), `SELECT name FROM usernames WHERE owner = $1 LIMIT 1`, addr).Scan(&username); err == nil {
		out["username"] = username
	}
	var followers, following, posts, reels int64
	_ = a.db.QueryRow(r.Context(), `SELECT count(*) FROM follows WHERE target = $1`, addr).Scan(&followers)
	_ = a.db.QueryRow(r.Context(), `SELECT count(*) FROM follows WHERE follower = $1`, addr).Scan(&following)
	_ = a.db.QueryRow(r.Context(), `SELECT count(*) FROM posts WHERE author = $1 AND NOT deleted`, addr).Scan(&posts)
	_ = a.db.QueryRow(r.Context(), `SELECT count(*) FROM reels WHERE author = $1 AND NOT deleted`, addr).Scan(&reels)
	out["followers"], out["following"], out["posts"], out["reels"] = followers, following, posts, reels
	var devices []byte
	if err := a.db.QueryRow(r.Context(), `SELECT devices FROM identities WHERE address = $1`, addr).Scan(&devices); err == nil {
		out["devices"] = json.RawMessage(devices)
	}
	writeJSON(w, 200, out)
}

func (a *API) addressList(w http.ResponseWriter, r *http.Request, sql string) {
	rows, err := a.db.Query(r.Context(), sql, r.PathValue("address"), limitOf(r, 100, 1000))
	if err != nil {
		writeErr(w, 500, err.Error())
		return
	}
	defer rows.Close()
	out := []string{}
	for rows.Next() {
		var s string
		if rows.Scan(&s) == nil {
			out = append(out, s)
		}
	}
	writeJSON(w, 200, out)
}

func (a *API) followers(w http.ResponseWriter, r *http.Request) {
	a.addressList(w, r, `SELECT follower FROM follows WHERE target = $1 ORDER BY ts DESC LIMIT $2`)
}

func (a *API) following(w http.ResponseWriter, r *http.Request) {
	a.addressList(w, r, `SELECT target FROM follows WHERE follower = $1 ORDER BY ts DESC LIMIT $2`)
}

func (a *API) stories(w http.ResponseWriter, r *http.Request) {
	now := time.Now().Unix()
	rows, err := a.db.Query(r.Context(), `SELECT id, author, caption, media, sensitive, ts, expires_at FROM stories WHERE author = $1 AND expires_at > $2`+safetyClause("id", safeMode(r), "sensitive")+` ORDER BY ts DESC LIMIT 50`, r.PathValue("address"), now)
	if err != nil {
		writeErr(w, 500, err.Error())
		return
	}
	defer rows.Close()
	type story struct {
		ID        string          `json:"id"`
		Author    string          `json:"author"`
		Caption   string          `json:"caption"`
		Media     json.RawMessage `json:"media"`
		Sensitive bool            `json:"sensitive"`
		Ts        int64           `json:"timestamp"`
		ExpiresAt int64           `json:"expires_at"`
	}
	out := []story{}
	for rows.Next() {
		var s story
		var media []byte
		if rows.Scan(&s.ID, &s.Author, &s.Caption, &media, &s.Sensitive, &s.Ts, &s.ExpiresAt) == nil {
			s.Media = media
			out = append(out, s)
		}
	}
	writeJSON(w, 200, out)
}

func (a *API) channels(w http.ResponseWriter, r *http.Request) {
	rows, err := a.db.Query(r.Context(), `SELECT c.id, c.creator, c.name, c.description, c.avatar_cid, c.open_posting, c.ts, (SELECT count(*) FROM posts p WHERE p.channel = c.id AND NOT p.deleted) FROM channels c ORDER BY c.ts DESC LIMIT $1`, limitOf(r, 100, 500))
	if err != nil {
		writeErr(w, 500, err.Error())
		return
	}
	defer rows.Close()
	type ch struct {
		ID          string `json:"id"`
		Creator     string `json:"creator"`
		Name        string `json:"name"`
		Description string `json:"description"`
		AvatarCID   string `json:"avatar_cid"`
		OpenPosting bool   `json:"open_posting"`
		Ts          int64  `json:"timestamp"`
		Posts       int64  `json:"posts"`
	}
	out := []ch{}
	for rows.Next() {
		var c ch
		if rows.Scan(&c.ID, &c.Creator, &c.Name, &c.Description, &c.AvatarCID, &c.OpenPosting, &c.Ts, &c.Posts) == nil {
			out = append(out, c)
		}
	}
	writeJSON(w, 200, out)
}

func (a *API) searchUsers(w http.ResponseWriter, r *http.Request) {
	q := strings.ToLower(strings.TrimPrefix(r.URL.Query().Get("q"), "@"))
	if q == "" {
		writeJSON(w, 200, []any{})
		return
	}
	rows, err := a.db.Query(r.Context(), `
SELECT u.name, u.owner, COALESCE(p.display_name, '') FROM usernames u LEFT JOIN profiles p ON p.address = u.owner
WHERE u.name LIKE $1 || '%' ESCAPE '\' ORDER BY u.name LIMIT $2`, escapeLike(q), limitOf(r, 20, 100))
	if err != nil {
		writeErr(w, 500, err.Error())
		return
	}
	defer rows.Close()
	type hit struct {
		Username    string `json:"username"`
		Address     string `json:"address"`
		DisplayName string `json:"display_name"`
	}
	out := []hit{}
	for rows.Next() {
		var h hit
		if rows.Scan(&h.Username, &h.Address, &h.DisplayName) == nil {
			out = append(out, h)
		}
	}
	writeJSON(w, 200, out)
}

func (a *API) searchHashtags(w http.ResponseWriter, r *http.Request) {
	q := strings.ToLower(strings.TrimPrefix(r.URL.Query().Get("q"), "#"))
	rows, err := a.db.Query(r.Context(), `
SELECT tag, count(*) FROM (SELECT unnest(hashtags) AS tag FROM posts WHERE NOT deleted UNION ALL SELECT unnest(hashtags) FROM reels WHERE NOT deleted) t
WHERE tag LIKE $1 || '%' ESCAPE '\' GROUP BY tag ORDER BY count(*) DESC LIMIT $2`, escapeLike(q), limitOf(r, 20, 100))
	if err != nil {
		writeErr(w, 500, err.Error())
		return
	}
	defer rows.Close()
	type hit struct {
		Tag   string `json:"tag"`
		Count int64  `json:"count"`
	}
	out := []hit{}
	for rows.Next() {
		var h hit
		if rows.Scan(&h.Tag, &h.Count) == nil {
			out = append(out, h)
		}
	}
	writeJSON(w, 200, out)
}

func (a *API) transfers(w http.ResponseWriter, r *http.Request) {
	addr := r.PathValue("address")
	rows, err := a.db.Query(r.Context(), `SELECT height, txhash, sender, recipient, amount_uhash::text FROM transfers WHERE sender = $1 OR recipient = $1 ORDER BY height DESC, id DESC LIMIT $2`, addr, limitOf(r, 50, 500))
	if err != nil {
		writeErr(w, 500, err.Error())
		return
	}
	defer rows.Close()
	type t struct {
		Height    int64  `json:"height"`
		TxHash    string `json:"txhash"`
		Sender    string `json:"sender"`
		Recipient string `json:"recipient"`
		Amount    string `json:"amount_uhash"`
	}
	out := []t{}
	for rows.Next() {
		var x t
		if rows.Scan(&x.Height, &x.TxHash, &x.Sender, &x.Recipient, &x.Amount) == nil {
			out = append(out, x)
		}
	}
	writeJSON(w, 200, out)
}

func (a *API) transactions(w http.ResponseWriter, r *http.Request) {
	rows, err := a.db.Query(r.Context(), `SELECT hash, height, code, gas_used, fee_uhash::text, memo, msg_types FROM transactions WHERE $1 = ANY(signers) ORDER BY height DESC LIMIT $2`, r.PathValue("address"), limitOf(r, 50, 500))
	if err != nil {
		writeErr(w, 500, err.Error())
		return
	}
	defer rows.Close()
	type t struct {
		Hash     string   `json:"hash"`
		Height   int64    `json:"height"`
		Code     int      `json:"code"`
		GasUsed  int64    `json:"gas_used"`
		Fee      string   `json:"fee_uhash"`
		Memo     string   `json:"memo"`
		MsgTypes []string `json:"msg_types"`
	}
	out := []t{}
	for rows.Next() {
		var x t
		if rows.Scan(&x.Hash, &x.Height, &x.Code, &x.GasUsed, &x.Fee, &x.Memo, &x.MsgTypes) == nil {
			out = append(out, x)
		}
	}
	writeJSON(w, 200, out)
}

func (a *API) tx(w http.ResponseWriter, r *http.Request) {
	var height int64
	var code int
	var gas int64
	var fee, memo string
	var types, signers []string
	err := a.db.QueryRow(r.Context(), `SELECT height, code, gas_used, fee_uhash::text, memo, msg_types, signers FROM transactions WHERE hash = $1`, strings.ToUpper(r.PathValue("hash"))).
		Scan(&height, &code, &gas, &fee, &memo, &types, &signers)
	if err != nil {
		writeErr(w, 404, "not found")
		return
	}
	writeJSON(w, 200, map[string]any{"hash": strings.ToUpper(r.PathValue("hash")), "height": height, "code": code, "gas_used": gas, "fee_uhash": fee, "memo": memo, "msg_types": types, "signers": signers})
}

func (a *API) providers(w http.ResponseWriter, r *http.Request) {
	rows, err := a.db.Query(r.Context(), `SELECT operator, reward_address, roles, bond_uhash::text, declared_storage, jailed, fraud_score, moniker FROM providers ORDER BY operator LIMIT $1`, limitOf(r, 200, 1000))
	if err != nil {
		writeErr(w, 500, err.Error())
		return
	}
	defer rows.Close()
	type p struct {
		Operator        string   `json:"operator"`
		RewardAddress   string   `json:"reward_address"`
		Roles           []string `json:"roles"`
		Bond            string   `json:"bond_uhash"`
		DeclaredStorage int64    `json:"declared_storage_bytes"`
		Jailed          bool     `json:"jailed"`
		FraudScore      int      `json:"fraud_score"`
		Moniker         string   `json:"moniker"`
	}
	out := []p{}
	for rows.Next() {
		var x p
		if rows.Scan(&x.Operator, &x.RewardAddress, &x.Roles, &x.Bond, &x.DeclaredStorage, &x.Jailed, &x.FraudScore, &x.Moniker) == nil {
			out = append(out, x)
		}
	}
	writeJSON(w, 200, out)
}

func (a *API) latestBlocks(w http.ResponseWriter, r *http.Request) {
	rows, err := a.db.Query(r.Context(), `SELECT height, time, proposer, tx_count FROM blocks ORDER BY height DESC LIMIT $1`, limitOf(r, 20, 200))
	if err != nil {
		writeErr(w, 500, err.Error())
		return
	}
	defer rows.Close()
	type b struct {
		Height   int64     `json:"height"`
		Time     time.Time `json:"time"`
		Proposer string    `json:"proposer"`
		TxCount  int       `json:"tx_count"`
	}
	out := []b{}
	for rows.Next() {
		var x b
		if rows.Scan(&x.Height, &x.Time, &x.Proposer, &x.TxCount) == nil {
			out = append(out, x)
		}
	}
	writeJSON(w, 200, out)
}
