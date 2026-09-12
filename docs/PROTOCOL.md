# Hashgram Protocol

This is the wire-level specification of the Hashgram network: what a node
speaks, what a client speaks, and what every signature commits to. The chain
protocol (Cosmos SDK modules, CometBFT) is specified by the protobuf files in
`proto/hashgram/{network,founder,feerouter,welcome,serviceproof,username,identity,treasury}`
and by `docs/ARCHITECTURE.md`; this document covers everything off-chain and
the seams between the two.

The normative artefacts are the `.proto` files and the signing vectors in
`node/testdata/signing-vectors.json`. Where this document and a `.proto`
disagree, the `.proto` is right and this document has a bug.

## 1. Network identity

A Hashgram network is identified by five parts, all pinned on every node in
`/etc/hashgram/network.json`:

| Part | Mainnet | Devnet |
| --- | --- | --- |
| `network_name` | Hashgram Mainnet | Hashgram Devnet (DEVNET ONLY) |
| `network_id` | `hashgram-mainnet` | `hashgram-devnet` |
| `chain_id` | `hashgram-1` | `hashgram-devnet-1` |
| `network_magic` | `HGM1` | `HGD1` |
| `genesis_hash` | SHA-256 of the genesis file | SHA-256 of the genesis file |

`protocol_major_version` is `1`. The genesis hash is the only part a fork
cannot copy without becoming a different network, which is why the P2P
handshake checks it (§4) and CometBFT's own handshake, which does not, is not
relied on for that.

## 2. Canonical signing

Protobuf is not a canonical encoding, so nothing signs protobuf bytes.
Every signed object has a hand-built preimage, produced identically by Go
(`app/canonical`) and Rust (`hashgram-net::canonical`):

```text
Buf   (bounded fields):   u32be(len) || bytes    per variable-length field
Fixed (unbounded payloads): u64be(len) || bytes
```

The preimage is then wrapped in the network's signing domain:

```text
magic(4) || u64be(len(domain)) || domain || u64be(len(payload)) || payload
domain = "hashgram/v<major>/<network_id>/<purpose>"
digest = SHA-256(the above)
```

and the digest is what the signature scheme signs (ed25519 or secp256k1).
The thirteen purposes:

| Purpose | Signer | Object |
| --- | --- | --- |
| `social-event` | device key | `SocialEvent` |
| `device-cert` | root key | `DeviceCertificate`, root rotation |
| `service-receipt` | client device key | `ServiceReceipt` |
| `storage-challenge` | provider node key | `ChallengeResponse` |
| `eligibility-attestation` | welcome attestor | `x/welcome` attestation |
| `content-attestation` | safety attestor | `ContentAttestation` |
| `bootstrap-record` | release signer | `BootstrapRecord` |
| `peer-handshake` | reserved | — |
| `node-announce` | node key | `NodeAnnounce` |
| `mailbox-fetch` | device key | `MailboxFetch` |
| `mailbox-ack` | device key | `MailboxAck` |
| `key-package` | device key | `KeyPackagePublish` |
| `blob-upload` | device key | `BlobPutManifest` |

Purposes are distinct domains: a signature made for one never verifies under
another, and a signature made on devnet never verifies on mainnet. The test
vectors cover every purpose on both networks; `tools/signing-vectors`
regenerates them and CI fails if Go and the committed file disagree.

Per-object field orders are in `node/hashgram-proto/src/signing.rs`, each
mirrored by a Go implementation where Go verifies it (`indexer/social.go`,
`safety/attest.go`, `x/serviceproof/types`).

## 3. Transport

libp2p, QUIC first (`/udp/26670/quic-v1`) with TCP + Noise + Yamux as a
fallback (`/tcp/26670`). Peer identities are ed25519 libp2p keys; the peer id
is derived from the public key. Both transports are always dialled; which
are *listened on* is configuration.

Behaviours on every node: request-response on `/hashgram/rpc/1`, Gossipsub,
Kademlia (protocol `/hashgram/<network_id>/kad/1`, so a fork's DHT cannot
merge with ours), identify (`/hashgram/id/1`), ping, AutoNAT, circuit relay
(served by `relay` and `bootstrap` roles), DCUtR hole punching.

Connection limits: per peer, per /24 (IPv4) or /64 (IPv6), and global
inbound, with outbound slots reserved so inbound pressure cannot isolate a
node. Peer scoring is local and decaying; below −50 a peer is graylisted,
below −100 banned for an hour. Bans also block the peer at the connection
gate and remove it from the DHT.

## 4. The Hashgram handshake

The first request on every connection, sent by the dialer:

```protobuf
message Handshake {
  bytes  network_magic = 1;        // "HGM1"
  string network_id = 2;
  string chain_id = 3;
  string genesis_hash = 4;
  uint32 protocol_major_version = 5;
  repeated string roles = 6;       // informational
  string operator_address = 7;     // provider operator, for receipts
}
message HandshakeAck { bool accepted = 1; string reason = 2; Handshake identity = 3; }
```

The listener verifies all five identity parts in order (magic, version,
network id, chain id, genesis hash), answers with its own identity and the
reason if refusing, then disconnects and bans a refused peer. The dialer
verifies the responder's identity the same way. **Nothing else is served to
a peer that has not passed.** A peer that sends no handshake within ten
seconds is disconnected.

This is the check CometBFT does not perform. It was verified live: a fork
that kept the real chain id and changed only the genesis opened a CometBFT
transport connection; at the Hashgram layer it is refused with
`genesis hash mismatch, expected …, received …` and banned
(`scripts/testnet/phase2.sh`, "FORK REJECTION").

## 5. `/hashgram/rpc/1`

One request-response protocol with a typed body, framed as
`u32be(len) || protobuf`, `len ≤ 1 MiB + 64 KiB` (one blob chunk plus
headroom). The prefix is checked before the body is read. The bodies:

| Family | Request → Response | Served by role |
| --- | --- | --- |
| Handshake | `Handshake` → `HandshakeAck` | every node |
| Peer exchange | `PeerExchange` → `PeerExchangeResult` | every node (swarm answers) |
| Mailbox | `MailboxPut`, `MailboxFetch`, `MailboxAck`, `KeyPackagePublish`, `KeyPackageFetch` | `store` |
| Social | `EventFetch`, `EventPublish` | every node |
| Blob | `BlobGetManifest`, `BlobGetChunk`, `BlobHas`, `BlobPutManifest`, `BlobPutChunk` | `store`, `media` |
| Announce | `AnnounceQuery` → `AnnounceQueryResult` | every node |
| Safety | `AttestationQuery` | every node |
| Receipts | `ReceiptDeliver` | registered providers |
| Calls | `TurnCredentialRequest` → `TurnCredentialResult` | `call` |
| Chain relay | `ChainQuery` → `ChainQueryResponse`, `ChainBroadcast` → `ChainBroadcastResult` | `relay`, `bootstrap` (with a chain node) |

Errors are a typed `Error { code, message }` body with codes
`unauthenticated`, `unsupported`, `rate_limited`, `quota`, `not_found`,
`invalid`, `internal`. Inbound requests are rate-limited per peer (40/s
sustained, 80 burst); exceeding it costs score.

### Chain relay

A wallet needs two things from the chain — answers to read queries and a
way to hand in a signed transaction — and nodes bind their REST gateway to
loopback. `ChainQuery { path, query }` and `ChainBroadcast { tx_bytes }` let
a `relay` or `bootstrap` node forward those from the peers it already talks
to over this protocol to its co-located `hashgramd` gateway, so a client
needs no HTTP endpoint at all (`node/hashgram-node/src/chain_relay.rs`).

- **Allow-list, one definition.** Only read paths under `cosmos/bank/`,
  `cosmos/auth/`, `cosmos/staking/`, `cosmos/distribution/`, `cosmos/gov/`,
  `cosmos/base/`, `hashgram/`, plus `cosmos/tx/v1beta1/txs` (the list) and
  `cosmos/tx/v1beta1/txs/{64-hex hash}` are forwarded. `simulate` is
  refused (it executes a transaction against the gateway). Any path with
  `..`, `//`, `?`, `#`, whitespace, a backslash, a percent-encoded separator
  or a non-printable byte is refused as `invalid` before the gateway sees it.
  The local API's `/v1/chain/{path}` passthrough uses the same function.
- **Verbatim answers.** `ChainQueryResponse { status, body, height }` carries
  the gateway's HTTP status, the JSON body as served (≤ 64 KiB; larger
  answers come back as status 413 so the client narrows its query) and the
  block height from `grpc-metadata-x-cosmos-block-height`. Nothing is
  re-encoded, so two nodes can be compared byte for byte.
- **Bounds.** 5 s upstream timeout; a separate per-peer token bucket (10/s
  sustained, 20 burst) on top of the protocol-wide one; transactions ≤ 32 KiB;
  metrics `hashgram_node_chain_relay_requests{kind}` and
  `hashgram_node_chain_relay_outcomes{outcome}`.
- **Broadcast** is `POST /cosmos/tx/v1beta1/txs` in `BROADCAST_MODE_SYNC`.
  The relay only ever sees bytes the client signed.
- **Not rewarded.** Relaying chain reads earns no service credit; it is a
  public good like peer exchange (`docs/SERVICE_REWARDS.md`, "Known
  limitations").

The client side (`hashgram_sdk::chain_relay`) trusts no single relay: every
read goes to two peers run by different operators (operator address from
the handshake), answers are compared after JSON normalisation, heights must
be within 3 blocks, a mismatch marks both peers disputed and asks a third,
and the result is reported as "verified by N nodes" or refused. This is not
a light client — no Merkle proof is checked — and `docs/CLIENT_CONNECTIVITY_SPEC.md`
§10 lists that as still missing.

## 6. Gossip topics

```text
hashgram/<network_id>/social/shard/<0..63>   social events, shard = BLAKE3(author) mod 64
hashgram/<network_id>/channel/<hex id>       reserved for channel-only subscriptions
hashgram/<network_id>/tag/<hash>             reserved for tag subscriptions
hashgram/<network_id>/mailbox/<0..15>        "mailbox has mail" notifies, no content
hashgram/<network_id>/announce               NodeAnnounce
hashgram/<network_id>/safety                 ContentAttestation
```

Every gossip payload is a `Gossip` wrapper with a oneof body, so a message on
the wrong topic is a decode-level rejection. Message ids are BLAKE3 of the
payload (a re-signed duplicate is still a duplicate). Nodes run Gossipsub in
strict validation mode and report `Accept`, `Ignore` or `Reject` after the
application checks the message; a `Reject` costs the forwarding peer score,
a message from an unverified peer is ignored. A social event must travel on
its author's shard.

## 7. Discovery

No single source. A node dials, in order of availability: configured
`bootstrap_peers`, the persisted peerstore (peers that completed the
handshake before), signed `BootstrapRecord` files from trusted signers,
`/dnsaddr/` names if configured, and peers learned by peer exchange and
Kademlia. Kademlia also carries provider records:

```text
"hashgram/blob/"    || cid          who holds a blob
"hashgram/mailbox/" || mailbox_id   who holds a device's mailbox and key packages
```

Roles and services are advertised by signed `NodeAnnounce` messages (node
key signature, one-hour TTL, republished every ten minutes) carrying roles,
multiaddrs, operator address, declared storage and, for call nodes, TURN
URIs and an optional SFU URL. An announcement is a claim about
availability; what a node is paid for is settled on chain against evidence.

## 8. Identifiers

| Identifier | Definition |
| --- | --- |
| peer id | libp2p, from the node's ed25519 key |
| operator address | `hash1…` account that registered the provider |
| device key | raw ed25519 public key, registered on chain under an identity |
| mailbox id | BLAKE3(device public key) |
| event id | BLAKE3 of the domain-separated social-event preimage |
| envelope id | BLAKE3 of the envelope's canonical fields |
| CID | BLAKE3 of the canonical blob manifest (§`docs/STORAGE.md`) |
| chunk hash | BLAKE3 of the chunk (storage); SHA-256 leaf hash for chain challenges |

## 9. Limits

From `node/hashgram-proto/src/limits.rs`, enforced before allocation:

| Limit | Value |
| --- | --- |
| RPC frame | 1 MiB + 64 KiB |
| gossip frame | 64 KiB |
| chunk size | 1 MiB |
| blob | 4 GiB |
| envelope ciphertext | 256 KiB |
| envelope retention | 30 days |
| key package | 8 KiB |
| event payload | 32 KiB; 20 media refs; 10,000 bytes of text; 30 tags |
| story lifetime | 48 hours |
| future skew | 300 s; signed requests older than 300 s are replays |
| announcement TTL | 24 hours; 16 addresses |

## 10. Versioning

`protocol_major_version` appears in every signing domain and in the
handshake. Bumping it invalidates every signature and refuses every
connection to the old version — the intended cost of a breaking wire
change. Additive changes (new oneof variants, new fields with defaults) do
not bump it; unknown variants are refused as `invalid`, unknown fields are
ignored by protobuf.

## 11. What is not on chain

Message envelopes, key packages, social events, blobs, announcements and
attestations never touch the chain. The chain carries balances, names,
identities and devices, provider registrations, storage assignments,
challenges and receipts — what must be globally agreed and settled. See
`docs/SERVICE_REWARDS.md` for the boundary.
