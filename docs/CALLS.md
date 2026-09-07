# Calls

Voice and video calls on Hashgram are WebRTC between devices. The network's
part is small and deliberate: help devices find call infrastructure without
a fixed hostname, hand out time-limited TURN credentials, and carry
signalling inside the same end-to-end encrypted channel as messages. Media
never touches the chain, and never touches a node in a form the node can
read.

## Components

| Component | Where | What it does |
| --- | --- | --- |
| WebRTC stack | the application | captures, encodes, encrypts (DTLS-SRTP), and negotiates |
| Signalling | MLS group of the conversation | `CallSignal` messages: offer, answer, ICE, hangup, ring, busy |
| TURN | coturn on `call` nodes | relays encrypted media when a direct path fails |
| SFU (optional) | LiveKit on `call` nodes | group calls and live streams; terminates media for its rooms |
| Discovery | signed `NodeAnnounce` | TURN URIs, realm, SFU URL under the `call` role |

## Discovery

A node serving the `call` role announces:

```protobuf
message TurnInfo { repeated string uris = 1; string realm = 2; bool issues_credentials = 3; }
message SfuInfo  { string url = 1; string kind = 2; }   // kind = "livekit"
```

inside its `NodeAnnounce`, signed with its node key and expiring after an
hour. A client lists them with `AnnounceQuery{roles:["call"]}`
(`hashgram-client call discover`). Because the announcement carries the
operator address, a client can check the provider is registered and bonded
on chain before relying on it.

## TURN credentials

coturn runs in `use-auth-secret` mode (RFC 8489 long-term credentials with
a shared secret). The node holds the secret (`/etc/hashgram/turn.secret`,
root:hashgram-node 0640) and issues credentials to a device that proves it
holds its key:

```text
client ── TurnCredentialRequest{device_pubkey, timestamp, signature} ──▶ call node
       ◀── TurnCredentialResult{username = "<expiry>:<label>", password = base64(HMAC-SHA1(secret, username)), uris}
```

The signature is the device's `mailbox-fetch` signature over an empty
cursor and limit 0 with a fresh timestamp (it proves the same thing —
"I hold this key now" — without a new signing domain). Credentials expire
after one hour; the HMAC was checked against `openssl dgst -sha1 -hmac`.
`hashgram-client call turn` fetches a credential; the SDK returns it in the
`IceServer` shape WebRTC stacks take.

## Signalling

Offers, answers and ICE candidates are `chat.CallSignal` messages sent with
`Messaging::send_call_signal` into the conversation's MLS group:

```protobuf
message CallSignal {
  string kind = 1;        // offer | answer | ice | hangup | busy | ring
  bytes  call_id = 2;     // random 16 bytes
  string sdp = 3;         string candidate = 4;  string sdp_mid = 5;  uint32 sdp_mline_index = 6;
  bool   video = 7;
}
```

Only group members can read that a call is happening; store nodes see an
envelope like any other. A receiving client gets the signal from `sync` as a
`CALL` message with the `call` field populated and hands it to its WebRTC
stack.

## 1:1 calls

1. Caller creates a `call_id`, gets TURN credentials, builds an SDP offer
   with the TURN server in its ICE configuration, sends `offer` (+ `ring`).
2. Callee gets its own TURN credentials, answers with `answer`, both sides
   exchange `ice` candidates through the group.
3. Media flows directly or through TURN, encrypted end to end by WebRTC.
4. Either side sends `hangup`.

## Group calls

Group calls use the SFU. The `call` node runs LiveKit
(`scripts/install/livekit.sh`); the node mints room tokens (it can read the
LiveKit API key). An SFU terminates media, so it sees decrypted audio and
video for the rooms it hosts — the same trust level as any SFU anywhere.
Clients should show which node hosts a group call, and applications that
need end-to-end encrypted group calls should use WebRTC insertable frames
on top (not implemented in the reference client).

## Rewards

Call nodes are paid on client-signed session receipts (`SERVICE_ROLE_CALL`,
units = seconds; `docs/SERVICE_REWARDS.md`). The reference client does not
yet sign call receipts; the SDK exposes `Link::deliver_receipt` for
applications that measure sessions.

## Operator setup

```text
sudo scripts/install/coturn.sh --realm calls.example.org [--external-ip 203.0.113.5]
sudo scripts/install/livekit.sh --domain sfu.example.org          # optional SFU
hashgramctl configure-role <existing>,call --restart
hashgramctl health                                                 # checks coturn and the TURN announcement
hashgram-client call discover                                      # from any device
```

Ports: 3478 TCP/UDP, 5349 TCP/UDP (TURN), 49152–65535 UDP (relay); LiveKit
7881 TCP and 50000–60000 UDP, with a TLS reverse proxy for 7880. Media is
bandwidth; a call node's `MemoryMax` and network are the operator's
capacity planning.

## Limitations

- No push notification for incoming calls: the callee must be polling
  (`message receive --watch`) or connected. Applications integrate their
  platform's push using an encrypted, content-free wake-up.
- The reference client does not include a WebRTC stack; it proves discovery,
  credentials and signalling. Each application uses its platform's stack.
- Group calls through the SFU are not end-to-end encrypted against the SFU
  operator.
