package indexer

import (
	"context"
	"fmt"

	"github.com/jackc/pgx/v5/pgxpool"
)

// schema is applied idempotently at startup. Every table is derived; the
// only durable state is the cursors in index_state, and even those can be
// discarded at the cost of re-reading.
const schema = `
CREATE TABLE IF NOT EXISTS index_state (
  key   text PRIMARY KEY,
  value text NOT NULL
);

-- Chain ---------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS blocks (
  height    bigint PRIMARY KEY,
  time      timestamptz NOT NULL,
  proposer  text NOT NULL DEFAULT '',
  tx_count  int NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS transactions (
  hash       text PRIMARY KEY,
  height     bigint NOT NULL REFERENCES blocks(height) ON DELETE CASCADE,
  index      int NOT NULL,
  code       int NOT NULL,
  gas_used   bigint NOT NULL DEFAULT 0,
  fee_uhash  numeric NOT NULL DEFAULT 0,
  memo       text NOT NULL DEFAULT '',
  msg_types  text[] NOT NULL DEFAULT '{}',
  signers    text[] NOT NULL DEFAULT '{}'
);
CREATE INDEX IF NOT EXISTS transactions_height ON transactions(height DESC);
CREATE INDEX IF NOT EXISTS transactions_signers ON transactions USING gin(signers);

CREATE TABLE IF NOT EXISTS transfers (
  id         bigserial PRIMARY KEY,
  height     bigint NOT NULL,
  txhash     text NOT NULL,
  sender     text NOT NULL,
  recipient  text NOT NULL,
  amount_uhash numeric NOT NULL
);
CREATE INDEX IF NOT EXISTS transfers_sender ON transfers(sender, height DESC);
CREATE INDEX IF NOT EXISTS transfers_recipient ON transfers(recipient, height DESC);

CREATE TABLE IF NOT EXISTS usernames (
  name        text PRIMARY KEY,
  owner       text NOT NULL,
  synced_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS usernames_owner ON usernames(owner);

CREATE TABLE IF NOT EXISTS identities (
  address         text PRIMARY KEY,
  root_pubkey     text NOT NULL,
  rotation_count  int NOT NULL DEFAULT 0,
  devices         jsonb NOT NULL DEFAULT '[]',
  synced_at       timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS providers (
  operator          text PRIMARY KEY,
  reward_address    text NOT NULL,
  roles             text[] NOT NULL DEFAULT '{}',
  bond_uhash        numeric NOT NULL DEFAULT 0,
  declared_storage  bigint NOT NULL DEFAULT 0,
  jailed            boolean NOT NULL DEFAULT false,
  fraud_score       int NOT NULL DEFAULT 0,
  moniker           text NOT NULL DEFAULT '',
  synced_at         timestamptz NOT NULL DEFAULT now()
);

-- Social ---------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS social_events (
  id             text PRIMARY KEY,
  type           text NOT NULL,
  author         text NOT NULL,
  device_pubkey  text NOT NULL,
  ts             bigint NOT NULL,
  sequence       bigint NOT NULL,
  previous_event text NOT NULL DEFAULT '',
  payload        jsonb NOT NULL DEFAULT '{}',
  media          jsonb NOT NULL DEFAULT '[]',
  received_at    timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS social_events_author ON social_events(author, sequence);
CREATE INDEX IF NOT EXISTS social_events_ts ON social_events(ts DESC);
CREATE INDEX IF NOT EXISTS social_events_type_ts ON social_events(type, ts DESC);

CREATE TABLE IF NOT EXISTS profiles (
  address      text PRIMARY KEY,
  display_name text NOT NULL DEFAULT '',
  bio          text NOT NULL DEFAULT '',
  avatar_cid   text NOT NULL DEFAULT '',
  banner_cid   text NOT NULL DEFAULT '',
  website      text NOT NULL DEFAULT '',
  attributes   jsonb NOT NULL DEFAULT '{}',
  updated_ts   bigint NOT NULL
);

CREATE TABLE IF NOT EXISTS follows (
  follower  text NOT NULL,
  target    text NOT NULL,
  ts        bigint NOT NULL,
  PRIMARY KEY (follower, target)
);
CREATE INDEX IF NOT EXISTS follows_target ON follows(target);

CREATE TABLE IF NOT EXISTS posts (
  id         text PRIMARY KEY,
  author     text NOT NULL,
  text       text NOT NULL,
  hashtags   text[] NOT NULL DEFAULT '{}',
  mentions   text[] NOT NULL DEFAULT '{}',
  channel    text NOT NULL DEFAULT '',
  reply_to   text NOT NULL DEFAULT '',
  sensitive  boolean NOT NULL DEFAULT false,
  language   text NOT NULL DEFAULT '',
  media      jsonb NOT NULL DEFAULT '[]',
  ts         bigint NOT NULL,
  edited_ts  bigint,
  deleted    boolean NOT NULL DEFAULT false
);
CREATE INDEX IF NOT EXISTS posts_author_ts ON posts(author, ts DESC);
CREATE INDEX IF NOT EXISTS posts_ts ON posts(ts DESC);
CREATE INDEX IF NOT EXISTS posts_channel_ts ON posts(channel, ts DESC);
CREATE INDEX IF NOT EXISTS posts_hashtags ON posts USING gin(hashtags);

CREATE TABLE IF NOT EXISTS comments (
  id       text PRIMARY KEY,
  post     text NOT NULL,
  author   text NOT NULL,
  text     text NOT NULL,
  parent   text NOT NULL DEFAULT '',
  ts       bigint NOT NULL
);
CREATE INDEX IF NOT EXISTS comments_post ON comments(post, ts);

CREATE TABLE IF NOT EXISTS reactions (
  author    text NOT NULL,
  target    text NOT NULL,
  reaction  text NOT NULL,
  ts        bigint NOT NULL,
  PRIMARY KEY (author, target)
);
CREATE INDEX IF NOT EXISTS reactions_target ON reactions(target);

CREATE TABLE IF NOT EXISTS reposts (
  id      text PRIMARY KEY,
  post    text NOT NULL,
  author  text NOT NULL,
  comment text NOT NULL DEFAULT '',
  ts      bigint NOT NULL
);
CREATE INDEX IF NOT EXISTS reposts_post ON reposts(post);

CREATE TABLE IF NOT EXISTS channels (
  id           text PRIMARY KEY,
  creator      text NOT NULL,
  name         text NOT NULL,
  description  text NOT NULL DEFAULT '',
  avatar_cid   text NOT NULL DEFAULT '',
  open_posting boolean NOT NULL DEFAULT false,
  ts           bigint NOT NULL
);

CREATE TABLE IF NOT EXISTS reels (
  id           text PRIMARY KEY,
  author       text NOT NULL,
  caption      text NOT NULL DEFAULT '',
  hashtags     text[] NOT NULL DEFAULT '{}',
  video_cid    text NOT NULL,
  video_mime   text NOT NULL DEFAULT '',
  duration_ms  int NOT NULL DEFAULT 0,
  thumbnail_cid text NOT NULL DEFAULT '',
  sensitive    boolean NOT NULL DEFAULT false,
  min_age      int NOT NULL DEFAULT 0,
  allow_comments boolean NOT NULL DEFAULT true,
  ts           bigint NOT NULL,
  deleted      boolean NOT NULL DEFAULT false
);
CREATE INDEX IF NOT EXISTS reels_ts ON reels(ts DESC);
CREATE INDEX IF NOT EXISTS reels_author ON reels(author, ts DESC);
CREATE INDEX IF NOT EXISTS reels_hashtags ON reels USING gin(hashtags);

CREATE TABLE IF NOT EXISTS stories (
  id         text PRIMARY KEY,
  author     text NOT NULL,
  caption    text NOT NULL DEFAULT '',
  media      jsonb NOT NULL DEFAULT '[]',
  sensitive  boolean NOT NULL DEFAULT false,
  ts         bigint NOT NULL,
  expires_at bigint NOT NULL
);
CREATE INDEX IF NOT EXISTS stories_author ON stories(author, expires_at);

-- Safety ----------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS attestations (
  subject      text NOT NULL,
  kind         text NOT NULL,
  attestor     text NOT NULL,
  verdict      text NOT NULL,
  policy       text NOT NULL DEFAULT '',
  reason_code  text NOT NULL DEFAULT '',
  ts           bigint NOT NULL,
  trusted      boolean NOT NULL DEFAULT false,
  PRIMARY KEY (subject, attestor)
);
CREATE INDEX IF NOT EXISTS attestations_subject ON attestations(subject);
`

// derivedTables are truncated by a rebuild. index_state is reset separately.
var derivedTables = []string{
	"transfers", "transactions", "blocks", "usernames", "identities", "providers",
	"social_events", "profiles", "follows", "posts", "comments", "reactions",
	"reposts", "channels", "reels", "stories", "attestations",
}

// Migrate applies the schema.
func Migrate(ctx context.Context, db *pgxpool.Pool) error {
	if _, err := db.Exec(ctx, schema); err != nil {
		return fmt.Errorf("applying schema: %w", err)
	}
	return nil
}

// Reset clears every derived table and cursor. The next run refills them.
func Reset(ctx context.Context, db *pgxpool.Pool) error {
	tx, err := db.Begin(ctx)
	if err != nil {
		return err
	}
	defer tx.Rollback(ctx) //nolint:errcheck // rollback after commit is a no-op
	for _, t := range derivedTables {
		if _, err := tx.Exec(ctx, "TRUNCATE TABLE "+t+" CASCADE"); err != nil {
			return fmt.Errorf("truncating %s: %w", t, err)
		}
	}
	if _, err := tx.Exec(ctx, "DELETE FROM index_state"); err != nil {
		return err
	}
	return tx.Commit(ctx)
}

// getState reads a cursor.
func getState(ctx context.Context, db *pgxpool.Pool, key string) (string, error) {
	var v string
	err := db.QueryRow(ctx, "SELECT value FROM index_state WHERE key = $1", key).Scan(&v)
	if err != nil {
		if err.Error() == "no rows in result set" {
			return "", nil
		}
		return "", err
	}
	return v, nil
}

// setState writes a cursor.
func setState(ctx context.Context, db *pgxpool.Pool, key, value string) error {
	_, err := db.Exec(ctx,
		"INSERT INTO index_state(key, value) VALUES ($1, $2) ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
		key, value)
	return err
}
