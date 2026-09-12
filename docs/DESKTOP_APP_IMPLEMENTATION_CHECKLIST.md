# Hashgram One Desktop — Implementation Checklist

Ordered engineering stages for the desktop application described in
`DESKTOP_APP_MASTER_PROMPT.md`. Each stage ends with a runnable build and
its tests green. File paths refer to the actual repository layout.

## Stage 0 — Bootstrap

- [ ] Branch from `hashgram-one`; `cd node && cargo build -p hashgram-node -p hashgram-client -p hashgram-devtools`; run `scripts/testnet/hashgram-one-e2e.sh debug` and watch all 31 checks pass. Read `node/hashgram-client/src/one.rs` end to end.
- [ ] `apps/desktop`: `pnpm install`, `pnpm tauri dev` builds the old app once. Note the pieces to keep (§3 of the prompt).
- [ ] Remove `src/routes/{Messages,Calls,Reels,Channels,Home}.tsx`, `src-tauri/src/{chat,social_hub,commands_social}.rs`; make the crate compile with a stub command set.
- [ ] Bump `tauri.conf.json` and `package.json` to `0.2.0-dev`.

## Stage 1 — SDK binding

- [ ] `src-tauri/src/state.rs`: `AppState { one: Mutex<Option<HashgramOne>>, settings, db, lock_deadline, sync_task }`.
- [ ] `UiError { code, message, retryable }` + `From<SdkError>` mapping table (prompt §1.3).
- [ ] `open_one(settings, passphrase)` building `Config`/`Paths` (data dir from `paths.rs`), `chain_api = None`, Mainnet identity constants.
- [ ] Sync loop task: `round()` → `save()` → emit `sync:phase` / `sync:event` → sleep `next_delay()`; starts at unlock, stops at lock.
- [ ] Test: unit test for the error mapping; integration test opening a devnet account through `HashgramOne::with_account`.

## Stage 2 — Vault, onboarding, lock

- [ ] Reuse `Onboarding.tsx`, `Lock.tsx`, `winsec.rs`; commands `onboarding_*`, `unlock`, `lock`, `hello_*`, `change_passphrase`, `wipe_local_data`.
- [ ] `restore_from_backup` via `hashgram_sdk::backup::import_backup`; `backup_export`/`backup_inspect`.
- [ ] Auto-lock (default 15 min) drops `HashgramOne`.
- [ ] Tests: mnemonic never in any command return type except `onboarding_generate`; `no_plaintext.rs` extended to the new data files (`local.redb`).

## Stage 3 — Shell and navigation

- [ ] Left rail (Mail · Drive · Feed · People · Spaces · Earn · Wallet · Network · Settings), badges from `mail_counts` and request counts, sync indicator from `SyncPhase`, lock button, `Ctrl+K` global search shell, deep links.
- [ ] Design tokens (black/near-black/greys/white + one accent), Inter bundled, dark/light.
- [ ] Empty / loading / offline / error / locked state components shared by all screens.
- [ ] vitest: palette test, rules test (no `186.241`, no "mining").

## Stage 4 — Mail

- [ ] Commands `mail_*` (prompt §4) over `one.mail()`.
- [ ] Folder pane, virtualized list (threaded toggle), reading pane with `authenticated_sender`, External badge, BCC chip, receipts.
- [ ] Composer: recipient resolution chips, CC/BCC, attachments (file → `make_attachment`; Drive → `attach_from_drive` snapshot/live), read-receipt toggle, reply/reply-all/forward drafts, drafts autosave.
- [ ] Requests folder actions (Accept → `accept_request`, Block → `people_block`, Delete).
- [ ] HTML body in sandboxed iframe; linkified plain text.
- [ ] Attachment Save/Open (decrypt in Rust, temp file for Open).
- [ ] Keyboard shortcuts. Notifications on `SyncEvent::NewMail{folder: inbox|requests}` (no subject in the toast).
- [ ] Tests: composer chips, Requests actions, HTML sandbox, devnet A→B send/receive through command functions.

## Stage 5 — Drive

- [ ] Commands `drive_*` over `one.drive()`; upload reads the file in Rust; progress events; 256 MiB cap or streaming via `SegmentEncryptor` + `blob::upload_sealed`.
- [ ] Browser (breadcrumbs, list/grid, sort, context menu), drag-drop, sidebar (My Drive / Shared with me / Starred / Trash), usage bar with `dirty` badge.
- [ ] Share dialog (snapshot vs live, note, existing shares + revoke), Versions panel (restore, download version), Shared-with-me (open, download, save to Drive, folder shares), Rekey.
- [ ] Tests: share dialog, versions, upload/download round trip on the devnet.

## Stage 6 — People

- [ ] Commands `people_*`; search (resolve + local), lists, contact card, request/accept/reject, block/mute/trust, follow, send card, own profile editor.
- [ ] Tests: request → accept flow on the devnet through commands.

## Stage 7 — Feed and Circles

- [ ] Commands `feed_*`, `circles_*`; tabs Friends / Following / Circles / Explore (indexer-gated); composer (public + circle + poll); post view; profile page.
- [ ] Tests: poll tally rendering from `Item.votes`; Explore disabled without indexer URL.

## Stage 8 — Spaces

- [ ] Commands `spaces_*`; list, space view tabs (Overview, Posts, Drive tree, Members, Mail), invite dialog with roles, role gating from `my_role` (table in `docs/SPACES.md`), owner transfer confirmation.
- [ ] Tests: role gating matrix (Guest/Member/Admin/Owner) against `hashgram_app::space::State::apply` outcomes.

## Stage 9 — Earn

- [ ] Commands `earn_*` over `one.provider()`; lifecycle sentences; earnings; register/update/unbond/withdraw; node manager from the previous app (`node_manager.rs`, `commands_node.rs`).
- [ ] Tests: `Lifecycle` → sentence mapping.

## Stage 10 — Wallet

- [ ] Commands `wallet_*`; balance with verification; send with preview; receive QR; history; staking; usernames with availability reasons; identity/devices management (`devices_*`).
- [ ] Tests: `parse_amount`/`format_hash` parity in TS (`format.test.ts`).

## Stage 11 — Network

- [ ] Commands `network_*`; peers, rejected, height/verification, supply, leaderboards + stats from the configured indexer, diagnostics export (no IPs, no contact addresses).

## Stage 12 — Notifications and offline/sync

- [ ] Event bridge from `SyncEvent` to UI stores and OS notifications; offline banner; retry actions; Backoff display.
- [ ] Tests: simulate `Backoff` and `Idle` phases; ensure UI renders local data with no peers (start app with node stopped).

## Stage 13 — Settings

- [ ] Network (indexer URL, gateway address), Security (passphrase, Hello, auto-lock, backup), Devices, Notifications, Mail, Appearance, Updates, Advanced (leases, rekey all, wipe), About.

## Stage 14 — Security hardening

- [ ] Forbidden-field test over command return types; sandbox test; auto-lock test; logging policy check (`scripts/dev/check-logging.sh` adapted); CSP and capabilities review; `cargo audit`; `pnpm audit`.

## Stage 15 — Installer and release

- [ ] `release.ps1` → NSIS + MSI; minisign; `latest.json`; `desktop-release.yml` tag guard; Authenticode step designed and gated on a certificate secret.
- [ ] About page shows version, genesis hash, "signed"/"unsigned preview".

## Stage 16 — QA

- [ ] Run the Definition of Done list in the prompt on a clean Windows 11 VM.
- [ ] Two-device test: second install adds itself via pasted device key; receives Drive keyring and contacts after sync.
- [ ] Mainnet smoke: create identity (unregistered), Drive round trip through the genesis node, balance 0 shown without error, Network page shows the genesis peer.
- [ ] Record results in `docs/HASHGRAM_ONE_AI_HANDOFF.md` ("Desktop readiness").
