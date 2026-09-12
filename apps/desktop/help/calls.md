# Calls

## How a call is set up

1. The app discovers nodes with the `call` role from their signed
   announcements on the network.
2. It fetches short-lived **TURN** credentials from one of them (signed
   request from your device key; the credentials last one hour).
3. The offer, answer and ICE candidates travel inside the **encrypted
   chat** (MLS) between you and the other person — never through a
   signalling server.
4. Media is WebRTC. The TURN node only relays encrypted packets it cannot
   read.

## 1:1 calls

Audio, video and screen share, end-to-end encrypted (DTLS-SRTP, keyed as
the protocol specifies).

## Group calls

Group calls need an SFU node to be announced on the network. When none is,
the group-call button is disabled and says so. **Group calls hosted by an
SFU are not end-to-end encrypted against the SFU operator.** The app tells
you this before you join.

## If a call fails

- No `call` node reachable: nobody is offering TURN right now.
- Behind a strict NAT: TURN relays should still work; a firewall blocking
  UDP will not.
