# node — the Rust off-chain layer

Everything Hashgram does that is not consensus lives here: network identity
and handshake, the libp2p swarm, store-and-forward mailboxes, content-
addressed blobs, signed social events, the useful-service rewards agent, the
Hashgram One application protocol, and the client SDK that ties them
together. The chain itself (`hashgramd`, Go) is in the repository root.

This is a Cargo workspace (`node/Cargo.toml`). Members outside this directory
(`../sdk/rust/hashgram-sdk`, `../apps/desktop/*`) build against the same
lock file.

## Crates

| Crate | What it is |
| --- | --- |
| `hashgram-net` | Network identity (magic, network id, chain id, genesis hash), the 13 domain-separated signing purposes, canonical preimages, and the peer handshake. Byte-identical to the Go implementation, checked against generated vectors. |
| `hashgram-proto` | Off-chain wire types generated from `proto/hashgram/{p2p,chat,app}/v1`, plus frame bounds (`limits`), structural validation (`validate`), signing helpers, CID and Merkle computation. |
| `hashgram-chain` | REST/relay chain client, secp256k1 wallet, transaction messages, pluggable transport (HTTP or the P2P chain relay). |
| `hashgram-identity` | Encrypted device vault: Argon2id + XChaCha20-Poly1305, atomic writes, zeroisation. |
| `hashgram-mls` | OpenMLS (RFC 9420) wrapper with one ciphersuite; used by messaging, Mail, Circles and Spaces. |
| `hashgram-p2p` | libp2p swarm (QUIC/TCP, Noise, Yamux), gossipsub, Kademlia, identify, autonat; handshake gate, peer scoring, per-peer and per-subnet connection limits, persistent peerstore, Prometheus metrics. |
| `hashgram-node` | The node daemon: role-based services (`store`, `media`, `relay`, `bootstrap`, `call`), maintenance sweeps, rewards agent, local operator API. |
| `hashgram-app` | Pure application protocol for Hashgram One: HashMail, HashDrive, People, Circles, Spaces. Message models and bounds, Drive encryption and manifest merge, capabilities, signed Space event log. No network or storage dependency. |
| `hashgram-client` | Reference CLI over `hashgram-sdk`; the developer harness for identities, wallet, messaging, social, media and the Hashgram One commands. |
| `hashgram-devtools` | Mock chain gateway for devnet only. |
| `fuzz` | cargo-fuzz targets for every attacker-facing decoder (frames, handshake, envelopes, gossip, manifests, events, chat and MLS messages). |

## The node daemon

`hashgram-node run` reads `/etc/hashgram/node.toml` and starts the services
its roles call for:

* **store / media** — mailboxes and one-time key packages (`mailbox.rs`),
  content-addressed blobs with 1 MiB chunks, per-uploader quota and a
  replication target of three (`blob.rs`).
* **relay / bootstrap** — allow-listed chain reads and broadcast for wallets
  without a gateway (`chain_relay.rs`), peer exchange, signed bootstrap
  records.
* **call** — TURN/SFU announcement and time-limited coturn credentials
  (`calls.rs`). Present on the wire; not part of the Hashgram One desktop
  surface.
* Always: signed social events with device authority checked against the
  chain (`social.rs`), signed content attestations from trusted attestors
  (`safety.rs`), node announcements, and the rewards agent when an operator
  key is present (`rewards.rs`: registration, storage challenges, receipt
  batching, assignments).

A 60 s maintenance tick sweeps expired envelopes and replay keys, drops
uploads still incomplete after 24 h and releases their quota, runs a bounded
replication pass, answers challenges and submits receipts.

The operator API (loopback by default) serves `/metrics`, `/v1/health`,
`/v1/status`, `/v1/peers`, `/v1/rewards`, and same-host endpoints for
social events, blobs, mailbox hints, safety attestations, TURN credentials
and a read-only chain pass-through.

## Security properties that shape the code

* **Bounds before allocation.** Every frame and field limit in
  `hashgram-proto::limits` is checked before bytes are decoded or stored.
* **Panicking lints are denied.** `unwrap_used`, `panic`, `todo`,
  `unimplemented` are compile errors outside tests; the daemon parses
  attacker-controlled bytes and an unwrap is a remote denial of service.
* **Fail-closed handshake.** All five identity parts are verified before any
  application data; a same-chain-id fork with a different genesis is
  refused, which CometBFT's own handshake does not do. Handshake claims
  (roles, operator address) are bounded.
* **Signed requests are fresh and served once.** Mailbox fetches and acks
  bind a timestamp (±300 s) and the node refuses an exact request it has
  already served inside that window. TURN credential requests use a reserved
  sentinel limit so they can never be confused with a mailbox fetch.
* **Local, decaying peer scores; per-subnet limits; reserved outbound
  slots.** Reputation is never shared, so it cannot be weaponised; a /24
  does not bypass a per-peer limit; an inbound flood cannot isolate the node.

## Building and testing

```bash
cd node
cargo test -p hashgram-net -p hashgram-proto -p hashgram-p2p -p hashgram-node
cargo clippy -p hashgram-node --all-targets
cargo fmt --all -- --check
```

Cross-language parity vectors for `hashgram-net` are regenerated from the Go
side and must match byte for byte:

```bash
go run ./tools/signing-vectors > node/testdata/signing-vectors.json
cd node && cargo test -p hashgram-net --test vectors
```

## Configuration

`/etc/hashgram/node.toml`. Two fields have no default and the node refuses to
start without them:

```toml
network      = "mainnet"
genesis_hash = "…64 hex characters…"
```

`hashgramctl join-mainnet` writes the verified hash to
`/etc/hashgram/network.json`; copy it from there. Roles, listen address,
storage quota, chain API, TURN and rewards settings are documented in
`hashgram-p2p/src/config.rs` and validated by `hashgram-node check-config`.
