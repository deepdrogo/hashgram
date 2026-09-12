# HashMail — Specification

Status: describes the code on the `hashgram-one` branch. Model and rules:
`node/hashgram-app/src/mail.rs`, `spam.rs`, `envelope.rs`, `version.rs`.
Transport and local mailbox: `sdk/rust/hashgram-sdk/src/mail.rs`. Wire
shape: `proto/hashgram/app/v1/app.proto`. Developer harness:
`node/hashgram-client/src/one.rs` (`hashgram-client one mail …`). Companion
documents: `HASHGRAM_ONE_ARCHITECTURE.md`, `PRIVACY_MODEL.md`,
`MESSAGING.md`, `HASHDRIVE.md`, `SYNC_ENGINE.md`.

HashMail is end-to-end encrypted mail between Hashgram identities. A
`MailMessage` is an `AppMessage` body carried inside an MLS application
message; the MLS group is the only place it is ever plaintext. Store nodes
move opaque envelopes. There is no mail server.

---

## 1. Addresses

`hashgram_app::mail::parse_address` turns whatever a user typed into an
`AddressForm`:

| Input | `AddressForm` | Authority |
| --- | --- | --- |
| `hash1…` (39–90 chars, bech32 charset, checked by `ids::is_address`) | `Address` | the chain |
| `@alice`, `alice` | `Username("alice")` | `x/username` lookup → owner address |
| `alice@hashgram.io` (domain compared case-insensitively to `MAIL_DOMAIN = "hashgram.io"`) | `Username("alice")` | same |
| `someone@example.com` (local and domain non-empty, domain contains `.`, ≤ 254 chars) | `External` (lower-cased) | not a Hashgram identity; needs the gateway |

Usernames are lower-cased and must be 2–32 characters of
`[a-z0-9-_.]`. `mail_address_for("alice")` gives `alice@hashgram.io`.

Only the account address is an identity. The SDK (`People::resolve`)
resolves `Username` through `hashgram/username/v1/lookup/{name}` and carries
the result in `MailAddress{address, username, display_name}`; `username` and
`display_name` are hints that readers re-resolve. `Mail::resolve_recipients`
refuses `External` forms; those go through `services/mail-gateway` (§10).

---

## 2. The `MailMessage` model

Every field, its meaning, and the bound `hashgram_app::mail::validate`
enforces on both the builder and the reader.

| Field | Type | Meaning | Bound / rule |
| --- | --- | --- | --- |
| `version` | u32 | schema version | must equal `MAIL_VERSION = 1` (`version::check_body`) |
| `message_id` | bytes | identity for receipts, threading, dedup | exactly `ID_LEN = 16` random bytes |
| `thread_id` | bytes | thread root id | 16 bytes; equals `message_id` on a thread's first message |
| `from` | `MailAddress` | sender as claimed | required; `address` must be `hash1…`; `username`, `display_name` ≤ `MAX_NAME = 128` |
| `to`, `cc` | `MailAddress[]` | visible recipients | `to.len + cc.len ≤ MAX_RECIPIENTS = 100`, at least one |
| `bcc_copy` | bool | this copy went to a BCC recipient | set by `Draft::build_all` |
| `created_at_ms` | u64 | sender clock | — |
| `subject` | string | subject | ≤ `MAX_SUBJECT = 998` bytes; no `\n` or `\r`; no NUL |
| `in_reply_to` | bytes | parent `message_id` | empty or 16 bytes |
| `references` | bytes[] | ancestors, oldest first | ≤ `MAX_REFERENCES = 64`, each 16 bytes |
| `body_text` | string | plain body, always present | ≤ `MAX_BODY_TEXT = 1 MiB` |
| `body_html` | string | optional HTML | ≤ `MAX_BODY_HTML = 1 MiB`; readers must sanitise |
| `attachments` | `MailAttachment[]` | see §5 | ≤ `MAX_ATTACHMENTS = 64` |
| `expire_after_secs` | u32 | delete this long after first read | advisory; enforced by `Mail::purge` |
| `origin` | `MailOrigin` | `NATIVE` or `EXTERNAL_GATEWAY` | `external` required iff `EXTERNAL_GATEWAY` |
| `external` | `ExternalMailMeta` | what the gateway preserved | `gateway` is a `hash1…`; headers ≤ `MAX_HEADER = 998`; ≤ 16 `auth_results` of ≤ 128 bytes; `spam_score ≤ 1000` |
| `request_read_receipt` | bool | ask for a READ receipt | honoured only if the reader opted in (§6) |
| `importance` | `MailImportance` | NORMAL / LOW / HIGH | — |
| `labels` | string[] | sender-chosen hints | ≤ `MAX_LABELS = 32`, each 1–64 bytes |

Other constants in `mail.rs`: `MAX_FILENAME = 255`, `MAX_MIME = 128`,
`MAX_INLINE_ATTACHMENT = 64 KiB`. The whole encoded `AppMessage` is capped
at `envelope::MAX_APP_MESSAGE_BYTES = 256 KiB`, so a message that is valid
field by field can still be refused at `encode_for_mls` if its inline
attachments together exceed the envelope.

---

## 3. Transport

### 3.1 Groups keyed by participant set

`mail::participant_set(m)` is `{from} ∪ to ∪ cc` (addresses, sorted,
deduplicated). `Mail::send_built` removes our own address and calls
`HashgramOne::conversation_group(participants)`, which walks
`Messaging::conversations()` for a group whose member-address set equals
`participants ∪ {me}` and whose `group_kind` is not `CIRCLE` or `SPACE`;
if none exists it calls `Messaging::create_conversation`, which adds every
active on-chain device of every participant (and our own other devices).
The same recipient set therefore always reuses one MLS group, and a Circle
can never be repurposed as a mail channel.

The message is wrapped by `envelope::wrap` (fresh 16-byte `AppMessage.id`,
`APP_ENVELOPE_VERSION = 1`), placed in a `ChatMessage{kind: CHAT_KIND_APP}`
by `envelope::to_chat`, encrypted once by MLS, and delivered as one
`Envelope` per member device to the store nodes that hold that device's
mailbox (`Messaging::send` → `deliver`).

### 3.2 BCC

`Draft::build_all` → `Outgoing{main, bcc_copies}`. Every copy shares one
`message_id`. The `main` copy (absent if there are only BCC recipients) goes
to the To+CC group. Each BCC recipient gets its own copy with
`bcc_copy = true`, sent to the sender+recipient group only, so the visible
recipients never learn of it. The BCC copy keeps the To/CC lines (like
SMTP); a pure-BCC message puts the sender in `to` so `validate` holds.
Our Sent record keeps `bcc_copy = false` and the full BCC list; if some
copies fail the record gets the label `partial-delivery` and the error is
logged, never the content.

### 3.3 Sequence

```
 sender device              store node(s)                 recipient device
 ─────────────              ─────────────                 ────────────────
 Draft::build_all
 conversation_group ──(create group if new: Welcome per device)──▶ mailbox
 envelope::wrap/to_chat
 MLS encrypt
 MailboxPut ─────────────▶ holds Envelope{mailbox=blake3(devkey),
                             size, created_at, expires_at}
                                                  ◀── MailboxFetch (signed, cursor)
                                                  ──▶ envelopes
                                                       MLS process → MailMessage
                                                       validate; sender check (§7)
                                                       spam::dispose → folder
                                                       MailRecord stored
                                                  ◀── MailboxAck
                           ◀──────────────────────── MailReceipt{DELIVERED}
 MailboxFetch ───────────▶
 receive_receipt: delivered_to[recipient] = at_ms
```

The recipient's DELIVERED receipt travels back in the same group, so the
node sees another opaque envelope of receipt size and nothing else.

---

## 4. Threading

* A new message: `thread_id = message_id`, `in_reply_to` and `references`
  empty.
* A reply (`Draft.in_reply_to = Some(ReplyTarget{message_id, thread_id,
  references})`): `thread_id` inherited, `in_reply_to = parent.message_id`,
  `references = parent.references ++ [parent.message_id]`; when that
  exceeds `MAX_REFERENCES` the root is kept and the oldest non-root
  ancestors are dropped (`refs.remove(1)`).
* `reply_recipients(m)`: To = sender only.
* `reply_all_recipients(m, me)`: To = sender + `to` minus me, CC = `cc`
  minus me, deduplicated; a `bcc_copy` replies to the sender only so the
  hidden recipient does not reveal itself.
* `normalised_subject`: strips repeated `re:`, `fwd:`, `fw:`, `aw:`, `wg:`
  prefixes, collapses whitespace, lower-cases. `Mail::reply_draft` uses it
  to decide whether to prepend `Re: `.
* `forward_draft(from, original)`: new thread, body quotes the original
  (`---------- Forwarded message ----------`), attachments carried over as
  references (no re-upload), subject prefixed `Fwd: ` once.

Locally, `MailState.by_thread` indexes message ids per `thread_id`;
`Mail::thread` sorts by `(created_at_ms, message_id)` and hides Trash/Spam;
`Mail::threads` gives one row per thread with newest activity first.

---

## 5. Attachments

`MailAttachment{name, mime, size, plaintext_hash, content_id, source}` with
exactly one `source`:

| Source | When | Built by | Opened by |
| --- | --- | --- | --- |
| `inline_data` | `bytes.len() ≤ MAX_INLINE_ATTACHMENT` (64 KiB) | `Mail::make_attachment` | `Mail::attachment_bytes` returns the bytes |
| `blob` (`BlobRef{cid, key, nonce, …}`) | larger files | `make_attachment` → `blob::upload(… private=true, replicas=2)`, XChaCha20-Poly1305 with a random key; `BlobRef.key` 32 B, `nonce` 24 B | `blob::download` + `blob::decrypt_private`, then BLAKE3 compared with `plaintext_hash` |
| `drive` (`DriveCapability`) | a Drive entry, snapshot or live | `Mail::attach_from_drive(entry_id, recipients, live)` → `Drive::share` grants in the recipients' conversation group | `Drive::download_capability` (segmented, see `HASHDRIVE.md`) |

`validate_attachment` refuses names containing `/`, `\`, `.` or `..`, an
inline `size` that disagrees with the data, a blob key of the wrong length,
and any capability `drive::validate_capability` rejects. A live Drive
attachment keeps following the owner's edits through `DriveShareUpdate`
messages in the same group until the owner revokes.

---

## 6. Receipts

`MailReceipt{version, message_id, kind, at_ms}`:

* **DELIVERED** — sent automatically by `receive_mail` after a message is
  filed, into the group it arrived in, unless the disposition was `Spam`.
* **READ** — sent by `Mail::mark_read(id, true)` only when all hold: the
  record was unread, it is not our own outgoing copy, the sender set
  `request_read_receipt`, and `MailSettings.send_read_receipts` is true
  (default `false`).

`receive_receipt` applies a receipt only to a record with `outgoing = true`,
recording `delivered_to[sender] = at_ms` or `read_by[sender] = at_ms`. A
receipt from a non-recipient for a message we did not send is ignored.

---

## 7. The authenticated-sender rule

`Received.sender` is the address parsed from the MLS credential of the
device that encrypted the message, which is an on-chain device key. In
`receive_mail`:

* `MailRecord.authenticated_sender = Received.sender` is what the UI shows.
* If `message.from.address != Received.sender` and `origin` is not
  `EXTERNAL_GATEWAY`, the message is an impersonation attempt by a group
  member; `facts.is_blocked` is forced to true, `spam::dispose` returns
  `Drop`, and the message is not stored.
* If `Received.sender` is our own address it is another of our devices'
  copy and is filed in `Sent` with `outgoing = true`.
* Gateway mail is exempt because its `from.address` is the gateway's own
  identity (the bridge sets `from.display_name` from the original header
  and the true origin lives in `ExternalMailMeta.from_header`).

---

## 8. The local mailbox

Nothing about folders, flags or labels leaves the device except as hints
to the user's own other devices.

### 8.1 Records and folders

`MailRecord{message, folder, read, starred, labels, received_at_ms,
authenticated_sender, group_id, delivered_to, read_by, outgoing,
trust_score}` is stored in the sealed local store under
`mail/msg/<message_id>`. Drafts (`DraftRecord`) live under `mail/draft`,
settings and the rate window under `mail/meta`.

Folder names are the constants in `hashgram_sdk::mail::folder`:
`inbox`, `sent`, `drafts`, `archive`, `trash`, `spam`, `requests`
(`folder::ALL`). `Mail::move_to` rejects anything else.

### 8.2 `MailState` indexes

Rebuilt by `MailState::load` at open and kept current by `put`/`remove`:

| Index | Shape | Used by |
| --- | --- | --- |
| `by_folder` | folder → sorted `(received_at_ms, id)` | `list`, `threads`, `purge` |
| `by_thread` | thread hex → message ids | `thread` |
| `index` | id → `(folder, read, starred, thread)` | dedup in `receive_mail`, `counts`, `starred` |
| `pending_hint` | `MailStateHint` accumulating flag changes | flushed by the sync engine |
| `window` | `spam::RateWindow` | §9 |
| `settings` | `MailSettings{send_read_receipts, keep_sent, purge_after_days}` | defaults `false`, `true`, `30` |

Listing a folder is a reverse slice with an optional `before_ms` cursor,
not a scan.

### 8.3 Operations

`mark_read`, `star`, `move_to`, `archive`, `trash` (a second `trash` on a
trashed message deletes it), `delete`, `accept_request` (Requests → Inbox),
`set_label`/`by_label` (labels 1–64 bytes, local only), `starred`,
`search` (case-insensitive substring over subject, body, participants,
attachment names; queries shorter than 2 characters return nothing),
`counts`, `purge`.

### 8.4 Device sync of flags

Each flag change appends the `message_id` to the matching list of
`pending_hint` (`read`, `unread`, `archived`, `trashed`, `starred`,
`unstarred`, `deleted`). `Mail::take_pending_hint` is called in the sync
engine's `Outbox` stage and the hint is sent as
`DeviceSync{mail_state}` in the self group. `Devices::handle_incoming`
accepts a `DeviceSync` only when the MLS sender is our own address and
calls `Mail::apply_state_hint`, which applies each list in order; `deleted`
removes the record. Label changes are not synced.

### 8.5 Purge and expiry

`Mail::purge` (run every sync round) removes: Trash and Spam records older
than `purge_after_days` (0 disables), and read messages whose
`expire_after_secs > 0` and `now − received_at_ms > expire_after_secs`.
Expiry is measured from local receipt, and only cooperating clients honour
it.

---

## 9. Spam and unknown senders

Filtering is local (`hashgram_app::spam`); no server decides who may write
to whom. `receive_mail` builds `SenderFacts` from the contact book
(`Contacts::sender_facts`: `is_contact`, `is_trusted`, `is_blocked`,
`is_muted`, `has_username`), then fills `has_username` from the chain if
unknown (`People::username_of`), `previously_written_to` by scanning our
Sent folder for the sender, and `prior_messages` from the Inbox size
(capped at 100).

`spam::dispose(facts, msg, window, now)`:

1. `is_blocked` → `Drop` (not stored; the sender learns nothing).
2. `is_trusted || is_contact` → `Inbox`.
3. `previously_written_to && !is_muted` → `Inbox`.
4. `RateWindow::record(sender, now)` (1-hour sliding window, ≤ 4096
   arrivals kept): more than `MAX_PER_SENDER_PER_HOUR = 5` first-contact
   messages from one sender → `Spam`.
5. `score = trust_score(facts, msg)`; more than `MAX_UNKNOWN_PER_HOUR = 30`
   distinct unknown senders in the hour and `score < 30` → `Spam`.
6. `score ≥ REQUESTS_THRESHOLD = −20` → `Requests`, else `Spam`.

`trust_score` terms (clamped to −100..100):

| Fact | Δ |
| --- | --- |
| previously written to | +40 |
| holds a username | +15; age ≥ 180 d +15, ≥ 30 d +8, known < 30 d +2 |
| no username | −10 |
| prior accepted messages | +3 each, ≤ 10 |
| > 20 recipients and not a reply | −25 (> 5: −8) |
| is a reply (`in_reply_to` set) | +10 |
| empty subject and body < 20 bytes | −5 |
| `EXTERNAL_GATEWAY` origin | −15; `dkim=pass`/`spf=pass` +8 each, `dmarc=pass` +10, any `*=fail` −20; `spam_score > 700` −40, > 400 −15 |

A plain first message from a stranger without a username scores −10 and
lands in **Requests**. The record keeps `trust_score` for the UI. The
transport backstop is the store node's per-mailbox quota; mailbox griefing
remains a stated tension (`HASHGRAM_ONE_AUDIT.md` F8).

---

## 10. External mail

`MailOrigin::EXTERNAL_GATEWAY` marks mail bridged from the Internet by
`services/mail-gateway`. The gateway is an ordinary Hashgram identity; the
message it sends has `from.address` = the gateway, `external =
ExternalMailMeta{gateway, from_header, message_id_header, auth_results,
spam_score}`. `validate` rejects `external` on a native message and a
gateway message without it. Clients must label such mail; the CLI prints
`[EXTERNAL — bridged by a gateway, not end-to-end encrypted]`. Outbound
uses the `ext-to:`/`ext-cc:` label convention documented in
`services/mail-gateway/src/bridge.rs`. The gateway library (SMTP state
machine, MIME, DKIM, SQLite queue, `Bridge::run`) exists; its `main.rs` is a
placeholder, so no gateway binary runs yet.

---

## 11. Versioning

Two gates, both in `hashgram-app`:

* Envelope: `envelope::open` reads `AppMessage.version` (0 read as 1) and
  refuses anything above `APP_ENVELOPE_MAX_READ = 1` or with
  `min_reader_version` above it, and any unknown body arm, as
  `AppError::Unsupported`. The sync engine stores the ciphertext under
  `unsupported/<id>` and emits `SyncEvent::UnsupportedMessage` so a later
  build can render it; partial rendering is never attempted.
* Body: `validate` requires `MailMessage.version == MAIL_VERSION = 1`
  exactly; `MailReceipt` is written with the same constant.

---

## 12. What nodes see

A store node holding a mail envelope knows `mailbox = blake3(device key)`,
ciphertext size, `created_at`/`expires_at`, and the envelope kind. It does
not know the sender, the group, the recipients, the subject, or that the
envelope is mail rather than a Circle post. A blob attachment adds one
private blob (CID, ciphertext size, chunk count) to a store/media node.
Receipts are envelopes of receipt size. Full table: `PRIVACY_MODEL.md` §3.4.

---

## 13. CLI examples

```bash
# send with an inline file and a live Drive attachment
hashgram-client one mail send --to @bob --cc carol@hashgram.io \
  --subject 'Contract for review' --body 'see attached' \
  --attach note.txt --attach-drive-live <entry_hex>

# BCC only; read receipt requested
hashgram-client one mail send --bcc @alice --subject bcc --body x --read-receipt

# mailbox
hashgram-client one mail list requests            # unknown senders
hashgram-client one mail accept <id>              # → inbox
hashgram-client one mail list inbox --threads
hashgram-client one mail show <id>
hashgram-client one mail thread <thread_id>
hashgram-client one mail attachment <id> 1 out.bin
hashgram-client one mail send --reply-to <id> --all --body 'Looks good'

# flags and folders (synced to your other devices via MailStateHint)
hashgram-client one mail read <id>; hashgram-client one mail star <id>
hashgram-client one mail archive <id>; hashgram-client one mail trash <id>
hashgram-client one mail label <id> invoice
hashgram-client one mail search contract --limit 20
hashgram-client one mail counts
hashgram-client one mail settings --read-receipts true

# pull new mail and flush receipts / hints
hashgram-client one sync
```

`scripts/testnet/hashgram-one-e2e.sh` exercises send, Requests filing,
accept, inline and Drive attachment decryption, reply threading, the
DELIVERED receipt on the Sent copy, and the `bcc_copy` flag on a devnet.

---

## 14. Limitations

* Delivery is best-effort per device: `send_built` succeeds if at least one
  copy was accepted by a store node and labels the record
  `partial-delivery` otherwise; there is no retry queue for failed copies.
* `expire_after_secs` and read receipts depend on the recipient's client.
* `previously_written_to` is computed by scanning Sent on each arrival
  from a stranger; large Sent folders make that arrival slower.
* `search` is a bounded substring scan, not an index; the desktop keeps its
  own.
* Labels do not sync between devices; folders and flags do.
* No `MailStateHint` is emitted on a single-device account
  (`self_group()` is `None`), which is correct but means flags start
  syncing only once a second device exists.
* External mail requires a running gateway, which does not exist as a
  binary yet (§10).
