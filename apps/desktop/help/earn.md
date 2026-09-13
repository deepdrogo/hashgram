# Earn by running a node

A Hashgram node stores encrypted mailboxes and Drive objects and relays
traffic for people who cannot connect directly. Operators are paid from
the service reserve for **work that was proven**: storage challenges the
chain issues and receipts that clients sign for bytes served. Nothing is
paid for declared capacity, and there is no mining.

## On this PC

Earn → **Run a node** installs the bundled node as a per-user scheduled
task (or a Windows service when the app runs elevated) and keeps it
running at logon. The node reaches the chain through the app's loopback
gateway, so no chain software is needed on a home PC.

## Registering

Registration is one transaction with a bond. The **reward address must
differ from the operator address**: the operator key is hot (it lives on
the node and signs receipts); rewards should go to a key that only
receives. The app can generate a cold reward address and show its 24 words
once.

## What the lifecycle means

- **Waiting for assignment** — registered as a storage provider; the
  network has not assigned data yet. This needs an assigner to be
  registered by governance.
- **Active** — earning credit in the current epoch.
- **Degraded** — the fraud score is above zero; weight is reduced until it
  decays.
- **Jailed** — no rewards until the height shown.
- **Unbonding** — the bond is released after 21 days.
