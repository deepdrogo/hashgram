# Hashgram Threat Model

What an attacker can do, what they cannot, and what is still an open weakness.

The value of a threat model is in the second and third lists. A document that
only describes defences is marketing. Section 3 in particular is the part
worth reading before deciding how much to trust this network today.

## Adversaries considered

| Adversary | Capability |
| --- | --- |
| Remote attacker | Can send arbitrary traffic to any exposed port |
| Malicious user | Has valid keys and can submit any transaction |
| Malicious node operator | Runs a relay, store or validator and wants to be paid without working |
| Colluding operators | Several operators cooperating |
| Host attacker | Has code execution on one node as one service user |
| Root on a node | Full control of one machine |
| Founder | Holds the Founder key and wrote the code |
| Nation state | Can compel a hosting provider, seize hardware, block traffic |

## 1. What the design stops

### Supply inflation

**Attack.** Create HASH beyond the 1,000,000,000 ceiling.

**Why it fails.** There is no code path that creates coins after genesis.
`x/mint` is not wired into the application, and no module account holds the
`Minter` permission. This is not a parameter set to zero, which a proposal
could change; it is an absent module, which requires a new binary the
validator set adopts.

Enforced at four levels: absent module, no minter permission, a unit test that
fails if either changes, and a runtime metric with the highest-severity alert
in the set.

### A hidden Founder tax on transfers

**Attack.** The Founder takes a cut of every transfer.

**Why it fails.** `x/feerouter` operates on the fee collector and its own
revenue pool. It has no access to a transfer's principal and no code path that
touches one. The devnet acceptance suite sends 100 HASH and asserts that
exactly 100 HASH arrives.

### The Founder raising their own cut

**Attack.** A governance proposal, or a quiet parameter change, raises the
Founder share above 1%.

**Why it fails.** `MaxFounderFeeBasisPoints` is a compile-time constant equal
to the current value. `Params.Validate` rejects anything above it, so the
proposal fails rather than passing and taking effect. Raising the ceiling
requires a binary upgrade adopted by the validator set.

`hashgram_founder_fee_basis_points` is exported with an alert on any change, so
even a *lowering* is visible.

### Freezing an account or reversing a transfer

**Attack.** An authority freezes a balance or claws back a transfer.

**Why it fails.** No such transaction exists. There is no admin module, no
blacklist, and `x/circuit` — which would allow an authority to disable message
types — is deliberately not wired in.

### Fake service work

**Attack.** A provider reports traffic it did not carry, or storage it does
not hold.

**Why it fails.**

- Relay, retrieval and call rewards require a receipt signed by the **client
  that was served**, and the client's public key is inside the signed bytes.
  Without it, an observed receipt could be re-presented alongside a different
  declared key.
- A provider cannot be its own client: self-traffic is detected and rejected,
  and rejected evidence adds 50 to the fraud score.
- Storage rewards require passing randomised challenges. The challenged chunk
  is derived from the previous block's app hash, which no single party can
  predict or steer, so a provider cannot keep only the chunk that will be
  asked for.
- Declared capacity is advertising. Payment follows assignment and challenge
  success.

### Two nodes trading fake receipts

**Attack.** Two colluding providers sign each other's receipts, so both have
"client-signed" evidence.

**Why it fails.** The concentration cap allows at most 20% of a provider's
credit to come from a single counterparty. A ring of two collects a fifth of
what its raw receipts claim, while an honest relay serving many clients is
unaffected. This is the single most load-bearing anti-abuse parameter in
`x/serviceproof`, and `Validate` refuses to let governance set it to zero.

There is a test that builds the two-node ring and asserts the discount.

### Receipt replay

**Attack.** Submit the same receipt repeatedly.

**Why it fails.** Per-provider nonce tracking, plus an expiry height that
bounds how long a receipt is valid at all.

### Username homograph attacks

**Attack.** Register `@а1ice` (Cyrillic а, digit one) to impersonate
`@alice`.

**Why it fails.** Four layers. NFKC normalisation, Unicode-aware lowercasing,
mixed-script rejection, and confusable folding to a skeleton form. A
registration whose skeleton collides with an existing one is refused. The
devnet suite registers `@alice` and then asserts `@a1ice` is rejected.

### Namespace exhaustion

**Attack.** Script the registration of every short name.

**Why it fails.** A registration fee routed through `x/feerouter`, and an
expiry with a grace period so abandoned names return to circulation. A
million names costs a million HASH.

### A fork impersonating Hashgram

**Attack.** Run modified software claiming to be Hashgram, and get operators
or clients to join it.

**Why it partly fails, and where the gap is.** This one deserves precision,
because the four-validator test found that the layers are not
interchangeable:

1. A fork under **its own chain id** is refused at the CometBFT handshake and
   never opens a connection. Verified.
2. A fork that keeps `hashgram-1` and changes only the genesis **does** open a
   transport connection, because CometBFT's handshake compares chain ids and
   does not hash the genesis file. Verified, and it is a real gap at that
   layer.
3. It still cannot join consensus. Its validators are not in the real
   validator set and its app hash diverges at height one. The test confirms
   the real chain kept advancing, all four validators kept one app hash, and
   the validator set stayed at exactly four while the fork was connected.
4. The layer that actually protects a **person** is the operator tooling.
   `hashgramctl join-mainnet` requires `--genesis-hash` and refuses any
   genesis that does not match.

The Hashgram P2P handshake verifies all five identity parts before any
application protocol is offered, so the gap does not exist on the layer
clients use; `scripts/testnet/phase2.sh` shows a same-chain-id fork refused
and banned.

### Signature confusion across purposes or networks

**Attack.** Take a signature over one kind of object and present it as
another, or replay a devnet signature on mainnet.

**Why it fails.** Every signature is over a domain-separated digest that
commits to the network magic and the purpose. Nine purposes are defined and
each produces a distinct domain. Signing preimages are hand-built with
explicit length prefixes in one shared implementation, so `("ab","c")` and
`("a","bc")` cannot collide.

### Message content leaking through logs

**Attack.** Read message plaintext from a node's logs.

**Why it fails.** No log call at any level writes message content. Enforced by
a checker that runs in CI and self-tests its own patterns against deliberate
violations, so a broken check cannot pass silently. Metric labels are also
forbidden from carrying user identifiers, which would turn the metrics
endpoint into an enumerable list of who uses the node.

## 2. What an attacker can still do

Being honest about this list is the point of the document.

### Compromise one node

Root on a node gets: that node's data, its P2P key, its consensus key if one
is present and not behind a remote signer, and the ability to serve wrong
answers to anyone querying that node's RPC.

It does **not** get: the ability to forge blocks the rest of the network
accepts, other nodes' keys, the Founder key, or any user's private key.

Mitigations that limit the blast radius: per-role service users with separate
data directories, systemd confinement, and a remote signer so the consensus
key is not on the internet-facing host at all.

### Get you slashed

An attacker who takes a consensus key can double-sign deliberately. The stake
is slashed and the validator is jailed. This is unrecoverable — the slash is
consensus, not a mistake to be corrected.

The defence is prevention: a remote signer or HSM, and never running two
processes with the same key. Note that `priv_validator_state.json` prevents a
*single* node from double signing and cannot protect against two nodes with
the same key, because neither can see the other's state file.

### Censor transactions from one node

A malicious node can refuse to relay a transaction it receives. It cannot stop
the transaction from reaching the network by another route, and it cannot
prevent a block containing it from being committed.

A validator with more than a third of voting power can halt the chain. That is
a property of BFT consensus, not a Hashgram defect, and the defence is validator
set decentralisation rather than code.

### Correlate metadata

Even with content encrypted, a relay sees who connected to it, when, and how
much data moved. That is traffic analysis material, and Hashgram does not
currently defend against it.

The store-and-forward envelope design reduces the direct-connection signal:
a store node sees a sender's device deliver to a mailbox and a recipient's
device fetch from it, not a direct connection between the two. A global
passive adversary observing many stores could still infer communication
patterns from timing and sizes. `docs/MESSAGING.md` lists exactly what a
store learns. Mixnet-grade protection is not in scope and should not be
assumed.

### Compel a hosting provider

A nation state can seize a server, compel its operator, or block traffic to
it. Against a single VPS this is decisive.

The mitigation is not cryptographic. It is that no single node is necessary:
the chain continues with two thirds of voting power, users hold their own
keys, and a seized node yields no user private keys and no message plaintext.
That mitigation is only real if the validator set is actually distributed
across operators and jurisdictions, which today it is not — see below.

### Spam the network

Transaction fees and mempool limits bound this, but a well-funded attacker can
raise costs for everyone for as long as they are willing to pay. There is no
complete defence, and claiming one would be dishonest.

## 3. Known weaknesses

The section to read before deciding how much to trust this.

### Everything runs on one server today

Phase 1 has been validated on a single VPS. The four-validator test runs four
validators on one host as one operating-system user, which tests consensus and
gossip and tests nothing about network partitions, independent operators or
geographic distribution.

**Four validators on one VPS is still one VPS.** The test says so in its own
output. Until mainnet has independent operators in different jurisdictions,
"decentralised" describes the design and not the deployment. See
[DECENTRALIZATION.md](DECENTRALIZATION.md), which is blunter still.

### The Founder holds 20% of the supply

20,000,000 HASH is liquid at genesis and 180,000,000 vests over eight years.
Governance is stake-weighted, so a Founder who stakes their whole allocation
has substantial influence over proposals.

This is a structural fact of the distribution, not something the code fixes.
What the code does limit: the Founder cannot mint, cannot raise their fee
share, cannot freeze an account, and cannot bypass the vesting schedule. What
it does not limit is voting weight.

### The welcome attestor is trusted

`x/welcome` requires a signed eligibility attestation, and it does not — and
cannot — verify that the attestor is honest. A compromised or malicious
attestor can issue attestations for addresses it controls, up to its
per-epoch cap.

The mitigations are the cap, the on-chain visibility of which attestor signed
each claim, and governance's ability to remove an attestor. The mechanism is
deliberately pluggable because what counts as evidence of a distinct human is
a policy question that will change; that pluggability is also the weakness.

### Storage challenges do not prove unique storage

A challenge proves the provider can produce a chunk. It does not prove the
provider stores it independently rather than fetching it from another replica
on demand.

The response window (about twenty minutes) is deliberately short enough to
make on-demand fetching from a distant peer awkward, and it is not a proof of
replication. Proof-of-replication schemes exist and are considerably more
expensive; the trade was made knowingly.

### No formal verification

The economic invariants are enforced by unit tests, integration tests and a
runtime metric. They are not machine-checked proofs. The tests are good and
they are not the same thing.

### CometBFT's handshake does not verify the genesis hash

Documented in section 1 and worth repeating here, because it is a real gap
rather than a defence: a same-chain-id fork reaches the transport layer.
Consensus and operator tooling turn it away, but the P2P layer today does not.

### The network layer has had no external review

Messaging, social, storage and calls exist and are tested, including a live
check that no store node holds plaintext. The MLS and libp2p libraries carry
their own audits; the code that joins them — envelope handling, mailbox
authentication, blob encryption, the rewards agent — has one implementation
and no independent review. Statements about its security are statements
about tested code, not audited code.

### Store nodes see who talks to whom, roughly

A mailbox is addressed by recipient. A store node therefore knows that a
device it authenticated fetched from mailbox X and that some device
delivered to X at a given time and size. It does not learn the sender's
identity (delivery is unauthenticated by design) or any content.

### A safety operator can suppress public content

A signed `ContentAttestation` with verdict `BLOCK` causes compliant nodes
to stop serving a public CID or event and indexers to hide it. Attestations
are public and signed, so suppression is visible and attributable, and a
client is free to ignore attestors it does not trust. Private content is
never seen by the safety engine and cannot be attested.

## 4. Assumptions

The design is only as good as these:

1. **Two thirds of voting power is honest.** Standard BFT. Below that,
   consensus safety is lost, and no amount of application code helps.
2. **Users protect their own keys.** There is no recovery for a lost key
   beyond social recovery the user configured in advance. This is the cost of
   having no key escrow.
3. **The cryptographic primitives hold.** secp256k1, ed25519, SHA-256,
   BLAKE3. If one breaks, so does everything built on it.
4. **Clocks are roughly synchronised.** CometBFT depends on it for proposal
   timing. There is an alert on drift beyond half a second.
5. **The Founder key was generated off-server.** Nothing in the code can
   verify this. It is a procedural guarantee, which is why
   `hashgram-keygen` exists as a separate offline binary and why the tooling
   has no code path that could generate the key on the server.
