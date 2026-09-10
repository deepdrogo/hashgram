# Prompt 2 of 2 — hashgram.io: live explorer and documentation site

Copy everything below the line into an AI coding agent in a **new, empty
repository** for the website. It consumes the public API produced by
`docs/PROMPT_EXPLORER_API.md` (`https://api.hashgram.io`). Give the agent
read access to this repository too (or a copy of `docs/`), because the
documentation section is rendered from these Markdown files.

---

You are building **hashgram.io**, the official website of Hashgram — a live
Layer-1 blockchain (Cosmos SDK / CometBFT, chain-id `hashgram-1`, launched
2026-09-10) with a fixed supply of 1,000,000,000 HASH, useful-service rewards
for storage/relay/media nodes, a 1 % founder revenue share, on-chain
usernames and identity, and a peer-to-peer social/messaging layer. The site is
a **block explorer, a network dashboard, and the documentation**, in English,
with everything updating live from the chain. It must feel like a product
built in 2026 by people who care, not like a template.

## Non-negotiable design constraints

1. **Black and white only.** Exactly two colours: `#000000` and `#FFFFFF`, plus
   opacity-derived greys of those two (e.g. `rgba(255,255,255,0.6)`). No accent
   colour anywhere — not for links, charts, status, focus rings, favicon or the
   logo. Meaning is carried by weight, size, spacing, borders, motion, and
   icons/glyphs (✓ ✗ ▲ ▼), never by hue. Charts are monochrome: line weight,
   dash patterns, hatching and opacity distinguish series. Enforce this with a
   lint rule (Stylelint `color-no-hex` allow-list) and a Playwright test that
   samples rendered pixels and fails on any saturated colour.
2. Two themes, both monochrome: dark (black background) as default, light
   (white background) via toggle, honouring `prefers-color-scheme`.
3. Typography does the work: one variable sans for UI (e.g. Inter or Geist),
   one monospace for hashes, addresses and numbers (tabular figures). Hashes
   are truncated in the middle (`hash13t8…ynjpy`) with copy-on-click and full
   value on hover/focus.
4. Motion is subtle and purposeful: new blocks slide in, numbers tick with
   `font-variant-numeric: tabular-nums`, respect `prefers-reduced-motion`.
5. Accessibility: WCAG 2.2 AA contrast (trivial in monochrome — but check
   grey-on-grey), full keyboard navigation, skip links, ARIA live regions for
   the live feed, focus visible.
6. Self-hosted fonts; no third-party scripts, analytics, tag managers or
   CDNs. Privacy is part of the brand.

## Logo

Design the logo in SVG, monochrome, deliverable as `public/logo.svg`
(mark), `public/wordmark.svg` (mark + "Hashgram"), `public/favicon.svg`,
`public/og-image.png` (1200×630, black background). Direction: derive the mark
from the hash sign `#` — four strokes forming a grid whose intersections
suggest blocks in a chain; it must work at 16 px and as a 2-metre wall print,
in black-on-white and white-on-black, with no gradients. Provide a
`brand/README.md` with clear-space and minimum-size rules. Show me three
candidates as SVG before committing to one.

## Stack

- Next.js (App Router, TypeScript, React Server Components), Tailwind with a
  two-token colour palette, `next/font` self-hosting, MDX for docs.
- Data: fetch from `NEXT_PUBLIC_API_BASE` (default `https://api.hashgram.io`),
  typed by generating a client from `GET /v1/openapi.yaml` (openapi-typescript).
  Live updates via `EventSource` on `/v1/live` with reconnect and backoff;
  fall back to polling `/v1/chain` every 6 s if SSE fails.
- Charts: a small monochrome chart layer (visx or D3 primitives) — no heavy
  charting library with its own colour theme.
- Tests: Vitest for units, Playwright for e2e (including the colour test),
  Lighthouse CI budget: performance ≥ 95, accessibility 100, no layout shift.
- Deploy: static-first (`output: 'standalone'` or static export where
  possible), Dockerfile, and a Caddy site block for `hashgram.io` /
  `www.hashgram.io` → this app. The API is a separate host (`api.hashgram.io`);
  never proxy it through the website.

## Site map and what each page shows

Every number below comes from the API; nothing is hardcoded except the
genesis hash, which the site **must** display and compare against
`GET /v1/chain.genesis_hash` — a mismatch shows a full-width warning banner
("This API is not serving Hashgram Mainnet") and disables the explorer.
Genesis hash: `e322bc2319f6e0173286fa526dab5a8ff8ad0797c7b80dd03e7c9d98621d5e4d`.

**/ Home — the network right now**
- Hero: wordmark, one-sentence definition, the live block height ticking, the
  last block's age ("2 s ago"), search bar (height / tx hash / address /
  @username / validator / peer id → `/v1/search`).
- Live strip: latest 10 blocks and latest 10 transactions sliding in from
  `/v1/live`.
- Key stats: total supply (fixed, 1,000,000,000 HASH, "no mint module"),
  circulating, bonded ratio, validators, consensus peers, P2P peers,
  providers, transactions total, accounts, average block time, current
  epoch and its budget, founder revenue accrued.
- A monochrome sparkline per stat (last 24 h).

**/blocks, /blocks/[height]**
- Paginated list with live prepend; detail shows header, proposer (linked
  validator), signatures count / missing, txs, events summary, raw JSON
  toggle, prev/next.

**/txs, /txs/[hash]**
- List with type filter (Send, Delegate, Vote, RegisterProvider,
  SubmitReceipt, ClaimFounderRevenue, RegisterUsername, …); detail shows
  decoded messages in a readable form, fee and where the fee went (validator
  share, founder 1 %, service revenue — from the API's fee split), events,
  raw log on failure, JSON toggle.

**/accounts, /accounts/[address]**
- "Top holders" table: rank, label or address, kind (module / vesting / user /
  operator), balance, share of supply as a monochrome bar, tx count. Module
  and reserve accounts are labelled and explained in a footnote.
- Detail: balance, spendable vs vesting (with the vesting schedule drawn as a
  step chart), delegations, rewards, username, provider record, transactions
  and transfers tabs, QR of the address (monochrome, naturally).

**/validators, /validators/[operator]**
- Voting power distribution (monochrome stacked bar), uptime, commission,
  jailed state, self-delegation, delegators; detail with recent proposed
  blocks and delegation list. Explain plainly the >⅔ liveness rule and how
  many validators the network has today.

**/rewards — "How nodes earn"** (this is what a visitor reading "prizes" wants)
- The reserve: 500,000,000 HASH, live remaining balance.
- The emission schedule chart: `budget = min(remaining × 5 / 10,000, 250,000)`
  per epoch (≈ 1 day), projected 1 / 5 / 10 years, from `/v1/rewards/reserve`.
- Current epoch: number, blocks remaining, budget, providers, per-provider
  cap (5 % → 12,500 HASH/day today).
- Providers table: roles, bond, declared storage, challenge pass rate,
  credit this epoch, lifetime paid, reward address; detail page per provider
  with payout history.
- Past epochs table.
- What earns credit (from `/v1/rewards/params`): storage 100 / GiB-epoch,
  relay 200 / GiB, retrieval 150 / GiB, calls 300 / hour; bond 1,000 HASH;
  fraud → 5 % slash + jail. Say clearly: **there is no mining**; rewards
  come from real bytes stored and served, and the budget is a ceiling, not a
  guarantee.
- Welcome rewards section: tiers, and "currently disabled — no attestor
  registered" when `/v1/rewards/welcome` says so.

**/founder — transparency**
- 1 % of protocol fee revenue (not a transfer tax), hardcoded ceiling 100 bps,
  beneficiary address, accrued / paid / pending live, payout period 7,200
  blocks, payout history.
- Allocation: 199,000,000 HASH; 19,000,000 spendable; 180,000,000 vesting in
  96 monthly steps over 8 years — drawn as a step chart with "today" marked.
- Delegation and voting power of the founder address, live.
- Link to the governance parameters that constrain all of this.

**/governance, /governance/[id]**
- Proposals with status, tally bars (monochrome), quorum 40 %, threshold
  50 %, veto 33.4 %, voting period 7 days, timeline; detail with decoded
  message and votes.

**/network**
- Consensus peers, libp2p peers, validators — as three separate numbers with
  one-line definitions. Peer table (moniker/peer id, version, roles, first
  seen, last seen; IPs truncated to /24 as the API provides).
- Built-in seeds and bootstrap peers (public), and how to join:
  `hashgramctl join-mainnet` with no arguments.
- Client and node software versions seen.
- Protocol facts from `/v1/network`: network id, magic `HGM1`, protocol
  major version, fork-isolation explanation.

**/docs — the documentation**
- Render these Markdown files from the Hashgram repository's `docs/` with
  MDX, a left navigation, right-hand table of contents, full-text search
  (client-side index built at build time), copy buttons on code, and
  "edit on git" links: `ARCHITECTURE.md`, `PROTOCOL.md`, `TOKENOMICS.md`,
  `SERVICE_REWARDS.md`, `DECENTRALIZATION.md`, `MAINNET.md`, `OPERATIONS.md`,
  `NODE_ROLES.md`, `CLIENT_CONNECTIVITY_SPEC.md`, `SOCIAL_PROTOCOL.md`,
  `MESSAGING.md`, `CALLS.md`, `STORAGE.md`, `MODERATION.md`, `SECURITY.md`,
  `THREAT_MODEL.md`, `DISASTER_RECOVERY.md`, `LOGGING_POLICY.md`.
  Exclude `PROMPT_*.md`, `*_KA.md`, `FOUNDER_LAUNCH_RUNBOOK.md`, `PHASE1_REPORT.md`,
  `FINAL_REPORT.md`.
- Add three written-for-the-web pages: **What is Hashgram** (plain language,
  ten minutes), **Run a node** (from `hashgramctl init` to earning; be honest
  that earnings need real traffic), **API** (embed `/v1/docs` from the API
  host).
- Every docs page shows its source commit hash and last-modified date.

**/status** — API and node health (`/v1/health`, head lag, indexer lag,
SSE connected), for people who wonder whether the site or the chain is slow.

## Behaviour details

- All timestamps: absolute in UTC on hover, relative in the UI.
- All amounts: `uhash` strings from the API → format client-side as HASH
  with six decimals max, thousands separators, tabular figures; never use
  JavaScript `Number` for uhash (use `BigInt`).
- Empty and error states are designed, not blank: an unsynced indexer shows
  "Indexer is N blocks behind" with the numbers.
- Deep links everywhere; every table row is a link; every hash/address is
  copyable.
- Open Graph and Twitter cards per page (monochrome OG image with the value
  in large type, e.g. "Block 4,716").
- i18n structure ready but English only shipped.

## Out of scope — what not to build

- No wallet, no key generation, no signing, no "connect wallet", no
  broadcasting — this site is read-only. Say so in the footer.
- No token price, market cap, exchange links or "buy" buttons.
- No user accounts, cookies beyond the theme preference, or analytics.
- No fake liveness: if the SSE stream is down, show it; do not animate stale
  data.
- Do not call the node's RPC/REST directly from the browser; only
  `api.hashgram.io`.

## Definition of done

1. `pnpm build` and `pnpm test` pass; Lighthouse: performance ≥ 95,
   accessibility 100, best practices 100, SEO 100 on `/`, `/blocks`, `/docs`.
2. The Playwright colour test finds no saturated pixel on any route in either
   theme.
3. With the API live, `/` shows a new block within 5 s of it being produced,
   without a page reload, and the founder page shows accrued revenue > 0.
4. `/accounts` lists the useful-service reserve as the largest holder with
   the correct label, and the Founder with its vesting split.
5. `/rewards` renders the emission schedule and today's per-provider cap
   from live parameters, and states that there is no mining.
6. `/docs` renders all listed files with working internal links, code copy,
   and search.
7. A README explains: environment variables, how docs are synced from the
   Hashgram repository (a script that copies `docs/*.md` and records the
   commit hash), how to run against a local API, and the deploy steps with
   the Caddy block.
