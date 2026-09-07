# Messaging

Private messaging on Hashgram is end-to-end encrypted with MLS (RFC 9420,
via OpenMLS) and delivered through store-and-forward mailboxes on `store`
nodes. This document describes the model, the delivery path, what a node can
and cannot learn, and the client procedures.

## What a node can learn

A store node holding a message for an offline device knows: the mailbox id
(a hash of the recipient device's public key), when the envelope was posted,
when it expires, and its size. It does not know the sender, the group, or a
byte of content, and it holds no key that could change that. The acceptance
test greps every store node's database for the plaintext of a delivered
message and fails if it is found (`scripts/testnet/phase2.sh`, "no plaintext
in any store node's mailbox database").

## Model

```text
Account (hash1…)  ─ owns ─▶ Identity (root key, on chain)
                                 ├─ Device A (ed25519, on chain)   ← MLS member
                                 ├─ Device B                      ← MLS member
Conversation = one MLS group; every device of every participant is a member.
```

- **Ciphersuite** `MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519`, fixed.
- **Credential**: basic credential with identity `<address>/<hex device key>`
  whose signature key *is* the device key registered on chain. To trust a
  member's claim to be Alice, a client asks the chain whether that key is
  one of Alice's active devices. There is no identity server.
- **1:1 chats** are two-member groups (or more, when either side has several
  devices). Direct chats are recognised as groups with two addresses and no
  name and are reused for both directions.
- **Multi-device**: adding a participant adds every active device the chain
  lists for them; a device never shares a private key with another.
- **Forward secrecy and post-compromise security** are MLS's: every commit
  ratchets the group; a removed device cannot read what follows (tested in
  `node/hashgram-mls`).

## Key packages

To be added to a group a device needs a published MLS key package. Devices
publish two kinds to store nodes (`KeyPackagePublish`, signed with the device
key under `key-package`):

- **one-time** packages, kept in a queue (up to 64 per device per node) and
  handed out once each, like one-time prekeys;
- a **last-resort** package (MLS last-resort extension) served when the queue
  is empty, so a device that has been offline for a long time can still be
  added, at the cost of weaker forward secrecy for that Welcome.

The client publishes a last-resort package plus enough one-time packages to
keep eight queued at each store node, and replenishes on every sync. A store
node that receives a device's key package advertises
`hashgram/mailbox/<mailbox id>` in the DHT, which is how senders find it.

## Delivery

```text
sender                             store nodes (providers of the mailbox)     recipient device
MLS encrypt ─▶ Envelope ─────────▶ MailboxPut  (stored, ciphertext only)
                                   MailboxNotify gossip (mailbox id only)
                                                ◀──── MailboxFetch (signed, fresh timestamp)
                                                ────▶ envelopes, cursor
                                                             MLS process ─▶ plaintext
                                                ◀──── MailboxAck (signed) ─▶ deleted
```

```protobuf
message Envelope {
  string network_id = 1;   uint32 version = 2;   bytes id = 3;      // BLAKE3 of the rest
  bytes mailbox = 4;       EnvelopeKind kind = 5;                   // MLS_WELCOME | MLS_MESSAGE
  bytes ciphertext = 6;    uint64 created_at = 7;  uint64 expires_at = 8;
}
```

- A message to a group produces one MLS ciphertext and one envelope per
  other member device, each put to up to three providers of that device's
  mailbox (DHT), falling back to any connected store node.
- Anyone may deposit; only the device whose key hashes to the mailbox may
  fetch or acknowledge, proven by a signature over a timestamp that must be
  within 300 s. Per-mailbox quota: 2,000 envelopes, 64 MiB.
- Expiry is requested by the sender and clamped to 30 days; a sweep deletes
  expired envelopes read or not. Acknowledged envelopes are deleted at once.
- The same envelope arriving from two store nodes is recognised by id on the
  client (a bounded seen-set) before MLS would refuse it as a reused secret.
- Relay receipts: after a fetch that delivered bytes, the client signs a
  `ServiceReceipt` for the store node's operator (see
  `docs/SERVICE_REWARDS.md`). The client is never blocked by this.

## Content format

Inside the MLS application message is `hashgram.chat.v1.ChatMessage`
(`proto/hashgram/chat/v1/chat.proto`): `TEXT`, `EDIT`, `DELETE`, `REACTION`,
`READ`, `DELIVERED`, `TYPING`, `GROUP_INFO`, `CALL` (signalling), `VOICE`;
attachments are references to encrypted blobs with the key and nonce inside
the message (`docs/STORAGE.md`); `disappear_after_secs` is a client-enforced
timer. No node decodes this format; it is versioned and bounded anyway,
because a malicious member can send anything.

## Client procedures (SDK)

`hashgram-sdk::messaging::Messaging`:

| Operation | What happens |
| --- | --- |
| `publish_key_package` | last-resort + one-time packages to every store peer |
| `create_conversation(name, participants)` | new group; fetch devices from chain, key packages from stores; `add_members`; Welcome to each new device; `GROUP_INFO` if named |
| `add_participant` / `remove_participant` | commit to existing members, Welcome to new ones / removal commit |
| `send_text`, `send_call_signal`, `send` | encrypt once, deliver per device |
| `sync` | fetch every provider of own mailbox, process, ack, replenish key packages, sign relay receipts |
| `persist` | MLS state snapshot (all group secrets) into the encrypted vault |

The developer client exposes these as `hashgram-client message send|receive|list|send-group`
and `group create|add|remove|members`.

## Storage on the device

MLS state lives in memory and is snapshotted into the vault
(`docs/CLIENT_CONNECTIVITY_SPEC.md`, "Keystore"), encrypted with
XChaCha20-Poly1305 under an Argon2id key. Nothing is written in the clear.
Losing the vault loses group state; the device is then re-added by another
device or by the peers, exactly as with any MLS client.

## Known limitations

- Store nodes see mailbox ids and traffic timing. Cover traffic and mixing
  are not implemented; a global passive observer of all store nodes could
  correlate who talks when.
- Devices must poll; there is no push. The mailbox notify gossip is a hint
  a connected client can act on, not a push channel.
- Message history is not stored on the network. A new device sees messages
  from the point it was added.
- Sender identity inside a group is authenticated by MLS, but the *chain
  check* that a member's device key belongs to the claimed address is the
  client's responsibility at add time and on membership changes; the
  reference client performs it when adding, and shows credential identities
  in `group members`.
