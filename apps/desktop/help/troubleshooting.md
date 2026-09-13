# Troubleshooting

## "Offline — showing what's on this device"

No node has completed the handshake. Check the network, then allow
outbound UDP and TCP to port 26670 in Windows Firewall. Network →
**Reconnect** retries; **Forget peers** starts again from the built-in
list. Everything you did offline is sent at the next successful round.

## "The recipient has no device online yet"

Their identity has no device key on chain, or their device has not
published an encryption key package to a store node. They must open
Hashgram once while online (and, for a brand-new identity, register it
with a small fee). Try again after they did.

## Mail I sent shows "partial delivery"

One of several copies (for example a BCC copy) did not reach a store
node. The label stays on your Sent copy; resend to the people who did not
get it.

## "Wrong passphrase"

The vault is Argon2id-protected; there is no reset. If you truly lost the
passphrase, wipe local data from Settings → Advanced and restore from the
24 words or a backup file.

## Windows Hello stopped working

Hello wraps the passphrase; after a passphrase change, a Windows
reinstall or a hardware change it must be enrolled again from Settings →
Security.

## A node on this PC does not start

Earn → Run a node → **Log** shows the node's own output. The node uses
the app's loopback chain gateway on `127.0.0.1:26680`; if another program
holds that port the app says so at start.

## Export diagnostics

Network → **Export diagnostics** writes a report without IP addresses or
contact addresses. Attach it to a bug report.
