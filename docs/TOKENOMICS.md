# Hashgram Tokenomics

Every number here is a compile-time constant in
[`app/params/params.go`](../app/params/params.go), not a governance parameter.
That is the difference between a published schedule and an announced
intention: changing any of these requires shipping a new binary that the
validator set consciously adopts.

`ValidateSupplyInvariants()` runs in that package's `init()`, so a build whose
constants do not add up cannot start, let alone produce a genesis file.

## The coin

| | |
| --- | --- |
| Display denomination | `HASH` |
| Base denomination | `uhash` |
| Decimals | 6 (1 HASH = 1,000,000 uhash) |
| Maximum supply | 1,000,000,000 HASH = 1,000,000,000,000,000 uhash |
| Address prefix | `hash` |
| BIP-44 coin type | 118 |

Coin type 118 is Cosmos's. Hashgram uses it so that standard hardware wallets
and keyring tooling work unmodified, which matters when the Founder key is
meant to live on a hardware wallet.

## Supply is fixed, and here is why that is more than a claim

There is no inflation, no administrative mint and no bridge that can create
canonical HASH. Every uhash that will ever exist is created in the genesis
block and thereafter only moves between accounts.

What enforces it, in order of strength:

1. **`x/mint` is not wired into the application.** A parameter set to zero can
   be changed by a proposal. A module that is absent cannot.
2. **No module account holds the `Minter` permission.** `maccPerms` in
   [`app/app.go`](../app/app.go) grants `Burner` to the modules that need it
   and `Minter` to none.
3. **A test asserts both.** [`app/app_test.go`](../app/app_test.go) fails if
   any module gains minting authority or if `x/mint` reappears.
4. **A metric exposes it at runtime.** `hashgram_supply_over_ceiling_uhash`
   must be zero forever, and the alert on it is the highest-severity rule in
   [the alert set](../deploy/monitoring/rules/hashgram-alerts.yml).
5. **The devnet acceptance suite checks it against a live chain**, not just in
   unit tests: total supply is exactly 1e15 uhash and does not move across
   blocks.

## Genesis distribution

| Allocation | HASH | Share | Held by |
| --- | --- | --- | --- |
| Founder | 200,000,000 | 20% | Founder account, partly vesting |
| Community / useful-service reserve | 500,000,000 | 50% | `x/serviceproof` module account |
| Treasury | 150,000,000 | 15% | `x/treasury` sub-account, governance only |
| Growth | 50,000,000 | 5% | `x/treasury` sub-account; 1,850,000 carved out for welcome |
| Developer grants | 50,000,000 | 5% | `x/treasury` sub-account, governance only |
| Liquidity | 50,000,000 | 5% | `x/treasury` sub-account, governance only |
| **Total** | **1,000,000,000** | **100%** | |

The four governance-controlled pools are separate module sub-accounts rather
than one lump in the community pool. That costs a little complexity and buys
the property that anyone can query each one individually and see what it holds
and what has been spent from it, with every spend leaving a disbursement
record. A single pool would make "the treasury" a number nobody could
decompose.
[`x/treasury/types/types.go`](../x/treasury/types/types.go)

## Founder allocation

| | HASH | Availability |
| --- | --- | --- |
| Unlocked at genesis | 20,000,000 | Spendable immediately |
| Vesting | 180,000,000 | 96 monthly periods over 8 years |

Implemented as a Cosmos SDK `PeriodicVestingAccount`, so the lock is enforced
by the state machine rather than by a promise. Integer division of 180,000,000
across 96 periods leaves a remainder, which goes into the final period.

Eight years is long. That is the point: it means the Founder's interest is in
the network still being worth something in year eight, and it means a Founder
who leaves early forfeits most of the allocation.

**Verify it on a running chain:**

```bash
hashgramctl wallet-info <founder-address>
```

reports the total balance, the vesting schedule and how much is currently
spendable. The devnet acceptance suite asserts exactly 200,000,000 HASH held
and exactly 20,000,000 HASH spendable at genesis.

## The Founder revenue share

**100 basis points (1%) of qualifying protocol fee revenue. Not a tax on
transfers.**

This is the number most likely to be misread, so it is worth being blunt about
what it is not. If Alice sends Bob 100 HASH, Bob receives exactly 100 HASH.
Nothing is deducted. The Founder share applies only to fees the protocol has
already collected as revenue: transaction gas, username registration and
renewal, and service fees charged by protocol modules.

### The arithmetic

```text
cut = floor(amount × 100 / 10,000)
```

Worked through with the specification's example. 100 HASH of qualifying
revenue is 100,000,000 uhash:

```text
100,000,000 × 100 / 10,000 = 1,000,000 uhash = exactly 1 HASH
```

Truncation is toward zero, which means any remainder stays with validators and
delegators rather than with the Founder. That direction is deliberate:
rounding should not accumulate in favour of the party who wrote the code.

### The ceiling

`FounderFeeBasisPoints` is 100 and `MaxFounderFeeBasisPoints` is also 100.
Governance can lower the share freely. Raising it above 100 bps is not
representable in the state machine: `Params.Validate` rejects it, so the
proposal fails rather than passing and taking effect. Changing the ceiling
requires a binary upgrade the validator set adopts.

"The Founder quietly raised their cut" is therefore not a thing that can
happen, and `hashgram_founder_fee_basis_points` is exported as a metric with an
alert on any change so that even a *lowering* is visible.

### Payout

Revenue accrues to the `x/founder` module account and is paid to the
beneficiary automatically once the accrual clears the minimum, batched every
7,200 blocks (about eight hours at four second blocks). Batching matters
because the beneficiary is expected to be a cold wallet or multisig, and one
dust transfer per block would be both wasteful and hostile to whoever has to
reconcile it.

Anyone can trigger a payout permissionlessly with `MsgClaimFounderRevenue`.
The Founder does not need to be online for the Founder to be paid, and nobody
needs to trust that they will bother.

The beneficiary address is set at genesis and changeable only by governance,
with every change appended to an on-chain audit trail.

**Verify it on a running chain:**

```bash
hashgram-test-client founder verify --node tcp://127.0.0.1:26657
```

reports the configured basis points, the accrued and paid totals, and the
realised share as a fraction of collected revenue. The devnet suite asserts
that a 100 HASH transfer yields the Founder zero and that gas fees yield
exactly 1%.

## Welcome rewards

A tiered joining reward, funded from the growth allocation.

| Claim sequence | Reward |
| --- | --- |
| 1 – 10,000 | 50 HASH |
| 10,001 – 100,000 | 5 HASH |
| 100,001 – 1,000,000 | 1 HASH |
| 1,000,001 and beyond | 0 |

Worst-case total, and therefore the pool size:

```text
 10,000 × 50 =   500,000
 90,000 ×  5 =   450,000
900,000 ×  1 =   900,000
                ─────────
                1,850,000 HASH
```

The pool is funded with exactly that, and `ValidateSupplyInvariants` recomputes
the worst case from the tier schedule and refuses to build if the two disagree.
The tiers cannot drift away from the pool because the pool is derived from
them.

### Creating a key earns nothing

This is the part that makes the reward survive contact with a script. A claim
requires:

- A **signed eligibility attestation** from a registered attestor, over a
  domain-separated digest
- A **sequence number** that has not been used
- A **nonce** that has not been seen from that attestor
- The attestation not to have **expired**
- The attestor to be within its **per-epoch cap**

The attestation mechanism is pluggable on purpose: what counts as evidence of
a distinct human is a policy question that will change, and hard-coding
today's answer would mean a binary upgrade every time it does. What is not
pluggable is the requirement that *something* attests. A fresh keypair with no
attestation claims nothing.

The tier schedule itself is fixed in the binary rather than in governance. A
chain that could quietly re-tier its own published schedule would not have
published anything meaningful.

## Useful-service rewards

500,000,000 HASH at genesis. Finite. Never topped up.

### The emission schedule

Each epoch, the budget is a fixed fraction of what **remains**:

```text
budget = min( floor(remaining × 5 / 10,000), 250,000 HASH )
```

Epochs are 21,600 blocks, roughly a day at the four second target.

Taking a fraction of the remainder rather than a fixed amount is what makes
the schedule both declining and provably bounded:

- **Declining.** Each epoch's budget is a fixed proportion of a shrinking
  balance, so budgets fall geometrically. Early epochs are subsidised more
  heavily, which is the intended shape; a mature network is meant to be funded
  increasingly by real service revenue.
- **Bounded.** The sum of all budgets is a geometric series over the initial
  reserve. It converges to the reserve without reaching it. There is no epoch
  at which the schedule asks for coins the reserve does not hold, and no cliff
  at which subsidy stops abruptly.

At 5 bps per day the reserve falls to roughly 16% of genesis after ten years
and 2.6% after twenty, never reaching zero.

### The absolute cap

250,000 HASH is exactly the geometric schedule's own first-epoch budget
(500,000,000 × 5 / 10,000). Since the reserve only shrinks, the cap binds at
genesis and never afterwards: it is a backstop against a future parameter
mistake rather than a shape that overrides the schedule.

An earlier draft used 50,000, and a test caught it: a flat 50,000 per epoch
spends only 40% of the reserve in eleven years and flattens the declining
curve into a straight line, defeating the design.

### The budget is a ceiling, not a payout

A single provider may receive at most 5% of an epoch's budget. A network needs
at least twenty providers before the full budget can be drawn, and the undrawn
remainder stays in the reserve to fund later epochs. Nothing is lost.

That cap is a deliberate pressure toward many operators, and it has a larger
effect on the reserve timeline than user growth does. It is also what protects
an early network from paying its whole subsidy to whichever handful of nodes
happened to be online first.

### What earns credit

| Role | Paid on | Rate |
| --- | --- | --- |
| Storage | Assigned bytes held for a full epoch, scaled by challenge success | 100 credit per GiB-epoch |
| Relay | Client-signed receipts for bytes forwarded | 200 credit per GiB |
| Retrieval | Client-signed receipts for bytes served | 150 credit per GiB |
| Call | Client-signed session receipts | 300 credit per hour |

Registration requires a bond of 1,000 HASH. The bond is what makes fraud
expensive: without it, the cost of being caught cheating would be the cost of
generating a new keypair. A failed challenge adds 25 to a provider's fraud
score, forged or replayed evidence adds 50, 100 jails and slashes 5% of bond,
and the score decays by 10 per epoch so one bad day does not permanently
condemn an honest operator.

### After the reserve

The reserve declines asymptotically and never empties. As it thins, the
subsidy per unit of work falls and operators are funded increasingly by real
protocol service-fee revenue. There is no mechanism to refill it, and adding
one would require creating coins, which the supply invariant forbids.

## Simulating it

[`tools/tokenomics-simulator`](../tools/tokenomics-simulator) projects all of
the above over decades. It **imports the chain's own functions** —
`EmissionForEpoch`, `ProviderRewardCap`, `TierAmountHash`, `FounderCut` —
rather than reimplementing them, because a simulator that models a different
schedule from the chain is worse than no simulator: it produces numbers that
look authoritative and are not.

```bash
make tools
build/tokenomics-simulator -all          # four preset scenarios, compared
build/tokenomics-simulator -list         # what the presets assume
build/tokenomics-simulator -format csv   # for a spreadsheet
```

It asserts the supply invariant after every simulated epoch, so a modelling
mistake surfaces immediately rather than as a plausible-looking wrong total.

Read the reserve curves as informative and the absolute revenue figures as
illustrative. The protocol constants are the chain's; the adoption
assumptions are guesses, and the tool prints them next to every result so the
two are never confused.

## Verifying all of this yourself

None of the above needs to be taken on trust.

```bash
scripts/testnet/devnet.sh
```

builds a genesis with the real tooling, starts a chain, and asserts against
it: total supply exactly 1e15 uhash and unchanged across blocks; the Founder
holding exactly 200,000,000 HASH with exactly 20,000,000 spendable; a 100 HASH
transfer delivering 100 HASH; the Founder accruing exactly 1% of collected gas
fees; the service reserve holding exactly 500,000,000 HASH; the welcome pool
holding exactly 1,850,000 HASH with no attestors registered; and no mint
module present.
