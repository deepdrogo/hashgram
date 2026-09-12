# Hashgram One — Multi-Device Security

Status: normative for the `hashgram-one` branch. Companion documents:
`PRIVACY_MODEL.md`, `MESSAGING.md`, `HASHGRAM_ONE_ARCHITECTURE.md` §13,
`THREAT_MODEL.md`.

This document states exactly what a user with several devices gets, what a
new device receives, what a lost or stolen device keeps, and where the
guarantees stop. The rules quoted from chain code are those in
`x/identity/keeper/identity.go` and `recovery.go`; the client-side behaviour
is that of `node/hashgram-identity`, `node/hashgram-mls` and
`sdk/rust/hashgram-sdk`. Where a behaviour depends on a device being online,
the text says so, because that is where most of the limitations live.

Spelling is British throughout.

---

## 1. Key hierarchy

From `node/hashgram-identity/src/lib.rs` and `hashgram-sdk::account`:

```text
Mnemonic (BIP-39)
  └─▶ Account key        secp256k1        hash1… address; pays fees; owns the identity on chain
        └─▶ Root key     ed25519          seed = blake3::derive_key("hashgram identity root key v1", account secret)
              │                           signs device certificates and root rotations; may live offline
              ├─▶ Device #1  ed25519      random seed, generated on the device, never leaves it
              ├─▶ Device #2  ed25519      …
              └─▶ Device #n  ed25519      (n ≤ MaxDevicesPerIdentity = 20 active)
```

| Key | Derivation | Where it lives | Signs |
| --- | --- | --- | --- |
| Account (secp256k1) | From the mnemonic | Vault of devices the user chose to give it to; hardware wallet possible | Chain transactions (fees, identity messages, transfers) |
| Root (ed25519) | Deterministically from the account secret | Same vaults as the account key; can be kept offline | Device certificates (`device-cert` purpose), root rotation |
| Device (ed25519) | Random per device | Only that device's vault | Everything the device does: MLS credential, mailbox fetch/ack, blob upload, social events, Space events, receipts |

Consequences of the derivation: **the mnemonic restores the account and the
root key** (§7.1) but never a device key. A device key that is lost is gone;
the remedy is revocation on chain, not recovery.

A *device-only* vault (`Account::create_device_only`) holds a device seed and
no account or root key. Such a device can mail, post, sync Drive and manage
Spaces, but cannot add or revoke devices, rotate the root, or spend HASH. The
user decides per device; a phone typically gets device-only, a laptop the
full set.

The vault (`node/hashgram-identity/src/vault.rs`) is one file, Argon2id
(64 MiB, t = 3, p = 1) → XChaCha20-Poly1305, `0600`, atomic write, zeroised
on drop. MLS group state is stored under `extra["mls_state"]`.

---

## 2. Device certificates and chain rules

A device joins an identity by `MsgAddDevice` carrying a `DeviceCertificate`
signed by the **root key**, not by the transaction signer. Whoever pays gas
cannot insert a device. The rules enforced in `AddDevice`:

| Rule | Value / behaviour | Source |
| --- | --- | --- |
| Active devices per identity | ≤ `MaxDevicesPerIdentity` = 20 | `x/identity/types/genesis.go` |
| Certificate expiry | `ExpiryHeight ≥ current height` and `≤ height + MaxCertificateAgeBlocks` (21,600 ≈ 24 h). The certificate must be *used* within a day of issue; the device authorisation itself does not expire | `identity.go` `AddDevice` |
| Rotation counter | `cert.RotationCount == identity.RotationCount`; a certificate from a superseded root is refused | `identity.go` |
| Device id reuse | A revoked device id is **never** reusable; a new id must be chosen | `identity.go` |
| Device key reuse | A device public key may be registered under one identity only, ever | `DeviceKeyInUse` |
| Domain separation | Certificate digest mixes network magic, protocol major version and the `device-cert` purpose; a devnet certificate does not authorise on Mainnet | `verifyCertificate` |
| Revocation | `MsgRevokeDevice` marks `Revoked = true`, `RevokedHeight`; the record is kept for attribution | `RevokeDevice` |
| Last device | Cannot revoke the last active device (`ErrLastDeviceRemove`): add a replacement first | `RevokeDevice` |
| Identity revocation | `RevokeIdentity` revokes every device and the identity itself; irreversible | `RevokeIdentity` |

Every node checks device authority against the chain before accepting a
signed request; every client checks it before adding a member to a group
and, in Hashgram One, on receipt of any application message (§4).

### 2.1 Device lifecycle

```text
                MsgAddDevice (root-signed cert, rotation == current)
   [absent] ───────────────────────────────────────────────▶ [ACTIVE]
                                                              │   │
             MsgRevokeDevice (not the last active one)        │   │  MsgRotateRootKey{revoke_existing=true}
                                                              │   │  ExecuteRecovery
                                                              ▼   ▼
                                                           [REVOKED]  (terminal; id and key never reusable)
```

---

## 3. Devices in MLS groups

Every conversation — direct chat, mail recipient set, Circle, Space, and the
user's **self-group** — is one MLS group. **Every device of every
participant is a member.** Adding a participant adds every active device the
chain lists for that address; a device never shares a private key with
another (`MESSAGING.md`).

Key packages: each device publishes one *last-resort* package plus a queue
of *one-time* packages (target 8 per store node) to the store nodes that
advertise its mailbox. A sender adding the device consumes a one-time
package if one is available, otherwise the last-resort package, which gives
weaker forward secrecy for that Welcome only.

MLS credentials are basic credentials `<address>/<hex device key>` whose
signature key **is** the on-chain device key. MLS therefore authenticates
"this message came from device D claiming address A"; the **chain** is what
says whether D is currently an active device of A. The SDK performs that
check when adding members and, in Hashgram One readers, on every inbound
application message (a message from a device the chain lists as revoked is
refused and reported, even though MLS decrypted it).

```text
Identity A (chain)                         MLS group G (members = devices)
 ├─ A.d1 ACTIVE   ──────────────────────▶   A.d1
 ├─ A.d2 ACTIVE   ──────────────────────▶   A.d2
 └─ A.d3 REVOKED  ─ ─ must be removed ─ ▶   A.d3   ◀── still a member until an
Identity B                                         honest member commits its removal (§5)
 └─ B.d1 ACTIVE   ──────────────────────▶   B.d1
```

The self-group is the group whose participant set is `{A}`. It carries
`DeviceSync` (`DriveKeyring`, `MailStateHint`, `ContactsSnapshot`), and a
reader **must** verify that the MLS sender address is its own address before
applying any of it (the proto says so; the SDK enforces it).

---

## 4. What a new device receives

MLS is epoch-based. A member added in epoch *e* holds the secrets of epochs
≥ *e* only. There is no key that decrypts earlier epochs, and store nodes
hold nothing older than 30 days anyway (envelopes are deleted on ack).

| Data | New device gets it? | How |
| --- | --- | --- |
| Membership of every group the identity is in | **Only after an existing member adds it** to each group; there is no "join all my groups" primitive in MLS | Existing devices add the new device on their next send to each group, or `devices::bootstrap_new_device` does it eagerly (below) |
| Mail received before it joined | **No** | Unless an existing device re-sends (below) |
| Mail received after it joined | Yes, as a full member | Normal delivery |
| Drive manifest key, drive id, latest manifest CID and revision | Yes | `DeviceSync.DriveKeyring` from an existing device in the self-group |
| The Drive tree itself | Yes | Fetches the manifest blob by CID and decrypts it with the manifest key; then any object on demand |
| Contacts, friend/block/mute state | Yes | `DeviceSync.ContactsSnapshot` |
| Mail flags (read, archived, starred…) | Only for messages it has | `DeviceSync.MailStateHint`; hints about messages it never received are kept and applied if the message later arrives via re-send |
| Circle and Space history | **No** for Circles unless `history_visible_to_new_members` and a member re-shares; Spaces: the event log is replayed from what the new device receives, so its role table is complete **only if an existing device re-sends the log** | `devices::bootstrap_new_device` |
| MLS state of the old devices | **Never**; each device has its own leaf and secrets | — |
| Account / root key | Only if the user chose a full vault on that device | User decision at setup |

### 4.1 `devices::bootstrap_new_device`

`hashgram-sdk::devices::bootstrap_new_device(new_device_pubkey)` runs on an
**existing** device after the new device's certificate is on chain and its
key packages are published. It:

1. adds the new device to the self-group and sends `DriveKeyring`,
   `ContactsSnapshot` and a `MailStateHint` covering the local flag state;
2. adds the new device to every other group the local device is a member of
   (one MLS commit per group, one Welcome to the new device);
3. re-sends, into the self-group, the local copies of mail from the last
   `bootstrap_mail_window` (default 30 days, configurable, bounded by
   `MAX_BOOTSTRAP_BYTES`) as ordinary `MailMessage`s marked
   `labels += "resent"`, so the new device has a usable Inbox;
4. re-sends the `SpaceEvent` log of every Space the local device is in
   (events are self-authenticating: signed by their actors, so a re-sent
   log is verified by the new device exactly as the original was);
5. for Circles with `history_visible_to_new_members = true`, re-sends the
   local post history; for others, nothing.

It does **not** and cannot: recover mail the existing device itself does not
have; add the new device to groups the existing device is not in (a group
containing only another of the user's devices, now lost, is unreachable);
transfer the existing device's MLS leaf. If the user has no existing device
online, the new device has an empty Drive and no groups until one comes
online or until recovery (§7).

### 4.2 Lazy path

Without bootstrap, every existing device adds the new device to a group the
next time it sends into that group (`devices::reconcile`, §5). A new phone
therefore fills in gradually, group by group, and starts with no history.
This is correct MLS behaviour and is stated here so that "my new phone shows
nothing" is understood as expected rather than as a fault.

---

## 5. What a revoked device keeps

### 5.1 The facts

A revoked device **keeps everything it already decrypted**: its local store,
its MLS state (all current epoch secrets of every group it is in), the Drive
manifest key, every object key it ever held, its contacts. Revocation on
chain does not reach into the device.

What revocation changes:

| Capability of the revoked device | After `RevokeDevice` is on chain |
| --- | --- |
| Fetch its own mailbox from store nodes | **Refused**: nodes check device authority against the chain |
| Publish key packages, upload blobs, post social events, sign receipts | **Refused** by nodes |
| Decrypt new envelopes it somehow obtains for a group | **Possible until that group commits its removal** (it still holds the epoch secret) |
| Decrypt envelopes after the removal commit | Impossible (MLS forward secrecy from the next epoch) |
| Be believed by honest readers as a sender | **Refused**: Hashgram One readers check the sender device against the chain and reject messages from a revoked device even inside a group that has not yet removed it |
| Fetch Drive objects it already holds keys for | **Possible** for any provider that still serves the CID (blob fetch is not authenticated); the manifest key opens the current manifest revision until the owner re-keys the Drive (§5.4) |
| Spend HASH / add devices | Only if it held the account/root key — see §6 |

### 5.2 `devices::reconcile`

`hashgram-sdk::devices::reconcile()` runs at the start of every sync round
and whenever the identity's device list changes on chain. For every group
the local device is a member of, it compares the MLS roster against the
chain's active device lists of every participant address:

* a member device the chain lists as **revoked** (or that no longer belongs
  to the participant) → **removal commit** for that group;
* an **active** device of a participant that is not yet a member → add on
  the next send (or eagerly, if `reconcile(eager = true)`);
* a participant address whose identity is revoked → all its devices removed.

Each removal is one MLS commit per group; MLS then ratchets and the removed
device cannot decrypt from that epoch on. The commit is sent to every
remaining member's mailbox as normal.

### 5.3 The stolen-device window

A removal commit requires an **honest member device to be online**. Until
one is, the revoked device remains an MLS member of that group:

> **A revoked device can decrypt new messages in a group until the next sync
> of any remaining device of any member of that group.**

Quantified: for each group, the window is
`max(time revocation lands on chain → first honest device of any member runs
reconcile)`. For an active group of several people this is typically
minutes. For a dormant two-person group where the other person is on
holiday, it is as long as that. For the user's own self-group, it is until
the user's other device syncs. During the window the revoked device must
also *obtain* the envelopes; with mailbox fetch refused, that means a
colluding store node or a network position that lets it capture deposits
addressed to its mailbox — harder, but not excluded, which is why the window
is stated as a window and not as zero.

```text
   t0                 t1                       t2                        t3
   │ device stolen    │ RevokeDevice on chain  │ some member device      │ that device's
   │                  │                        │ comes online, syncs     │ removal commit
   ▼                  ▼                        ▼                         ▼ propagates
   ├── thief reads normally ──┤── thief cannot fetch; can decrypt if fed ──┤── thief locked out of this group
                                                                               (per group; repeats for every group)
```

Every group has its own t2/t3. The SDK reports, per group, whether
reconciliation is complete, so a client can show "3 of 17 conversations
still include the revoked device".

### 5.4 Drive after revocation

The revoked device holds the manifest key and every object key it cached.
`devices::reconcile` on the owner's remaining device therefore also runs
`drive::rekey()`: a new manifest key is generated, the current manifest is
re-uploaded under it, the vault pointer is updated, and a fresh
`DriveKeyring` is sent in the self-group (after the revoked device's removal
commit). Objects are **not** re-encrypted en masse; each object keeps its
key until it is next modified, so the revoked device can still fetch
unchanged objects whose CID and key it already had. Re-encrypting the whole
Drive is offered as an explicit, potentially long, user action
(`drive::rekey_all_objects`).

---

## 6. Key rotation

`MsgRotateRootKey{new_root, signature_by_old_root, revoke_existing_devices}`:

* the **old root** must sign; the account key alone cannot rotate;
* `RotationCount` increments; every certificate issued under the old count
  is refused by `AddDevice` from then on;
* with `revoke_existing_devices = true` (the right default after suspected
  root compromise) every active device is revoked in the same transaction;
  the user must then add devices under the new root — which means the user
  needs a device that can produce a certificate signed by the new root, so
  the new root must be generated on a device the user still controls.

Consequences for MLS: **existing groups keep working.** MLS credentials are
device keys, not root keys; the group does not know or care about the root.
Two observable effects:

1. Readers that check device authority against the chain will, after a
   rotation with `revoke_existing_devices = true`, refuse **new** messages
   from every old device until it is re-added under the new root — including
   the user's own honest devices. The client must re-add them. Because a
   revoked device id is never reusable and a revoked device key is refused
   by `DeviceKeyInUse`, re-adding means a **fresh device key and id** per
   device, fresh key packages, and re-adding every device to every group
   (the old leaves are removed by `reconcile` on every member's device).
2. Without `revoke_existing_devices`, nothing changes for MLS at all; the
   rotation only prevents future certificates from the old root.

Rotation is therefore cheap for the chain and expensive for the client. The
SDK exposes `devices::rotate_root(revoke_existing)` and, when `revoke_existing`
is set, drives the re-add and reconcile sequence and reports progress.

---

## 7. Recovery

### 7.1 Mnemonic

The mnemonic restores the account key and, by derivation, the root key. With
those, a new device can: sign a certificate for itself, `MsgAddDevice`, then
revoke the lost devices (subject to the last-device rule: add first, revoke
after). It restores **no MLS state, no Drive manifest key, no local store**.
If no other device survives, Drive contents are unrecoverable in the
protocol as built — the manifest key is random and lives only in vaults and
in the self-group. This is why the backup export in §7.3 exists.

### 7.2 Guardian recovery (on chain)

From `x/identity/keeper/recovery.go`:

1. A guardian calls `InitiateRecovery{new_root, new_address?}`; only a
   configured guardian may; one open request at a time.
2. Other guardians `ApproveRecovery` until `Threshold` (≤ `MaxGuardians` = 10)
   is met.
3. The owner may `CancelRecovery` at any time before execution; the request
   is kept, cancelled, as evidence.
4. After `RecoveryDelayBlocks` (≥ `MinRecoveryDelayBlocks` = 43,200 ≈ 48 h)
   anyone may `ExecuteRecovery`: **every device is revoked**, the root is
   replaced (`RotationCount + 1`), and either the identity stays on the same
   address or moves to `new_address` (old record marked revoked, kept).

Effects on Hashgram One state: identical to a root rotation with
`revoke_existing_devices = true` (§6) plus, when the address moves, every
group's participant set now contains an address whose identity is revoked.
`devices::reconcile` on **other people's** devices removes the old devices;
the recovered user must be re-invited under the new address by a member of
each group, because nothing links the old address to the new one inside MLS.
Contacts and mail history are lost unless a backup (§7.3) is restored. The
guardians never hold any key of the user.

```text
   guardian G1: Initiate ──▶ [OPEN, approvals=1]
                                    │ G2..Gk Approve
                                    ▼
                          [OPEN, approvals ≥ threshold] ── owner Cancel ──▶ [CANCELLED]
                                    │ height ≥ executable
                                    ▼ anyone: Execute
                          [EXECUTED]: all devices REVOKED, root replaced, rotation+1
```

### 7.3 Encrypted vault backup — `account::export_backup`

New in the SDK. Produces a file the user can put anywhere (cloud drive,
USB stick, another person) that **the host cannot decrypt**, and that a
fresh device can import to become a full replacement for the exporting one
(minus its device key, which is deliberately not exported: the new device
gets its own).

Format, versioned:

```text
offset  size   field
0       8      magic  "HGBKUP\0\x01"            (last byte = format version 1)
8       1      kdf id                            0x01 = Argon2id
9       4      m_cost  (KiB, u32 LE)             default 262144 (256 MiB)
13      4      t_cost  (u32 LE)                  default 4
17      1      p_cost                            default 1
18      16     salt                              random
34      24     nonce                             random (XChaCha20-Poly1305)
58      4      ciphertext length (u32 LE)
62      n      ciphertext
62+n    16     Poly1305 tag
```

* **AAD** = bytes 0..58 (the whole header), so a downgrade of KDF parameters
  or format version fails authentication.
* **Key** = Argon2id(passphrase, salt, m, t, p) → 32 bytes. The backup
  passphrase is independent of the vault passphrase and the SDK refuses one
  shorter than 12 characters. KDF cost is higher than the vault's (which
  protects a file on the user's own disk) because the backup is expected to
  sit on hostile storage.
* **Plaintext** = the vault JSON (`VaultContents`) with `device_seed` and
  `device_id` **removed**, plus a `backup` object:
  `{ "exported_at": unix_secs, "address", "drive": {drive_id, manifest_key,
  manifest_cid, revision}, "contacts": ContactsSnapshot, "mls_state":
  omitted }`. MLS state is omitted on purpose: importing it onto a second
  live device would fork the ratchet and be refused by every group. The
  importer instead gets a fresh device that is added to groups by
  `bootstrap_new_device` or lazily.
* **What the backup restores**: account key, root key, mnemonic if the vault
  held it, Drive manifest key and pointer (so the whole Drive tree and every
  object key inside it), contacts. **What it does not restore**: mail (lives
  in the local store and other devices), MLS group membership, Space and
  Circle history, flags.
* A device-only vault exports a backup containing only the Drive keyring and
  contacts; the SDK labels it `partial = true` and says so in the UI.

Import: `account::import_backup(path, passphrase)` → creates a fresh device
key, writes a new vault, and returns the certificate request the user must
get signed by the root (which the backup contains, so it can self-sign) and
submit as `MsgAddDevice`.

---

## 8. Threat table

| Threat | What the attacker gets | What stops them / what the user must do | Residual |
| --- | --- | --- | --- |
| **Lost phone, locked** (device-only vault, OS lock intact) | Nothing without the vault passphrase; Argon2id 64 MiB slows guessing | User revokes the device from another device (add a replacement first if it was the last). Reconcile removes it from groups | Envelopes still in the phone's mailbox on store nodes are unreadable by anyone; they expire in ≤ 30 d |
| **Stolen unlocked laptop** (full vault, session open) | Everything on it: mail, Drive keys, contacts, MLS state, and the account+root keys if held | User must, from another device or from the mnemonic: rotate root with `revoke_existing_devices` (thief may hold the root) → re-add own devices → reconcile → `drive::rekey` → move funds. Race: the thief can do the same first | Whoever transacts first wins the identity; the recovery delay (48 h) gives the owner a cancel window only for guardian recovery, not for rotation. **State this to users.** Everything the laptop had decrypted is the thief's |
| **Stolen unlocked device-only laptop** | Mail, Drive keys, contacts, MLS state; cannot add devices, rotate or spend | Revoke it; reconcile; `drive::rekey` | Stolen-device window per group (§5.3); previously cached objects remain fetchable by CID |
| **Malicious store node** | Mailbox ids ↔ addresses, timing, sizes, key packages; can withhold | Nothing to do; multi-provider delivery; content is MLS-protected | Metadata (see `PRIVACY_MODEL.md` §4). Cannot add or impersonate a device: authority is on chain |
| **Compromised guardian(s) below threshold** | Can open a recovery request | Owner cancels within the delay; visible on chain | None beyond nuisance |
| **Compromised guardians at or above threshold** | After the delay, a new root; every device revoked; identity possibly moved to their address | Owner must notice within `RecoveryDelayBlocks` and send `CancelRecovery`. That is a chain transaction signed by the **account key**; a device-only vault cannot send it. **A user with only device-only vaults to hand cannot cancel a recovery.** Clients should surface open recovery requests prominently | The attacker does not obtain any MLS state, Drive key or mail; they obtain the identity going forward and can be re-invited to groups as "the user" by unsuspecting members |
| **Compromised mnemonic** | Account key + root key: can add devices, revoke the user's devices, rotate root, spend all HASH | Nothing on chain distinguishes them from the owner. Guardian recovery moving to a new address is the only remedy, and only if guardians were configured and the attacker has not already changed them (`SetRecoveryConfig` is signed by the account key) | Total. The mnemonic is the identity |
| **Compromised vault passphrase (file exfiltrated)** | Same as the stolen device of that vault's type | Same responses | Same |
| **Malicious other member's device** | Everything in shared groups; nothing from the user's other groups | Remove from the Space/Circle; MLS forward secrecy from that epoch | History already read |

---

## 9. State diagram — one device across its life

```text
                     mnemonic / root on another device signs cert
  ┌──────────┐   MsgAddDevice   ┌──────────────┐  bootstrap_new_device  ┌───────────────────┐
  │ generated│ ───────────────▶ │ ACTIVE       │ ─────────────────────▶ │ ACTIVE, in groups │
  │ (no cert)│                  │ (no groups)  │   or lazy adds         │                   │
  └──────────┘                  └──────────────┘                        └─────────┬─────────┘
                                                                                  │ MsgRevokeDevice /
                                                                                  │ RotateRoot{revoke} /
                                                                                  │ ExecuteRecovery
                                                                                  ▼
                                                                        ┌───────────────────┐
                                    nodes refuse it immediately         │ REVOKED on chain, │
                                    readers refuse its messages         │ still MLS member  │
                                                                        └─────────┬─────────┘
                                                                                  │ per group: first honest
                                                                                  │ member syncs → removal commit
                                                                                  ▼
                                                                        ┌───────────────────┐
                                                                        │ REVOKED, out of   │
                                                                        │ all groups        │  keeps what it decrypted
                                                                        └───────────────────┘
```

---

## 10. Limitations

Stated once, plainly.

1. **Revocation is not instantaneous for MLS.** A revoked device remains a
   group member until an honest member device of that group performs the
   removal commit (§5.3). Nodes refuse it at once; groups do not.
2. **A revoked device keeps everything it had.** No remote wipe exists or
   can exist in this design.
3. **A new device has no history.** MLS epochs forbid it; history arrives
   only by re-send from an existing device, bounded by what that device has.
4. **Drive is only as recoverable as the manifest key.** Lose every device
   and every backup and the Drive is gone, although the ciphertext persists
   on providers.
5. **Cached object keys survive `drive::rekey`.** Only modified objects get
   new keys; full re-encryption is manual and costs a full re-upload.
6. **Root rotation with revocation is a full re-enrolment.** Every device is
   re-added with a new id and key and re-joins every group.
7. **The chain check on senders is a client responsibility.** MLS does not
   consult the chain; the SDK does. A client that skips it (or is offline
   and uses stale device lists) accepts messages from revoked devices.
8. **Guardian recovery cancels only from an account-key holder.** Device-only
   vaults cannot send `CancelRecovery`.
9. **No protection against a mnemonic holder.** Whoever holds it *is* the
   identity, on chain and therefore everywhere.
10. **Device counts are public.** How many devices a user has, and when each
    was added or revoked, is on chain for anyone to read.
11. **The last-device rule can strand a user.** With one active device and a
    compromised root, the user cannot revoke it without first adding another
    — which the attacker can also do. Guardian recovery is the only exit.
12. **Backup files are only as strong as their passphrase.** Argon2id at
    256 MiB slows guessing; it does not make a weak passphrase strong.
13. **None of the SDK `devices::*` behaviour is enforced by protocol.** A
    third-party client that never calls `reconcile` leaves revoked devices in
    its groups indefinitely. Interoperating clients should treat
    reconciliation as mandatory.
