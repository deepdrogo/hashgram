# Hashgram Logging Policy

This document states what a Hashgram node may and may not write to a log, a
metric or a chain event, and how that is enforced.

It exists because logging is the most common way an end-to-end encrypted
system leaks the thing it encrypts. The cryptography is rarely what fails.
What fails is a debug line added during an incident, left in, and shipped.

Enforcement lives in [`scripts/dev/check-logging.sh`](../scripts/dev/check-logging.sh),
which runs in CI. It includes a self-test that asserts each of its patterns
fires on a deliberate violation, because a check that always passes is
indistinguishable from a check that is broken.

## The rule

**Message plaintext is never written to a log, at any level, in any build.**

Not at `debug`. Not behind a build tag. Not in a development profile. There is
no log level at which a Hashgram node prints the contents of a private
message, and no configuration that enables one.

The same applies to:

- Private keys, in any encoding
- Seed phrases and BIP-39 mnemonics
- Validator consensus keys
- Device private keys
- MLS group secrets and ratchet state
- Decrypted media bytes

## What may be logged

Identifiers, sizes, counts, timings and outcomes. Enough to diagnose a
delivery problem without learning what was delivered.

Compliant:

```go
logger.Info("envelope delivered",
    "envelope_id", env.ID,
    "size_bytes", len(env.Ciphertext),
    "recipient_device", deviceID,
    "latency_ms", elapsed.Milliseconds())
```

Not compliant:

```go
logger.Debug("envelope delivered", "plaintext", string(plaintext))
logger.Debug("envelope delivered", "envelope", fmt.Sprintf("%+v", env))
```

The second is the more dangerous of the two, and it is why the checker looks
for whole-struct formatting separately. Logging a struct with `%+v` prints
every field it currently has *and every field it gains later*. A struct that
is safe to log today starts leaking the day someone adds a `Plaintext` field
to it, with no change to the log line and nothing for a reviewer to notice.

## Sender and recipient addresses

Addresses may be logged at `info` and below on a node the operator runs for
themselves. They must not be used as **metric labels**.

A Prometheus label containing a user address creates one time series per user.
That is a cardinality problem and a privacy problem in the same change: the
metrics endpoint becomes an enumerable list of who uses the node, retained for
as long as the time-series database keeps data. Check 4 of the enforcement
script rejects `address`, `user`, `sender`, `recipient` and `account` as label
names.

Label by category instead: status, role, service kind, message type.

## Chain events

Chain events are stricter than logs, because they are worse when wrong.

An event attribute is written into the block. It is replicated to every node,
retained permanently, and readable by anyone. A log line is a mistake on one
machine that rotates away in a week; an event attribute is a publication.

No event attribute may carry message content, key material, or anything
derived from plaintext. Events carry identifiers, amounts, addresses and
outcomes.

## Metrics

Metrics are aggregate by construction and are the preferred way to observe
messaging traffic. `hashgram_*` metric definitions are in
[`app/metrics.go`](../app/metrics.go).

Counting envelopes relayed, bytes moved and delivery latency reveals nothing
about content. Where a metric would need a per-user label to be useful, the
metric is the wrong tool.

## Log levels

| Level   | Use |
| ------- | --- |
| `error` | The node could not do something it was asked to do. |
| `warn`  | Degraded but continuing: a peer dropped, a payout skipped. |
| `info`  | Default. Lifecycle, block progress, connection changes. |
| `debug` | Protocol-level detail. Still no plaintext. |

Shipped configurations default to `info`. No configuration in `deploy/` or
`scripts/` sets `debug`, and check 6 enforces that.

Raising a node to `debug` temporarily is a legitimate operator action. The
policy holds because `debug` contains no plaintext to reveal, not because
operators are asked to avoid it.

## Backups and diagnostics

`hashgramctl backup` excludes private key material. `hashgramctl` diagnostic
output is subject to this policy in full: an operator pasting a status dump
into a support channel must not thereby publish anything sensitive.

## Retention

Logs go to the systemd journal. Set retention in `journald.conf`:

```ini
[Journal]
SystemMaxUse=2G
MaxRetentionSec=2week
```

Two weeks is long enough to investigate an incident and short enough that a
compromised host yields a bounded amount of metadata. Shipping logs to an
aggregator off the host is an operator decision; if you do, note that the
metadata this policy permits — who talked to whom, when, and how much — is
still traffic analysis material, and the aggregator becomes part of the trust
boundary.

## What this policy does not achieve

The enforcement script is a grep. It catches the patterns that have
historically leaked plaintext in messaging systems. It cannot see through a
variable rename, a helper function, or a struct field accessed through an
interface.

The structural fix is stronger than the check, and is the design requirement
for the phase 2 messaging code: **envelope and message types should carry no
exported plaintext field at all.** Plaintext should exist only as a local
value inside the function that decrypts it, so that logging it is not
expressible rather than merely discouraged. A type that cannot be misused does
not need a policy document.

Until that code exists, this document plus the checker plus code review is
what there is, and it is worth being clear that the first two are the weaker
half.
