# Build Prompt: Hashgram Windows Desktop Application

A specification you can hand to an engineer or a coding model to build the
Hashgram Windows client.

**Every API in this document exists.** The endpoint paths were extracted from
`proto/hashgram/*/v1/query.proto`, the transaction types from `tx.proto`, and
the CLI commands from `--help` on built binaries. Nothing is invented.

Where a capability is not yet built, this document says so and tells you to
leave it out rather than describing an API to code against. A client written
against an imaginary endpoint compiles, ships, and fails in the user's hands.

---

## 0. Read this first

### What Hashgram is today

A blockchain with a fixed 1,000,000,000 HASH supply, a finite reward reserve,
an on-chain identity registry storing public keys only, a username registry
with confusable-character defence, and a useful-service reward system.

### What Hashgram is not yet

**There is no messaging, no social feed, no media, and no calls.** The
peer-to-peer layer that would carry them is Phase 2 and is unbuilt: no
end-to-end encryption, no envelope store, no blob storage, no call
signalling.

So **do not build a messenger.** Build a wallet, an identity manager and a
node console. Those have real APIs behind them. When Phase 2 lands, this
document will be extended with the interfaces that actually exist then.

If a specification tells you to build a chat screen against
`/hashgram/messaging/v1/send`, that endpoint does not exist and the
specification is wrong.

### The one rule that must not be broken

**The application must never transmit, log, or write to disk unencrypted the
user's private key or mnemonic.**

Keys are generated on the device, encrypted at rest with a key derived from
the user's passphrase, and used only to sign locally. There is no server-side
key escrow in Hashgram — that is a deliberate design property, and it means a
client that leaks a key has caused an unrecoverable loss.

---

## 1. Platform and stack

| | |
| --- | --- |
| Target | Windows 10 22H2 and later, x64 and ARM64 |
| Recommended stack | .NET 8, C#, WinUI 3 (Windows App SDK) |
| Alternative | Avalonia if you want the same code to run on Linux and macOS later |
| Not recommended | Electron. A wallet shipping a browser engine is a large attack surface for a small convenience. |

### Key storage

Use **DPAPI** (`ProtectedData` with `DataProtectionScope.CurrentUser`) as an
outer layer over a passphrase-derived key. Two layers because each covers the
other's gap: DPAPI alone is transparent to anything running as the user, and a
passphrase alone is exposed if the file is copied off the machine.

```csharp
// Argon2id for the passphrase, then DPAPI over the result.
// Argon2id parameters: 64 MiB memory, 3 iterations, 4 lanes.
// PBKDF2 is acceptable only if Argon2 is genuinely unavailable, and then at
// 600,000 iterations or more.
```

Never `Registry`, never plain `AppData`, never a file next to the executable.

---

## 2. The interfaces you have

### 2.1 CometBFT RPC — port 26657

Standard CometBFT. Use it for chain status and for broadcasting transactions.

| Endpoint | Use |
| --- | --- |
| `GET /status` | Height, chain id, catching-up state, node info |
| `GET /net_info` | Connected peers |
| `GET /abci_info` | Application version |
| `GET /validators` | The current validator set |
| `POST /broadcast_tx_sync` | Submit a signed transaction |
| `GET /tx?hash=0x…` | Look up a transaction result |
| `GET /genesis_chunked?chunk=N` | Fetch the genesis, in chunks |

**A trap that will cost you a day if you miss it.** CometBFT re-serialises the
genesis it serves: it drops the SDK's `app_name` and `app_version` fields,
renders `initial_height` as a string, and emits compact rather than indented
JSON. **The SHA-256 of the RPC response never equals the SHA-256 of the
genesis file.** Do not build a "verify my genesis" feature by hashing the RPC
response and comparing it to a published hash — it will always mismatch. Verify
the **chain id** instead, or hash a genesis file the user downloaded
separately.

`hashgramctl network-info` made exactly this mistake and warned on every
healthy node.

### 2.2 Cosmos SDK REST — port 1317

| Endpoint | Use |
| --- | --- |
| `/cosmos/bank/v1beta1/balances/{address}` | Balances |
| `/cosmos/bank/v1beta1/supply/by_denom?denom=uhash` | Total supply |
| `/cosmos/auth/v1beta1/accounts/{address}` | Account number and sequence, and the vesting schedule for a vesting account |
| `/cosmos/staking/v1beta1/delegations/{address}` | Delegations |
| `/cosmos/staking/v1beta1/validators` | Validators |
| `/cosmos/distribution/v1beta1/delegators/{address}/rewards` | Pending staking rewards |
| `/cosmos/gov/v1/proposals` | Governance proposals |
| `/cosmos/tx/v1beta1/txs?events=…` | Transaction history |

### 2.3 Hashgram REST — port 1317

The 39 endpoints that exist. This is the complete list.

**Network**

```text
GET /hashgram/network/v1/info
GET /hashgram/network/v1/fork_isolation
GET /hashgram/network/v1/signing_domain/{purpose}
```

**Founder economics**

```text
GET /hashgram/founder/v1/params
GET /hashgram/founder/v1/revenue
GET /hashgram/founder/v1/beneficiary_history
GET /hashgram/feerouter/v1/params
GET /hashgram/feerouter/v1/totals
GET /hashgram/feerouter/v1/service_revenue
```

**Treasury**

```text
GET /hashgram/treasury/v1/reserves
GET /hashgram/treasury/v1/reserve/{name}
GET /hashgram/treasury/v1/disbursements
```

`{name}` is one of `treasury`, `growth`, `dev_grants`, `liquidity`.

**Usernames**

```text
GET /hashgram/username/v1/params
GET /hashgram/username/v1/lookup/{name}
GET /hashgram/username/v1/reverse/{owner}
GET /hashgram/username/v1/availability/{name}
GET /hashgram/username/v1/registrations
```

**Identity**

```text
GET /hashgram/identity/v1/identity/{address}
GET /hashgram/identity/v1/devices/{address}
GET /hashgram/identity/v1/device/{address}/{device_id}
GET /hashgram/identity/v1/resolve_device_key
GET /hashgram/identity/v1/recovery/{root_address}
GET /hashgram/identity/v1/identities
```

**Welcome rewards**

```text
GET /hashgram/welcome/v1/params
GET /hashgram/welcome/v1/status
GET /hashgram/welcome/v1/tiers
GET /hashgram/welcome/v1/claim/{subject}
GET /hashgram/welcome/v1/claims
```

**Useful-service rewards**

```text
GET /hashgram/serviceproof/v1/params
GET /hashgram/serviceproof/v1/reserve
GET /hashgram/serviceproof/v1/provider/{operator}
GET /hashgram/serviceproof/v1/providers
GET /hashgram/serviceproof/v1/epoch/current
GET /hashgram/serviceproof/v1/epoch/{number}
GET /hashgram/serviceproof/v1/rewards/{operator}
GET /hashgram/serviceproof/v1/assignments/{provider}
GET /hashgram/serviceproof/v1/challenges/{provider}
GET /hashgram/serviceproof/v1/fraud/{provider}
GET /hashgram/serviceproof/v1/emission_schedule
```

### 2.4 gRPC — port 9091

The same services, as gRPC. Generate a C# client from
`proto/hashgram/*/v1/*.proto` with `Grpc.Tools`. Prefer gRPC over REST for
anything you poll: it is a persistent connection and typed at compile time,
so a renamed field is a build error rather than a null at runtime.

### 2.5 Ports and exposure

**26657, 1317 and 9091 are bound to localhost on a properly configured node,
and `hashgramctl mainnet-preflight` fails the launch if the admin RPC is
publicly reachable.**

So the desktop client connects to:

- **A local node** the user runs, over localhost, or
- **A public RPC endpoint** an operator chose to expose, over HTTPS

Design for both from the start. Default to a configurable endpoint list with a
health indicator per endpoint, and never hard-code a single hostname: that
would make one operator load-bearing for every user.

---

## 3. Transactions

### 3.1 Signing

Standard Cosmos SDK signing. `SIGN_MODE_DIRECT`, secp256k1, BIP-44 coin type
**118**, address prefix **`hash`**.

```text
m/44'/118'/0'/0/0
```

Coin type 118 is Cosmos's, chosen so that standard hardware wallets and
keyring tooling work unmodified. That is a feature you should use: support
Ledger from the first release, because the users with the most at stake are
exactly the ones who will not paste a mnemonic into a desktop app.

Use `NBitcoin` for BIP-32/39/44 and a maintained secp256k1 binding. Do not
implement key derivation yourself.

### 3.2 The 32 transaction types that exist

**Standard Cosmos:** `MsgSend`, `MsgMultiSend`, `MsgDelegate`, `MsgUndelegate`,
`MsgBeginRedelegate`, `MsgWithdrawDelegatorReward`,
`MsgWithdrawValidatorCommission`, `MsgSetWithdrawAddress`, `MsgVote`,
`MsgDeposit`, `MsgSubmitProposal`, `MsgGrant`, `MsgRevoke`, `MsgExec`,
`MsgGrantAllowance`, `MsgRevokeAllowance`.

**Hashgram:**

| Module | Messages | Who can send |
| --- | --- | --- |
| `founder` | `MsgClaimFounderRevenue` | Anyone. Permissionless. |
| `founder` | `MsgUpdateParams` | Governance only |
| `username` | `MsgRegister`, `MsgRenew`, `MsgTransfer`, `MsgSetTransferable`, `MsgRelease` | The owner |
| `identity` | `MsgCreateIdentity`, `MsgAddDevice`, `MsgRevokeDevice`, `MsgRotateRootKey`, `MsgSetRecoveryConfig`, `MsgInitiateRecovery`, `MsgApproveRecovery`, `MsgCancelRecovery`, `MsgExecuteRecovery`, `MsgRevokeIdentity` | The identity owner or a guardian |
| `welcome` | `MsgClaimWelcome` | The subject, with an attestation |
| `serviceproof` | `MsgRegisterProvider`, `MsgUpdateProvider`, `MsgSubmitReceipts`, `MsgAnswerChallenge`, `MsgUnjail`, `MsgBeginUnbonding`, `MsgWithdrawBond`, `MsgAssignStorage`, `MsgReleaseStorage` | The provider operator |
| `treasury` | `MsgSpend` | Governance only |
| `feerouter`, `serviceproof` | `MsgUpdateParams` | Governance only |

Build UI for the ones a desktop user actually sends: `MsgSend`, the staking
messages, the username messages, the identity messages, `MsgVote`,
`MsgClaimWelcome`. Do not build UI for governance-only messages; show their
effects, not a button that will always fail.

### 3.3 Fees

Transaction fees are paid in `uhash`. Estimate gas with the SDK's simulate
endpoint rather than guessing, and show the user the fee **in HASH with its
uhash value alongside**, because a fee of "2500" is meaningless and "0.0025
HASH" is what they need to see.

**Say plainly in the UI that a transfer is not taxed.** If the user sends 100
HASH, the recipient receives 100 HASH; the fee is separate and goes to
validators, with 1% of it to the Founder. Users arriving from chains with
transfer taxes will assume otherwise, and the confirmation screen is the right
place to correct that.

---

## 4. Screens to build

### 4.1 Onboarding

Three paths: create a new wallet, restore from a mnemonic, connect a Ledger.

**Creating a wallet**

1. Generate 24 words with 256 bits of entropy from `RNGCryptoServiceProvider`.
2. Show them **once**, with an explicit warning.
3. **Require the user to re-enter three words at random positions** before
   continuing. Not all 24 — that trains people to screenshot. Three positions
   proves they wrote it down without being so tedious they skip it.
4. Show the derived address and tell them to record it separately.
5. Then, and only then, take a passphrase and encrypt the key at rest.

**Be accurate about BIP-39's checksum.** It catches most single-word errors and
typos. It does **not** reliably catch two words being swapped, because a swap
can produce a valid checksum. So on restore, show the derived address and ask
the user to confirm it matches what they recorded. Do not tell them the
mnemonic is "verified" merely because it parsed.

**Never** offer to email, back up to a cloud service, print, or copy the
mnemonic to the clipboard. If you must allow a clipboard copy of the address,
that is fine; the mnemonic, never.

### 4.2 Wallet

- Balance in HASH, with the uhash value available on hover or in a detail row
- Spendable versus vesting, for an account with a vesting schedule. Read the
  schedule from `/cosmos/auth/v1beta1/accounts/{address}` and render the
  next unlock date, because "why can't I send my own coins" is otherwise the
  most common support question a vesting account generates
- Send, with address validation against the `hash` Bech32 prefix and a
  checksum check before the confirm button enables
- Username resolution in the recipient field: `@alice` resolves through
  `/hashgram/username/v1/lookup/alice`
- Transaction history from `/cosmos/tx/v1beta1/txs`
- A confirmation screen that shows amount, recipient, fee, and the total
  leaving the account, as separate lines

**Confusable-name warning.** When a user types a username, also query
`/hashgram/username/v1/availability/{name}`. The chain refuses to register a
name whose confusable skeleton collides with an existing one, but a user can
still be handed a *visually similar registered* name in a message. If the
resolved address is not one the user has transacted with before, say so.

### 4.3 Staking

- Validator list with voting power, commission and jailed status
- Delegate, undelegate, redelegate
- Pending rewards and a claim action
- **Show the 21-day unbonding period before the user confirms**, not after.
  Undelegating and then discovering the funds are locked for three weeks is
  the single worst surprise in Cosmos staking.
- Show slashing risk plainly: 5% for double signing, 0.01% for downtime

### 4.4 Identity and devices

This is where the desktop client is genuinely useful, because managing
identities on a phone is worse.

- Create a root identity: `MsgCreateIdentity`
- List authorised devices: `/hashgram/identity/v1/devices/{address}`
- Authorise a device: `MsgAddDevice`, with a certificate signed by the root key
- Revoke a device: `MsgRevokeDevice`
- Rotate the root key: `MsgRotateRootKey`
- Configure recovery guardians: `MsgSetRecoveryConfig`

**Only public keys go on chain.** Say so in the UI. Users need to understand
that authorising a device publishes its public key and nothing else, and that
Hashgram cannot recover their account because nobody but them has the private
key.

**Recovery has a mandatory delay**, and the delay exists so that a user whose
guardians are being socially engineered has time to cancel. Show the countdown
and a prominent cancel action.

### 4.5 Usernames

- Search and availability, with the confusable result shown as *unavailable
  with a reason*, not just unavailable
- Register, renew, transfer, release
- Show the expiry date and the grace period, because a name that expires
  silently is a name someone else registers

### 4.6 Founder verification

A screen that lets **anyone** check the Founder claims. Not an admin panel — a
transparency panel, available to every user.

- Configured fee: `/hashgram/founder/v1/params` → must be 100 bps
- Accrued, paid and pending: `/hashgram/founder/v1/revenue`
- Beneficiary history: `/hashgram/founder/v1/beneficiary_history`
- Realised share: `founder_share / total_qualifying` from
  `/hashgram/feerouter/v1/totals`, rendered in basis points

Show the realised figure next to the configured one. That is the number that
demonstrates the 1% is what the chain **did**, not merely what it is set to.

State on this screen that the share applies to protocol fee revenue only and
never to transferred principal.

### 4.7 Supply

- Total supply from `/cosmos/bank/v1beta1/supply/by_denom?denom=uhash`
- The 1,000,000,000 HASH ceiling
- **Say that there is no mint module**, so inflation is absent rather than set
  to zero
- The genesis allocation table
- The service reserve and its depletion: `/hashgram/serviceproof/v1/reserve`
  and `/hashgram/serviceproof/v1/emission_schedule`

### 4.8 Node console (optional, for operators)

Only if the user runs a node locally.

- Status from CometBFT `/status`, peers from `/net_info`
- Provider registration and earnings from the `serviceproof` endpoints
- Storage assignments and challenge history

Do **not** try to control systemd from the GUI. Shelling out to `hashgramctl
start` from a desktop app means either running the app elevated or wiring up a
privileged helper service, and both are a large security cost for a button.
Show status and print the command to run.

---

## 5. Things that will bite you

Collected from problems that actually occurred while building the chain.

**The genesis hash from RPC never matches the file hash.** Covered in §2.1.
Verify the chain id.

**`cometbft_p2p_peers` does not exist until the node's first peer event.** If
you read metrics, an absent series means "never peered", which is a different
state from zero peers. Render them differently.

**There is no missed-blocks metric.** CometBFT does not export one. Derive
validator signing health from the gap between the chain height and the
validator's last signed height.

**Amounts are strings in JSON, not numbers.** `uhash` reaches 10^15, which
exceeds the exact-integer range of a double. Parse into `System.Numerics.
BigInteger` or `decimal`, never `double` or `float`. A wallet that displays a
balance wrong because of float rounding is a wallet nobody trusts again.

**Account sequence numbers must be fetched fresh before each transaction.**
Two transactions signed with the same sequence means the second is rejected.
Refetch, do not cache.

**Bech32 addresses use the `hash` prefix.** A `cosmos1…` address is not a
Hashgram address. Validate the prefix and the checksum before enabling send.

**The expedited governance voting period must be shorter than the regular one,
and its deposit larger.** If you build a proposal form, enforce that; the SDK
rejects the reverse and the error is unhelpful.

---

## 6. Security requirements

Non-negotiable.

1. **Never transmit a private key or mnemonic.** Not to a server, not to
   telemetry, not to a crash reporter.
2. **Never log a private key or mnemonic**, at any log level, in any build.
   Add a test that greps your own log output for the mnemonic used in the
   test. Hashgram does this for its own logging policy and the check
   self-tests against a deliberate violation.
3. **Encrypt keys at rest** with Argon2id plus DPAPI. Zero the plaintext key
   material after use — and know that .NET's GC makes this imperfect, so also
   minimise how long a key is in managed memory.
4. **Sign the installer** with an Authenticode certificate. An unsigned wallet
   installer trains users to click through the warning that would otherwise
   have protected them from a fake one.
5. **Pin the certificate** for any HTTPS RPC endpoint you ship as a default.
6. **Verify update signatures** before applying them. An auto-updater on a
   wallet is a remote code execution path with a user base attached.
7. **Show the network prominently.** A devnet build must be visually
   unmistakable — a coloured banner reading DEVNET, not a small label. Users
   have sent real funds to test networks.
8. **Clear the clipboard** after a timeout if you ever copy anything sensitive
   to it.
9. **No analytics on transaction contents.** Amounts, addresses and
   counterparties are not telemetry.

---

## 7. Deliberately out of scope

Because the APIs do not exist. Adding them means inventing an interface, and
the client will then have to be rewritten when the real one lands.

- Messaging of any kind
- Social feed, posts, reels, stories, channels
- Media upload, storage or playback
- Voice and video calls
- Contact discovery beyond username lookup
- Group management

When Phase 2 is built, this document gains a section describing the real
interfaces. Until then, a Hashgram desktop client is a wallet, an identity
manager and a node console — and those three, done well, are worth more than
four more done against imagined APIs.

---

## 8. Definition of done

- [ ] Create, restore and Ledger onboarding all work
- [ ] Three random words re-entered before a new mnemonic is accepted
- [ ] Restore shows the derived address for the user to compare
- [ ] Keys encrypted with Argon2id plus DPAPI; never logged or transmitted
- [ ] Balance shows spendable versus vesting, with the next unlock date
- [ ] Send validates the `hash` prefix and the Bech32 checksum
- [ ] `@username` resolves in the recipient field
- [ ] The confirmation screen states that transfers are untaxed
- [ ] Staking shows the 21-day unbonding period **before** confirming
- [ ] Devices can be listed, authorised and revoked
- [ ] Recovery shows the delay countdown and a cancel action
- [ ] The Founder screen shows configured **and realised** basis points
- [ ] The supply screen states that there is no mint module
- [ ] All amounts parsed as `BigInteger` or `decimal`, never floating point
- [ ] Sequence numbers refetched before every transaction
- [ ] A devnet build is unmistakable at a glance
- [ ] The installer is Authenticode signed
- [ ] A test asserts that no log line contains the test mnemonic
- [ ] No messaging, social or media screens

---

## 9. Where to look in the repository

| What | Where |
| --- | --- |
| Endpoint definitions | `proto/hashgram/*/v1/query.proto` |
| Transaction definitions | `proto/hashgram/*/v1/tx.proto` |
| Address prefix, coin type, denominations | `app/params/params.go` |
| Network identity and signing domains | `app/params/network.go` |
| Canonical signing preimages | `app/canonical/encode.go` |
| A working client to copy behaviour from | `cmd/hashgram-test-client/` |
| Cross-language signing vectors | `node/testdata/signing-vectors.json` |

`cmd/hashgram-test-client` is the most useful of these. It is a real client
that exercises wallet, staking and Founder verification against a running
chain, and its output shows exactly what each query returns.

For signing, `node/testdata/signing-vectors.json` gives you 90 preimages and
digests to test your implementation against. If your client ever signs a
Hashgram-specific object — a device certificate, for instance — check it
against those vectors rather than against your reading of the specification.
