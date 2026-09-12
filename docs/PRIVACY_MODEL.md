# Hashgram One — Privacy Model

Status: normative for the `hashgram-one` branch. Companion documents:
`HASHGRAM_ONE_ARCHITECTURE.md` (what is built), `THREAT_MODEL.md` (chain
and node adversaries), `MESSAGING.md`, `STORAGE.md`, `LOGGING_POLICY.md`,
`MULTI_DEVICE_SECURITY.md`.

This document states, actor by actor and data class by data class, what
Hashgram One reveals and to whom. It is written so that a reader can decide
what *not* to rely on. Every "cannot see" below is a statement about the
protocol as implemented on this branch; every "can see" is a statement about
metadata the design does not, and in most cases cannot, hide. Where a
property is merely intended rather than enforced, the text says so.

Spelling is British throughout.

---

## 1. Principles

1. **Content is opaque to infrastructure.** Every application payload
   (`hashgram.app.v1.AppMessage`, `proto/hashgram/app/v1/app.proto`) travels
   inside an MLS application message. Store, relay and media nodes, the
   indexer and the chain never hold a key that opens one. The only party
   outside the MLS group that ever sees plaintext is the optional mail
   gateway, and only for mail that crossed the public Internet (§3.7).
2. **Nothing private is placed on chain.** The chain records identity roots,
   device certificates, usernames, balances and provider state. It records
   no message, folder, filename, share, contact or group.
3. **Local state stays local.** Folders, flags, threads, contacts, block and
   mute lists, Space state and sync cursors live in the SDK local store
   (`hashgram-sdk::store`, redb, sealed per record) and reach only the user's
   own devices, through `DeviceSync` in the self-group.
4. **Metadata is minimised, not eliminated.** A store-and-forward mailbox
   labelled by a hash of the recipient's device key is the smallest label
   that still allows delivery. It is still a label. §4 lists what follows
   from it.
5. **Public is public by design.** Follows, posts, profiles and usernames are
   signed, public events. The privacy model does not attempt to hide a
   social graph the user chose to publish.
6. **No party is trusted with more than it needs.** Every actor in §2 is
   assumed to be curious and, where the threat model permits, malicious. The
   claims below hold against a curious actor; §6 says which hold against a
   malicious one.

---

## 2. Actors

| Actor | Role in the system | Trust assumed |
| --- | --- | --- |
| Chain / validators | Consensus over identity, usernames, balances, providers | Honest majority (BFT); everything on chain is public |
| Store node | Holds envelopes and key packages for offline devices; may also hold blobs | Untrusted; may be curious, may collude with others |
| Relay node | Forwards RPC, bootstrap, chain reads and broadcasts | Untrusted |
| Media node | Serves blob manifests and chunks | Untrusted |
| Indexer | PostgreSQL read model over chain and public social events | Untrusted for privacy; trusted only as much as any public web service |
| Mail gateway | Optional SMTP bridge owning an ordinary Hashgram identity | **Trusted with plaintext of bridged mail** (§3.7) |
| Recipient | A member of the MLS group the message was sent to | Trusted with the content, by definition |
| Other group members | Every device of every participant in a Circle, Space or mail recipient set | Trusted with the content, by definition |
| Passive network observer | Sees IP-level traffic between a client and the nodes it talks to | Untrusted; assumed global in §4 |
| The user's other devices | Members of the self-group | Trusted; they hold the same keys |

A single operator may run several of these at once (a store node that is
also a media node, a relay and a validator). The matrix in §3 is per role;
an operator combining roles sees the union.

---

## 3. What each actor can see

Legend: **Y** sees it in the clear · **M** sees metadata only (what kind is
stated in the cell) · **–** sees nothing · **P** public by design.

### 3.1 On-chain data

| Data | Chain | Store | Relay | Media | Indexer | Gateway | Observer |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Address, identity root key, rotation count | P | P | P | P | P | P | P |
| Device ids, device public keys, certificates, revocations | P | P | P | P | P | P | P |
| Username ↔ owner address | P | P | P | P | P | P | P |
| Balances, transfers, provider registrations, rewards | P | P | P | P | P | P | P |

Everything in this table is readable by anyone with a chain node or the
indexer. A username is a public pseudonym bound to an address; registering
one is a choice to make that binding public.

### 3.2 MLS envelopes (mail, Circle, Space, People, DeviceSync)

| Field | Store node holding the mailbox | Relay | Observer | Recipient |
| --- | --- | --- | --- | --- |
| `mailbox = blake3(device key)` | Y | – (envelopes go direct to store nodes) | M (TLS/Noise-encrypted stream; sees peer, not field) | Y |
| Ciphertext size | Y | – | M (stream size ± padding) | Y |
| `created_at`, `expires_at`, put/fetch times | Y | – | M (timing) | Y |
| `kind` (WELCOME / MESSAGE) | Y | – | – | Y |
| **Sender address or device** | **–** (deposit is unauthenticated) | – | M (IP of depositing client) | Y (MLS credential) |
| **Group id, group kind, member list** | – | – | – | Y |
| **Content** (`AppMessage`) | – | – | – | Y |

`proto/hashgram/p2p/v1/envelope.proto` fixes the envelope fields; there is
no field for sender or group. The `MailboxNotify` gossip carries the mailbox
id only.

### 3.3 Key packages

| Field | Store node | Anyone who asks | Notes |
| --- | --- | --- | --- |
| Device public key, MLS key package, expiry, `last_resort` flag | Y | Y (`KeyPackageFetch` is unauthenticated) | Public by necessity: a sender must be able to fetch it |

A key package reveals that a device exists and is reachable. It reveals
nothing about who has added it to which groups.

### 3.4 Mail

| Data | Store | Relay | Media | Indexer | Gateway | Recipient | BCC recipient |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Subject, bodies, labels, importance | – | – | – | – | Y for bridged mail only | Y | Y |
| To / CC lists, `From` | – | – | – | – | Y for bridged mail only | Y | Y (sees To/CC, as SMTP) |
| BCC list | – | – | – | – | Y for bridged mail only | **–** (separate group per BCC) | Sees only itself |
| Thread ids, `in_reply_to`, `references` | – | – | – | – | Y for bridged mail only | Y | Y |
| Inline attachments (≤ 64 KiB) | – | – | – | – | Y for bridged mail only | Y | Y |
| Blob attachments | M (CID, ciphertext size, chunk count) | – | M (same) | – | Y for bridged | Y | Y |
| Delivery / read receipts | M (an envelope of receipt size) | – | – | – | – | Y | Y |
| Folders, flags, read state | – | – | – | – | – | – (own devices only) | – |

`MailMessage.subject` is documented in the proto as "never leaves the MLS
group"; that is a structural fact, not a policy.

### 3.5 Drive

| Data | Store / media node | Recipient of a capability | Owner's other devices |
| --- | --- | --- | --- |
| Object bytes | M: ciphertext, CID = BLAKE3(ciphertext manifest), size, chunk count, 1 MiB segmentation | Y (holds `DriveKey`) | Y |
| Filenames, MIME, folder structure, versions, trash, share graph | **–** (inside the encrypted `DriveManifest` blob) | Y for the shared entry only (`DriveCapability.name`, `mime`) | Y (via `DriveKeyring`) |
| Drive manifest blob | M: one more private blob of some size; the node cannot tell it is a manifest | – | Y |
| Capability (`DriveCapability`) | – (MLS only) | Y | Y |
| Uploader device key | Y (`BlobPutManifest.uploader_pubkey`, for quota) | – | – |
| Who downloaded what | Y for the node that served it (peer id, CID, time); the retrieval receipt names the client device key | – | – |

The segment design in `node/hashgram-app/src/drive.rs` (AAD =
`"hashgram-drive-v1" ‖ le64(i) ‖ le64(total)`) prevents a node from
reordering, dropping or truncating segments undetected; it does not hide
the number of segments, which is ⌈size / (1 MiB − 16)⌉ and therefore
reveals the plaintext size to within 16 bytes per MiB.

### 3.6 Social, Circles, Spaces, People

| Data | Store | Indexer | Anyone | Group members |
| --- | --- | --- | --- | --- |
| Public posts, comments, reactions, reposts, profiles, follows, channels | P | P (projected into `posts`, `follows`, `profiles`, … `indexer/schema.go`) | P | P |
| Author device key of each public event | P | P | P | P |
| Circle / Space posts, announcements, member lists, roles, Space Drive shares | – | – | – | Y |
| Contact requests, responses, `ProfileCard` (incl. optional wallet disclosure) | – | – | – | Y (the two parties only) |
| Friend / block / mute / trusted state | – | – | – | – (own devices only, via `ContactsSnapshot`) |

A Space exists nowhere but in its members' MLS state and local stores.
Nothing about a Space is on chain or on any node beyond envelopes.

### 3.7 Gateway-bridged mail

The mail gateway (`services/mail-gateway`) owns an ordinary Hashgram
identity. Mail arriving from the public Internet reaches it as SMTP plaintext
(or TLS-terminated plaintext); it is re-encoded as
`MailMessage{origin: MAIL_ORIGIN_EXTERNAL_GATEWAY, external: {...}}` and
sent through MLS to the mapped user. Outbound mail to an external address is
sent by the user *to the gateway identity* through MLS; the gateway then
holds plaintext in order to render MIME and DKIM-sign it.

| Data | Gateway operator | Everyone downstream on the Internet |
| --- | --- | --- |
| Full plaintext of bridged mail, both directions | **Y** | Y (ordinary email) |
| SPF / DKIM / DMARC results, spam score | Y (recorded into `ExternalMailMeta.auth_results`) | – |
| Mapping username ↔ external address | Y | Y (it is the address) |
| Native HashMail between Hashgram identities | **–** (never routed through a gateway) | – |

Clients **must** label bridged mail (`MailOrigin` in the proto says so) and
the spam policy (`node/hashgram-app/src/spam.rs`) uses the gateway verdicts
only as one input. The gateway is a trusted party for the mail it bridges,
exactly as any email provider is. Anyone may run one under their own domain.

### 3.8 Wallet ↔ username linkage

| Situation | Who can link the wallet address to the username |
| --- | --- |
| Username registered by address A on chain | Everyone (public; the indexer shows "verified") |
| `ProfileCard.wallet_address` sent to a contact | That contact only |
| Neither | Nobody, except by traffic analysis (§4) |

The indexer joins usernames to addresses **only** through the on-chain
`x/username` owner relation (`indexer/chain.go` `SyncRegistries`). It has no
other source and adds none.

### 3.9 Indexer projections

The indexer (`indexer/`) ingests blocks, transactions, transfers, usernames,
identities, providers, public social events and safety attestations. The
Hashgram One additions are a `balances` projection and leaderboards. It
never receives an envelope, a blob, or anything from inside MLS. What it
publishes is a rearrangement of data that is already public; its privacy
relevance is that it makes public data *convenient* (holder rankings, follower
lists, per-address transfer history), which is a real change in practical
exposure and is stated here for that reason.

---

## 4. Metadata that cannot realistically be hidden

This section is the one to read before relying on the system for anything
where *who* matters more than *what*.

### 4.1 Mailbox id ↔ device key ↔ address

A mailbox is `blake3(device public key)`. Device public keys are **public on
chain**, listed under the owning address (`x/identity`). Therefore:

> **A store node can compute the mailbox id of every device of every
> identity on the chain, and so can learn which address a mailbox belongs
> to.** The hash is a label, not a disguise.

Consequences a store node (or anyone who can query one) can draw:

* which addresses have devices whose mailboxes it hosts;
* how many envelopes each of those addresses receives, of what size, when;
* when each device of an address comes online to fetch (signed
  `MailboxFetch` proves the device key, which is on chain);
* which store nodes advertise a mailbox in the DHT
  (`hashgram/mailbox/<id>` provider records are public).

What it still cannot learn from envelopes: the sender, the group, and the
content. Deposit is deliberately unauthenticated so that a store node
cannot demand the sender's identity; the price of that is the griefing
tension in §6.1.

### 4.2 Timing and size correlation

A device that sends to a group with M other devices deposits M envelopes of
identical ciphertext within milliseconds. An observer of several store
nodes, or one store node hosting several of the recipients' mailboxes, can
infer with high confidence that those mailboxes are in one group, and by
§4.1 which addresses those are. Repeated over days this reconstructs the
communication graph. No padding, batching, delay or cover traffic is
implemented. `THREAT_MODEL.md` §2 says the same; it is repeated here because
Hashgram One's mail semantics (one group per recipient set, one extra group
per BCC recipient) make the pattern more legible, not less.

### 4.3 Who talks to which store nodes

The SDK deposits to up to three DHT providers of each mailbox, falling back
to any connected store node, and fetches from every provider of its own
mailbox. The set of store nodes a device uses is therefore visible to those
nodes, to the DHT, and to a network observer.

### 4.4 Client IP addresses

Every client connects directly to store, relay and media nodes over libp2p.
The node sees the client's IP address and its device public key (from the
handshake and from every signed request). Nothing in Hashgram One hides a
client's IP from the nodes it uses; a user who needs that must bring their
own network-layer protection (VPN, Tor transport is not implemented).

### 4.5 DHT provider records

`hashgram/mailbox/<id>` and `hashgram/blob/<cid>` records are public in the
DHT. Anyone can enumerate which store nodes claim a mailbox and which
providers claim a blob. Combined with §4.1 this tells anyone where a given
address's mail is held; combined with a leaked CID it tells anyone where an
encrypted object lives (still without the key).

### 4.6 Blob access patterns

A media or store node knows which device key fetched which CID and when
(the retrieval receipt names the client device). It cannot open the object,
but for a shared object it learns the set of devices that fetched it, which
approximates the set of people the object was shared with.

### 4.7 Public social graph

Follows and public posts are public. Deriving "who is close to whom" from
public follows, reaction patterns and posting times requires no attack.

---

## 5. What we do NOT claim

* **No anonymity.** Every action is attributable to a device key that is
  publicly bound to an address on chain. Hashgram One is pseudonymous at
  best, and only until the pseudonym is linked to a person by any means.
* **No traffic-analysis resistance.** No mixing, padding, cover traffic or
  onion routing. A global passive observer, or a coalition of store nodes,
  can reconstruct communication patterns (§4.2).
* **No hiding of the public social graph.** Follows are public by design.
* **No protection against a compromised member device.** An attacker holding
  any member's device key and MLS state reads everything that member can,
  until that device is removed from each group (`MULTI_DEVICE_SECURITY.md`
  §5 for the window).
* **No protection against a malicious recipient.** A recipient can forward,
  screenshot, or republish anything. A Drive capability holder holds the key
  (§6.3).
* **No forced deletion on nodes.** Blob deletion is at the provider's
  discretion (`STORAGE.md` "Retention and deletion"); a revoked share cannot
  recall bytes already downloaded.
* **No formal proof.** The properties above are enforced by tested code and
  a CI grep for logging violations, not by machine-checked proofs. The
  application layer (`hashgram-app`, `hashgram-sdk`) has had no external
  audit.
* **No plausible deniability.** MLS authenticates the sender to every group
  member; a recipient can prove to a third party who sent what, if they
  export their MLS state.

---

## 6. Threats and mitigations specific to Mail and Drive

### 6.1 Mailbox griefing (audit F8)

Anyone may deposit into any mailbox; the store node cannot authenticate the
sender without learning who they are, which the design refuses. An attacker
can therefore fill a victim's mailbox up to the per-mailbox quota
(2,000 envelopes / 64 MiB per node) with junk that MLS will reject, delaying
legitimate mail until the victim acks and drains it.

Mitigations in place: per-mailbox quota and 30-day expiry (transport
backstop); envelopes are cheap to reject client-side (MLS refuses them
before any application parsing); the application-layer spam policy
(`spam.rs`) files unknown senders under *Requests* and rate-limits first
contacts. **Not mitigated:** exhaustion of the quota itself. A deposit
fee or proof-of-work on `MailboxPut` is a protocol change under
consideration and is not on this branch.

### 6.2 Replay of a signed fetch within the 300 s window (audit F2)

`MailboxFetch`, `MailboxAck` and `BlobPutManifest` are signed over a
timestamp but not over the responding node's identity. Within
`MAX_REQUEST_AGE_SECS = 300` a malicious store node can replay a fetch it
received to another store node that holds the same mailbox, and receive
that node's copy of the victim's envelopes. It gains ciphertext it may
already have had (the envelopes are the same MLS messages) plus confirmation
of which envelopes the other node holds. It cannot decrypt anything.

Mitigation on this branch: the SDK signs a fresh timestamp per peer, and a
store node refuses a `(mailbox, timestamp, cursor)` triple it has already
served within the window (replay cache in `node/hashgram-node/src/mailbox.rs`,
metric `hashgram_mailbox_replay_refused_total`). Planned fix: the responder
peer id enters the signed preimage in protocol minor v1.1
(`HASHGRAM_ONE_ARCHITECTURE.md` §12); this is a wire change and is not on
this branch.

### 6.3 Capability leakage by a malicious recipient

A `DriveCapability` contains the object key. Whoever holds it can fetch the
ciphertext by CID from any provider and decrypt it, forever, for that
object version. Forwarding a capability is indistinguishable from the
grantee fetching it themselves. **This is unavoidable in any design where
the recipient must be able to read the file.**

Semantics that limit the damage:

* Revoke (`DriveShareRevoke`) stops future `DriveShareUpdate`s and the owner
  encrypts the *next* version under a fresh `ObjectKey`; the leaked key
  opens only the versions it was issued for.
* Snapshot shares never follow later versions.
* Folder shares (`DriveFolderManifest`) leak the folder's listing to the
  holder; revoking a folder share re-keys the folder manifest and every
  subsequently modified object in it, not objects that were never modified.
* Bytes already downloaded cannot be recalled. The proto comment on
  `DriveShareRevoke` says so; the UI must not imply otherwise.

### 6.4 Gateway operator trust

The gateway sees bridged plaintext (§3.7). A malicious operator can read,
alter, delay or fabricate external mail, and can forge `auth_results`. What
it cannot do: read native HashMail, sign as the user (it holds no user
keys), or add devices to the user's identity. Mitigations: `MailOrigin`
labelling is mandatory; the spam policy treats gateway verdicts as
advisory; users may choose which gateway (if any) their username is mapped
to, or run their own.

### 6.5 Malicious store node

Can: withhold or delay envelopes; serve stale cursors; refuse acks so
envelopes are redelivered; learn everything in §4. Cannot: read content,
forge envelopes that MLS accepts, or fetch another device's mailbox
(signature check). The SDK talks to up to three providers per mailbox so a
single withholding node does not lose mail.

### 6.6 Receipt metadata

`MailReceipt{DELIVERED}` is sent automatically when a device decrypts,
`READ` only if the user opted in. Both are envelopes of a characteristic
small size deposited to the sender's devices shortly after a fetch; a store
node can infer "read/delivered" timing from them even though it cannot
decode them. Disabling delivered-receipts is a client setting.

---

## 7. Retention

| Data | Where | Retention | Enforced by |
| --- | --- | --- | --- |
| Envelopes | Store node redb | Requested by sender (SDK default 14 d), clamped to `MAX_ENVELOPE_RETENTION_SECS` = 30 d; deleted on ack | Node sweep; `node/hashgram-proto/src/limits.rs` |
| One-time key packages | Store node | Handed out once then deleted; `KEY_PACKAGE_TTL_SECS` = 30 d advertised | Node |
| Last-resort key package | Store node | Replaced on republish; 30 d expiry | Node |
| Blobs (Drive objects, attachments, media, manifests) | Store / media node | **No provider-enforced TTL today.** Held until operator deletion or a `BLOCK` attestation | Operator only |
| Incomplete blob uploads | Store / media node | 24 h (audit F4 fix) | Node sweep |
| Public social events | Store node social log | Configurable, default 90 d node-side; indexer keeps its projection | Node config; indexer policy |
| Drive manifest blob (old revisions) | Store node | As blobs: indefinite; the SDK does not delete superseded revisions | — |
| Local store (mail, flags, contacts, Space state) | Client device | Indefinite until the user deletes; `expire_after_secs` honoured by cooperating clients only | Client |
| MLS state | Client vault | Indefinite; persists until the device leaves the group | Client |
| Node logs | Journal | Recommended 2 weeks (`LOGGING_POLICY.md`) | Operator |
| Indexer | PostgreSQL | Indefinite; rebuildable from chain and nodes | Operator |

The blob row is the important one: **a Drive object or attachment, once
uploaded, should be assumed to exist as ciphertext somewhere for as long as
any provider keeps it.** Permanent delete in Drive removes the entry from
the manifest and the key from the user's devices; it does not and cannot
remove the ciphertext from providers. `ADR_HASH_STORAGE_MARKET.md` proposes
lease-bound retention, which would give providers a reason to delete
unleased blobs; it is not on this branch.

---

## 8. Logging policy summary

`LOGGING_POLICY.md` is normative. For Hashgram One specifically:

* Subjects, bodies, filenames, folder names, Space names, contact names,
  capabilities and any key material are **never** logged at any level in
  any component (node, SDK, gateway, indexer).
* Addresses, mailbox ids, device keys and CIDs may appear at `info` and
  below on a node the operator runs for themselves; they are **never**
  metric labels (checked by `scripts/dev/check-logging.sh`, check 4).
* The SDK emits `tracing` spans with group ids and message ids (random
  16-byte values) but never content; subjects and bodies are not present in
  any exported type's `Debug` output (`ObjectKey` in `drive.rs` implements
  `Debug` as `finish_non_exhaustive()` for this reason).
* The gateway logs SMTP envelope metadata (sender, recipient, size,
  auth results) as any MTA does, and must not log bodies.
* Chain events carry identifiers, amounts, addresses and outcomes only.

---

## 9. Summary table: who learns what about one mail with a Drive attachment

Alice (2 devices) sends mail with a live Drive attachment to Bob (1 device)
and Carol (2 devices), BCC Dan (1 device).

| Party | Learns |
| --- | --- |
| Store nodes for Bob, Carol, Alice's second device | One envelope each, of the mail's ciphertext size, from an unauthenticated depositor at time *t*; by §4.1, that the mailbox belongs to Bob/Carol/Alice |
| Store node for Dan | One envelope of the same size at *t* (a different group, same ciphertext size) |
| Media/store node holding the attachment | A private blob of ⌈size/1 MiB⌉ chunks uploaded by Alice's device key; later fetched by Bob's, Carol's and Dan's device keys |
| Bob, Carol | The mail, each other's addresses, the attachment, the capability (can read all future versions until revoked) |
| Dan | The mail, that it was sent To Bob and Carol, the attachment; `bcc_copy = true` |
| Bob and Carol about Dan | Nothing |
| Chain, indexer, relay | Nothing about the mail; that Alice, Bob, Carol, Dan have identities and devices (already public) |
| Global observer | That Alice's IP deposited to store nodes X, Y, Z at *t*; Bob's, Carol's and Dan's IPs fetched shortly after; by §4.1–4.2, plausibly that the four are in contact |
| Gateway | Nothing (native mail) |

That last-but-one row is the honest summary of this document.
