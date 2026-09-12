# Sending and fees

## Amounts

HASH has six decimals: 1 HASH = 1,000,000 uhash. Hover any balance to see
uhash. The total supply is fixed at 1,000,000,000 HASH; there is no mint
module and no way to create more.

## Transfers are untaxed

Sending 100 HASH delivers exactly 100 HASH. The only cost is the **network
fee** on the transaction (gas × gas price, shown before you confirm).

## The 1 %

1 % of **protocol fee revenue** — never of transferred principal — goes to
the Founder. This share is a hard-coded ceiling of 100 basis points and is
visible live under **Founder**. If you send 100 HASH with a 0.001 HASH fee,
the Founder's share of that transaction is 0.00001 HASH.

## Recipients

- A `hash1…` address. The app validates it before **Send** enables; a
  `cosmos1…` address is not a Hashgram address.
- An `@username`, resolved on chain (`x/username`). The app shows the
  address it resolved to and warns about confusable names.

## Tracking

After broadcast the transaction shows as *pending* until the chain includes
it. The app polls the transaction hash; **History** lists everything sent
and received, with CSV export.

## When the network cannot simulate

Reading the chain through the peer-to-peer relay is safe by design: nodes
forward only read queries and the final signed transaction, never a
simulation. In that mode the app estimates gas generously instead of asking
a node; unused gas is not charged beyond the fee shown.
