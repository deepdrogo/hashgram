# Hashgram External Mail Gateway

`services/mail-gateway` — crate `hashgram-mail-gateway`, binary
`hashgram-mail-gateway`. The SMTP compatibility bridge between the public
Internet and HashMail described in `HASHGRAM_ONE_ARCHITECTURE.md` §15 and
`PRIVACY_MODEL.md` §3.7 / §6.4.

This document is the spec, the operator manual and the runbook. If the code
and this document disagree, the code is wrong or this document is stale; fix
one of them in the same change.

---

## 1. What it is, in one paragraph

The gateway is a separate daemon that owns an **ordinary Hashgram identity**
(the *bridge identity*) and speaks SMTP outward. Mail from the Internet to
`alice@hashgram.io` is parsed and sent through MLS to the on-chain owner of
username `alice` as a `MailMessage{origin: MAIL_ORIGIN_EXTERNAL_GATEWAY}`.
Mail from a Hashgram user to the Internet is native HashMail sent *to the
bridge identity* with an `ext-to:<address>` label; the gateway renders it as
RFC 5322 MIME, DKIM-signs it and relays it to the recipient's MX. The gateway
holds **no user keys**; it holds the plaintext of exactly the messages that
cross it, which are plaintext on the Internet anyway.

---

## 2. Architecture

```
            Internet                              Hashgram network
 ┌───────────────────────┐                  ┌──────────────────────────┐
 │ remote MTA            │  SMTP :25 (TLS)  │                          │
 │  (gmail, fastmail…)   │─────────────────▶│ fronting MTA / proxy     │
 └───────────────────────┘                  │  (TLS termination,       │
            ▲                               │   SPF/DKIM/DMARC checks, │
            │ SMTP :25 (+STARTTLS)          │   Authentication-Results)│
            │                               └────────────┬─────────────┘
            │                                            │ SMTP 127.0.0.1:2525
            │                               ┌────────────▼─────────────┐
            │                               │ hashgram-mail-gateway    │
            │                               │  smtp::server ─▶ mime::inbound ─▶ bridge ─▶ MLS
            └───────────────────────────────│  MLS ─▶ bridge ─▶ mime::outbound ─▶ dkim ─▶ smtp::client
                                            │  store (SQLite)  metrics (/healthz /metrics)
                                            └────────────┬─────────────┘
                                                         │ hashgram_sdk::HashgramOne (bridge identity)
                                            ┌────────────▼─────────────┐
                                            │ store / relay nodes, chain│
                                            └──────────────────────────┘
```

### 2.1 Crate layout

| Path | Purpose |
| --- | --- |
| `src/main.rs` | CLI: `run`, `init`, `register`, `check-config`, `dns` |
| `src/lib.rs` | Module map, `GatewayError` |
| `src/config.rs` | `gateway.toml` schema (serde, `deny_unknown_fields`), validation, SDK `Config` construction |
| `src/address.rs` | `alice@<domain>` ↔ username (`Mailbox::Local` / `Mailbox::Remote`), plus-addressing, source routes |
| `src/smtp/mod.rs` | `Reply`, line splitting, dot-stuffing, reply-line parsing |
| `src/smtp/server.rs` | RFC 5321 server: pure `Session` state machine + tokio driver + listener with per-IP limits |
| `src/smtp/client.rs` | RFC 5321 client: EHLO, opportunistic STARTTLS (rustls + webpki-roots), MAIL/RCPT/DATA, error classification |
| `src/mime/mod.rs` | RFC 2047 encoded-words, quoted-printable, base64 folding, RFC 5322 dates, RFC 2231 parameters |
| `src/mime/inbound.rs` | `mailparse` → `InboundMail` (bodies, attachments, threading headers, `Authentication-Results`, spam score) |
| `src/mime/outbound.rs` | `OutboundMail` → deterministic RFC 5322/MIME bytes |
| `src/mime/html.rs` | HTML → text for HTML-only mail |
| `src/thread.rs` | `Message-ID` ↔ 16-byte id: `blake3(normalised)[..16]`, `<hex@domain>` form |
| `src/dkim.rs` | RFC 6376 relaxed/relaxed canonicalisation and signing (rsa-sha256, ed25519-sha256) |
| `src/dns.rs` | MX lookup (`hickory-resolver`), null MX, implicit MX, `StaticResolver` for tests |
| `src/policy.rs` | `[policy]` decisions, `AuthResults` |
| `src/ratelimit.rs` | Per-IP sliding windows |
| `src/store.rs` | SQLite with embedded migrations: `inbound`, `outbound`, `queue`, `thread_map`, `cursor`, `rate_events`, `schema_version` |
| `src/metrics.rs` | prometheus-client registry, axum `/healthz` + `/metrics` |
| `src/bridge.rs` | Pure mapping (thread ids, `MailMessage` build, `ext-to:` extraction, outbound build) + runtime (`GatewayHandler`, `Bridge` worker, queues, bounces) |
| `gateway.example.toml` | Annotated configuration |
| `deploy/hashgram-mail-gateway.service` | Hardened systemd unit (example; not installed by anything) |

### 2.2 SDK surface used

The gateway is a normal SDK client. The one addition made for it is
`hashgram_sdk::mail::Mail::send_built(main, bcc_copies)`: the delivery half
of `Mail::send`, for callers that must adjust the built `MailMessage`
(origin, external metadata, derived ids) before it leaves. `Draft::build_all`
still always produces native messages; `send_built` re-validates, so bounds
cannot be bypassed. Everything else — `people().resolve`, `mail().list/get/
archive/send/make_attachment/attachment_bytes`, `sync().round`, `save` — is
the public facade.

---

## 3. Inbound: Internet → HashMail

1. **SMTP.** The listener (default `127.0.0.1:2525`) implements `EHLO/HELO`,
   `MAIL FROM` (with `SIZE`, `BODY`), `RCPT TO`, `DATA` (dot-unstuffing,
   `CRLF.CRLF`, size limit with a clean `552`), `RSET`, `NOOP`, `QUIT`,
   `VRFY` (`252`), `HELP`. `STARTTLS` and `AUTH` answer `502` (§9). Ten
   consecutive protocol errors close the connection with `421`. Pipelining
   works because the driver drains every buffered line before reading.
2. **Recipient check at `RCPT TO`.** `alice@<domain>` → username `alice` →
   `people().resolve("alice")` (cached 10 min for hits, 60 s for misses).
   Unknown username or username without a registered identity → `550 5.1.1`.
   Another domain → `550 5.7.1 relaying denied`. Chain unreachable →
   `451 4.4.3` so the sender retries. Plus-addressing (`alice+tag@`) maps to
   `alice`.
3. **Intake at end of `DATA`.** Per-IP message rate (`451 4.7.1`), MIME parse
   (`550 5.6.0` if unparseable), `[policy]` (`550 5.7.1`), thread-id
   resolution, dedup by derived message id (a re-delivery gets `250` and
   nothing new is queued), then the raw message is written to the SQLite
   `queue` and every recipient logged in `inbound` with status `queued`.
   Only then `250 2.0.0 OK: queued`. Accepting means we own it.
4. **Delivery (worker).** Parse again, resolve recipients again (the identity
   may have gained devices), place each on To or Cc according to where the
   sender listed it (RCPT recipients absent from both headers were BCC'd and
   go on To), upload attachments (`make_attachment`: ≤ 64 KiB inline, else an
   encrypted blob), build the `MailMessage`, `send_built`. `NoPeer`/delivery
   errors reschedule with exponential backoff (30 s · 2ⁱ, capped at 6 h,
   `outbound.max_attempts` total); `Invalid` errors drop the message with
   status `failed`.

### 3.1 What the recipient sees

| `MailMessage` field | Value |
| --- | --- |
| `origin` | `MAIL_ORIGIN_EXTERNAL_GATEWAY` — clients **must** label it |
| `from.address` | the bridge identity (what MLS authenticates) |
| `from.display_name` | `Bob Example <bob@example.com>` (bounded to 128) |
| `external.gateway` | bridge address |
| `external.from_header` | the raw `From:` line |
| `external.message_id_header` | normalised `Message-ID` (minted `<hex@domain>` when absent) |
| `external.auth_results` | `spf=…`, `dkim=…`, `dmarc=…` from the fronting MTA's `Authentication-Results`; `none` each when absent — recorded honestly, never guessed |
| `external.spam_score` | `X-Spam-Score` / `X-Spam-Status score=` × 100 clamped to 0..1000; 0 = unknown |
| `subject`, `body_text`, `body_html` | decoded; text derived from HTML when there is no `text/plain`; bounded (1 MiB each) with a note appended when truncated |
| `attachments` | files, inline images (`content_id` kept), `message/rfc822`, calendar parts; at most 64, remainder noted in the body |
| `labels` | `["external"]` |
| `created_at_ms` | the `Date` header unless it is in the future; else arrival time |
| `importance` | from `Importance:` / `X-Priority:` |

Lost: `Received:` chains, list headers, the original `To`/`Cc` lines for
addresses outside our domain (there is no `MailAddress` for them; v1
limitation, §9).

### 3.2 Threading across the bridge

HashMail ids are 16 random bytes; Internet ids are `Message-ID` strings.
The gateway **derives** `blake3(normalised header)[..16]` and remembers each
pairing in `thread_map(hashgram_id, message_id_header, thread_id)`:

* The same Internet message delivered twice yields the same id → the SDK's
  dedup files it once.
* An Internet reply with `In-Reply-To: <x@example.com>` maps to the same id
  we filed `<x@example.com>` under, even without the map row.
* Ids we mint for **outbound** mail (`<hex(id)@domain>`) are parsed back
  directly, and the map row also gives the native `thread_id`, so a reply
  from the Internet lands in the user's existing thread.
* A native reply to an Internet message goes out with `In-Reply-To:` set to
  the **original** `Message-ID` (looked up in the map), so the remote MUA
  threads it correctly. This is why the map must persist: those original
  ids cannot be re-derived from a 16-byte value.

---

## 4. Outbound: HashMail → Internet (the `ext-to:` convention)

A Hashgram user cannot put `bob@example.com` into a `MailAddress` (only
chain identities fit). v1 convention, which the desktop composer applies when
a typed recipient parses as `AddressForm::External`:

> Send an ordinary native message **to the bridge identity** and add one
> label per external recipient: `ext-to:bob@example.com` (To) or
> `ext-cc:carol@example.org` (Cc). Subject, bodies, attachments, reply
> threading are the normal fields.

Users find the bridge identity by its username (the operator claims one,
e.g. `gateway`, and publishes it with the domain). The worker, after each
`sync().round()`:

1. Lists the bridge's `inbox` and `requests` folders newer than a persisted
   cursor (unknown senders land in Requests by the spam policy, so both are
   scanned).
2. For each new message with `ext-to:`/`ext-cc:` labels: policy
   (`allow_outbound_from_contacts_only`), per-user hourly rate
   (`outbound.per_user_per_hour`, persisted in `rate_events`), sender username
   (re-resolved on chain; no username → bounce), recipient validation (an
   `ext-to:` under our own domain is refused: use native mail), attachment
   decryption (`attachment_bytes`; transient failures retry up to 20 rounds
   then bounce), render, DKIM-sign, size check, one queue job per recipient
   domain, `outbound` log row, archive the message in the bridge mailbox.
3. Delivers each due job: `outbound.smarthost` if set, else MX lookup
   (preference order, null MX → immediate bounce, no MX → implicit MX);
   up to 5 hosts tried; opportunistic STARTTLS; `5xx` → bounce; `4xx` / I/O →
   backoff; exhausted attempts → bounce. Partially accepted recipient lists
   bounce only the refused ones.

Rendered message: `From: "<display name>" <username@domain>`, `To`/`Cc` from
labels, `Subject` (RFC 2047 when non-ASCII), `Date`, `Message-ID:
<hex@domain>`, `In-Reply-To`/`References` via the thread map,
`Importance`/`X-Priority`, `X-Hashgram-Origin: native`, `MIME-Version`, then
`text/plain` (quoted-printable), `multipart/alternative` when HTML exists,
`multipart/mixed` when attachments exist (base64, RFC 2231 file names,
`Content-ID` for inline images). The envelope `MAIL FROM` is
`username@domain`, so remote bounces come back through the inbound path to
the user.

**Bounces** are native HashMail from the bridge to the sender, subject
`Undeliverable: <subject>`, threaded onto the original.

---

## 5. What the gateway can see — trust model

| Data | Gateway operator | Everyone downstream on the Internet |
| --- | --- | --- |
| Full plaintext of bridged mail, both directions | **yes** | yes (it is ordinary email) |
| SPF/DKIM/DMARC results and spam score of inbound mail | yes (recorded into `ExternalMailMeta`) | – |
| Mapping username ↔ external address | yes | yes (it is the address) |
| Native HashMail between Hashgram identities | **no** — never routed through a gateway | – |
| Any user's root/device keys | **no** | – |

A malicious operator can read, alter, delay or fabricate *bridged* mail and
forge `auth_results`. It cannot read native mail, sign as a user, or add
devices to anyone's identity. Mitigations that are not optional: clients
label `MAIL_ORIGIN_EXTERNAL_GATEWAY`; `hashgram_app::spam` treats gateway
verdicts as advisory; users choose which gateway (if any) they correspond
through by choosing whom they add as a contact and which domain they hand
out; anyone may run one (§8).

Native mail **only** touches a gateway when the user addresses it there
(`ext-to:`). A message from Alice to Bob, both Hashgram users, never leaves
their MLS group.

Header injection: a sender on the Internet can add a fake
`Authentication-Results:` header. The fronting MTA **must** strip incoming
`Authentication-Results` headers claiming its own authserv-id before adding
its own (Postfix: `header_checks`; rspamd/OpenDKIM do this by default). The
gateway reads the first such header; it does not verify SPF/DKIM itself (§9).

Logging follows `LOGGING_POLICY.md`: envelope metadata (sizes, recipient
counts, auth results, outcomes) at `info`; addresses at `debug` only; never
subjects, bodies or file names; metric labels are closed enumerations.

---

## 6. Configuration reference (`gateway.toml`)

Unknown keys are rejected. Passphrase: environment variable
`HASHGRAM_GATEWAY_PASSPHRASE` (never in the file).

| Key | Default | Meaning |
| --- | --- | --- |
| `hashgram.home` | required | Directory of the bridge identity (`keystore.json`, `local.redb`, `peerstore.json`) |
| `hashgram.network` | required | `"mainnet"` or `"devnet"` |
| `hashgram.genesis_hash` | compiled-in on mainnet | 64-hex genesis pin; required on devnet |
| `hashgram.chain_api` | none (P2P relay) | Optional REST chain API URL |
| `hashgram.bootstrap` | compiled-in mainnet list | Multiaddrs |
| `hashgram.connect_wait_secs` | 10 | Wait for a verified peer at start |
| `hashgram.sync_interval_secs` | 5 | Worker round interval |
| `hashgram.light_kdf` | false | Argon2 light cost — devnet only |
| `smtp.listen` | `127.0.0.1:2525` | Listener; ports < 1024 refused unless `allow_privileged` |
| `smtp.allow_privileged` | false | Permit a privileged port |
| `smtp.hostname` | `domain.name` | Banner / `EHLO` name |
| `smtp.max_message_bytes` | 26214400 (25 MiB) | `DATA` limit, announced as `SIZE` |
| `smtp.connections_per_ip_per_minute` | 60 | 0 = unlimited |
| `smtp.messages_per_ip_per_hour` | 300 | 0 = unlimited |
| `smtp.max_recipients` | 50 | Per transaction (≤ 100) |
| `smtp.idle_timeout_secs` | 300 | Per connection |
| `smtp.max_connections` | 200 | Concurrent |
| `domain.name` | required | The mail domain whose local parts are usernames |
| `domain.dkim_selector` | none | With `dkim_private_key_file`, enables signing |
| `domain.dkim_private_key_file` | none | PKCS#8/PKCS#1 PEM (RSA) or Ed25519 seed (PEM, hex, base64) |
| `policy.require_spf_or_dkim` | false | Reject inbound without `spf=pass` or `dkim=pass` |
| `policy.reject_spam_score_over` | 0 (off) | Reject inbound with score above this (0..1000) |
| `policy.allow_outbound_from_contacts_only` | false | Relay only for the bridge's contacts |
| `store.sqlite_path` | required | SQLite file |
| `http.listen` | `127.0.0.1:9725` | `/healthz`, `/metrics`; must be loopback; `""` disables |
| `outbound.per_user_per_hour` | 100 | Outbound messages per Hashgram user per hour |
| `outbound.require_tls` | false | Refuse plaintext delivery |
| `outbound.smtp_timeout_secs` | 60 | Per SMTP step |
| `outbound.smtp_port` | 25 | Remote port |
| `outbound.max_attempts` | 12 | Then bounce (≈ 2 days with the backoff curve) |
| `outbound.smarthost` | none | `host:port` relay instead of MX lookup |

`hashgram-mail-gateway --config gateway.toml check-config` validates and
prints a summary (never secrets).

---

## 7. DNS setup

`hashgram-mail-gateway dns --mx-host mx.hashgram.io --ip4 <public ip>` prints
these with the real DKIM public key:

```
hashgram.io.               3600 IN MX  10 mx.hashgram.io.
hashgram.io.               3600 IN TXT "v=spf1 ip4:203.0.113.10 -all"
s1._domainkey.hashgram.io. 3600 IN TXT "v=DKIM1; k=rsa; p=MIIBIjANBg…"
_dmarc.hashgram.io.        3600 IN TXT "v=DMARC1; p=quarantine; rua=mailto:postmaster@hashgram.io; adkim=s; aspf=s"
```

* **MX** points at the host that terminates port 25 (the fronting MTA).
* **SPF** lists every IP that sends outbound for the domain (the gateway
  host, or the smarthost provider's `include:`). `-all` once you are sure.
* **DKIM** key generation: `openssl genrsa 2048 | openssl pkcs8 -topk8
  -nocrypt > dkim-s1.pem` (RSA, universally verified) or `openssl genpkey
  -algorithm ed25519 > dkim-e1.pem` (RFC 8463; publish alongside RSA until
  receivers catch up). Rotate by adding a new selector, switching the
  config, and removing the old TXT after a week. Long TXT values are split
  into 250-byte strings by the `dns` command.
* **DMARC** starts at `p=none` while you watch the reports, then
  `quarantine`, then `reject`. `postmaster@<domain>` must map to a real
  Hashgram user (§9) or an external mailbox at the fronting MTA.
* **PTR/rDNS** of the sending IP must resolve to the `EHLO` hostname
  (`smtp.hostname`), or large receivers grey-list you.

---

## 8. Deployment

**Not on a validator.** The gateway is Internet-facing and parses untrusted
input; run it on its own small host.

1. Build: `cargo build --release -p hashgram-mail-gateway` (from `node/`).
   `--no-default-features` drops RSA DKIM (the `rsa` crate) and leaves
   ed25519 only.
2. Fronting MTA on :25 with TLS, SPF/DKIM/DMARC verification and
   `Authentication-Results`, relaying `*@hashgram.io` to
   `127.0.0.1:2525`. Postfix sketch:
   ```
   relay_domains = hashgram.io
   transport_maps = inline:{ hashgram.io=smtp:[127.0.0.1]:2525 }
   smtpd_tls_security_level = may
   smtpd_milters = inet:localhost:11332   # rspamd: adds Authentication-Results + X-Spam-Score
   ```
   Alternatively any TLS-terminating TCP proxy; the gateway then records
   `spf=none dkim=none` and `[policy].require_spf_or_dkim` cannot be used.
3. `hashgram-mail-gateway --config gateway.toml init` — creates the vault,
   prints the address and (once) the mnemonic. Store the mnemonic offline;
   it *is* the bridge identity.
4. Fund the address; `hashgram-mail-gateway register`; claim a username for
   it (`hashgram-client` username commands), e.g. `gateway`. Publish
   "@gateway is the hashgram.io mail bridge" so users can add it as a
   contact and the desktop can pre-fill it.
5. Publish DNS (§7). Wait for propagation.
6. Install `deploy/hashgram-mail-gateway.service` (edit paths), a
   `passphrase.env` with `HASHGRAM_GATEWAY_PASSPHRASE=…` (mode 0640), the
   DKIM key (0640), `gateway.toml`. `systemctl enable --now`.
7. Verify: `curl -s 127.0.0.1:9725/healthz` → `{"ok":true,…}` once the
   first sync round succeeds; send yourself a message from an external
   account; send an `ext-to:` message from a Hashgram client and check it
   arrives with `DKIM-Signature` and `dkim=pass` at the receiver.

### 8.1 Running your own gateway under your own domain

Nothing in the protocol names `hashgram.io`. Set `[domain] name =
"mail.example.org"`, create your own bridge identity, publish MX/SPF/DKIM/
DMARC for `mail.example.org`, and tell your users to add *your* bridge
username as a contact. `alice@mail.example.org` then maps to on-chain
username `alice` exactly as `alice@hashgram.io` does — usernames are global,
domains are just which operator you trust with your Internet mail. Two
gateways under two domains can serve the same username; the user decides
which address to hand out. The only hard-coded domain in the tree is the
display form `hashgram_app::mail::MAIL_DOMAIN` that clients show; a client
configured for another operator's gateway should show that operator's domain.

---

## 9. Limitations (v1, honest list)

* **No STARTTLS or AUTH on the inbound listener.** TLS is terminated by the
  fronting MTA/proxy. Nobody submits mail through this gateway with a
  password; outbound is native mail to the bridge identity. Both commands
  answer `502`.
* **Outbound TLS is opportunistic** (`require_tls = false` by default): a
  MX without STARTTLS, or with a certificate that does not verify, gets
  plaintext, exactly as other MTAs do. Set `require_tls = true` to bounce
  instead. No DANE/MTA-STS yet.
* **No SPF/DKIM/DMARC verification in the gateway itself**; it records what
  the fronting MTA wrote into `Authentication-Results`. Without a fronting
  MTA every message is `spf=none dkim=none`.
* **Single operator identity**: every inbound message's MLS sender is the
  bridge. Clients show the external sender from `from.display_name` and
  `external.from_header` and label the origin. If a user blocks the bridge,
  they block all Internet mail.
* **Original `To`/`Cc` for addresses outside our domain are not preserved**
  (no `MailAddress` can hold them). Reply-all from HashMail to a mixed
  Internet thread therefore reaches only the sender.
* **`postmaster@`, `abuse@` etc.** are ordinary usernames here; register
  them or handle them at the fronting MTA (RFC 5321 §4.5.1 requires
  `postmaster` to be deliverable).
* **Bounces for inbound mail** we could not deliver into HashMail are not
  sent back to the Internet sender (we log `failed`); the fronting MTA's
  queue already told the sender we accepted it. A future version should
  generate DSNs.
* **One SQLite file, one process.** No horizontal scaling; the SDK facade
  is single-threaded behind a mutex. Adequate for tens of thousands of
  messages a day, not millions.
* **Ed25519 DKIM** is implemented but not yet verified by all large
  receivers; publish RSA.

---

## 10. Runbook

**Health.** `GET /healthz` is `200 {"ok":true}` after the first successful
sync round and `503` before / when the network is unreachable. Alert on
`hashgram_mail_gateway_connected == 0` for > 5 min and on
`hashgram_mail_gateway_queue_depth{kind="inbound"}` growing.

**Metrics** (`/metrics`, OpenMetrics):
`messages_total{direction,outcome}` (`accepted|rejected|delivered|deferred|failed|bounced`),
`smtp_messages_offered_total`, `sync_rounds_total`, `sync_failures_total`,
`queue_depth{kind}`, `connected`, `last_sync_ok_seconds`.

**Logs.** `RUST_LOG=info` for envelope-level events, `debug` adds addresses
and per-MX attempts. Bodies and subjects are never logged at any level.

**Inbound mail not arriving.**
1. `journalctl -u hashgram-mail-gateway | grep -i "inbound message queued"` —
   if absent, the fronting MTA is not relaying: check its transport map and
   that the gateway answered `250` at `RCPT`.
2. `550 5.1.1 no such user` — the username has no identity with devices on
   chain (`hashgram-client` lookup). `451 4.4.3` — the gateway cannot reach
   a chain relay; check `connected` and bootstrap peers.
3. Queued but `deferred` — `sqlite3 gateway.sqlite 'select id,attempts,
   next_attempt_at,last_error from queue where kind="inbound"'`. Typical
   `last_error`: `NoPeer` (no store node), `NoKeyPackage` (recipient device
   has not published a key package — their client must sync once).

**Outbound mail not arriving.**
1. Is the message in the bridge mailbox at all? The user must have sent *to
   the bridge identity* with an `ext-to:` label; the log shows
   `outbound message queued` / `outbound message refused`.
2. `refused` with a bounce: no username, rate limit, contacts-only policy,
   `ext-to:` under our own domain.
3. `deferred`: `select … from queue where kind="outbound"`; `last_error`
   shows the MX reply. `4xx greylisted` is normal for a first contact.
4. `failed` with `5.7.x`: DKIM/SPF/DMARC alignment. Check `dns` output
   against what is published, the sending IP against SPF, rDNS.

**Rotate the DKIM key.** Generate a new key with a new selector, publish the
TXT, change `[domain]`, `systemctl restart`, remove the old TXT a week later.

**Rotate the bridge identity's vault passphrase.** Not supported in place;
create a new identity (`init` into a new `home`), register, claim a new
username, announce it. Messages in the old mailbox stay readable with the old
vault.

**Schema upgrades.** Embedded migrations run at start; a newer binary
upgrades in place, an older binary refuses a newer file. Back up
`gateway.sqlite` (it contains the queue and the thread map) with
`sqlite3 gateway.sqlite '.backup gateway.bak'`.

**Emergency stop.** `systemctl stop hashgram-mail-gateway`: the listener
stops accepting, in-flight `DATA` is either committed to the queue with
`250` or not (the sender retries), the worker saves MLS state and exits.
Nothing is lost that was acknowledged.

---

## 11. Tests

`cargo test -p hashgram-mail-gateway` (no network):

* `smtp::server` — happy path with dot-stuffing, null sender and source
  routes, bad sequences (`503/500/501/502/555`), size limits (`552`),
  recipient limits (`452`) and rejections, consecutive-error `421`, path
  parsing, driver over an in-memory duplex with pipelining, real TCP
  listener with per-IP limit and graceful stop.
* `smtp::client` — full transaction against a scripted MX (dot-stuffing,
  `SIZE`, `BODY=8BITMIME`, partial recipient rejection), 4xx/5xx/`SIZE`
  classification, `require_tls`, extension parsing, TLS connector build.
* `mime` — encoded-words round trip through `mailparse`, display names,
  quoted-printable known answers, base64 folding, RFC 5322 dates
  (1970-01-01, 2000-02-29, 2026-09-12), file-name sanitising.
* `mime::inbound` — plain message with all threading/auth/spam/priority
  headers, `multipart/mixed(alternative, pdf, inline png)`, HTML-only →
  text, `Authentication-Results` and spam parsing, garbage headers, subject
  folding, attachment limit note.
* `mime::outbound` — round trip through `mailparse` and our own inbound
  parser (structure, RFC 2231 names, `Content-ID`, importance), plain-only
  and alternative-only, determinism, line-length/CRLF checks.
* `mime::html` — conversion, garbage, tables/whitespace, attributes.
* `dkim` — RFC 6376 §3.4.5 relaxed known answer, body edge cases and the
  empty-body hash, header unfolding, ed25519 sign + independent verify +
  tamper detection, key formats, RSA sign + verify (feature `dkim-rsa`).
* `thread` — stable derivation with known answer, references splitting,
  `<hex@domain>` round trip.
* `bridge` — thread ids derive/remember/reply-to-ours/minted, outbound
  headers map back to original `Message-ID`s, inbound `MailMessage` is
  external and passes `hashgram_app::mail::validate`, BCC placement,
  `ext-to:` extraction and refusals, outbound build via the thread map with
  a later Internet reply landing in the same thread, backoff.
* `store`, `policy`, `address`, `ratelimit`, `dns`, `config`, `metrics`.
