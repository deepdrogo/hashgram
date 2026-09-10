# Decentralisation

An honest account of where control sits.

Most projects publish a document like this to argue they are decentralised.
The useful version distinguishes three separate things: what the code makes
impossible, what the code merely makes difficult, and what is currently
centralised in practice. The third list is the one that matters, and it is
long.

## 1. Made impossible by the code

These are not policies or promises. There is no transaction that does them and
no code path that reaches them.

| | Why |
| --- | --- |
| Create new HASH | `x/mint` is not wired into the application; no module account holds the `Minter` permission |
| Raise the Founder revenue share above 1% | A compile-time ceiling equal to the current value; `Validate` rejects a higher one, so the proposal fails |
| Freeze or seize an account | No admin module, no blacklist, no such message type |
| Reverse a confirmed transfer | No such message type |
| Disable a message type | `x/circuit`, which exists in the SDK for exactly this, is deliberately not wired in |
| Tax a transfer | The fee router operates on the fee collector and its own pool; it has no access to a transfer's principal |
| Change the network's identity after genesis | `x/network` writes it once and refuses thereafter |
| Recover a user's account administratively | No component holds a user's private key |
| Read a private message from a server | Messages are end-to-end encrypted with MLS; store nodes hold ciphertext and the acceptance test greps their databases for plaintext |

Changing anything on this list requires a new binary that the validator set
consciously adopts. That is a real bar, and it is deliberately the only bar
available for these properties.

## 2. Made difficult but not impossible

| | Requires |
| --- | --- |
| Spend from the treasury, growth, dev-grant or liquidity pools | A passed governance proposal, leaving an on-chain disbursement record |
| Change the Founder beneficiary address | A passed governance proposal, leaving an audit trail |
| Lower the Founder revenue share | A passed governance proposal |
| Change service-reward parameters | A passed governance proposal, within bounds `Validate` enforces |
| Add or remove a welcome attestor | A passed governance proposal |
| Upgrade the chain | A passed governance proposal plus operators actually installing the binary |
| Halt the chain | More than a third of voting power going offline or colluding |

Governance is stake-weighted, so "requires a governance proposal" means "requires
whoever holds the stake to agree". That is not the same as "requires broad
consent" — see the next section.

## 3. Centralised today

The section that matters.

### Everything runs on one server

Phase 1 was built and validated on a single VPS. The four-validator test runs
four validators on one host as one operating-system user. That tests consensus
and block gossip. It tests **nothing** about network partitions, independent
operators, or geographic distribution, which are what actually makes a network
resilient.

Four validators on one VPS is still one VPS. The test script says so in its
own output rather than letting the result read as more than it is.

Until mainnet has independent operators in different jurisdictions,
"decentralised" describes the design and not the deployment.

### The Founder holds 20% of the supply and wrote the code

20,000,000 HASH liquid at genesis, 180,000,000 vesting over eight years.
Governance is stake-weighted, so a Founder who stakes the full allocation has
substantial influence over every proposal.

The code limits what that influence can reach: no minting, no fee increase, no
freezing, no bypassing the vesting schedule. It does not limit voting weight,
and no code can.

### The genesis validator set is chosen by whoever runs genesis

At launch, the validator set is exactly the set of gentxs collected into the
genesis file. That is a decision made by one person before the network exists.
It becomes permissionless immediately afterwards — anyone can bond and join —
but the starting set is not.

### The welcome attestor is trusted

`x/welcome` verifies that an attestation is correctly signed by a registered
attestor. It cannot verify that the attestor is honest. A malicious attestor
can issue attestations for addresses it controls, up to its per-epoch cap.

The per-epoch cap bounds the damage, and every claim records which attestor
signed it, so abuse is visible after the fact rather than prevented.

### There are no independent implementations

One implementation of the protocol exists. A bug in it is a bug in the whole
network, and there is no second implementation to disagree and expose it.
Multiple implementations are the strongest available defence against
consensus bugs, and Hashgram does not have one.

### The safety engine has an operator

The safety engine scans public content only and never private
messages. But somebody configures it, and that somebody decides what it
flags. Its attestations are signed and published on chain so the decisions are
auditable, which is a meaningful constraint and is not the same as nobody
deciding.

### Storage assigners are a named set

Storage rewards flow only for bytes recorded on chain by a registered
assigner (`x/serviceproof` `assigners`). At genesis that set is whoever the
Founder names — on a one-node launch, the Founder's own node operator — and
adding independent assigners is a governance action. Until several
operators are assigners, one party decides which stored bytes earn. What
that party cannot do: pay for bytes that are not there (challenges require
the bytes), take more than 5% of an epoch (the provider cap), or read any
content. See `docs/SERVICE_REWARDS.md`.

### Bootstrap peers ship with the binary

New nodes need somewhere to start. The bundled bootstrap list is chosen by
whoever builds the release. The node adds a persistent peerstore, DHT and peer-exchange discovery, and signed bootstrap records, but
the first connection has to come from somewhere, and that somewhere is a
decision made at build time.

As of the 2026-09-10 launch the lists (`app/params/mainnet/seeds.txt`,
`bootstrap_peers.txt`, `dns_seeds.txt`, shared by the Go and Rust builds)
contain exactly one operator: the genesis validator. That is the same shape
Bitcoin had on its first day and it is the weakest point in this table. The
pinned genesis hash means a seed can at most withhold peers from a new node,
never route it onto a different chain; but one host, one jurisdiction, one
person is still one host. Entries from independent operators are accepted
by pull request, and the DNS layer exists so that a name published by
someone else can add peers without a release.

## 4. What would make it genuinely decentralised

Concrete, not aspirational. In rough order of how much each would change the
picture:

1. **Twenty or more validators, independently operated, in five or more
   jurisdictions, with no single operator above 10% of voting power.** This is
   the one that matters most and the one that cannot be achieved by writing
   code.
2. **A second implementation of the protocol**, so a consensus bug in one is
   caught by the other rather than becoming network-wide.
3. **A Founder stake that is not decisive in governance**, whether by
   distribution, by delegation to others, or by abstention.
4. **Multiple independent welcome attestors**, so no single one can mint
   eligibility.
5. **A bootstrap list contributed to by several operators**, so discovery does
   not begin at a single point of trust.
6. **Storage and relay providers numbering in the hundreds**, so the
   per-provider reward cap stops binding and no operator is load-bearing.
7. **An independent security audit**, published, with findings addressed.

None of this is done. Some of it is not achievable by the person writing the
code, which is the honest reason to say so plainly rather than describe the
design and let the reader infer the deployment.

## 5. Verifying the claims in section 1

Every item in section 1 is checkable without trusting this document.

```bash
# No mint module, no minter permission, and a test that fails if either changes
go test ./app/ -run TestNoModuleCanMint -v
go test ./app/ -run TestMintModuleIsNotWired -v

# Supply is exactly 1e15 uhash and does not move
scripts/testnet/devnet.sh

# A transfer is untaxed; fee revenue yields exactly 1%
build/hashgram-test-client wallet send --from alice --to bob --amount 100
build/hashgram-test-client founder verify

# The Founder fee ceiling cannot be exceeded
go test ./x/founder/types/ -run TestFeeCeiling -v

# A fork cannot join, and the layers that stop it
scripts/testnet/four-validator.sh
```

The devnet script is the most useful of these: it builds a real genesis with
the real tooling, starts a real chain, and asserts the economic claims against
it rather than against a mock.

## 6. On reading this document

If a project's decentralisation page has no section 3, that is the finding.
Every real network has one; the question is whether it is written down.

Hashgram's is long because the network has not launched. It should get shorter
over time, and if it does not, that is worth noticing.
