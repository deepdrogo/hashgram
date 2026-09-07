# hashgram-node

The Rust peer-to-peer layer. **Phase 2, partially built.**

Read this before reading the code, because the gap between what is here and
what the design calls for is large and stating it plainly is more useful than
letting the crate names imply completeness.

## What is implemented and tested

| Crate | Module | State |
| --- | --- | --- |
| `hashgram-net` | `identity` | Network identity, signing domains, canonical preimages. **Byte-identical to Go**, verified against generated vectors. |
| `hashgram-net` | `purpose` | The nine signature purposes and their wire strings. |
| `hashgram-net` | `handshake` | The five-part peer handshake, including the genesis hash check CometBFT does not perform. |
| `hashgram-p2p` | `score` | Peer scoring with proportional decay, graylisting and banning. |
| `hashgram-p2p` | `limits` | Per-peer, per-subnet and global connection limits. |
| `hashgram-p2p` | `config` | Configuration loading and validation. |

72 tests, `cargo clippy --all-targets --all-features` clean with the
workspace's deny list.

## What is not implemented

The libp2p swarm itself, and everything that rides on it:

- Transport: QUIC and TCP, Noise, Yamux
- Behaviours: Gossipsub, Kademlia, identify, autonat, circuit relay, ping
- Bootstrap discovery: bundled peers, DHT, peer exchange, signed records
- Persistent peerstore
- OpenMLS end-to-end encrypted messaging
- Store-and-forward envelope store
- Signed social events and their partitioned topics
- Content-addressed blob storage
- Call discovery
- The `hashgram-node` binary, which currently prints what is missing and exits
  non-zero rather than pretending to run a node

## Why the identity crate exists separately

Network identity is a protocol fact used by the P2P handshake, by social event
signing, by device certificates and by every client SDK. Those have very
different dependency needs — a mobile SDK does not want libp2p — so identity
lives in a crate with four small dependencies.

## Cross-language parity

`hashgram-net` and Go's `app/params` must produce **byte-identical** signing
preimages. A signature is only verifiable if the verifier builds the same
preimage the signer built.

The tests check against vectors generated from the Go implementation:

```bash
go run ./tools/signing-vectors > node/testdata/signing-vectors.json
cd node && cargo test -p hashgram-net --test vectors
```

That direction matters. The vectors come from the code the chain actually
runs, so this is a parity test rather than a second reading of the
specification: a specification can be misread twice in the same way, and a
generated vector cannot.

90 preimages and 90 digests are checked across both networks and all nine
purposes. A framing change makes them fail, which is correct — such a change
invalidates every signature ever produced and both implementations must adopt
it deliberately.

The mechanism was verified by breaking it on purpose: changing the outer
length prefix from 64-bit to 32-bit made the parity tests fail immediately.

## The gap this layer closes

CometBFT's peer handshake compares chain ids. It does **not** hash the genesis
file. That was verified rather than assumed: a fork keeping the real chain id
and changing only the genesis was pointed at a four-validator testnet, and the
transport connection opened. Consensus refused its blocks and the real chain
was unaffected, but the connection happened.

Three layers turn a fork away today: the CometBFT handshake for a different
chain id, consensus for a same-chain-id fork, and `hashgramctl join-mainnet`
for an operator handed the wrong file. **None of them stops a same-chain-id
fork from opening a socket.**

`hashgram-net::Handshake` closes that gap by verifying all five identity
parts before any application data is exchanged, and returns a distinct error
per part so an operator sees which one disagreed.

## Design notes worth knowing

**The panicking lints are denied, not warned.** This daemon parses
attacker-controlled bytes, so an `unwrap` on a malformed frame is a remote
denial of service. `unwrap_used`, `panic`, `todo` and `unimplemented` are
compile errors in non-test code. Test crates opt out at their own crate root,
because a test that unwraps a fixture it just built is a different situation.

**Peer scores are local, private and decaying.** A shared reputation system is
one an attacker can use to get honest peers banned, so every node forms its
own view from its own observations. Scores decay proportionally toward zero, so
recent behaviour dominates and a briefly broken node recovers. They are bounded
in both directions, so a long-lived peer cannot bank enough credit to
misbehave freely — there is a test for exactly that.

**Connection limits are per-subnet, not only per-peer.** A per-peer limit is
bypassed by an attacker with a /24: 256 addresses, each within the limit.
Grouping by /24 for IPv4 and /64 for IPv6 raises the cost to addresses in many
networks. The /64 choice matters: a single host is routinely handed a whole
/64, so grouping by full address would be no limit at all.

**Outbound slots are reserved.** Inbound connections are attacker-controlled;
outbound are ours. A flood filling the inbound table must not stop this node
reaching the peers it chose, or isolation becomes cheap.

## Building

```bash
cd node
cargo test --all
cargo clippy --all-targets --all-features
cargo fmt --all -- --check
```

## Configuration

`/etc/hashgram/node.toml`. Only two fields have no sensible default:

```toml
network      = "mainnet"
genesis_hash = "…64 hex characters…"
```

Copy the hash from `/etc/hashgram/network.json`, which `hashgramctl
join-mainnet` writes after verifying it. A node with no pinned genesis is
refused at startup rather than at first handshake, because an unpinned node
would join whichever network reached it first.
