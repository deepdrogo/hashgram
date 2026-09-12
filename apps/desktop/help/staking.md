# Staking and governance

## Staking

Delegating HASH to a validator secures the chain and earns a share of its
rewards. Before you confirm, the app shows what you are agreeing to:

- **Unbonding takes 21 days.** Undelegated tokens earn nothing and cannot
  be moved during that time.
- **Slashing.** A validator that double-signs loses 5 % of its stake; one
  that is down for too long loses 0.01 %. Delegators share those losses.
- A redelegation cannot be redelegated again for 21 days.

Rewards accrue per validator and are withdrawn with a transaction.

## Vesting accounts

If your account vests, **Wallet → Vesting** shows the schedule and the next
unlock from the chain's account record. Delegating from a vesting account
costs more gas (about 520k); the app allows for it.

## Governance

Proposals are listed from the chain with their tally. Voting options are
yes, no, abstain and no-with-veto. Parameters:

- voting period: 7 days
- quorum: 40 %
- threshold: 50 %
- veto: 33.4 %
