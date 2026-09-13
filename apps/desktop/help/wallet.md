# Wallet

## Balance

The balance is read from the chain through the peer-to-peer relay and
cross-checked between nodes. The note under the amount says how it was
verified: by several operators, by a single operator (a warning, not a
badge), or not at all. Amounts are exact integers of `uhash`
(1 HASH = 1,000,000 uhash); the app never rounds through floating point.

## Send

Paste an address or type an `@username`, enter the amount, review the
fee, confirm. Transfers are untaxed: 100 HASH sent is 100 HASH received.
1 % of the network fee (not of the amount) goes to the Founder.

## Receive

Your address as text and as a QR code. Sharing it is safe: it is public
by construction.

## Staking

Delegate to a validator to earn staking rewards. Undelegating takes
21 days, during which the tokens earn nothing and cannot move. Validators
are slashed for double-signing and downtime; delegators share the loss.

## Usernames

An `@username` costs 1 HASH, is valid about a year (7,884,000 blocks) and
has a 30-day grace period to renew. The availability check tells you
*why* a name is unavailable: taken, reserved, confusable with an existing
name, too short, mixed scripts.

## Devices

Every PC has its own device key on chain. Add a second device by pasting
its public key from its own Wallet → Devices page (needs the root key,
which the device that created the identity holds). Revoke a lost device;
every encrypted group removes it at the next sync.
