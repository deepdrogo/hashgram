# Hashgram Indexer

`hashgram-indexer` derives PostgreSQL query tables from canonical Hashgram
data and serves a loopback read API on `127.0.0.1:1318`. It is a **read
model, not a source of truth**: the chain is canonical for balances, names,
identities, validators and providers; signed, replicated social events are
canonical for the public social graph. If the database is lost,
`hashgram-indexer rebuild` followed by `run` recreates it. If the index ever
disagrees with the chain, the index is wrong.

The indexer never holds a key and never writes to the network. It reads the
co-located chain node over its loopback CometBFT RPC (`26657`) and REST
gateway (`1317`), the co-located `hashgram-node` over its loopback API, and
serves its own loopback API. Exposing that API publicly is a reverse-proxy
decision made elsewhere (see `docs/OPERATIONS.md`).

Source: `indexer/` (library) and `cmd/hashgram-indexer/` (binary).

---

## 1. Commands and configuration

```
hashgram-indexer run     --config /etc/hashgram/indexer.toml
hashgram-indexer rebuild --config /etc/hashgram/indexer.toml
hashgram-indexer check   --config /etc/hashgram/indexer.toml
```

`indexer.toml`:

| Key | Default | Meaning |
| --- | --- | --- |
| `database_url` | required | PostgreSQL connection string. The role must own only the index database. |
| `chain_rpc` | `http://127.0.0.1:26657` | CometBFT RPC of the co-located chain node. |
| `chain_api` | `http://127.0.0.1:1317` | REST gateway of the co-located chain node. |
| `node_api` | `http://127.0.0.1:26672` | Loopback API of the co-located `hashgram-node`. |
| `listen` | `127.0.0.1:1318` | Read API bind address. |
| `poll_interval` | `3s` | How often sources are polled (minimum `500ms`). |
| `start_height` | `0` | First block to index; `0` means genesis. See §3.1 for the effect on balances. |
| `trusted_attestors` | `[]` | Hex ed25519 keys whose safety verdicts the API enforces. |
| `genesis_path` | `""` | Local `genesis.json` used to seed balances. Empty means fetch `GET /genesis` from `chain_rpc`, falling back to `/genesis_chunked`. |
| `validator_refresh_blocks` | `50` | Indexed blocks between refreshes of the `validators` projection. |

The network pin (`network.json` next to the config) must match the chain-id
the node reports; the indexer refuses to index the wrong network.

---

## 2. Tables

The schema in `indexer/schema.go` is applied idempotently at start
(`CREATE TABLE IF NOT EXISTS`, `ADD COLUMN IF NOT EXISTS`). Migration 2 is
the additive block at the end of the schema; nothing is ever dropped or
rewritten by a migration.

### Chain

| Table | Content | Source |
| --- | --- | --- |
| `blocks` | height, time, proposer, tx_count | REST `/cosmos/tx/v1beta1/txs/block/{h}` |
| `transactions` | hash, height, code, gas, fee, memo, msg_types, signers | same + RPC `/block_results`, `/block` |
| `transfers` | uhash `transfer` events of successful txs | RPC `/block_results` |
| `usernames` | name → owner (the public on-chain registration) | REST `/hashgram/username/v1/registrations` |
| `identities` | address, root pubkey, rotation count, devices | REST `/hashgram/identity/v1/...` |
| `providers` | operator, reward address, roles, bond, declared storage, jailed, fraud score, moniker, **`total_paid`** | REST `/hashgram/serviceproof/v1/providers`, `/rewards/{operator}` (`lifetime_paid`) |
| **`balances`** | `address`, `amount numeric(40,0)`, `updated_height` | genesis `app_state.bank.balances` + `coin_spent` / `coin_received` events (§3) |
| **`validators`** | `operator`, `moniker`, `tokens`, `commission_rate`, `status`, `jailed`, `updated_height` | REST `/cosmos/staking/v1beta1/validators` every `validator_refresh_blocks` |
| **`stats`** | `key`, `value`, `updated_height` — currently `hashgram_supply = sum(balances)` | recomputed by the ingester |
| `index_state` | cursors: `chain_height`, `chain_latest`, `balances_height`, `balances_seeded`, `balances_partial`, `validators_height`, `registry_cursor:<path>`, social cursors | — |

### Social (public, signed events only)

`social_events`, `profiles`, `follows`, `posts`, `comments`, `reactions`,
`reposts`, `channels`, `reels`, `stories`, `attestations`. See
`docs/SOCIAL_PROTOCOL.md`; unchanged by this document.

---

## 3. Projections

### 3.1 Balances

`balances` is x/bank folded forward:

1. **Seed.** On first run (state `balances_seeded` empty) the genesis
   document is read (`genesis_path`, else RPC `/genesis`, else
   `/genesis_chunked` reassembled) and every `app_state.bank.balances`
   entry's `uhash` coins are inserted at `updated_height = 0`.
2. **Fold.** For every block, every `coin_spent` (`spender`, `amount`) and
   `coin_received` (`receiver`, `amount`) event is applied for denom `uhash`.
   Amounts are Cosmos coin strings (`"12345uhash"`, multi-coin
   `"7ibc/X,5000uhash"`); other denominations are ignored. Events are taken
   from `finalize_block_events` (CometBFT 0.38; `begin_block_events` /
   `end_block_events` are also decoded) **and** from every transaction's
   events, including failed transactions, because the SDK charges their fee
   and reports the ante-handler events on the failed result.
3. **Atomicity.** In the steady state the deltas are written in the same
   transaction as the block's rows and the `chain_height` cursor, so a crash
   can neither skip nor double-apply a block. When the projection lags (first
   run of this binary against an older index, or after a crash) a catch-up
   loop replays `/block_results` from `balances_height + 1`, 200 blocks per
   poll, until it meets `chain_height`.
4. **Sanity.** After each poll `stats.hashgram_supply = sum(balances)`.
   Compare it with `GET /cosmos/bank/v1beta1/supply/by_denom?denom=uhash`;
   they must be equal. A difference means the projection drifted; rebuild.

`start_height > 1` does not by itself break balances: the catch-up replays
block results from height 1 as long as the node still has them. If the node
is pruned or state-synced and cannot serve early `/block_results`, the loop
skips to the first indexed block, sets `balances_partial = 1` and logs a
warning; `/v1/network/stats` then reports `"balances_partial": true` and the
holders figures are lower bounds only.

### 3.2 Validators

Every `validator_refresh_blocks` indexed blocks the full staking validator
list is read (all pages, from the first page) and upserted with
`updated_height` = the indexed height; rows not seen in that pass are
deleted, so the table is the current set, not a history. `status` is
normalised (`BOND_STATUS_BONDED` → `bonded`).

### 3.3 Providers and earnings

The registry sync (every 60 s) upserts providers and then reads
`/hashgram/serviceproof/v1/rewards/{operator}`; the `uhash` part of
`lifetime_paid` becomes `providers.total_paid`. If the rewards query fails
the previous figure is kept.

### 3.4 Registry pagination (audit G7)

Registry listings walk at most 1000 pages × 200 rows per sync call. The
`pagination.next_key` is checkpointed in `index_state` under
`registry_cursor:<path>`; when the cap is hit the sync returns
`errPaginationIncomplete` (logged as a warning) and the **next** call resumes
from the checkpoint instead of silently stopping. A checkpoint the chain no
longer accepts is discarded and the listing restarts from page one. The
validators refresh always clears its checkpoint first, because it prunes
rows not seen in the pass.

---

## 4. Read API

All endpoints are `GET`, return JSON, and inherit a 15 s request timeout.
Every list has a default and a hard maximum `limit`.

Two pagination styles exist:

* **Time cursor** (feeds, transfers): `?before=<timestamp>&limit=`; the
  response is a bare array.
* **Rank cursor** (leaderboards, validators): `?cursor=<rank>&limit=`
  returns rows with `rank > cursor` as
  `{"items": [...], "next_cursor": "<rank>"}`; `next_cursor` is omitted on
  the last page. `limit` defaults to 50 and is capped at 200.

### 4.1 Pre-existing endpoints

| Path | Parameters | Notes |
| --- | --- | --- |
| `/v1/health` | — | `{"status":"ok","chain_height":"…"}` |
| `/v1/stats` | — | Row counts per table, `chain_height`, `hashgram_supply` |
| `/v1/feed/chronological` | `before`, `limit`(50/200), `safe` | |
| `/v1/feed/following/{address}` | same | |
| `/v1/feed/author/{address}` | same | |
| `/v1/feed/hashtag/{tag}` | same | |
| `/v1/feed/channel/{id}` | same | |
| `/v1/reels` | `before`, `limit`(30/100), `max_age`, `tag`, `safe` | |
| `/v1/reels/author/{address}` | `before`, `limit`(30/100), `safe` | |
| `/v1/posts/{id}` | — | post or reel with comments/reactions |
| `/v1/profiles/{address}` | — | includes `username` via the on-chain owner relation |
| `/v1/profiles/{address}/followers` | `limit`(100/1000) | |
| `/v1/profiles/{address}/following` | `limit`(100/1000) | |
| `/v1/stories/{address}` | `safe` | |
| `/v1/channels` | `limit`(100/500) | |
| `/v1/search/users` | `q`, `limit`(20/100) | prefix search on usernames; `%`, `_`, `\` in `q` are escaped (G7) |
| `/v1/search/hashtags` | `q`, `limit`(20/100) | same escaping |
| `/v1/accounts/{address}/transfers` | `limit`(50/500) | |
| `/v1/accounts/{address}/transactions` | `limit`(50/500) | |
| `/v1/providers` | `limit`(200/1000) | |
| `/v1/blocks/latest` | `limit`(20/200) | |
| `/v1/txs/{hash}` | — | |

### 4.2 `GET /v1/leaderboards/holders`

Parameters: `limit` (default 50, max 200), `cursor` (rank).

Accounts with `amount > 0` ordered by amount desc, address asc. `kind` is
`"module"` for a known module account (with `label` = module name) and
`"account"` otherwise. `username` and `verified: true` appear **only** when
the `usernames` table has a registration whose `owner` equals the address —
the public on-chain relation. No other name source is consulted.

```json
{
  "items": [
    {"rank": 1, "address": "hash1znsxwrqcg7svmw5zzmeswqpc0v5ddjdsf8djn4", "amount": "500000000000000", "kind": "module", "label": "serviceproof", "verified": false},
    {"rank": 2, "address": "hash13t8v5nnghrvgcuuqcrt9k5wyhtqwq7fl3ynjpy", "amount": "199000000000000", "kind": "account", "verified": false},
    {"rank": 3, "address": "hash1…", "amount": "1000000000", "kind": "account", "username": "alice", "verified": true}
  ],
  "next_cursor": "50"
}
```

Known module accounts (derived as `bech32("hash", sha256(name)[:20])`,
identical to `authtypes.NewModuleAddress`; pinned by
`TestModuleAccountsMatchGenesis`):

| Name | Address |
| --- | --- |
| `fee_collector` | `hash17xpfvakm2amg962yls6f84z3kell8c5lmr3pm6` |
| `distribution` | `hash1jv65s3grqf6v6jl3dp4t6c9t9rk99cd8v9kaec` |
| `bonded_tokens_pool` | `hash1fl48vsnmsdzcv85q5d2q4z5ajdha8yu37pmwfs` |
| `not_bonded_tokens_pool` | `hash1tygms3xhhs3yv487phx3dw4a95jn7t7l2p8lly` |
| `gov` | `hash10d07y265gmmuvt4z0w9aw880jnsr700j3cpyx5` |
| `founder` | `hash1t9zc2z9qsf707huqa5y3a0vgpra7vlyhyvdeeh` |
| `feerouter` | `hash1gpqrlrk3nkatprdehjv6s5q5dj6yt029glcpwh` |
| `welcome` | `hash19qx5f2c7naumtn8zm4843a07j8c0htx6xzkrt3` |
| `serviceproof` (reward reserve) | `hash1znsxwrqcg7svmw5zzmeswqpc0v5ddjdsf8djn4` |
| `serviceproof_bond` (bond pool) | `hash12ta0j4xxxgv2p9c3zsua9cy7nc9439usnp77e6` |
| `treasury_treasury` | `hash1j8nrdutkgj2cdctuj7zlcf0n754juc0jfluut2` |
| `treasury_dev_grants` | `hash1p3sevw2enxjankvzuhmt7ysqy7re09txvvyefr` |
| `treasury_liquidity` | `hash1vup3q25ce68v7nm2se970lcmdwkar46cq555el` |
| `treasury_growth` | `hash1vualfelayplprjlpx60vn69lgkgy8ft72qvapt` |

A module account added to `app.maccPerms` must be added to
`indexer/accounts.go` as well, or it ranks as a plain account.

### 4.3 `GET /v1/leaderboards/validators`

Parameters: `limit`, `cursor`. Validators by `tokens` desc.

```json
{"items": [{"rank": 1, "operator": "hashvaloper1…", "moniker": "genesis-full", "tokens": "60900000000000", "commission_rate": "0.100000000000000000", "status": "bonded", "jailed": false, "updated_height": 47450}]}
```

### 4.4 `GET /v1/leaderboards/providers` and `GET /v1/leaderboards/earners`

Parameters: `limit`, `cursor`, `role` (`storage`, `relay`, `media`, … or
the `SERVICE_ROLE_*` form). Providers by `total_paid` desc, then
`bond_uhash` desc, then operator. `earners` is the same data under the
product name.

```json
{"items": [{"rank": 1, "operator": "hash1…", "moniker": "", "roles": ["storage","relay","media"], "total_paid": "0", "bond_uhash": "1000000000", "declared_storage_bytes": 200000000000, "jailed": false, "fraud_score": 0}]}
```

### 4.5 `GET /v1/validators` and `GET /v1/validators/{operator}`

`/v1/validators` is the ranked list with the same shape and pagination as
§4.3. `/v1/validators/{operator}` returns one row (with its rank) or
`404 {"error":"not found"}`.

### 4.6 `GET /v1/network/stats`

One document. Each field is derived from the projection; a failing query
leaves its field at the zero value and logs at debug level rather than
failing the response.

```json
{
  "height": 47452,
  "chain_latest_height": 47453,
  "indexer_lag": 1,
  "block_time_seconds": 5.02,
  "latest_block_time": "2026-09-12T17:48:10Z",
  "tx_count_24h": 12,
  "accounts_with_balance": 9,
  "total_supply": "1000000000000000",
  "balances_height": 47452,
  "balances_partial": false,
  "bonded_tokens": "60900000000000",
  "validators": {"total": 1, "bonded": 1, "jailed": 0},
  "providers": {"total": 1, "by_role": {"media": 1, "relay": 1, "storage": 1}},
  "usernames": 3,
  "identities": 2,
  "social": {"posts": 0, "profiles": 0}
}
```

* `block_time_seconds` is the mean interval over the last 100 intervals
  (101 most recent indexed blocks).
* `tx_count_24h` sums `blocks.tx_count` for blocks timestamped in the last
  24 h.
* `total_supply` is `sum(balances)` (all accounts, including module
  accounts) computed live; `/v1/stats.hashgram_supply` is the same figure as
  of the last poll.
* `bonded_tokens` is `sum(validators.tokens)` where `status = 'bonded'`.

---

## 5. Privacy rules

These follow `docs/HASHGRAM_ONE_ARCHITECTURE.md` §2 and §11.

* **Never index private content.** Only chain state and signed public
  social events are read. Envelopes, Drive objects, MLS state, Circle or
  Space posts and anything else that is encrypted or scoped to a group never
  reach the indexer, and there is no code path that could ingest them.
* **Never map an address to a name except through the public on-chain
  username registration.** `usernames.owner` is the only join used by
  `/v1/profiles/{address}`, `/v1/search/users` and
  `/v1/leaderboards/holders`. Transfers, social graph, timing or profile
  content are never used to attach a name to an address.
* **Module accounts are labelled from a compile-time list**, not inferred.
* **Balances are public chain state.** The projection reproduces what any
  bank query already reveals; it does not add information.
* The read API is loopback by default; it enforces trusted safety verdicts
  (`attestations`) on every content query.

---

## 6. Rebuild procedure

The index is a cache. To rebuild from canonical data:

```
systemctl stop hashgram-indexer            # or however the unit is managed
hashgram-indexer rebuild --config /etc/hashgram/indexer.toml
systemctl start hashgram-indexer
```

`rebuild` truncates every derived table (including `balances`, `validators`,
`stats`) and deletes every cursor in `index_state`. On the next `run` the
ingester seeds balances from genesis, indexes blocks from `start_height`,
replays balance events from height 1, refreshes validators on the first
step and the registries within a minute. Watch `/v1/network/stats`:
`indexer_lag` falls to ~0–1 and `balances_height` reaches `height` when the
rebuild is complete; then verify `total_supply` against the bank supply.

Upgrading an existing index to a binary with Migration 2 needs no rebuild:
the new tables are created at start, balances are seeded and caught up in
the background (200 blocks per poll), and `balances_height` in
`/v1/network/stats` shows the progress.

---

## 7. Lag metric

`indexer_lag = chain_latest_height − height` in `/v1/network/stats`.

* `chain_latest_height` is what the node's `/status` reported on the last
  poll (state `chain_latest`, written every `poll_interval`).
* `height` is the last fully indexed block (state `chain_height`, written in
  the same transaction as the block's rows).

Steady state is 0–1 blocks. A growing lag means the node is producing blocks
faster than the indexer ingests them (200 per poll), the database is slow,
or the node's RPC/REST is failing (see the `chain ingest` warnings in the
log). `balances_height` lags `height` only during a catch-up; if it stays
behind while `height` advances, read the `balances` warnings in the log.
`validators.updated_height` in any validator row shows when the set was
last refreshed.

---

## 8. Verification

```
nice -n 15 go build ./indexer/... ./cmd/hashgram-indexer/...
nice -n 15 go vet ./indexer/...
nice -n 15 go test ./indexer/...
gofmt -l indexer cmd/hashgram-indexer     # must print nothing
```

Unit tests need no PostgreSQL: they cover the coin parser, balance event
folding (begin/end/finalize-block and tx events, failed-tx fees, burns),
genesis parsing against the embedded mainnet genesis (sum = bank supply),
bech32 and module-account derivation against genesis, `escapeLike`, cursor
and limit handling, validator REST mapping, refresh scheduling, config
defaults and route registration. Handlers that query the database are
exercised operationally against the loopback API.
