package safety

import (
	"bytes"
	"context"
	"crypto/ed25519"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/pelletier/go-toml/v2"

	hgparams "github.com/hashgram/hashgram/app/params"
	"github.com/hashgram/hashgram/pkg/p2ppb"
)

// Config is /etc/hashgram/safety.toml.
type Config struct {
	// NodeAPI is the loopback API of the co-located hashgram-node.
	NodeAPI string `toml:"node_api"`
	// Home holds the attestor key and the cursor.
	Home string `toml:"home"`
	// Policy names the rule set verdicts are issued under.
	Policy string `toml:"policy"`
	// PollInterval as a duration string.
	PollIntervalStr string        `toml:"poll_interval"`
	PollInterval    time.Duration `toml:"-"`

	// HashListFile: one BLAKE3 hex per line, optional reason.
	HashListFile string `toml:"hash_list_file"`
	// TextRulesFile: JSON array of {pattern, verdict, reason}.
	TextRulesFile string `toml:"text_rules_file"`

	// External model, optional.
	ModelURL           string  `toml:"model_url"`
	ModelToken         string  `toml:"model_token"`
	ModelSendMedia     bool    `toml:"model_send_media"`
	ModelMinConfidence float64 `toml:"model_min_confidence"`

	// MaxMediaBytes bounds what is fetched for inspection.
	MaxMediaBytes int64 `toml:"max_media_bytes"`

	// PublishAllow also publishes explicit ALLOW verdicts, so an indexer in
	// "quarantine everything until reviewed" mode can release content.
	PublishAllow bool `toml:"publish_allow"`
}

// Defaults fills unset fields.
func (c *Config) Defaults() {
	if c.NodeAPI == "" {
		c.NodeAPI = "http://127.0.0.1:26672"
	}
	if c.Home == "" {
		c.Home = "/var/lib/hashgram/safety"
	}
	if c.Policy == "" {
		c.Policy = "hashgram-public-v1"
	}
	if c.PollIntervalStr != "" {
		if d, err := time.ParseDuration(c.PollIntervalStr); err == nil {
			c.PollInterval = d
		}
	}
	if c.PollInterval == 0 {
		c.PollInterval = 5 * time.Second
	}
	if c.MaxMediaBytes == 0 {
		c.MaxMediaBytes = 64 << 20
	}
	if c.ModelMinConfidence == 0 {
		c.ModelMinConfidence = 0.8
	}
}

// LoadConfig reads a configuration file.
func LoadConfig(path string) (Config, error) {
	var c Config
	raw, err := os.ReadFile(path)
	if err != nil {
		return c, fmt.Errorf("reading %s: %w", path, err)
	}
	if err := toml.Unmarshal(raw, &c); err != nil {
		return c, fmt.Errorf("parsing %s: %w", path, err)
	}
	c.Defaults()
	return c, nil
}

// LoadOrCreateKey reads the attestor key (hex ed25519 seed) or creates it.
func LoadOrCreateKey(home string) (ed25519.PrivateKey, error) {
	path := filepath.Join(home, "attestor.key")
	raw, err := os.ReadFile(path)
	if err == nil {
		seed, err := hex.DecodeString(strings.TrimSpace(string(raw)))
		if err != nil || len(seed) != ed25519.SeedSize {
			return nil, fmt.Errorf("%s is not a 32-byte hex seed", path)
		}
		return ed25519.NewKeyFromSeed(seed), nil
	}
	if !os.IsNotExist(err) {
		return nil, err
	}
	if err := os.MkdirAll(home, 0o700); err != nil {
		return nil, err
	}
	_, priv, err := ed25519.GenerateKey(nil)
	if err != nil {
		return nil, err
	}
	if err := os.WriteFile(path, []byte(hex.EncodeToString(priv.Seed())+"\n"), 0o600); err != nil {
		return nil, err
	}
	return priv, nil
}

// Engine polls the node, evaluates, publishes.
type Engine struct {
	cfg      Config
	identity hgparams.NetworkIdentity
	key      ed25519.PrivateKey
	pipeline *Pipeline
	http     *http.Client
	log      *slog.Logger
	// Stats for the operator.
	Reviewed, Published, Errors int64
}

// NewEngine builds the pipeline from the configuration.
func NewEngine(cfg Config, identity hgparams.NetworkIdentity, key ed25519.PrivateKey, log *slog.Logger) (*Engine, error) {
	p := &Pipeline{}
	hl, err := NewHashList(cfg.HashListFile)
	if err != nil {
		return nil, fmt.Errorf("hash list: %w", err)
	}
	p.Stages = append(p.Stages, hl)
	tr, err := NewTextRules(cfg.TextRulesFile)
	if err != nil {
		return nil, fmt.Errorf("text rules: %w", err)
	}
	p.Stages = append(p.Stages, tr)
	if cfg.ModelURL != "" {
		p.Stages = append(p.Stages, NewHTTPModel(cfg.ModelURL, cfg.ModelToken, cfg.ModelSendMedia, cfg.ModelMinConfidence, 0))
	}
	log.Info("safety pipeline", "hash_list", hl.Len(), "text_rules", tr.Len(), "model", cfg.ModelURL != "", "policy", cfg.Policy)
	return &Engine{cfg: cfg, identity: identity, key: key, pipeline: p, http: &http.Client{Timeout: 60 * time.Second}, log: log}, nil
}

// AttestorPubkey is the hex public key to configure as trusted.
func (e *Engine) AttestorPubkey() string {
	return hex.EncodeToString(e.key.Public().(ed25519.PublicKey))
}

func (e *Engine) cursorPath() string { return filepath.Join(e.cfg.Home, "cursor") }

func (e *Engine) loadCursor() (string, string) {
	raw, err := os.ReadFile(e.cursorPath())
	if err != nil {
		return "", ""
	}
	ts, id, _ := strings.Cut(strings.TrimSpace(string(raw)), ":")
	return ts, id
}

func (e *Engine) saveCursor(ts, id string) {
	_ = os.WriteFile(e.cursorPath(), []byte(ts+":"+id+"\n"), 0o600)
}

type nodeEvent struct {
	ID        string `json:"id"`
	Type      string `json:"type"`
	Author    string `json:"author"`
	Timestamp uint64 `json:"timestamp"`
	Payload   string `json:"payload"`
	Media     []struct {
		CID  string `json:"cid"`
		Mime string `json:"mime"`
		Size uint64 `json:"size"`
		Kind string `json:"kind"`
	} `json:"media"`
}

// Run polls until the context ends.
func (e *Engine) Run(ctx context.Context) {
	for {
		if ctx.Err() != nil {
			return
		}
		n, err := e.step(ctx)
		if err != nil {
			e.Errors++
			e.log.Warn("safety step", "error", err)
		}
		if n == 0 || err != nil {
			select {
			case <-ctx.Done():
				return
			case <-time.After(e.cfg.PollInterval):
			}
		}
	}
}

func (e *Engine) step(ctx context.Context) (int, error) {
	ts, id := e.loadCursor()
	u := e.cfg.NodeAPI + "/v1/social/events?limit=200"
	if ts != "" {
		u += "&after_ts=" + url.QueryEscape(ts) + "&after_id=" + url.QueryEscape(id)
	}
	var events []nodeEvent
	if err := e.getJSON(ctx, u, &events); err != nil {
		return 0, err
	}
	for _, ev := range events {
		if err := e.review(ctx, ev); err != nil {
			e.log.Warn("review", "event", ev.ID, "error", err)
		}
		e.saveCursor(strconv.FormatUint(ev.Timestamp, 10), ev.ID)
	}
	return len(events), nil
}

func (e *Engine) getJSON(ctx context.Context, u string, out any) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, u, nil)
	if err != nil {
		return err
	}
	resp, err := e.http.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode/100 != 2 {
		b, _ := io.ReadAll(io.LimitReader(resp.Body, 512))
		return fmt.Errorf("%s: HTTP %d: %s", u, resp.StatusCode, string(b))
	}
	return json.NewDecoder(io.LimitReader(resp.Body, 64<<20)).Decode(out)
}

// textOf extracts the human-readable fields of an event's payload.
func textOf(ev nodeEvent) string {
	raw, err := hex.DecodeString(ev.Payload)
	if err != nil {
		return ""
	}
	var parts []string
	add := func(s string) {
		if s != "" {
			parts = append(parts, s)
		}
	}
	switch ev.Type {
	case "POST_CREATE":
		var p p2ppb.PostCreate
		if unmarshal(raw, &p) {
			add(p.Text)
			add(strings.Join(p.Hashtags, " "))
		}
	case "POST_EDIT":
		var p p2ppb.PostEdit
		if unmarshal(raw, &p) {
			add(p.Text)
		}
	case "COMMENT_CREATE":
		var p p2ppb.CommentCreate
		if unmarshal(raw, &p) {
			add(p.Text)
		}
	case "PROFILE_UPDATE":
		var p p2ppb.ProfileUpdate
		if unmarshal(raw, &p) {
			add(p.DisplayName)
			add(p.Bio)
			add(p.Website)
		}
	case "CHANNEL_CREATE":
		var p p2ppb.ChannelCreate
		if unmarshal(raw, &p) {
			add(p.Name)
			add(p.Description)
		}
	case "REEL_CREATE":
		var p p2ppb.ReelCreate
		if unmarshal(raw, &p) {
			add(p.Caption)
			add(strings.Join(p.Hashtags, " "))
		}
	case "STORY_CREATE":
		var p p2ppb.StoryCreate
		if unmarshal(raw, &p) {
			add(p.Caption)
		}
	case "REPOST":
		var p p2ppb.Repost
		if unmarshal(raw, &p) {
			add(p.Comment)
		}
	}
	return strings.Join(parts, "\n")
}

func (e *Engine) review(ctx context.Context, ev nodeEvent) error {
	e.Reviewed++
	item := &Item{EventID: ev.ID, Author: ev.Author, Type: ev.Type, Text: textOf(ev)}
	for _, m := range ev.Media {
		mi := MediaItem{CID: m.CID, Mime: m.Mime, Size: m.Size}
		if int64(m.Size) <= e.cfg.MaxMediaBytes {
			if data, err := e.fetchBlob(ctx, m.CID); err == nil {
				mi.Data = data
				mi.ContentHash = ContentHash(data)
			} else {
				e.log.Debug("media not fetched", "cid", m.CID, "error", err)
			}
		}
		item.Media = append(item.Media, mi)
	}
	findings, errs := e.pipeline.Evaluate(ctx, item)
	for _, err := range errs {
		e.Errors++
		e.log.Warn("stage failed", "event", ev.ID, "error", err)
	}
	// Merge: most severe per subject.
	best := map[string]Finding{}
	for _, f := range findings {
		key := f.SubjectCID
		if cur, ok := best[key]; !ok || f.Verdict > cur.Verdict {
			best[key] = f
		}
	}
	if len(best) == 0 && e.cfg.PublishAllow {
		best[""] = Finding{Stage: "pipeline", Verdict: Allow, ReasonCode: "reviewed"}
	}
	for subject, f := range best {
		if f.Verdict == Allow && !e.cfg.PublishAllow {
			continue
		}
		a := &p2ppb.ContentAttestation{
			Verdict:    f.Verdict.Proto(),
			Policy:     e.cfg.Policy,
			ReasonCode: f.ReasonCode,
			Timestamp:  uint64(time.Now().Unix()),
		}
		if subject == "" {
			a.EventId, _ = hex.DecodeString(ev.ID)
		} else {
			a.Cid, _ = hex.DecodeString(subject)
		}
		if err := e.Publish(ctx, a); err != nil {
			e.Errors++
			e.log.Warn("publish attestation", "event", ev.ID, "error", err)
			continue
		}
		e.Published++
		e.log.Info("attestation published", "event", ev.ID, "subject", subject, "verdict", f.Verdict.String(), "reason", f.ReasonCode, "stage", f.Stage)
	}
	// Drop content bytes as soon as the review is done.
	for i := range item.Media {
		item.Media[i].Data = nil
	}
	return nil
}

func (e *Engine) fetchBlob(ctx context.Context, cid string) ([]byte, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, e.cfg.NodeAPI+"/v1/blobs/"+cid, nil)
	if err != nil {
		return nil, err
	}
	resp, err := e.http.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != 200 {
		return nil, fmt.Errorf("HTTP %d", resp.StatusCode)
	}
	return io.ReadAll(io.LimitReader(resp.Body, e.cfg.MaxMediaBytes+1))
}

// Publish signs an attestation and hands it to the node, which records it
// and gossips it.
func (e *Engine) Publish(ctx context.Context, a *p2ppb.ContentAttestation) error {
	if err := Sign(e.identity, e.key, a); err != nil {
		return err
	}
	body := map[string]any{
		"cid":             hex.EncodeToString(a.Cid),
		"event_id":        hex.EncodeToString(a.EventId),
		"content_hash":    hex.EncodeToString(a.ContentHash),
		"verdict":         a.Verdict.String(),
		"policy":          a.Policy,
		"reason_code":     a.ReasonCode,
		"timestamp":       a.Timestamp,
		"attestor_pubkey": hex.EncodeToString(a.AttestorPubkey),
		"signature":       hex.EncodeToString(a.Signature),
	}
	raw, _ := json.Marshal(body)
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, e.cfg.NodeAPI+"/v1/safety/attestations", bytes.NewReader(raw))
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", "application/json")
	resp, err := e.http.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode/100 != 2 {
		b, _ := io.ReadAll(io.LimitReader(resp.Body, 512))
		return fmt.Errorf("node refused attestation: HTTP %d: %s", resp.StatusCode, string(b))
	}
	return nil
}
