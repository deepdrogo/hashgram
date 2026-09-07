# Moderation

Hashgram carries two kinds of content and treats them differently on
principle. Private messages are end-to-end encrypted; no node, indexer or
safety engine can read them, so no mechanism here reaches them. Public
content — posts, reels, profiles, comments, public media — is reviewed by a
Safety Engine that signs verdicts, and nodes and indexers that trust that
engine's key enforce them. Nobody has to trust any particular engine.

## What an attestation is

```protobuf
message ContentAttestation {
  string network_id = 1;   uint32 version = 2;
  bytes cid = 3;  bytes event_id = 4;  bytes content_hash = 5;   // exactly one set
  Verdict verdict = 6;     // ALLOW | RESTRICT | QUARANTINE | BLOCK | UNBLOCK
  string policy = 7;       // e.g. "hashgram-public-v1"
  string reason_code = 8;  // machine code: "scam-airdrop", "csam-hash-match", never free text
  uint64 timestamp = 9;    bytes attestor_pubkey = 10;   bytes signature = 11;
}
```

An attestation can name a blob CID, a social event id, or a public media
content hash. It cannot name an encrypted envelope: an envelope has none of
those, and a node could not act on one if it did. Reason codes are
restricted to `[a-z0-9._-]` so an attestation can never carry the content it
judges. `UNBLOCK` supersedes `BLOCK` by timestamp.

## Enforcement

- **Nodes** (`hashgram-node`) keep every attestation they see, gossip only
  those from attestors in `trusted_attestors`, and for trusted `BLOCK`
  verdicts stop serving the event or blob and refuse to relay it.
- **Indexers** never return blocked content; with `?safe=1` they also hide
  content the author marked sensitive and content a trusted attestor
  `RESTRICT`ed (age gate / limited distribution). `QUARANTINE` is for
  operators running "hold until reviewed" indexers together with
  `publish_allow`.
- **Clients** may apply their own policy on top; they receive
  `sensitive` and `min_age` from events directly.

Trust is a configuration decision per node and per indexer. Two operators
may trust different engines; the network carries all signed verdicts.

## The Safety Engine

`hashgram-safety` runs as its own Unix user with no access to the node's
data directory (`InaccessiblePaths=/var/lib/hashgram/node` in its unit). It
reads public events and media through the node's loopback API, runs a
pipeline of stages, and publishes signed verdicts back through the node.

Stages, all pluggable (`safety/pipeline.go`):

| Stage | Input | Output |
| --- | --- | --- |
| hash list | BLAKE3 of public media plaintext | `BLOCK` with the list's reason code — how industry hash lists plug in |
| text rules | post text, captions, comments, profile fields | regex → `BLOCK`/`RESTRICT`/`QUARANTINE` with a reason |
| HTTP model | text and, if enabled, media bytes | any classifier with a small JSON interface; verdicts below `model_min_confidence` are ignored |

The most severe finding per subject wins. A stage that errors is reported
and skipped: a broken classifier must neither silently allow nor silently
block. Content bytes are dropped as soon as a review completes.

Manual review is a command:

```text
hashgram-safety attest --event <hex> --verdict BLOCK  --reason manual-review
hashgram-safety attest --cid   <hex> --verdict UNBLOCK --reason appeal-upheld
```

## Configuration

`/etc/hashgram/safety.toml` (written by the installer): `node_api`, `home`,
`policy`, `hash_list_file`, `text_rules_file`, `model_url`,
`model_send_media`, `model_min_confidence`, `max_media_bytes`,
`publish_allow`. The attestor key is generated on first run in `home`
(`attestor.key`, 0600); `hashgram-safety key` prints the public key to put in
`trusted_attestors` on nodes and indexers that should enforce it.

The repository ships **no** hash lists and **no** classifier. Those are
operator and jurisdiction decisions; the engine gives them a place to plug
in and a signed, auditable output.

## Verified behaviour

In `scripts/testnet/phase2.sh` a post matching a scam rule is blocked within
seconds: the safety log shows `verdict=CONTENT_BLOCK`, the node stops serving
the event, the indexer's author feed omits it. In development an `UNBLOCK`
restored it on both.

## Reporting

There is no protocol message for user reports yet. Applications should send
reports to the operator of the engine they trust; a report can name an event
id or CID, which is what an attestation needs.

## What this is not

- Not deletion: blocked content remains in the logs of nodes that do not
  trust the attestor, and in the author's device. Blocking limits
  distribution through cooperating infrastructure.
- Not identity action: an attestation names content, not accounts. Account
  level consequences (unbonding a provider, refusing a device) are chain
  matters with their own rules.
- Not a view into private messages, by construction.
