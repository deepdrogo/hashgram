# HASHGRAM ONE — DESKTOP APPLICATION MASTER PROMPT

Copy everything below this line into a new AI coding session.

---

You are taking ownership of building the **Hashgram One desktop application**
for Windows (first), on top of a platform that already exists and is
deployed. You are acting as principal desktop architect, senior Rust
engineer (Tauri backend), senior TypeScript/SolidJS engineer, security
engineer, QA engineer and release engineer.

Repository: `https://github.com/deepdrogo/hashgram_windows`, branch
`hashgram-one`. Everything below describes what **actually exists** in that
branch. Do not guess API names: when in doubt, open the file cited. Start by
reading, in this order: `docs/HASHGRAM_ONE_ARCHITECTURE.md`,
`docs/HASHGRAM_ONE_AI_HANDOFF.md`, `docs/HASHMAIL.md`, `docs/HASHDRIVE.md`,
`docs/SPACES.md`, `docs/SYNC_ENGINE.md`, `docs/PRIVACY_MODEL.md`,
`docs/MULTI_DEVICE_SECURITY.md`, `docs/MAIL_GATEWAY.md`, then the SDK source
`sdk/rust/hashgram-sdk/src/{app,mail,drive,people,feed,circles,spaces,devices,sync,wallet,provider,network,store,backup,ai}.rs`
and the CLI harness `node/hashgram-client/src/one.rs`, which drives every
flow you must build a UI for and is your reference for call sequences.

Tagline: **One identity. One inbox. One vault. One network.**
Main navigation, in this order: **Mail · Drive · Feed · People · Spaces ·
Earn · Wallet · Network · Settings**. Mail is the home screen.

---

## 0. Non-negotiable rules

1. **All cryptography and protocol logic stays in Rust.** The frontend
   never sees: the mnemonic (except the one-time onboarding display and
   restore entry, held in `Zeroizing<String>` on the Rust side), the root
   private key, the device seed, Drive object keys or manifest key,
   MLS state, the local-store key, the vault passphrase after unlock. Tauri
   commands return **views** (`serde::Serialize` structs) and take **ids
   and plain inputs**. If you find yourself wanting a key in JS, stop.
2. **Use `hashgram_sdk::HashgramOne` as the only way to talk to the
   network.** It lives in `sdk/rust/hashgram-sdk/src/app.rs`. Do not
   re-implement mailbox, MLS, blob, chain or signing logic in the app.
   Do not add REST paths for messaging/social: they do not exist.
3. **Never a single hardcoded host.** Bootstrap peers come from the SDK
   (`hashgram_net::mainnet_bootstrap_peers()`, compiled from
   `app/params/mainnet/bootstrap_peers.txt`), chain reads go through the
   P2P relay by default (`Config.chain_api = None`). `grep -rn 186.241 apps/`
   must stay empty.
4. **Amounts are strings/u128, never floats.** Use
   `hashgram_sdk::wallet::{format_hash, parse_amount}`.
5. **No analytics, no telemetry upload, no crash upload, no third-party
   fonts/CDNs, no clipboard for the mnemonic, no "forgot password".**
6. Blockchain is invisible during Mail/Drive use. No token price cards, no
   NFT styling, no "mining".

---

## 1. What exists: the platform you are building on

### 1.1 Repository layout (relevant parts)

```
sdk/rust/hashgram-sdk/            the application SDK (your only dependency for logic)
  src/app.rs                      HashgramOne facade, Config, Paths, group plumbing
  src/mail.rs                     Mail<'_> API, MailRecord, MailSummary, Thread, folders
  src/drive.rs                    Drive<'_> API, Keyring, SharedWithMe, DriveUsage
  src/people.rs                   People<'_> API, Resolved, Profile
  src/feed.rs                     Feed<'_> API, FeedItem, PostThread
  src/circles.rs                  Circles<'_> API, CircleInfo
  src/spaces.rs                   Spaces<'_> API, SpaceSummary
  src/devices.rs                  Devices<'_> API, ReconcileReport
  src/sync.rs                     Sync<'_> API, SyncPhase, Stage, SyncEvent, SyncReport
  src/wallet.rs                   WalletApi<'_>, Balance, format_hash, parse_amount
  src/provider.rs                 Provider<'_>, ProviderStatus, Lifecycle, Earnings
  src/network.rs                  Network<'_>, Overview, PeerView, indexer reads
  src/store.rs                    LocalStore (encrypted redb) — SDK-internal, do not bypass
  src/backup.rs                   export_backup / import_backup / inspect_backup
  src/storage_lease.rs            Lease<'_> (paid storage, partially wired; see §7)
  src/ai.rs                       AiProvider trait, LocalAiProvider, UserConsent
  src/account.rs                  Account (vault), create/import/open, on-chain identity txs
  src/messaging.rs                MLS transport (used by the facade; you do not call it)
node/hashgram-app/                pure protocol crate: types + rules (re-exported as hashgram_sdk::protocol)
  src/mail.rs                     Draft, ReplyTarget, bounds (MAX_*), parse_address, AddressForm
  src/drive.rs                    Manifest, EntryView, seal/open, merge
  src/space.rs                    State, Member, Content, SharedEntry, Rejected
  src/circle.rs                   Timeline, Item
  src/people.rs                   Contacts state flags (FRIEND, PENDING_IN, …)
  src/spam.rs                     Disposition, trust_score
proto/hashgram/app/v1/app.proto   wire types; generated as hashgram_sdk::protocol::pb
node/hashgram-client/src/one.rs   CLI over the facade — the reference for every flow
apps/desktop/                     the PREVIOUS desktop app (v0.1.1, Tauri 2 + SolidJS). Reuse its
                                  shell pieces where they fit (see §3); its Messages/Reels/Stories/
                                  Channels/Calls screens are superseded and must not be carried over.
scripts/testnet/hashgram-one-e2e.sh  local devnet acceptance test (31 checks) — run it to see
                                  every flow working before you write UI
docs/                             specs listed above
```

### 1.2 The facade

```rust
use hashgram_sdk::{Config, HashgramOne, Paths};
use hashgram_sdk::identity::KdfCost;   // hashgram_identity::vault::KdfCost re-export

let cfg = Config {
    paths: Paths::new(&data_dir),                   // keystore.json, local.redb, peerstore.json, cache/
    network: hashgram_sdk::NetworkIdentity::mainnet(GENESIS_HASH),
    bootstrap: vec![],                              // empty => compiled-in Mainnet list
    chain_api: None,                                // None => chain reads through the P2P relay, cross-checked
    kdf: KdfCost::default(),                        // Argon2id 64 MiB; KdfCost::light() is DEVNET ONLY
    connect_wait: std::time::Duration::from_secs(10),
};
let mut one = HashgramOne::open(cfg, &passphrase).await?;     // existing vault
// or: let (account, mnemonic) = Account::create(&paths.vault(), &passphrase, "windows-1", kdf)?;
//     let mut one = HashgramOne::with_account(cfg, account).await?;
one.mail().send(draft).await?;
one.sync().round().await?;
one.save()?;                                                  // after every mutating command
```

`HashgramOne` fields you may read: `account: Account` (`.address()`,
`.contents.device_id`), `network: NetworkIdentity`, `link: Arc<Link>`,
`chain: ChainClient`, `paths`. The `Mail<'_>`, `Drive<'_>` … views borrow
`&mut HashgramOne`; hold `HashgramOne` in a `tokio::sync::Mutex` inside the
Tauri managed state and lock it per command. Every mutation is followed by
`one.save()` (persists MLS state + Drive keyring into the vault). `open`
connects in the background and returns after the first verified peer or
`connect_wait`; the app must render from local state immediately and show
sync state from `one.sync().phase()`.

Mainnet constants (from `app/params/mainnet.go`, `node/hashgram-net/src/mainnet.rs`):
chain-id `hashgram-1`, network-id `hashgram-mainnet`, genesis hash
`e322bc2319f6e0173286fa526dab5a8ff8ad0797c7b80dd03e7c9d98621d5e4d`,
denom `uhash` (1 HASH = 1,000,000 uhash), address prefix `hash1`.

### 1.3 Error type

`hashgram_sdk::SdkError` (in `sdk/rust/hashgram-sdk/src/lib.rs`): variants
`Vault, Wallet, Chain, Link, Mls, Sign, Canonical, Key` (transparent),
`NoWalletKey, NoRootKey, NoDeviceKey, NoKeyPackage(String), NoRecipients,
Delivery(String), Invalid(String), NotFound(String), Corrupt(String),
Store(String), Unsupported(String)`. Map them to user-facing states:
`Link(NoPeer)` → "offline / connecting"; `NoRecipients`/`NoKeyPackage` →
"recipient has no device online yet (they must open the app once)";
`Unsupported` → "update Hashgram to read this"; `Chain(NoAccount)` →
balance 0 (never an error dialog); `Delivery` → retry banner.

### 1.4 Data model you will render

All below are `serde::Serialize` and can be passed to the frontend as-is.

**Mail** (`sdk/rust/hashgram-sdk/src/mail.rs`)
- Folders: `hashgram_sdk::mail::folder::{INBOX, SENT, DRAFTS, ARCHIVE, TRASH, SPAM, REQUESTS}` — string constants `"inbox"` etc.
- `MailSummary { id, thread_id, folder, from, from_username, to: Vec<String>, subject, preview, created_at_ms, received_at_ms, read, starred, attachments: usize, labels, external, bcc_copy, outgoing }`
- `MailRecord { message: protocol::pb::MailMessage, folder, read, starred, labels, received_at_ms, authenticated_sender, group_id, delivered_to: BTreeMap<addr, ms>, read_by, outgoing, trust_score }` — **display `authenticated_sender`**, never trust `message.from.address` alone (see HASHMAIL.md, impersonation rule).
- `Thread { id, subject, messages: Vec<MailRecord>, participants, unread }`
- `FolderCounts { total, unread }`; `MailSettings { send_read_receipts, keep_sent, purge_after_days }`
- `DraftRecord { to, cc, bcc, subject, body_text, body_html, attachments, in_reply_to, updated_at_ms }`
- `protocol::pb::MailMessage` fields: `message_id, thread_id, from, to, cc, bcc_copy, created_at_ms, subject, in_reply_to, references, body_text, body_html, attachments: Vec<MailAttachment>, expire_after_secs, origin (MailOrigin::Native | ExternalGateway), external: Option<ExternalMailMeta>, request_read_receipt, importance, labels`
- `MailAttachment { name, mime, size, plaintext_hash, content_id, source: InlineData | Blob(BlobRef) | Drive(DriveCapability) }` — a `Drive` source with `mode == DriveShareMode::Live` is a live attachment (show "updates" when `Drive::shared_with_me` has a newer `version_no` for its `share_id`).
- `protocol::mail::Draft { from (ignored; set by SDK), to, cc, bcc: Vec<MailAddress>, subject, body_text, body_html, attachments, in_reply_to: Option<ReplyTarget>, expire_after_secs, request_read_receipt, importance, labels }`
- Bounds to enforce in the composer (from `protocol::mail`): `MAX_RECIPIENTS=100, MAX_SUBJECT=998, MAX_BODY_TEXT=1 MiB, MAX_BODY_HTML=1 MiB, MAX_ATTACHMENTS=64, MAX_INLINE_ATTACHMENT=64 KiB`.

**Drive** (`sdk/rust/hashgram-sdk/src/drive.rs`, `hashgram_app::drive`)
- `EntryView { id, parent_id, kind: "file"|"folder", name, mime, size, created_at_ms, modified_at_ms, versions, trashed, starred, path }`
- `DriveUsage { files, folders, trashed, bytes, revision, committed_revision, dirty }`
- `SharedWithMe { capability: DriveCapability, from, group_id, note, received_at_ms, updates, revoked }`
- `protocol::pb::DriveShareRecord { share_id, entry_id, grantee, mode, permission, granted_at_ms, revoked, revoked_at_ms, group_id }`
- `protocol::pb::DriveVersion { version_no, object, created_at_ms, device_pubkey, note }`
- Ids are hex strings; the root folder is `""`.

**People** (`sdk/rust/hashgram-sdk/src/people.rs`)
- `Resolved { address, username, display_name, mail_address, has_identity, devices }`
- `Profile { address, username, display_name, bio, avatar_cid, states }`
- `protocol::pb::ContactRecord { address, username, display_name, states: Vec<String>, updated_at_ms }` with state flags from `protocol::people::{FRIEND, PENDING_OUT, PENDING_IN, BLOCKED, MUTED, TRUSTED, FOLLOWING}`.

**Feed** (`sdk/rust/hashgram-sdk/src/feed.rs`)
- `FeedItem { id, kind ("POST_CREATE"|"REPOST"|"COMMENT_CREATE"|…), author, timestamp (s), payload: serde_json::Value, media: Vec<(cid_hex, mime, size)>, visibility }`
- `PostThread { post, comments, reactions: BTreeMap<String, u32> }`

**Circles** (`sdk/rust/hashgram-sdk/src/circles.rs`, `hashgram_app::circle`)
- `CircleInfo { id, name, description, members, since, owner }`
- `protocol::circle::Item { id, kind: "post"|"comment", author, at_ms, text, post_id, reply_to, media, drive_refs, poll: Option<Poll>, reactions, votes: BTreeMap<u32,u32>, deleted }`

**Spaces** (`sdk/rust/hashgram-sdk/src/spaces.rs`, `hashgram_app::space`)
- `SpaceSummary { id, name, description, my_role: i32, members, group_id, created_at_ms }`
- `protocol::space::State { space_id, name, description, avatar, members: BTreeMap<addr, Member>, content: Vec<Content>, drive: BTreeMap<share_hex, SharedEntry>, sequences, applied, head, created_at_ms }`
- `Member { address, role, since_ms }`; roles `protocol::pb::SpaceRole::{Guest=1, Member=2, Admin=3, Owner=4}`
- `Content { id, kind: "post"|"comment"|"announcement", actor, at_ms, title, text, post_id, drive_refs, media }`
- `SharedEntry { capability, path, by, at_ms }`
- Rule violations surface as `SdkError::Invalid("space rule: actor … may not …")` — show them as disabled controls *before* the user tries (derive from `my_role`; the table is in `docs/SPACES.md`).

**Sync** (`sdk/rust/hashgram-sdk/src/sync.rs`)
- `SyncPhase::{Offline, Connecting, Discovering, Syncing(Stage), Idle, Backoff{attempt, wait_secs}}`, `Stage::{Mailbox, Outbox, Feed, Wallet, Devices}`
- `SyncEvent::{Phase, NewMail{id, folder}, DriveShareChanged, ContactsChanged, CircleActivity(hex), SpaceActivity(hex), Balance(uhash string), UnsupportedMessage, Warning(String)}`
- `SyncReport { envelopes, mail, drive, people, circles, spaces, device_sync, unsupported, chat: Vec<Received>, feed, drive_committed, elapsed_ms, balance_uhash }` — `chat` holds legacy chat lines from the old messenger; the new app ignores them (do not build a chat UI).

**Wallet / Earn / Network**
- `wallet::Balance { address, uhash: String, display, verification: Option<String> }`
- `provider::ProviderStatus { operator, reward_address, roles, bond_uhash, declared_storage_bytes, fraud_score, jailed, jailed_until_height, unbonding_height, registered_height, moniker, lifecycle: Lifecycle, raw }`, `Lifecycle::{Unregistered, Registered, WaitingForAssignment, Active, Degraded, Jailed, Unbonding, Withdrawn}`, `Earnings { total_paid_uhash, pending_credit, epoch, reserve_remaining_uhash, raw }`
- `network::Overview { network_id, chain_id, genesis_hash, peers: Vec<PeerView{peer, roles, operator}>, rejected: Vec<(peer, reason)>, height, verification }`
- Indexer (optional, user-configured URL; label data "from indexer"): `Network::top_holders/top_validators/top_providers/stats(indexer, limit)` return `Option<serde_json::Value>` shaped as documented in `docs/INDEXER.md` (`/v1/leaderboards/holders` rows: `rank, address, amount, kind, username?, verified`).

**Devices**: `account::DeviceView { device_id, device_pubkey, label, platform, revoked }`, `devices::ReconcileReport { groups, removed, added, errors }`.

**Backup**: `backup::BackupMeta { exported_at, address, partial, from_device }`; `export_backup(&one.account, path, passphrase, None)`, `inspect_backup(bytes) -> (m_kib, t, p)`, `import_backup(bytes, passphrase, vault_path, vault_passphrase, device_id, kdf) -> Imported { account, meta, new_device_pubkey_hex }`.

### 1.5 The complete SDK call surface (what your Tauri commands wrap)

From `one.mail()`: `resolve_recipients(&[String])`, `my_address()`, `send(Draft) -> id`, `reply_draft(id, all) -> Draft`, `forward_draft(id) -> Draft`, `save_draft/drafts/delete_draft`, `get(id)`, `list(folder, before_ms, limit)`, `threads(folder, before_ms, limit)`, `thread(thread_id)`, `counts()`, `mark_read(id, bool)` (async; sends READ receipt if allowed), `star`, `move_to(id, folder)`, `archive`, `trash` (second call permanently deletes), `delete`, `accept_request(id)`, `set_label(id, label, on)`, `by_label`, `starred`, `search(q, limit)`, `purge()`, `settings()/set_settings()`, `attachment_bytes(&MailAttachment) -> Vec<u8>`, `make_attachment(name, mime, bytes)`, `attach_from_drive(entry_hex, &recipients, live)`.

From `one.drive()`: `list(parent_hex)`, `trash_list()`, `starred()`, `search(q, limit)`, `entry(id)`, `versions(id)`, `resolve_path(path)`, `usage()`, `mkdir(parent, name)`, `upload(parent, name, mime, bytes) -> id`, `update(id, bytes, note) -> version_no`, `rekey(id)`, `rekey_all()`, `restore_version(id, no)`, `rename`, `mv`, `copy`, `trash`, `restore`, `delete` (trashed only), `empty_trash`, `star`, `download(id)`, `download_version(id, no)`, `download_capability(&cap)`, `shared_folder_entries(&cap)`, `save_capability(&cap, parent)`, `share(id, "addr1,addr2", DriveShareMode, DrivePermission, note) -> DriveCapability`, `revoke(share_hex)`, `shares()`, `shared_with_me()`, `commit() -> revision`. Note: `upload`/`update`/mutations are local until `commit()`; the sync engine commits automatically when `usage().dirty`. **Large files**: `upload` takes `&[u8]`; for files above ~256 MiB stream them through the Tauri backend into memory in one go is unacceptable — add a streaming path using `hashgram_app::drive::SegmentEncryptor` + `hashgram_sdk::blob::upload_sealed` chunk-by-chunk (the SDK functions are public; see `docs/HASHDRIVE.md`), or cap uploads at 256 MiB in v1 and say so in the UI.

From `one.people()`: `resolve(input)`, `profile(addr)`, `username_of`, `owner_of`, `set_my_display_name`, `request(input, message)`, `respond(addr, accept)`, `remove`, `block/unblock`, `mute(addr, on)`, `trust(addr, on)`, `follow(addr, on)` (public event), `friends/incoming_requests/outgoing_requests/blocked/all/search_local`, `send_card(addr, bio, disclose_wallet)`, `card_of(addr)`.

From `one.feed()`: `post(text, hashtags, media, sensitive)`, `comment`, `react`, `repost`, `edit_post`, `delete_post`, `update_profile(name, bio, avatar_cid_hex)`, `set_follow`, `follows()`, `refresh()`, `refresh_author`, `following(before, limit)`, `friends(before, limit)`, `author(addr, before, limit)`, `thread(post_hex)`, `fetch(ids)`, `upload_media(bytes, mime, kind)`. Explore = indexer `/v1/feed/chronological` and `/v1/feed/hashtag/{tag}` when configured.

From `one.circles()`: `create(name, desc, &members)`, `add_member`, `remove_member`, `leave`, `post(circle, text, media, drive_refs, poll)`, `comment`, `react`, `vote(circle, post, choices)`, `delete`, `set_info`, `list()`, `posts(circle, before, limit)`, `comments(circle, post)`, `merged(before, limit)`.

From `one.spaces()`: `create(name, desc) -> space_hex`, `invite(space, addr, SpaceRole)`, `remove(space, addr, reason)`, `set_role`, `set_info`, `announce(space, title, text, attachments)`, `post(space, text, media, drive_refs)`, `comment`, `share_drive(space, entry_hex, path, live)`, `unshare_drive`, `list()`, `state(space)`, `members(space)`, `content(space, before, limit)`, `drive_entries(space)`, `mail(space, subject, body)`.

From `one.devices()`: `list()`, `this_device() -> (id, pubkey_hex)`, `add_on_chain(device_id, pubkey_hex, label, platform)` (needs root key), `revoke_on_chain(device_id)` (needs wallet key), `reconcile()`, `bootstrap_new_device()`.

From `one.sync()`: `subscribe() -> UnboundedReceiver<SyncEvent>`, `phase()`, `round() -> SyncReport`, `next_delay()`.

From `one.wallet()`: `balance(None)`, `send(to, uhash, memo)`, `preview_send`, `stake`, `unstake`, `withdraw_rewards`, `register_username(name)` (1 HASH), `renew_username`, `username_availability(name)` (returns the chain's reason code), `delegations()`, `history(limit)`, `tx(hash)`.

From `one.provider()`: `status(None)`, `earnings(None)`, `list()`, `register(reward_address, node_pubkey_hex, &roles, bond_uhash, storage_bytes, moniker)`, `update`, `unbond`, `withdraw`. The node itself is installed/managed as in the previous app (`apps/desktop/src-tauri/src/node_manager.rs`, scheduled task or Windows service) — reuse that.

From `one.network_api()`: `overview()`, `validators()`, `supply()`, `indexer(url, path)`, `top_holders/top_validators/top_providers/stats`.

From `hashgram_sdk::account`: `Account::create(vault_path, passphrase, device_id, kdf) -> (Account, mnemonic)`, `Account::import(…mnemonic…)`, `Account::open`, `create_identity_on_chain(&account, &network, &chain, label, platform)` (requires funds for gas; a brand-new user has 0 HASH — show the "get HASH" state honestly; welcome rewards are inert on Mainnet today).

### 1.6 The sync loop you must run

```rust
// background task, after unlock
let mut rx = one.sync().subscribe();
loop {
    let delay = { let mut g = state.one.lock().await; let r = g.sync().round().await; let _ = g.save(); g.sync().next_delay() };
    // emit rx events to the webview via app.emit("sync", event)
    tokio::time::sleep(delay).await;
}
```

Timer: `next_delay()` returns 4 s in Idle and the backoff during `Backoff`.
`round()` never panics on network input; treat `Err` as "offline" and keep
the UI usable from local state. Reconciliation of devices happens every 20
rounds inside `round()`.

### 1.7 What is partial or absent in the platform (do not paper over)

- **External e-mail**: `services/mail-gateway` bridges Internet mail. From
  the app's point of view: mail with `message.origin == ExternalGateway`
  must show an "External — not end-to-end encrypted" badge and the
  `external.auth_results`; sending to an external address is done by
  sending native mail to a gateway identity with a label
  `ext-to:<address>` (see `docs/MAIL_GATEWAY.md` §"ext-to convention");
  the gateway address is a user setting (empty by default → external
  sending disabled with an explanation).
- **Paid storage leases**: `one.leases()` exists (`draft`, `record`,
  `verify_epoch`, `pay_epoch`, `list`, `terminate`) but `offer()` returns
  `Unsupported` until protocol v1.1. Do not build a lease marketplace UI;
  an "Advanced" page may list recorded leases and run verification.
- **Hash AI**: `hashgram_sdk::ai` is interfaces only; `LocalAiProvider` is
  keyword retrieval. Ship a local "Ask" box over `Corpus` built from local
  Mail/Drive/People only, with no remote provider.
- **Push notifications** do not exist; notifications come from `SyncEvent`.
- **Calls** are removed from the product; do not build them.
- **New-user funding**: creating an on-chain identity needs gas; welcome
  attestors are empty on Mainnet. The app must work fully *offline-local*
  (Drive, drafts, settings) with a clear "register your identity" step
  gated on balance > 0, and messaging to a user requires that user's
  devices to be on chain and to have published key packages (they must have
  opened the app online once).
- **Multi-device**: a second device is added via `Devices::add_on_chain`
  from the device that holds the root key; the new device's Drive keyring
  and contacts arrive through `DeviceSync` on its next sync. Two devices
  that created separate Drives before ever syncing keep separate Drives.

---

## 2. Stack (decided)

- **Tauri 2** (Rust backend, WebView2), **SolidJS 1.9** + **TypeScript
  5.9** + **Vite 7**, **Tailwind v4**, `@kobalte/core` for accessible
  primitives, `@tanstack/solid-virtual` for lists, `lucide-solid` icons,
  **vitest** for TS tests. Same as `apps/desktop/package.json`; upgrade
  patch versions only.
- Rust: workspace member at `apps/desktop/src-tauri` (already in
  `node/Cargo.toml` members), depends on `hashgram-sdk` by path.
  Keep the strict workspace lints (`unwrap_used = deny`, `panic = deny`).
- Local DB: SQLite via `rusqlite` (bundled) for **UI caches and indexes
  only** (search index, list ordering, thumbnails metadata) with
  column-level sealing exactly as `apps/desktop/src-tauri/src/crypto.rs`
  and `db.rs` do today (XChaCha20-Poly1305, key from the vault's
  `extra["desktop_db_key"]`). The SDK's `LocalStore` remains the source of
  truth for Mail/Drive/People/Spaces state; never write to it directly.
- Windows Hello convenience unlock: reuse `apps/desktop/src-tauri/src/winsec.rs`
  (DPAPI-wrapped passphrase, entropy from a Hello signature).
- Updater: `tauri-plugin-updater` with the existing minisign key and
  endpoint in `apps/desktop/src-tauri/tauri.conf.json`; installer targets
  `nsis` + `msi`, per-user install (`installMode: currentUser`).
- Paths: `%LOCALAPPDATA%\Hashgram\data\` (`apps/desktop/src-tauri/src/paths.rs`);
  the SDK `Paths::new(data_dir)` creates `keystore.json`, `local.redb`,
  `peerstore.json`, `cache/`. Migrate the v0.1.x vault in place (same file
  format).

---

## 3. What to reuse from `apps/desktop` and what to delete

Reuse (adapt): `src-tauri/src/{paths,winsec,crypto,db,settings,perf,help,node_manager,commands_node,chain_access,chain_proxy}.rs`, the onboarding/lock/wallet routes (`src/routes/Onboarding.tsx`, `Lock.tsx`, `wallet/*`), `Network.tsx` (peer panel), `Earn.tsx`/`RunNode.tsx` (adapt to `provider()` API), `Settings.tsx` skeleton, `Help.tsx` + `help/*.md`, tests infrastructure (`tests/setup.ts`, `rules.test.ts`, `palette.test.ts`, `no_plaintext.rs`), `release.ps1`, `.github/workflows/desktop-release.yml`.

Delete: `src/routes/{Messages,Calls,Reels,Channels,Home}.tsx`,
`src-tauri/src/{chat,social_hub,commands_social}.rs` (replace with the new
command modules below). `Feed.tsx`/`Profile.tsx` are rewritten over the
`Feed<'_>`/`People<'_>` APIs.

---

## 4. Tauri command surface to implement (Rust)

One module per area under `apps/desktop/src-tauri/src/`:
`cmd_identity.rs, cmd_mail.rs, cmd_drive.rs, cmd_people.rs, cmd_feed.rs,
cmd_circles.rs, cmd_spaces.rs, cmd_earn.rs, cmd_wallet.rs, cmd_network.rs,
cmd_sync.rs, cmd_settings.rs, cmd_backup.rs`. Every command: takes
`State<'_, AppState>` (holds `Mutex<Option<HashgramOne>>`, settings, db,
auto-lock deadline), locks, calls the SDK, calls `save()` on mutation,
returns a `Serialize` view or a `String` error mapped from `SdkError` via a
`UiError { code, message, retryable }` type. Never return raw `Debug`
strings. Commands must be listed in `tauri::generate_handler![]` and
scoped by `capabilities/default.json` (no shell, no fs, no http from the
webview — keep the CSP `connect-src ipc: http://ipc.localhost`).

Required commands (names are yours, semantics are these):

Identity/vault: `app_status`, `onboarding_generate` (24 words, held in
`Zeroizing<String>` until `onboarding_create`), `onboarding_check_words`,
`onboarding_create(passphrase, device_label)`, `onboarding_restore(mnemonic, passphrase, device_label)`,
`restore_from_backup(path, backup_passphrase, vault_passphrase, device_label)` (→ `backup::import_backup`),
`unlock(passphrase)`, `hello_enable/hello_unlock/hello_disable`, `lock`,
`change_passphrase`, `identity_status` (on-chain registered? devices? username?),
`identity_register(label)` (→ `account::create_identity_on_chain`),
`devices_list/this_device/device_add(pubkey_hex,label)/device_revoke(id)/devices_reconcile`,
`backup_export(path, passphrase)`, `backup_inspect(path)`, `wipe_local_data(confirm)`.

Mail: `mail_counts`, `mail_list(folder, before_ms, limit, threaded)`,
`mail_thread(thread_id)`, `mail_get(id)`, `mail_send(draft_json)`,
`mail_reply_draft(id, all)`, `mail_forward_draft(id)`, `mail_draft_save/list/delete`,
`mail_mark_read(id, read)`, `mail_star`, `mail_move(id, folder)`, `mail_archive`,
`mail_trash`, `mail_delete`, `mail_accept_request`, `mail_label(id, label, on)`,
`mail_by_label`, `mail_starred`, `mail_search(q)`, `mail_attachment_save(id, index, path)`
(decrypts via `attachment_bytes` in Rust and writes with the dialog plugin),
`mail_attachment_open(id, index)` (to a temp file then opener plugin),
`mail_attach_file(path) -> MailAttachment` (→ `make_attachment`),
`mail_attach_drive(entry_id, recipients, live)`, `mail_settings_get/set`,
`mail_resolve_recipients(inputs)`.

Drive: `drive_list(parent)`, `drive_trash`, `drive_starred`, `drive_search`,
`drive_entry`, `drive_versions`, `drive_usage`, `drive_mkdir`,
`drive_upload(parent, path)` (read file in Rust; show progress via events per
segment if you implement streaming), `drive_upload_bytes(parent, name, mime, base64)` for
drag-drop of small files, `drive_update(id, path)`, `drive_download(id, out_path)`,
`drive_download_version`, `drive_open(id)` (temp file + opener),
`drive_rename/move/copy/trash/restore/delete/empty_trash/star`,
`drive_share(id, grantees, live, note)`, `drive_revoke`, `drive_shares`,
`drive_shared_with_me`, `drive_shared_download(share_id, out_path)`,
`drive_shared_save(share_id, parent)`, `drive_shared_folder_list(share_id)`,
`drive_rekey(id)`, `drive_commit`.

People: `people_resolve(input)`, `people_profile(addr)`, `people_request`,
`people_respond(addr, accept)`, `people_remove/block/unblock/mute/trust/follow`,
`people_list(which)`, `people_search_local(q)`, `people_set_display_name`,
`people_send_card`, `people_card_of`.

Feed: `feed_following(before, limit)`, `feed_friends`, `feed_author`,
`feed_explore(indexer_url, before, limit, tag?)`, `feed_thread(post)`,
`feed_post(text, hashtags, media_paths, sensitive)`, `feed_comment`,
`feed_react`, `feed_repost`, `feed_edit`, `feed_delete`, `feed_refresh`,
`feed_profile_update(name, bio, avatar_path)`, `circles_merged(before, limit)`.

Circles: `circles_list/create/add_member/remove_member/leave/post/poll/comment/react/vote/delete/set_info/posts/comments`.

Spaces: `spaces_list/create/state/members/content/drive/invite/remove/set_role/set_info/announce/post/comment/share_drive/unshare_drive/mail`.

Earn: `earn_status`, `earn_earnings`, `earn_providers`, `earn_register(...)`,
`earn_unbond`, `earn_withdraw`, plus the node manager commands from the
previous app (`node_overview/configure/install/start/stop/uninstall/log_tail`).

Wallet: `wallet_balance`, `wallet_send(to, amount_str, memo)` (parse with
`parse_amount`), `wallet_preview_send`, `wallet_stake/unstake/withdraw_rewards`,
`wallet_username_availability`, `wallet_register_username`, `wallet_renew_username`,
`wallet_delegations`, `wallet_history`, `wallet_tx(hash)`.

Network: `network_overview`, `network_validators`, `network_supply`,
`network_top(what, limit)` (uses the configured indexer URL or returns null),
`network_stats`, `net_reconnect`, `net_forget_peers`, `diagnostics_export`
(no IP addresses, no addresses of contacts).

Sync: `sync_phase`, `sync_now` (one round), background loop started at
unlock, events `sync:phase`, `sync:event` emitted to the webview.

Settings: everything in a `Settings` struct persisted as today; new fields:
`indexer_url: Option<String>`, `gateway_address: Option<String>` (for
`ext-to`), `read_receipts: bool`, `auto_lock_minutes`, `theme`,
`notifications: {mail, requests, spaces, circles}`.

---

## 5. Screens (design + behaviour)

Design language: **dark-first**, black `#000000`, near-blacks `#0d0d0d
#1a1a1a #262626`, greys `#404040 #808080`, white; one accent (use a
desaturated blue-grey; never orange/gold "crypto" palettes). Typography:
Inter (bundled locally), 13–14 px body, tabular numerals for amounts.
Professional density (Outlook/Fastmail-like), fast transitions (≤150 ms),
no gradients, no glass. Light theme derived from the same tokens. All
strings through an i18n table (English first; Georgian second — the
project owner reads Georgian).

Shell: left rail with the nine sections + unread badges (Mail requests
and inbox unread from `mail_counts`), top bar with global search
(`Ctrl+K` → Mail/Drive/People search merged), sync indicator
(phase-driven: dot + tooltip "Synced 12 s ago" / "Offline — showing local
data" / "Connecting…"), lock button. Deep links `hashgram://mail/<id>`,
`hashgram://space/<id>`, `hashgram://drive/<id>`, `hashgram://user/<addr>`
via `tauri-plugin-deep-link` (already configured with single-instance).

**Onboarding**: choose "Create identity" / "Restore from 24 words" /
"Restore from backup file". Create: show words once (screen-blur when the
window loses focus, screenshots not blocked but warned), 3-word check,
passphrase (strength meter, ≥ 10 chars), device label, optional Windows
Hello. Then: "Your address hash1… — to send and receive mail you need to
register this identity on chain (needs a small amount of HASH for the fee)
and choose a username." Show honest gating: balance 0 → "Receive HASH
first" with QR of the address. Registration → `identity_register`. Username
→ `wallet_username_availability` live check → `wallet_register_username`.

**Lock screen**: passphrase or Hello. Auto-lock after N minutes idle
(default 15) → `lock` drops `HashgramOne` (zeroizes via Drop).

**Mail** (home): three panes — folders (Inbox, Requests with count,
Starred, Sent, Drafts, Archive, Spam, Trash, Labels), list (virtualized,
threaded toggle; rows show authenticated sender name (@username or short
address), subject, preview, time, attachment icon, star, "External" badge,
"BCC" chip), reading pane (headers with `authenticated_sender` and a
"verified device" mark; body_text rendered with linkification only;
body_html rendered in a sandboxed iframe with `sandbox=""`, no remote
loads, no scripts; attachments with Save/Open, live attachments show
version + "updated" chip; delivered/read receipts on sent mail; Reply /
Reply all / Forward). Composer: recipients with resolution chips (call
`mail_resolve_recipients` on blur; show @username and address; external
addresses only when gateway configured), CC/BCC toggles, subject, plain
text editor (HTML optional, generated from a minimal markdown subset),
attachments via file picker or drag-drop (≤ 64 KiB inline automatically),
"Attach from Drive" (snapshot/live choice), request read receipt, send →
progress → Sent. Requests folder: Accept (moves to inbox) / Block sender /
Delete. Keyboard: `c` compose, `r` reply, `a` reply all, `f` forward, `e`
archive, `#` trash, `s` star, `j/k` navigate, `/` search, `Ctrl+Enter`
send.

**Drive**: breadcrumb path, list/grid toggle, folders first, columns Name/
Modified/Size/Versions, right-click menu (Open, Download, Rename, Move,
Copy, Star, Share…, Versions, Trash), drag-drop upload with progress,
sidebar sections My Drive / Shared with me / Starred / Trash, usage bar
(`drive_usage`), uncommitted badge when `dirty` ("changes will sync"),
Share dialog (recipients, Snapshot vs Live explained in one sentence each,
note) listing existing shares with Revoke, Versions panel with Restore,
Shared-with-me rows with owner, version, "updated" and "revoked" states,
Save to my Drive, Open folder share (lists `DriveFolderManifest.entries`).
Rekey action under "…" with a one-line explanation.

**Feed**: tabs Friends / Following / Circles / Explore (Explore disabled
until an indexer URL is set, with the sentence why). Chronological only.
Composer for public posts (text, up to 20 media, hashtags, sensitive
flag) and circle posts (choose circle, poll builder). Post view with
comments and reactions. Profile page from `people_profile` + author feed;
Follow button; "Add as contact".

**People**: search box (username / mail address / hash1… / local
contacts), lists Friends / Requests (incoming with Accept/Reject, outgoing) /
Following / Blocked, contact card (display name, @username, address with
copy, mail button → composer, devices count, trust toggle, mute, block,
"Send my card" with wallet-disclosure checkbox), and the user's own
profile editor (public display name/bio/avatar → `feed_profile_update`;
private display name → `people_set_display_name`).

**Spaces**: list with role badge; space view with tabs Overview
(announcements), Posts, Drive (path tree from `SharedEntry.path`),
Members (role management enabled only when `my_role` allows; owner
transfer with confirmation), Mail (send to all members). Invite dialog
resolves the address and picks a role. Rule errors from the SDK shown as
inline toasts.

**Earn**: provider lifecycle card (`Lifecycle` → plain sentences, e.g.
WaitingForAssignment → "Registered as a storage provider; the network has
not assigned data yet — this needs an assigner to be registered by
governance"), earnings, node manager (install/start/stop, log tail),
register form (reward address must differ from the operator; explain
why), never the word "mining".

**Wallet**: balance with verification note (`Balance.verification`),
send with fee preview and confirmation, receive (QR via the `qrcode`
crate), history, staking (validators list, delegate/undelegate, rewards),
usernames (register/renew, availability reason codes), identity/devices
management (add device by pasting its pubkey, revoke, reconcile),
vesting info if present, Founder transparency page kept from the old app.

**Network**: peers with roles/operator, rejected peers greyed with
reason, chain height and verification, supply/reserve figures, top
holders/validators/providers (indexer; each row: rank, address, amount,
username only when `verified`), network stats, diagnostics export.

**Settings**: Network (indexer URL, gateway address, bootstrap override),
Security (passphrase change, Windows Hello, auto-lock, backup export with
a passphrase field enforcing ≥ 12 chars, backup restore), Devices,
Notifications, Mail (read receipts, purge days, keep sent), Appearance
(dark/light/system, density), Updates (check now, channel), Advanced
(leases list + verify, rekey all, wipe local data with typed confirmation),
About (version, genesis hash, licenses).

Empty/loading/error/offline/security states for every screen: skeletons
while local store loads (should be < 100 ms), "Offline — showing what's on
this device" banner from `SyncPhase`, explicit error toasts with a Retry
action, "Locked" overlay.

Notifications (`tauri-plugin-notification`): new Inbox mail (never
subject text in the notification body — only "New mail from @name"),
new Request, Space/Circle activity (configurable). Nothing leaves the
device.

---

## 6. Security requirements (testable)

1. `tests/no_plaintext.rs` (extend the existing one): after creating an
   account, sending a mail to a second local account, uploading a Drive
   file and saving settings, scan every byte written under the data dir for:
   mnemonic words, wallet secret, root seed, device seed, Drive object
   key, manifest key, the mail subject and body, the file name and
   content. None may appear in the clear.
2. Frontend never receives keys: a vitest test greps the Rust command
   return types (a generated JSON schema or a checked-in list) for
   forbidden field names (`seed`, `secret`, `mnemonic` outside
   onboarding, `key`, `nonce`).
3. HTML mail renders in a sandboxed iframe; a test injects `<script>` and
   `<img src=http://…>` and asserts no execution/no request.
4. Auto-lock drops the `HashgramOne` value; a test asserts commands fail
   with `locked` afterwards.
5. Mnemonic display component blocks copy events and blurs on window
   blur.
6. CSP unchanged; capability file allows only the plugins listed.
7. Logging: `tracing` to a rotating file under `logs/`; never log
   subjects, bodies, addresses of contacts, or key material (reuse the
   `check-logging.sh` policy from `scripts/dev/`).

---

## 7. Performance budgets (measured in CI where possible)

Cold start to lock screen < 1.5 s; unlock to Mail rendered from local
store < 400 ms with 10,000 messages; list scroll 60 fps (virtualized);
installer < 45 MB; idle RAM < 180 MB; a sync round on Mainnet with one
store node < 3 s (the SDK bounds DHT lookups at 3 s); Drive upload of
100 MiB shows progress and completes without the UI freezing (do the work
on the Tauri async runtime, never on the main thread).

---

## 8. Testing

- Rust: unit tests for every command module's mapping functions; the
  `no_plaintext` integration test; a headless integration test that runs
  `scripts/testnet/hashgram-one-e2e.sh`'s devnet (mock gateway + node) and
  drives two app backends through the command functions directly (send
  mail A→B, share Drive, create Space) — reuse the script's setup steps.
- TS: vitest for formatting (`format_hash` parity), rules tests (no
  hardcoded host, no forbidden words like "mining", palette limited to
  the tokens), component tests for the composer recipient chips, the
  Requests folder actions, Drive share dialog, Space role gating.
- Manual QA checklist in `docs/DESKTOP_APP_IMPLEMENTATION_CHECKLIST.md`
  (already written; follow it stage by stage).

---

## 9. Release

`release.ps1` builds NSIS + MSI, signs with minisign
(`TAURI_SIGNING_PRIVATE_KEY`), publishes `latest.json` to the GitHub
release; the tag must equal `tauri.conf.json` version; bump to `0.2.0`
for the first Hashgram One release. Windows code signing (Authenticode)
is a separate certificate step: architect it (signtool in CI with the
certificate from a secret), leave it disabled until a certificate exists,
and say so in the About page ("unsigned preview" vs "signed").

---

## 10. Definition of done

- [ ] Fresh install → create identity → lock/unlock → Mail home renders
- [ ] Two accounts on the devnet exchange mail with inline and live Drive attachments; Requests → Accept flow; reply lands in Inbox; receipts shown
- [ ] Drive: upload/download/rename/move/versions/restore/trash/share/revoke/shared-with-me/save-to-drive all through the UI
- [ ] People: resolve @name and name@hashgram.io; request/accept; block; follow
- [ ] Feed: post/comment/react; Friends and Following tabs; Circle with poll
- [ ] Spaces: create, invite, roles gated, announcement, posts, Space Drive, Space mail
- [ ] Earn: lifecycle sentence matches `Lifecycle`; node manager works
- [ ] Wallet: balance with verification, send with preview, username register
- [ ] Network: peers, height, leaderboards from a configured indexer
- [ ] Backup export/restore round trip
- [ ] Multi-device: add a second device, it receives Drive keyring + contacts after sync
- [ ] `no_plaintext` and forbidden-field tests pass; CSP intact; `grep -rn 186.241 apps/` empty
- [ ] Installer builds, updater manifest valid, `desktop-release.yml` green

Work through `docs/DESKTOP_APP_IMPLEMENTATION_CHECKLIST.md` in order. When
something in the SDK is missing for a screen, add it to the SDK
(`sdk/rust/hashgram-sdk`) with tests rather than working around it in the
app, and note it in `docs/HASHGRAM_ONE_AI_HANDOFF.md`.
