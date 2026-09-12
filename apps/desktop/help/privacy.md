# Privacy

## What leaves this PC

- Transactions you sign, to the network.
- Encrypted message envelopes, to store nodes (they cannot read them).
- Social events you publish, signed by your device key. These are public
  by design.
- Media you upload, to store/media nodes (private attachments are
  encrypted before upload).
- A signed update check to `hashgram.io` if enabled in Settings → Updates.
- Requests to any REST endpoint you pasted yourself.

## What never leaves

- The 24 words, the passphrase, the vault, the database key.
- Decrypted messages.
- Your local mute and block lists.
- Analytics, crash dumps or telemetry of any kind: there are none.

## What nodes can see

A node you connect to sees your IP address and an ephemeral peer id that
changes every session (a client keeps no stable network identity). It
learns which mailboxes you fetch and which blobs you request, not their
contents. Reads of the chain are visible to the relaying nodes as queries.

## What is public on chain

Addresses, balances, transactions, `@username` registrations, device
public keys, provider registrations. This is a public ledger.

## Diagnostics export

Settings → Network → Export diagnostics writes a report with your own
addresses and peer list *without IP addresses of other peers*, so it can be
shared with a node operator without exposing them.
