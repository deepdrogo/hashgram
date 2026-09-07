package indexer

import (
	"context"
	"crypto/ed25519"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"log/slog"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"

	"github.com/hashgram/hashgram/app/canonical"
	hgparams "github.com/hashgram/hashgram/app/params"
	"github.com/hashgram/hashgram/pkg/p2ppb"
)

// nodeEvent is the node API's JSON rendering of a social event (hex bytes).
type nodeEvent struct {
	ID            string `json:"id"`
	Type          string `json:"type"`
	Author        string `json:"author"`
	DevicePubkey  string `json:"device_pubkey"`
	Timestamp     uint64 `json:"timestamp"`
	Sequence      uint64 `json:"sequence"`
	PreviousEvent string `json:"previous_event"`
	Payload       string `json:"payload"`
	Media         []struct {
		CID          string `json:"cid"`
		Mime         string `json:"mime"`
		Size         uint64 `json:"size"`
		Kind         string `json:"kind"`
		Width        uint32 `json:"width"`
		Height       uint32 `json:"height"`
		DurationMs   uint32 `json:"duration_ms"`
		ThumbnailCID string `json:"thumbnail_cid"`
		ContentHash  string `json:"content_hash"`
	} `json:"media"`
	Signature string `json:"signature"`
	NetworkID string `json:"network_id"`
	Version   uint32 `json:"version"`
}

// SocialIngester pulls verified events from the node and projects them.
type SocialIngester struct {
	cfg      Config
	db       *pgxpool.Pool
	http     *http.Client
	log      *slog.Logger
	identity hgparams.NetworkIdentity
}

// NewSocialIngester constructs one. The network identity is what events are
// verified against; an event for another network is refused here even if a
// node forwarded it.
func NewSocialIngester(cfg Config, db *pgxpool.Pool, identity hgparams.NetworkIdentity, log *slog.Logger) *SocialIngester {
	return &SocialIngester{cfg: cfg, db: db, http: &http.Client{Timeout: 20 * time.Second}, log: log, identity: identity}
}

// EventPayloadBytes mirrors hashgram-proto's `social_event_payload`: every
// field but id and signature, in tag order, through the canonical encoder.
func EventPayloadBytes(ev *p2ppb.SocialEvent) ([]byte, error) {
	b := canonical.New(256+len(ev.Payload)).
		String("network_id", ev.NetworkId).
		Uint32(ev.Version).
		String("type", ev.Type).
		String("author", ev.Author).
		Bytes("device_pubkey", ev.DevicePubkey).
		Uint64(ev.Timestamp).
		Uint64(ev.Sequence).
		Bytes("previous_event", ev.PreviousEvent).
		Bytes("payload", ev.Payload).
		Len("media", len(ev.Media))
	for _, m := range ev.Media {
		b = b.Bytes("media.cid", m.Cid).
			String("media.mime", m.Mime).
			Uint64(m.Size).
			String("media.kind", m.Kind).
			Uint32(m.Width).
			Uint32(m.Height).
			Uint32(m.DurationMs).
			Bytes("media.thumbnail_cid", m.ThumbnailCid).
			Bytes("media.content_hash", m.ContentHash)
	}
	return b.Finish()
}

// VerifyEvent checks network, id and ed25519 signature. Device authority
// (is this key one of the author's devices?) is checked against the
// identities table, which the chain sync fills.
func (s *SocialIngester) VerifyEvent(ev *p2ppb.SocialEvent) error {
	if ev.NetworkId != s.identity.NetworkID {
		return fmt.Errorf("event is for network %q, this index is %q", ev.NetworkId, s.identity.NetworkID)
	}
	payload, err := EventPayloadBytes(ev)
	if err != nil {
		return err
	}
	preimage := s.identity.SigningPreimage(hgparams.PurposeSocialEvent, payload)
	if id := blake3Sum(preimage); !bytesEq(id, ev.Id) {
		return fmt.Errorf("event id does not match content")
	}
	digest := s.identity.SigningDigest(hgparams.PurposeSocialEvent, payload)
	if len(ev.DevicePubkey) != ed25519.PublicKeySize {
		return fmt.Errorf("device key is %d bytes", len(ev.DevicePubkey))
	}
	if !ed25519.Verify(ed25519.PublicKey(ev.DevicePubkey), digest[:], ev.Signature) {
		return fmt.Errorf("signature does not verify")
	}
	return nil
}

func bytesEq(a, b []byte) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}

func fromNode(n nodeEvent) (*p2ppb.SocialEvent, error) {
	h := func(s string) ([]byte, error) {
		if s == "" {
			return nil, nil
		}
		return hex.DecodeString(s)
	}
	id, err := h(n.ID)
	if err != nil {
		return nil, err
	}
	dev, err := h(n.DevicePubkey)
	if err != nil {
		return nil, err
	}
	prev, err := h(n.PreviousEvent)
	if err != nil {
		return nil, err
	}
	payload, err := h(n.Payload)
	if err != nil {
		return nil, err
	}
	sig, err := h(n.Signature)
	if err != nil {
		return nil, err
	}
	ev := &p2ppb.SocialEvent{
		NetworkId:     n.NetworkID,
		Version:       n.Version,
		Id:            id,
		Type:          n.Type,
		Author:        n.Author,
		DevicePubkey:  dev,
		Timestamp:     n.Timestamp,
		Sequence:      n.Sequence,
		PreviousEvent: prev,
		Payload:       payload,
		Signature:     sig,
	}
	for _, m := range n.Media {
		cid, err := h(m.CID)
		if err != nil {
			return nil, err
		}
		th, _ := h(m.ThumbnailCID)
		ch, _ := h(m.ContentHash)
		ev.Media = append(ev.Media, &p2ppb.MediaReference{
			Cid: cid, Mime: m.Mime, Size: m.Size, Kind: m.Kind, Width: m.Width, Height: m.Height,
			DurationMs: m.DurationMs, ThumbnailCid: th, ContentHash: ch,
		})
	}
	return ev, nil
}

// Follow pulls events from the node until the context ends.
func (s *SocialIngester) Follow(ctx context.Context) {
	for {
		if ctx.Err() != nil {
			return
		}
		n, err := s.step(ctx)
		if err != nil {
			s.log.Warn("social ingest", "error", err)
		}
		if n == 0 || err != nil {
			select {
			case <-ctx.Done():
				return
			case <-time.After(s.cfg.PollInterval):
			}
		}
	}
}

func (s *SocialIngester) step(ctx context.Context) (int, error) {
	cursor, err := getState(ctx, s.db, "social_cursor")
	if err != nil {
		return 0, err
	}
	u := s.cfg.NodeAPI + "/v1/social/events?limit=500"
	if cursor != "" {
		ts, id, _ := strings.Cut(cursor, ":")
		u += "&after_ts=" + url.QueryEscape(ts) + "&after_id=" + url.QueryEscape(id)
	}
	var events []nodeEvent
	if err := httpJSON(ctx, s.http, u, &events); err != nil {
		return 0, err
	}
	var last string
	for _, n := range events {
		ev, err := fromNode(n)
		if err != nil {
			s.log.Warn("event from node did not decode", "id", n.ID, "error", err)
			continue
		}
		if err := s.VerifyEvent(ev); err != nil {
			s.log.Warn("event refused", "id", n.ID, "error", err)
			continue
		}
		if err := s.Apply(ctx, ev); err != nil {
			return 0, fmt.Errorf("applying %s: %w", n.ID, err)
		}
		last = strconv.FormatUint(n.Timestamp, 10) + ":" + n.ID
	}
	if last != "" {
		if err := setState(ctx, s.db, "social_cursor", last); err != nil {
			return 0, err
		}
	}
	return len(events), nil
}

func hexs(b []byte) string { return hex.EncodeToString(b) }

func payloadJSON(msg proto.Message) string {
	b, err := protojson.MarshalOptions{UseProtoNames: true, EmitUnpopulated: true}.Marshal(msg)
	if err != nil {
		return "{}"
	}
	return string(b)
}

func mediaJSON(ms []*p2ppb.MediaReference) string {
	type m struct {
		CID        string `json:"cid"`
		Mime       string `json:"mime"`
		Size       uint64 `json:"size"`
		Kind       string `json:"kind"`
		Width      uint32 `json:"width"`
		Height     uint32 `json:"height"`
		DurationMs uint32 `json:"duration_ms"`
		Thumbnail  string `json:"thumbnail_cid"`
	}
	out := make([]m, 0, len(ms))
	for _, x := range ms {
		out = append(out, m{hexs(x.Cid), x.Mime, x.Size, x.Kind, x.Width, x.Height, x.DurationMs, hexs(x.ThumbnailCid)})
	}
	b, _ := json.Marshal(out)
	return string(b)
}

// Apply projects one verified event into the derived tables.
func (s *SocialIngester) Apply(ctx context.Context, ev *p2ppb.SocialEvent) error {
	tx, err := s.db.Begin(ctx)
	if err != nil {
		return err
	}
	defer tx.Rollback(ctx) //nolint:errcheck

	id := hexs(ev.Id)
	ts := int64(ev.Timestamp)
	var payload proto.Message
	switch ev.Type {
	case "PROFILE_UPDATE":
		payload = &p2ppb.ProfileUpdate{}
	case "FOLLOW":
		payload = &p2ppb.Follow{}
	case "UNFOLLOW":
		payload = &p2ppb.Unfollow{}
	case "POST_CREATE":
		payload = &p2ppb.PostCreate{}
	case "POST_EDIT":
		payload = &p2ppb.PostEdit{}
	case "POST_DELETE":
		payload = &p2ppb.PostDelete{}
	case "COMMENT_CREATE":
		payload = &p2ppb.CommentCreate{}
	case "REACTION":
		payload = &p2ppb.Reaction{}
	case "REPOST":
		payload = &p2ppb.Repost{}
	case "CHANNEL_CREATE":
		payload = &p2ppb.ChannelCreate{}
	case "REEL_CREATE":
		payload = &p2ppb.ReelCreate{}
	case "STORY_CREATE":
		payload = &p2ppb.StoryCreate{}
	default:
		return fmt.Errorf("unknown event type %q", ev.Type)
	}
	if err := proto.Unmarshal(ev.Payload, payload); err != nil {
		return fmt.Errorf("payload: %w", err)
	}

	tag, err := tx.Exec(ctx,
		`INSERT INTO social_events(id, type, author, device_pubkey, ts, sequence, previous_event, payload, media)
		 VALUES ($1,$2,$3,$4,$5,$6,$7,$8::jsonb,$9::jsonb) ON CONFLICT (id) DO NOTHING`,
		id, ev.Type, ev.Author, hexs(ev.DevicePubkey), ts, int64(ev.Sequence), hexs(ev.PreviousEvent),
		payloadJSON(payload), mediaJSON(ev.Media))
	if err != nil {
		return err
	}
	if tag.RowsAffected() == 0 {
		return tx.Commit(ctx) // already applied
	}

	switch p := payload.(type) {
	case *p2ppb.ProfileUpdate:
		attrs, _ := json.Marshal(p.Attributes)
		_, err = tx.Exec(ctx,
			`INSERT INTO profiles(address, display_name, bio, avatar_cid, banner_cid, website, attributes, updated_ts)
			 VALUES ($1,$2,$3,$4,$5,$6,$7::jsonb,$8)
			 ON CONFLICT (address) DO UPDATE SET display_name = EXCLUDED.display_name, bio = EXCLUDED.bio,
			   avatar_cid = EXCLUDED.avatar_cid, banner_cid = EXCLUDED.banner_cid, website = EXCLUDED.website,
			   attributes = EXCLUDED.attributes, updated_ts = EXCLUDED.updated_ts
			 WHERE profiles.updated_ts <= EXCLUDED.updated_ts`,
			ev.Author, p.DisplayName, p.Bio, hexs(p.AvatarCid), hexs(p.BannerCid), p.Website, string(attrs), ts)
	case *p2ppb.Follow:
		_, err = tx.Exec(ctx,
			`INSERT INTO follows(follower, target, ts) VALUES ($1,$2,$3) ON CONFLICT (follower, target) DO UPDATE SET ts = EXCLUDED.ts`,
			ev.Author, p.Target, ts)
	case *p2ppb.Unfollow:
		_, err = tx.Exec(ctx, `DELETE FROM follows WHERE follower = $1 AND target = $2`, ev.Author, p.Target)
	case *p2ppb.PostCreate:
		_, err = tx.Exec(ctx,
			`INSERT INTO posts(id, author, text, hashtags, mentions, channel, reply_to, sensitive, language, media, ts)
			 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10::jsonb,$11) ON CONFLICT (id) DO NOTHING`,
			id, ev.Author, p.Text, lower(p.Hashtags), nonNil(p.Mentions), hexs(p.Channel), hexs(p.ReplyTo), p.Sensitive, p.Language, mediaJSON(ev.Media), ts)
	case *p2ppb.PostEdit:
		// Only the author may edit their own post.
		_, err = tx.Exec(ctx,
			`UPDATE posts SET text = $1, hashtags = $2, edited_ts = $3 WHERE id = $4 AND author = $5`,
			p.Text, lower(p.Hashtags), ts, hexs(p.Post), ev.Author)
	case *p2ppb.PostDelete:
		_, err = tx.Exec(ctx, `UPDATE posts SET deleted = true, text = '' WHERE id = $1 AND author = $2`, hexs(p.Post), ev.Author)
		if err == nil {
			_, err = tx.Exec(ctx, `UPDATE reels SET deleted = true WHERE id = $1 AND author = $2`, hexs(p.Post), ev.Author)
		}
	case *p2ppb.CommentCreate:
		_, err = tx.Exec(ctx,
			`INSERT INTO comments(id, post, author, text, parent, ts) VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT (id) DO NOTHING`,
			id, hexs(p.Post), ev.Author, p.Text, hexs(p.ParentComment), ts)
	case *p2ppb.Reaction:
		if p.Reaction == "" {
			_, err = tx.Exec(ctx, `DELETE FROM reactions WHERE author = $1 AND target = $2`, ev.Author, hexs(p.Target))
		} else {
			_, err = tx.Exec(ctx,
				`INSERT INTO reactions(author, target, reaction, ts) VALUES ($1,$2,$3,$4)
				 ON CONFLICT (author, target) DO UPDATE SET reaction = EXCLUDED.reaction, ts = EXCLUDED.ts`,
				ev.Author, hexs(p.Target), p.Reaction, ts)
		}
	case *p2ppb.Repost:
		_, err = tx.Exec(ctx,
			`INSERT INTO reposts(id, post, author, comment, ts) VALUES ($1,$2,$3,$4,$5) ON CONFLICT (id) DO NOTHING`,
			id, hexs(p.Post), ev.Author, p.Comment, ts)
	case *p2ppb.ChannelCreate:
		_, err = tx.Exec(ctx,
			`INSERT INTO channels(id, creator, name, description, avatar_cid, open_posting, ts) VALUES ($1,$2,$3,$4,$5,$6,$7) ON CONFLICT (id) DO NOTHING`,
			id, ev.Author, p.Name, p.Description, hexs(p.AvatarCid), p.OpenPosting, ts)
	case *p2ppb.ReelCreate:
		if int(p.VideoIndex) >= len(ev.Media) {
			return fmt.Errorf("reel video index out of range")
		}
		v := ev.Media[p.VideoIndex]
		_, err = tx.Exec(ctx,
			`INSERT INTO reels(id, author, caption, hashtags, video_cid, video_mime, duration_ms, thumbnail_cid, sensitive, min_age, allow_comments, ts)
			 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12) ON CONFLICT (id) DO NOTHING`,
			id, ev.Author, p.Caption, lower(p.Hashtags), hexs(v.Cid), v.Mime, int(v.DurationMs), hexs(v.ThumbnailCid), p.Sensitive, int(p.MinAge), p.AllowComments, ts)
	case *p2ppb.StoryCreate:
		_, err = tx.Exec(ctx,
			`INSERT INTO stories(id, author, caption, media, sensitive, ts, expires_at) VALUES ($1,$2,$3,$4::jsonb,$5,$6,$7) ON CONFLICT (id) DO NOTHING`,
			id, ev.Author, p.Caption, mediaJSON(ev.Media), p.Sensitive, ts, int64(p.ExpiresAt))
	}
	if err != nil {
		return err
	}
	return tx.Commit(ctx)
}

func lower(xs []string) []string {
	out := make([]string, 0, len(xs))
	for _, x := range xs {
		out = append(out, strings.ToLower(strings.TrimPrefix(x, "#")))
	}
	return out
}

// nonNil turns a nil slice into an empty one, because pgx renders nil as
// SQL NULL and the columns are NOT NULL.
func nonNil(xs []string) []string {
	if xs == nil {
		return []string{}
	}
	return xs
}

// attestationView is the node API's rendering.
type attestationView struct {
	Subject        string `json:"subject"`
	Verdict        string `json:"verdict"`
	Policy         string `json:"policy"`
	ReasonCode     string `json:"reason_code"`
	Timestamp      uint64 `json:"timestamp"`
	AttestorPubkey string `json:"attestor_pubkey"`
	Trusted        bool   `json:"trusted"`
	Kind           string `json:"kind"`
}

// SyncAttestations pulls recent attestations from the node.
func (s *SocialIngester) SyncAttestations(ctx context.Context) error {
	var list []attestationView
	if err := httpJSON(ctx, s.http, s.cfg.NodeAPI+"/v1/safety/recent?limit=5000", &list); err != nil {
		return err
	}
	trusted := map[string]bool{}
	for _, t := range s.cfg.TrustedAttestors {
		trusted[strings.ToLower(t)] = true
	}
	batch := &pgx.Batch{}
	for _, a := range list {
		batch.Queue(
			`INSERT INTO attestations(subject, kind, attestor, verdict, policy, reason_code, ts, trusted)
			 VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
			 ON CONFLICT (subject, attestor) DO UPDATE SET verdict = EXCLUDED.verdict, policy = EXCLUDED.policy,
			   reason_code = EXCLUDED.reason_code, ts = EXCLUDED.ts, trusted = EXCLUDED.trusted
			 WHERE attestations.ts <= EXCLUDED.ts`,
			a.Subject, a.Kind, a.AttestorPubkey, a.Verdict, a.Policy, a.ReasonCode, int64(a.Timestamp),
			trusted[strings.ToLower(a.AttestorPubkey)])
	}
	if batch.Len() == 0 {
		return nil
	}
	return s.db.SendBatch(ctx, batch).Close()
}
