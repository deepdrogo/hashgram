# Earn by running a node

**There is no mining.** Nodes earn for storing and serving real bytes; the
budget is a ceiling, not a guarantee.

## What earns credit

| Role | Earns for |
| --- | --- |
| `store` | keeping mailboxes and blob replicas, answering fetches |
| `media` | serving blob chunks |
| `relay` | relaying traffic between peers |
| `bootstrap` | being a first-contact node |
| `call` | TURN relaying for calls |

Chain-query relaying (what lets this wallet read balances without a
server) earns **nothing**; it is a public good like peer exchange.

## The numbers

- Reserve: 500,000,000 HASH.
- Epoch: 21,600 blocks (about a day). Budget per epoch:
  `min(remaining × 5 / 10,000, 250,000 HASH)`.
- At most 5 % of an epoch's budget to any one provider.
- Bond: 1,000 HASH, slashed 5 % on proven fraud. Unbonding takes 21 days.

## Running a node from this PC

**Earn → Run a node** installs `hashgram-node` as a Windows service managed
by the app: choose roles, disk quota, bandwidth cap and a **reward
address**. The default reward address is a separate cold address you write
down, not your hot wallet. The node's operator key lives in the same vault.

Then the screen shows: reachability (autonat), storage assignments,
challenges passed, fraud score, credit this epoch, lifetime paid and the
payout history to the reward address.

## Honest expectations

A home PC behind a NAT that hole punching cannot cross will receive fewer
assignments and earn less; the app says so when it detects it. Earnings
depend on real demand for storage and bandwidth on the network. Nothing
here promises a return.
