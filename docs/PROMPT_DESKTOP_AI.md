# COPY-PASTE PROMPT — Hashgram desktop application

Hand this entire file to a coding agent that has the Hashgram repository
open. Do not summarise it. Do not invent endpoints. If a capability is
marked missing, leave it out.

---

You are building the Hashgram desktop client in this repository.

## What Hashgram is

A Cosmos SDK chain (`hashgram-1` after Mainnet launch) with a fixed
1,000,000,000 HASH supply, plus an off-chain libp2p network for MLS
messaging, signed social events, content-addressed media, and call
signalling. There is no mint module. Transfers are not taxed. The Founder
receives 100 basis points (1%) of **protocol fee revenue only**
(gas, username fees, module service fees), paid to the genesis beneficiary.

## Stack you will use

**Tauri 2 + Rust + the in-repo `hashgram-sdk`.**

Do not use Electron. Do not start from WinUI/C# — UniFFI and C-ABI
bindings do not exist yet. The SDK is already Rust
(`sdk/rust/hashgram-sdk`). `node/hashgram-client` is the reference CLI
that already exercises every SDK call. Copy its commands, do not
reimplement MLS, the handshake, or the blob protocol.

Put the app in `apps/desktop/`. Add it to the existing Rust workspace
in `node/Cargo.toml` or give it its own workspace that path-depends on
`sdk/rust/hashgram-sdk`, `node/hashgram-chain`, `node/hashgram-identity`.

UI: Tauri webview with a small Rust backend. Frontend may be vanilla
HTML/CSS/JS or a lightweight framework. No analytics. No crash reporter
that can see keys.

## Two stages (do not skip ahead)

### Stage 1 — ship this first

Wallet, identity, staking, usernames, Founder transparency, supply.
Talks to the chain over REST/RPC only.

### Stage 2 — only after Stage 1 works

Messenger, feed, media, calls. Drive `hashgram-sdk` from the Rust
backend. There is **no** REST path such as `/hashgram/messaging/v1/send`.
If you build a chat screen against HTTP, you have invented an API and
the build is wrong.

Still out of scope after Stage 2: WebRTC media stack (bring webrtc-rs
yourself; the SDK only gives TURN credentials and a signalling channel),
push notifications, SFU group-call E2EE, a token bridge, an in-app exchange.

## Network endpoints

On a correctly configured node these bind **localhost**.
`hashgramctl mainnet-preflight` fails the launch if admin RPC is public.

| Service | Port | Default |
| --- | --- | --- |
| CometBFT RPC | 26657 | `http://127.0.0.1:26657` |
| Cosmos / Hashgram REST | 1317 | `http://127.0.0.1:1317` |
| Cosmos gRPC | 9091 | localhost |
| Hashgram P2P (clients) | 26670 | `/ip4/<host>/udp/26670/quic-v1/p2p/<peer-id>` |

Genesis VPS public IP today: `186.241.19.230`.
CometBFT node id: `2f6629254d6568e48a09aff20ba01a61a26c47f0`.
P2P seed: `2f6629254d6568e48a09aff20ba01a61a26c47f0@186.241.19.230:26656`.
Chain id after launch: `hashgram-1`.
Genesis hash: read `/etc/hashgram/network.json` on the server **after**
`finalize-genesis`. Never hash the RPC `/genesis` response — CometBFT
re-serialises it and the SHA-256 will not match the file.

The desktop must support all three:

1. Localhost (user runs a node, or an SSH tunnel
   `ssh -N -L 26657:127.0.0.1:26657 -L 1317:127.0.0.1:1317 root@HOST`)
2. A configurable HTTPS RPC list the user pastes
3. Never a single hard-coded hostname as the only option

Show a health indicator per endpoint. A DEVNET profile must be visually
unmistakable (coloured banner "DEVNET").

## Signing and keys

- Bech32 prefix `hash`. Coin type **118**. Path `m/44'/118'/0'/0/0`.
- `SIGN_MODE_DIRECT`, secp256k1.
- Amounts are strings in JSON. Parse as integer / `u128`, never `f64`.
- Refetch account sequence before every transaction. Never cache it.
- Validate `hash` prefix and Bech32 checksum before enabling Send.

**Never transmit, log, or write plaintext keys or the mnemonic.**
Encrypt the vault with Argon2id (64 MiB, 3 iterations, 4 lanes) the same
way `hashgram-identity` already does — prefer calling `Vault` in
`hashgram-sdk` over rolling your own. On Windows, an extra DPAPI layer
is welcome; on Linux/macOS the vault file plus passphrase is enough.

The mnemonic is shown **once**. Require the user to re-enter three words
at random positions. On restore, show the derived address and ask them
to confirm it matches what they recorded. BIP-39 checksum does not catch
swapped words.

Never offer email, cloud backup, print, or clipboard copy of the mnemonic.
Clipboard copy of an address is fine. Clear the clipboard after a timeout
if you copy anything sensitive.

## Stage 1 screens

Implement exactly these. Endpoints are complete; they were extracted
from `proto/hashgram/*/v1/query.proto` and `tx.proto`.
`docs/CLIENT_CONNECTIVITY_SPEC.md` is the reference. `scripts/dev/check-docs.sh`
fails CI if a documented path does not exist.

### Onboarding

Create wallet / restore mnemonic / later: Ledger (coin type 118).

### Wallet

- Balance in HASH, uhash on hover
- Spendable vs vesting from `/cosmos/auth/v1beta1/accounts/{address}`
  plus next unlock date
- Send: `MsgSend`. Confirmation screen states that transfers are **untaxed**
  (100 HASH sent = 100 HASH received; fee is separate; 1% of the fee, not
  the principal, goes to the Founder)
- `@username` resolves via `GET /hashgram/username/v1/lookup/{name}`
- History: `/cosmos/tx/v1beta1/txs`
- Confusable-name warning via `/hashgram/username/v1/availability/{name}`

### Staking

`/cosmos/staking/v1beta1/validators`, delegate / undelegate / redelegate,
rewards. Show the **21-day unbonding period before confirm**.
Slashing: 5% double-sign, 0.01% downtime.

### Identity

`MsgCreateIdentity`, list/add/revoke devices, recovery with mandatory
delay and a cancel action. Only public keys go on chain. Say so in the UI.

### Usernames

Register / renew / transfer / release. Show expiry and grace period.

### Founder verification (every user, not an admin panel)

- `GET /hashgram/founder/v1/params` — must show 100 bps and ceiling 100
- `GET /hashgram/founder/v1/revenue` — accrued, paid, pending
- `GET /hashgram/founder/v1/beneficiary_history`
- Realised share = `10000 * founder_share / total_qualifying` from
  `GET /hashgram/feerouter/v1/totals`
- State that the share is on protocol fees, never on transferred principal

### Supply

- `GET /cosmos/bank/v1beta1/supply/by_denom?denom=uhash`
- Ceiling 1,000,000,000 HASH
- Say there is **no mint module**
- Genesis table: Founder 20%, service reserve 50%, treasury 15%,
  growth 5%, grants 5%, liquidity 5%
- `GET /hashgram/serviceproof/v1/reserve` and
  `/hashgram/serviceproof/v1/emission_schedule`

### Node console (optional)

Only if a local node is configured. Status from `/status`, peers from
`/net_info`. Do not shell out to `hashgramctl start`.

## Complete Hashgram REST list (39)

Network: `/hashgram/network/v1/info`, `/fork_isolation`,
`/signing_domain/{purpose}`.

Founder / fees: `/hashgram/founder/v1/params`, `/revenue`,
`/beneficiary_history`; `/hashgram/feerouter/v1/params`, `/totals`,
`/service_revenue`.

Treasury: `/hashgram/treasury/v1/reserves`, `/reserve/{name}`,
`/disbursements`. `{name}` is `treasury` | `growth` | `dev_grants` | `liquidity`.

Usernames: `/hashgram/username/v1/params`, `/lookup/{name}`,
`/reverse/{owner}`, `/availability/{name}`, `/registrations`.

Identity: `/hashgram/identity/v1/identity/{address}`, `/devices/{address}`,
`/device/{address}/{device_id}`, `/resolve_device_key`,
`/recovery/{root_address}`, `/identities`.

Welcome: `/hashgram/welcome/v1/params`, `/status`, `/tiers`,
`/claim/{subject}`, `/claims`. (Welcome is disabled until an attestor
is registered. Creating a key earns nothing.)

Service rewards: `/hashgram/serviceproof/v1/params`, `/reserve`,
`/provider/{operator}`, `/providers`, `/epoch/current`, `/epoch/{number}`,
`/rewards/{operator}`, `/assignments/{provider}`, `/challenges/{provider}`,
`/fraud/{provider}`, `/emission_schedule`.

Plus standard Cosmos bank / auth / staking / distribution / gov / tx
endpoints listed in `docs/CLIENT_CONNECTIVITY_SPEC.md`.

## Stage 2 — map each screen to an existing CLI command

Read `node/hashgram-client/src/main.rs` and call the same SDK functions.

| Screen | SDK | CLI to copy |
| --- | --- | --- |
| Devices | `account` + identity txs | `identity add-device` / `remove-device` |
| Publish key packages | `messaging::publish_key_packages` | `identity publish-keys` |
| Direct / group chat | `messaging::send` / `receive` / group | `message send` / `receive`, `group create` |
| Feed / profile / follow / post | `social::*` | `post`, `comment`, `react`, `follow`, `profile set` |
| Reels / stories | `blob::upload` then social | `reel publish`, `story publish` |
| Channels | `social::channel_*` | `channel create`, `channel post` |
| Media | `blob::download`, `blob::verify` | `blob download`, `blob verify` |
| Calls | `calls::discover`, TURN, signal | `call discover`, `call turn-creds` |

Rules Stage 2 must keep:

- Vault passphrase never leaves the process
- Plaintext messages never written outside the encrypted vault
- The app verifies every social event signature itself; indexer is a cache
- A peer whose handshake fails the genesis-hash check is surfaced as
  "wrong network", not retried silently

## Traps that have already wasted time

- SHA-256 of RPC genesis ≠ SHA-256 of `genesis.json`. Verify chain id,
  or hash a file the user downloaded.
- `cometbft_p2p_peers` is absent until the first peer event. Absent ≠ zero.
- No missed-blocks metric. Derive signing health from
  (chain height − last signed height).
- Amounts are strings. `uhash` reaches 1e15.
- A `cosmos1…` address is not a Hashgram address.

## Definition of done

Stage 1:

- [ ] Create, restore (and Ledger when you get to it) onboarding
- [ ] Three random words re-entered before a new mnemonic is accepted
- [ ] Restore shows the derived address for comparison
- [ ] Keys in the existing Argon2id vault; never logged or transmitted
- [ ] Balance shows spendable vs vesting and the next unlock
- [ ] Send validates `hash` + Bech32
- [ ] `@username` resolves
- [ ] Confirmation screen states transfers are untaxed
- [ ] Staking shows 21-day unbonding **before** confirm
- [ ] Devices list / authorise / revoke
- [ ] Recovery shows delay countdown and cancel
- [ ] Founder screen shows configured **and realised** bps
- [ ] Supply screen states there is no mint module
- [ ] All amounts integer, never float
- [ ] Sequence refetched before every tx
- [ ] DEVNET build unmistakable
- [ ] A test greps logs for the test mnemonic and fails if found
- [ ] No messaging / social / media screens in Stage 1

Stage 2:

- [ ] Every screen above is an SDK call, not an invented REST path
- [ ] A test asserts no plaintext message appears in any file the app
      writes outside the vault

## Where to look

| What | Where |
| --- | --- |
| This connectivity spec | `docs/CLIENT_CONNECTIVITY_SPEC.md` |
| Wallet / identity / founder screens | `docs/PROMPT_WINDOWS_DESKTOP.md` |
| Messaging | `docs/MESSAGING.md` |
| Social | `docs/SOCIAL_PROTOCOL.md` |
| Storage | `docs/STORAGE.md` |
| Calls | `docs/CALLS.md` |
| SDK | `sdk/rust/hashgram-sdk/` |
| Working network client | `node/hashgram-client/src/main.rs` |
| Working chain client | `cmd/hashgram-test-client/` |
| Signing vectors (90) | `node/testdata/signing-vectors.json` |
| Params, prefix, coin type | `app/params/params.go` |
| Owner launch state | `docs/OWNER_LAUNCH_KA.md` |

Start with Stage 1. Open a PR-quality tree under `apps/desktop/`.
Do not generate a Founder mnemonic. Do not phone home.
After Stage 1 runs against localhost (or the SSH tunnel), stop and
show the Founder 1% screen with live numbers from the chain.
