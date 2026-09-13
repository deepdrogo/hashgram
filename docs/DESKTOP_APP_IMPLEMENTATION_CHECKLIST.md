# Hashgram One Desktop — Implementation Checklist

Ordered engineering stages for the desktop application described in
`DESKTOP_APP_MASTER_PROMPT.md`. Each stage ends with a runnable build and
its tests green. File paths refer to the actual repository layout.

Status as of 2026-09-13 (branch `hashgram-one`, app v0.2.2): stages 0–15
done; stage 16 partly (see the notes). Details in
`HASHGRAM_ONE_AI_HANDOFF.md` §12.

## Stage 0 — Bootstrap

- [x] Branch from `hashgram-one`; `cd node && cargo build -p hashgram-node -p hashgram-client -p hashgram-devtools`; run `scripts/testnet/hashgram-one-e2e.sh debug` and watch all 31 checks pass. Read `node/hashgram-client/src/one.rs` end to end.
- [x] `apps/desktop`: `pnpm install`, `pnpm tauri dev` builds the old app once. Note the pieces to keep (§3 of the prompt).
- [x] Remove `src/routes/{Messages,Calls,Reels,Channels,Home}.tsx`, `src-tauri/src/{chat,social_hub,commands_social}.rs`; make the crate compile with a stub command set. (Also removed `commands.rs`, `net/`, `chain_access/`, Founder/Supply/Profile/RunNode routes and old wallet screens; `main` merged in first for the Windows p2p fixes.)
- [x] Bump `tauri.conf.json` and `package.json` to 0.2.0 (`src-tauri/Cargo.toml` too).

## Stage 1 — SDK binding

- [x] `src-tauri/src/state.rs`: `AppState { one: Mutex<Option<HashgramOne>>, settings, db, session, link, sync, sync_task, pending_tx, sync_wake, .. }`.
- [x] `UiError { code, message, retryable }` + `From<SdkError>` mapping table (prompt §1.3) in `error.rs`.
- [x] `session::open_session(settings, passphrase)` building `Config`/`Paths` (data dir from `paths.rs`), `chain_api = None`, Mainnet identity constants; the link is started at boot and shared (`HashgramOne::with_link`, added to the SDK).
- [x] Sync loop task: `round()` → `save()` → emit `sync:phase` / `sync:event` / `sync:tick` → sleep `next_delay()`; starts at unlock, stops at lock; `sync_now` wakes it.
- [x] Test: unit test for the error mapping; integration test opening two devnet accounts through `HashgramOne::with_link` (`tests/devnet.rs`).

## Stage 2 — Vault, onboarding, lock

- [x] Reuse `winsec.rs`; rewritten `Onboarding.tsx`, `Lock.tsx`; commands `onboarding_*`, `unlock`, `lock`, `hello_*`, `change_passphrase`, `wipe_local_data`.
- [x] `restore_from_backup` via `hashgram_sdk::backup::import_backup`; `backup_export` (via the new `export_backup_contents`) / `backup_inspect`.
- [x] Auto-lock (default 15 min, housekeeping every 15 s) drops `HashgramOne`, aborts the sync task, wipes `tmp/`.
- [x] Tests: mnemonic never in any command return type except `onboarding_generate` (`forbidden-fields.test.ts`, Rust view tests); `no_plaintext.rs` covers `keystore.json`, `local.redb` (mail record, draft, Drive manifest), Drive ciphertext, keyring, UI cache, settings.

## Stage 3 — Shell and navigation

- [x] Left rail (Mail · Drive · Feed · People · Spaces · Earn · Wallet · Network · Settings), badges from `mail_counts` and request counts, sync indicator from `SyncPhase`, lock button, `Ctrl+K` global search shell, deep links.
- [x] Design tokens (black/near-black/greys/white + one accent), Inter bundled, dark/light, English + Georgian.
- [x] Empty / loading / offline / error / locked state components shared by all screens (`components/States.tsx`).
- [x] vitest: palette test, rules test (no IPv4 literal, no "mining", nav order, 24 words, `chain_api = None`).

## Stage 4 — Mail

- [x] Commands `mail_*` (prompt §4) over `one.mail()`.
- [x] Folder pane, virtualized list (threaded toggle), reading pane with `authenticated_sender`, External badge, BCC chip, receipts.
- [x] Composer: recipient resolution chips, CC/BCC, attachments (file → `make_attachment`; Drive → `attach_from_drive` snapshot/live), read-receipt toggle, reply/reply-all/forward drafts, drafts autosave (staged in the SDK store under a client draft id).
- [x] Requests folder actions (Accept → `accept_request`, Block → `people_block`, Delete).
- [x] HTML body in sandboxed iframe (`sandbox=""`, CSP via srcdoc); linkified plain text.
- [x] Attachment Save/Open (decrypt in Rust, temp file for Open, wiped on lock).
- [x] Keyboard shortcuts. Notifications on `SyncEvent::NewMail{folder: inbox|requests}` (no subject in the toast).
- [x] Tests: composer chips, Requests actions, HTML sandbox, devnet A→B send/receive through command-level SDK calls.

## Stage 5 — Drive

- [x] Commands `drive_*` over `one.drive()`; upload reads the file in Rust; progress events; 256 MiB cap (streaming wrapper is SDK TODO).
- [x] Browser (breadcrumbs, list/grid, sort, context menu), drag-drop, sidebar (My Drive / Shared with me / Starred / Trash), usage bar with `dirty` badge.
- [x] Share dialog (snapshot vs live, note, existing shares + revoke), Versions panel (restore, download version), Shared-with-me (open, download, save to Drive, folder shares), Rekey.
- [x] Tests: upload/download round trip and live share update on the devnet; Mainnet 27 MiB upload through the genesis node.

## Stage 6 — People

- [x] Commands `people_*`; search (resolve + local), lists, contact card, request/accept/reject, block/mute/trust, follow, send card, own profile editor.
- [x] Tests: request → accept flow on the devnet (mail Requests → accept).

## Stage 7 — Feed and Circles

- [x] Commands `feed_*`, `circles_*`; tabs Friends / Following / Circles / Explore (indexer-gated); composer (public + circle + poll); post view; profile page.
- [x] Tests: poll tally rendering from `Item.votes`; Explore disabled without indexer URL.

## Stage 8 — Spaces

- [x] Commands `spaces_*`; list, space view tabs (Overview, Posts, Drive tree, Members, Mail), invite dialog with roles, role gating from `my_role` (table in `docs/SPACES.md`), owner transfer confirmation.
- [x] Tests: role gating matrix (Guest/Member/Admin/Owner) in `components.test.tsx`; devnet rule violation + promote; SDK fix for out-of-order Space events (`hashgram-app::space`).

## Stage 9 — Earn

- [x] Commands `earn_*` over `one.provider()`; lifecycle sentences; earnings; register/update/unbond/withdraw; node manager from the previous app (`node_manager.rs`).
- [x] Tests: `Lifecycle` → sentence mapping.

## Stage 10 — Wallet

- [x] Commands `wallet_*`; balance with verification; send with preview (`tx_preview`/`tx_submit`); receive QR; history; staking; usernames with availability reasons; identity/devices management (`devices_*`).
- [x] Tests: `parse_amount`/`format_hash` parity in TS (`format.test.ts`).

## Stage 11 — Network

- [x] Commands `network_*`; peers, rejected, height/verification, supply, leaderboards + stats from the configured indexer, diagnostics export (no IPs, no contact addresses); overview works while locked.

## Stage 12 — Notifications and offline/sync

- [x] Event bridge from `SyncEvent` to UI stores and OS notifications; offline banner; retry actions; Backoff display.
- [x] Tests: locked/offline semantics in `devnet.rs`; UI renders local data with no peers (Lock screen shows the link state, screens show `OfflineState`).

## Stage 13 — Settings

- [x] Network (indexer URL, gateway address), Security (passphrase, Hello, auto-lock, backup), Devices, Notifications, Mail, Appearance, Updates, Advanced (leases, rekey all, wipe), About (version, commit, genesis, signed / unsigned preview, licences).

## Stage 14 — Security hardening

- [x] Forbidden-field test over command return types; sandbox test; auto-lock housekeeping; logging policy check (`scripts/dev/check-logging.sh` now covers `apps/` and Rust `tracing`/TS `console`); CSP and capabilities review; `pnpm audit`; `cargo clippy -p hashgram-desktop`. (`cargo audit` is not installed on the build machine; run it in CI.)

## Stage 15 — Installer and release

- [x] `release.ps1` → typecheck, tests, rule checks, NSIS + MSI (per-user), minisign, `latest.json`, 45 MB budget; `desktop-release.yml` tag guard; Authenticode step designed and gated on a certificate secret (`HASHGRAM_CODESIGN_PFX_BASE64`).
- [x] About page shows version, genesis hash, "signed"/"unsigned preview" (`HASHGRAM_CODESIGNED` baked at build time).

## Stage 16 — QA

- [ ] Run the Definition of Done list in the prompt on a clean Windows 11 VM. (Run on the developer machine only: onboarding, lock/unlock, all screens, Drive upload, wallet/network reads on Mainnet.)
- [ ] Two-device test: second install adds itself via pasted device key; receives Drive keyring and contacts after sync. (Covered by the SDK e2e script and `devnet.rs` device flows; not yet with two installed apps.)
- [x] Mainnet smoke: create identity (unregistered), Drive round trip through the genesis node, balance 0 shown without error, Network page shows the genesis peer.
- [x] Record results in `docs/HASHGRAM_ONE_AI_HANDOFF.md` (§12).
