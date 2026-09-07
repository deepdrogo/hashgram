# Social Protocol

Everything public on Hashgram is a signed event. Feeds, profiles, follower
counts and hashtag pages are projections of events that anyone can rebuild;
the event is the record. No node can forge one, because it cannot sign as the
author's device, and no node can decide a device's authority, because that
is recorded on chain.

## The event

```protobuf
message SocialEvent {
  string network_id = 1;    uint32 version = 2;
  bytes  id = 3;            // BLAKE3 of the domain-separated preimage; derived, not chosen
  string type = 4;          // one of the twelve families
  string author = 5;        // hash1…
  bytes  device_pubkey = 6; // raw ed25519, an active device of the author on chain
  uint64 timestamp = 7;     uint64 sequence = 8;  bytes previous_event = 9;
  bytes  payload = 10;      // canonical encoding of the typed payload
  repeated MediaReference media = 11;
  bytes  signature = 12;    // over fields 1,2,4..11 under "social-event"
}
```

Twelve families, each with a typed payload in `proto/hashgram/p2p/v1/social.proto`:

| Type | Payload | Notes |
| --- | --- | --- |
| `PROFILE_UPDATE` | display name, bio, avatar/banner CIDs, website, attributes | latest wins by timestamp |
| `FOLLOW` / `UNFOLLOW` | target | cannot follow yourself |
| `POST_CREATE` | text, hashtags, mentions, channel, reply_to, sensitive, language | needs text or media |
| `POST_EDIT` / `POST_DELETE` | post id (+ text) | only the author's own posts take effect |
| `COMMENT_CREATE` | post, text, parent comment | |
| `REACTION` | target, reaction | empty reaction removes |
| `REPOST` | post, comment | |
| `CHANNEL_CREATE` | name, description, avatar, open_posting | |
| `REEL_CREATE` | caption, hashtags, mentions, video_index, audio credit, sensitive, min_age, allow_comments, allow_remix | the referenced media must be a video |
| `STORY_CREATE` | caption, expires_at (≤ 48 h), sensitive | needs media |

Media never travels in an event: `MediaReference` carries a CID, MIME, size,
kind, dimensions, duration, thumbnail CID and, for public media, a content
hash so a safety verdict can name the bytes independently of chunking.

## Acceptance, in order

Every node that relays events runs the same pipeline; cheapest first.

1. **Structure and bounds** (`hashgram-proto::validate::social_event`):
   version 1, 32-byte id and device key, 64-byte signature, timestamp not
   more than 300 s ahead, sequence/previous consistency, payload ≤ 32 KiB,
   ≤ 20 media, text ≤ 10,000 bytes, ≤ 30 tags of ≤ 64 bytes, a known type,
   and the typed payload's own rules.
2. **Network, id and signature** (`verify_social_event`): the id must equal
   BLAKE3 of the preimage; the signature must verify for `device_pubkey`.
3. **Device authority**: the node asks the co-located chain
   (`/hashgram/identity/v1/resolve_device_key`) whether `device_pubkey` is
   an active device of `author`. Answers are cached ten minutes. If the chain
   is unreachable the event is *ignored* (not stored, not forwarded, peer not
   penalised).
4. **Not already held.** Two events with the same `(author, sequence)` and
   different ids are both kept as evidence of a misbehaving device.
5. **Not blocked** by a trusted safety attestation (`docs/MODERATION.md`).

A `Reject` at steps 1–2 costs the forwarding peer score; enough of them ban
it. Step 3 is what makes a relay unable to forge: it may sign anything, and
none of its keys is Alice's device.

## Transport

Events are gossiped on `hashgram/<network_id>/social/shard/<n>` with
`n = BLAKE3(author) mod 64`, and must travel on their author's shard (a
mismatch is a reject). Nodes subscribe to all 64 shards; sharding exists so a
client can subscribe to a slice. Clients publish through a node
(`EventPublish`), which validates fully before gossiping, and fetch by id or
by author from any node (`EventFetch`); the SDK re-verifies every event it
receives.

Every node keeps a log of the events it accepted (`social.redb`), bounded by
`event_retention_days` (90) and `max_events` (2,000,000), oldest dropped
first, and serves it over `/v1/social/events` for indexers.

## Per-author chain

Each device keeps a sequence counter and the id of its previous event in its
vault; the chain of `previous_event` links lets an indexer detect gaps and
lets two events with one sequence number be recognised as a fork of the
author's own history. Sequence space is per device in practice (two devices
of one account each keep their own); indexers order by timestamp.

## Projections

The indexer (`docs/OPERATIONS.md`, "Indexer") turns events into tables:
posts, comments, reactions, reposts, follows, profiles, channels, reels,
stories, and serves feeds:

| Feed | Query |
| --- | --- |
| chronological | posts by time |
| following | posts by the addresses `{address}` follows, plus their own |
| author, hashtag, channel | filtered by field |
| reels | with `?max_age=` age gate and `?tag=` |
| post detail | comments, reaction counts |
| profile | profile + username + follower/following/post/reel counts + devices |

Blocked content is never returned; `?safe=1` also hides content the author
marked sensitive and content a trusted attestor restricted. A client that
does not trust the operator's indexer verifies events itself; the indexer is
a cache, and `hashgramctl indexer rebuild` recreates it from nodes and chain.

## Reels and stories

A reel is a first-class event family, not a post with a video: it carries
`video_index`, `min_age`, `allow_comments` and `allow_remix`, and the video
is a public blob replicated to three providers (`docs/STORAGE.md`). Clients
should upload the video first, then publish the event with the CID. A story
carries its own expiry (≤ 48 hours); the indexer stops serving it after that
and nodes drop the media reference with the event on retention.

## What the protocol does not do

- No server-side ranking. Feeds are chronological or set-filtered; ranking
  is a client's choice over data it can verify.
- No deletion from the network. `POST_DELETE` is an instruction indexers and
  clients honour; the original event remains in logs until retention drops
  it. A block by a trusted attestor stops nodes serving it.
- No private posts. Anything not public goes through messaging.
