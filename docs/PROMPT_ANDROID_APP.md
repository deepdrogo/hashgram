# Build Prompt: Hashgram Android Application

A specification you can hand to an engineer or a coding model to build the
Hashgram Android client.

**Every API here exists.** Endpoint paths came from
`proto/hashgram/*/v1/query.proto`, transaction types from `tx.proto`. Nothing
is invented.

Read [PROMPT_WINDOWS_DESKTOP.md](PROMPT_WINDOWS_DESKTOP.md) first. It contains
the complete API surface, the transaction list and the traps, and this document
does not repeat them. What follows is what differs on Android.

---

## 0. What not to build

**There is no messaging, no social feed, no media and no calls.** The
peer-to-peer layer is Phase 2 and unbuilt: no end-to-end encryption, no
envelope store, no blob storage, no call signalling.

Build a wallet and an identity manager. Do not build a chat tab against an
endpoint that does not exist.

---

## 1. Platform

| | |
| --- | --- |
| Minimum | API 29 (Android 10) |
| Target | The current API level |
| Language | Kotlin |
| UI | Jetpack Compose |
| Networking | OkHttp and Retrofit, or `grpc-kotlin` if you generate a gRPC client |
| Crypto | Tink for symmetric work, plus a maintained secp256k1 binding |

API 29 rather than lower, because that is where hardware-backed keystore
attestation and scoped storage can both be relied on. Supporting API 24 means
supporting devices where key material sits in a file the user can copy.

For secp256k1 use `kethereum` or a JNI binding to libsecp256k1. Do **not**
substitute a curve the platform happens to provide: the key type is part of
the protocol and a P-256 key produces an address nobody can pay.

---

## 2. Key storage

Android's story is more fragmented than iOS's. The important thing is knowing
which guarantee you actually have on the device in front of you.

### Hardware-backed keystore, with a check

`AndroidKeyStore` may be backed by a Trusted Execution Environment, by a
StrongBox secure element, or by software only — and the API does not make you
notice which. Query it and treat the answer as security-relevant:

```kotlin
val info = KeyFactory.getInstance(key.algorithm, "AndroidKeyStore")
    .getKeySpec(key, KeyInfo::class.java)

val level = when {
    // API 31+
    info.securityLevel == KeyProperties.SECURITY_LEVEL_STRONGBOX -> "StrongBox"
    info.securityLevel == KeyProperties.SECURITY_LEVEL_TRUSTED_ENVIRONMENT -> "TEE"
    else -> "software"
}
```

**Tell the user which one they have.** A wallet on a software-keystore device
is meaningfully less protected, and the honest thing is to say so rather than
present the same lock icon everywhere.

The keystore does not hold secp256k1, so as on iOS it holds the key that
encrypts the account key. That moves the extractable secret from "in a file"
to "in a file, unusable without the keystore".

Require these on the wrapping key:

```kotlin
KeyGenParameterSpec.Builder(alias, PURPOSE_ENCRYPT or PURPOSE_DECRYPT)
    .setUserAuthenticationRequired(true)
    .setInvalidatedByBiometricEnrollment(true)   // new fingerprint invalidates
    .setIsStrongBoxBacked(true)                  // fall back if unavailable
    .build()
```

`setInvalidatedByBiometricEnrollment(true)` matters: without it, an attacker
who coerces a screen lock and enrols their own fingerprint gains signing
ability.

### The mnemonic

`EncryptedSharedPreferences` (Jetpack Security) over a keystore-held master
key, or do not store it and require re-entry.

**Never** plain `SharedPreferences`, never a file in external storage, never
Room without encryption.

### Backup

**Exclude every key artefact from Android Backup and from Auto Backup:**

```xml
<application
    android:allowBackup="false"
    android:dataExtractionRules="@xml/data_extraction_rules">
```

A seed phrase in Google Drive backup is a seed phrase protected by a Google
account password, which is a materially weaker guarantee than the user expects
from a wallet.

---

## 3. What differs from desktop

Same screens as the desktop client, minus the node console.

Four destinations: **Wallet**, **Identity**, **Stake**, **About**. Founder
verification and supply belong under About.

### Biometric gating

`BiometricPrompt` before:

- Revealing the mnemonic, if stored
- Signing any transaction
- Changing recovery guardians

Not before reading a balance. Gating read-only screens trains reflexive
authentication, which is the habit that makes a coerced unlock easy.

### Screenshots and the recents screen

```kotlin
window.setFlags(
    WindowManager.LayoutParams.FLAG_SECURE,
    WindowManager.LayoutParams.FLAG_SECURE
)
```

`FLAG_SECURE` on any screen showing a mnemonic or a balance. It blocks
screenshots, blocks screen recording, and blanks the recents thumbnail —
which is otherwise written to disk.

Apply it per-activity or per-dialog rather than app-wide, so users can still
screenshot a transaction result for a support request.

### Clipboard

Android 13 and later shows a clipboard preview, which will render a copied
address on screen. Mark sensitive copies:

```kotlin
val clip = ClipData.newPlainText("", address).apply {
    description.extras = PersistableBundle().apply {
        putBoolean(ClipDescription.EXTRA_IS_SENSITIVE, true)
    }
}
```

Never copy a mnemonic to the clipboard at all.

### Root and integrity

Check for a rooted device and for an unlocked bootloader, and **warn without
blocking**. Blocking rooted devices punishes a small population of
knowledgeable users and is bypassed by the attackers it targets; a clear
warning respects both.

Use Play Integrity API if you ship through Play, and treat a failed verdict as
a reason to warn rather than to refuse. A wallet that refuses to open on a
device the user owns is a wallet they will replace with a worse one.

---

## 4. Connectivity

The phone will not run a node. It connects to a public RPC endpoint over
HTTPS.

**Design for endpoint choice from the first release.** Ship a list, let the
user add their own, show health per endpoint, and never hard-code a single
hostname: that would make one operator load-bearing for every user.

Pin certificates with OkHttp's `CertificatePinner` for shipped defaults.

Handle offline properly and show the last-updated time when not connected. A
wallet showing a stale balance without saying so causes users to send funds
they do not have.

Use `WorkManager` for background refresh, and expect Doze to defer it. Do not
build anything that assumes a background task ran on time.

---

## 5. Play Store

**Financial services policy.** A self-custody wallet is permitted. Read the
current crypto policy before adding anything that looks like an exchange or a
custodial service, because those have additional requirements including, in
some regions, licensing.

**Describe it accurately.** If the listing says "decentralised messenger" and
there is no messaging, that is a policy problem and a user-trust problem. Say
"self-custody wallet and identity manager for the Hashgram network".

**Data safety form.** Answer it accurately. The app stores keys locally, does
not transmit them, and should not collect analytics on transaction contents.

**Target API level** requirements move every year. Budget for it.

---

## 6. Deliberately out of scope

- Messaging
- Social feed, posts, reels, stories
- Media upload or playback
- Voice and video calls
- Running a node
- Validator operations

---

## 7. Definition of done

Everything in the desktop prompt's checklist that applies, plus:

- [ ] Keystore security level queried and **shown to the user**
- [ ] StrongBox requested, with a graceful fallback
- [ ] `setUserAuthenticationRequired(true)` and
      `setInvalidatedByBiometricEnrollment(true)` on the wrapping key
- [ ] Mnemonic in `EncryptedSharedPreferences`, or not stored at all
- [ ] `allowBackup="false"` and key artefacts excluded from Auto Backup
- [ ] `BiometricPrompt` before signing, before revealing a mnemonic, and
      before changing guardians — and **not** before reading a balance
- [ ] `FLAG_SECURE` on mnemonic and balance screens
- [ ] `EXTRA_IS_SENSITIVE` on copied addresses; mnemonic never copied
- [ ] Root and bootloader state warned about, not blocked
- [ ] Endpoint list configurable, with per-endpoint health, no single
      hard-coded host
- [ ] `CertificatePinner` on shipped default endpoints
- [ ] Stale-data indicator when offline
- [ ] Amounts parsed as `BigInteger` or `BigDecimal`, never `Double`
- [ ] A devnet build is unmistakable at a glance
- [ ] Play listing does not claim messaging
- [ ] No messaging, social or media features
