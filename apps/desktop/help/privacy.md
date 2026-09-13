# Privacy

## What a node can see

A store node sees a mailbox id (a hash of a device key), ciphertext sizes
and times. It cannot read a subject, a body, a file name, a folder or who
shared what. A relay sees that a client asked for a public chain fact.

## What is on chain

Only global facts: your address, your device public keys, a username if
you registered one, balances, provider records. No content, no contacts,
no folders, no Spaces.

## What is on this PC

The vault (keys), the local store (mail, Drive tree, contacts, Space
state) sealed per record, and a small UI cache with sealed columns. A test
in the repository writes a mail, a file and a draft and scans every byte
on disk for them; the build fails if any appears in the clear.

## What leaves this PC

Encrypted envelopes to store nodes, signed public posts if you post,
chain transactions you confirm. The app has **no analytics, no
telemetry, no crash upload** and loads no fonts or scripts from the
Internet. Notifications are local toasts and never contain a subject.

## Metadata that remains

Nodes learn *when* a device is online and roughly *how much* it sends.
The app cannot hide that; it can and does avoid revealing who talks to
whom beyond the envelope's mailbox id.

## External mail

Mail bridged from the Internet through a gateway was plaintext on the
Internet and at the gateway. The app labels it so you never mistake it
for an end-to-end encrypted message.
