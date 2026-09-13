# Network and nodes

## How the app connects

At start the app dials the bootstrap nodes compiled into it and completes
the Hashgram handshake with each: same genesis hash, same chain id,
compatible protocol. Nodes that fail are listed greyed with the reason
("wrong network", "incompatible protocol") so a mistake is visible, not
silent. Verified peers advertise roles: **store** (mailboxes, Drive
objects), **relay** (chain reads and broadcast, NAT traversal), **media**.

Nothing is pinned to one machine: the peerstore remembers nodes it met,
and **Forget peers** starts again from the compiled-in list.

## Chain reads

Balances, identities and usernames are read through relay nodes and
compared across operators. The page shows the height and how the last
read was verified.

## Leaderboards and stats

Top holders, validators and providers come from a public **indexer**
when one is configured (Settings → Network) and are labelled as such.
The indexer is a convenience, never an authority: a username is shown
next to an address only when that address registered it on chain.

## Diagnostics

**Export diagnostics** writes a text report with peer ids, roles,
counts and errors — no IP addresses, no contact addresses, no subjects.
