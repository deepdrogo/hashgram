> **Superseded (2026-09-12).** This is the messenger-era prompt kept for the
> record. The Hashgram One desktop application is specified in
> [DESKTOP_APP_MASTER_PROMPT.md](DESKTOP_APP_MASTER_PROMPT.md); use that.

# COPY-PASTE PROMPT — Hashgram for Windows, the complete application

Hand this entire file to a coding agent that has the Hashgram repository
open. Do not summarise it. Do not invent endpoints or protocols. If a
capability is marked missing or out of scope, leave it out and say so in
the UI. `scripts/dev/check-docs.sh` fails CI when a prompt names a path
that does not exist; every path below does.

---

## Step 0 — get the code (there is no git remote yet)

The Hashgram repository lives on the owner's server. **Do not ask for SSH
access to that server**: it is the Mainnet validator, and nothing that runs
on a developer laptop may hold a key to it. The owner hands you a
`git bundle` (full history, no secrets — every key file is git-ignored and
absent from history) plus its SHA-256 and the expected HEAD commit. Then:

```powershell
# in the folder where the project should live, e.g. A:\hashgram
curl.exe -O <URL>/hashgram.bundle
curl.exe -O <URL>/hashgram.bundle.sha256
curl.exe -O <URL>/HEAD.txt
(Get-FileHash hashgram.bundle -Algorithm SHA256).Hash.ToLower()   # must equal the first field of hashgram.bundle.sha256
git bundle verify hashgram.bundle
git clone hashgram.bundle hashgram
cd hashgram
git rev-parse HEAD          # must equal HEAD.txt
git remote remove origin    # the bundle is not a remote; a real one is added later
```

Work on a branch (`git switch -c desktop`). When the owner later creates a
remote, `git remote add origin <url>` and push — history is preserved
because the bundle carried it. Until then, hand changes back the same way
in reverse: `git bundle create desktop.bundle main..desktop` and give the
file to the owner.

Toolchain on Windows: Rust ≥ 1.90 (`x86_64-pc-windows-msvc`, MSVC Build
Tools with the C++ workload), Node 22, `corepack enable` (pnpm), Tauri 2
prerequisites (WebView2 is present on Windows 11), NSIS is fetched by the
Tauri bundler. Protobuf needs **no** `protoc` — the Rust build uses `protox`
(pure Rust, `node/hashgram-proto/build.rs`). Go is **not** required for this
application (Stage 0 changes Rust crates only); `scripts/dev/check-docs.sh`
needs Git Bash and Go — run it on the server or skip it locally and say so.

First commands that must succeed before anything else:

```powershell
cd node;  cargo build -p hashgram-client -p hashgram-node;  cargo test -p hashgram-net -p hashgram-sdk
```

Then, to confirm the network is reachable from this laptop with **no**
server address typed by anyone (the seed list is compiled in):

```powershell
$env:HASHGRAM_PASSPHRASE = "throwaway-for-this-check"
.\target\debug\hashgram-client.exe configure --network mainnet --genesis-hash e322bc2319f6e0173286fa526dab5a8ff8ad0797c7b80dd03e7c9d98621d5e4d
.\target\debug\hashgram-client.exe net peers
```

Expected: at least one line `12D3Koo…  <roles>` within 10 s. The client
already falls back to `hashgram_net::mainnet_bootstrap_peers()` when a
Mainnet profile has no `--bootstrap` (see `link()` in
`node/hashgram-client/src/main.rs`) — the desktop app uses the same rule.
If nothing appears, the laptop's firewall is blocking outbound UDP 26670
(QUIC); TCP 26670 is the fallback and must be allowed too.

---

You are building **Hashgram for Windows**: one native, fast, beautiful
application that is the user's wallet, messenger, social network (feed,
reels, stories, channels), calls, identity, and — if they choose — their
node and its earnings dashboard. English UI. It talks **directly to the
peer-to-peer network of Hashgram nodes**, never to one server, so it keeps
working when any particular machine — including the genesis server —
disappears.

## What Hashgram is (facts you build on)

- Cosmos SDK v0.53 / CometBFT Layer-1, chain-id `hashgram-1`, launched
  2026-09-10. Genesis SHA-256
  `e322bc2319f6e0173286fa526dab5a8ff8ad0797c7b80dd03e7c9d98621d5e4d` —
  a compile-time constant (`app/params/mainnet.go`,
  `node/hashgram-net/src/mainnet.rs`, genesis file embedded). The app hardcodes
  it and refuses any node whose handshake reports a different hash.
- Fixed supply 1,000,000,000 HASH (`uhash`, 6 decimals). **No mint module.
  No mining.** Transfers are untaxed. The Founder receives 1 % (100 bps,
  hardcoded ceiling) of **protocol fee revenue only**.
- Rewards exist for **useful service**: nodes that store, relay and serve
  bytes (roles `store`, `relay`, `media`, `bootstrap`, `call`) earn from a
  500,000,000 HASH reserve, budget per epoch (21,600 blocks ≈ 1 day)
  `min(remaining × 5/10,000, 250,000 HASH)`, at most 5 % per provider, bond
  1,000 HASH. Anything in the UI that says "mining" is wrong; say "earn by
  running a node".
- Off-chain layer over libp2p (QUIC/TCP on 26670): MLS end-to-end encrypted
  messaging with store-and-forward mailboxes, signed public social events,
  content-addressed media (blobs), call discovery/signalling with TURN.
- Bech32 prefix `hash`, coin type 118, path `m/44'/118'/0'/0/0`,
  secp256k1, `SIGN_MODE_DIRECT`. Amounts are strings; parse as integers.

## Stack — decided, not negotiable

**Tauri 2 + Rust + the in-repo `hashgram-sdk`** (`sdk/rust/hashgram-sdk`),
frontend in TypeScript (Svelte 5 or SolidJS — pick one, no React; virtualised
lists everywhere), WebView2 for rendering, Rust for everything that is not
pixels: networking, crypto, MLS, media hashing and chunking, database
(SQLite via `rusqlite`, WAL, encrypted at rest with the vault key), image
decoding/resizing, video thumbnailing.

Why not Electron: 150 MB, two runtimes, slow start. Why not WinUI/C#:
UniFFI/C-ABI bindings for the SDK do not exist; the SDK is Rust; the
reference client `node/hashgram-client` already exercises every SDK call
— copy its command handlers, do not reimplement MLS, the handshake or the
blob protocol.

Project layout: `apps/desktop/` with `src-tauri/` (Rust) and `src/`
(frontend); add the crate to the workspace in `node/Cargo.toml` (or a
workspace that path-depends on `sdk/rust/hashgram-sdk`,
`node/hashgram-chain`, `node/hashgram-identity`, `node/hashgram-net`).

**Performance budgets (tests enforce them):** cold start to interactive
< 1.5 s on a 2020 laptop; installer < 40 MB; idle RAM < 150 MB; 60 fps
scrolling on a 10,000-item feed; a 100 MB reel uploads with UI responsive;
message send-to-ack < 300 ms on LAN. Profile with `tracing` spans; ship a
hidden "Performance" panel (Ctrl+Shift+P) that shows them.

## Connectivity — "talks to nodes, not to a server"

There are two things a client needs from the network, and they travel
differently today:

1. **P2P layer** (messaging, social, media, calls, node discovery):
   `hashgram_sdk::link::Link` — a light libp2p swarm. It dials bootstrap
   peers, completes the Hashgram handshake (network id, chain id, magic
   `HGM1`, protocol major version, **genesis hash**), learns roles, and then
   uses Kademlia and `AnnounceQuery` to find more nodes. Bootstrap list:
   `hashgram_net::mainnet_bootstrap_peers()` (compiled in, same file the
   nodes use, `app/params/mainnet/bootstrap_peers.txt`) **plus** the
   peerstore the app persists (`%LOCALAPPDATA%\Hashgram\peers.json`), so
   after the first run the built-in list is only a fallback. This layer
   already works without any HTTP endpoint.

2. **Chain reads and transaction broadcast** (balances, staking, usernames,
   founder data, sending HASH): today the SDK's `ChainClient` needs an HTTP
   REST base (`chain_api`, e.g. `http://127.0.0.1:1317`), and nodes bind
   that to loopback. There is **no** chain-query protocol over libp2p yet
   (the P2P request protocol `hashgram/rpc/1`, with a leading slash, carries
   `AnnounceQuery`, `AttestationQuery`, `TurnCredentialRequest` and the
   messaging/blob/social requests only).

**Stage 0 of this build therefore adds that protocol — in this repository,
node and SDK — before the wallet screens are wired:**

- Node (`node/hashgram-node`, `node/hashgram-p2p`, `node/hashgram-proto`):
  new request/response pair `ChainQuery { path, query }` /
  `ChainQueryResponse { status, body, height }` on the existing request
  protocol, served by nodes with the `relay` or `bootstrap` role. The node
  forwards **only** an allow-list of read paths to its local REST
  (`/cosmos/bank/…`, `/cosmos/auth/…`, `/cosmos/staking/…`,
  `/cosmos/distribution/…`, `/cosmos/gov/…`, `/cosmos/tx/v1beta1/txs/{hash}`,
  `/hashgram/…`), plus `ChainBroadcast { tx_bytes }` →
  `POST /cosmos/tx/v1beta1/txs` (sync mode). Per-peer rate limits, 64 KB
  response cap, 5 s timeout, metrics. Same allow-list the local API's
  `/v1/chain/{*path}` passthrough uses — reuse it. Add `ServiceRole` credit?
  **No**: chain relaying is not rewarded (say so in `docs/SERVICE_REWARDS.md`
  "Known limitations"); it is a public good like PEX.
- SDK (`sdk/rust/hashgram-sdk`): `link.chain_get(path)` and
  `link.chain_broadcast(tx)`; `ChainClient` gains a transport enum
  `{ Http(url), P2p(Link) }`. **Cross-check**: every read is issued to two
  peers of different operators (operator address from the handshake /
  `link.operator_of`) and compared byte-for-byte after JSON normalisation;
  a mismatch marks both peers "disputed", asks a third, and the UI shows
  "verified by 2 nodes" or a warning. Heights must be within 3 blocks.
  This is not a light client (Merkle proofs are a later milestone, note it
  in `docs/CLIENT_CONNECTIVITY_SPEC.md` §10 "What still does not exist"),
  but it is what makes "no single server" true for the wallet.
- Tests: node-side allow-list denies `/cosmos/tx/v1beta1/simulate`,
  anything with `..`, anything not on the list; SDK-side cross-check test
  with a lying mock peer; `scripts/dev/check-docs.sh` and `make test` pass;
  update `docs/PROTOCOL.md` and `docs/CLIENT_CONNECTIVITY_SPEC.md`.

**Endpoint precedence in the app**, in order, each with a live health dot:
1. A node on this PC (the app can install one — see "Earn"), `127.0.0.1`.
2. The P2P chain relay above, across ≥ 2 nodes.
3. HTTPS REST endpoints the user pasted (Settings → Network). Never a
   single hardcoded hostname as the only option.

A **DEVNET** profile is visually unmistakable (persistent banner).

**"Connected nodes" panel** (status bar click): every peer from
`link.peers()` — peer id, moniker/operator, roles, latency, transport
(QUIC/TCP/relayed), region guess is **not** shown (no IP geolocation), which
nodes served the last chain reads and whether they agreed, own NAT status
(autonat), and which discovery layer found each peer (built-in list /
peerstore / DHT / DNS). A "wrong network" peer (handshake genesis mismatch)
is listed greyed with the reason, never retried silently.

## Accounts: 24 words, and how people find each other — all on chain

There are **no server-side accounts**. Hashgram has no sign-up service, no
email, no phone number, no password database. An account **is** a key.

- **Creating an account = generating a 24-word mnemonic** (256-bit entropy;
  `hashgram_chain::Wallet::generate` already does exactly this — use it, do
  not offer 12 words). The address `hash1…` is derived from it
  (`m/44'/118'/0'/0/0`). This is the only way to create an account in the
  app; there is no other "register" path.
- **Logging in = restoring from the 24 words.** On a new machine the user
  types the 24 words → the app shows the derived address → user confirms →
  sets a local passphrase. Day-to-day unlock is the passphrase or Windows
  Hello; the mnemonic is never asked for again and never stored in plaintext
  (vault only). "Forgot passphrase" = restore from the 24 words; there is no
  reset by anyone else, and the UI says so on the create screen.
- **Identity on chain.** The first device registers with `MsgCreateIdentity`
  (root key = the wallet key, one device key for this PC). A second PC uses
  the same 24 words and `MsgAddDevice`; devices are listed and revocable
  (`MsgRevokeDevice`). Only public keys go on chain. Messaging (MLS) and
  social events are signed by device keys that resolve on chain
  (`/hashgram/identity/v1/devices/{address}`,
  `/hashgram/identity/v1/resolve_device_key`), which is what lets any peer
  verify who wrote what without a server.
- **Finding people — by public key first.** Every user is reachable by their
  address (`hash1…`), shown with a monochrome QR and a `hashgram://` link on
  the Receive/Profile screens. Search accepts a full address; pasting one
  opens the profile immediately (no network round-trip beyond loading the
  profile and devices from chain).
- **Optional `@username`, on chain, searchable.** From `x/username`:
  `MsgRegister` (fee 1 HASH, valid 7,884,000 blocks ≈ 1 year, grace 648,000
  ≈ 30 days, 3–32 chars, lowercase, reserved names refused by the chain —
  `/hashgram/username/v1/params`), `MsgRenew`, `MsgTransfer`, `MsgRelease`.
  Search resolves `@name` → address with `/hashgram/username/v1/lookup/{name}`
  and shows a name next to an address with
  `/hashgram/username/v1/reverse/{owner}`; availability and confusable
  warnings via `/hashgram/username/v1/availability/{name}`. Registration is
  offered as an optional step at onboarding and later in Wallet → Usernames.
- **Display names are not identity.** A social profile's display name is a
  signed social event anyone can set to anything. The UI always shows the
  verified `@username` (or the middle-truncated address when there is none)
  next to a display name, in the monospace face, on every message, post,
  comment and call screen — that is the anti-impersonation rule and a test
  checks it on every component that renders a person.
- **Search box (Ctrl+K)** therefore resolves, in this order: `hash1…`
  address → `@username` (chain lookup) → tx hash → `#hashtag` → channel.
  Never a fuzzy "people you may know" from a server; there is none.

## Keys and security (unchanged rules, enforced by tests)

- Vault: `hashgram_sdk::Vault` (Argon2id 64 MiB / 3 / 4 lanes) + Windows
  DPAPI wrapping of the vault key; optional Windows Hello unlock (stores the
  DPAPI-wrapped key behind the biometric prompt, never the mnemonic).
- Mnemonic shown once; three random words re-entered; restore shows the
  derived address for confirmation. No email, cloud, print or clipboard for
  the mnemonic. Clipboard of an address is fine; sensitive copies clear
  after 30 s.
- Nothing plaintext (keys, messages) is written outside the encrypted SQLite
  and vault. A test scans every file the app writes for a test mnemonic and a
  test message and fails if found.
- Sequence refetched before every transaction. Bech32 + `hash` prefix
  validated before Send enables. `cosmos1…` is not a Hashgram address.
- No analytics, no crash reporter that can see memory, no phone-home. The
  updater is the only outbound HTTPS besides user-configured endpoints.
- The app verifies every social event signature and every blob hash itself;
  any indexer is a cache.

## Screens

Navigation: left rail — Home, Messages, Feed, Reels, Channels, Calls,
Wallet, Earn, Network, Settings; global search (Ctrl+K: @username, address,
tx hash, hashtag, channel). Window remembers size/position; system tray
with unread badge; native Windows notifications (toast) for messages and
calls; deep links `hashgram://` (address, username, post, channel).

### Onboarding (first run, animated)
Splash with the monochrome mark animating into the wordmark (≤ 1.2 s,
skippable, respects reduced-motion). Then: Create wallet / Restore / (later)
Ledger. Create → mnemonic once → three-word check → set passphrase → optional
Windows Hello → optional `@username` (explains fee and expiry) → optional
profile (display name, avatar → blob upload) → "Connecting to the network…"
with the real peer list appearing as handshakes complete. Restore → mnemonic
→ derived address shown → confirm → passphrase. A **device** key is created
and registered (`MsgCreateIdentity` for a new identity, `MsgAddDevice` for
an existing one); explain that only public keys go on chain.

### Home
Balance (HASH, uhash on hover), spendable vs vesting, last messages, feed
highlights, network status (nodes connected / verified reads), current epoch
and — if this PC runs a node — today's credit.

### Wallet
- Receive: address, QR (monochrome), `@username` if owned, "share" copies a
  `hashgram://` link.
- Send: recipient (address or `@username` via
  `GET /hashgram/username/v1/lookup/{name}`, confusable warning via
  `/hashgram/username/v1/availability/{name}`), amount, memo, fee preview;
  confirmation states transfers are **untaxed** — 100 HASH sent = 100 HASH
  received; 1 % of the *fee*, not the principal, goes to the Founder.
  `MsgSend`, broadcast via precedence above, then track by
  `/cosmos/tx/v1beta1/txs/{hash}` until committed.
- History: `/cosmos/tx/v1beta1/txs` by sender/recipient events, local cache
  in SQLite, filters, export CSV.
- Vesting: schedule from `/cosmos/auth/v1beta1/accounts/{address}`, next
  unlock date.
- Staking: validators (`/cosmos/staking/v1beta1/validators`), delegate /
  undelegate / redelegate (`MsgDelegate`, `MsgUndelegate`,
  `MsgBeginRedelegate`), rewards + withdraw (`MsgWithdrawDelegatorReward`);
  the **21-day unbonding** and slashing (5 % double-sign, 0.01 % downtime)
  are shown **before** confirm.
- Governance: proposals from `/cosmos/gov/v1/proposals`, tally, vote
  (`MsgVote`) with the 7-day period, quorum 40 %, threshold 50 %, veto 33.4 %.
- Usernames: register / renew / transfer / release (`MsgRegister`,
  `MsgRenew`, `MsgTransfer`, `MsgRelease`); expiry and grace shown.
- Identity & devices: list / add / revoke (`MsgAddDevice`,
  `MsgRevokeDevice`), recovery config and flow (`MsgInitiateRecovery`,
  `MsgCancelRecovery`) with the mandatory delay countdown and a cancel.

### Messages (MLS, `hashgram_sdk::messaging`)
Direct and group chats; key packages published on first run
(`messaging::publish_key_packages`); text, replies, reactions, attachments
(blobs, encrypted, chunked, resumable), voice notes, read/delivery state as
the protocol provides; typing indicators only if the protocol has them
(it does not — leave out); search over local plaintext (in the encrypted
DB only); disappearing messages if MLS layer supports the timer (check
`docs/MESSAGING.md`; if not, leave out). Multi-device: messages arrive on
every registered device via mailboxes; show "sent from device X".
Store-and-forward means offline recipients get messages when they return;
show which `store` nodes hold the mailbox (from the SDK) in chat info.

### Feed (`hashgram_sdk::social`)
Chronological following feed, author profiles, follow/unfollow, posts
(text + media via blob), comments, reactions, reposts, hashtags, mentions,
mute/block (local), report (safety attestations
`/v1/safety/attestations/{subject}` are readable on a node's local API only
— in the app, read safety verdicts through the P2P `AttestationQuery`).
Every event is signature-verified against on-chain devices before display;
unverifiable events are hidden with a count.

### Reels and Stories
Vertical full-screen reels (`reel publish` flow: transcode to H.264/AAC in
Rust via `ffmpeg` bindings or accept pre-encoded MP4 ≤ 100 MB, chunk, upload
to `store`/`media` providers, publish the social event), autoplay, mute,
like/comment/share, author reels tab; stories (24 h) ring at the top of
Feed. Downloads are hash-verified per chunk; a bad chunk drops that
provider and retries another.

### Channels
Create/join/post/moderate broadcast channels (`social::channel_*`),
subscriber count, pinned posts, channel search.

### Calls (`hashgram_sdk::calls`)
Discover call nodes (`calls::discover`), fetch TURN credentials, exchange
signalling over the SDK channel, media via `webrtc-rs` (you bring it; the
SDK gives TURN + signalling only). 1:1 audio/video, screen share; group
calls only if an SFU is announced — otherwise the button is disabled with
"no SFU node available". E2EE for 1:1 (DTLS-SRTP + the MLS-derived key
where the protocol specifies); group SFU E2EE is **not** available — say so.

### Earn (this replaces any "mining" idea)
- **Network view** (everyone): the reserve, this epoch's budget and
  per-provider cap, provider count, emission chart, "what earns credit"
  table, from `/hashgram/serviceproof/v1/params`, `/hashgram/serviceproof/v1/reserve`,
  `/hashgram/serviceproof/v1/emission_schedule`, `/hashgram/serviceproof/v1/epoch/current`,
  `/hashgram/serviceproof/v1/providers`. Plain sentence: "There is no
  mining. Nodes earn for storing and serving real bytes; the budget is a
  ceiling, not a guarantee."
- **Run a node on this PC** (Stage 3): bundle `hashgram-node.exe` (cross-
  compile the Rust node for `x86_64-pc-windows-msvc`; it already has autonat,
  hole punching and circuit relay) as a Windows service managed by the app:
  choose roles (`store`, `media`, `relay`), disk quota, bandwidth cap,
  reward address (default: **a separate cold address the user writes down**,
  not the hot wallet), bond 1,000 HASH (`MsgRegisterProvider` — explain the
  bond, slashing 5 % on fraud, 21-day `MsgBeginUnbonding` /
  `MsgWithdrawBond`), then live status: reachability (autonat), assignments
  (`/hashgram/serviceproof/v1/assignments/{provider}`), challenges passed
  (`/hashgram/serviceproof/v1/challenges/{provider}`), fraud score
  (`/hashgram/serviceproof/v1/fraud/{provider}`), credit this epoch and
  lifetime paid (`/hashgram/serviceproof/v1/rewards/{operator}`), payout
  history to the reward address. The node's operator key lives in the same
  vault. If the PC is behind a NAT that hole punching cannot cross, say so
  and explain the earnings impact honestly.
- **Welcome reward** status from `/hashgram/welcome/v1/status`,
  `/hashgram/welcome/v1/tiers`, `/hashgram/welcome/v1/claim/{subject}`;
  disabled until an attestor exists — show that state, never a fake "claim".

### Founder transparency (every user sees it)
`/hashgram/founder/v1/params` (100 bps, ceiling 100),
`/hashgram/founder/v1/revenue` (accrued / paid / pending),
`/hashgram/founder/v1/beneficiary_history`; realised share
`10000 × founder_share / total_qualifying` from `/hashgram/feerouter/v1/totals`;
the vesting schedule of the beneficiary (199,000,000 HASH: 19,000,000
spendable, 180,000,000 over 96 months) from auth; its delegations. State
that the share is on protocol fees, never on transferred principal.

### Supply
`/cosmos/bank/v1beta1/supply/by_denom?denom=uhash`, ceiling 1,000,000,000,
"no mint module", genesis table (Founder 20 %, service reserve 50 %,
treasury 15 %, growth 5 %, grants 5 %, liquidity 5 %),
`/hashgram/treasury/v1/reserves`.

### Network
The "Connected nodes" panel in full, plus: chain height and block time from
the relayed reads, genesis pin check (green ✓ with the hash), protocol
facts from `/hashgram/network/v1/info` and `/hashgram/network/v1/fork_isolation`,
built-in seed list, peerstore size, "forget peers" button, and a
diagnostics export (no IPs of other peers in the export).

### Settings
Network (endpoint precedence, HTTPS list, DEVNET profile toggle with the
banner), Security (passphrase change, Windows Hello, auto-lock timer,
clipboard clearing), Devices, Notifications, Media (autoplay, download
quality, cache size, storage location), Node (if installed: roles, quota,
bandwidth, start with Windows), Appearance (monochrome brand chrome like
hashgram.io — black/white UI, user content in colour; dark/light; reduced
motion), Updates (channel, check now, signature key fingerprint), Language
(English only shipped, i18n-ready), Advanced (log level, export logs with
secrets redacted), About (version, commit, licences).

### In-app Help
Bundled Markdown rendered offline: Quick start · Your keys and what they
control · Sending and fees (untaxed transfers, the 1 %) · Messages and
devices · Feed, reels, channels · Calls · Earn by running a node (honest
expectations) · Staking and governance · Network and nodes · Troubleshooting
(NAT, wrong network, out of sync) · FAQ · Privacy. Each page links to the
matching file in `docs/` of this repository by commit.

## Installer and updates

- Tauri bundler → **NSIS** installer, per-user by default (no UAC), with a
  branded, animated first page (monochrome mark), progress with real file
  names, "Launch Hashgram" at the end; `/S` silent flag; MSI for enterprise
  as a second artefact. Installer < 40 MB.
- Windows code signing (EV or OV certificate the owner provides; without it
  SmartScreen warns — say so in the release notes). Reproducible build
  script `apps/desktop/release.ps1` printing SHA-256 of every artefact.
- Auto-update via Tauri updater: manifest on `hashgram.io/desktop/latest.json`
  signed with a **minisign key kept offline** (the public key is compiled
  into the app; the private key is never on a server); delta not required;
  update never runs while a transaction is pending; release notes shown
  before restart.
- Uninstall asks whether to keep the vault and messages; default keep.
- Start with Windows (optional), single instance, `hashgram://` protocol
  registration, file association none.

## Motion and design

Monochrome UI chrome consistent with hashgram.io (black/white, opacity
greys; content — photos, videos, avatars — in colour). One variable sans,
one monospace for hashes and amounts (tabular figures, middle-truncated
hashes with copy). Motion: purposeful and short (page transitions ≤ 200 ms,
list items 60 fps, skeletons instead of spinners), `prefers-reduced-motion`
honoured. Keyboard-first: every action reachable; Windows accessibility
(UIA names on all controls, high-contrast mode respected).

## Stages — build in this order, ship each

- **Stage 0** — P2P chain relay (node + SDK) with cross-checking. Done when
  `hashgram-client wallet balance` works with **no** `chain_api` configured,
  through two nodes.
- **Stage 1** — App shell, onboarding, vault, Wallet (send/receive/history/
  vesting), Staking, Governance, Usernames, Identity, Founder, Supply,
  Network panel, Settings, Help, installer + updater. Done when a fresh
  Windows 11 VM installs the app, restores a wallet, sees a live balance
  "verified by 2 nodes", sends HASH, and the Founder screen shows live
  numbers — with no HTTPS endpoint configured.
- **Stage 2** — Messages, Feed, Reels, Stories, Channels, Calls (1:1).
  Done when two fresh installs on two networks exchange an encrypted
  message and a reel through public nodes only.
- **Stage 3** — Earn: bundled node service, provider registration, earnings
  dashboard. Done when a home PC registers as a `media`/`store` provider
  from the app, shows its assignments and challenge results, and a payout
  reaches the cold reward address.

## Out of scope — what not to build

- No mining, hashing, "GPU" or "CPU" earning of any kind; no "mine" wording.
- No REST paths for messaging or social (there is **no**
  `/hashgram/messaging/v1/send`; those live in the SDK over P2P).
- No custodial features, no in-app exchange, price, fiat, token bridge.
- No email/cloud/print backup of the mnemonic; no clipboard for it.
- No analytics, telemetry, crash uploads, third-party CDNs or fonts.
- No group-call E2EE against the SFU operator (not available — say so).
- No push notifications (poll/stay connected); no iOS/Android here.
- No hardcoded single node/IP/hostname as the only way in; no dependence on
  the genesis server (`grep -rn 186.241 apps/` must be empty).
- No Merkle-proof light client in this build (cross-checking is the
  interim; document it as such).

## Traps that have already wasted time

- SHA-256 of RPC `/genesis` ≠ SHA-256 of `genesis.json`; use the handshake's
  genesis hash and the compiled-in constant.
- Amounts are strings; `uhash` reaches 1e15 — `u128`, never `f64`.
- Account sequence: refetch before every tx; sync broadcast can return
  before commit — poll the tx hash.
- Delegating from a vesting account costs ~520k gas, not 250k: simulate or
  use generous limits with `--gas-adjustment`.
- `cometbft_p2p_peers` metric absent ≠ zero peers.
- Windows: long paths, `%LOCALAPPDATA%` not roaming, file locks on SQLite
  (WAL + busy timeout), QUIC needs UDP allowed in Windows Firewall — the
  installer adds the rule for the app binary only.

## Definition of done (all stages)

- [ ] Stage 0 protocol merged with tests; docs updated; `check-docs.sh` passes
- [ ] Fresh Windows 11 VM: install (< 40 MB, signed or SmartScreen note),
      first-run animation, create wallet, three-word check, Windows Hello
- [ ] Account creation is 24 words only; restore on a second VM with the same
      words yields the same address and `MsgAddDevice` registers the device
- [ ] Search finds a user by `hash1…` address and by `@username` from chain;
      a display name never appears without the verified handle/address
- [ ] Balance and history "verified by 2 nodes" with no HTTPS endpoint set
- [ ] Send HASH; confirmation says untaxed; tx tracked to commit
- [ ] Staking shows 21-day unbonding before confirm; vote on a proposal
- [ ] Founder screen shows configured and realised bps, live
- [ ] Encrypted DM and group message between two installs via public nodes
- [ ] Post, comment, react, follow; a reel uploaded and played elsewhere
- [ ] 1:1 call through TURN; group call button disabled with honest reason
- [ ] Earn: node installed as a service, provider registered, assignments
      and challenges visible, payout to cold address
- [ ] Connected-nodes panel shows peers, roles, which served reads, agreement
- [ ] Genesis mismatch peer shown as "wrong network", never retried silently
- [ ] Test: no plaintext key/message/mnemonic in any file outside vault/DB
- [ ] Test: performance budgets (cold start, RAM, 60 fps list)
- [ ] Test: no saturated colour in UI chrome (content excluded)
- [ ] Auto-update from a signed manifest; refuses unsigned
- [ ] `grep -rn 186.241 apps/` empty; DEVNET banner unmistakable

## Where to look

| What | Where |
| --- | --- |
| Connectivity spec, all endpoints | `docs/CLIENT_CONNECTIVITY_SPEC.md` |
| Windows screens spec (wallet/identity/founder) | `docs/PROMPT_WINDOWS_DESKTOP.md` |
| Messaging / Social / Storage / Calls | `docs/MESSAGING.md`, `docs/SOCIAL_PROTOCOL.md`, `docs/STORAGE.md`, `docs/CALLS.md` |
| Rewards, roles | `docs/SERVICE_REWARDS.md`, `docs/NODE_ROLES.md`, `docs/TOKENOMICS.md` |
| SDK | `sdk/rust/hashgram-sdk/` (`account`, `link`, `messaging`, `social`, `blob`, `calls`) |
| Working network client to copy | `node/hashgram-client/src/main.rs` |
| Node P2P request protocol | `node/hashgram-p2p/src/`, `node/hashgram-proto/`, the `p2p` folder under `proto/hashgram` |
| Node local API passthrough allow-list | `node/hashgram-node/src/api.rs` (`/v1/chain/{*path}`) |
| Built-in seeds / genesis | `app/params/mainnet/`, `node/hashgram-net/src/mainnet.rs` |
| Signing vectors (90) | `node/testdata/signing-vectors.json` |
| Params, prefix, coin type | `app/params/params.go` |

Start with Stage 0. Open a PR-quality tree. Do not generate a Founder
mnemonic. Do not phone home. After Stage 1 runs on a clean VM with no
HTTPS endpoint configured, stop and show the Wallet with "verified by 2
nodes" and the Founder screen with live numbers.
