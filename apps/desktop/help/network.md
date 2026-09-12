# Network and nodes

## Two layers

1. **Peer-to-peer** (libp2p, QUIC or TCP on port 26670): messaging, social
   events, media, calls, discovery. The app dials the bootstrap peers
   compiled into it, completes the Hashgram handshake — network id, chain id,
   magic, protocol version and **genesis hash** — and from there finds more
   nodes through the DHT and announcements. After the first run the peers
   it remembers come first; the built-in list is only a fallback.
2. **Chain reads and broadcast**: balances, staking, usernames, founder
   data. Read in this order, each with a live health dot:
   - a chain node on this PC (`127.0.0.1:1317`);
   - the peer-to-peer chain relay, cross-checked across two nodes run by
     different operators;
   - HTTPS endpoints you pasted in Settings → Network.

## "Verified by 2 nodes"

Every read over the relay goes to two nodes with different operator
addresses and is compared byte for byte. If they disagree, both are marked
disputed and a third decides. When only one operator is reachable the app
says "single operator" instead — that is a warning, not a badge. This is
not a light client: no Merkle proof is checked yet.

## Wrong network

A peer whose handshake reports a different genesis hash is listed greyed
out as **wrong network** with the reason, and is never retried silently.
The genesis hash is compiled into the app.

## Connected nodes panel

Click the status bar: peer id, operator, roles, latency, transport
(QUIC/TCP/relayed), which discovery layer found each peer, which served
the last chain reads and whether they agreed, and your own NAT status. No
geolocation is shown or computed.

## Firewall

Outbound UDP and TCP on port 26670 must be allowed. The installer adds a
rule for the app binary only.
