# hashgram.io — one prompt: live explorer, network dashboard, documentation

Copy everything below the line into an AI coding agent opened on a clone of
this repository on a **fresh, separate VPS** — the "explorer VPS". That
machine runs its **own full node** of the chain (not a validator), its own
indexer and PostgreSQL, the API at `hashgram.io/api/v1/…`, the website at
`hashgram.io`, and Caddy in front. It follows the network like any other
node, through the seed list built into the binaries, so it keeps working
when the genesis server is one node among a thousand — or gone.

---

You are building **hashgram.io**, the official site of Hashgram — a live
Layer-1 blockchain (Cosmos SDK v0.53 / CometBFT, chain-id `hashgram-1`,
launched 2026-09-10, genesis SHA-256
`e322bc2319f6e0173286fa526dab5a8ff8ad0797c7b80dd03e7c9d98621d5e4d`), with a
fixed supply of 1,000,000,000 HASH, useful-service rewards for
storage/relay/media nodes, a 1 % founder revenue share, on-chain usernames
and identity, and a peer-to-peer social/messaging layer.

The site is a **block explorer, a network dashboard and the documentation**,
in English, with everything updating live from **this machine's own node**.
Four parts, one delivery:

- **Part 0 — the explorer VPS**: a full node + indexer on this host, joined
  to Mainnet with no manual peer configuration.
- **Part A — the read API** (`indexer/`, Go): extend the existing indexer so
  it can serve every number the site shows, plus a live event stream.
- **Part B — the website** (`web/`, Next.js/TypeScript): black-and-white,
  ultra-modern, live.
- **Part C — publishing**: Caddy on this host serves `hashgram.io` with
  `/api/*` → the indexer and everything else → the site.

## Independence is the design goal

The website must never depend on the genesis server. This VPS reads only
from its own node (`127.0.0.1`), which learns peers from the seed list
compiled into `hashgramd` / `hashgram-node` (`app/params/mainnet/`) and then
from peer exchange and the DHT. If the genesis host disappears, this node
keeps following the chain for as long as the chain itself is live (≥ ⅔ of
validator power online), and the site keeps working. Do not hardcode the
genesis server's IP, node id or peer id anywhere in `web/` or `indexer/`.

Read before writing: `docs/CLIENT_CONNECTIVITY_SPEC.md` (all endpoints and
the traps), `docs/TOKENOMICS.md`, `docs/SERVICE_REWARDS.md`,
`docs/DECENTRALIZATION.md`, `docs/OPERATIONS.md`, `docs/LOGGING_POLICY.md`,
`indexer/` (`api.go`, `chain.go`, `schema.go`, `config.go`),
`cmd/hashgram-indexer/main.go`, `deploy/systemd/hashgram-indexer.service`,
`indexer.toml` in `/etc/hashgram`.

## The rule that shapes everything

The node's admin interfaces — CometBFT RPC `127.0.0.1:26657`, Cosmos REST
`127.0.0.1:1317`, gRPC `127.0.0.1:9091`, hashgram-node API `127.0.0.1:26672`,
indexer `127.0.0.1:1318` — are **loopback only** and stay that way.
`hashgramctl mainnet-preflight` fails if 26657 or 1317 is reachable from
outside. Do not change any `laddr`, `address` or `listen` in the chain
home's `config` directory or in `/etc/hashgram`. The only thing the internet
reaches is Caddy on 80/443. The browser talks to `hashgram.io/api`, never to
the node.

Second rule: the indexer database is a **rebuildable cache**. Every number
served must be derivable from the chain; `hashgram-indexer rebuild` must
reproduce it. Store nothing that is not on chain or in the node API.

---

# Part 0 — the explorer VPS (a full node that follows Mainnet)

Fresh Ubuntu Server 24.04, ≥ 4 vCPU, 8 GB RAM, ≥ 200 GB NVMe (chain +
PostgreSQL grow), public IPv4. Then, as root, in the repository clone:

```bash
sudo scripts/install/bootstrap-ubuntu.sh        # users, dirs, ufw, PostgreSQL on loopback, all binaries, units
# the chain home created by the installer is picked up automatically; no HASHGRAM_HOME needed
hashgramctl init --moniker hashgram-io           # a full node; its consensus key is never used for signing
hashgramctl join-mainnet                         # no arguments: genesis, hash and seeds are built in
hashgramctl network-info                         # pin must be e322bc23…5e4d
hashgramctl configure-role indexer               # P2P layer follows social events for the feed projections; no rewards role
hashgramctl start
hashgramctl chain-status                         # wait until catching_up = false and peers > 0
systemctl enable --now hashgram-indexer          # indexer.toml is written by the installer, points at 127.0.0.1
curl -s 127.0.0.1:1318/v1/health                 # {"chain_height":…,"status":"ok"}
```

Checks that must hold before Part A starts: `ss -ltn` shows 26657, 1317,
9091, 26672, 1318 and 5432 on `127.0.0.1` only; `hashgramctl mainnet-preflight`
passes (this is a non-validator node; the validator-specific checks report
not-applicable, everything else must pass); `journalctl -u hashgram-node`
shows "using the Mainnet list built into this binary" followed by
connections. **Never** copy `priv_validator_key.json` or `node_key.json` from
any other machine.

Install Node.js 22 LTS (for `web/`) from NodeSource or `fnm`; `pnpm` via
corepack. Nothing else is installed system-wide.

---

# Part A — the read API (Go, `indexer/`)

## What exists

`indexer/` polls `chain_rpc` every 3 s (`ChainIngester.IndexBlock`), stores
`blocks`, `transactions`, `transfers`, `usernames`, `identities`,
`providers` and the social projections, and serves on `127.0.0.1:1318`:
`/v1/health`, `/v1/stats`, `/v1/blocks/latest`, `/v1/txs/{hash}`,
`/v1/accounts/{address}/transactions`, `/v1/accounts/{address}/transfers`,
`/v1/providers`, plus `/v1/feed/…`, `/v1/reels…`, `/v1/profiles/…`,
`/v1/search/…` (social — leave as is). `systemctl status hashgram-indexer`
is active.

## Add these routes

All JSON; lists paginated (`?limit=` max 100, `?cursor=` opaque); money as
`uhash` **strings** (use Postgres `numeric`; never `int64` for sums, never
format on the server); `Cache-Control` per volatility.

**Chain**
- `GET /v1/chain` — chain_id, network_id, genesis_hash (from the
  `network.json` pin in `/etc/hashgram`), genesis_time, height, block time,
  average block time (last 100), node/app versions (`/status`), validator
  count, bonded tokens, total supply (`/cosmos/bank/v1beta1/supply`),
  circulating (supply minus module accounts and the Founder's unvested
  balance), total txs, total accounts.
- `GET /v1/blocks?limit&cursor`, `GET /v1/blocks/{height}` — header, proposer
  (consensus address → moniker via `/cosmos/staking/v1beta1/validators` and RPC
  `/validators`), txs, size, gas, events summary (transfers, delegations,
  founder payouts), signatures present/missing.
- `GET /v1/txs?limit&cursor&type=`, `GET /v1/txs/{hash}` — decoded messages
  (use the SDK codec from `app/`, not string parsing), fee and its split (see
  fees), events, raw log on failure.
- `GET /v1/search?q=` — height, tx hash, block hash, `hash1…`,
  `hashvaloper1…`, `@username` (`/hashgram/username/v1/lookup/{name}`), peer id
  (`12D3Koo…`) → `{type,id}`.

**Accounts**
- New table `balances(address, uhash, spendable_uhash, kind, updated_height)`
  refreshed every 20 blocks from `/cosmos/bank/v1beta1/denom_owners/uhash`
  (paginate to the end) and `/cosmos/bank/v1beta1/spendable_balances/{addr}`;
  `kind` ∈ `user | module | vesting | validator_operator` from
  `/cosmos/auth/v1beta1/accounts/{addr}` (`@type` distinguishes module and
  `PeriodicVestingAccount`).
- `GET /v1/accounts/top?limit` — rank, label, kind, balance, share of supply.
  Labels: `serviceproof` module = "Useful-service reserve"; the four
  sub-accounts from `/hashgram/treasury/v1/reserves` = "Treasury", "Growth",
  "Developer grants", "Liquidity"; founder module
  `hash1t9zc2z9qsf707huqa5y3a0vgpra7vlyhyvdeeh` = "Founder revenue (module)";
  the beneficiary from `/hashgram/founder/v1/params` = "Founder (vesting)".
- `GET /v1/accounts/{address}` — balance, spendable, vesting schedule (auth),
  delegations (`/cosmos/staking/v1beta1/delegations/{addr}`), rewards
  (`/cosmos/distribution/v1beta1/delegators/{addr}/rewards`), username
  (`/hashgram/username/v1/reverse/{owner}`), provider record
  (`/hashgram/serviceproof/v1/provider/{operator}`), tx count, first/last seen.
- `GET /v1/accounts/count`.

**Validators & staking**
- Add `signatures(height, validator_cons, signed)` from
  `block.last_commit.signatures`.
- `GET /v1/validators`, `GET /v1/validators/{operator}` — moniker, operator,
  consensus address, tokens, voting power %, commission, status, jailed,
  uptime over `signed_blocks_window` (30,000 blocks), missed blocks,
  self-delegation, delegator count, delegations, last 50 proposed blocks.
- `GET /v1/staking` — bonded/unbonded/total, ratio, unbonding 504 h, max
  validators 100, params.

**Rewards (what visitors mean by "prizes")** — `x/serviceproof`
- `GET /v1/rewards/params` — `/hashgram/serviceproof/v1/params` (epoch 21,600
  blocks, emission 5 bps, cap 250,000 HASH/epoch, bond 1,000 HASH, provider
  cap 500 bps, credit rates).
- `GET /v1/rewards/reserve` — `/hashgram/serviceproof/v1/reserve` and
  `/hashgram/serviceproof/v1/emission_schedule`; also project the budget for
  the next 30 / 365 / 3,650 epochs.
- `GET /v1/rewards/epochs?limit&cursor` — from
  `/hashgram/serviceproof/v1/epoch/current` and `/hashgram/serviceproof/v1/epoch/{number}`,
  stored as they close: budget, paid, providers paid.
- `GET /v1/rewards/providers`, `GET /v1/rewards/providers/{operator}` —
  roles, bond, declared storage, reward address, jailed, fraud score,
  current-epoch credit and lifetime paid (`/hashgram/serviceproof/v1/rewards/{operator}`),
  assignments (`/hashgram/serviceproof/v1/assignments/{provider}`), challenge
  pass rate (`/hashgram/serviceproof/v1/challenges/{provider}`), fraud
  (`/hashgram/serviceproof/v1/fraud/{provider}`), payout history (transfers
  from the `serviceproof` module account).
- `GET /v1/rewards/welcome` — `/hashgram/welcome/v1/params`, `/hashgram/welcome/v1/status`,
  `/hashgram/welcome/v1/tiers`, `/hashgram/welcome/v1/claims`. Disabled until an
  attestor exists; say so in the payload.

**Founder, fees, treasury**
- `GET /v1/founder` — `/hashgram/founder/v1/params` (fee_basis_points 100,
  ceiling 100 — hardcoded in the binary), `/hashgram/founder/v1/revenue`,
  `/hashgram/founder/v1/beneficiary_history`, the beneficiary's vesting
  schedule (199,000,000 HASH: 19,000,000 spendable, 180,000,000 in 96 monthly
  periods), its delegations, payout history (transfers from the founder module
  account every 7,200 blocks).
- `GET /v1/fees` — `/hashgram/feerouter/v1/params`, `/hashgram/feerouter/v1/totals`,
  `/hashgram/feerouter/v1/service_revenue`.
- `GET /v1/treasury` — `/hashgram/treasury/v1/reserves`,
  `/hashgram/treasury/v1/reserve/{name}` (`treasury|growth|dev_grants|liquidity`),
  `/hashgram/treasury/v1/disbursements`.

**Governance**
- `GET /v1/gov/proposals`, `GET /v1/gov/proposals/{id}` — from
  `/cosmos/gov/v1/proposals`: tally, votes, deposit, timeline, decoded message
  (`MsgUpdateParams` shown as a diff against current params). Proposal #1
  (storage assigner) ends 2026-09-17T13:05:25Z.

**Network**
- `GET /v1/network` — CometBFT `/net_info` (peer count; peers with ip
  truncated to /24), hashgram-node `127.0.0.1:26672/v1/status` and `/v1/peers`
  (libp2p peer count, roles, this node's own peer id — read at runtime, not
  hardcoded), the built-in seed lists from `app/params/mainnet/*.txt`,
  `/hashgram/network/v1/info`, `/hashgram/network/v1/fork_isolation`.
- `GET /v1/network/nodes` — distinct nodes seen in 24 h across both layers.
  Return `{"consensus_peers":n,"p2p_peers":m,"validators":v}` as three
  numbers; never sum them into one.

**Live**
- `GET /v1/live` — Server-Sent Events: `block`, `tx`, `stats` (every 10 s),
  `epoch`, `founder_payout`, `proposal`. Source: CometBFT WebSocket
  `ws://127.0.0.1:26657/websocket` (`tm.event='NewBlock'`, `tm.event='Tx'`)
  with polling fallback; heartbeat every 15 s; max 2,000 clients then 503 +
  `Retry-After`.

**OpenAPI** — `indexer/openapi.yaml` (3.1) for every `/v1/*` route, served at
`/v1/openapi.yaml` and rendered at `/v1/docs` (Scalar or Redoc, monochrome).

## API quality bar
- Backfill from `index_state` to head, then follow; survive node restarts and
  RPC timeouts with backoff; `hashgram-indexer check` verifies `blocks` count
  = height and re-indexes gaps.
- Every list endpoint < 50 ms at 1,000,000 blocks; comment every index in
  `schema.go`.
- Table-driven Go tests with recorded fixtures from this chain
  (`indexer/testdata/`); `make test`, `make lint` pass.
- Add `public_base_url` and `allowed_origins` to `indexer/config.go`
  (defaults keep today's behaviour). The indexer keeps listening on
  `127.0.0.1:1318`.

---

# Part B — the website (`web/`, Next.js)

## Non-negotiable design constraints

1. **Black and white only.** Exactly `#000000` and `#FFFFFF`, plus
   opacity-derived greys of those two. No accent colour anywhere — not links,
   charts, status, focus rings, favicon or logo. Meaning is carried by weight,
   size, spacing, borders, motion and glyphs (✓ ✗ ▲ ▼), never by hue. Charts
   are monochrome (line weight, dash pattern, hatching, opacity). Enforce it:
   Stylelint `color-no-hex` allow-list, and a Playwright test that samples
   rendered pixels on every route in both themes and fails on any saturated
   colour.
2. Two monochrome themes: dark (black background, default) and light, via
   toggle, honouring `prefers-color-scheme`.
3. One variable sans for UI (Inter or Geist), one monospace for hashes,
   addresses, numbers (tabular figures). Hashes truncated in the middle
   (`hash13t8…ynjpy`), copy on click, full value on hover/focus.
4. Motion subtle and purposeful (new blocks slide in, numbers tick); respect
   `prefers-reduced-motion`.
5. WCAG 2.2 AA, full keyboard navigation, ARIA live regions for the feed.
6. Self-hosted fonts; no third-party scripts, analytics, tag managers, CDNs.

## Logo
SVG, monochrome: `web/public/logo.svg` (mark), `wordmark.svg`,
`favicon.svg`, `og-image.png` (1200×630, black). Direction: derive the mark
from the hash sign `#` — four strokes forming a grid whose intersections read
as blocks in a chain; must work at 16 px and as a wall print, black-on-white
and white-on-black, no gradients. `web/brand/README.md` with clear-space and
minimum-size rules. Show three candidates before committing to one.

## Stack
Next.js App Router, TypeScript, RSC, Tailwind with a two-token palette,
`next/font`, MDX for docs. Data from `NEXT_PUBLIC_API_BASE` (default `/api`,
same origin), typed client generated from `/api/v1/openapi.yaml`
(openapi-typescript). Live via `EventSource('/api/v1/live')` with reconnect;
fall back to polling `/api/v1/chain` every 6 s. Charts: visx/D3 primitives.
Tests: Vitest, Playwright (incl. the colour test), Lighthouse CI budget
(performance ≥ 95, accessibility 100, no layout shift). `output: 'standalone'`,
systemd unit `deploy/systemd/hashgram-web.service` listening on
`127.0.0.1:3000`.

## Genesis pin in the browser
The site hardcodes the genesis hash above and compares it to
`/api/v1/chain.genesis_hash` on load. Mismatch → full-width banner "This
API is not serving Hashgram Mainnet" and the explorer is disabled. Nothing
else is hardcoded.

## Site map

**/ Home** — wordmark, one-sentence definition, live height ticking, last
block age, search (height / tx / address / @username / validator / peer id →
`/api/v1/search`), live strip of 10 blocks + 10 txs, key stats with 24 h
monochrome sparklines: supply (fixed — "no mint module"), circulating, bonded
ratio, validators, consensus peers, P2P peers, providers, txs, accounts,
block time, current epoch + budget, founder revenue accrued.

**/blocks, /blocks/[height]** — live-prepending list; detail with header,
proposer, signatures present/missing, txs, events, raw JSON toggle, prev/next.

**/txs, /txs/[hash]** — type filter (Send, Delegate, Vote, RegisterProvider,
SubmitReceipt, ClaimFounderRevenue, RegisterUsername, …); detail with decoded
messages, fee and where it went (validators, founder 1 %, service revenue),
events, raw log on failure.

**/accounts, /accounts/[address]** — top holders: rank, label/address, kind,
balance, share bar, tx count; footnote explaining module and reserve
accounts. Detail: balance, spendable vs vesting (step chart), delegations,
rewards, username, provider record, txs/transfers tabs, monochrome QR.

**/validators, /validators/[operator]** — voting power distribution, uptime,
commission, jailed, self-delegation, delegators; recent proposed blocks. Explain
the >⅔ liveness rule and how many validators exist today.

**/rewards — "How nodes earn"** — reserve 500,000,000 HASH live; emission
chart `budget = min(remaining × 5/10,000, 250,000)` per epoch (≈ 1 day)
projected 1/5/10 years; current epoch (blocks left, budget, providers,
per-provider cap 5 % = 12,500 HASH/day today); providers table + detail with
payout history; past epochs; what earns credit (storage 100/GiB-epoch, relay
200/GiB, retrieval 150/GiB, calls 300/h; bond 1,000 HASH; fraud → 5 % slash +
jail). State plainly: **there is no mining** — rewards are for real bytes
stored and served, and the budget is a ceiling, not a guarantee. Welcome
rewards: tiers, and "currently disabled — no attestor registered" when the API
says so.

**/founder — transparency** — 1 % of protocol fee revenue (not a transfer
tax), hardcoded ceiling, beneficiary, accrued/paid/pending live, payout every
7,200 blocks, history; allocation 199,000,000 HASH with the 96-step vesting
chart and "today" marked; the founder address's delegation and voting power
live; link to the governance parameters.

**/governance, /governance/[id]** — proposals, monochrome tally bars, quorum
40 %, threshold 50 %, veto 33.4 %, 7-day voting, timeline, decoded message,
votes.

**/network** — consensus peers, libp2p peers, validators as three separate
numbers with definitions; peer table (moniker/peer id, version, roles,
first/last seen, ip /24); built-in seeds and "join with `hashgramctl
join-mainnet`, no arguments"; versions seen; network id, magic `HGM1`,
protocol major version, fork isolation explained.

**/docs** — render from this repository's `docs/` with MDX, left nav, right
TOC, build-time full-text search, code copy, source commit + last modified:
`ARCHITECTURE.md`, `PROTOCOL.md`, `TOKENOMICS.md`, `SERVICE_REWARDS.md`,
`DECENTRALIZATION.md`, `MAINNET.md`, `OPERATIONS.md`, `NODE_ROLES.md`,
`CLIENT_CONNECTIVITY_SPEC.md`, `SOCIAL_PROTOCOL.md`, `MESSAGING.md`,
`CALLS.md`, `STORAGE.md`, `MODERATION.md`, `SECURITY.md`, `THREAT_MODEL.md`,
`DISASTER_RECOVERY.md`, `LOGGING_POLICY.md`. Exclude `PROMPT_*.md`, `*_KA.md`,
`FOUNDER_LAUNCH_RUNBOOK.md`, `PHASE1_REPORT.md`, `FINAL_REPORT.md`. Add three
web-native pages: **What is Hashgram** (plain language), **Run a node**
(`hashgramctl init` → `join-mainnet` → `configure-role` → earning; honest that
earnings need real traffic), **API** (embeds `/api/v1/docs`).

**/status** — `/api/v1/health`, head lag, indexer lag, SSE state.

## Behaviour
Timestamps relative in UI, absolute UTC on hover. Amounts: `uhash` strings →
`BigInt` → HASH with ≤ 6 decimals, thousands separators, tabular figures.
Designed empty/error states ("Indexer is N blocks behind"). Every row a link,
every hash copyable. Open Graph images per page (monochrome, large type).
i18n-ready, English shipped.

---

# Part C — publishing on this host

- `deploy/caddy/Caddyfile`:
  `hashgram.io` — `handle /api/*` → strip `/api` → `127.0.0.1:1318`; `handle` →
  `127.0.0.1:3000`; `www.hashgram.io` → 308 to apex. Automatic TLS, HTTP/2 +
  HTTP/3, security headers, zstd/gzip, request body limit 1 KB on `/api`, rate
  limit 60 req/min/IP on `/api` and 10 SSE connections/IP, access log with IPs
  truncated to /24 (`docs/LOGGING_POLICY.md`).
- `deploy/systemd/caddy.service` and `hashgram-web.service`, hardened like
  `hashgram-indexer.service` (Go/Node services need
  `MemoryDenyWriteExecute=false`; `StartLimit*` keys belong in `[Unit]`).
- A new `install-hashgram-io.sh` in `scripts/install`: builds `web/`, installs
  Caddy, opens **only** 80/443 in ufw on top of what `bootstrap-ubuntu.sh`
  opened (26656, 26670), enables the units, then runs
  `hashgramctl mainnet-preflight` and fails if it fails. Idempotent.
- DNS the operator sets: `hashgram.io` A → the explorer VPS, `www` CNAME →
  apex. The genesis server is not involved.
- Backups: the chain data and the index are both rebuildable from the network;
  the only state worth backing up on this host is `web/` content and the Caddy
  config, and they live in git. Document a full rebuild of the explorer VPS
  from scratch in `web/README.md` (Part 0 → Part C, expected time).
- Update `docs/CLIENT_CONNECTIVITY_SPEC.md` §10 and `docs/OPERATIONS.md` with
  the routes and the procedure; run `scripts/dev/check-docs.sh`.

---

## Out of scope — what not to build
- No wallet, key generation, signing, "connect wallet", broadcasting or
  faucet. Read-only, and the footer says so.
- No token price, market cap, exchange links, "buy" buttons.
- No accounts, cookies beyond theme, analytics, external CDNs, call-home.
- No exposure of 26657 / 1317 / 9091 / 26672 / 1318 / 3000 / 5432 to the
  internet.
- No dependency on the genesis server: no hardcoded IP, node id or peer id of
  any specific machine in `web/` or `indexer/`; no reading from a remote RPC.
- No genesis hash computed from RPC `/genesis` (CometBFT re-serialises it;
  read the pin file in `/etc/hashgram`).
- No single invented "nodes online" number; no fake liveness when SSE is down.
- No full IP addresses of peers or visitors in storage or logs.

## Definition of done
0. This VPS's own node is synced (`catching_up = false`) with > 0 consensus
   peers and > 0 libp2p peers, joined with a zero-argument `join-mainnet`;
   `grep -rn 186.241 web/ indexer/` returns nothing.
1. `https://hashgram.io/` shows a new block within 5 s of production without
   reload; `/api/v1/chain` returns the live head; `/api/v1/live` streams.
2. `/accounts` lists the useful-service reserve (500,000,000 HASH) as the
   largest holder with its label; the Founder with 19,000,000 spendable /
   180,000,000 vesting; shares sum ≤ 100 %.
3. `/founder` shows fee_basis_points 100, beneficiary
   `hash13t8v5nnghrvgcuuqcrt9k5wyhtqwq7fl3ynjpy`, accrued > 0, 96 vesting
   periods, delegation 60,000,000 HASH to
   `hashvaloper127zemcfnxd3jrldpjzzgcckek4dswyw0l7rfcq`.
4. `/rewards` renders the emission schedule and today's per-provider cap from
   live params and states there is no mining.
5. `/docs` renders all listed files with working links, code copy, search.
6. Playwright colour test finds no saturated pixel on any route, both themes;
   Lighthouse performance ≥ 95, accessibility 100, best practices 100, SEO 100.
7. `hashgram-indexer rebuild` reproduces identical API responses.
8. `hashgramctl mainnet-preflight` passes; `ss -ltn` shows only 22, 80, 443,
   26656, 26670 on non-loopback addresses (no TURN on this host — it is not a
   call node).
9. `make test`, `make lint`, `scripts/dev/check-docs.sh`, `pnpm test`,
   `pnpm build` pass; `web/README.md` documents env vars, local development
   against the local indexer, and deployment.
