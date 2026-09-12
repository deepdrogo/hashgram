package indexer

import (
	"net/http/httptest"
	"testing"
	"time"
)

func TestEscapeLike(t *testing.T) {
	cases := map[string]string{
		"alice":      "alice",
		"a_b":        `a\_b`,
		"100%":       `100\%`,
		`back\slash`: `back\\slash`,
		"%_%":        `\%\_\%`,
		"":           "",
		`\%`:         `\\\%`,
	}
	for in, want := range cases {
		if got := escapeLike(in); got != want {
			t.Errorf("escapeLike(%q) = %q want %q", in, got, want)
		}
	}
}

func TestCursorHelpers(t *testing.T) {
	for in, want := range map[string]int64{"": 0, "0": 0, "50": 50, " 7 ": 7, "-1": 0, "abc": 0, "1e3": 0} {
		if got := parseCursor(in); got != want {
			t.Errorf("parseCursor(%q) = %d want %d", in, got, want)
		}
	}
	r := httptest.NewRequest("GET", "/v1/leaderboards/holders?cursor=120&limit=500", nil)
	if got := cursorOf(r); got != 120 {
		t.Errorf("cursorOf = %d want 120", got)
	}
	if got := limitOf(r, leaderboardDefaultLimit, leaderboardMaxLimit); got != leaderboardMaxLimit {
		t.Errorf("limit not capped: %d", got)
	}
	r = httptest.NewRequest("GET", "/v1/leaderboards/holders", nil)
	if got := limitOf(r, leaderboardDefaultLimit, leaderboardMaxLimit); got != leaderboardDefaultLimit {
		t.Errorf("default limit = %d want %d", got, leaderboardDefaultLimit)
	}

	if got := nextCursor(50, 50, 50); got != "50" {
		t.Errorf("full page should continue, got %q", got)
	}
	if got := nextCursor(37, 37, 50); got != "" {
		t.Errorf("short page should end, got %q", got)
	}
	if got := nextCursor(0, 0, 50); got != "" {
		t.Errorf("empty page should end, got %q", got)
	}
}

func TestNormalizeRole(t *testing.T) {
	for in, want := range map[string]string{"SERVICE_ROLE_STORAGE": "storage", "storage": "storage", " Relay ": "relay", "": ""} {
		if got := normalizeRole(in); got != want {
			t.Errorf("normalizeRole(%q) = %q want %q", in, got, want)
		}
	}
}

func TestBlockTimeSeconds(t *testing.T) {
	t0 := time.Unix(1_700_000_000, 0)
	if got := blockTimeSeconds(101, t0, t0.Add(500*time.Second)); got != 5 {
		t.Errorf("100 intervals over 500s = %v want 5", got)
	}
	if got := blockTimeSeconds(1, t0, t0); got != 0 {
		t.Errorf("single block should be 0, got %v", got)
	}
	if got := blockTimeSeconds(0, time.Time{}, time.Time{}); got != 0 {
		t.Errorf("no blocks should be 0, got %v", got)
	}
	if got := blockTimeSeconds(3, t0.Add(time.Second), t0); got != 0 {
		t.Errorf("non-monotonic clock should be 0, got %v", got)
	}
}

// TestRoutesRegistered: every documented network endpoint resolves to a
// handler (a 404 from the mux would mean a typo in the route table). The
// handlers themselves need PostgreSQL and are exercised operationally.
func TestRoutesRegistered(t *testing.T) {
	a := &API{}
	mux := a.routes()
	for _, p := range []string{
		"/v1/leaderboards/holders", "/v1/leaderboards/validators", "/v1/leaderboards/providers",
		"/v1/leaderboards/earners", "/v1/validators", "/v1/validators/hashvaloper1x", "/v1/network/stats",
		// Pre-existing endpoints must still resolve.
		"/v1/health", "/v1/stats", "/v1/search/users", "/v1/providers",
	} {
		// Resolve the pattern without invoking the handler.
		r := httptest.NewRequest("GET", p, nil)
		if _, pattern := mux.Handler(r); pattern == "" {
			t.Errorf("no route for %s", p)
		}
	}
	if _, pattern := mux.Handler(httptest.NewRequest("POST", "/v1/network/stats", nil)); pattern != "" {
		t.Errorf("POST should not match a GET route, got %q", pattern)
	}
	if a.Handler() == nil {
		t.Fatal("Handler returned nil")
	}
}
