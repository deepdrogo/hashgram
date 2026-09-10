# Prompt 1 of 2 — the public read API behind hashgram.io

Copy everything below the line into an AI coding agent opened on this
repository (`/home/hashgram`) on the genesis VPS, or on any machine with a
synced node and PostgreSQL. It extends the existing Go indexer; it does not
start a new project. Prompt 2 (`docs/PROMPT_HASHGRAM_IO.md`) builds the
website on top of what this produces.

---

You are extending **Hashgram**, a live Cosmos SDK v0.53 / CometBFT blockchain
(chain-id `hashgram-1`, launched 2026-09-10, genesis SHA-256
`e322bc2319f6e0173286fa526dab5a8ff8ad0797c7b80dd03e7c9d98621d5e4d`). Your
task is the **public, read-only HTTP API** that the explorer and documentation
website `hashgram.io` will consume. Everything the website shows must come
from the node, through this API, in near real time.

Work in this repository. Read before writing: `docs/CLIENT_CONNECTIVITY_SPEC.md`
(all endpoints and the traps), `docs/TOKENOMICS.md`, `docs/SERVICE_REWARDS.md`,
`docs/OPERATIONS.md`, `indexer/` (the existing indexer: `api.go`, `chain.go`,
`schema.go`, `config.go`), `cmd/hashgram-indexer/main.go`,
`deploy/systemd/hashgram-indexer.service`, `indexer.toml` in `/etc/hashgram`.

## The rule that shapes everything

The node's admin interfaces — CometBFT RPC `127.0.0.1:26657`, Cosmos REST
`127.0.0.1:1317`, gRPC `127.0.0.1:9091`, hashgram-node API `127.0.0.1:26672`,
indexer `127.0.0.1:1318` — are **loopback only** and must stay that way.
`hashgramctl mainnet-preflight` fails if 26657 or 1317 is reachable from
outside; do not change any `laddr`, `address` or `listen` in
the chain home's `config` directory (the `$HASHGRAM_HOME` used by hashgramd) or in `/etc/hashgram`. The
public API is the indexer's HTTP server, published through a TLS reverse
proxy. Nothing else is exposed. The browser never talks to 26657 or 1317.

Second rule: the database is a **rebuildable cache**. Every number the API
serves must be derivable from the chain (`hashgram-indexer rebuild` must
reproduce it). Never store anything that is not on chain or in the node API.

## What exists already

`indexer/` ingests blocks by polling `chain_rpc` every 3 s (`ChainIngester.IndexBlock`),
stores `blocks`, `transactions`, `transfers`, `usernames`, `identities`,
`providers`, plus the social projections, and serves on `127.0.0.1:1318`:

```
GET /v1/health                              GET /v1/stats
GET /v1/blocks/latest                       GET /v1/txs/{hash}
GET /v1/accounts/{address}/transactions     GET /v1/accounts/{address}/transfers
GET /v1/providers                           GET /v1/profiles/{address} ...
GET /v1/feed/... /v1/reels ... /v1/search/...  (social; leave as is)
```

`systemctl status hashgram-indexer` is active on this host; PostgreSQL is
local (`database_url` in `indexer.toml` in `/etc/hashgram`).

## Deliverable A — explorer endpoints (Go, `indexer/`)

Add, all JSON, all paginated where lists (`?limit=` max 100, `?cursor=` opaque),
all with `Cache-Control` appropriate to their volatility:

**Chain**
- `GET /v1/chain` — chain_id, network_id, genesis_hash (from
  the `network.json` pin in `/etc/hashgram`), genesis_time, latest height, latest block time,
  average block time over last 100 blocks, node version (`/status` →
  `node_info.version`), app version, validator count, bonded tokens, total
  supply (`/cosmos/bank/v1beta1/supply`), circulating (= supply minus the
  module accounts `serviceproof`, `treasury` sub-accounts, `founder`, and the
  Founder's unvested balance), transactions total, accounts total.
- `GET /v1/blocks?limit&cursor` — height, time, proposer (map consensus address
  → validator moniker via `/cosmos/staking/v1beta1/validators` and
  `/validators` on RPC), tx_count, size bytes, gas used/wanted.
- `GET /v1/blocks/{height}` — full header, txs (hash, type summary, result),
  `block_results` events summarised (transfers, delegations, founder payouts).
- `GET /v1/txs?limit&cursor&type=` — hash, height, time, first message type,
  signer(s), fee, success, gas.
- `GET /v1/txs/{hash}` — extend the existing one: decoded messages (use the
  SDK codec from `app/`, not string parsing), events, fee split (see fee
  routing below), raw log on failure.
- `GET /v1/search?q=` — resolves: height (digits), tx hash (64 hex), block
  hash, `hash1…` address, `hashvaloper1…`, `@username` (via
  `/hashgram/username/v1/lookup/{name}`), peer id (`12D3Koo…`). Returns
  `{type, id}`; the website navigates.

**Accounts**
- New table `balances(address, uhash, spendable_uhash, kind, updated_height)`
  refreshed every 20 blocks from `/cosmos/bank/v1beta1/denom_owners/uhash`
  (paginate to the end) and `/cosmos/bank/v1beta1/spendable_balances/{addr}`
  for vesting accounts; `kind` ∈ `user | module | vesting | validator_operator`
  by looking the address up in `/cosmos/auth/v1beta1/accounts/{addr}` (module
  accounts and `PeriodicVestingAccount` are distinguishable by `@type`).
- `GET /v1/accounts/top?limit` — top holders with share of supply, kind, label
  (label module accounts: `serviceproof` = "Useful-service reserve",
  `treasury`/`growth`/`dev_grants`/`liquidity` sub-accounts from
  `/hashgram/treasury/v1/reserves`, `founder` module = "Founder revenue",
  Founder beneficiary from `/hashgram/founder/v1/params` = "Founder (vesting)").
- `GET /v1/accounts/{address}` — balance, spendable, vesting schedule if
  vesting (from auth), delegations (`/cosmos/staking/v1beta1/delegations/{addr}`),
  rewards (`/cosmos/distribution/v1beta1/delegators/{addr}/rewards`),
  username (`/hashgram/username/v1/reverse/{owner}`), provider record if any
  (`/hashgram/serviceproof/v1/provider/{operator}`), tx count, first/last seen.
- `GET /v1/accounts/count` — distinct addresses seen in transfers ∪ denom_owners.

**Validators & staking**
- `GET /v1/validators` — moniker, operator, consensus address, tokens, voting
  power %, commission, status, jailed, uptime over `signed_blocks_window`
  (compute from `block.last_commit.signatures` you already ingest — add a
  `signatures(height, validator_cons, signed bool)` table), missed blocks,
  self-delegation, delegator count.
- `GET /v1/validators/{operator}` — the above plus delegations list and the
  last 50 proposed blocks.
- `GET /v1/staking` — bonded/unbonded/total, bonded ratio, unbonding time
  (504h), max validators, params.

**Rewards ("prizes")** — the useful-service reward system, `x/serviceproof`
- `GET /v1/rewards/params` — from `/hashgram/serviceproof/v1/params`:
  epoch_blocks 21600, emission_rate_bps 5, max_epoch_emission 250,000 HASH,
  min_bond 1,000 HASH, max_provider_share_bps 500, credit rates.
- `GET /v1/rewards/reserve` — `/hashgram/serviceproof/v1/reserve` and
  `/hashgram/serviceproof/v1/emission_schedule` (the geometric schedule; also
  compute and return the projected budget for the next 30, 365 epochs).
- `GET /v1/rewards/epochs?limit&cursor` — per epoch: number, start/end height,
  budget, paid, providers paid, from `/hashgram/serviceproof/v1/epoch/{number}`
  and `.../epoch/current`; store as they close.
- `GET /v1/rewards/providers` — extend `/v1/providers`: roles, bond, declared
  storage, reward address, jailed, fraud score, current-epoch credit and
  lifetime paid from `/hashgram/serviceproof/v1/rewards/{operator}`,
  assignments count (`/assignments/{provider}`), challenge pass rate
  (`/challenges/{provider}`).
- `GET /v1/rewards/providers/{operator}` — detail with payout history from
  `transfers` where sender is the `serviceproof` module account.
- Welcome rewards: `GET /v1/rewards/welcome` from `/hashgram/welcome/v1/params`,
  `/hashgram/welcome/v1/status`, `/hashgram/welcome/v1/tiers`, `/hashgram/welcome/v1/claims`.
  It is disabled until an attestor exists; the API must say so plainly.

**Founder & fee transparency**
- `GET /v1/founder` — `/hashgram/founder/v1/params` (fee_basis_points 100,
  ceiling 100 — hardcoded), `/hashgram/founder/v1/revenue` (accrued, paid,
  pending), `/hashgram/founder/v1/beneficiary_history`, the beneficiary's
  vesting schedule from auth (199,000,000 HASH: 19,000,000 spendable,
  180,000,000 over 96 monthly periods), its delegations, and payout history
  (transfers from the founder module account `hash1t9zc2z9qsf707huqa5y3a0vgpra7vlyhyvdeeh`).
- `GET /v1/fees` — `/hashgram/feerouter/v1/params`, `/hashgram/feerouter/v1/totals`,
  `/hashgram/feerouter/v1/service_revenue`: where every fee went, cumulative.
- `GET /v1/treasury` — `/hashgram/treasury/v1/reserves`,
  `/hashgram/treasury/v1/reserve/{name}` for the four names,
  `/hashgram/treasury/v1/disbursements`.

**Governance**
- `GET /v1/gov/proposals`, `GET /v1/gov/proposals/{id}` — from
  `/cosmos/gov/v1/proposals`, with tally, votes, deposit, timeline, and the
  decoded message (e.g. `MsgUpdateParams` diff against current params).
  Proposal #1 (add storage assigner) is live and ends 2026-09-17T13:05:25Z.

**Network**
- `GET /v1/network` — CometBFT `/net_info` peer count and peers (id, moniker,
  remote ip /24 only — never log full IPs of others), hashgram-node
  `127.0.0.1:26672/v1/status` and `/v1/peers` (libp2p peer count, roles seen,
  our peer id `12D3KooWF53MV7ECXQMifSTDShv952Po83P89fbAYy6RTZ4NioMq`), the
  built-in seed lists from `app/params/mainnet/*.txt` (public by design),
  `/hashgram/network/v1/info` and `/hashgram/network/v1/fork_isolation`.
- `GET /v1/network/nodes` — distinct nodes seen in the last 24 h across both
  P2P layers with first/last seen and version; this is the "how many nodes"
  number. Be honest in the payload: `{"consensus_peers":n,"p2p_peers":m,"validators":v}`
  are different things; do not sum them into one fake number.

**Live**
- `GET /v1/live` — Server-Sent Events. Events: `block` (height, time,
  tx_count, proposer), `tx` (hash, type, signer), `stats` every 10 s (the
  `/v1/chain` summary), `epoch` on epoch close, `founder_payout` when the
  founder module pays out (every 7200 blocks), `proposal` on gov state change.
  Source: CometBFT WebSocket `ws://127.0.0.1:26657/websocket` subscribing to
  `tm.event='NewBlock'` and `tm.event='Tx'`; fall back to polling if the
  socket drops. Heartbeat comment every 15 s. Max 2,000 concurrent clients,
  then 503 with `Retry-After`.

## Deliverable B — OpenAPI

Generate `indexer/openapi.yaml` (OpenAPI 3.1) for every `/v1/*` route, served
at `GET /v1/openapi.yaml` and rendered at `GET /v1/docs` (Redoc or Scalar,
monochrome theme). The website links to it.

## Deliverable C — publishing (`deploy/`)

- `deploy/caddy/Caddyfile` for `api.hashgram.io` → `127.0.0.1:1318`:
  automatic TLS, HTTP/2 and HTTP/3, `Access-Control-Allow-Origin` restricted
  to `https://hashgram.io` and `https://www.hashgram.io`, security headers,
  gzip/zstd, request body size 1 KB (it is read-only), rate limit 60 req/min/IP
  on `/v1/*` and 10 SSE connections/IP, access log with IPs truncated to /24
  (see `docs/LOGGING_POLICY.md`).
- `deploy/systemd/caddy.service` hardening in the style of
  `deploy/systemd/hashgram-indexer.service` (note: Go services must run with
  `MemoryDenyWriteExecute=false`; see the comment in that unit, and keep
  `StartLimit*` keys in `[Unit]`).
- a new `install-public-api.sh` in `scripts/install`: installs Caddy, opens **only**
  80/443 in ufw, enables the units, runs `hashgramctl mainnet-preflight` at the
  end and fails if it fails.
- The indexer keeps listening on `127.0.0.1:1318`. Add `public_base_url` and
  `allowed_origins` to `indexer/config.go` for the OpenAPI servers block and
  CORS; defaults keep today's behaviour.

## Deliverable D — performance and correctness

- Backfill: on start, the ingester must catch up from `index_state` to head
  and then follow; it must survive node restarts and RPC timeouts with
  exponential backoff; `hashgram-indexer check` must verify `blocks` count =
  height and re-index gaps.
- Every list endpoint must answer in < 50 ms at 1,000,000 blocks: add the
  indexes you need; explain each in `schema.go` comments.
- Money is `uhash` integers (`int64` is not enough for supply sums — use
  `numeric` in Postgres and strings in JSON, exactly as the chain does).
  `HASH` = `uhash / 1,000,000`; never format on the server.
- Tests: table-driven Go tests for every decoder and every SQL projection,
  using recorded fixtures from this chain (record them with a script in
  `indexer/testdata/`, commit the fixtures). Run `make test` and
  `scripts/dev/check-docs.sh`; both must pass.
- Update `docs/CLIENT_CONNECTIVITY_SPEC.md` §10 "Same-host and indexer APIs"
  and `docs/OPERATIONS.md` with the new routes and the publishing procedure.

## Out of scope — what not to build

- Do not expose 26657 / 1317 / 9091 / 26672, not even behind Caddy.
- Do not add write endpoints, wallets, key handling, faucets or broadcast
  proxies. This API is read-only by design.
- Do not compute the genesis hash from RPC `/genesis`; CometBFT re-serialises
  it and the hash will not match. Read the pin from the `network.json` file in `/etc/hashgram`.
- Do not invent a single "nodes online" number; report the three real ones.
- Do not add third-party analytics, external CDNs or call-home telemetry.
- Do not store full IP addresses of peers or API clients.

## Definition of done

1. `curl https://api.hashgram.io/v1/chain` returns the live head within 5 s of
   the block; `/v1/live` streams blocks to a browser.
2. `/v1/accounts/top` shows the reserve (500,000,000 HASH), treasury
   sub-accounts, Founder (199,000,000 incl. vesting) and the operator with
   correct labels and shares summing to ≤ 100 %.
3. `/v1/founder` shows fee_basis_points 100, beneficiary
   `hash13t8v5nnghrvgcuuqcrt9k5wyhtqwq7fl3ynjpy`, accrued > 0, vesting
   schedule with 96 periods, delegation 60,000,000 HASH to
   `hashvaloper127zemcfnxd3jrldpjzzgcckek4dswyw0l7rfcq`.
4. `/v1/rewards/reserve` shows exactly 500,000,000,000,000 uhash minus
   anything paid, and the schedule's first budget 250,000 HASH.
5. `hashgram-indexer rebuild` from height 1 reproduces identical responses.
6. `hashgramctl mainnet-preflight` still passes; `ss -ltn` shows only 22,
   80, 443, 26656, 26670, 3478, 5349 bound to non-loopback addresses.
7. `make test`, `make lint`, `scripts/dev/check-docs.sh` pass; commit with a
   message that lists the new routes.
