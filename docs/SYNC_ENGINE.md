# Sync Engine — Specification

Status: describes the code on the `hashgram-one` branch. Engine:
`sdk/rust/hashgram-sdk/src/sync.rs`. Transport it drives: `messaging.rs`
(`Messaging::sync`), `blob.rs`, `link.rs`, `devices.rs`, and the
per-application `handle_incoming` functions in `mail.rs`, `drive.rs`,
`people.rs`, `circles.rs`, `spaces.rs`. Facade: `app.rs`
(`HashgramOne::open`, `save`). Companion documents:
`HASHGRAM_ONE_ARCHITECTURE.md` §14, `MESSAGING.md`, `HASHMAIL.md`,
`HASHDRIVE.md`, `SPACES.md`, `MULTI_DEVICE_SECURITY.md`.

The sync engine is the one place where a Hashgram One client talks to the
network on a schedule. It is an explicit state machine driven by
`Sync::round()`, so a client owns the timing, and every round is resumable
and idempotent, so a round interrupted anywhere can simply run again.

---

## 1. State machine

`SyncPhase` (from `sync.rs`):

```
 Offline ──connect──▶ Connecting ──first verified peer──▶ Discovering
    ▲                     │ timeout                          │ peers/roles known
    │                     ▼                                  ▼
    └───── Backoff(n) ◀── (error) ◀───────────────────── Syncing{stage}
                                                             │ all stages done
                                                             ▼
                                                           Idle ──tick──▶ Syncing
```

| Phase | Set when |
| --- | --- |
| `Offline` | `SyncState::default()`; before the first round |
| `Connecting` | `round_inner` finds `link.peers()` empty; the round fails with `LinkError::NoPeer("any")` |
| `Discovering` | at least one verified peer; store peers are looked up (`peers_with_role("store")`); none → `Warning("no store node reachable; mailbox not synced")` and the round continues |
| `Syncing(Stage)` | one entry per stage, in the order of §2 |
| `Idle` | `round_inner` returned `Ok` |
| `Backoff{attempt, wait_secs}` | `round_inner` returned `Err`; `attempt = failures`, `wait_secs = backoff_secs(attempt)` |

`set_phase` emits `SyncEvent::Phase(p)` only on change.

---

## 2. Stages

`Stage` in execution order; each is one section of `Sync::round_inner`.

| # | Stage | What it does |
| --- | --- | --- |
| 1 | `Mailbox` | `Messaging::sync(link, network, Some(chain))`: fetch every store peer's page of envelopes for this device, process through MLS, ack, return `Received`s; then `dispatch` each to its application (§4). Also signs relay receipts for bytes served and replenishes key packages. |
| 2 | `Outbox` | Send to the self group: `Mail::take_pending_hint()` as `DeviceSync{mail_state}`; `People::take_snapshot_if_dirty()` as `DeviceSync{contacts}`. If `drive_state.dirty`, `Drive::commit()` (which itself announces the `DriveKeyring`). Then `Mail::purge()`. |
| 3 | `Feed` | `Feed::refresh()`: for every followed author plus ourselves, `refresh_author(a, 100)` from that author's stored cursor. |
| 4 | `Wallet` | `chain.balance(me)`; `NoAccount` is reported as 0; emits `SyncEvent::Balance`. |
| 5 | `Devices` | Only when `rounds > 0 && rounds % reconcile_every == 0`: `Devices::reconcile()` — remove revoked devices from every group by MLS commit, add missing active devices. |
| — | save | `HashgramOne::save()`: MLS snapshot, cursors, seen/failed sets, Drive keyring → vault. |

Failures inside stages 2–5 are reported as `SyncEvent::Warning` and do not
fail the round. Only a missing peer (before stage 1), a `Messaging::sync`
error, a `self_group()` error in the Outbox, or a failed `save()` return
`Err` and move the engine to `Backoff`.

---

## 3. Events and report

`SyncEvent` (delivered through the unbounded channel returned by
`Sync::subscribe()`):

| Event | Meaning |
| --- | --- |
| `Phase(SyncPhase)` | phase changed |
| `NewMail{id, folder}` | a `MailRecord` was filed (folder is where the spam policy put it) |
| `DriveShareChanged` | a capability arrived, was updated or revoked |
| `ContactsChanged` | request / response / card applied |
| `CircleActivity(circle_hex)` | a `CircleEvent` recorded |
| `SpaceActivity(space_hex)` | a `SpaceEvent` accepted or kept pending |
| `Balance(uhash string)` | wallet balance fetched |
| `UnsupportedMessage` | an `AppMessage` this build cannot read was stored opaquely |
| `Warning(String)` | non-fatal problem (text never contains content) |

`SyncReport` per round: `envelopes`, `mail`, `drive`, `people`, `circles`,
`spaces`, `device_sync`, `unsupported`, `chat: Vec<Received>` (legacy chat
lines handed back to the caller), `feed`, `drive_committed: Option<u64>`,
`elapsed_ms`, `balance_uhash: Option<u128>`. The CLI prints it as
`sync ok: N envelopes, N mail, N drive, …`.

---

## 4. Dispatch

`Sync::dispatch(received, report)`:

1. `HashgramOne::app_of(&r)` — a `ChatMessage` whose `kind != CHAT_KIND_APP`
   is a legacy chat line and goes to `report.chat`.
2. Version gate: `body.is_none()` (unknown oneof arm under proto3) or
   `version > APP_ENVELOPE_MAX_READ` → the raw `ChatMessage` is stored
   under `unsupported/<AppMessage.id>` with group and sender,
   `report.unsupported += 1`, `SyncEvent::UnsupportedMessage`. Partial
   rendering is never attempted.
3. Route by body:

| Body | Handler | Counted in |
| --- | --- | --- |
| `Mail`, `MailReceipt` | `Mail::handle_incoming` | `mail` (only when a new record was filed) |
| `DriveShare`, `DriveShareUpdate`, `DriveShareRevoke` | `Drive::handle_incoming` | `drive` |
| `ContactRequest`, `ContactResponse`, `ProfileCard` | `People::handle_incoming` | `people` |
| `CircleEvent` | `Circles::handle_incoming` | `circles` |
| `SpaceEvent` | `Spaces::handle_incoming` | `spaces` |
| `DeviceSync` | `Devices::handle_incoming` (refused unless the MLS sender is our own address) | `device_sync` |

A handler error becomes `Warning("<kind>: <error>")` with
`envelope::kind_name` as the kind; the round continues.

---

## 5. Idempotency

Three layers make re-running a round harmless.

**Envelope layer (`Messaging::sync`).** Per store peer, pages of 50 signed
`MailboxFetch`es from that peer's cursor. Within a page:

* envelopes are ordered **Welcomes first**, then by `(created_at, id)`,
  because a page routinely holds a Welcome and the first message of the
  same group with the same second-resolution timestamp;
* an id already in `seen` (last `SEEN_CAP = 5000` processed envelope ids,
  persisted under `VAULT_SEEN_KEY`) is acked without reprocessing — this is
  what makes a copy held by a second store node harmless, since MLS would
  refuse the second decryption as secret reuse;
* an envelope that fails is **retried once within the page** after the
  others (its Welcome may be later in the same page);
* an envelope that still fails is **not acknowledged**, so the store keeps
  it for the next round; its count in `failed` (`VAULT_FAILED_KEY`) is
  incremented, and at `FAILED_ATTEMPTS_CAP = 5` it is given up, marked
  seen and acked so a permanently undecryptable envelope does not stay
  forever;
* everything processed or skipped is acked in one signed `MailboxAck`.

**Application layer.** `AppMessage.id`/`message_id`/`event_id` dedup:
`Mail::receive_mail` returns early if the id is already indexed;
`Timeline::apply` and `State::apply_verified` ignore duplicates (`seen`
set, `Rejected::Duplicate`); `Drive::handle_incoming` overwrites the same
`share_id` record; `Contacts` transitions are idempotent.

**Device-sync layer.** `MailStateHint` lists are applied as absolute flag
settings; `ContactsSnapshot` merges newer-wins per address;
`DriveKeyring` is ignored unless its revision is higher.

---

## 6. Resumability

| Sub-sync | Resume point | Where kept |
| --- | --- | --- |
| Mailbox | cursor per store peer (`Messaging.cursors`, `VAULT_CURSOR_KEY`); a page's `cursor` becomes the next fetch's; empty cursor means the peer's mailbox is drained | vault |
| Blob upload | `BlobPutManifest` reply lists `missing_chunks`; only those are sent (`blob::upload_sealed` / `push_manifest_and_chunks`) | node |
| Blob download | chunk by chunk with per-chunk hash check; a failed provider is skipped for the next (`blob::download`) | — |
| Drive | manifest `revision`; `apply_keyring` fetches only a higher revision and merges; `dirty` survives a crash (manifest cached in the local store) | vault + local store |
| Feed | `cursor/<author>` = next sequence per followed author | local store |
| Rounds | `sync/meta/rounds` — the round counter is persisted so periodic work (device reconcile) is spread over real rounds, not repeated at every process start | local store |

---

## 7. Backoff, cadence, bounds

* `backoff_secs(attempt) = clamp(2^min(attempt, 9), 2, 300)`: 2, 4, 8, …,
  300 s. `failures` resets to 0 on a successful round.
* `Sync::next_delay()` returns `wait_secs` in `Backoff`, otherwise **4 s**.
* `SyncState.reconcile_every = 20` rounds (≈ 80 s at the idle cadence);
  `Devices::reconcile` is also run directly after `revoke_on_chain`.
* `link::PROVIDER_QUERY_TIMEOUT = 3 s`: a DHT provider lookup
  (`Link::providers`) returns whatever Kademlia found within 3 s and the
  caller falls back to its connected store peers, so a sparse network never
  waits out Kademlia's 30 s query timeout.
* Mailbox fetch page size 50; per-page ack; `seen` bounded at 5000 ids;
  `failed` cleared entirely if it ever exceeds 5000.

---

## 8. Offline start

`HashgramOne::open(config, passphrase)` → `Link::connect(network,
bootstrap, peerstore, config.connect_wait)`: the swarm starts, dials the
bootstrap list (compiled-in Mainnet peers when `bootstrap` is empty on
Mainnet), and waits **up to `connect_wait`** for every distinct bootstrap
peer id to verify, then returns whether or not any did. The chain client is
either REST (`chain_api`) or `chain_client_over_link` (allow-listed reads
over `/hashgram/rpc/1`). Every module's state is loaded from the vault and
the sealed local store before any network round trip: mail folders, the
Drive manifest, contacts, Space groups, Circle info.

Consequences for an application:

* Reading mail, Drive listings, contacts, Space and Circle state works with
  no peer at all.
* Local mutations work offline: Drive changes stay `dirty`, mail flags
  accumulate in `pending_hint`, contact changes set `dirty`; all are
  flushed by the next successful `Outbox` stage.
* Anything that must reach a node (`Mail::send`, `Drive::upload`,
  `commit`, `share`, `Spaces::invite`) fails with `SdkError::Link(NoPeer)`
  or `Delivery` and can be retried on the next round.
* `Sync::round()` with no peer sets `Connecting` and returns `Err`; the
  engine backs off; the link keeps dialling in the background.

The CLI harness passes `connect_wait = 10 s` and refuses to run if no peer
verified in that window, because every command there needs the network.

---

## 9. Conflict handling

| State | Rule | Code |
| --- | --- | --- |
| Drive manifest | per-entry last-writer-wins by `modified_at_ms` (device-key tiebreak), version union, sticky trash, tombstones with edit-after-delete resurrection, orphans to root, name-collision suffix | `hashgram_app::drive::merge`, `Drive::apply_keyring` |
| Contacts | newer `updated_at_ms` wins per address | `Contacts::merge_snapshot` |
| Mail flags | hints are applied as absolute settings in list order (`read`, `unread`, `archived`, `trashed`, `starred`, `unstarred`, `deleted`); a later hint from another device overrides | `Mail::apply_state_hint` |
| Space state | every device replays the same signed log; no merge needed | `space::State` |
| Circle timeline | deterministic projection by `event_id` | `circle::Timeline` |

Two devices that created separate Drives before ever syncing are **not**
merged: `apply_keyring` keeps the local Drive and logs a warning
(`HASHDRIVE.md` §5.1).

---

## 10. What a desktop application should do

```
let mut one = HashgramOne::open(config, passphrase).await?;   // returns after connect_wait at most
let mut events = one.sync().subscribe();                       // UnboundedReceiver<SyncEvent>

// background task
loop {
    let _ = one.sync().round().await;      // Err ⇒ phase is Backoff; events already emitted
    one.save()?;                           // round() saves on success; save again after your own mutations
    tokio::time::sleep(one.sync().next_delay()).await;   // 4 s idle, 2…300 s in Backoff
}

// UI task
while let Some(ev) = events.recv().await {
    match ev {
        SyncEvent::NewMail { id, folder } => refresh_folder(folder),
        SyncEvent::DriveShareChanged     => refresh_shared_with_me(),
        SyncEvent::Phase(p)              => show_connection_state(p),
        SyncEvent::Warning(w)            => log_or_toast(w),
        _ => {}
    }
}
```

Rules of engagement:

1. Call `round()` on a timer sized by `next_delay()`; never in parallel —
   the facade is `&mut self` and the MLS state is one object.
2. Call `save()` after every user action that touched MLS or the vault
   (send, share, invite, commit). `round()` saves at the end of a
   successful round only.
3. Render from local state, not from the report; the events say what to
   refresh.
4. Keep `SyncReport.chat` if you show legacy chat; otherwise drop it.
5. Show `Backoff{attempt, wait_secs}` and `Connecting` to the user; both
   are normal on a laptop that just woke up.
6. Treat `UnsupportedMessage` as "update the app"; the ciphertext is kept
   under `unsupported/` for a later build.

The CLI equivalent is `hashgram-client one sync --watch 5`.

---

## 11. Failure modes

| Failure | Where it surfaces | Engine behaviour |
| --- | --- | --- |
| No verified peer | `round()` → `Err(Link(NoPeer("any")))`, `Phase(Connecting)` | `Backoff`, retry with `backoff_secs` |
| Peers but no store role | `Warning("no store node reachable; mailbox not synced")` | round continues (Outbox may still fail with `NoPeer("store")` → `Warning`) |
| A store peer's fetch fails | `debug` log; that peer's loop breaks, cursor unchanged | other peers still synced; round `Ok` |
| Envelope fails MLS processing | not acked; `failed[id] += 1` | retried next round; given up after 5 (`warn`) |
| Application rejects a message (bounds, impersonation, rule) | `Warning("<kind>: <error>")` | message dropped or, for Spaces, `Rejected` logged; round `Ok` |
| Newer-protocol message | `UnsupportedMessage`, stored under `unsupported/` | round `Ok` |
| Flags/contacts not deliverable to self group | `Warning("mail flags not synced …")` / silent for contacts | hint already taken; contacts `dirty` cleared — the next change re-sends the whole snapshot |
| Drive commit fails | `Warning("drive commit deferred: …")` | stays `dirty`; retried next round |
| Feed refresh fails for an author | swallowed per author (`unwrap_or(0)`) | count omits that author |
| Balance query fails | `Warning("balance: …")` | no `Balance` event |
| Device reconcile fails | `Warning("device reconcile: …")` | retried after `reconcile_every` more rounds |
| `save()` fails | `round()` → `Err(...)` | `Backoff`; in-memory state remains, so the next successful round persists it |

Known gaps: a `MailStateHint` that fails to send is lost for the other
devices (the hint was already taken from `pending_hint`); envelopes given
up after `FAILED_ATTEMPTS_CAP` are gone from the store once acked; the
engine has no notion of "urgent" — a send does not trigger an immediate
fetch of the reply, the next timer tick does.
