# Troubleshooting

## No nodes connected

- Wait ten seconds; the first handshake can take a moment.
- Check Windows Firewall: outbound **UDP and TCP 26670** must be allowed
  for Hashgram. QUIC needs UDP; TCP is the fallback.
- **Network → forget peers** and reconnect if the peerstore holds stale
  addresses.

## "Wrong network"

A node reported a different genesis hash. This app is pinned to the
Hashgram Mainnet genesis and will never talk to a fork. If *every* node is
"wrong network", the app itself is on a DEVNET profile — check the banner
and Settings → Network.

## "Waiting for a node with chain relay"

The nodes you reached do not yet forward chain queries (an older node
version). Balances and history stay unavailable until one does, or until
you add an HTTPS endpoint in Settings → Network. Messaging and social still
work.

## "Single operator"

Only one operator's nodes are reachable, so the app cannot cross-check
chain reads. The numbers shown are from one source. More independent
operators fix this.

## Out of sync / stale balance

The app caches reads for a few seconds and re-reads after every
transaction. Pull the page again or wait for the next block (~4 s).

## NAT and calls

Behind a strict NAT the app uses TURN relays for calls. If the firewall
blocks UDP entirely, calls fail; nothing in the app can work around that.

## Locked out

Forgot the passphrase: **Restore from the 24 words**. There is no other
reset. If Windows Hello stops working (new PC, re-imaged Windows), the
passphrase still unlocks the vault.

## Logs

Settings → Advanced → Export logs. Secrets are redacted before export.
