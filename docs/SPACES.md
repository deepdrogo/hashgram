# Spaces and Circles — Specification

Status: describes the code on the `hashgram-one` branch. Rules and
projections: `node/hashgram-app/src/space.rs`, `circle.rs`, `signing.rs`.
Transport and MLS reconciliation: `sdk/rust/hashgram-sdk/src/spaces.rs`,
`circles.rs`, `messaging.rs`. Wire shape: `proto/hashgram/app/v1/app.proto`.
Harness: `hashgram-client one space …`, `one circle …`. Companion documents:
`HASHGRAM_ONE_ARCHITECTURE.md` §8–9, `MESSAGING.md`, `MULTI_DEVICE_SECURITY.md`,
`HASHDRIVE.md` §6, `PRIVACY_MODEL.md` §3.6.

Both are private groups built on one MLS group each. A **Circle** is flat:
membership is the MLS roster and every member may post. A **Space** adds a
signed, hash-chained event log with roles that every member replays to the
same state, so authorisation does not depend on trusting whoever holds the
group.

---

## Part I — Spaces

### 1. Model

```
Space = MLS group (confidentiality; which devices are in the room)
      + SpaceEvent log (authorisation; who may change the room)

  every member: receive SpaceEvent ─▶ verify signature ─▶ actor == MLS sender?
                ─▶ State::apply (sequence, role rules) ─▶ same State everywhere
```

`SpaceEvent{version, space_id (16 B, = event_id of the Create), event_id
(16 B), actor (hash1…), device_pubkey (32 B), sequence, parent_id (0 or 16
B), at_ms, body, signature (64 B)}`. Bodies: `SpaceCreate`,
`SpaceInfoUpdate`, `SpaceMemberAdd`, `SpaceMemberRemove`, `SpaceRoleChange`,
`SpaceAnnouncement`, `SpaceDriveShare`, `SpaceDriveUnshare`, `SpacePost`,
`SpaceComment`.

Roles (`SpaceRole`, ranked by `space::rank`): `NONE 0 < GUEST 1 < MEMBER 2
< ADMIN 3 < OWNER 4`.

Bounds (`space.rs`): `MAX_MEMBERS = 10,000`, `MAX_NAME = 128`,
`MAX_TEXT = 64 KiB` (description, announcement, post, comment),
`MAX_PENDING = 4096`, `MAX_CONTENT = 50,000` in-memory content rows,
`MemberRemove.reason ≤ 512`, `SpaceDriveShare.path ≤ 1024`.

### 2. Signature domain

`hashgram_app::signing` gives application objects their own domain family,
disjoint from the 13 protocol purposes in `hashgram-net` (the prefix
`hashgram-app/` differs from the protocol's `hashgram/` in its first bytes;
tested in `app_and_protocol_domains_are_disjoint`):

```
domain   = "hashgram-app/v1/<network_id>/space-event"      (APP_DOMAIN_VERSION = 1,
                                                            AppPurpose::SpaceEvent)
preimage = network_magic(4) ‖ u64be(len(domain)) ‖ domain ‖ u64be(len(payload)) ‖ payload
digest   = SHA-256(preimage);  signature = ed25519(device key, digest)
```

`payload = space::canonical_payload(e)`, built with `hashgram_net::CanonicalBuf`
(big-endian integers; bytes and strings prefixed with `u32be(len)`), in
this exact order:

```
u32 version
bytes space_id
bytes event_id
string actor
bytes device_pubkey
u64 sequence
bytes parent_id
u64 at_ms
u32 body_tag          (Create 10, Info 11, MemberAdd 12, MemberRemove 13,
                       RoleChange 14, Announcement 15, DriveShare 16,
                       DriveUnshare 17, Post 18, Comment 19)
bytes body            (protobuf encoding of the body message)
```

`space::sign(network, device, &mut e)` sets `device_pubkey` and
`signature`; `space::verify(network, e)` runs `validate` (structure and
bounds) and then `signing::verify`. Signatures are bound to the network, so
a devnet event cannot be replayed on Mainnet.

### 3. Role rules, exactly as `State::apply` enforces them

`State::apply(network, e, mls_sender)` rejects, in order: wrong `space_id`
(`Rejected::WrongSpace`); bad signature or structure (`Rejected::Invalid`);
`e.actor != mls_sender` (`Rejected::Forbidden{action: "act as another
address"}`); a `Create` for a space that already exists; anything else
before the `Create` (`Rejected::Pending("space not created yet")`);
duplicate `event_id` (`Rejected::Duplicate`); `sequence ≤` last accepted
for that actor (`Rejected::Sequence`). Then:

| Event | Allowed when | Effect |
| --- | --- | --- |
| `Create` | space does not exist; `body.owner == actor`; `space_id == event_id` | actor becomes OWNER; name, description, `created_at_ms` set |
| `Info` | actor ≥ ADMIN | name (if non-empty), description, avatar (if present) |
| `MemberAdd{address, role}` | actor ≥ ADMIN; `role` ∈ {GUEST, MEMBER, ADMIN} (`validate`); `rank(role) ≤ rank(actor)`; adding an ADMIN requires OWNER; target not already a member; `< MAX_MEMBERS` | member inserted with `since_ms` |
| `MemberRemove{address}` | target is a member (else `Pending`); **either** self-leave (`address == actor` and actor is not OWNER) **or** actor ≥ ADMIN and `rank(target) < rank(actor)` | member removed |
| `RoleChange{address, role}` | target is a member (else `Pending`); `role != NONE`; OWNER may set any role on anyone; ADMIN may change a target below ADMIN to ≤ MEMBER; nobody but the OWNER may change their own role | role set; if `role == OWNER` the previous owner (the actor) becomes ADMIN |
| `Announcement` | actor ≥ ADMIN | content row `kind = "announcement"` |
| `DriveShare{capability, path}` | actor ≥ MEMBER | `state.drive[share_id] = SharedEntry{capability, path, by, at_ms}` |
| `DriveUnshare{share_id}` | share known (else `Pending`); actor is the sharer or ≥ ADMIN | entry removed |
| `Post` | actor ≥ MEMBER (GUEST is read-only) | content row `kind = "post"` |
| `Comment{post_id}` | actor ≥ MEMBER; the post exists (else `Pending`) | content row `kind = "comment"` |

The OWNER cannot self-leave (`self_leave` is false for the owner) and
cannot be removed by anyone; ownership is transferred with a `RoleChange`
to OWNER, after which the former owner is an ADMIN and can leave. On
success the event's `sequence` is recorded for the actor, its id is added
to `applied`, and `head` becomes the event id.

### 4. Ordering, replay protection, pending events

* **Per-actor sequence.** `State::next_sequence(actor)` is last + 1;
  `apply_verified` refuses `sequence ≤ last`, so a replayed or re-signed
  older event is `Rejected::Sequence` and a byte-identical replay is
  `Rejected::Duplicate`.
* **Order of application.** The SDK sorts stored events by `(at_ms,
  event_id)` when replaying (`Spaces::state_mut`, `Spaces::invite`).
  `parent_id` (the actor's last seen head) is a causal hint only; it is
  signed but not enforced.
* **Pending.** An event that depends on state not yet seen — a comment on
  an unknown post, a remove/role change of an unknown member, an unshare of
  an unknown share, anything before `Create` — returns `Rejected::Pending`
  and is kept (up to `MAX_PENDING`); after every successful apply
  `retry_pending` re-runs the set until no progress. A post from a
  non-member is **not** pending: it is a definitive `Forbidden` against the
  current state (tested in `out_of_order_events_become_applicable`).

`Rejected` variants: `Invalid(AppError)`, `Forbidden{actor, role, action}`,
`Sequence{actor, got, last}`, `Duplicate`, `Pending(&str)`, `WrongSpace`.

### 5. The SDK: sending, receiving, MLS reconciliation

`Spaces::emit(space, body)` builds with `space::build(space_id, me,
next_sequence(me), head_id(), body)`, signs with the device key, **applies
locally first** (so our own rule violation fails before anything leaves
the device — this is what the e2e check "member cannot announce" observes
as `space rule: actor … (Member) may not announce`), records the event in
the local store under `spaces/events/<space>/<at_ms>/<event_id>` together
with the sender, and sends `AppMessage{space_event}` to the Space's MLS
group.

`Spaces::create(name, description)` creates an MLS group with our own
devices (falling back to `MlsClient::create_group` on a single-device
account), tags it `group_kind::SPACE`, binds `space_id → group_id` under
`spaces/group`, and emits `Create`.

`Spaces::invite(space, address, role)`:
1. emits `MemberAdd` (rule check happens here);
2. `Messaging::add_participant` — every active on-chain device of the
   address joins the MLS group in a new epoch;
3. re-sends the **whole stored event log** in `(at_ms, event_id)` order to
   the group so the newcomer can replay roles. Existing members receive
   these as `Rejected::Duplicate` and ignore them.

`Spaces::remove(space, address, reason)` emits `MemberRemove` then
`Messaging::remove_participant` (an MLS removal commit; from the next epoch
the removed devices decrypt nothing). `set_role`, `set_info`, `announce`,
`post`, `comment`, `share_drive`, `unshare_drive` are thin `emit` wrappers.

`Spaces::handle_incoming` binds the space to the MLS group on first sight
and **refuses an event for a known space arriving from a different group**
(a member cannot move a Space). Events accepted or `Pending` are recorded;
`Duplicate` and other rejections are dropped with a debug log. The sync
engine emits `SyncEvent::SpaceActivity(space_hex)` per accepted event.

`Devices::reconcile` (every `reconcile_every = 20` sync rounds) also acts
on Space groups: revoked devices are removed by MLS commit and new devices
of existing members are added, independently of the event log.

### 6. Space drive

`Spaces::share_drive(space, entry, path, live)` calls
`Drive::grant_in_group(entry, "space:<space_hex>", gid, mode, READ)` — a
`DriveCapability` (file, or folder with a sealed `DriveFolderManifest`)
granted inside the Space's MLS group — then emits
`SpaceDriveShare{capability, path}` and commits the Drive manifest. Live
shares receive `DriveShareUpdate`s in the same group when the owner edits.
`Spaces::drive_entries` lists `SharedEntry`s sorted by `path`, then name;
the CLI prints `Contracts/contract.txt`. Only the sharer or an ADMIN may
unshare. Revocation semantics are those of `HASHDRIVE.md` §6.1.

### 7. Space mail

`Spaces::mail(space, subject, body)` resolves every member except us
through `Mail::resolve_recipients` and sends one native `Draft` with `to =
members` and the label `space:<space_hex>`. It is ordinary HashMail: the
recipient set is the member list at send time, so it travels in a
conversation group keyed by that set, not in the Space's own MLS group.

### 8. What is NOT on chain

Nothing. Space ids, names, members, roles, events, drive shares and the MLS
group are never written to the chain and never seen by a node in the clear.
The chain is consulted only for device keys (MLS credentials, membership
reconciliation) and address resolution.

### 9. CLI

```bash
SP=$(hashgram-client one space create Team --description 'e2e')
hashgram-client one space invite $SP hash1bob… --role member      # guest|member|admin
hashgram-client one space announce $SP Welcome 'first'            # admin+
hashgram-client one space post $SP 'hello'                        # member+
hashgram-client one space comment $SP <post_hex> 'nice'
hashgram-client one space role $SP hash1bob… admin                # owner only for admin/owner
hashgram-client one space role $SP hash1bob… owner                # ownership transfer
hashgram-client one space remove $SP hash1bob… --reason 'left the team'
hashgram-client one space remove $SP <my address>                 # leave (non-owner)
hashgram-client one space info $SP 'New name' --description '…'
hashgram-client one space share-drive $SP <entry_hex> --path Contracts   # --snapshot for fixed
hashgram-client one space drive $SP
hashgram-client one space mail $SP 'Subject' 'body'
hashgram-client one space list; one space show $SP; one space members $SP; one space content $SP
```

---

## Part II — Circles

### 10. Model

A Circle **is** an MLS group tagged `group_kind::CIRCLE`; membership is the
MLS roster (`Circles::list` refreshes `members` from
`Messaging::conversations()`). Content is `CircleEvent{version:
CIRCLE_VERSION = 1, event_id (16 B), at_ms, body}` with bodies
`CircleInfo`, `CirclePost{text, media, drive_refs, poll}`,
`CircleComment{post_id, reply_to, text}`, `CircleReaction{target_id,
reaction}` (empty reaction removes), `CircleVote{post_id, option_indexes}`,
`CircleDelete{target_id}`. There is no signature: the author is the MLS
sender, and there are no roles to enforce.

Bounds (`circle.rs`): `MAX_TEXT = 64 KiB`, `MAX_MEDIA = 20`,
`MAX_POLL_OPTIONS = 12` (and ≥ 2), poll question ≤ 512, option ≤ 128,
reaction ≤ 32 bytes, name ≤ 128, description ≤ 4096, `MAX_TIMELINE =
50,000` rows. A post must have text, media, a drive ref or a poll.

### 11. `Timeline` projection

`Timeline::apply(author, e)` (author = MLS sender) is deterministic and
idempotent by `event_id` (`seen` set; duplicate delivery from several
devices or store nodes is ignored):

| Event | Rule |
| --- | --- |
| `Info` | replaces `info` |
| `Post` / `Comment` | appended as `Item{kind, author, at_ms, text, post_id, reply_to, media, drive_refs, poll, reactions, votes, deleted}` |
| `Reaction` | **one reaction per author per target**: a previous reaction by the same author is decremented/removed, the new one counted; empty string removes |
| `Vote` | one ballot per author per post; re-voting replaces; indexes ≥ option count dropped; deduplicated; single-choice polls keep only the first; **ignored after `closes_at_ms`** (when non-zero) |
| `Delete` | **author-only**: marks `deleted`, clears text, media and drive refs; a delete from anyone else is ignored |

`posts(before_ms, limit)` returns posts newest first; `comments(post_hex)`
oldest first. `Circles::merged` interleaves posts from every circle for the
Feed's private view.

### 12. Membership and history

* `Circles::create(name, description, members)` → `Messaging::create_conversation`
  with the initial members, tags the group, stores `CircleInfo`, sends an
  `Info` event.
* `Circles::add_member(circle, address)` → `Messaging::add_participant`: all
  of the address's active devices join in a **new MLS epoch**, so they
  cannot decrypt anything sent before.
* `Circles::remove_member` → `Messaging::remove_participant` (removal
  commit; nothing decryptable from the next epoch). `Circles::leave`
  removes our own devices and deletes the local `CircleInfo`.
* **History policy.** `CircleInfo.history_visible_to_new_members` is
  carried in the proto and the SDK currently always sends `false`. Nothing
  automatically re-sends earlier posts to a newcomer; a client that honours
  `true` would have to re-share old events deliberately. Compare Spaces,
  where the log **is** replayed so roles can be reconstructed.

`Circles::handle_incoming` accepts a `CircleEvent` only in a group tagged
`CIRCLE` or not yet tagged (then it tags it and stores a `CircleInfo` with
`owner = false`); a circle event inside a conversation, self or Space group
is ignored, so a mail thread cannot be turned into a circle by a member.
Events are stored under `circles/events/<circle>/<at_ms>/<event_id>` and the
timeline is rebuilt lazily from the store.

### 13. CLI

```bash
CI=$(hashgram-client one circle create Family --member hash1bob… --member hash1carol…)
hashgram-client one circle add $CI hash1dave…
hashgram-client one circle post $CI 'dinner at 8?'
PID=$(hashgram-client one circle poll $CI 'pizza or sushi' --option pizza --option sushi)
hashgram-client one circle vote $CI $PID --choice 1
hashgram-client one circle react $CI $PID '👍'
hashgram-client one circle comment $CI $PID 'sushi!'
hashgram-client one circle posts $CI            # shows "votes {1: 1}" after the vote
hashgram-client one circle comments $CI $PID
hashgram-client one circle remove $CI hash1dave…
hashgram-client one circle leave $CI
```

### 14. Limitations

Spaces:
* `parent_id` is a hint; ordering is by sender clocks, so a member with a
  wrong clock can have their events applied out of intended order (never
  out of rule).
* A newcomer receives the history as a burst of re-sent events; there is no
  compaction, so invite cost grows with log length.
* `DrivePermission::WRITE` is modelled but no code path lets a grantee
  publish a new version.
* MLS membership follows the role table only when the acting client is
  online: `invite`/`remove` do both steps; an event applied from the log
  alone does not trigger an MLS add or remove on the receiving device.
* Pending events are in-memory only (`#[serde(skip)]`) and are rebuilt from
  the stored log on the next open.

Circles:
* **No roles.** Every member can post, comment, react, vote — and invite:
  `add_member` is not gated, so anyone in the circle can add anyone.
* Anyone can remove anyone at the MLS level for the same reason.
* `Delete` is author-only; there is no moderator delete.
* `history_visible_to_new_members` is not acted upon by this SDK.
* `CirclesState` is not persisted; timelines are re-projected from stored
  events at first access after open.
