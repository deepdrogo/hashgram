# Build Prompt: Hashgram iOS Application

A specification you can hand to an engineer or a coding model to build the
Hashgram iOS client.

**Every API here exists.** Endpoint paths came from
`proto/hashgram/*/v1/query.proto`, transaction types from `tx.proto`. Nothing
is invented.

Read [PROMPT_WINDOWS_DESKTOP.md](PROMPT_WINDOWS_DESKTOP.md) first. It contains
the complete API surface, the transaction list and the traps, and this document
does not repeat them. What follows is what differs on iOS.

---

## 0. Build in two stages

**Stage 1: a wallet and an identity manager.** Everything below is about
stage 1 and has real chain APIs behind it. Ship it first.

**Stage 2: messenger, feed, media, calls.** The network layer exists —
MLS end-to-end encrypted messaging, signed social events, content-addressed
media, TURN credentials and call signalling — and is exposed by the Rust
`hashgram-sdk` (`sdk/rust/hashgram-sdk`). Bind it with UniFFI; do not
reimplement MLS or the transport. The desktop prompt §7 lists the screen
to SDK call mapping, and `node/hashgram-client` is the reference program
to copy. The SDK does not include a WebRTC media stack or push
notifications; the app brings those.

Do not build a chat tab against an HTTP endpoint. Messaging is peer-to-peer
through the SDK; a REST path such as `/hashgram/messaging/v1/send` does not
exist and any specification naming one is wrong.

This matters more on iOS than elsewhere: App Review will ask what the app
does, and a stage 1 build whose main tab is a non-functional messenger is a
rejection. Ship stage 1 as a wallet; add the messenger when stage 2 works
end to end against a live network.

---

## 1. Platform

| | |
| --- | --- |
| Minimum | iOS 17 |
| Language | Swift 5.9 or later |
| UI | SwiftUI |
| Networking | `URLSession`, or `grpc-swift` if you generate a gRPC client |
| Crypto | CryptoKit, plus `secp256k1.swift` for the curve CryptoKit lacks |

CryptoKit does not implement secp256k1, which is what Cosmos account keys use.
Do not attempt to substitute P-256 because CryptoKit has it: the key type is
part of the protocol and a P-256 key produces an address nobody can pay.

---

## 2. Key storage

This is the section that matters most, and iOS gives you better tools than any
other platform. Use them.

### Secure Enclave where possible

The Secure Enclave holds a key that cannot be extracted, even by an attacker
with the device unlocked and your process compromised. It does **not** support
secp256k1, so it cannot hold the account key directly — but it can hold the
key that encrypts the account key, which moves the extractable secret from
"in a file" to "in a file, unusable without the Enclave".

```swift
// Enclave-held P-256 key, used only to unwrap the account key.
let access = SecAccessControlCreateWithFlags(
    nil,
    kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
    [.privateKeyUsage, .biometryCurrentSet],
    nil
)
```

`kSecAttrAccessibleWhenUnlockedThisDeviceOnly` matters: it excludes the key
from any backup, including iCloud, so restoring a device backup onto an
attacker's phone does not carry the key with it.

`.biometryCurrentSet` matters too: it invalidates the key if a new fingerprint
or face is enrolled, so an attacker who coerces a passcode and adds their own
biometric does not gain signing ability.

### The mnemonic

Store it in the Keychain with `kSecAttrAccessibleWhenUnlockedThisDeviceOnly`
and biometric protection, or do not store it at all and require re-entry.

**Never** with `kSecAttrAccessibleAlways`, and **never** in
`UserDefaults`, a file in `Documents`, or Core Data.

### iCloud

**Exclude every key artefact from iCloud backup and from iCloud Keychain.** A
seed phrase synced to iCloud is a seed phrase protected by an Apple ID
password and whatever recovery flow that account has, which is a materially
weaker guarantee than the user expects from a wallet.

Set `isExcludedFromBackup` on any container that touches key material.

---

## 3. What differs from desktop

### Screens

Same set as the desktop client, minus the node console — nobody runs a
validator from a phone, and a console screen implies they might.

Four tabs is enough: **Wallet**, **Identity**, **Stake**, **About**. Put
Founder verification and supply under About; they are transparency features
users read once, not daily tools.

### Biometric gating

Require Face ID or Touch ID before:

- Revealing the mnemonic, if you store it at all
- Signing any transaction
- Changing recovery guardians

Not before viewing a balance. Gating read-only screens trains users to
authenticate reflexively, which is exactly the habit that makes a coerced
authentication easy.

### Screenshots and the app switcher

Blur or replace the view when the app backgrounds, on any screen showing a
mnemonic or a balance:

```swift
// In the scene delegate, on willResignActive.
```

iOS screenshots the app for the switcher, and that image is written to disk.

Detect screenshots on the mnemonic screen with
`UIApplication.userDidTakeScreenshotNotification` and warn — you cannot
prevent it, but a user who screenshots their seed phrase into their photo
library, which syncs to iCloud, should be told what they just did.

### Pasteboard

If you copy an address, set an expiry:

```swift
UIPasteboard.general.setItems(
    [[UTType.utf8PlainText.identifier: address]],
    options: [.expirationDate: Date().addingTimeInterval(60)]
)
```

Never copy a mnemonic to the pasteboard. Universal Clipboard means it leaves
the device.

---

## 4. Connectivity

The phone will not run a node. It connects to a public RPC endpoint over
HTTPS.

**Design for endpoint choice from the first release.** Ship a list, let the
user add their own, show health per endpoint, and never hard-code a single
hostname: that would make one operator load-bearing for every user, which is
the opposite of what this network is for.

Pin certificates for any endpoint you ship as a default.

Handle offline properly. A wallet that shows a stale balance without saying it
is stale causes users to send funds they do not have and then not understand
the rejection. Show the last-updated time whenever the app is not connected.

---

## 5. App Review

Practical notes, because a rejection costs a week.

**Do not describe the app as a "cryptocurrency exchange"** in App Store
Connect. It is a self-custody wallet. Exchange functionality invites a
different and much harder review.

**Guideline 3.1.1**, in-app purchase: a self-custody wallet that does not sell
anything is fine. Do not add a "buy HASH" button routed through a third party
without reading the current rules first.

**Provide a demo account or a devnet build** for the reviewer. A wallet with no
funds looks broken, and a reviewer who cannot see the app work rejects it.

**Explain what the app does not do.** If your description says "decentralised
messenger" and there is no messaging, that is a rejection for a misleading
description. Say "self-custody wallet and identity manager for the Hashgram
network".

**Export compliance.** The app uses cryptography. Answer the encryption
questions accurately; standard cryptography for authentication and signing
generally qualifies for an exemption, but answer it rather than guessing.

---

## 6. Deliberately out of scope

Stage 1: messaging, social feed, reels, stories, media, calls (see §0).
Always: running a node, validator operations, a token bridge.

Stage 2 additions on iOS: the vault lives in the app sandbox with
`NSFileProtectionComplete`, is excluded from iCloud backup, and background
mailbox polling uses `BGAppRefreshTask` — there is no push service.
Calls need `CallKit` and `PushKit` only if you run your own VoIP push
relay; Hashgram does not provide one, so document that incoming calls ring
only while the app is foregrounded or refreshing.

---

## 7. Definition of done

Everything in the desktop prompt's checklist that applies, plus:

- [ ] Account key encrypted by a Secure Enclave key with
      `.biometryCurrentSet`
- [ ] `kSecAttrAccessibleWhenUnlockedThisDeviceOnly` on every key item
- [ ] Every key artefact excluded from iCloud backup and iCloud Keychain
- [ ] Biometric gate before signing, before revealing a mnemonic, and before
      changing guardians — and **not** before reading a balance
- [ ] View blurred when backgrounded on any screen showing a mnemonic or
      balance
- [ ] Screenshot detection on the mnemonic screen, with a warning
- [ ] Pasteboard expiry on addresses; mnemonic never copied
- [ ] Endpoint list configurable, with per-endpoint health, no single
      hard-coded host
- [ ] Certificate pinning on shipped default endpoints
- [ ] Stale-data indicator when offline
- [ ] Amounts parsed as `Decimal` or a big-integer type, never `Double`
- [ ] A devnet build is unmistakable at a glance
- [ ] App Store description does not claim messaging
- [ ] No messaging, social or media features
