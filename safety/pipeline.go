package safety

import (
	"bytes"
	"context"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"regexp"
	"strings"
	"sync"
	"time"

	"lukechampine.com/blake3"

	"github.com/hashgram/hashgram/pkg/p2ppb"
)

// Item is one piece of public content under review.
type Item struct {
	// EventID is the hex social event id, when the item is or comes from an
	// event.
	EventID string
	// Author address.
	Author string
	// Type of the event.
	Type string
	// Text: post body, caption, comment, profile fields, joined.
	Text string
	// Media: public blobs referenced, with their bytes when fetched.
	Media []MediaItem
}

// MediaItem is one public blob.
type MediaItem struct {
	CID  string
	Mime string
	Size uint64
	// Data is the plaintext, fetched from the node; nil if too large or
	// unavailable.
	Data []byte
	// ContentHash is BLAKE3 of Data.
	ContentHash string
}

// Verdict ordering: higher is more severe. The pipeline keeps the most
// severe verdict any stage returns.
type Verdict int

const (
	// Allow explicitly.
	Allow Verdict = iota + 1
	// Quarantine: hold from public feeds pending review.
	Quarantine
	// Restrict: age-gate / limit distribution.
	Restrict
	// Block: never serve.
	Block
)

func (v Verdict) String() string {
	switch v {
	case Allow:
		return "CONTENT_ALLOW"
	case Quarantine:
		return "CONTENT_QUARANTINE"
	case Restrict:
		return "CONTENT_RESTRICT"
	case Block:
		return "CONTENT_BLOCK"
	}
	return "UNSPECIFIED"
}

// Proto maps to the wire enum.
func (v Verdict) Proto() p2ppb.Verdict {
	switch v {
	case Allow:
		return p2ppb.Verdict_CONTENT_ALLOW
	case Quarantine:
		return p2ppb.Verdict_CONTENT_QUARANTINE
	case Restrict:
		return p2ppb.Verdict_CONTENT_RESTRICT
	case Block:
		return p2ppb.Verdict_CONTENT_BLOCK
	}
	return p2ppb.Verdict_VERDICT_UNSPECIFIED
}

// Finding is one stage's output.
type Finding struct {
	Stage      string
	Verdict    Verdict
	ReasonCode string
	// Subject: "event", or a media CID hex when the finding is about one
	// blob rather than the whole event.
	SubjectCID string
}

// Stage is a pluggable check. Stages must not retain content: the pipeline
// holds it for the duration of one evaluation and drops it.
type Stage interface {
	Name() string
	Evaluate(ctx context.Context, item *Item) ([]Finding, error)
}

// Pipeline runs stages in order and merges findings.
type Pipeline struct {
	Stages []Stage
}

// Evaluate returns the findings from every stage. An erroring stage is
// reported and skipped: a broken classifier must not silently allow or
// silently block.
func (p *Pipeline) Evaluate(ctx context.Context, item *Item) ([]Finding, []error) {
	var out []Finding
	var errs []error
	for _, s := range p.Stages {
		f, err := s.Evaluate(ctx, item)
		if err != nil {
			errs = append(errs, fmt.Errorf("%s: %w", s.Name(), err))
			continue
		}
		out = append(out, f...)
	}
	return out, errs
}

// -----------------------------------------------------------------------------
// Known-content matching
// -----------------------------------------------------------------------------

// HashList blocks media whose BLAKE3 content hash is on a list.
//
// The list format is one lowercase hex hash per line, optionally followed by
// a space and a reason code. This is how industry hash lists (which this
// project does not ship) plug in: convert to BLAKE3-of-plaintext and load.
type HashList struct {
	mu     sync.RWMutex
	hashes map[string]string
	path   string
}

// NewHashList loads a list file. A missing file is an empty list.
func NewHashList(path string) (*HashList, error) {
	h := &HashList{hashes: map[string]string{}, path: path}
	return h, h.Reload()
}

// Reload re-reads the file.
func (h *HashList) Reload() error {
	if h.path == "" {
		return nil
	}
	raw, err := os.ReadFile(h.path)
	if err != nil {
		if os.IsNotExist(err) {
			return nil
		}
		return err
	}
	m := map[string]string{}
	for _, line := range strings.Split(string(raw), "\n") {
		line = strings.TrimSpace(line)
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		parts := strings.Fields(line)
		reason := "known-content-match"
		if len(parts) > 1 {
			reason = parts[1]
		}
		m[strings.ToLower(parts[0])] = reason
	}
	h.mu.Lock()
	h.hashes = m
	h.mu.Unlock()
	return nil
}

// Name implements Stage.
func (h *HashList) Name() string { return "hash-list" }

// Evaluate implements Stage.
func (h *HashList) Evaluate(_ context.Context, item *Item) ([]Finding, error) {
	h.mu.RLock()
	defer h.mu.RUnlock()
	var out []Finding
	for _, m := range item.Media {
		if m.ContentHash == "" {
			continue
		}
		if reason, ok := h.hashes[m.ContentHash]; ok {
			out = append(out, Finding{Stage: h.Name(), Verdict: Block, ReasonCode: reason, SubjectCID: m.CID})
		}
	}
	return out, nil
}

// Len is the list size.
func (h *HashList) Len() int {
	h.mu.RLock()
	defer h.mu.RUnlock()
	return len(h.hashes)
}

// -----------------------------------------------------------------------------
// Text rules
// -----------------------------------------------------------------------------

// TextRule is a regular expression with a verdict.
type TextRule struct {
	Pattern string `json:"pattern"`
	Verdict string `json:"verdict"`
	Reason  string `json:"reason"`
	re      *regexp.Regexp
	verdict Verdict
}

// TextRules applies regular-expression rules to text. Deliberately simple:
// it catches scam patterns, banned links and the like, and it is the stage
// an operator can extend without a model.
type TextRules struct {
	rules []TextRule
}

// NewTextRules loads a JSON array of rules.
func NewTextRules(path string) (*TextRules, error) {
	t := &TextRules{}
	if path == "" {
		return t, nil
	}
	raw, err := os.ReadFile(path)
	if err != nil {
		if os.IsNotExist(err) {
			return t, nil
		}
		return nil, err
	}
	var rules []TextRule
	if err := json.Unmarshal(raw, &rules); err != nil {
		return nil, fmt.Errorf("%s: %w", path, err)
	}
	for i := range rules {
		re, err := regexp.Compile("(?i)" + rules[i].Pattern)
		if err != nil {
			return nil, fmt.Errorf("%s rule %d: %w", path, i, err)
		}
		rules[i].re = re
		rules[i].verdict = ParseVerdict(rules[i].Verdict)
		if rules[i].verdict == 0 {
			return nil, fmt.Errorf("%s rule %d: unknown verdict %q", path, i, rules[i].Verdict)
		}
		if rules[i].Reason == "" {
			rules[i].Reason = "text-rule"
		}
	}
	t.rules = rules
	return t, nil
}

// ParseVerdict accepts "BLOCK", "CONTENT_BLOCK", "restrict", etc.
func ParseVerdict(s string) Verdict {
	switch strings.ToUpper(strings.TrimPrefix(strings.ToUpper(s), "CONTENT_")) {
	case "ALLOW":
		return Allow
	case "QUARANTINE":
		return Quarantine
	case "RESTRICT":
		return Restrict
	case "BLOCK":
		return Block
	}
	return 0
}

// Name implements Stage.
func (t *TextRules) Name() string { return "text-rules" }

// Evaluate implements Stage.
func (t *TextRules) Evaluate(_ context.Context, item *Item) ([]Finding, error) {
	if item.Text == "" {
		return nil, nil
	}
	var out []Finding
	for _, r := range t.rules {
		if r.re.MatchString(item.Text) {
			out = append(out, Finding{Stage: t.Name(), Verdict: r.verdict, ReasonCode: r.Reason})
		}
	}
	return out, nil
}

// Len is the rule count.
func (t *TextRules) Len() int { return len(t.rules) }

// -----------------------------------------------------------------------------
// External model adapter
// -----------------------------------------------------------------------------

// HTTPModel calls an external classifier. Any provider works that accepts
//
//	POST {url}  {"kind":"text"|"image"|"video","event_id":"…","text":"…","mime":"…","data_base64":"…"}
//
// and answers
//
//	{"verdict":"allow"|"quarantine"|"restrict"|"block","reason":"…","confidence":0.0-1.0}
//
// Replaceable by configuration: the engine has no opinion about which model
// runs, only about the interface. Media bytes are sent only when
// `send_media` is set, so an operator can run text-only classification with
// nothing leaving the machine but text.
type HTTPModel struct {
	URL           string
	Token         string
	SendMedia     bool
	MinConfidence float64
	Timeout       time.Duration
	http          *http.Client
}

// NewHTTPModel constructs one.
func NewHTTPModel(url, token string, sendMedia bool, minConfidence float64, timeout time.Duration) *HTTPModel {
	if timeout == 0 {
		timeout = 30 * time.Second
	}
	return &HTTPModel{URL: url, Token: token, SendMedia: sendMedia, MinConfidence: minConfidence, Timeout: timeout, http: &http.Client{Timeout: timeout}}
}

// Name implements Stage.
func (m *HTTPModel) Name() string { return "http-model" }

type modelRequest struct {
	Kind       string `json:"kind"`
	EventID    string `json:"event_id"`
	Text       string `json:"text,omitempty"`
	Mime       string `json:"mime,omitempty"`
	CID        string `json:"cid,omitempty"`
	DataBase64 []byte `json:"data_base64,omitempty"`
}

type modelResponse struct {
	Verdict    string  `json:"verdict"`
	Reason     string  `json:"reason"`
	Confidence float64 `json:"confidence"`
}

func (m *HTTPModel) call(ctx context.Context, req modelRequest) (*modelResponse, error) {
	body, _ := json.Marshal(req)
	r, err := http.NewRequestWithContext(ctx, http.MethodPost, m.URL, bytes.NewReader(body))
	if err != nil {
		return nil, err
	}
	r.Header.Set("Content-Type", "application/json")
	if m.Token != "" {
		r.Header.Set("Authorization", "Bearer "+m.Token)
	}
	resp, err := m.http.Do(r)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode/100 != 2 {
		return nil, fmt.Errorf("model returned HTTP %d", resp.StatusCode)
	}
	var out modelResponse
	if err := json.NewDecoder(resp.Body).Decode(&out); err != nil {
		return nil, err
	}
	return &out, nil
}

// Evaluate implements Stage.
func (m *HTTPModel) Evaluate(ctx context.Context, item *Item) ([]Finding, error) {
	var out []Finding
	if item.Text != "" {
		resp, err := m.call(ctx, modelRequest{Kind: "text", EventID: item.EventID, Text: item.Text})
		if err != nil {
			return nil, err
		}
		if v := ParseVerdict(resp.Verdict); v > Allow && resp.Confidence >= m.MinConfidence {
			out = append(out, Finding{Stage: m.Name(), Verdict: v, ReasonCode: sanitizeReason(resp.Reason)})
		}
	}
	if m.SendMedia {
		for _, md := range item.Media {
			if md.Data == nil {
				continue
			}
			kind := "image"
			if strings.HasPrefix(md.Mime, "video/") {
				kind = "video"
			}
			resp, err := m.call(ctx, modelRequest{Kind: kind, EventID: item.EventID, Mime: md.Mime, CID: md.CID, DataBase64: md.Data})
			if err != nil {
				return nil, err
			}
			if v := ParseVerdict(resp.Verdict); v > Allow && resp.Confidence >= m.MinConfidence {
				out = append(out, Finding{Stage: m.Name(), Verdict: v, ReasonCode: sanitizeReason(resp.Reason), SubjectCID: md.CID})
			}
		}
	}
	return out, nil
}

// sanitizeReason keeps reason codes machine-readable: a model's free text
// could echo the content it judged, and attestations are public.
func sanitizeReason(s string) string {
	s = strings.ToLower(strings.TrimSpace(s))
	var b strings.Builder
	for _, r := range s {
		switch {
		case r >= 'a' && r <= 'z', r >= '0' && r <= '9', r == '-', r == '_', r == '.':
			b.WriteRune(r)
		case r == ' ':
			b.WriteRune('-')
		}
		if b.Len() >= 48 {
			break
		}
	}
	if b.Len() == 0 {
		return "model"
	}
	return b.String()
}

// ContentHash is BLAKE3 of plaintext media, hex.
func ContentHash(data []byte) string {
	h := blake3.Sum256(data)
	return hex.EncodeToString(h[:])
}
