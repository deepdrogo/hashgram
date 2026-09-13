# Hashgram One for Windows

One identity. One inbox. One vault. One network.

Encrypted mail, an encrypted drive, a signed public feed, private circles,
role-based Spaces, provider earnings and the wallet — one application, one
24-word identity, talking to the Hashgram peer-to-peer network and never to
one server. Built to `docs/HASHGRAM_ONE_ARCHITECTURE.md`; implementation
notes in `docs/HASHGRAM_ONE_AI_HANDOFF.md`, progress against
`docs/DESKTOP_APP_IMPLEMENTATION_CHECKLIST.md`.

Navigation order is fixed: Mail · Drive · Feed · People · Spaces · Earn ·
Wallet · Network · Settings. Mail is home.

## Stack

- **Tauri 2** (Rust core, WebView2 UI); **SolidJS 1.9 + TypeScript 5.9 +
  Vite 7 + Tailwind v4**; `@kobalte/core` primitives, `@tanstack/solid-virtual`
  lists, `lucide-solid` icons, `vitest`.
- The in-repo **`hashgram-sdk`** (`HashgramOne` facade) is the only way the
  app talks to the network: vault and keys, MLS mail, Drive manifests and
  objects, People, Feed/Circles, Spaces, Earn, Wallet, and chain reads over
  the P2P relay (`chain_api = None`). The desktop crate holds no protocol or
  cryptography of its own.
- **rusqlite** (WAL) only for UI caches, with XChaCha20-Poly1305 column
  sealing under a key kept inside the vault; **DPAPI + Windows Hello**
  (`winsec.rs`) for day-to-day unlock; Argon2id for the passphrase.
- Dark-first design: black, near-blacks, greys, white and one desaturated
  blue-grey accent (`src/styles/tokens.css`); Inter bundled (OFL); English
  first, Georgian second (`src/lib/i18n.ts`).

## What the webview never sees

The mnemonic (except the one-time onboarding display and the restore entry
field), the root private key, the device seed, Drive object/manifest keys,
MLS state, the local-store key and the vault passphrase after unlock. Tauri
commands return *views* (`src-tauri/src/views.rs`) and take ids or plain
inputs; `tests/forbidden-fields.test.ts` and the Rust view tests check that
no key material crosses the boundary, `src-tauri/tests/no_plaintext.rs`
checks that nothing the app writes to disk is readable.

## Layout

```
apps/desktop/
  src/                 SolidJS app: routes/ (mail, drive, feed, People, spaces, Earn, wallet, Network, Settings, Help,
                       Onboarding, Lock), components/, lib/ (ipc, store, i18n, format, updates, devshim)
  src-tauri/src/       Rust: session (vault, link, sync loop), cmd_* per area, views, settings, db, winsec, notify,
                       node_manager (Earn), chain_proxy (loopback gateway for a node on this PC)
  src-tauri/tests/     no_plaintext (disk audit), devnet (two backends over a real swarm and mock gateway)
  service/             hashgram-node-service.exe: supervising Windows service / task wrapper
  help/                In-app Help pages (Markdown, bundled)
  tests/               vitest: rules, palette, format parity, forbidden fields, component behaviour
  release.ps1          Reproducible release: typecheck, tests, rule checks, sidecars, NSIS/MSI, SHA-256, latest.json
```

## Develop

```powershell
pnpm install                      # pnpm 12 (hoisted linker; see pnpm-workspace.yaml)
cd ..\..\node; cargo build -p hashgram-node -p hashgram-node-service; cd ..\apps\desktop
pnpm tauri dev                    # HASHGRAM_DESKTOP_HOME=<dir> for a throwaway profile
pnpm test                         # vitest
pnpm exec tsc --noEmit
cd ..\..\node; cargo test -p hashgram-desktop            # unit + no_plaintext
cd ..\..\node; cargo test -p hashgram-desktop --test devnet   # needs hashgram-node + mock-gateway debug builds
```

`HASHGRAM_LIGHT_KDF=true` (only honoured together with `HASHGRAM_DESKTOP_HOME`)
makes throwaway profiles unlock fast. `pnpm dev` alone serves the UI in a
browser against an in-memory shim (`src/lib/devshim.ts`, DEV builds only) for
layout work; nothing in it ships.

Shortcuts: Ctrl+K search · c compose · j/k move · Enter open · r reply ·
e archive · Ctrl+L lock · Ctrl+Shift+P performance panel · F1 help.

## Release

```powershell
pwsh .\release.ps1                # artefacts + SHA256SUMS.txt + RELEASE_NOTES.txt in dist/desktop
```

The script runs the typecheck, vitest, the rule checks (no hardcoded IPv4
outside loopback, no CDN/analytics references), builds the sidecars, runs the
desktop crate tests, `tauri build` (NSIS + MSI, per-user, no admin), then
checks the 45 MB installer budget.

Signing (owner's machine or CI secrets only):

- `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PATH` (+ `_PASSWORD`):
  minisign key for updater artefacts and `latest.json`. The public key is
  compiled in (`src-tauri/tauri.conf.json`).
- `HASHGRAM_CODESIGN_THUMBPRINT`: Authenticode. Architected but disabled
  until a certificate exists; when set, `signtool` signs the `.exe`/`.msi`
  and `HASHGRAM_CODESIGNED=1` is baked in so About shows **signed** instead
  of **unsigned preview**. In CI the optional secrets
  `HASHGRAM_CODESIGN_PFX_BASE64` + `HASHGRAM_CODESIGN_PFX_PASSWORD` import the
  certificate first.

### Self-update

The app fetches
`https://github.com/deepdrogo/hashgram_windows/releases/latest/download/latest.json`
(on start when enabled, and from Settings → Updates → Check now), verifies the
minisign signature against the compiled-in public key, downloads the NSIS
installer from that release and runs it passively for the current user.
Unsigned or foreign manifests are refused.

Publishing a new version:

1. bump `version` in `src-tauri/tauri.conf.json` and `package.json`
   (`src-tauri/Cargo.toml` too);
2. commit and `git tag vX.Y.Z && git push origin <branch> vX.Y.Z`;
3. `.github/workflows/desktop-release.yml` builds with `release.ps1`, signs
   with the `TAURI_SIGNING_PRIVATE_KEY` secret and uploads
   `Hashgram One_X.Y.Z_x64-setup.exe`, its `.sig`, the MSI, `SHA256SUMS.txt`
   and `latest.json` to the release. A tag whose release already carries a
   `latest.json` is left untouched.

The private key never enters the repository; losing it means installed apps
cannot verify future updates, so keep an offline copy.

## Rules enforced by tests

- Identity is 24 words only; login is restore; no server registration, no
  "forgot password".
- Nav order and names as above; the word "mining" never appears as a feature.
- No hardcoded server address anywhere under `apps/`; no IP literal but loopback.
- Amounts are strings/`u128` end to end; `format.test.ts` checks parity with
  the SDK's `format_hash`/`parse_amount`.
- Nothing plaintext (keys, mnemonic, mail, drafts, file names, Drive
  manifests) in any file the app writes.
- HTML mail renders only inside a `sandbox=""` iframe with a strict CSP.
- Notifications never contain subjects or file names.

## Data

`%LOCALAPPDATA%\Hashgram\data`: `keystore.json` (encrypted vault; a v0.1.x
`vault.json` is migrated in place on first start), `local.redb` (SDK
encrypted store), `hashgram.db` (sealed UI caches), `peerstore.json`,
`settings.json`, `cache\`, `tmp\` (wiped on lock), `logs\` (7 days, no
subjects, addresses or IPs), `node\` (when a node is run from this PC).
Uninstall keeps the data by default.
